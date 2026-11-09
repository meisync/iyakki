//! WAV reading (training data only; the runtime spotter never touches a file) and writing (the data generator and the recorder docs).
//! Reads 8/16/24/32-bit integer and 32/64-bit float PCM, any channel count (averaged) and any rate (resampled to 16 kHz).

use crate::features::SAMPLE_RATE;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Decode a WAV file's bytes to mono f32 at 16 kHz.
pub fn decode(b: &[u8]) -> Result<Vec<f32>, String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut fmt, mut data): (Option<(u16, u16, u32, u16)>, Option<&[u8]>) = (None, None);
    let mut o = 12;
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let mut sz = u32le(b, o + 4) as usize;
        let body = o + 8;
        if body + sz > b.len() {
            sz = b.len() - body; // a stream cut short: take what is there
        }
        if id == b"fmt " && sz >= 16 {
            fmt = Some((u16le(b, body), u16le(b, body + 2), u32le(b, body + 4), u16le(b, body + 14)));
        } else if id == b"data" {
            data = Some(&b[body..body + sz]);
        }
        o = body + sz + (sz & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    let ch = ch.max(1) as usize;
    let bytes = (bits / 8) as usize;
    if bytes == 0 || rate == 0 {
        return Err("bad fmt chunk".into());
    }
    let sample = |p: &[u8]| -> Result<f32, String> {
        Ok(match (tag, bits) {
            (1, 8) => (p[0] as f32 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, p[0], p[1], p[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([p[0], p[1], p[2], p[3]]) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes([p[0], p[1], p[2], p[3]]),
            (3, 64) => f64::from_le_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]]) as f32,
            _ => return Err(format!("unsupported WAV format {tag} with {bits} bits")),
        })
    };
    let frames = data.len() / (bytes * ch);
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut s = 0.0;
        for c in 0..ch {
            s += sample(&data[(f * ch + c) * bytes..])?;
        }
        mono.push((s / ch as f32).clamp(-1.0, 1.0));
    }
    Ok(resample(&mono, rate as usize, SAMPLE_RATE))
}

/// Linear-interpolation resampling; going down, each output sample first averages the input span it covers.
pub fn resample(x: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    let ratio = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            if ratio > 1.0 {
                let (a, b) = (pos as usize, ((pos + ratio) as usize).min(x.len()).max(pos as usize + 1));
                x[a..b.min(x.len())].iter().sum::<f32>() / (b.min(x.len()) - a).max(1) as f32
            } else {
                let a = pos as usize;
                let f = (pos - a as f64) as f32;
                let b = (a + 1).min(x.len() - 1);
                x[a] * (1.0 - f) + x[b] * f
            }
        })
        .collect()
}

/// A 16-bit mono 16 kHz WAV.
pub fn encode(samples: &[f32]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    v.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_keeps_the_samples_within_one_step() {
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.05).sin() * 0.7).collect();
        let y = decode(&encode(&x)).unwrap();
        assert_eq!(y.len(), x.len());
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn stereo_float_at_another_rate_is_mixed_and_resampled() {
        // 32 kHz stereo float: left 0.5, right -0.1 -> mono 0.2, half as many samples at 16 kHz
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        for v in [3u16, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&32_000u32.to_le_bytes());
        b.extend_from_slice(&(32_000u32 * 8).to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(100u32 * 8).to_le_bytes());
        for _ in 0..100 {
            b.extend_from_slice(&0.5f32.to_le_bytes());
            b.extend_from_slice(&(-0.1f32).to_le_bytes());
        }
        let y = decode(&b).unwrap();
        assert_eq!(y.len(), 50);
        assert!(y.iter().all(|v| (v - 0.2).abs() < 1e-5));
    }

    #[test]
    fn garbage_is_refused_without_a_panic() {
        assert!(decode(b"hello").is_err());
        assert!(decode(b"RIFF\0\0\0\0WAVEjunk").is_err());
        let mut b = encode(&[0.1; 10]);
        b.truncate(30);
        let _ = decode(&b);
    }
}
