//! Pixel helpers: NV12 → BGRA for the preview, synthetic NV12 test frames.

/// Convert an NV12 image (with arbitrary strides) to BGRA, BT.601 limited
/// range. `out` is resized to `width * height * 4`.
pub fn nv12_to_bgra(
    nv12: &[u8],
    stride_y: usize,
    stride_uv: usize,
    width: usize,
    height: usize,
    out: &mut Vec<u8>,
) {
    let chroma_offset = stride_y * height;
    out.clear();
    out.resize(width * height * 4, 0);

    let mut y_row = 0usize;
    let mut uv_row = 0usize;
    for row in 0..height {
        let y_line = &nv12[y_row..y_row + stride_y];
        let uv_line = &nv12[chroma_offset + uv_row..chroma_offset + uv_row + stride_uv];
        let out_line = &mut out[row * width * 4..(row + 1) * width * 4];
        for column in 0..width {
            let y = y_line[column] as i32;
            let u = uv_line[(column / 2) * 2] as i32;
            let v = uv_line[(column / 2) * 2 + 1] as i32;
            let c = (y - 16).max(0);
            let d = u - 128;
            let e = v - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let pixel = &mut out_line[column * 4..column * 4 + 4];
            pixel.copy_from_slice(&[b, g, r, 255]);
        }
        y_row += stride_y;
        if row % 2 == 1 {
            uv_row += stride_uv;
        }
    }
}

const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// A moving test pattern, deliberately *hard to skip*: a scrolling diagonal
/// ramp with an inverting checker texture (every block changes on every
/// frame, so the encoder cannot just emit skip macroblocks), a travelling
/// bar and the frame index drawn top-left.
pub fn synthetic_nv12(width: usize, height: usize, frame_index: u64) -> Vec<u8> {
    let mut frame = vec![128u8; width * height * 3 / 2];
    let y_plane = &mut frame[..width * height];

    for row in 0..height {
        let line = &mut y_plane[row * width..(row + 1) * width];
        for (column, value) in line.iter_mut().enumerate() {
            let scroll = ((column + row) as u64 + frame_index * 23) % 180;
            let texture = if ((column / 8) + (row / 8) + frame_index as usize).is_multiple_of(2) {
                24
            } else {
                0
            };
            *value = (16 + scroll as usize + texture).min(235) as u8;
        }
    }

    let bar_x = (frame_index as usize * 8) % width.max(1);
    let bar_width = (width / 40).max(2);
    let bar_end = (bar_x + bar_width).min(width);
    for row in (height / 4)..(height * 3 / 4) {
        y_plane[row * width + bar_x..row * width + bar_end].fill(235);
    }

    // Frame index on a dark plate, so it survives even heavy quantisation.
    let scale = (width / 160).clamp(2, 10);
    let plate_width = 4 * 4 * scale + 2 * scale;
    let plate_height = 5 * scale + 2 * scale;
    for row in 0..plate_height.min(height) {
        y_plane[row * width..row * width + plate_width.min(width)].fill(16);
    }
    let label = format!("{:02}", frame_index % 100);
    for (position, character) in label.chars().enumerate() {
        let Some(digit) = character.to_digit(10) else { continue };
        let glyph = &DIGITS[digit as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = position * 4 * scale + column * scale + dx + scale;
                        let y = row * scale + dy + scale;
                        if x < width && y < height {
                            y_plane[y * width + x] = 235;
                        }
                    }
                }
            }
        }
    }

    let chroma_offset = width * height;
    let uv_plane = &mut frame[chroma_offset..];
    let shift = (frame_index % 64) as u8;
    for row in 0..height / 2 {
        for column in 0..width / 2 {
            let u = (96 + column * 40 / (width / 2).max(1)) as u8;
            let v = (150 - row * 40 / (height / 2).max(1)) as u8;
            uv_plane[(row * width / 2 + column) * 2] = u.wrapping_add(shift / 2);
            uv_plane[(row * width / 2 + column) * 2 + 1] = v.wrapping_sub(shift / 2);
        }
    }
    frame
}

/// Write a BGRA frame as a 24-bit BMP (handy to eyeball a decoded frame).
pub fn write_bmp(path: &std::path::Path, bgra: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    use std::io::Write;

    let row_bytes = width as usize * 3;
    let padding = (4 - row_bytes % 4) % 4;
    let image_bytes = (row_bytes + padding) * height as usize;
    let file_bytes = 54 + image_bytes;

    let mut out = Vec::with_capacity(file_bytes);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_bytes as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());

    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(image_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for row in (0..height as usize).rev() {
        let line = &bgra[row * width as usize * 4..(row + 1) * width as usize * 4];
        for chunk in line.chunks(4) {
            out.extend_from_slice(&[chunk[0], chunk[1], chunk[2]]);
        }
        out.extend_from_slice(&vec![0u8; padding]);
    }

    std::fs::File::create(path)?.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_24bit_bmp() {
        let bgra = vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255];
        let path = std::env::temp_dir().join("wcl-color-test.bmp");
        write_bmp(&path, &bgra, 2, 2).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bytes[10..14].try_into().unwrap()), 54);
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 24);
        assert_eq!(bytes.len(), 54 + 2 * (6 + 2));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn converts_uniform_gray() {
        // 2x2 image, Y = 16 (black), UV = 128.
        let nv12 = vec![16, 16, 16, 16, 128, 128];
        let mut bgra = Vec::new();
        nv12_to_bgra(&nv12, 2, 2, 2, 2, &mut bgra);
        assert_eq!(bgra.len(), 2 * 2 * 4);
        assert_eq!(&bgra[..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn converts_white_and_respects_stride() {
        // 2x2, Y = 235, UV = 128, stride_y = 4 (2 padding bytes per row).
        let nv12 = vec![235, 235, 0, 0, 235, 235, 0, 0, 128, 128];
        let mut bgra = Vec::new();
        nv12_to_bgra(&nv12, 4, 2, 2, 2, &mut bgra);
        assert_eq!(&bgra[..4], &[255, 255, 255, 255]);
        assert_eq!(&bgra[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn synthetic_frame_has_expected_layout() {
        let frame = synthetic_nv12(64, 48, 7);
        assert_eq!(frame.len(), 64 * 48 * 3 / 2);
        // The frame index is drawn near the top-left corner: some bright pixels.
        let bright = frame[..64 * 48].iter().filter(|value| **value >= 230).count();
        assert!(bright > 50, "le motif doit contenir le numéro de frame (brillant)");
    }

    #[test]
    fn consecutive_frames_change_almost_everywhere() {
        // Guards the test pattern's usefulness: an encoder must not be able to
        // skip whole macroblocks, otherwise the clip silently repeats content.
        let (width, height) = (160usize, 96usize);
        let first = synthetic_nv12(width, height, 0);
        let second = synthetic_nv12(width, height, 1);
        let luma = width * height;
        let changed = (0..luma).filter(|index| first[*index] != second[*index]).count();
        assert!(
            changed * 100 / luma > 90,
            "seulement {changed}/{luma} pixels changent entre deux frames"
        );
    }
}
