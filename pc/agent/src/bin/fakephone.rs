//! `wcl-fakephone` — a fake iPhone for the WCLN protocol over plain TCP.
//!
//! Replays a WCLC clip produced by `wcl-mkh264` as if it were the iOS app:
//! Hello/Caps handshake, SetFormat/Start/Stop handling, VideoConfig +
//! VideoFrames paced at the clip rate, Status every 2 s, heartbeat echo.
//!
//! Usage: wcl-fakephone <clip.wclc> [--port 5321] [--once]

use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use wcl_agent::clip::{ClipFrame, ClipReader};
use wcl_agent::protocol::{
    encode_heartbeat, Caps, Camera, CameraFormat, Frame, FrameDecoder, Hello, ImageCaps,
    MessageType, ProtocolError, SetFormat, SetImage, Status, VideoConfig, FRAME_FLAG_KEYFRAME,
    IMAGE_CAPS_EXPOSURE_DURATION, IMAGE_CAPS_FOCUS_DISTANCE, IMAGE_CAPS_FOCUS_POINT,
    IMAGE_CAPS_ISO, IMAGE_CAPS_STABILIZATION, IMAGE_CAPS_TORCH, IMAGE_CAPS_TORCH_LEVEL,
    IMAGE_CAPS_WHITE_BALANCE, IMAGE_CAPS_ZOOM, LENS_WIDE, POSITION_BACK, STABILIZATION_CINEMATIC,
    STABILIZATION_OFF, STABILIZATION_STANDARD,
};

struct Device {
    header_caps: Caps,
    config: VideoConfig,
    frames: Vec<ClipFrame>,
    fps: u32,
}

/// The pro controls the fake device claims: enough for the whole PC chain
/// (agent, caps file, image settings window) to be exercised without an iPhone.
/// HDR is deliberately left out so the "unsupported" path stays reachable.
fn fake_image_caps() -> ImageCaps {
    ImageCaps {
        flags: IMAGE_CAPS_TORCH
            | IMAGE_CAPS_TORCH_LEVEL
            | IMAGE_CAPS_ZOOM
            | IMAGE_CAPS_ISO
            | IMAGE_CAPS_EXPOSURE_DURATION
            | IMAGE_CAPS_WHITE_BALANCE
            | IMAGE_CAPS_FOCUS_POINT
            | IMAGE_CAPS_FOCUS_DISTANCE
            | IMAGE_CAPS_STABILIZATION,
        zoom_max_x100: 800,
        iso_min: 25,
        iso_max: 3200,
        exposure_min_us: 25,
        exposure_max_us: 1_000_000,
        wb_min_k: 2000,
        wb_max_k: 9000,
        wb_tint_min_x100: -150,
        wb_tint_max_x100: 150,
        stabilization_modes: STABILIZATION_OFF | STABILIZATION_STANDARD | STABILIZATION_CINEMATIC,
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let mut path: Option<PathBuf> = None;
    let mut port = 5321u16;
    let mut once = false;

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--port" => port = arguments.next().context("--port attend un numéro")?.parse()?,
            "--once" => once = true,
            "--help" | "-h" => {
                println!("wcl-fakephone <clip.wclc> [--port 5321] [--once]");
                return Ok(());
            }
            other if !other.starts_with("--") => path = Some(PathBuf::from(other)),
            other => bail!("argument inconnu: {other}"),
        }
    }

    let path = path.context("usage: wcl-fakephone <clip.wclc> [--port 5321] [--once]")?;
    let mut reader = ClipReader::open(&path)?;
    let header = reader.header().clone();
    let mut frames = Vec::new();
    while let Some(frame) = reader.next_frame()? {
        frames.push(frame);
    }
    if frames.is_empty() {
        bail!("le clip ne contient aucune frame");
    }

    let device = Device {
        header_caps: Caps {
            cameras: vec![Camera {
                camera_id: 0,
                position: POSITION_BACK,
                lens: LENS_WIDE,
                label: "Fake iPhone (clip)".to_string(),
                formats: vec![CameraFormat {
                    width: header.width,
                    height: header.height,
                    max_fps: header.fps,
                }],
            }],
            codecs: wcl_agent::protocol::CAPS_CODEC_H264,
        },
        config: VideoConfig {
            codec: header.codec,
            width: header.width,
            height: header.height,
            fps: header.fps,
            parameter_sets: header.parameter_sets.clone(),
        },
        frames,
        fps: u32::from(header.fps.max(1)),
    };

    let listener = TcpListener::bind(("127.0.0.1", port)).context("bind")?;
    println!(
        "wcl-fakephone: {} frame(s) {}x{} @ {} fps, écoute sur 127.0.0.1:{port}",
        device.frames.len(),
        header.width,
        header.height,
        header.fps
    );

    for incoming in listener.incoming() {
        let mut stream = incoming.context("accept")?;
        let peer = stream.peer_addr().ok();
        println!("Client connecté ({peer:?})");
        match serve(&device, &mut stream) {
            Ok(()) => println!("Client déconnecté"),
            Err(error) => eprintln!("Session terminée: {error:#}"),
        }
        if once {
            break;
        }
    }
    Ok(())
}

fn serve(device: &Device, stream: &mut TcpStream) -> Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(20)))?;

    let mut decoder = FrameDecoder::new();
    let mut buffer = [0u8; 64 * 1024];

    let mut hello_done = false;
    let mut streaming = false;
    let mut frame_index = 0usize;
    let mut pts_us = 0u64;
    let mut next_frame_at = Instant::now();
    let mut last_status = Instant::now();
    let mut requested_fps = device.fps;
    let mut sent_bytes = 0u64;

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                for frame in decoder.push(&buffer[..count])? {
                    match frame.message_type {
                        MessageType::Hello => {
                            let hello = Hello::decode(&frame.payload)?;
                            log::info!("Hello reçu: {} ({})", hello.name, hello.model);
                            if !hello_done {
                                hello_done = true;
                                let reply = Hello {
                                    min_version: 1,
                                    max_version: 1,
                                    role: 1,
                                    app_version: "wcl-fakephone".to_string(),
                                    name: "Fake iPhone".to_string(),
                                    model: "iPhone-fake".to_string(),
                                };
                                send(stream, MessageType::Hello, reply.encode())?;
                                send(stream, MessageType::Caps, device.header_caps.encode())?;
                                send(stream, MessageType::ImageCaps, fake_image_caps().encode())?;
                            }
                        }
                        MessageType::SetCamera => {
                            let camera_id = frame.payload.first().copied().unwrap_or(0);
                            log::info!("SetCamera({camera_id})");
                            send(stream, MessageType::ImageCaps, fake_image_caps().encode())?;
                        }
                        MessageType::SetFormat => {
                            let requested = SetFormat::decode(&frame.payload)?;
                            log::info!(
                                "SetFormat {}x{} @ {} fps, codec {}, {} kbps",
                                requested.width,
                                requested.height,
                                requested.fps,
                                requested.codec,
                                requested.bitrate_kbps
                            );
                            if requested.width != device.config.width
                                || requested.height != device.config.height
                            {
                                send(
                                    stream,
                                    MessageType::Error,
                                    ProtocolError {
                                        code: 3,
                                        message: format!(
                                            "format {}x{} non supporté par le clip de test",
                                            requested.width, requested.height
                                        ),
                                    }
                                    .encode(),
                                )?;
                            } else if requested.fps > 0 {
                                requested_fps = u32::from(requested.fps).min(device.fps);
                            }
                        }
                        MessageType::SetOptions => log::info!("SetOptions"),
                        MessageType::SetImage => {
                            let image = SetImage::default().decode_into(&frame.payload)?;
                            log::info!(
                                "SetImage: zoom {}.{:02}× · expo mode {} biais {} · ISO {} · {} µs · \
                                 focus mode {} ({}, {}) · WB mode {} {} K teinte {} · torche {} {} % · \
                                 stab {} · HDR {}",
                                image.zoom_x100 / 100,
                                image.zoom_x100 % 100,
                                image.exposure_mode,
                                image.exposure_bias_ev_x100,
                                image.iso,
                                image.exposure_duration_us,
                                image.focus_mode,
                                image.focus_x_x10000,
                                image.focus_y_x10000,
                                image.wb_mode,
                                image.wb_temp_k,
                                image.wb_tint_x100,
                                image.torch,
                                image.torch_level,
                                image.stabilization,
                                image.hdr
                            );
                        }
                        MessageType::RequestKeyframe => {
                            // The clip already carries an IDR every couple of
                            // seconds; nothing to do beyond logging.
                            log::info!("RequestKeyframe (le clip fournit ses IDR périodiques)");
                        }
                        MessageType::Start => {
                            log::info!("Start: flux {}x{} @ {requested_fps} fps", device.config.width, device.config.height);
                            streaming = true;
                            frame_index = 0;
                            pts_us = 0;
                            next_frame_at = Instant::now();
                            last_status = Instant::now();
                            send(stream, MessageType::VideoConfig, device.config.encode())?;
                        }
                        MessageType::Stop => {
                            log::info!("Stop");
                            streaming = false;
                        }
                        MessageType::Heartbeat => {
                            let stamp = wcl_agent::protocol::decode_heartbeat(&frame.payload)?;
                            send(stream, MessageType::Heartbeat, encode_heartbeat(stamp))?;
                        }
                        other => log::debug!("message {other:?} ignoré"),
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }

        if streaming && Instant::now() >= next_frame_at {
            let interval = Duration::from_secs_f64(1.0 / f64::from(requested_fps.max(1)));
            next_frame_at += interval;
            if next_frame_at < Instant::now() {
                next_frame_at = Instant::now() + interval;
            }
            let frame = &device.frames[frame_index % device.frames.len()];
            let video_frame = wcl_agent::protocol::VideoFrame {
                pts_us,
                flags: if frame.flags & FRAME_FLAG_KEYFRAME != 0 { FRAME_FLAG_KEYFRAME } else { 0 },
                data: frame.data.clone(),
            };
            let bytes = Frame::new(MessageType::VideoFrame, video_frame.encode()).encode();
            stream.write_all(&bytes)?;
            sent_bytes += bytes.len() as u64;
            frame_index += 1;
            pts_us += (1_000_000.0 / f64::from(requested_fps.max(1))) as u64;

            if last_status.elapsed() >= Duration::from_secs(2) {
                let elapsed = last_status.elapsed().as_secs_f64().max(0.5);
                let status = Status {
                    thermal_state: 0,
                    battery_percent: 100,
                    charging: true,
                    actual_bitrate_kbps: (sent_bytes as f64 * 8.0 / 1000.0 / elapsed) as u32,
                    actual_fps_x100: (requested_fps * 100) as u16,
                    camera_id: 0,
                };
                sent_bytes = 0;
                last_status = Instant::now();
                send(stream, MessageType::Status, status.encode())?;
            }
        }
    }
}

fn send(stream: &mut TcpStream, message_type: MessageType, payload: Vec<u8>) -> Result<()> {
    stream.write_all(&Frame::new(message_type, payload).encode())?;
    Ok(())
}
