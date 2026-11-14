//! H.264 decoder built on the Media Foundation decoder MFT.
//!
//! Input is the Annex-B byte stream carried by `VideoFrame` messages;
//! output is NV12 in system memory (what the preview and, later, the virtual
//! camera's shared memory consume).

use crate::mf;
use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVLowLatencyMode, ICodecAPI, IMF2DBuffer, IMFMediaType, IMFSample, IMFTransform,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MFT_CATEGORY_VIDEO_DECODER,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE,
};

#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// NV12, tightly packed (`stride_y == width`, `stride_uv == width`).
    pub nv12: Vec<u8>,
    pub stride_y: usize,
    pub stride_uv: usize,
    pub width: u32,
    pub height: u32,
    pub pts_us: Option<u64>,
}

pub struct H264Decoder {
    transform: IMFTransform,
    stream_info: MFT_OUTPUT_STREAM_INFO,
    width: u32,
    height: u32,
    /// Height the decoder actually writes, before cropping: H.264 codes 1080p as
    /// 1920x1088 and the chroma plane starts after those 1088 rows.
    coded_height: u32,
    configured: bool,
    streaming: bool,
}

impl H264Decoder {
    pub fn new(prefer_hardware: bool) -> Result<Self> {
        let transform = mf::create_transform(
            MFT_CATEGORY_VIDEO_DECODER,
            Some((MFMediaType_Video, MFVideoFormat_H264)),
            None,
            prefer_hardware,
        )
        .context("création du décodeur H.264")?;
        let stream_info = unsafe { transform.GetOutputStreamInfo(0) }.context("GetOutputStreamInfo")?;
        Ok(Self {
            transform,
            stream_info,
            width: 0,
            height: 0,
            coded_height: 0,
            configured: false,
            streaming: false,
        })
    }

    /// Declare the stream (from `VideoConfig`) and pick an output type.
    pub fn configure(&mut self, width: u32, height: u32, fps: u32) -> Result<()> {
        unsafe {
            let input = mf::new_media_type()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, mf::pack_size(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, mf::pack_ratio(fps.max(1), 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, mf::pack_ratio(1, 1))?;
            self.transform.SetInputType(0, &input, 0).context("SetInputType H.264")?;
        }

        self.width = width;
        self.height = height;
        self.coded_height = height;
        self.pick_output_type()?;

        // Without this, the decoder keeps a reordering queue of frames: bad for
        // glass-to-glass latency and for the "requested by the PC" Start flow.
        // The Microsoft decoder wants a VT_UI4 here even though the property is
        // documented as a BOOL, so try both shapes.
        match self.transform.cast::<ICodecAPI>() {
            Ok(codec) => {
                let mut accepted = false;
                for value in [mf::variant_u32(1), mf::variant_bool(true)] {
                    match unsafe { codec.SetValue(&CODECAPI_AVLowLatencyMode, &value) } {
                        Ok(()) => {
                            accepted = true;
                            log::debug!("CODECAPI_AVLowLatencyMode accepté (vt {})", unsafe {
                                value.Anonymous.Anonymous.vt.0
                            });
                            break;
                        }
                        Err(error) => log::debug!("CODECAPI_AVLowLatencyMode refusé: {error}"),
                    }
                }
                if !accepted {
                    log::warn!("CODECAPI_AVLowLatencyMode non appliqué — le décodeur peut retarder les frames");
                }
            }
            Err(error) => log::warn!("ICodecAPI indisponible: {error}"),
        }

        self.configured = true;
        Ok(())
    }

    /// Prefer NV12 (what we ship downstream); fall back to whatever the
    /// decoder offers first.
    fn pick_output_type(&mut self) -> Result<IMFMediaType> {
        let mut first: Option<IMFMediaType> = None;
        for index in 0..16u32 {
            let Ok(candidate) = (unsafe { self.transform.GetOutputAvailableType(0, index) }) else {
                break;
            };
            let subtype = unsafe { candidate.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
            let size = unsafe { candidate.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
            let (width, height) = mf::unpack_size(size);
            log::info!("sortie décodeur candidate {index}: {subtype:?} {width}x{height}");
            if subtype == MFVideoFormat_NV12 {
                unsafe { self.transform.SetOutputType(0, &candidate, 0) }
                    .context("SetOutputType NV12")?;
                self.stream_info = unsafe { self.transform.GetOutputStreamInfo(0)? };
                self.coded_height = height.max(self.height);
                return Ok(candidate);
            }
            if first.is_none() {
                first = Some(candidate);
            }
        }

        let Some(fallback) = first else {
            anyhow::bail!("le décodeur n'expose aucun type de sortie");
        };
        let subtype = unsafe { fallback.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
        if subtype != MFVideoFormat_NV12 {
            log::warn!("sortie décodeur non NV12 ({subtype:?}) — l'aperçu peut être incorrect");
        }
        unsafe { self.transform.SetOutputType(0, &fallback, 0) }
            .context("SetOutputType (repli)")?;
        self.stream_info = unsafe { self.transform.GetOutputStreamInfo(0)? };
        let size = unsafe { fallback.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
        self.coded_height = mf::unpack_size(size).1.max(self.height);
        Ok(fallback)
    }

    /// Decode one access unit (Annex-B); returns the frames it produced.
    pub fn feed(&mut self, access_unit: &[u8], pts_us: u64) -> Result<Vec<DecodedFrame>> {
        anyhow::ensure!(self.configured, "décodeur non configuré");
        if access_unit.is_empty() {
            return Ok(Vec::new());
        }

        if !self.streaming {
            unsafe {
                self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            }
            self.streaming = true;
        }

        unsafe {
            let buffer = MFCreateMemoryBuffer(access_unit.len() as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(access_unit.as_ptr(), pointer, access_unit.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(access_unit.len() as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((pts_us as i64).saturating_mul(10))?;
            self.transform.ProcessInput(0, &sample, 0).context("ProcessInput décodeur")?;
        }

        self.pull_frames()
    }

    /// Signal end of stream and collect the remaining frames.
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>> {
        if !self.streaming {
            return Ok(Vec::new());
        }
        unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)? };
        self.pull_frames()
    }

    fn pull_frames(&mut self) -> Result<Vec<DecodedFrame>> {
        let mut frames = Vec::new();

        loop {
            let mut output = MFT_OUTPUT_DATA_BUFFER::default();
            if self.stream_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) == 0 {
                let capacity = self
                    .stream_info
                    .cbSize
                    .max(self.width.saturating_mul(self.height).saturating_mul(3) / 2);
                unsafe {
                    let buffer = MFCreateMemoryBuffer(capacity.max(64))
                        .context("MFCreateMemoryBuffer (sortie décodeur)")?;
                    let sample =
                        MFCreateSample().context("MFCreateSample (sortie décodeur)")?;
                    sample.AddBuffer(&buffer).context("AddBuffer (sortie décodeur)")?;
                    output.pSample = core::mem::ManuallyDrop::new(Some(sample));
                }
            }

            let mut status = 0u32;
            let result = unsafe {
                self.transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
            };
            // Le sample de sortie est le nôtre. Il vit dans un `ManuallyDrop`,
            // donc rien ne le libère sur les sorties anticipées : on le reprend
            // avant de regarder le résultat, sinon chaque trame sans sortie
            // (le cas courant) fuit son tampon et le processus meurt en
            // E_OUTOFMEMORY au bout de quelques minutes.
            let sample: Option<IMFSample> =
                unsafe { core::mem::ManuallyDrop::take(&mut output.pSample) };

            match result {
                Ok(()) => {}
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(frames),
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.pick_output_type().context("renégociation du type de sortie")?;
                    continue;
                }
                Err(error) => return Err(error).context("ProcessOutput décodeur"),
            }

            let Some(sample) = sample else { continue };

            let pts_us = unsafe { sample.GetSampleTime() }
                .ok()
                .map(|value| (value.max(0) as u64) / 10);
            let mut frame = read_nv12(&sample, self.width, self.height, self.coded_height)?;
            frame.pts_us = pts_us;
            frames.push(frame);
        }
    }
}

fn read_nv12(sample: &IMFSample, width: u32, height: u32, coded_height: u32) -> Result<DecodedFrame> {
    let width = width as usize;
    let height = height as usize;
    let coded_height = (coded_height as usize).max(height);
    let mut nv12 = vec![0u8; width * height * 3 / 2];
    let buffer = unsafe { sample.GetBufferByIndex(0) }.context("GetBufferByIndex")?;

    // `copy_nv12` strips the source padding: the resulting buffer is compact,
    // so the reported strides are always the width (returning the decoder's
    // pitch here shifts every row when the pitch is aligned above the width).
    if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
        let mut scanline = std::ptr::null_mut();
        let mut pitch = 0i32;
        unsafe {
            buffer2d.Lock2D(&mut scanline, &mut pitch)?;
            let pitch = if pitch > 0 { pitch as usize } else { width };
            copy_nv12(scanline, pitch, width, height, coded_height, &mut nv12);
            buffer2d.Unlock2D()?;
        }
    } else {
        let mut pointer = std::ptr::null_mut();
        let mut current = 0u32;
        unsafe {
            buffer.Lock(&mut pointer, None, Some(&mut current))?;
            // A 1-D buffer may still pad its rows. Its current length (not the
            // allocated capacity: the MFT may size it for a 2-byte-per-pixel
            // format) reveals the pitch when it divides exactly.
            let length = current as usize;
            let coded_bytes = (coded_height as u64) * 3 / 2;
            let pitch = if coded_height > 0 && coded_bytes > 0 && length as u64 % coded_bytes == 0
            {
                ((length as u64 / coded_bytes) as usize).max(width)
            } else {
                width
            };
            copy_nv12(pointer, pitch, width, height, coded_height, &mut nv12);
            buffer.Unlock()?;
        }
    }

    Ok(DecodedFrame {
        nv12,
        stride_y: width,
        stride_uv: width,
        width: width as u32,
        height: height as u32,
        pts_us: None,
    })
}

/// Copy NV12 (Y plane then interleaved UV plane) stripping row padding. The
/// chroma plane starts after `coded_height` rows: 1080p is coded as 1088 and
/// the 8 padding rows are not part of the picture.
unsafe fn copy_nv12(
    scanline: *const u8,
    pitch: usize,
    width: usize,
    height: usize,
    coded_height: usize,
    out: &mut [u8],
) {
    for row in 0..height {
        let source = scanline.add(row * pitch);
        let destination = out.as_mut_ptr().add(row * width);
        std::ptr::copy_nonoverlapping(source, destination, width);
    }
    let chroma_source = scanline.add(pitch * coded_height.max(height));
    for row in 0..height / 2 {
        let source = chroma_source.add(row * pitch);
        let destination = out.as_mut_ptr().add(width * height + row * width);
        std::ptr::copy_nonoverlapping(source, destination, width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chroma_plane_starts_after_the_coded_height() {
        // A 4x2 picture coded as 4x4: the decoder writes four luma rows and the
        // chroma plane starts at row 4. Reading it at row `height` (2) would take
        // the padding rows for chroma and shift the whole picture.
        let (width, height, coded_height, pitch) = (4usize, 2usize, 4usize, 8usize);
        let mut source = vec![0u8; pitch * coded_height + pitch * (coded_height / 2)];
        for row in 0..coded_height {
            source[row * pitch..row * pitch + width].fill(40 + row as u8);
            source[row * pitch + width..(row + 1) * pitch].fill(99);
        }
        let chroma = pitch * coded_height;
        for row in 0..coded_height / 2 {
            source[chroma + row * pitch..chroma + row * pitch + width].fill(128);
        }

        let mut out = vec![0u8; width * height * 3 / 2];
        unsafe { copy_nv12(source.as_ptr(), pitch, width, height, coded_height, &mut out) };

        assert_eq!(&out[..width * height], &[40, 40, 40, 40, 41, 41, 41, 41]);
        assert!(
            out[width * height..].iter().all(|value| *value == 128),
            "chroma décalée: {:?}",
            &out[width * height..]
        );
        assert!(!out.contains(&99), "le remplissage de ligne ne doit pas être copié");
    }
}
