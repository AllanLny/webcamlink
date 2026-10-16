//! H.264 encoder built on the Media Foundation encoder MFT.
//!
//! Used by the dev tool that generates test clips (`wcl-mkh264`); the iPhone
//! is the real encoder on the product path.

use crate::h264;
use crate::mf;
use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_Base, ICodecAPI, IMFSample,
    IMFTransform,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    CODECAPI_AVEncCommonLowLatency, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQuality,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncNumWorkerThreads,
};
use windows::Win32::System::Variant::VARIANT;

#[derive(Debug, Clone)]
pub struct EncodedAccessUnit {
    pub pts_us: u64,
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub struct H264Encoder {
    transform: IMFTransform,
    fps: u32,
    frame_bytes: usize,
    stream_info: MFT_OUTPUT_STREAM_INFO,
    streaming: bool,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Result<Self> {
        let transform = mf::create_transform(
            MFT_CATEGORY_VIDEO_ENCODER,
            Some((MFMediaType_Video, MFVideoFormat_NV12)),
            Some((MFMediaType_Video, MFVideoFormat_H264)),
            false,
        )
        .context("création de l'encodeur H.264")?;

        unsafe {
            let output = mf::new_media_type()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_kbps.saturating_mul(1000))?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, mf::pack_size(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, mf::pack_ratio(fps, 1))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, mf::pack_ratio(1, 1))?;
            transform.SetOutputType(0, &output, 0).context("SetOutputType H.264")?;

            let input = mf::new_media_type()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, mf::pack_size(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, mf::pack_ratio(fps, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, mf::pack_ratio(1, 1))?;
            transform.SetInputType(0, &input, 0).context("SetInputType NV12")?;

            tune(transform.cast::<ICodecAPI>()?, fps, bitrate_kbps);
        }

        let stream_info = unsafe { transform.GetOutputStreamInfo(0) }.context("GetOutputStreamInfo")?;

        Ok(Self {
            transform,
            fps,
            frame_bytes: (width * height * 3 / 2) as usize,
            stream_info,
            streaming: false,
        })
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    fn frame_duration_100ns(&self) -> i64 {
        10_000_000 / i64::from(self.fps.max(1))
    }

    /// Encode one NV12 frame; returns every access unit the encoder released.
    pub fn encode(&mut self, frame_index: u64, nv12: &[u8]) -> Result<Vec<EncodedAccessUnit>> {
        anyhow::ensure!(
            nv12.len() >= self.frame_bytes,
            "frame NV12 trop courte: {} < {}",
            nv12.len(),
            self.frame_bytes
        );

        if !self.streaming {
            unsafe {
                self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            }
            self.streaming = true;
        }

        let duration = self.frame_duration_100ns();
        let pts = frame_index as i64 * duration;

        unsafe {
            let buffer = MFCreateMemoryBuffer(self.frame_bytes as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), pointer, self.frame_bytes);
            buffer.Unlock()?;
            buffer.SetCurrentLength(self.frame_bytes as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;

            self.transform.ProcessInput(0, &sample, 0).context("ProcessInput encodeur")?;
        }

        let mut units = self.pull_outputs()?;
        for unit in &mut units {
            unit.pts_us = (pts / 10).max(0) as u64;
        }
        Ok(units)
    }

    /// Drain the encoder (no more input).
    pub fn finish(&mut self) -> Result<Vec<EncodedAccessUnit>> {
        if !self.streaming {
            return Ok(Vec::new());
        }
        unsafe {
            self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        self.pull_outputs()
    }

    fn pull_outputs(&mut self) -> Result<Vec<EncodedAccessUnit>> {
        let mut units = Vec::new();
        loop {
            let mut output = MFT_OUTPUT_DATA_BUFFER::default();

            // Let the encoder allocate its own samples when it can.
            if self.stream_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) == 0 {
                let capacity = self.stream_info.cbSize.max(self.frame_bytes as u32);
                unsafe {
                    let buffer = MFCreateMemoryBuffer(capacity)?;
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&buffer)?;
                    output.pSample = core::mem::ManuallyDrop::new(Some(sample));
                }
            }

            let mut status = 0u32;
            match unsafe {
                self.transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
            } {
                Ok(()) => {}
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(units),
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    anyhow::bail!("changement de flux inattendu à l'encodage");
                }
                Err(error) => return Err(error).context("ProcessOutput encodeur"),
            }

            let sample: Option<IMFSample> = unsafe { core::mem::ManuallyDrop::take(&mut output.pSample) };
            let Some(sample) = sample else { continue };

            let data = sample_bytes(&sample)?;
            if data.is_empty() {
                continue;
            }
            let pts_us = unsafe { sample.GetSampleTime() }.unwrap_or(0).max(0) as u64 / 10;
            units.push(EncodedAccessUnit {
                pts_us,
                keyframe: h264::contains_idr_slice(&data),
                data,
            });
        }
    }
}

/// Read a sample's payload through a contiguous buffer.
pub fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().context("ConvertToContiguousBuffer")?;
        let mut pointer = std::ptr::null_mut();
        let mut current = 0u32;
        buffer.Lock(&mut pointer, None, Some(&mut current))?;
        let data = std::slice::from_raw_parts(pointer, current as usize).to_vec();
        buffer.Unlock()?;
        Ok(data)
    }
}

/// Low-latency, real-time oriented settings; unsupported ones are ignored.
fn tune(codec: ICodecAPI, fps: u32, bitrate_kbps: u32) {
    let settings: [(windows::core::GUID, VARIANT); 7] = [
        (CODECAPI_AVEncCommonLowLatency, mf::variant_u32(1)),
        (
            CODECAPI_AVEncCommonRateControlMode,
            // CBR would drop/repeat frames on this synthetic content; the clip
            // tool wants a strict 1:1 frame mapping.
            mf::variant_u32(eAVEncCommonRateControlMode_Quality.0 as u32),
        ),
        (CODECAPI_AVEncCommonQuality, mf::variant_u32(70)),
        (CODECAPI_AVEncCommonMeanBitRate, mf::variant_u32(bitrate_kbps.saturating_mul(1000))),
        (CODECAPI_AVEncMPVGOPSize, mf::variant_u32(fps.saturating_mul(2))),
        // No B-frames: decode order stays display order, so the decoder never
        // reorders and players can show every frame as it arrives.
        (CODECAPI_AVEncMPVDefaultBPictureCount, mf::variant_u32(0)),
        (CODECAPI_AVEncNumWorkerThreads, mf::variant_u32(0)),
    ];
    for (key, value) in settings {
        if let Err(error) = unsafe { codec.SetValue(&key, &value) } {
            log::debug!("ICodecAPI: réglage ignoré ({key:?}): {error}");
        }
    }
}
