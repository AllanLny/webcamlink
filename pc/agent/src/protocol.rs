//! WCLN wire protocol v1 — see `docs/protocol.md`.

use anyhow::{bail, Result};

pub const MAGIC: [u8; 4] = *b"WCLN";
pub const VERSION: u8 = 1;
pub const HEADER_SIZE: usize = 12;
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

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

#[derive(Debug, Clone, Default)]
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
        let min_version = reader.u8()?;
        let max_version = reader.u8()?;
        let role = reader.u8()?;
        let app_version = reader.string()?;
        let name = reader.string()?;
        let model = reader.string()?;
        Ok(Self { min_version, max_version, role, app_version, name, model })
    }
}

pub fn push_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let length = bytes.len().min(u16::MAX as usize);
    out.extend_from_slice(&(length as u16).to_le_bytes());
    out.extend_from_slice(&bytes[..length]);
}

struct PayloadReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.offset + count > self.data.len() {
            bail!("payload WCLN tronqué");
        }
        let slice = &self.data[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn string(&mut self) -> Result<String> {
        let length = self.u16()? as usize;
        let bytes = self.take(length)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}
