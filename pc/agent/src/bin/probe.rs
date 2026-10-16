//! `wcl-probe` — validation tool for the USB link (P0).
//!
//! Modes: list | lockdown | wcl-hello [port]

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wcl_agent::protocol::{Frame, FrameDecoder, Hello, MessageType};
use wcl_agent::usbmux;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("list");

    match mode {
        "list" => {
            let mut mux = usbmux::open()?;
            let devices = mux.list_devices()?;
            if devices.is_empty() {
                println!("0 appareil visible (iPhone branché, déverrouillé et approuvé ?)");
            }
            for device in devices {
                println!(
                    "- DeviceID={} type={} name={} model={} udid={}…",
                    device.device_id,
                    device.connection_type,
                    device.name,
                    device.product_type,
                    device.udid.chars().take(16).collect::<String>()
                );
            }
            Ok(())
        }

        "lockdown" => {
            let device = usbmux::first_usb_device()?;
            println!("Device: {} (ID {})", device.name, device.device_id);
            let mut tunnel = usbmux::connect_device(device.device_id, 62078)?;
            tunnel.set_read_timeout(Some(Duration::from_secs(5)))?;

            let mut request = plist::Dictionary::new();
            request.insert("Label".to_string(), plist::Value::String("webcamlink-probe".to_string()));
            request.insert("Request".to_string(), plist::Value::String("GetValue".to_string()));
            request.insert("Key".to_string(), plist::Value::String("ProductVersion".to_string()));
            let mut body = Vec::new();
            plist::to_writer_xml(&mut body, &plist::Value::Dictionary(request))?;
            tunnel.write_all(&body)?;

            let mut reply = vec![0u8; 64 * 1024];
            let count = tunnel.read(&mut reply)?;
            let value: plist::Value = plist::from_bytes(&reply[..count]).context("réponse lockdownd illisible")?;
            let version = value
                .as_dictionary()
                .and_then(|dict| dict.get("Value"))
                .and_then(|value| value.as_string())
                .unwrap_or("?");
            println!("lockdownd répond: iOS {version}");
            println!("CHAÎNE USB OK — plist + Connect byte-swapped + listener device validés.");
            Ok(())
        }

        "wcl-hello" => {
            let port: u16 = args.get(1).map(|value| value.parse()).transpose()?.unwrap_or(5321);
            let device = usbmux::first_usb_device()?;
            println!("Device: {} (ID {}), port {}", device.name, device.device_id, port);
            let mut tunnel = usbmux::connect_device(device.device_id, port)?;
            tunnel.set_read_timeout(Some(Duration::from_secs(8)))?;
            println!("Tunnel ouvert — envoi Hello…");

            tunnel.write_all(&Frame::new(MessageType::Hello, Hello::host().encode()).encode())?;

            let mut decoder = FrameDecoder::new();
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut buffer = [0u8; 64 * 1024];
            'outer: while Instant::now() < deadline {
                let count = tunnel.read(&mut buffer)?;
                if count == 0 {
                    println!("Tunnel fermé par le device.");
                    return Ok(());
                }
                for frame in decoder.push(&buffer[..count])? {
                    match frame.message_type {
                        MessageType::Hello => {
                            let hello = Hello::decode(&frame.payload)?;
                            println!(
                                "Hello reçu de l'iPhone — app={} name=« {} » model={} v{}–{}",
                                hello.app_version, hello.name, hello.model, hello.min_version, hello.max_version
                            );
                            break 'outer;
                        }
                        other => println!("Message {other:?} reçu, ignoré"),
                    }
                }
            }

            let stamp_us = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
            tunnel.write_all(&Frame::new(MessageType::Heartbeat, stamp_us.to_le_bytes().to_vec()).encode())?;

            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                let count = tunnel.read(&mut buffer)?;
                for frame in decoder.push(&buffer[..count])? {
                    if frame.message_type == MessageType::Heartbeat && frame.payload.len() >= 8 {
                        let echoed = u64::from_le_bytes(frame.payload[..8].try_into().unwrap());
                        println!("Heartbeat écho reçu (drift {} µs)", echoed.abs_diff(stamp_us));
                        println!("TUNNEL USB OK — round-trip WCLN complet validé.");
                        return Ok(());
                    }
                }
            }
            println!("Pas d'écho heartbeat reçu.");
            Ok(())
        }

        other => {
            eprintln!("Mode inconnu: {other} (attendu: list | lockdown | wcl-hello [port])");
            std::process::exit(2);
        }
    }
}
