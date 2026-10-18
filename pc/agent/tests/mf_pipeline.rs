//! Media Foundation round-trip: encode synthetic frames, decode them back.
//!
//! Guards the properties the stream depends on:
//! * every access unit yields exactly one frame, in order, with the matching
//!   content (no dropping, no reordering, no duplicates);
//! * the decoder releases the frame immediately (low-latency mode) instead of
//!   sitting on a reordering queue;
//! * hard-to-skip source frames stay distinguishable after the round trip.

#![cfg(windows)]

use wcl_agent::color::synthetic_nv12;
use wcl_agent::decode::H264Decoder;
use wcl_agent::encode::H264Encoder;
use wcl_agent::mf::MediaFoundation;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const FPS: u32 = 30;
const FRAMES: u64 = 30;

fn mean_absolute_difference(a: &[u8], b: &[u8]) -> f64 {
    let length = a.len().min(b.len());
    let total: u64 = (0..length).map(|index| u64::from(a[index].abs_diff(b[index]))).sum();
    total as f64 / length as f64
}

#[test]
fn encoder_decoder_round_trip_is_frame_exact() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug"))
        .is_test(true)
        .try_init();
    let _mf = MediaFoundation::start().unwrap();

    let mut encoder = H264Encoder::new(WIDTH, HEIGHT, FPS, 4000).unwrap();
    let mut decoder = H264Decoder::new(false).unwrap();
    decoder.configure(WIDTH, HEIGHT, FPS).unwrap();

    let sources: Vec<Vec<u8>> = (0..FRAMES)
        .map(|index| synthetic_nv12(WIDTH as usize, HEIGHT as usize, index))
        .collect();

    let mut units = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        units.extend(encoder.encode(index as u64, source).unwrap());
    }
    units.extend(encoder.finish().unwrap());
    assert_eq!(units.len() as u64, FRAMES, "un access unit par frame");

    let mut decoded = Vec::new();
    let mut first_output_at: Option<usize> = None;
    for (index, unit) in units.iter().enumerate() {
        let frames = decoder.feed(&unit.data, unit.pts_us).unwrap();
        if first_output_at.is_none() && !frames.is_empty() {
            first_output_at = Some(index);
        }
        decoded.extend(frames);
    }
    decoded.extend(decoder.flush().unwrap());

    assert_eq!(decoded.len(), units.len(), "un frame décodée par access unit");
    // A decoder can sit on the very first access unit while it primes itself:
    // the CI runner does, on the same commit that a desktop machine releases
    // immediately. What the stream needs is that no reordering queue builds up,
    // so the first picture may be one unit late, never more.
    assert!(
        first_output_at.is_some_and(|index| index <= 1),
        "première frame sortie à {first_output_at:?} : file de réordonnancement trop profonde"
    );

    for (index, frame) in decoded.iter().enumerate() {
        assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
        assert_eq!(frame.nv12.len(), (WIDTH * HEIGHT * 3 / 2) as usize, "NV12 compacté");
        assert_eq!(frame.stride_y, WIDTH as usize);
        assert_eq!(frame.stride_uv, WIDTH as usize);

        // The decoded frame must match its own source better than any other.
        let own = mean_absolute_difference(&frame.nv12[..(WIDTH * HEIGHT) as usize], &sources[index]);
        let best_other = sources
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, source)| {
                mean_absolute_difference(&frame.nv12[..(WIDTH * HEIGHT) as usize], source)
            })
            .fold(f64::MAX, f64::min);
        assert!(
            own < best_other - 1.0,
            "frame {index}: contenu ambigu (écart propre {own:.2}, meilleur autre {best_other:.2})"
        );
    }
}

#[test]
fn decoded_pixels_match_the_source_pattern() {
    let _mf = MediaFoundation::start().unwrap();

    let source = synthetic_nv12(WIDTH as usize, HEIGHT as usize, 0);
    let mut encoder = H264Encoder::new(WIDTH, HEIGHT, FPS, 6000).unwrap();
    let mut decoder = H264Decoder::new(false).unwrap();
    decoder.configure(WIDTH, HEIGHT, FPS).unwrap();

    let mut units = encoder.encode(0, &source).unwrap();
    for index in 1..10 {
        units.extend(
            encoder.encode(index, &synthetic_nv12(WIDTH as usize, HEIGHT as usize, index)).unwrap(),
        );
    }
    units.extend(encoder.finish().unwrap());

    let mut first = None;
    for unit in &units {
        for frame in decoder.feed(&unit.data, unit.pts_us).unwrap() {
            if first.is_none() {
                first = Some(frame);
            }
        }
    }

    let frame = first.expect("première frame décodée");
    // Compression drifts a little, but the luma plane must still match the
    // pattern (average brightness well within a few units of the source).
    let source_y: u64 = source[..(WIDTH * HEIGHT) as usize].iter().map(|v| u64::from(*v)).sum();
    let decoded_y: u64 = frame.nv12[..(WIDTH * HEIGHT) as usize].iter().map(|v| u64::from(*v)).sum();
    let pixels = u64::from(WIDTH * HEIGHT);
    let source_avg = source_y as f64 / pixels as f64;
    let decoded_avg = decoded_y as f64 / pixels as f64;
    assert!(
        (source_avg - decoded_avg).abs() < 8.0,
        "luminosité moyenne source {source_avg:.1} vs décodée {decoded_avg:.1}"
    );
}

/// Diagnostic: how does the encoder behave, and which source frame does each
/// decoder output contain? Run with `--ignored --nocapture`.
#[test]
#[ignore = "diagnostic — lancer avec --ignored --nocapture"]
fn encoder_decoder_content_probe() {
    let _mf = MediaFoundation::start().unwrap();
    let (width, height) = (160usize, 96usize);

    let mut encoder = H264Encoder::new(width as u32, height as u32, FPS, 4000).unwrap();
    let mut decoder = H264Decoder::new(false).unwrap();
    decoder.configure(width as u32, height as u32, FPS).unwrap();

    let sources: Vec<Vec<u8>> = (0..24)
        .map(|index| synthetic_nv12(width, height, index))
        .collect();

    let mut units = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let produced = encoder.encode(index as u64, source).unwrap();
        for unit in &produced {
            eprintln!("entrée {index} → sortie encodeur {} octets", unit.data.len());
        }
        units.extend(produced);
    }
    for unit in encoder.finish().unwrap() {
        eprintln!("drain → {} octets", unit.data.len());
        units.push(unit);
    }

    for (index, unit) in units.iter().enumerate() {
        for frame in decoder.feed(&unit.data, unit.pts_us).unwrap() {
            let luma = &frame.nv12[..width * height];
            let best = sources
                .iter()
                .enumerate()
                .map(|(source_index, source)| {
                    (source_index, mean_absolute_difference(luma, &source[..width * height]))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            eprintln!("unité {index} → contenu source {} (écart {:.2})", best.0, best.1);
        }
    }
}
