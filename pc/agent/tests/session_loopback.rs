//! Loopback integration test: a fake device speaks WCLN over TCP to a real
//! `Session`, exercising handshake, caps, format negotiation, streaming,
//! heartbeat and stop — no iPhone required.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};
use wcl_agent::protocol::{
    self, Caps, Camera, CameraFormat, Frame, FrameDecoder, Hello, MessageType, ProtocolError,
    SetFormat, Status, VideoConfig, VideoFrame, CAPS_CODEC_H264, CODEC_H264, FRAME_FLAG_KEYFRAME,
    LENS_WIDE, POSITION_BACK,
};
use wcl_agent::session::{Event, Session};

const FRAME_COUNT: u32 = 5;

fn sample_device_caps() -> Caps {
    Caps {
        cameras: vec![Camera {
            camera_id: 0,
            position: POSITION_BACK,
            lens: LENS_WIDE,
            label: "Back Camera".to_string(),
            formats: vec![
                CameraFormat { width: 1920, height: 1080, max_fps: 60 },
                CameraFormat { width: 3840, height: 2160, max_fps: 30 },
            ],
        }],
        codecs: CAPS_CODEC_H264,
    }
}

/// A minimal device: handshake, then answer Start with a short stream.
fn fake_device(stream: &mut TcpStream) {
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut announced = false;
    let mut streaming = false;
    let mut frame_index = 0u32;
    let mut sent_frames = 0u32;
    let mut last_send = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                for frame in decoder.push(&buffer[..count]).unwrap() {
                    match frame.message_type {
                        MessageType::Hello => {
                            if !announced {
                                announced = true;
                                let hello = Hello {
                                    min_version: 1,
                                    max_version: 1,
                                    role: 1,
                                    app_version: "test-device".to_string(),
                                    name: "Fake iPhone".to_string(),
                                    model: "iPhone-test".to_string(),
                                };
                                write_frame(stream, MessageType::Hello, hello.encode());
                                write_frame(stream, MessageType::Caps, sample_device_caps().encode());
                            }
                        }
                        MessageType::SetFormat => {
                            let format = SetFormat::decode(&frame.payload).unwrap();
                            assert_eq!(format.width, 1920, "le PC doit demander le 1080p du test");
                            assert_eq!(format.codec, CODEC_H264);
                        }
                        MessageType::Start => {
                            streaming = true;
                            announced = true;
                            let config = VideoConfig {
                                codec: CODEC_H264,
                                width: 1920,
                                height: 1080,
                                fps: 30,
                                parameter_sets: vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE],
                            };
                            write_frame(stream, MessageType::VideoConfig, config.encode());
                            last_send = Instant::now() - Duration::from_millis(33);
                        }
                        MessageType::Heartbeat => {
                            write_frame(stream, MessageType::Heartbeat, frame.payload);
                        }
                        MessageType::Stop => {
                            streaming = false;
                        }
                        other => panic!("message inattendu: {other:?}"),
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => panic!("erreur de lecture du device: {error}"),
        }

        if streaming && sent_frames < FRAME_COUNT && last_send.elapsed() >= Duration::from_millis(10)
        {
            last_send = Instant::now();
            let keyframe = frame_index.is_multiple_of(2);
            let video_frame = VideoFrame {
                pts_us: u64::from(frame_index) * 33_333,
                flags: if keyframe { FRAME_FLAG_KEYFRAME } else { 0 },
                data: vec![0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 0x88, 0x84],
            };
            write_frame(stream, MessageType::VideoFrame, video_frame.encode());
            frame_index += 1;
            sent_frames += 1;
            if sent_frames == FRAME_COUNT {
                let status = Status {
                    thermal_state: 0,
                    battery_percent: 77,
                    charging: true,
                    actual_bitrate_kbps: 12_000,
                    actual_fps_x100: 3000,
                    camera_id: 0,
                };
                write_frame(stream, MessageType::Status, status.encode());
                let error = ProtocolError { code: 4, message: "test terminé".to_string() };
                write_frame(stream, MessageType::Error, error.encode());
            }
        }
    }
}

fn write_frame(stream: &mut TcpStream, message_type: MessageType, payload: Vec<u8>) {
    stream.write_all(&Frame::new(message_type, payload).encode()).unwrap();
}

#[test]
fn session_talks_to_a_fake_device() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let device = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        fake_device(&mut stream);
    });

    let mut session = Session::connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
    let hello = session.handshake(Duration::from_secs(3)).unwrap();
    assert_eq!(hello.name, "Fake iPhone");
    assert_eq!(hello.role, 1);

    let caps = session.wait_caps(Duration::from_secs(3)).unwrap();
    assert_eq!(caps.cameras.len(), 1);
    assert!(caps.supports_h264());
    assert_eq!(caps.cameras[0].formats[1].width, 3840);

    session
        .set_format(SetFormat { width: 1920, height: 1080, fps: 30, codec: CODEC_H264, bitrate_kbps: 20_000 })
        .unwrap();
    session.start().unwrap();

    let mut config: Option<VideoConfig> = None;
    let mut frames = Vec::new();
    let mut status: Option<Status> = None;
    let mut error: Option<ProtocolError> = None;
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline && error.is_none() {
        match session.next_event_until(deadline).unwrap() {
            Some(Event::VideoConfig(received)) => {
                assert!(config.is_none(), "un seul VideoConfig attendu");
                config = Some(received);
            }
            Some(Event::VideoFrame(frame)) => frames.push(frame),
            Some(Event::Status(received)) => status = Some(received),
            Some(Event::Error(received)) => error = Some(received),
            Some(other) => panic!("événement inattendu: {other:?}"),
            None => break,
        }
    }

    let config = config.expect("VideoConfig reçu");
    assert_eq!(config.width, 1920);
    assert!(!config.parameter_sets.is_empty());
    assert_eq!(frames.len() as u32, FRAME_COUNT, "toutes les frames du device");
    assert!(frames[0].is_keyframe(), "la première frame est un IDR");
    assert_eq!(frames[1].pts_us, 33_333);
    assert!(!frames[1].is_keyframe());
    assert_eq!(status.expect("Status reçu").battery_percent, 77);
    assert_eq!(error.expect("Error reçu").code_label(), Some(protocol::ErrorCode::CameraUnavailable));

    let stamp = session.heartbeat().unwrap();
    match session.next_event(Duration::from_secs(3)).unwrap() {
        Some(Event::Heartbeat(echoed)) => assert_eq!(echoed, stamp, "le device renvoie notre horodatage"),
        other => panic!("écho heartbeat attendu, reçu {other:?}"),
    }

    session.stop().unwrap();
    drop(session);
    device.join().unwrap();
}

#[test]
fn session_reports_closed_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let device = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
    });

    let mut session = Session::connect_tcp(&format!("127.0.0.1:{port}")).unwrap();
    let error = session.handshake(Duration::from_secs(2)).unwrap_err();
    assert!(matches!(error, wcl_agent::session::SessionError::Closed), "attendu Closed, reçu {error:?}");
    device.join().unwrap();
}
