//! Putting the pieces of speech together: silence trimmed at the ends, the same loudness for every piece, no clicks where
//! pieces meet, and a pause that depends on the punctuation that ended the piece.
//!
//! The models make audio of very different loudness from one piece to the next (peaks between 0.2 and 1.0 for the same
//! speaker), and each piece starts and ends in a little silence of unpredictable length and sometimes at a non-zero sample.
//! Joining raw pieces would give jumps in volume and clicks.

/// What the loudness is set to: RMS of the speech parts, in dBFS, and the highest peak allowed (fraction of full scale).
pub const TARGET_RMS_DB: f32 = -21.0;
pub const PEAK_LIMIT: f32 = 0.89;
/// Silence kept at each end of a trimmed piece.
pub const KEEP_MS: u32 = 18;
/// Fade at each end of a piece, so a piece always starts and ends at zero.
pub const FADE_MS: u32 = 12;

fn ms(rate: u32, ms: u32) -> usize {
    (rate as u64 * ms as u64 / 1000) as usize
}

/// Remove the mean (a constant offset makes a click where a piece starts).
pub fn remove_dc(s: &mut [i16]) {
    if s.is_empty() {
        return;
    }
    let mean = s.iter().map(|&x| x as i64).sum::<i64>() / s.len() as i64;
    for x in s.iter_mut() {
        *x = (*x as i64 - mean).clamp(-32768, 32767) as i16;
    }
}

/// Cut silence from both ends: everything quieter than `-48 dB` below the peak of the piece, keeping [`KEEP_MS`].
pub fn trim_silence(s: &[i16], rate: u32) -> Vec<i16> {
    let peak = s.iter().map(|&x| (x as i32).unsigned_abs()).max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let thr = (peak as f32 * 0.004).max(8.0) as u32; // about -48 dB
    let first = s.iter().position(|&x| (x as i32).unsigned_abs() > thr);
    let last = s.iter().rposition(|&x| (x as i32).unsigned_abs() > thr);
    match (first, last) {
        (Some(a), Some(b)) => {
            let keep = ms(rate, KEEP_MS);
            s[a.saturating_sub(keep)..(b + 1 + keep).min(s.len())].to_vec()
        }
        _ => Vec::new(),
    }
}

/// RMS in dBFS of the samples louder than the trim threshold (so pauses inside a piece do not make it seem quiet).
pub fn speech_rms_db(s: &[i16]) -> Option<f32> {
    let peak = s.iter().map(|&x| (x as i32).unsigned_abs()).max()?;
    if peak == 0 {
        return None;
    }
    let thr = (peak as f32 * 0.02) as i64;
    let (mut sum, mut n) = (0f64, 0u64);
    for &x in s {
        if (x as i64).abs() > thr {
            sum += (x as f64 / 32768.0).powi(2);
            n += 1;
        }
    }
    (n > 0).then(|| (10.0 * (sum / n as f64).log10()) as f32)
}

/// Set the loudness of a piece to [`TARGET_RMS_DB`] without letting a peak pass [`PEAK_LIMIT`].
pub fn normalise_loudness(s: &mut [i16]) {
    let Some(db) = speech_rms_db(s) else { return };
    let peak = s.iter().map(|&x| (x as i32).unsigned_abs()).max().unwrap_or(1) as f32 / 32768.0;
    let mut gain = 10f32.powf((TARGET_RMS_DB - db) / 20.0);
    gain = gain.clamp(0.1, 12.0);
    if peak * gain > PEAK_LIMIT {
        gain = PEAK_LIMIT / peak;
    }
    for x in s.iter_mut() {
        *x = (*x as f32 * gain).round().clamp(-32768.0, 32767.0) as i16;
    }
}

/// Fade in and out (raised cosine) over `fade` ms, so the piece starts and ends at exactly zero.
pub fn fade_edges(s: &mut [i16], rate: u32, fade: u32) {
    let n = ms(rate, fade).min(s.len() / 2);
    for i in 0..n {
        let g = 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
        s[i] = (s[i] as f32 * g) as i16;
        let j = s.len() - 1 - i;
        s[j] = (s[j] as f32 * g) as i16;
    }
    if let Some(f) = s.first_mut() {
        *f = 0;
    }
    if let Some(l) = s.last_mut() {
        *l = 0;
    }
}

/// Resample by linear interpolation (the models make 22 050 Hz; used when two voices differ).
pub fn resample(s: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || s.is_empty() || from == 0 {
        return s.to_vec();
    }
    let n = (s.len() as u64 * to as u64 / from as u64) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * from as f64 / to as f64;
            let a = pos.floor() as usize;
            let f = pos - a as f64;
            let x = s[a.min(s.len() - 1)] as f64;
            let y = s[(a + 1).min(s.len() - 1)] as f64;
            (x + (y - x) * f).round() as i16
        })
        .collect()
}

/// One piece ready to be joined: samples at `rate`, and the quiet after it.
pub struct Part {
    pub samples: Vec<i16>,
    pub rate: u32,
    pub pause_ms: u32,
}

/// Clean one piece (DC removed, silence trimmed, loudness set, edges faded).
pub fn prepare(samples: &[i16], rate: u32) -> Vec<i16> {
    let mut v = samples.to_vec();
    remove_dc(&mut v);
    let mut v = trim_silence(&v, rate);
    normalise_loudness(&mut v);
    fade_edges(&mut v, rate, FADE_MS);
    v
}

/// Join prepared pieces into one recording at `rate`: each piece is followed by silence of its pause (the last one by none).
pub fn join(parts: &[Part], rate: u32) -> Vec<i16> {
    let mut out: Vec<i16> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let s = resample(&p.samples, p.rate, rate);
        out.extend_from_slice(&s);
        if i + 1 < parts.len() {
            out.extend(std::iter::repeat_n(0i16, ms(rate, p.pause_ms)));
        }
    }
    out
}

/// Largest jump between neighbouring samples (a click shows as a very large one).
pub fn max_step(s: &[i16]) -> i32 {
    s.windows(2).map(|w| (w[1] as i32 - w[0] as i32).abs()).max().unwrap_or(0)
}

/// What a quick look at a recording finds, for the bake-off and the tests.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub seconds: f32,
    pub peak: f32,
    pub rms_db: f32,
    pub clipped: usize,
    pub leading_silence_ms: u32,
    pub trailing_silence_ms: u32,
}

pub fn check(s: &[i16], rate: u32) -> Check {
    let peak = s.iter().map(|&x| (x as i32).unsigned_abs()).max().unwrap_or(0);
    let thr = (peak as f32 * 0.004).max(8.0) as u32;
    let first = s.iter().position(|&x| (x as i32).unsigned_abs() > thr).unwrap_or(s.len());
    let last = s.iter().rposition(|&x| (x as i32).unsigned_abs() > thr).map_or(0, |l| s.len() - 1 - l);
    Check {
        seconds: s.len() as f32 / rate.max(1) as f32,
        peak: peak as f32 / 32768.0,
        rms_db: speech_rms_db(s).unwrap_or(-120.0),
        clipped: s.iter().filter(|&&x| x == i16::MAX || x == i16::MIN).count(),
        leading_silence_ms: (first as u64 * 1000 / rate.max(1) as u64) as u32,
        trailing_silence_ms: (last as u64 * 1000 / rate.max(1) as u64) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f32, amp: f32, offset: i16) -> Vec<i16> {
        (0..(rate as f32 * secs) as usize).map(|i| ((i as f32 * 0.07).sin() * amp * 32767.0) as i16 + offset).collect()
    }

    #[test]
    fn pieces_of_different_loudness_come_out_equal_and_below_the_peak_limit() {
        let rate = 22050;
        let mut quiet = prepare(&tone(rate, 1.0, 0.05, 0), rate);
        let mut loud = prepare(&tone(rate, 1.0, 0.95, 0), rate);
        let (q, l) = (speech_rms_db(&quiet).unwrap(), speech_rms_db(&loud).unwrap());
        assert!((q - l).abs() < 2.0, "{q} vs {l}");
        for s in [&mut quiet, &mut loud] {
            let p = s.iter().map(|&x| (x as i32).abs()).max().unwrap() as f32 / 32768.0;
            assert!(p <= PEAK_LIMIT + 0.01, "{p}");
        }
        // a sine of RMS target reached when the peak allows it
        assert!((q - TARGET_RMS_DB).abs() < 2.5 || q > TARGET_RMS_DB - 6.0, "{q}");
    }

    #[test]
    fn silence_is_trimmed_but_a_little_is_kept() {
        let rate = 22050;
        let mut s = vec![0i16; 8000];
        s.extend(tone(rate, 0.5, 0.5, 0));
        s.extend(vec![0i16; 9000]);
        let t = trim_silence(&s, rate);
        let keep = (rate * KEEP_MS / 1000) as usize;
        assert!(t.len() < s.len() - 10000 && t.len() > (rate as usize / 2), "{}", t.len());
        let c = check(&t, rate);
        assert!(c.leading_silence_ms <= KEEP_MS + 2 && c.trailing_silence_ms <= KEEP_MS + 2, "{c:?} keep {keep}");
        assert!(trim_silence(&vec![0; 100], rate).is_empty() && trim_silence(&[], rate).is_empty());
    }

    #[test]
    fn a_constant_offset_is_removed_and_joins_have_no_click() {
        let rate = 22050;
        let a = prepare(&tone(rate, 0.6, 0.4, 3000), rate);
        let b = prepare(&tone(rate, 0.6, 0.8, -2500), rate);
        assert_eq!((a[0], *a.last().unwrap(), b[0], *b.last().unwrap()), (0, 0, 0, 0), "every piece starts and ends at zero");
        // raw join: the offsets give a big jump; prepared join: none above the signal's own step
        let raw = [tone(rate, 0.6, 0.4, 3000), tone(rate, 0.6, 0.8, -2500)].concat();
        assert!(max_step(&raw) > 5000);
        let joined = join(&[Part { samples: a.clone(), rate, pause_ms: 150 }, Part { samples: b.clone(), rate, pause_ms: 0 }], rate);
        let own = max_step(&a).max(max_step(&b));
        assert!(max_step(&joined) <= own, "{} vs {own}", max_step(&joined));
        // pause lengths: 150 ms of exact silence between the pieces, none after the last
        let gap = (rate as usize * 150) / 1000;
        assert_eq!(joined.len(), a.len() + gap + b.len());
        assert!(joined[a.len()..a.len() + gap].iter().all(|&x| x == 0));
    }

    #[test]
    fn resampling_keeps_the_duration() {
        let s = tone(16000, 1.0, 0.5, 0);
        let r = resample(&s, 16000, 22050);
        assert!((r.len() as i64 - 22050).abs() <= 1);
        assert_eq!(resample(&s, 16000, 16000), s);
    }

    #[test]
    fn the_check_sees_silence_and_clipping() {
        let rate = 22050;
        let mut s = vec![0i16; 2205];
        s.extend(vec![i16::MAX; 10]);
        s.extend(tone(rate, 0.3, 0.3, 0));
        s.extend(vec![0i16; 4410]);
        let c = check(&s, rate);
        assert_eq!(c.clipped, 10);
        assert!((95..=105).contains(&c.leading_silence_ms) && (195..=205).contains(&c.trailing_silence_ms), "{c:?}");
    }
}
