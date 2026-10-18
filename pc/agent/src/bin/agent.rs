//! `wcl-agent` — the PC side daemon for the virtual camera (P2).
//!
//! Waits for the virtual camera DLL (Frame Server) to create the shared memory
//! block, then drives the iPhone session on demand: `Start` when an application
//! opens the camera, `Stop` 2 s after the last consumer goes away, retrying
//! every second while the consumer is active.

use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};
use wcl_agent::config::{CameraCaps, CameraPreference, Settings};
use wcl_agent::decode::H264Decoder;
use wcl_agent::mf::MediaFoundation;
use wcl_agent::protocol::{
    Caps, Camera, ErrorCode, ImageCaps, ProtocolError, SetFormat, SetImage, SetOptions, CODEC_H264,
    LENS_TELE, LENS_ULTRA_WIDE, LENS_WIDE, POSITION_FRONT,
};
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
    bitrate_kbps: Option<u32>,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        tcp: None,
        port: 5321,
        camera: None,
        format: None,
        fps: None,
        bitrate_kbps: None,
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
            "--bitrate" => {
                options.bitrate_kbps =
                    Some(arguments.next().context("--bitrate attend des kbps")?.parse()?)
            }
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
    let mut settings = Settings::load();

    loop {
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();

            // Réglages : les changements s'appliquent à chaud — miroir
            // immédiat, caméra et débit par un redémarrage du flux (l'iPhone
            // ne reconfigure qu'au Start), format au prochain démarrage.
            let updated = Settings::load();
            if updated != settings {
                let previous = settings;
                settings = updated;
                if updated.mirror != previous.mirror {
                    log::info!(
                        "miroir {} (à chaud)",
                        if updated.mirror { "activé" } else { "désactivé" }
                    );
                    if let Some(active) = session.as_mut() {
                        let _ = active.set_options(SetOptions {
                            mirror: updated.mirror,
                            rotation: 0,
                            dim_preview: true,
                        });
                    }
                }
                if updated.preference != previous.preference
                    || updated.bitrate_kbps != previous.bitrate_kbps
                {
                    log::info!(
                        "caméra/débit modifiés ({} · {} kbps) — redémarrage du flux",
                        updated.preference,
                        updated.bitrate_kbps
                    );
                    if let Some(mut active) = session.take() {
                        let _ = active.stop();
                    }
                    decoder = None;
                    retry_at = None;
                }
                if image_changed(&previous, &updated) {
                    // Réglages de device : ils s'appliquent à chaud, sans
                    // redémarrer la session.
                    log::info!(
                        "réglages image (zoom {} %, expo mode {} biais {} ISO {} durée {} µs, \
                         focus mode {} distance {}, WB mode {} {} K teinte {}, torche {} {} %, \
                         stabilisation {}, HDR {})",
                        updated.zoom_x100,
                        updated.exposure_mode,
                        updated.exposure_bias_ev_x100,
                        updated.iso,
                        updated.exposure_duration_us,
                        updated.focus_mode,
                        updated.focus_distance_x100,
                        updated.wb_mode,
                        updated.wb_temp_k,
                        updated.wb_tint_x100,
                        updated.torch,
                        updated.torch_level,
                        updated.stabilization,
                        updated.hdr,
                    );
                    if let Some(active) = session.as_mut() {
                        if let Err(error) = active.set_image(image_message(&updated)) {
                            log::warn!("SetImage non envoyé: {error}");
                        }
                    }
                }
                if updated.width != previous.width
                    || updated.height != previous.height
                    || updated.fps != previous.fps
                {
                    log::info!(
                        "format {}x{}@{} demandé : la DLL l'applique au prochain démarrage de Windows",
                        updated.width,
                        updated.height,
                        updated.fps
                    );
                }
            }

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
                            match start_stream(&options, &control, &settings) {
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
                    publish_last_error(&error.message);
                    if error.code_label() == Some(ErrorCode::ImageUnsupported) {
                        // Un réglage refusé par la caméra ne coupe pas le flux :
                        // la fenêtre de réglages affiche l'erreur, le reste
                        // continue d'être appliqué.
                        log::warn!("réglage image refusé — flux conservé");
                    } else {
                        if let Some(block) = shm.as_mut() {
                            block.set_producer_state(PRODUCER_ERROR, u32::from(error.code));
                        }
                        drop_session = true;
                        retry_at = Some(Instant::now() + Duration::from_secs(1));
                    }
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

fn start_stream(
    options: &Options,
    control: &Control,
    settings: &Settings,
) -> Result<(Session, H264Decoder)> {
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

    let camera = resolve_camera(&caps, options, settings)?;
    let (width, height, fps) = choose_format(camera, options, control, settings)?;
    let bitrate_kbps = options.bitrate_kbps.unwrap_or(settings.bitrate_kbps);
    session.set_camera(camera.camera_id)?;
    session.set_options(SetOptions { mirror: settings.mirror, rotation: 0, dim_preview: true })?;
    // L'iPhone repart de ses propres défauts à chaque session : les réglages
    // d'image sont donc rejoués à chaque connexion.
    session.set_image(image_message(settings))?;
    // L'ImageCaps arrive juste après le SetCamera. On l'attend ici, avant Start,
    // pour ne pas avaler les premiers messages du flux en le guettant plus tard.
    match session.wait_image_caps(Duration::from_millis(500)) {
        Ok(Some(caps)) => publish_image_caps(&caps, &camera.label),
        Ok(None) => log::info!("cette caméra n'annonce pas ses plages (pas d'ImageCaps)"),
        Err(error) => log::debug!("ImageCaps: {error}"),
    }
    session.set_format(SetFormat { width, height, fps, codec: CODEC_H264, bitrate_kbps })?;
    session.start()?;
    log::info!(
        "session iPhone: caméra {} ({}) {width}x{height} @ {fps} fps, {bitrate_kbps} kbps, miroir {}",
        camera.camera_id,
        camera.label,
        if settings.mirror { "oui" } else { "non" }
    );

    Ok((session, H264Decoder::new(false)?))
}

/// Caméra à utiliser : `--camera` est prioritaire, sinon la préférence des
/// réglages est résolue contre les capacités annoncées par l'iPhone.
fn resolve_camera<'a>(
    caps: &'a Caps,
    options: &Options,
    settings: &Settings,
) -> Result<&'a Camera> {
    if let Some(id) = options.camera {
        return caps
            .cameras
            .iter()
            .find(|camera| camera.camera_id == id)
            .with_context(|| format!("caméra {id} absente de Caps"));
    }

    if settings.preference == CameraPreference::Auto {
        return caps.cameras.first().context("Caps ne contient aucune caméra");
    }

    let front = settings.preference == CameraPreference::Front;
    let wanted_lens = match settings.preference {
        CameraPreference::BackWide => Some(LENS_WIDE),
        CameraPreference::BackUltraWide => Some(LENS_ULTRA_WIDE),
        CameraPreference::BackTele => Some(LENS_TELE),
        _ => None,
    };
    caps.cameras
        .iter()
        .find(|camera| {
            if front {
                camera.position == POSITION_FRONT || camera.lens == wcl_agent::protocol::LENS_FRONT
            } else {
                camera.position != POSITION_FRONT
                    && wanted_lens.is_none_or(|lens| camera.lens == lens)
            }
        })
        .with_context(|| {
            format!("aucune caméra ne correspond à la préférence {}", settings.preference)
        })
}

fn choose_format(
    camera: &Camera,
    options: &Options,
    control: &Control,
    settings: &Settings,
) -> Result<(u16, u16, u16)> {
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
        .unwrap_or(if settings.fps > 0 { settings.fps as u16 } else { 30 });
    let fps = u32::from(wanted_fps).min(u32::from(format.max_fps)).max(1);
    Ok((format.width, format.height, fps as u16))
}

fn report_error(error: &ProtocolError) {
    match error.code_label() {
        Some(code) => log::warn!("erreur iPhone [{:?}] {} — {}", code, code.label(), error.message),
        None => log::warn!("erreur iPhone [{}] {}", error.code, error.message),
    }
}

/// Réglages image traduits en message `SetImage`.
fn image_message(settings: &Settings) -> SetImage {
    SetImage {
        zoom_x100: settings.zoom_x100.min(u32::from(u16::MAX)) as u16,
        exposure_mode: settings.exposure_mode,
        exposure_bias_ev_x100: settings.exposure_bias_ev_x100,
        iso: settings.iso,
        exposure_duration_us: settings.exposure_duration_us,
        focus_mode: settings.focus_mode,
        focus_x_x10000: settings.focus_x_x10000,
        focus_y_x10000: settings.focus_y_x10000,
        wb_mode: settings.wb_mode,
        wb_temp_k: settings.wb_temp_k,
        wb_tint_x100: settings.wb_tint_x100,
        torch: settings.torch,
        stabilization: settings.stabilization,
        hdr: settings.hdr,
        torch_level: settings.torch_level,
        focus_distance_x100: settings.focus_distance_x100,
    }
}

/// Vrai si un réglage d'image a bougé (le format et le débit ont leur propre
/// chemin, plus lourd : ils redémarrent la session).
fn image_changed(previous: &Settings, updated: &Settings) -> bool {
    image_message(previous) != image_message(updated)
}

/// Écrit `caps.ini` : ce que la caméra de l'iPhone sait faire, pour que la
/// fenêtre de réglages borne ses curseurs. La dernière erreur connue est
/// conservée.
fn publish_image_caps(caps: &ImageCaps, label: &str) {
    let file = CameraCaps {
        camera_label: label.to_string(),
        flags: u32::from(caps.flags),
        zoom_max_x100: u32::from(caps.zoom_max_x100),
        iso_min: u32::from(caps.iso_min),
        iso_max: u32::from(caps.iso_max),
        exposure_min_us: caps.exposure_min_us,
        exposure_max_us: caps.exposure_max_us,
        wb_min_k: u32::from(caps.wb_min_k),
        wb_max_k: u32::from(caps.wb_max_k),
        wb_tint_min_x100: i32::from(caps.wb_tint_min_x100),
        wb_tint_max_x100: i32::from(caps.wb_tint_max_x100),
        stabilization_modes: u32::from(caps.stabilization_modes),
        last_error: CameraCaps::load().last_error,
    };
    log::info!(
        "caméra « {} » : zoom max {}.{:02}×, ISO {}–{}, durée {}–{} µs, balance {}–{} K, \
         stabilisation {:#04x}, plages publiées dans {}",
        label,
        caps.zoom_max_x100 / 100,
        caps.zoom_max_x100 % 100,
        caps.iso_min,
        caps.iso_max,
        caps.exposure_min_us,
        caps.exposure_max_us,
        caps.wb_min_k,
        caps.wb_max_k,
        caps.stabilization_modes,
        wcl_agent::config::caps_path().display()
    );
    if let Err(error) = file.save() {
        log::warn!("caps.ini non écrit: {error}");
    }
}

/// Dernière erreur signalée par l'iPhone, dans `caps.ini`.
fn publish_last_error(message: &str) {
    let mut file = CameraCaps::load();
    file.last_error = message.to_string();
    if let Err(error) = file.save() {
        log::warn!("caps.ini non écrit: {error}");
    }
}
