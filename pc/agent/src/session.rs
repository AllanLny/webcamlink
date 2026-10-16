//! WCLN session — the host (PC) side of the conversation.
//!
//! A session owns one byte stream (usbmux tunnel to the iPhone, or a plain TCP
//! socket for development against `wcl-fakephone`) and turns it into typed
//! events.

use crate::protocol::{
    self, Caps, Frame, FrameDecoder, Hello, MessageType, ProtocolError, SetCamera, SetFormat,
    SetImage, SetOptions, Status, VideoConfig, VideoFrame,
};
use crate::usbmux;
use anyhow::{bail, Result};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 5321;
const READ_CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub enum Event {
    Hello(Hello),
    Caps(Caps),
    VideoConfig(VideoConfig),
    VideoFrame(VideoFrame),
    Status(Status),
    Heartbeat(u64),
    Error(ProtocolError),
}

impl Event {
    fn from_frame(frame: Frame) -> Result<Self> {
        Ok(match frame.message_type {
            MessageType::Hello => Self::Hello(Hello::decode(&frame.payload)?),
            MessageType::Caps => Self::Caps(Caps::decode(&frame.payload)?),
            MessageType::VideoConfig => Self::VideoConfig(VideoConfig::decode(&frame.payload)?),
            MessageType::VideoFrame => Self::VideoFrame(VideoFrame::decode(&frame.payload)?),
            MessageType::Status => Self::Status(Status::decode(&frame.payload)?),
            MessageType::Heartbeat => Self::Heartbeat(protocol::decode_heartbeat(&frame.payload)?),
            MessageType::Error => Self::Error(ProtocolError::decode(&frame.payload)?),
            other => bail!("message {other:?} inattendu du device"),
        })
    }
}

#[derive(Debug)]
pub enum SessionError {
    /// The peer closed the connection (app arrêtée, câble débranché…).
    Closed,
    /// No data within the requested window; the connection is still alive.
    Timeout,
    Io(std::io::Error),
    Protocol(anyhow::Error),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(formatter, "connexion fermée"),
            Self::Timeout => write!(formatter, "aucune donnée reçue"),
            Self::Io(error) => write!(formatter, "erreur réseau: {error}"),
            Self::Protocol(error) => write!(formatter, "erreur de protocole: {error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<anyhow::Error> for SessionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Protocol(error)
    }
}

pub struct Session {
    stream: TcpStream,
    decoder: FrameDecoder,
    pending: VecDeque<Frame>,
    pub hello: Hello,
    pub caps: Option<Caps>,
}

impl Session {
    pub fn connect_tcp(address: &str) -> Result<Self> {
        let stream = TcpStream::connect(address)?;
        stream.set_nodelay(true)?;
        Self::from_stream(stream)
    }

    /// Tunnel to `port` on the first USB device through usbmuxd.
    pub fn connect_usb(port: u16) -> Result<Self> {
        let device = usbmux::first_usb_device()?;
        let stream = usbmux::connect_device(device.device_id, port)?;
        stream.set_nodelay(true)?;
        Self::from_stream(stream)
    }

    fn from_stream(stream: TcpStream) -> Result<Self> {
        Ok(Self {
            stream,
            decoder: FrameDecoder::new(),
            pending: VecDeque::new(),
            hello: Hello::default(),
            caps: None,
        })
    }

    pub fn peer_label(&self) -> String {
        format!("{} ({})", self.hello.name, self.hello.model)
    }

    /// Send our `Hello`, wait for the device's, and keep the announced caps.
    pub fn handshake(&mut self, within: Duration) -> Result<Hello, SessionError> {
        self.send(MessageType::Hello, Hello::host().encode())?;
        let deadline = Instant::now() + within;
        loop {
            match self.next_event_until(deadline)? {
                Some(Event::Hello(hello)) => {
                    log::info!(
                        "Hello reçu: {} ({}) v{}–{}",
                        hello.name,
                        hello.model,
                        hello.min_version,
                        hello.max_version
                    );
                    self.hello = hello.clone();
                    return Ok(hello);
                }
                Some(Event::Caps(caps)) => {
                    self.caps = Some(caps);
                }
                Some(other) => log::debug!("message ignoré pendant le handshake: {other:?}"),
                None => return Err(SessionError::Protocol(anyhow::anyhow!("pas de Hello reçu"))),
            }
        }
    }

    /// Wait for the device `Caps` if it has not arrived yet.
    pub fn wait_caps(&mut self, within: Duration) -> Result<Caps, SessionError> {
        if let Some(caps) = &self.caps {
            return Ok(caps.clone());
        }
        let deadline = Instant::now() + within;
        loop {
            match self.next_event_until(deadline)? {
                Some(Event::Caps(caps)) => {
                    self.caps = Some(caps.clone());
                    return Ok(caps);
                }
                Some(other) => log::debug!("message ignoré en attendant Caps: {other:?}"),
                None => return Err(SessionError::Protocol(anyhow::anyhow!("pas de Caps reçu"))),
            }
        }
    }

    pub fn send(&mut self, message_type: MessageType, payload: Vec<u8>) -> Result<()> {
        let bytes = Frame::new(message_type, payload).encode();
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    pub fn set_camera(&mut self, camera_id: u8) -> Result<()> {
        self.send(MessageType::SetCamera, SetCamera { camera_id }.encode())
    }

    pub fn set_format(&mut self, format: SetFormat) -> Result<()> {
        self.send(MessageType::SetFormat, format.encode())
    }

    pub fn set_options(&mut self, options: SetOptions) -> Result<()> {
        self.send(MessageType::SetOptions, options.encode())
    }

    pub fn set_image(&mut self, image: SetImage) -> Result<()> {
        self.send(MessageType::SetImage, image.encode())
    }

    pub fn start(&mut self) -> Result<()> {
        self.send(MessageType::Start, Vec::new())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.send(MessageType::Stop, Vec::new())
    }

    pub fn request_keyframe(&mut self) -> Result<()> {
        self.send(MessageType::RequestKeyframe, Vec::new())
    }

    pub fn now_us() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64
    }

    pub fn heartbeat(&mut self) -> Result<u64> {
        let stamp = Self::now_us();
        self.send(MessageType::Heartbeat, protocol::encode_heartbeat(stamp))?;
        Ok(stamp)
    }

    /// Next event, waiting at most `within`.
    pub fn next_event(&mut self, within: Duration) -> Result<Option<Event>, SessionError> {
        self.next_event_until(Instant::now() + within)
    }

    pub fn next_event_until(&mut self, deadline: Instant) -> Result<Option<Event>, SessionError> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Event::from_frame(frame).map(Some).map_err(SessionError::Protocol);
            }

            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline - now;
            self.stream
                .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
                .map_err(SessionError::Io)?;

            let mut buffer = [0u8; READ_CHUNK];
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(SessionError::Closed),
                Ok(count) => {
                    let frames = self
                        .decoder
                        .push(&buffer[..count])
                        .map_err(SessionError::Protocol)?;
                    self.pending.extend(frames);
                }
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
                    ) =>
                {
                    return Err(SessionError::Closed);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(SessionError::Io(error)),
            }
        }
    }
}
