//! `wcl-agent` — the PC side daemon for the virtual camera (P2).
//!
//! Waits for the virtual camera DLL (Frame Server) to create the shared memory
//! block, then drives the iPhone session on demand: `Start` when an application
//! opens the camera, `Stop` 2 s after the last consumer goes away, retrying
//! every second while the consumer is active.

use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};
use wcl_agent::decode::H264Decoder;
use wcl_agent::mf::MediaFoundation;
use wcl_agent::protocol::{Caps, ProtocolError, SetFormat, SetOptions, CODEC_H264};
use wcl_agent::session::{Event, Session, SessionError};
use wcl_agent::shm::{
    Control, ShmBlock, ShmNames, PRODUCER_ERROR, PRODUCER_STOPPED, PRODUCER_STREAMING,
};

struct Options {
    tcp: Option<String>,
    port: u16,
    camera: Option<u8>,
    format: Option<(u16, u16)>,
    fps: Option<u16>,
    bitrate_kbps: u32,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        tcp: None,
        port: 5321,
        camera: None,
        format: None,
        fps: None,
        bitrate_kbps: 20_000,
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
            "--help" | "-h" => {
                println!(
                    "wcl-agent [--usb | --tcp host:port] [--port 5321] [--camera N] [--format WxH] [--fps N] [--bitrate K]"
                );
                std::process::exit(0);
            }
            other => bail!("argument inconnu: {other}"),
        }
    }
    Ok(options)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let options = parse_options()?;
    let _mf = MediaFoundation::start().context("initialisation Media Foundation")?;
    let names = ShmNames::global();

    log::info!("wcl-agent démarré — en attente de la webcam virtuelle (mémoire partagée)");

    let mut shm: Option<ShmBlock> = None;
    let mut session: Option<Session> = None;
    let mut decoder: Option<H264Decoder> = None;
    let mut last_tick = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut idle_since: Option<Instant> = None;
    let mut retry_at: Option<Instant> = None;
    let mut published = 0u64;

    loop {
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();

            if shm.is_none() {
                match ShmBlock::open(&names) {
                    Ok(Some(block)) => {
                        log::info!("mémoire partagée détectée (caméra ouverte par une application)");
                        shm = Some(block);
                    }
                    Ok(None) => {}
                    Err(error) => log::warn!("mémoire partagée illisible: {error:#}"),
                }
            }

            match shm.as_mut() {
                Some(block) => {
                    let control = block.control();
                    if control.consumer_active {
                        idle_since = None;
                        if session.is_none() && retry_at.is_none_or(|at| Instant::now() >= at) {
                            match start_stream(&options, &control) {
                                Ok((mut new_session, new_decoder)) => {
                                    log::info!(
                                        "flux démarré pour la webcam (demande {}x{}@{} fps)",
                                        control.width,
                                        control.height,
                                        control.fps
                                    );
                                    let _ = new_session.heartbeat();
                                    session = Some(new_session);
                                    decoder = Some(new_decoder);
                                    last_heartbeat = Instant::now();
                                }
                                Err(error) => {
                                    log::warn!("démarrage impossible: {error:#} — nouvel essai dans 1 s");
                                    retry_at = Some(Instant::now() + Duration::from_secs(1));
                                }
                            }
                        }
                    } else {
                        let since = *idle_since.get_or_insert(Instant::now());
                        if session.is_some() && since.elapsed() >= Duration::from_secs(2) {
                            log::info!("plus de consommateur depuis 2 s — arrêt du flux");
                            if let Some(mut active) = session.take() {
                                let _ = active.stop();
                            }
                            decoder = None;
                            block.set_producer_state(PRODUCER_STOPPED, 0);
                        }
                    }
                }
                None => {
                    if session.is_some() {
                        log::info!("mémoire partagée disparue — arrêt du flux");
                        if let Some(mut active) = session.take() {
                            let _ = active.stop();
                        }
                        decoder = None;
                    }
                }
            }
        }

        let mut drop_session = false;
        {
            let Some(active) = session.as_mut() else {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            };

            match active.next_event(Duration::from_millis(4)) {
                Ok(Some(Event::VideoConfig(config))) => {
                    log::info!(
                        "VideoConfig: {}x{} @ {} fps, {} octets de paramètres",
                        config.width,
                        config.height,
                        config.fps,
                        config.parameter_sets.len()
                    );
                    if let Some(decoder) = decoder.as_mut() {
                        decoder.configure(
                            u32::from(config.width),
                            u32::from(config.height),
                            u32::from(config.fps).max(1),
                        )?;
                    }
                    if let Some(block) = shm.as_mut() {
                        block.set_video_format(u32::from(config.width), u32::from(config.height));
                        block.set_producer_state(PRODUCER_STREAMING, 0);
                    }
                }
                Ok(Some(Event::VideoFrame(frame))) => {
                    if let Some(decoder) = decoder.as_mut() {
                        let decoded = decoder.feed(&frame.data, frame.pts_us)?;
                        if let Some(block) = shm.as_mut() {
                            for picture in decoded {
                                block
                                    .publish(&picture.nv12, picture.pts_us.unwrap_or(frame.pts_us))?;
                                published += 1;
                                if published % 300 == 0 {
                                    let stats = block.stats();
                                    log::debug!(
                                        "{} frames publiées ({} écrasées)",
                                        stats.published,
                                        stats.dropped
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(Some(Event::Status(status))) => log::debug!("status: {:?}", status),
                Ok(Some(Event::Error(error))) => {
                    report_error(&error);
                    if let Some(block) = shm.as_mut() {
                        block.set_producer_state(PRODUCER_ERROR, u32::from(error.code));
                    }
                    drop_session = true;
                    retry_at = Some(Instant::now() + Duration::from_secs(1));
                }
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(SessionError::Closed) => {
                    log::warn!("connexion à l'iPhone fermée — reconnexion dans 1 s");
                    drop_session = true;
                    retry_at = Some(Instant::now() + Duration::from_secs(1));
                }
                Err(error) => {
                    log::warn!("session interrompue: {error} — reconnexion dans 1 s");
                    drop_session = true;
                    retry_at = Some(Instant::now() + Duration::from_secs(1));
                }
            }

            if last_heartbeat.elapsed() >= Duration::from_secs(1) {
                last_heartbeat = Instant::now();
                let _ = active.heartbeat();
            }
        }

        if drop_session {
            session = None;
            decoder = None;
        }
    }
}

fn start_stream(options: &Options, control: &Control) -> Result<(Session, H264Decoder)> {
    let mut session = match &options.tcp {
        Some(address) => Session::connect_tcp(address)?,
        None => Session::connect_usb(options.port)?,
    };
    session
        .handshake(Duration::from_secs(5))
        .map_err(|error| anyhow::anyhow!("handshake: {error}"))?;
    let caps = session
        .wait_caps(Duration::from_secs(5))
        .map_err(|error| anyhow::anyhow!("Caps: {error}"))?;
    if !caps.supports_h264() {
        bail!("l'iPhone n'annonce pas H.264");
    }

    let (camera_id, (width, height), fps) = choose(&caps, options, control)?;
    session.set_camera(camera_id)?;
    session.set_options(SetOptions { mirror: false, rotation: 0, dim_preview: true })?;
    session.set_format(SetFormat {
        width,
        height,
        fps,
        codec: CODEC_H264,
        bitrate_kbps: options.bitrate_kbps,
    })?;
    session.start()?;
    log::info!("session iPhone: caméra {camera_id}, {width}x{height} @ {fps} fps");

    Ok((session, H264Decoder::new(false)?))
}

fn choose(caps: &Caps, options: &Options, control: &Control) -> Result<(u8, (u16, u16), u16)> {
    let camera = match options.camera {
        Some(id) if caps.cameras.iter().any(|camera| camera.camera_id == id) => {
            caps.cameras.iter().find(|camera| camera.camera_id == id).unwrap()
        }
        Some(id) => bail!("caméra {id} absente de Caps"),
        None => caps.cameras.first().context("Caps ne contient aucune caméra")?,
    };

    let requested = options.format.or_else(|| {
        if control.width >= 160 && control.height >= 120 {
            Some((control.width as u16, control.height as u16))
        } else {
            None
        }
    });

    let format = match requested {
        Some(wanted) => camera
            .formats
            .iter()
            .find(|format| (format.width, format.height) == wanted)
            .or_else(|| {
                camera
                    .formats
                    .iter()
                    .filter(|format| format.width <= 1920 && format.height <= 1080)
                    .max_by_key(|format| u32::from(format.width) * u32::from(format.height))
            })
            .context("aucun format utilisable")?,
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

    let wanted_fps = options
        .fps
        .or_else(|| if control.fps > 0 { Some(control.fps as u16) } else { None })
        .unwrap_or(30);
    let fps = u32::from(wanted_fps).min(u32::from(format.max_fps)).max(1);
    Ok((camera.camera_id, (format.width, format.height), fps as u16))
}

fn report_error(error: &ProtocolError) {
    match error.code_label() {
        Some(code) => log::warn!("erreur iPhone [{:?}] {} — {}", code, code.label(), error.message),
        None => log::warn!("erreur iPhone [{}] {}", error.code, error.message),
    }
}
