//! G.711 sound (ITU-T G.711, RFC 3551 payload types 0 and 8): mu-law and A-law bytes to 16-bit samples. In-house, a table-free
//! translation of the standard's formulas, used by the camera view to turn a camera's sound into samples a speaker can play.

use crate::sdp::AudioCodec;

/// One mu-law byte as a 16-bit sample.
pub fn ulaw(b: u8) -> i16 {
    let u = !b;
    let t = ((i32::from(u) & 0x0f) << 3) + 0x84;
    let t = t << ((u & 0x70) >> 4);
    (if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }) as i16
}

/// One A-law byte as a 16-bit sample.
pub fn alaw(b: u8) -> i16 {
    let a = b ^ 0x55;
    let mut t = (i32::from(a) & 0x0f) << 4;
    let seg = (a & 0x70) >> 4;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    (if a & 0x80 != 0 { t } else { -t }) as i16
}

/// A frame of G.711 as samples at the track's own rate (8000 Hz in practice). `None` for a codec this module does not decode (AAC, others).
pub fn decode(codec: &AudioCodec, data: &[u8]) -> Option<Vec<i16>> {
    match codec {
        AudioCodec::Pcmu => Some(data.iter().map(|b| ulaw(*b)).collect()),
        AudioCodec::Pcma => Some(data.iter().map(|b| alaw(*b)).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_and_the_middle_of_both_laws_are_the_standards() {
        // mu-law: 0xFF and 0x7F are silence, 0x80 and 0x00 are the largest values (+-32124)
        assert_eq!((ulaw(0xFF), ulaw(0x7F), ulaw(0x80), ulaw(0x00)), (0, 0, 32124, -32124));
        // A-law: 0xD5 and 0x55 are the smallest steps (+-8), 0xAA and 0x2A the largest (+-32256)
        assert_eq!((alaw(0xD5), alaw(0x55), alaw(0xAA), alaw(0x2A)), (8, -8, 32256, -32256));
    }

    #[test]
    fn every_byte_decodes_symmetrically_and_in_order_within_each_sign() {
        for b in 0u8..=127 {
            assert_eq!(ulaw(b), -ulaw(b | 0x80), "mu-law {b:#x}: the sign bit flips the sign");
        }
        // louder codes are larger magnitudes (mu-law codes count down in magnitude)
        let mags: Vec<i16> = (0u8..=127).map(|b| ulaw(0x80 | b)).collect();
        assert!(mags.windows(2).all(|w| w[0] >= w[1]), "mu-law magnitudes fall as the code rises");
        for b in 0u8..=255 {
            assert_eq!(alaw(b), -alaw(b ^ 0x80), "A-law {b:#x}: the sign bit flips the sign");
        }
    }

    #[test]
    fn a_frame_decodes_by_codec_and_other_codecs_are_left_alone() {
        assert_eq!(decode(&AudioCodec::Pcmu, &[0xFF, 0x80]), Some(vec![0, 32124]));
        assert_eq!(decode(&AudioCodec::Pcma, &[0xD5, 0x55]), Some(vec![8, -8]));
        assert_eq!(decode(&AudioCodec::Other("opus".into()), &[1, 2]), None);
        assert_eq!(decode(&AudioCodec::Pcmu, &[]), Some(vec![]));
    }
}
