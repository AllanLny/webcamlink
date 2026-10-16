//! `wcl-mkh264` — generate a synthetic H.264 test clip (WCLC) with Media
//! Foundation, so the PC pipeline can be exercised without an iPhone (and
//! without ffmpeg).
//!
//! Usage: wcl-mkh264 [out.wclc] [--size 1920x1080] [--fps 30] [--seconds 5] [--bitrate 8000]

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use wcl_agent::clip::{ClipFrame, ClipHeader, ClipWriter};
use wcl_agent::color::synthetic_nv12;
use wcl_agent::encode::H264Encoder;
use wcl_agent::h264;
use wcl_agent::mf::MediaFoundation;
use wcl_agent::protocol::{CODEC_H264, FRAME_FLAG_KEYFRAME};

fn main() -> Result<()> {
    env_logger::init();

    let mut output = PathBuf::from("test.wclc");
    let (mut width, mut height) = (1280u32, 720u32);
    let mut fps = 30u32;
    let mut seconds = 5u32;
    let mut bitrate = 8000u32;

    let mut arguments = std::env::args().skip(1);
    let mut position = 0usize;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--size" => {
                let value = arguments.next().context("--size attend WxH")?;
                let (w, h) = value.split_once('x').context("--size attend WxH (ex. 1920x1080)")?;
                width = w.parse().context("largeur invalide")?;
                height = h.parse().context("hauteur invalide")?;
            }
            "--fps" => fps = arguments.next().context("--fps attend un nombre")?.parse()?,
            "--seconds" => seconds = arguments.next().context("--seconds attend un nombre")?.parse()?,
            "--bitrate" => bitrate = arguments.next().context("--bitrate attend des kbps")?.parse()?,
            "--help" | "-h" => {
                println!(
                    "wcl-mkh264 [out.wclc] [--size 1920x1080] [--fps 30] [--seconds 5] [--bitrate 8000]"
                );
                return Ok(());
            }
            other if position == 0 && !other.starts_with("--") => {
                output = PathBuf::from(other);
                position += 1;
            }
            other => bail!("argument inconnu: {other}"),
        }
    }

    if width % 2 != 0 || height % 2 != 0 {
        bail!("les dimensions doivent être paires (NV12)");
    }

    let _mf = MediaFoundation::start()?;
    let mut encoder = H264Encoder::new(width, height, fps, bitrate)
        .context("initialisation de l'encodeur MF")?;

    let frame_count = fps * seconds;
    println!(
        "Encodage {width}x{height} @ {fps} fps, {seconds} s ({frame_count} frames), {bitrate} kbps…"
    );

    let mut parameter_sets = Vec::new();
    let mut keyframes = 0u32;
    let mut total_bytes = 0u64;

    // Header must be written first: buffer the access units, then stream them out.
    let mut units = Vec::new();
    for index in 0..u64::from(frame_count) {
        let nv12 = synthetic_nv12(width as usize, height as usize, index);
        units.extend(encoder.encode(index, &nv12)?);
    }
    units.extend(encoder.finish()?);

    for unit in &units {
        if unit.keyframe {
            keyframes += 1;
            if parameter_sets.is_empty() {
                parameter_sets = h264::parameter_sets(&unit.data);
            }
        }
        total_bytes += unit.data.len() as u64;
    }

    if parameter_sets.is_empty() {
        bail!("aucun SPS/PPS trouvé dans le flux généré");
    }

    let header = ClipHeader { codec: CODEC_H264, width: width as u16, height: height as u16, fps: fps as u16, parameter_sets };
    let mut writer = ClipWriter::create(&output, &header)?;
    for unit in &units {
        writer.write_frame(&ClipFrame {
            pts_us: unit.pts_us,
            flags: if unit.keyframe { FRAME_FLAG_KEYFRAME } else { 0 },
            data: unit.data.clone(),
        })?;
    }
    let written = writer.finish()?;

    println!(
        "{} écrit: {} frames ({} IDR), {:.2} Mo, paramètres {} octets",
        output.display(),
        written,
        keyframes,
        total_bytes as f64 / (1024.0 * 1024.0),
        header.parameter_sets.len()
    );
    Ok(())
}
