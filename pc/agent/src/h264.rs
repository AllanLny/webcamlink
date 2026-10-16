//! Annex-B helpers: split NAL units, locate parameter sets and IDR slices.
//!
//! Both sides of WebcamLink ship H.264 as an Annex-B byte stream (start
//! codes), so the agent needs to peek inside access units for the SPS/PPS
//! and the keyframe flag.

/// One NAL unit, without its start code. `data` starts at the NAL header byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalUnit<'a> {
    pub nal_type: u8,
    pub data: &'a [u8],
}

pub const NAL_IDR_SLICE: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

/// Iterate the NAL units of an Annex-B buffer (3- and 4-byte start codes).
pub fn split_annex_b(data: &[u8]) -> Vec<NalUnit<'_>> {
    let mut units = Vec::new();
    let mut start: Option<usize> = None;
    let mut index = 0;
    while index + 3 <= data.len() {
        let three_byte = data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 1;
        let four_byte = index + 4 <= data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 0
            && data[index + 3] == 1;
        if four_byte || three_byte {
            let prefix = if four_byte { 4 } else { 3 };
            if let Some(begin) = start {
                let end = index;
                if end > begin {
                    let unit = &data[begin..end];
                    units.push(NalUnit { nal_type: unit[0] & 0x1F, data: unit });
                }
            }
            index += prefix;
            start = Some(index);
        } else {
            index += 1;
        }
    }
    if let Some(begin) = start {
        if data.len() > begin {
            let unit = &data[begin..];
            units.push(NalUnit { nal_type: unit[0] & 0x1F, data: unit });
        }
    }
    units
}

/// SPS/PPS (and VPS for HEVC) carried by an access unit, start-code prefixed.
pub fn parameter_sets(access_unit: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for unit in split_annex_b(access_unit) {
        if matches!(unit.nal_type, NAL_SPS | NAL_PPS) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(unit.data);
        }
    }
    out
}

pub fn contains_idr_slice(access_unit: &[u8]) -> bool {
    split_annex_b(access_unit).iter().any(|unit| unit.nal_type == NAL_IDR_SLICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_mixed_start_codes() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&[0, 0, 0, 1, 0x67, 0xAA]); // SPS, 4-byte prefix
        stream.extend_from_slice(&[0, 0, 1, 0x68, 0xBB]); // PPS, 3-byte prefix
        stream.extend_from_slice(&[0, 0, 0, 1, 0x65, 0xCC, 0xDD]); // IDR

        let units = split_annex_b(&stream);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].nal_type, NAL_SPS);
        assert_eq!(units[0].data, &[0x67, 0xAA]);
        assert_eq!(units[1].nal_type, NAL_PPS);
        assert_eq!(units[1].data, &[0x68, 0xBB]);
        assert_eq!(units[2].nal_type, NAL_IDR_SLICE);
        assert_eq!(units[2].data, &[0x65, 0xCC, 0xDD]);

        assert!(contains_idr_slice(&stream));
        let sets = parameter_sets(&stream);
        assert_eq!(sets, vec![0, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x68, 0xBB]);
    }

    #[test]
    fn non_keyframe_has_no_idr() {
        let stream = [0, 0, 0, 1, 0x41, 0x9A, 0x00, 0x00, 0x01, 0x62];
        assert!(!contains_idr_slice(&stream));
        assert!(parameter_sets(&stream).is_empty());
    }

    #[test]
    fn handles_trailing_unit_without_prefix() {
        let units = split_annex_b(&[0, 0, 1, 0x41, 0x11]);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].data, &[0x41, 0x11]);
    }
}
