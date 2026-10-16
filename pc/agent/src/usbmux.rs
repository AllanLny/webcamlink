//! usbmux transport: talk to Apple's usbmuxd on Windows
//! (`127.0.0.1:27015`, 16-byte LE header + XML plist payload).

use anyhow::{bail, Context, Result};
use plist::{Dictionary, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub const USBMUX_ADDR: &str = "127.0.0.1:27015";
const MSG_PLIST: u32 = 8;
const HEADER_SIZE: usize = 16;
const MAX_PAYLOAD: usize = 1 << 20;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_id: u32,
    pub connection_type: String,
    pub name: String,
    pub udid: String,
    pub product_type: String,
}

pub struct MuxStream {
    pub stream: TcpStream,
    tag: u32,
}

pub fn open() -> Result<MuxStream> {
    let stream = TcpStream::connect(USBMUX_ADDR)
        .with_context(|| format!("usbmuxd introuvable sur {USBMUX_ADDR} — l'app Apple Devices est-elle lancée ?"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(MuxStream { stream, tag: 1 })
}

impl MuxStream {
    fn write_message(&mut self, tag: u32, body: &[u8]) -> Result<()> {
        let length = (HEADER_SIZE + body.len()) as u32;
        let mut header = Vec::with_capacity(HEADER_SIZE);
        header.extend_from_slice(&length.to_le_bytes());
        header.extend_from_slice(&1u32.to_le_bytes());
        header.extend_from_slice(&MSG_PLIST.to_le_bytes());
        header.extend_from_slice(&tag.to_le_bytes());
        self.stream.write_all(&header)?;
        self.stream.write_all(body)?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<(u32, u32, Vec<u8>)> {
        let mut header = [0u8; HEADER_SIZE];
        self.stream.read_exact(&mut header)?;
        let length = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let message_type = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let tag = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        if !(HEADER_SIZE..=MAX_PAYLOAD).contains(&length) {
            bail!("longueur usbmux suspecte: {length}");
        }
        let mut body = vec![0u8; length - HEADER_SIZE];
        if !body.is_empty() {
            self.stream.read_exact(&mut body)?;
        }
        Ok((message_type, tag, body))
    }

    /// Send a plist request and wait for the reply carrying our tag.
    pub fn request(&mut self, message_type: &str, extra: Dictionary) -> Result<Dictionary> {
        let mut dict = Dictionary::new();
        dict.insert("MessageType".to_string(), Value::String(message_type.to_string()));
        dict.insert("ProgName".to_string(), Value::String("webcamlink-agent".to_string()));
        dict.insert("ClientVersionString".to_string(), Value::String(env!("CARGO_PKG_VERSION").to_string()));
        for (key, value) in extra {
            dict.insert(key, value);
        }

        let tag = self.tag;
        self.tag += 1;

        let mut body = Vec::new();
        plist::to_writer_xml(&mut body, &Value::Dictionary(dict))?;
        self.write_message(tag, &body)?;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let (_message_type, reply_tag, payload) = self.read_message()?;
            if reply_tag != tag {
                continue; // hot-plug events etc.
            }
            let value: Value = plist::from_bytes(&payload).context("réponse usbmux illisible")?;
            return match value {
                Value::Dictionary(dict) => Ok(dict),
                other => bail!("réponse usbmux inattendue: {other:?}"),
            };
        }
        bail!("pas de réponse usbmux pour le tag {tag}")
    }

    pub fn list_devices(&mut self) -> Result<Vec<DeviceInfo>> {
        let reply = self.request("ListDevices", Dictionary::new())?;
        let Some(Value::Array(devices)) = reply.get("DeviceList") else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for device in devices {
            let Value::Dictionary(device) = device else { continue };
            let device_id = device
                .get("DeviceID")
                .and_then(|value| value.as_unsigned_integer())
                .unwrap_or(0) as u32;
            let connection_type = device
                .get("ConnectionType")
                .and_then(|value| value.as_string())
                .unwrap_or("USB")
                .to_string();
            let props = match device.get("Properties") {
                Some(Value::Dictionary(props)) => props,
                _ => &Dictionary::new(),
            };
            let name = props.get("DeviceName").and_then(|v| v.as_string()).unwrap_or("iPhone").to_string();
            let udid = props.get("UniqueDeviceID").and_then(|v| v.as_string()).unwrap_or("").to_string();
            let product_type = props.get("ProductType").and_then(|v| v.as_string()).unwrap_or("").to_string();
            out.push(DeviceInfo { device_id, connection_type, name, udid, product_type });
        }
        Ok(out)
    }
}

/// Open the raw tunnel to `port` on `device_id`. `Connect` must use the
/// byte-swapped port (5321 -> 51476); the socket becomes the byte stream on OK.
pub fn connect_device(device_id: u32, port: u16) -> Result<TcpStream> {
    let mut mux = open()?;
    let mut extra = Dictionary::new();
    extra.insert("DeviceID".to_string(), Value::Integer(device_id.into()));
    extra.insert("PortNumber".to_string(), Value::Integer(port.swap_bytes().into()));
    let reply = mux.request("Connect", extra)?;
    let number = reply
        .get("Number")
        .and_then(|value| value.as_unsigned_integer())
        .unwrap_or(u64::MAX);
    match number {
        0 => Ok(mux.stream),
        2 => bail!("Connect refusé: device invalide (code 2)"),
        3 => bail!("Connect refusé: personne n'écoute sur le port {port} (code 3) — l'app iPhone est-elle lancée ?"),
        other => bail!("Connect refusé: code {other}"),
    }
}

/// First USB device, if any.
pub fn first_usb_device() -> Result<DeviceInfo> {
    let mut mux = open()?;
    let devices = mux.list_devices()?;
    devices
        .into_iter()
        .find(|device| device.connection_type == "USB")
        .context("aucun iPhone en USB (branché, déverrouillé et « Trust » ?)")
}
