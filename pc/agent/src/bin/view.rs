//! `wcl-view` — PC side of WebcamLink for P1: connect (USB or TCP), negotiate
//! the stream, decode H.264 with Media Foundation and show a preview window
//! with live stats.
//!
//! Usage:
//!   wcl-view [--usb] [--port 5321]              tunnel through usbmuxd (real iPhone)
//!   wcl-view --tcp 127.0.0.1:5321               plain TCP (wcl-fakephone)
//!   wcl-view [--camera N] [--format 1920x1080] [--fps 30] [--bitrate 20000]
//!            [--seconds N] [--no-window] [--dump-frame out.bmp]

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use wcl_agent::clip::{ClipFrame, ClipHeader, ClipWriter};
use wcl_agent::color::{nv12_to_bgra, write_bmp};
use wcl_agent::decode::H264Decoder;
use wcl_agent::mf::MediaFoundation;
use wcl_agent::preview::PreviewWindow;
use wcl_agent::protocol::{Caps, ProtocolError, SetFormat, SetOptions, Status, CODEC_H264};
use wcl_agent::session::{Event, Session, SessionError, DEFAULT_PORT};
use wcl_agent::usbmux;

struct Options {
    tcp: Option<String>,
    port: u16,
    camera: Option<u8>,
    format: Option<(u16, u16)>,
    fps: Option<u16>,
    bitrate_kbps: u32,
    seconds: Option<u64>,
    window: bool,
    dump_frame: Option<PathBuf>,
    dump_index: u64,
    dump_luma: Option<PathBuf>,
    dump_clip: Option<PathBuf>,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        tcp: None,
        port: DEFAULT_PORT,
        camera: None,
        format: None,
        fps: None,
        bitrate_kbps: 20_000,
        seconds: None,
        window: true,
        dump_frame: None,
        dump_index: 0,
        dump_luma: None,
        dump_clip: None,
    };

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--tcp" => options.tcp = Some(arguments.next().context("--tcp attend host:port")?),
            "--usb" => options.tcp = None,
            "--port" => options.port = arguments.next().context("--port attend un numéro")?.parse()?,
            "--camera" => options.camera = Some(arguments.next().context("--camera attend un id")?.parse()?),
            "--format" => {
                let value = arguments.next().context("--format attend WxH")?;
                let (width, height) = value.split_once('x').context("--format attend WxH")?;
                options.format = Some((width.parse()?, height.parse()?));
            }
            "--fps" => options.fps = Some(arguments.next().context("--fps attend un nombre")?.parse()?),
            "--bitrate" => options.bitrate_kbps = arguments.next().context("--bitrate attend des kbps")?.parse()?,
            "--seconds" => options.seconds = Some(arguments.next().context("--seconds attend un nombre")?.parse()?),
            "--no-window" => options.window = false,
            "--dump-frame" => options.dump_frame = Some(PathBuf::from(arguments.next().context("--dump-frame attend un chemin")?)),
            "--dump-index" => options.dump_index = arguments.next().context("--dump-index attend un index")?.parse()?,
            "--dump-luma" => options.dump_luma = Some(PathBuf::from(arguments.next().context("--dump-luma attend un chemin")?)),
            "--dump-clip" => options.dump_clip = Some(PathBuf::from(arguments.next().context("--dump-clip attend un chemin")?)),
            "--help" | "-h" => {
                println!("{}", HELP);
                std::process::exit(0);
            }
            other => bail!("argument inconnu: {other}"),
        }
    }
    Ok(options)
}

const HELP: &str = "\
wcl-view — aperçu WebcamLink (P1)

  --usb                 tunnel usbmuxd vers l'iPhone (défaut)
  --tcp host:port       TCP direct (wcl-fakephone)
  --port N              port WCLN sur l'iPhone (défaut 5321)
  --camera N            id de caméra (défaut: premier de Caps)
  --format WxH          format demandé (défaut: meilleur ≤ 1080p)
  --fps N               images/s demandées (défaut 30)
  --bitrate K           débit demandé en kbps (défaut 20000)
  --seconds N           quitter après N secondes
  --no-window           pas d'aperçu (mesures seules)
  --dump-frame FILE.bmp enregistrer une frame décodée (voir --dump-index)
  --dump-index N        index de la frame à enregistrer (défaut 0)
  --dump-luma FILE.bmp  enregistrer la luma (niveaux de gris) de cette frame
  --dump-clip FILE.wclc enregistrer le flux reçu (clip WCLC rejouable)";

/// Pick a camera + format pair from the announced caps.
fn choose(caps: &Caps, options: &Options) -> Result<(u8, (u16, u16), u16)> {
    let camera = match options.camera {
        Some(id) if caps.cameras.iter().any(|camera| camera.camera_id == id) => {
            caps.cameras.iter().find(|camera| camera.camera_id == id).unwrap()
        }
        Some(id) => bail!("caméra {id} absente de Caps"),
        None => caps.cameras.first().context("Caps ne contient aucune caméra")?,
    };

    let candidate = match options.format {
        Some(requested) => camera
            .formats
            .iter()
            .find(|format| (format.width, format.height) == requested)
            .with_context(|| {
                format!("format {}x{} non annoncé par l'iPhone", requested.0, requested.1)
            })?,
        None => camera
            .formats
            .iter()
            .filter(|format| format.width <= 1920 && format.height <= 1080)
            .max_by_key(|format| u32::from(format.width) * u32::from(format.height))
            .or_else(|| {
                camera
                    .formats
                    .iter()
                    .min_by_key(|format| u32::from(format.width) * u32::from(format.height))
            })
            .context("aucun format utilisable")?,
    };

    let fps = u32::from(options.fps.unwrap_or(30)).min(u32::from(candidate.max_fps)).max(1);
    Ok((camera.camera_id, (candidate.width, candidate.height), fps as u16))
}

fn print_caps(caps: &Caps) {
    println!("Caps: {} caméra(s), codecs: {}", caps.cameras.len(), {
        let mut names = Vec::new();
        if caps.supports_h264() {
            names.push("H.264");
        }
        if caps.supports_hevc() {
            names.push("HEVC");
        }
        names.join(" + ")
    });
    for camera in &caps.cameras {
        let lens = match camera.lens {
            wcl_agent::protocol::LENS_WIDE => "large",
            wcl_agent::protocol::LENS_ULTRA_WIDE => "ultra-grand-angle",
            wcl_agent::protocol::LENS_TELE => "télé",
            wcl_agent::protocol::LENS_FRONT => "avant",
            _ => "autre",
        };
        let position = if camera.position == wcl_agent::protocol::POSITION_FRONT { "avant" } else { "arrière" };
        println!("  [{}] {} ({position}, {lens}) — {} format(s)", camera.camera_id, camera.label, camera.formats.len());
        for format in &camera.formats {
            println!("      {}x{} @ {} fps", format.width, format.height, format.max_fps);
        }
    }
}

#[derive(Default)]
struct Stats {
    frames: u64,
    bytes: u64,
    decode_ms_total: f64,
    decode_ms_max: f64,
    convert_ms_total: f64,
    frames_in_window: u64,
    bytes_in_window: u64,
    last_report: Option<Instant>,
    last_status: Option<Status>,
    skipped: u64,
    pts_last: Option<u64>,
}

impl Stats {
    fn report(&mut self, preview: &Option<PreviewWindow>) -> String {
        let now = Instant::now();
        let last = *self.last_report.get_or_insert(now);
        let elapsed = now.duration_since(last).as_secs_f64();
        if elapsed < 1.0 {
            return String::new();
        }

        let fps = self.frames_in_window as f64 / elapsed;
        let mbps = self.bytes_in_window as f64 * 8.0 / 1_000_000.0 / elapsed;
        let decode_avg = if self.frames > 0 { self.decode_ms_total / self.frames as f64 } else { 0.0 };
        let convert_avg = if self.frames > 0 { self.convert_ms_total / self.frames as f64 } else { 0.0 };

        let line = format!(
            "{fps:.1} fps · {mbps:.1} Mbit/s · décodage {decode_avg:.1} ms (max {:.1}) · rendu {convert_avg:.1} ms · {} frames",
            self.decode_ms_max, self.frames
        );

        if let Some(status) = self.last_status {
            let battery = if status.battery_percent == 255 {
                "?".to_string()
            } else {
                format!("{}%{}", status.battery_percent, if status.charging { "+" } else { "" })
            };
            let overlay = format!(
                "WebcamLink — {line}\niPhone: thermique {} · batterie {battery} · débit réel {} kbps · fps réel {:.1}\n(Échap: fermer la fenêtre)",
                status.thermal_label(),
                status.actual_bitrate_kbps,
                status.actual_fps(),
            );
            if let Some(preview) = preview {
                preview.set_stats(overlay);
            }
        } else if let Some(preview) = preview {
            preview.set_stats(format!("WebcamLink — {line}"));
        }

        self.last_report = Some(now);
        self.frames_in_window = 0;
        self.bytes_in_window = 0;
        line
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let options = parse_options()?;

    let _mf = MediaFoundation::start().context("initialisation Media Foundation")?;

    let mut session = match &options.tcp {
        Some(address) => {
            println!("Connexion TCP à {address}…");
            Session::connect_tcp(address)?
        }
        None => {
            let device = usbmux::first_usb_device()?;
            println!("Tunnel usbmuxd vers {} (port {})…", device.name, options.port);
            Session::connect_usb(options.port)?
        }
    };

    let hello = session
        .handshake(Duration::from_secs(5))
        .map_err(|error| anyhow::anyhow!("handshake: {error}"))?;
    println!("Connecté à {} ({}) — app {}", hello.name, hello.model, hello.app_version);

    let caps = session
        .wait_caps(Duration::from_secs(5))
        .map_err(|error| anyhow::anyhow!("Caps: {error}"))?;
    print_caps(&caps);
    if !caps.supports_h264() {
        bail!("l'iPhone n'annonce pas H.264 — seul codec supporté par le décodeur P1");
    }

    let (camera_id, (width, height), fps) = choose(&caps, &options)?;
    println!("Sélection: caméra {camera_id}, {width}x{height} @ {fps} fps, H.264, {} kbps", options.bitrate_kbps);

    session.set_camera(camera_id)?;
    session.set_options(SetOptions { mirror: false, rotation: 0, dim_preview: false })?;
    session.set_format(SetFormat { width, height, fps, codec: CODEC_H264, bitrate_kbps: options.bitrate_kbps })?;
    session.start()?;

    let preview = if options.window {
        Some(PreviewWindow::create(
            &format!("WebcamLink — {} — {width}x{height}", hello.name),
            u32::from(width),
            u32::from(height),
        )?)
    } else {
        None
    };

    let mut decoder: Option<H264Decoder> = None;
    let mut clip_writer: Option<ClipWriter> = None;
    let mut stats = Stats::default();
    let mut bgra = Vec::new();
    let mut last_heartbeat = Instant::now();
    let deadline = options.seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds));
    let mut dumped = false;
    let mut exit_error: Option<anyhow::Error> = None;

    'main: loop {
        if let Some(window) = &preview {
            if !window.pump() {
                println!("Fenêtre fermée.");
                break 'main;
            }
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                break 'main;
            }
        }

        match session.next_event(Duration::from_millis(4)) {
            Ok(Some(Event::VideoConfig(config))) => {
                println!(
                    "VideoConfig: codec {}, {}x{} @ {} fps, {} octets de paramètres",
                    config.codec, config.width, config.height, config.fps, config.parameter_sets.len()
                );
                if config.codec != CODEC_H264 {
                    exit_error = Some(anyhow::anyhow!("codec {} non supporté en P1", config.codec));
                    break 'main;
                }
                let mut created = H264Decoder::new(false)?;
                created.configure(u32::from(config.width), u32::from(config.height), u32::from(config.fps).max(1))?;
                decoder = Some(created);

                if let Some(path) = &options.dump_clip {
                    let header = ClipHeader {
                        codec: config.codec,
                        width: config.width,
                        height: config.height,
                        fps: config.fps,
                        parameter_sets: config.parameter_sets.clone(),
                    };
                    clip_writer = Some(ClipWriter::create(path, &header)?);
                    println!("Flux enregistré dans {} (clip WCLC)", path.display());
                }
            }
            Ok(Some(Event::VideoFrame(frame))) => {
                if let Some(writer) = clip_writer.as_mut() {
                    writer.write_frame(&ClipFrame {
                        pts_us: frame.pts_us,
                        flags: frame.flags,
                        data: frame.data.clone(),
                    })?;
                }
                if let Some(decoder) = decoder.as_mut() {
                    let started = Instant::now();
                    let decoded = decoder.feed(&frame.data, frame.pts_us)?;
                    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
                    stats.decode_ms_total += decode_ms;
                    stats.decode_ms_max = stats.decode_ms_max.max(decode_ms);
                    stats.frames += 1;
                    stats.frames_in_window += 1;
                    stats.bytes += frame.data.len() as u64;
                    stats.bytes_in_window += frame.data.len() as u64;
                    if let Some(last) = stats.pts_last {
                        if frame.pts_us <= last {
                            log::warn!("pts non monotone: {} après {}", frame.pts_us, last);
                        }
                    }
                    stats.pts_last = Some(frame.pts_us);

                    for decoded_frame in decoded {
                        let convert_started = Instant::now();
                        nv12_to_bgra(
                            &decoded_frame.nv12,
                            decoded_frame.stride_y,
                            decoded_frame.stride_uv,
                            decoded_frame.width as usize,
                            decoded_frame.height as usize,
                            &mut bgra,
                        );
                        stats.convert_ms_total += convert_started.elapsed().as_secs_f64() * 1000.0;

                        if !dumped && stats.frames > options.dump_index {
                            dumped = true;
                            if let Some(path) = &options.dump_frame {
                                write_bmp(path, &bgra, decoded_frame.width, decoded_frame.height)?;
                                println!(
                                    "Frame {} décodée écrite dans {}",
                                    stats.frames,
                                    path.display()
                                );
                            }

                            let luma_len = (decoded_frame.width as usize)
                                .saturating_mul(decoded_frame.height as usize)
                                .min(decoded_frame.nv12.len());
                            let luma = &decoded_frame.nv12[..luma_len];
                            let minimum = luma.iter().copied().min().unwrap_or(0);
                            let maximum = luma.iter().copied().max().unwrap_or(0);
                            let mean = luma.iter().map(|value| f64::from(*value)).sum::<f64>()
                                / (luma.len().max(1) as f64);
                            println!(
                                "NV12 {}x{} · stride_y={} stride_uv={} · {} octets · luma min={} max={} moyenne={:.1}",
                                decoded_frame.width,
                                decoded_frame.height,
                                decoded_frame.stride_y,
                                decoded_frame.stride_uv,
                                decoded_frame.nv12.len(),
                                minimum,
                                maximum,
                                mean
                            );

                            if let Some(path) = &options.dump_luma {
                                let mut gray = Vec::with_capacity(luma_len * 4);
                                for value in luma {
                                    gray.extend_from_slice(&[*value, *value, *value, 255]);
                                }
                                write_bmp(path, &gray, decoded_frame.width, decoded_frame.height)?;
                                println!("Luma écrite dans {}", path.display());
                            }
                        }
                        if let Some(window) = &preview {
                            window.present(bgra.clone(), decoded_frame.width, decoded_frame.height);
                        }
                    }
                } else {
                    stats.skipped += 1;
                    log::debug!("frame reçue avant VideoConfig — ignorée");
                }
            }
            Ok(Some(Event::Status(status))) => {
                stats.last_status = Some(status);
            }
            Ok(Some(Event::Error(error))) => {
                print_protocol_error(&error);
                exit_error = Some(anyhow::anyhow!("le device a signalé une erreur"));
                break 'main;
            }
            Ok(Some(Event::Heartbeat(_))) => {}
            Ok(Some(Event::Hello(_)) | Some(Event::Caps(_))) => {}
            Ok(None) => {}
            Err(SessionError::Timeout) => {}
            Err(error) => {
                exit_error = Some(anyhow::anyhow!("session: {error}"));
                break 'main;
            }
        }

        if last_heartbeat.elapsed() >= Duration::from_secs(1) {
            last_heartbeat = Instant::now();
            session.heartbeat()?;
        }

        let report = stats.report(&preview);
        if !report.is_empty() {
            println!("{report}");
        }
    }

    if let Some(window) = &preview {
        println!("Frames affichées: {}", window.frames());
    }
    let _ = session.stop();
    if stats.skipped > 0 {
        println!("{} frame(s) ignorée(s) faute de VideoConfig", stats.skipped);
    }

    if let Some(writer) = clip_writer.take() {
        let frames = writer.finish()?;
        println!("Clip enregistré : {frames} frame(s)");
    }

    match exit_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn print_protocol_error(error: &ProtocolError) {
    match error.code_label() {
        Some(code) => eprintln!("Erreur device [{:?}] {} — {}", code, code.label(), error.message),
        None => eprintln!("Erreur device [{}] {}", error.code, error.message),
    }
}
