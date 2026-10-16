//! WCLN wire protocol v1 — see `docs/protocol.md`.

use anyhow::{bail, Result};

pub const MAGIC: [u8; 4] = *b"WCLN";
pub const VERSION: u8 = 1;
pub const HEADER_SIZE: usize = 12;
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

pub const POSITION_BACK: u8 = 0;
pub const POSITION_FRONT: u8 = 1;

pub const LENS_WIDE: u8 = 0;
pub const LENS_ULTRA_WIDE: u8 = 1;
pub const LENS_TELE: u8 = 2;
pub const LENS_FRONT: u8 = 3;
pub const LENS_OTHER: u8 = 255;

pub const CODEC_H264: u8 = 0;
pub const CODEC_HEVC: u8 = 1;

pub const CAPS_CODEC_H264: u8 = 1 << 0;
pub const CAPS_CODEC_HEVC: u8 = 1 << 1;

pub const FRAME_FLAG_KEYFRAME: u8 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Hello = 0x01,
    Caps = 0x02,
    SetCamera = 0x10,
    SetFormat = 0x11,
    Start = 0x12,
    Stop = 0x13,
    RequestKeyframe = 0x14,
    SetOptions = 0x15,
    SetImage = 0x16,
    VideoConfig = 0x20,
    VideoFrame = 0x21,
    Heartbeat = 0x30,
    Status = 0x31,
    Error = 0x7F,
}

impl MessageType {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0x01 => Self::Hello,
            0x02 => Self::Caps,
            0x10 => Self::SetCamera,
            0x11 => Self::SetFormat,
            0x12 => Self::Start,
            0x13 => Self::Stop,
            0x14 => Self::RequestKeyframe,
            0x15 => Self::SetOptions,
            0x16 => Self::SetImage,
            0x20 => Self::VideoConfig,
            0x21 => Self::VideoFrame,
            0x30 => Self::Heartbeat,
            0x31 => Self::Status,
            0x7F => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub message_type: MessageType,
    pub flags: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(message_type: MessageType, payload: Vec<u8>) -> Self {
        Self { message_type, flags: 0, payload }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(self.message_type as u8);
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Incremental decoder for a byte stream (one TCP connection).
#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes, get every complete frame. Unknown message types are skipped;
    /// a malformed header is a hard error (the connection must be closed).
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Frame>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if self.buffer.len() < HEADER_SIZE {
                break;
            }
            let header = &self.buffer[..HEADER_SIZE];
            if header[0..4] != MAGIC || header[4] != VERSION {
                bail!("en-tête WCLN invalide (magic/version)");
            }
            let message_type = header[5];
            let flags = u16::from_le_bytes([header[6], header[7]]);
            let length = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
            if length > MAX_PAYLOAD {
                bail!("payload {length} > MAX_PAYLOAD");
            }
            let total = HEADER_SIZE + length;
            if self.buffer.len() < total {
                break;
            }
            let payload = self.buffer[HEADER_SIZE..total].to_vec();
            self.buffer.drain(..total);
            if let Some(message_type) = MessageType::from_u8(message_type) {
                frames.push(Frame { message_type, flags, payload });
            }
        }
        Ok(frames)
    }
}

// MARK: - Payload helpers

pub fn push_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

pub fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn push_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let length = bytes.len().min(u16::MAX as usize);
    out.extend_from_slice(&(length as u16).to_le_bytes());
    out.extend_from_slice(&bytes[..length]);
}

pub struct PayloadReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.offset + count > self.data.len() {
            bail!("payload WCLN tronqué");
        }
        let slice = &self.data[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn string(&mut self) -> Result<String> {
        let length = self.u16()? as usize;
        let bytes = self.take(length)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Same as `string()`, but returns `None` when the field is absent
    /// (a v1 message may legally be truncated: missing trailing fields keep
    /// their previous value).
    pub fn optional_string(&mut self) -> Result<Option<String>> {
        if self.remaining() < 2 {
            return Ok(None);
        }
        let length = u16::from_le_bytes([self.data[self.offset], self.data[self.offset + 1]]) as usize;
        if self.remaining() < 2 + length {
            return Ok(None);
        }
        Ok(Some(self.string()?))
    }
}

// MARK: - Hello (0x01)

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hello {
    pub min_version: u8,
    pub max_version: u8,
    pub role: u8,
    pub app_version: String,
    pub name: String,
    pub model: String,
}

impl Hello {
    pub fn host() -> Self {
        Self {
            min_version: VERSION,
            max_version: VERSION,
            role: 0,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows".to_string()),
            model: "Windows 11".to_string(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.min_version);
        out.push(self.max_version);
        out.push(self.role);
        push_string(&mut out, &self.app_version);
        push_string(&mut out, &self.name);
        push_string(&mut out, &self.model);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        Ok(Self {
            min_version: reader.u8()?,
            max_version: reader.u8()?,
            role: reader.u8()?,
            app_version: reader.string()?,
            name: reader.string()?,
            model: reader.string()?,
        })
    }
}

// MARK: - Caps (0x02)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraFormat {
    pub width: u16,
    pub height: u16,
    pub max_fps: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    pub camera_id: u8,
    pub position: u8,
    pub lens: u8,
    pub label: String,
    pub formats: Vec<CameraFormat>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps {
    pub cameras: Vec<Camera>,
    pub codecs: u8,
}

impl Caps {
    pub fn supports_h264(&self) -> bool {
        self.codecs & CAPS_CODEC_H264 != 0
    }

    pub fn supports_hevc(&self) -> bool {
        self.codecs & CAPS_CODEC_HEVC != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, self.cameras.len().min(u8::MAX as usize) as u8);
        for camera in &self.cameras {
            push_u8(&mut out, camera.camera_id);
            push_u8(&mut out, camera.position);
            push_u8(&mut out, camera.lens);
            push_string(&mut out, &camera.label);
            push_u16(&mut out, camera.formats.len().min(u16::MAX as usize) as u16);
            for format in &camera.formats {
                push_u16(&mut out, format.width);
                push_u16(&mut out, format.height);
                push_u16(&mut out, format.max_fps);
            }
        }
        push_u8(&mut out, self.codecs);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        let camera_count = reader.u8()? as usize;
        let mut cameras = Vec::with_capacity(camera_count);
        for _ in 0..camera_count {
            let camera_id = reader.u8()?;
            let position = reader.u8()?;
            let lens = reader.u8()?;
            let label = reader.string()?;
            let format_count = reader.u16()? as usize;
            let mut formats = Vec::with_capacity(format_count);
            for _ in 0..format_count {
                formats.push(CameraFormat {
                    width: reader.u16()?,
                    height: reader.u16()?,
                    max_fps: reader.u16()?,
                });
            }
            cameras.push(Camera { camera_id, position, lens, label, formats });
        }
        let codecs = reader.u8().unwrap_or(CAPS_CODEC_H264);
        Ok(Self { cameras, codecs })
    }
}

// MARK: - SetCamera (0x10)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCamera {
    pub camera_id: u8,
}

impl SetCamera {
    pub fn encode(&self) -> Vec<u8> {
        vec![self.camera_id]
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        Ok(Self { camera_id: PayloadReader::new(payload).u8()? })
    }
}

// MARK: - SetFormat (0x11)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetFormat {
    pub width: u16,
    pub height: u16,
    pub fps: u16,
    pub codec: u8,
    pub bitrate_kbps: u32,
}

impl SetFormat {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u16(&mut out, self.width);
        push_u16(&mut out, self.height);
        push_u16(&mut out, self.fps);
        push_u8(&mut out, self.codec);
        push_u32(&mut out, self.bitrate_kbps);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        Ok(Self {
            width: reader.u16()?,
            height: reader.u16()?,
            fps: reader.u16()?,
            codec: reader.u8()?,
            bitrate_kbps: reader.u32()?,
        })
    }
}

// MARK: - SetOptions (0x15)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetOptions {
    pub mirror: bool,
    pub rotation: u16,
    pub dim_preview: bool,
}

impl SetOptions {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, self.mirror as u8);
        push_u16(&mut out, self.rotation);
        push_u8(&mut out, self.dim_preview as u8);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        Ok(Self {
            mirror: reader.u8()? != 0,
            rotation: reader.u16()?,
            dim_preview: reader.u8()? != 0,
        })
    }
}

// MARK: - SetImage (0x16)

pub const EXPOSURE_AUTO: u8 = 0;
pub const EXPOSURE_BIAS: u8 = 1;
pub const EXPOSURE_MANUAL: u8 = 2;

pub const FOCUS_CONTINUOUS: u8 = 0;
pub const FOCUS_LOCKED: u8 = 1;
pub const FOCUS_POINT: u8 = 2;

pub const WB_AUTO: u8 = 0;
pub const WB_LOCKED: u8 = 1;
pub const WB_MANUAL: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetImage {
    pub zoom_x100: u16,
    pub exposure_mode: u8,
    pub exposure_bias_ev_x100: i16,
    pub iso: u16,
    pub exposure_duration_us: u32,
    pub focus_mode: u8,
    pub focus_x_x10000: u16,
    pub focus_y_x10000: u16,
    pub wb_mode: u8,
    pub wb_temp_k: u16,
    pub wb_tint_x100: i16,
    pub torch: bool,
    pub stabilization: u8,
    pub hdr: bool,
}

impl Default for SetImage {
    fn default() -> Self {
        Self {
            zoom_x100: 100,
            exposure_mode: EXPOSURE_AUTO,
            exposure_bias_ev_x100: 0,
            iso: 0,
            exposure_duration_us: 0,
            focus_mode: FOCUS_CONTINUOUS,
            focus_x_x10000: 5000,
            focus_y_x10000: 5000,
            wb_mode: WB_AUTO,
            wb_temp_k: 0,
            wb_tint_x100: 0,
            torch: false,
            stabilization: 0,
            hdr: false,
        }
    }
}

impl SetImage {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u16(&mut out, self.zoom_x100);
        push_u8(&mut out, self.exposure_mode);
        push_u16(&mut out, self.exposure_bias_ev_x100 as u16);
        push_u16(&mut out, self.iso);
        push_u32(&mut out, self.exposure_duration_us);
        push_u8(&mut out, self.focus_mode);
        push_u16(&mut out, self.focus_x_x10000);
        push_u16(&mut out, self.focus_y_x10000);
        push_u8(&mut out, self.wb_mode);
        push_u16(&mut out, self.wb_temp_k);
        push_u16(&mut out, self.wb_tint_x100 as u16);
        push_u8(&mut out, self.torch as u8);
        push_u8(&mut out, self.stabilization);
        push_u8(&mut out, self.hdr as u8);
        out
    }

    /// Trailing fields may legally be absent: they keep their previous value,
    /// so the caller passes its current state and gets an updated copy back.
    pub fn decode_into(mut self, payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        macro_rules! field {
            ($read:ident, $target:expr, $cast:expr) => {
                if reader.remaining() >= std::mem::size_of_val(&$target) {
                    $target = $cast(reader.$read()?);
                }
            };
        }
        field!(u16, self.zoom_x100, |v: u16| v);
        field!(u8, self.exposure_mode, |v: u8| v);
        field!(u16, self.exposure_bias_ev_x100, |v: u16| v as i16);
        field!(u16, self.iso, |v: u16| v);
        field!(u32, self.exposure_duration_us, |v: u32| v);
        field!(u8, self.focus_mode, |v: u8| v);
        field!(u16, self.focus_x_x10000, |v: u16| v);
        field!(u16, self.focus_y_x10000, |v: u16| v);
        field!(u8, self.wb_mode, |v: u8| v);
        field!(u16, self.wb_temp_k, |v: u16| v);
        field!(u16, self.wb_tint_x100, |v: u16| v as i16);
        field!(u8, self.torch, |v: u8| v != 0);
        field!(u8, self.stabilization, |v: u8| v);
        field!(u8, self.hdr, |v: u8| v != 0);
        Ok(self)
    }
}

// MARK: - VideoConfig (0x20)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoConfig {
    pub codec: u8,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
    pub parameter_sets: Vec<u8>,
}

impl VideoConfig {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, self.codec);
        push_u16(&mut out, self.width);
        push_u16(&mut out, self.height);
        push_u16(&mut out, self.fps);
        push_u32(&mut out, self.parameter_sets.len() as u32);
        out.extend_from_slice(&self.parameter_sets);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        let codec = reader.u8()?;
        let width = reader.u16()?;
        let height = reader.u16()?;
        let fps = reader.u16()?;
        let length = reader.u32()? as usize;
        let parameter_sets = reader.take(length)?.to_vec();
        Ok(Self { codec, width, height, fps, parameter_sets })
    }
}

// MARK: - VideoFrame (0x21)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub pts_us: u64,
    pub flags: u8,
    pub data: Vec<u8>,
}

impl VideoFrame {
    pub fn is_keyframe(&self) -> bool {
        self.flags & FRAME_FLAG_KEYFRAME != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.data.len());
        push_u64(&mut out, self.pts_us);
        push_u8(&mut out, self.flags);
        out.extend_from_slice(&self.data);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        let pts_us = reader.u64()?;
        let flags = reader.u8()?;
        let data = reader.take(reader.remaining())?.to_vec();
        Ok(Self { pts_us, flags, data })
    }
}

// MARK: - Heartbeat (0x30)

pub fn encode_heartbeat(send_time_us: u64) -> Vec<u8> {
    send_time_us.to_le_bytes().to_vec()
}

pub fn decode_heartbeat(payload: &[u8]) -> Result<u64> {
    PayloadReader::new(payload).u64()
}

// MARK: - Status (0x31)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub thermal_state: u8,
    pub battery_percent: u8,
    pub charging: bool,
    pub actual_bitrate_kbps: u32,
    pub actual_fps_x100: u16,
    pub camera_id: u8,
}

impl Status {
    pub fn actual_fps(&self) -> f32 {
        self.actual_fps_x100 as f32 / 100.0
    }

    pub fn thermal_label(&self) -> &'static str {
        match self.thermal_state {
            0 => "nominal",
            1 => "fair",
            2 => "serious",
            3 => "critical",
            _ => "inconnu",
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, self.thermal_state);
        push_u8(&mut out, self.battery_percent);
        push_u8(&mut out, self.charging as u8);
        push_u32(&mut out, self.actual_bitrate_kbps);
        push_u16(&mut out, self.actual_fps_x100);
        push_u8(&mut out, self.camera_id);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        Ok(Self {
            thermal_state: reader.u8()?,
            battery_percent: reader.u8()?,
            charging: reader.u8()? != 0,
            actual_bitrate_kbps: reader.u32()?,
            actual_fps_x100: reader.u16()?,
            camera_id: reader.u8()?,
        })
    }
}

// MARK: - Error (0x7F)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    UnsupportedVersion = 1,
    CameraPermissionDenied = 2,
    InvalidFormat = 3,
    CameraUnavailable = 4,
    EncoderFailure = 5,
    ThermalShutdown = 6,
    NotInForeground = 7,
}

impl ErrorCode {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::UnsupportedVersion,
            2 => Self::CameraPermissionDenied,
            3 => Self::InvalidFormat,
            4 => Self::CameraUnavailable,
            5 => Self::EncoderFailure,
            6 => Self::ThermalShutdown,
            7 => Self::NotInForeground,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "version non supportée",
            Self::CameraPermissionDenied => "accès caméra refusé sur l'iPhone",
            Self::InvalidFormat => "format invalide",
            Self::CameraUnavailable => "caméra indisponible (utilisée ailleurs ou app en arrière-plan)",
            Self::EncoderFailure => "échec de l'encodeur iPhone",
            Self::ThermalShutdown => "arrêt thermique (iPhone trop chaud)",
            Self::NotInForeground => "l'app iPhone doit être au premier plan",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: u16,
    pub message: String,
}

impl ProtocolError {
    pub fn code_label(&self) -> Option<ErrorCode> {
        ErrorCode::from_u16(self.code)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u16(&mut out, self.code);
        push_string(&mut out, &self.message);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = PayloadReader::new(payload);
        let code = reader.u16()?;
        let message = reader.optional_string()?.unwrap_or_default();
        Ok(Self { code, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trip() {
        let hello = Hello::host();
        let decoded = Hello::decode(&hello.encode()).unwrap();
        assert_eq!(hello, decoded);
    }

    #[test]
    fn caps_round_trip() {
        let caps = Caps {
            cameras: vec![
                Camera {
                    camera_id: 0,
                    position: POSITION_BACK,
                    lens: LENS_WIDE,
                    label: "Back Camera".to_string(),
                    formats: vec![
                        CameraFormat { width: 1920, height: 1080, max_fps: 60 },
                        CameraFormat { width: 3840, height: 2160, max_fps: 30 },
                    ],
                },
                Camera {
                    camera_id: 1,
                    position: POSITION_BACK,
                    lens: LENS_ULTRA_WIDE,
                    label: "Back Ultra Wide".to_string(),
                    formats: vec![CameraFormat { width: 1920, height: 1080, max_fps: 60 }],
                },
            ],
            codecs: CAPS_CODEC_H264 | CAPS_CODEC_HEVC,
        };
        let decoded = Caps::decode(&caps.encode()).unwrap();
        assert_eq!(caps, decoded);
        assert!(decoded.supports_h264() && decoded.supports_hevc());
    }

    #[test]
    fn set_camera_round_trip() {
        let message = SetCamera { camera_id: 3 };
        assert_eq!(message, SetCamera::decode(&message.encode()).unwrap());
    }

    #[test]
    fn set_format_round_trip() {
        let message =
            SetFormat { width: 3840, height: 2160, fps: 30, codec: CODEC_H264, bitrate_kbps: 40_000 };
        assert_eq!(message, SetFormat::decode(&message.encode()).unwrap());
    }

    #[test]
    fn set_options_round_trip() {
        let message = SetOptions { mirror: true, rotation: 180, dim_preview: false };
        assert_eq!(message, SetOptions::decode(&message.encode()).unwrap());
    }

    #[test]
    fn set_image_round_trip() {
        let message = SetImage {
            zoom_x100: 250,
            exposure_mode: EXPOSURE_MANUAL,
            exposure_bias_ev_x100: -75,
            iso: 800,
            exposure_duration_us: 16_666,
            focus_mode: FOCUS_POINT,
            focus_x_x10000: 2500,
            focus_y_x10000: 7500,
            wb_mode: WB_MANUAL,
            wb_temp_k: 5600,
            wb_tint_x100: -20,
            torch: true,
            stabilization: 1,
            hdr: true,
        };
        assert_eq!(message, SetImage::default().decode_into(&message.encode()).unwrap());
    }

    #[test]
    fn set_image_truncated_keeps_previous_values() {
        let message = SetImage { zoom_x100: 300, ..Default::default() };
        let encoded = message.encode();
        let decoded = SetImage::default().decode_into(&encoded[..4]).unwrap();
        assert_eq!(decoded.zoom_x100, 300);
        assert_eq!(decoded.wb_mode, WB_AUTO, "les champs absents gardent leur valeur initiale");
        assert_eq!(decoded.focus_x_x10000, 5000);
    }

    #[test]
    fn video_config_round_trip() {
        let message = VideoConfig {
            codec: CODEC_H264,
            width: 1920,
            height: 1080,
            fps: 30,
            parameter_sets: vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE],
        };
        assert_eq!(message, VideoConfig::decode(&message.encode()).unwrap());
    }

    #[test]
    fn video_frame_round_trip() {
        let message = VideoFrame {
            pts_us: 1_234_567,
            flags: FRAME_FLAG_KEYFRAME,
            data: vec![0, 0, 0, 1, 0x65, 0x88, 0x84],
        };
        let decoded = VideoFrame::decode(&message.encode()).unwrap();
        assert_eq!(message, decoded);
        assert!(decoded.is_keyframe());
    }

    #[test]
    fn heartbeat_round_trip() {
        assert_eq!(decode_heartbeat(&encode_heartbeat(42_000)).unwrap(), 42_000);
    }

    #[test]
    fn status_round_trip() {
        let message = Status {
            thermal_state: 1,
            battery_percent: 82,
            charging: false,
            actual_bitrate_kbps: 24_500,
            actual_fps_x100: 2997,
            camera_id: 0,
        };
        assert_eq!(message, Status::decode(&message.encode()).unwrap());
        assert!((message.actual_fps() - 29.97).abs() < 0.01);
    }

    #[test]
    fn protocol_error_round_trip() {
        let message = ProtocolError { code: 4, message: "caméra en arrière-plan".to_string() };
        let decoded = ProtocolError::decode(&message.encode()).unwrap();
        assert_eq!(message, decoded);
        assert_eq!(decoded.code_label(), Some(ErrorCode::CameraUnavailable));
    }

    #[test]
    fn frame_encoder_and_decoder_round_trip() {
        let frames = vec![
            Frame::new(MessageType::Hello, Hello::host().encode()),
            Frame::new(MessageType::Heartbeat, encode_heartbeat(7)),
        ];
        let mut bytes = Vec::new();
        for frame in &frames {
            bytes.extend_from_slice(&frame.encode());
        }

        let mut decoder = FrameDecoder::new();
        let mut decoded = Vec::new();
        // Feed one byte at a time: exercises the incremental parser.
        for byte in bytes {
            decoded.extend(decoder.push(&[byte]).unwrap());
        }
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].message_type, MessageType::Hello);
        assert_eq!(decoded[1].message_type, MessageType::Heartbeat);
    }

    #[test]
    fn decoder_skips_unknown_message_types() {
        let mut bytes = Frame::new(MessageType::Hello, Hello::default().encode()).encode();
        let mut unknown = vec![0u8; HEADER_SIZE];
        unknown[0..4].copy_from_slice(&MAGIC);
        unknown[4] = VERSION;
        unknown[5] = 0x42;
        bytes.extend_from_slice(&unknown);

        let frames = FrameDecoder::new().push(&bytes).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, MessageType::Hello);
    }

    #[test]
    fn decoder_rejects_bad_magic() {
        let mut bytes = Frame::new(MessageType::Start, Vec::new()).encode();
        bytes[0] = b'X';
        assert!(FrameDecoder::new().push(&bytes).is_err());
    }

    /// The wire layout is a contract shared with the iOS app: these byte
    /// sequences come straight from `docs/protocol.md`.
    #[test]
    fn wire_layout_matches_the_documentation() {
        let hello = Hello {
            min_version: 1,
            max_version: 1,
            role: 1,
            app_version: "0.1.0".to_string(),
            name: "iPhone".to_string(),
            model: "iPhone16,1".to_string(),
        };
        let expected_hello = vec![
            1, 1, 1, //
            5, 0, b'0', b'.', b'1', b'.', b'0', //
            6, 0, b'i', b'P', b'h', b'o', b'n', b'e', //
            10, 0, b'i', b'P', b'h', b'o', b'n', b'e', b'1', b'6', b',', b'1',
        ];
        assert_eq!(hello.encode(), expected_hello);

        let format = SetFormat { width: 1920, height: 1080, fps: 30, codec: CODEC_H264, bitrate_kbps: 20_000 };
        assert_eq!(
            format.encode(),
            vec![0x80, 0x07, 0x38, 0x04, 0x1E, 0x00, 0x00, 0x20, 0x4E, 0x00, 0x00]
        );

        let frame = VideoFrame { pts_us: 1_000_000, flags: FRAME_FLAG_KEYFRAME, data: vec![0, 0, 0, 1, 0x65, 0x88] };
        assert_eq!(
            frame.encode(),
            vec![0x40, 0x42, 0x0F, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, 1, 0x65, 0x88]
        );

        let status = Status {
            thermal_state: 1,
            battery_percent: 82,
            charging: false,
            actual_bitrate_kbps: 24_500,
            actual_fps_x100: 2997,
            camera_id: 0,
        };
        assert_eq!(status.encode(), vec![0x01, 0x52, 0x00, 0xB4, 0x5F, 0x00, 0x00, 0xB5, 0x0B, 0x00]);

        let config = VideoConfig {
            codec: CODEC_H264,
            width: 640,
            height: 480,
            fps: 60,
            parameter_sets: vec![0, 0, 0, 1, 0x67, 0x42],
        };
        let expected_config = vec![
            0x00, // codec H.264
            0x80, 0x02, // 640
            0xE0, 0x01, // 480
            0x3C, 0x00, // 60 fps
            6, 0, 0, 0, // longueur des paramètres
            0, 0, 0, 1, 0x67, 0x42, // SPS
        ];
        assert_eq!(config.encode(), expected_config);
    }
}
