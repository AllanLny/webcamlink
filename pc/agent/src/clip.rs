//! WCLC clip format — pre-encoded access units used by the dev tools
//! (`wcl-mkh264` writes it, `wcl-fakephone` replays it).
//!
//! Layout: `"WCLC"`, version u8, codec u8, width u16, height u16, fps u16,
//! parameter-set length u32 + bytes, then one record per access unit:
//! `pts_us` u64, flags u8, length u32, Annex-B bytes.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

pub const MAGIC: &[u8; 4] = b"WCLC";
pub const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipHeader {
    pub codec: u8,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
    pub parameter_sets: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipFrame {
    pub pts_us: u64,
    pub flags: u8,
    pub data: Vec<u8>,
}

pub struct ClipWriter {
    writer: BufWriter<File>,
    frames: u64,
}

impl ClipWriter {
    pub fn create(path: &Path, header: &ClipHeader) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("création de {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        writer.write_all(MAGIC)?;
        writer.write_all(&[VERSION, header.codec])?;
        writer.write_all(&header.width.to_le_bytes())?;
        writer.write_all(&header.height.to_le_bytes())?;
        writer.write_all(&header.fps.to_le_bytes())?;
        writer.write_all(&(header.parameter_sets.len() as u32).to_le_bytes())?;
        writer.write_all(&header.parameter_sets)?;
        Ok(Self { writer, frames: 0 })
    }

    pub fn write_frame(&mut self, frame: &ClipFrame) -> Result<()> {
        self.writer.write_all(&frame.pts_us.to_le_bytes())?;
        self.writer.write_all(&[frame.flags])?;
        self.writer.write_all(&(frame.data.len() as u32).to_le_bytes())?;
        self.writer.write_all(&frame.data)?;
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64> {
        self.writer.flush()?;
        Ok(self.frames)
    }
}

pub struct ClipReader {
    reader: BufReader<File>,
    header: ClipHeader,
}

impl ClipReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("ouverture de {}", path.display()))?;
        let mut reader = BufReader::new(file);
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            bail!("{} n'est pas un clip WCLC", path.display());
        }
        let mut fixed = [0u8; 8];
        reader.read_exact(&mut fixed)?;
        let version = fixed[0];
        if version != VERSION {
            bail!("version de clip {version} non supportée");
        }
        let codec = fixed[1];
        let width = u16::from_le_bytes([fixed[2], fixed[3]]);
        let height = u16::from_le_bytes([fixed[4], fixed[5]]);
        let fps = u16::from_le_bytes([fixed[6], fixed[7]]);
        let mut length = [0u8; 4];
        reader.read_exact(&mut length)?;
        let mut parameter_sets = vec![0u8; u32::from_le_bytes(length) as usize];
        reader.read_exact(&mut parameter_sets)?;
        Ok(Self {
            reader,
            header: ClipHeader { codec, width, height, fps, parameter_sets },
        })
    }

    pub fn header(&self) -> &ClipHeader {
        &self.header
    }

    pub fn next_frame(&mut self) -> Result<Option<ClipFrame>> {
        let mut stamp = [0u8; 8];
        match self.reader.read_exact(&mut stamp) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let pts_us = u64::from_le_bytes(stamp);
        let mut flags = [0u8; 1];
        self.reader.read_exact(&mut flags)?;
        let mut length = [0u8; 4];
        self.reader.read_exact(&mut length)?;
        let mut data = vec![0u8; u32::from_le_bytes(length) as usize];
        self.reader.read_exact(&mut data)?;
        Ok(Some(ClipFrame { pts_us, flags: flags[0], data }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::FRAME_FLAG_KEYFRAME;

    #[test]
    fn clip_round_trip() {
        let path = std::env::temp_dir().join("wcl-clip-test.wclc");
        let header = ClipHeader {
            codec: 0,
            width: 1920,
            height: 1080,
            fps: 30,
            parameter_sets: vec![0, 0, 0, 1, 0x67, 0x42],
        };
        let frames = vec![
            ClipFrame { pts_us: 0, flags: FRAME_FLAG_KEYFRAME, data: vec![0, 0, 0, 1, 0x65, 0x11] },
            ClipFrame { pts_us: 33_333, flags: 0, data: vec![0, 0, 0, 1, 0x41, 0x22] },
        ];

        let mut writer = ClipWriter::create(&path, &header).unwrap();
        for frame in &frames {
            writer.write_frame(frame).unwrap();
        }
        assert_eq!(writer.finish().unwrap(), 2);

        let mut reader = ClipReader::open(&path).unwrap();
        assert_eq!(*reader.header(), header);
        let mut read_back = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            read_back.push(frame);
        }
        assert_eq!(read_back, frames);

        std::fs::remove_file(&path).unwrap();
    }
}
