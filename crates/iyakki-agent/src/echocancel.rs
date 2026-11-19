//! Echo cancellation for "My IYAKKI" (F-VOICE-36, plan step 5): an adaptive filter (normalised least mean squares) that learns how the
//! assistant's own voice, as the speaker plays it, comes back through the microphone, and subtracts that estimate. What is left is the
//! person (and the room's noise). It replaces the fixed gate "microphone level above 0.20 + 1.5 x speaker level" once a reference of the
//! mixer's output is available.
//!
//! The filter is in-house (no library). 16 kHz mono floats in -1.0 to 1.0. The caller gives, for every block of microphone samples, the
//! block of samples that was sent to the speaker at the same moment; [`EchoCanceller::with_delay`] says how many samples later the
//! sound reaches the microphone at the very least (the sound card's latency), which lets the filter spend its taps on the room.
//!
//! Double talk (the person speaking over the answer, the very case barge-in needs) would make a plain filter learn the person's voice as
//! if it were echo. A Geigel detector freezes the learning while the microphone is much louder than the reference could explain.

/// Taps of the default filter: 1600 samples = 100 ms of room at 16 kHz.
pub const DEFAULT_TAPS: usize = 1600;

pub struct EchoCanceller {
    w: Vec<f32>,
    /// The last `taps` reference samples, newest first at `pos` going backwards (a ring).
    x: Vec<f32>,
    pos: usize,
    /// Sum of squares of the reference in the ring.
    energy: f32,
    mu: f32,
    /// Reference delayed by the sound card's latency before it enters the filter.
    delay: std::collections::VecDeque<f32>,
    delay_len: usize,
    peak: f32,
    /// The microphone must be this many times louder than the loudest recent reference sample to count as someone else talking.
    geigel: f32,
    /// Samples since the near end last looked like speech (adaptation stays frozen a little after it ends).
    hang: u32,
}

impl EchoCanceller {
    pub fn new(taps: usize) -> EchoCanceller {
        let taps = taps.clamp(16, 8192);
        EchoCanceller { w: vec![0.0; taps], x: vec![0.0; taps], pos: 0, energy: 0.0, mu: 0.5, delay: std::collections::VecDeque::new(), delay_len: 0, peak: 0.0, geigel: 1.2, hang: 0 }
    }

    /// The sound card's latency in samples (at most 4000): the reference is held back that long before it is used.
    pub fn with_delay(mut self, samples: usize) -> EchoCanceller {
        self.delay_len = samples.min(4000);
        self.delay = std::iter::repeat_n(0.0, self.delay_len).collect();
        self
    }

    /// Step size (0.05 to 1.0): smaller learns slower and is steadier.
    pub fn with_step(mut self, mu: f32) -> EchoCanceller {
        self.mu = mu.clamp(0.05, 1.0);
        self
    }

    pub fn taps(&self) -> usize {
        self.w.len()
    }

    /// One microphone sample and the reference sample played at the same moment; the microphone sample with the echo taken away.
    pub fn process_sample(&mut self, mic: f32, reference: f32) -> f32 {
        let mic = if mic.is_finite() { mic.clamp(-4.0, 4.0) } else { 0.0 };
        let r = if reference.is_finite() { reference.clamp(-4.0, 4.0) } else { 0.0 };
        let r = if self.delay_len > 0 {
            self.delay.push_back(r);
            self.delay.pop_front().unwrap_or(0.0)
        } else {
            r
        };
        // the reference enters the ring
        let n = self.x.len();
        self.pos = if self.pos == 0 { n - 1 } else { self.pos - 1 };
        let old = self.x[self.pos];
        self.energy = (self.energy - old * old + r * r).max(0.0);
        self.x[self.pos] = r;
        self.peak = (self.peak * 0.9995).max(r.abs());
        // the estimate of the echo: the filter applied to the last `n` reference samples
        let (a, b) = self.x.split_at(self.pos);
        let y: f32 = b.iter().zip(&self.w).map(|(x, w)| x * w).sum::<f32>() + a.iter().zip(&self.w[b.len()..]).map(|(x, w)| x * w).sum::<f32>();
        let e = mic - y;
        // learning: only while the reference carries something and nobody else seems to be talking
        let near = mic.abs() > self.geigel * self.peak.max(1e-4) + 0.02;
        if near {
            self.hang = 800; // 50 ms of caution after it
        } else if self.hang > 0 {
            self.hang -= 1;
        }
        if self.hang == 0 && self.energy > 1e-4 {
            let g = self.mu * e / (self.energy + 1e-3);
            let (wa, wb) = self.w.split_at_mut(b.len());
            for (w, x) in wa.iter_mut().zip(b) {
                *w += g * x;
            }
            for (w, x) in wb.iter_mut().zip(a) {
                *w += g * x;
            }
        }
        e.clamp(-1.0, 1.0)
    }

    /// A block: `mic` and `reference` are the same length (a shorter reference is taken as followed by silence).
    pub fn process(&mut self, mic: &[f32], reference: &[f32]) -> Vec<f32> {
        mic.iter().enumerate().map(|(i, m)| self.process_sample(*m, reference.get(i).copied().unwrap_or(0.0))).collect()
    }

    /// Forget what was learned (the speaker or the room changed).
    pub fn reset(&mut self) {
        self.w.iter_mut().for_each(|w| *w = 0.0);
        self.x.iter_mut().for_each(|x| *x = 0.0);
        self.energy = 0.0;
        self.peak = 0.0;
        self.hang = 0;
        self.delay.iter_mut().for_each(|d| *d = 0.0);
    }
}

/// Root mean square of a block (0 for an empty one).
pub fn rms(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt() }
}

/// How many decibels quieter `after` is than `before` (the echo return loss enhancement). Large is good.
pub fn erle_db(before: &[f32], after: &[f32]) -> f32 {
    20.0 * ((rms(before) + 1e-9) / (rms(after) + 1e-9)).log10()
}

/// The level the barge-in gate looks at: the microphone with the assistant's own echo taken away, 0 to 1.
pub fn barge_level(residual: &[f32]) -> f32 {
    (rms(residual) * 4.0).min(1.0)
}

// ------------------------------------------------------------------ what was played, for cleaning a capture afterwards (F-VOICE-36)

/// The sample rate of captures and references.
pub const RATE: usize = 16_000;

struct Seg {
    start_ms: u64,
    pcm: Vec<f32>,
    stop_ms: Option<u64>,
}

/// What the assistant itself played lately, with when each piece started (milliseconds on one clock): the reference of the canceller.
/// The assistant's own speech is the one sound whose samples are known exactly, so no tap of the mixer is needed for it.
#[derive(Default)]
pub struct Reference {
    segs: std::collections::VecDeque<Seg>,
}

impl Reference {
    /// A piece of 16 kHz mono sound (-1 to 1) that started playing at `start_ms`.
    pub fn add(&mut self, start_ms: u64, pcm: Vec<f32>) {
        self.segs.push_back(Seg { start_ms, pcm, stop_ms: None });
        while self.segs.len() > 4 {
            self.segs.pop_front();
        }
    }

    /// Everything playing was stopped at `at_ms` (a barge-in, Stop).
    pub fn stop_all(&mut self, at_ms: u64) {
        for s in self.segs.iter_mut().filter(|s| s.stop_ms.is_none()) {
            s.stop_ms = Some(at_ms);
        }
    }

    pub fn clear(&mut self) {
        self.segs.clear();
    }

    /// The reference for a capture that began at `start_ms` and holds `len` samples: the playing sound where there was any, zeros elsewhere.
    pub fn for_capture(&self, start_ms: u64, len: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; len];
        for s in &self.segs {
            // the sample of the capture at which this piece starts (negative: it began before the capture did)
            let off = (s.start_ms as i64 - start_ms as i64) * (RATE as i64 / 1000);
            let end = s.stop_ms.map(|t| (t as i64 - s.start_ms as i64).max(0) as usize * (RATE / 1000)).unwrap_or(usize::MAX).min(s.pcm.len());
            for (k, v) in s.pcm.iter().take(end).enumerate() {
                let i = off + k as i64;
                if (0..len as i64).contains(&i) {
                    out[i as usize] += *v;
                }
            }
        }
        out
    }
}

// ------------------------------------------------------------------ the whole output of the mixer as the reference (F-VOICE-44)

/// What the mixer wrote to the speaker lately, as 16 kHz mono floats with the time the newest sample was written: the reference that holds
/// EVERY app's sound, not only the assistant's own voice. The mixer's tee feeds it, but only while a voice session with echo cancellation wants
/// it (`want`): otherwise one atomic read per period and nothing kept.
pub struct MixedOutput {
    ring: std::collections::VecDeque<f32>,
    /// The time (ms on the echo clock) at which the newest sample in `ring` was written to the sound card.
    end_ms: u64,
    rs: Option<(u32, usize, crate::mixer::resample::Resampler)>,
}

/// How much is kept: ten seconds.
const MIXED_KEEP: usize = RATE * 10;
/// A voice session that wants the reference asks again at least this often (ms).
const WANT_FOR_MS: u64 = 60_000;

static WANT_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A voice session with echo cancellation is running or about to: the mixer feeds the reference for the next minute.
pub fn want_mixed_output(now_ms: u64) {
    WANT_UNTIL.store(now_ms + WANT_FOR_MS, std::sync::atomic::Ordering::Relaxed);
}

pub fn mixed_output_wanted(now_ms: u64) -> bool {
    WANT_UNTIL.load(std::sync::atomic::Ordering::Relaxed) > now_ms
}

impl MixedOutput {
    pub fn new() -> MixedOutput {
        MixedOutput { ring: std::collections::VecDeque::new(), end_ms: 0, rs: None }
    }

    /// One period of interleaved 16-bit sound at `rate` and `channels`, written at `now_ms`.
    pub fn feed(&mut self, rate: u32, channels: usize, pcm: &[i16], now_ms: u64) {
        if channels == 0 || rate == 0 || pcm.is_empty() {
            return;
        }
        if self.rs.as_ref().is_none_or(|(r, c, _)| *r != rate || *c != channels) {
            self.rs = Some((rate, channels, crate::mixer::resample::Resampler::new(rate, RATE as u32, channels)));
        }
        let f: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
        let mut out = Vec::new();
        if let Some((_, _, rs)) = self.rs.as_mut() {
            rs.process(&f, &mut out);
        }
        // the channels are averaged into one
        for fr in out.chunks(channels) {
            self.ring.push_back(fr.iter().sum::<f32>() / channels as f32);
        }
        while self.ring.len() > MIXED_KEEP {
            self.ring.pop_front();
        }
        self.end_ms = now_ms;
    }

    /// The reference for a capture that began at `start_ms` and holds `len` samples: what was written meanwhile, zeros where nothing was.
    pub fn for_capture(&self, start_ms: u64, len: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; len];
        let n = self.ring.len() as i64;
        // the ring's first sample was written this long before `end_ms`
        let first_ms = self.end_ms as i64 - n * 1000 / RATE as i64;
        let off = (first_ms - start_ms as i64) * (RATE as i64 / 1000);
        for (k, v) in self.ring.iter().enumerate() {
            let i = off + k as i64;
            if (0..len as i64).contains(&i) {
                out[i as usize] = *v;
            }
        }
        out
    }
}

impl Default for MixedOutput {
    fn default() -> Self {
        MixedOutput::new()
    }
}

/// The one the mixer's tee feeds.
pub fn mixed_output() -> std::sync::MutexGuard<'static, MixedOutput> {
    static M: std::sync::OnceLock<std::sync::Mutex<MixedOutput>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(MixedOutput::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// What the mixer's tee calls with every period it writes: kept only while a voice session wants it.
pub fn feed_mixed_output(rate: u32, channels: usize, pcm: &[i16], now_ms: u64) {
    if mixed_output_wanted(now_ms) {
        mixed_output().feed(rate, channels, pcm, now_ms);
    }
}

/// Take the echo of EVERYTHING the speaker played out of a capture (the mixer's whole output, [`MixedOutput`]), instead of only the assistant's
/// own voice. A capture that overlaps no sound is returned as it was.
pub fn clean_capture_mixed(pcm: &[i16], start_ms: u64, mixed: &MixedOutput) -> Cleaned {
    let r = mixed.for_capture(start_ms, pcm.len());
    clean_with(pcm, &r)
}

fn clean_with(pcm: &[i16], r: &[f32]) -> Cleaned {
    if !r.iter().any(|v| v.abs() > 1e-4) {
        return Cleaned { pcm: pcm.to_vec(), touched: false };
    }
    let mic: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
    let mut ec = EchoCanceller::new(3200);
    let out = ec.process(&mic, r);
    Cleaned { pcm: out.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect(), touched: true }
}

/// A capture with the assistant's own echo taken away.
pub struct Cleaned {
    pub pcm: Vec<i16>,
    /// The reference overlapped the capture, so the filter ran; false: the capture is returned as it was.
    pub touched: bool,
}

/// Take the echo of what was played out of a capture of the microphone (16 kHz mono). `start_ms` is when the capture began on the
/// reference's clock. A capture that overlaps no playing sound is returned untouched.
pub fn clean_capture(pcm: &[i16], start_ms: u64, reference: &Reference) -> Cleaned {
    let r = reference.for_capture(start_ms, pcm.len());
    if !r.iter().any(|v| v.abs() > 1e-4) {
        return Cleaned { pcm: pcm.to_vec(), touched: false };
    }
    let mic: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
    // the filter covers 200 ms of delay and room, which holds the sound card's latency
    let mut ec = EchoCanceller::new(3200);
    let out = ec.process(&mic, &r);
    Cleaned { pcm: out.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect(), touched: true }
}

// ------------------------------------------------------------------ streaming (the Gemini voice session, F-GEMVOICE-10)

/// 24 kHz 16-bit PCM (what Gemini's voice sends) to 16 kHz floats: every three input samples become two (linear interpolation; the
/// speech band of the voice is well below 8 kHz, so no extra filter is used). The result is the reference the canceller needs.
pub fn resample_24k_to_16k(pcm: &[i16]) -> Vec<f32> {
    let n = pcm.len() * 2 / 3;
    (0..n)
        .map(|i| {
            let pos = i as f32 * 1.5;
            let k = pos as usize;
            let f = pos - k as f32;
            let a = f32::from(pcm[k]) / 32768.0;
            let b = f32::from(*pcm.get(k + 1).unwrap_or(&pcm[k])) / 32768.0;
            a + (b - a) * f
        })
        .collect()
}

/// The canceller for a microphone that is read chunk after chunk: one filter that keeps what it learned from chunk to chunk, and the
/// reference of what was played ([`Reference`]). Used by the live voice session so the speaker does not retrigger the microphone.
pub struct StreamCleaner {
    ec: EchoCanceller,
    pub reference: std::sync::Arc<std::sync::Mutex<Reference>>,
}

impl StreamCleaner {
    /// 200 ms of room and sound card latency (the filter of [`clean_capture`]).
    pub fn new(reference: std::sync::Arc<std::sync::Mutex<Reference>>) -> StreamCleaner {
        StreamCleaner { ec: EchoCanceller::new(3200), reference }
    }

    /// `pcm` is 16 kHz mono that ended at `end_ms` on the reference's clock. Returns the chunk with the echo taken away and the level of
    /// what is left (0 to 1, [`barge_level`]): about 0 for pure echo, high for a person.
    pub fn process(&mut self, pcm: &[i16], end_ms: u64) -> (Vec<i16>, f32) {
        let start = end_ms.saturating_sub((pcm.len() * 1000 / RATE) as u64);
        let r = self.reference.lock().unwrap_or_else(std::sync::PoisonError::into_inner).for_capture(start, pcm.len());
        let mic: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
        let out = self.ec.process(&mic, &r);
        (out.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect(), barge_level(&out))
    }

    /// The room or the speaker changed: learn again.
    pub fn reset(&mut self) {
        self.ec.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise in -0.5..0.5 (a linear congruential generator): no clock, no randomness.
    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((s >> 8) as f32 / (1u32 << 24) as f32) - 0.5
            })
            .collect()
    }

    /// The room: the reference comes back three times, later and quieter, once upside down.
    fn echo_of(reference: &[f32], taps: &[(usize, f32)]) -> Vec<f32> {
        (0..reference.len()).map(|i| taps.iter().map(|(d, g)| if i >= *d { reference[i - d] * g } else { 0.0 }).sum()).collect()
    }

    const ROOM: [(usize, f32); 3] = [(20, 0.6), (90, 0.3), (200, -0.1)];

    #[test]
    fn the_echo_of_the_assistants_own_voice_is_learned_and_taken_away() {
        let r = noise(32_000, 7);
        let mic = echo_of(&r, &ROOM);
        let mut ec = EchoCanceller::new(512);
        let out = ec.process(&mic, &r);
        let (b, a) = (&mic[28_000..], &out[28_000..]);
        let erle = erle_db(b, a);
        assert!(erle > 25.0, "after two seconds the echo is {erle:.1} dB quieter");
        // at the start nothing is learned yet
        assert!(erle_db(&mic[..200], &out[..200]) < 6.0);
        assert!(barge_level(a) < 0.02, "its own echo does not look like a person: {}", barge_level(a));
    }

    #[test]
    fn a_person_talking_over_the_answer_stays_and_is_not_learned_as_echo() {
        let r = noise(48_000, 11);
        let echo = echo_of(&r, &ROOM);
        let mut ec = EchoCanceller::new(512);
        // learn the room first
        ec.process(&echo[..32_000], &r[..32_000]);
        // the person speaks (a tone) over the answer for one second
        let tone: Vec<f32> = (0..16_000).map(|i| 0.45 * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin()).collect();
        let mic: Vec<f32> = (0..16_000).map(|i| echo[32_000 + i] + tone[i]).collect();
        let out = ec.process(&mic, &r[32_000..48_000]);
        let kept = rms(&out[4_000..]);
        let want = rms(&tone[4_000..]);
        assert!((kept - want).abs() / want < 0.25, "the voice is {kept:.3}, it was {want:.3}");
        assert!(barge_level(&out[4_000..]) > 0.5, "and it is plainly louder than echo");
        // and the filter still works after it: the echo alone is cancelled again
        let after = ec.process(&echo[..8_000], &r[..8_000]);
        assert!(erle_db(&echo[4_000..8_000], &after[4_000..]) > 15.0);
    }

    #[test]
    fn without_a_reference_the_microphone_passes_unchanged() {
        let mic: Vec<f32> = noise(4_000, 3).iter().map(|x| x * 0.4).collect();
        let mut ec = EchoCanceller::new(256);
        let out = ec.process(&mic, &[]);
        assert_eq!(out, mic);
    }

    #[test]
    fn a_changed_room_is_learned_again_and_a_reset_forgets() {
        let r = noise(64_000, 21);
        let mut ec = EchoCanceller::new(512);
        let room1 = echo_of(&r[..32_000], &ROOM);
        ec.process(&room1, &r[..32_000]);
        let room2 = echo_of(&r[32_000..], &[(40, -0.5), (150, 0.4)]);
        let out = ec.process(&room2, &r[32_000..]);
        assert!(erle_db(&room2[28_000..], &out[28_000..]) > 20.0, "the new room is learned");
        ec.reset();
        let again = ec.process(&room2[..200], &r[32_000..32_200]);
        assert!(erle_db(&room2[..200], &again) < 6.0, "after a reset nothing is subtracted at first");
    }

    #[test]
    fn the_sound_card_latency_is_held_back_and_bad_numbers_never_escape() {
        // the room echo arrives 300 samples late because of the card; a canceller told so learns it with a short filter
        let r = noise(32_000, 5);
        let mic = echo_of(&r, &[(300, 0.7)]);
        let mut late = EchoCanceller::new(64).with_delay(290);
        let out = late.process(&mic, &r);
        assert!(erle_db(&mic[28_000..], &out[28_000..]) > 25.0);
        let mut plain = EchoCanceller::new(64);
        let out2 = plain.process(&mic, &r);
        assert!(erle_db(&mic[28_000..], &out2[28_000..]) < 6.0, "a short filter alone cannot reach that far");
        // NaN, infinity and huge samples do not poison the filter
        let mut ec = EchoCanceller::new(64);
        let o = ec.process(&[f32::NAN, f32::INFINITY, 1e9, -1e9, 0.1], &[1e9, f32::NAN, f32::NEG_INFINITY, 0.5, 0.5]);
        assert!(o.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert!(ec.process(&[0.1; 100], &[0.1; 100]).iter().all(|x| x.is_finite()));
        assert_eq!(ec.taps(), 64);
        assert_eq!(rms(&[]), 0.0);
    }

    // ------------------------------------------------------------------ cleaning a capture with what the assistant played

    /// Speech-like far-end sound: noise in syllable-sized bursts (deterministic).
    fn far_end(n: usize) -> Vec<f32> {
        let noise = noise(n, 99);
        (0..n).map(|i| noise[i] * (0.5 + 0.5 * ((i as f32 / 2400.0).sin()).abs())).collect()
    }

    fn to_i16(v: &[f32]) -> Vec<i16> {
        v.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
    }

    /// A stand-in for a recording: the far end played from 200 ms into the capture, the room 60 ms and 140 ms behind it, and a person
    /// speaking (a tone) from 1.5 s to 2.5 s of the capture. (No real room was recorded: this is a synthetic fixture.)
    fn fixture() -> (Vec<i16>, Vec<f32>, Vec<f32>) {
        let (n, lead) = (RATE * 4, RATE / 5);
        let played = far_end(RATE * 3);
        let mut mic = vec![0.0f32; n];
        for (k, v) in played.iter().enumerate() {
            for (d, g) in [(960usize, 0.5f32), (2240, -0.2)] {
                let i = lead + k + d;
                if i < n {
                    mic[i] += v * g;
                }
            }
        }
        let tone: Vec<f32> = (0..RATE).map(|i| 0.4 * (i as f32 * 2.0 * std::f32::consts::PI * 300.0 / RATE as f32).sin()).collect();
        for (k, v) in tone.iter().enumerate() {
            mic[RATE * 3 / 2 + k] += v;
        }
        (to_i16(&mic), played, tone)
    }

    fn rms16(v: &[i16]) -> f32 {
        rms(&v.iter().map(|x| f32::from(*x) / 32768.0).collect::<Vec<_>>())
    }

    #[test]
    fn a_capture_made_while_the_assistant_spoke_loses_its_echo_and_keeps_the_person() {
        let (cap, played, _) = fixture();
        let mut r = Reference::default();
        // the capture began at 10 000 ms; the sound started 200 ms later
        r.add(10_200, played);
        let c = clean_capture(&cap, 10_000, &r);
        assert!(c.touched && c.pcm.len() == cap.len());
        // the last second of the far end alone (2.5 s to 3.2 s of the capture) is much quieter
        let (a, b) = (RATE * 5 / 2 + 2000, RATE * 3 + 2000);
        let drop = 20.0 * (rms16(&cap[a..b]) / rms16(&c.pcm[a..b]).max(1e-6)).log10();
        assert!(drop > 15.0, "the echo is {drop:.1} dB quieter");
        // the person's second (1.5 s to 2.5 s) keeps its level: the tone alone is 0.4 peak, about 0.28 rms, on top of some echo
        let mid = RATE * 3 / 2 + 4000..RATE * 5 / 2 - 2000;
        let tone_rms = 0.4 / std::f32::consts::SQRT_2;
        let kept = rms16(&c.pcm[mid.clone()]);
        assert!((kept - tone_rms).abs() / tone_rms < 0.35, "the voice is {kept:.3}, the tone was {tone_rms:.3}");
        assert!(rms16(&cap[mid]) > kept * 0.9, "raw already had it");
        // before the sound began (the first 200 ms) nothing changes
        assert_eq!(&c.pcm[..RATE / 10], &cap[..RATE / 10]);
    }

    #[test]
    fn the_whole_output_of_the_mixer_is_a_reference_so_the_sound_of_other_apps_is_taken_out_too_and_it_is_kept_only_when_wanted() {
        let (_, played_raw, tone) = fixture();
        // the other app's sound is band-limited (a real mixer stream is): a moving average of the fixture's noise
        let played: Vec<f32> = (0..played_raw.len()).map(|i| (0..4).map(|k| played_raw[i.saturating_sub(k)]).sum::<f32>() / 4.0).collect();
        let n = RATE * 4;
        let mut micf = vec![0.0f32; n];
        for (k, v) in played.iter().enumerate() {
            for (d, g) in [(960usize, 0.5f32), (2240, -0.2)] {
                if RATE / 5 + k + d < n {
                    micf[RATE / 5 + k + d] += v * g;
                }
            }
        }
        for (k, v) in tone.iter().enumerate() {
            micf[RATE * 3 / 2 + k] += v;
        }
        let cap = to_i16(&micf);
        // another app's sound reaches the speaker at 48 kHz stereo, ten milliseconds at a time (the assistant said nothing at all)
        let feed = |m: &mut MixedOutput| {
            let mut frames: Vec<i16> = Vec::new();
            for v in &played {
                let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                for _ in 0..3 {
                    frames.push(s);
                    frames.push(s);
                }
            }
            for (p, period) in frames.chunks(960).enumerate() {
                m.feed(48_000, 2, period, 10_200 + (p as u64 + 1) * 10);
            }
        };
        let mut m = MixedOutput::new();
        feed(&mut m);
        let c = clean_capture_mixed(&cap, 10_000, &m);
        assert!(c.touched && c.pcm.len() == cap.len());
        let (a, b) = (RATE * 5 / 2 + 2000, RATE * 3 + 2000);
        let drop = 20.0 * (rms16(&cap[a..b]) / rms16(&c.pcm[a..b]).max(1e-6)).log10();
        assert!(drop > 12.0, "the other app's echo is {drop:.1} dB quieter");
        let mid = RATE * 3 / 2 + 4000..RATE * 5 / 2 - 2000;
        let tone_rms = 0.4 / std::f32::consts::SQRT_2;
        assert!((rms16(&c.pcm[mid]) - tone_rms).abs() / tone_rms < 0.4, "the person's voice stays");
        // nothing was played: the capture is left alone; a capture far from what was played too
        assert!(!clean_capture_mixed(&cap, 10_000, &MixedOutput::new()).touched);
        assert!(!clean_capture_mixed(&cap, 90_000, &m).touched);
        // only about ten seconds are kept
        let mut long = MixedOutput::new();
        for p in 0..2000u64 {
            long.feed(48_000, 2, &[100i16; 960], 1000 + p * 10);
        }
        assert!(long.ring.len() <= RATE * 10);
        // the tee feeds it only while a voice session wants it
        assert!(!mixed_output_wanted(5_000_000_000));
        want_mixed_output(5_000_000_000);
        assert!(mixed_output_wanted(5_000_000_000 + 1000) && !mixed_output_wanted(5_000_000_000 + 61_000));
    }

    #[test]
    fn a_capture_with_nothing_played_and_one_after_a_stop_are_left_as_they_were() {
        let (cap, played, _) = fixture();
        let none = Reference::default();
        let c = clean_capture(&cap, 10_000, &none);
        assert!(!c.touched && c.pcm == cap);
        // played long before the capture: no overlap
        let mut old = Reference::default();
        old.add(1_000, played.clone());
        assert!(!clean_capture(&cap, 20_000, &old).touched);
        // a barge-in at 400 ms stops the reference there: later echo (there is none in a real stopped sound) is not subtracted
        let mut stopped = Reference::default();
        stopped.add(10_200, played);
        stopped.stop_all(10_600);
        let r = stopped.for_capture(10_000, RATE * 4);
        assert!(r[RATE * 3 / 5..].iter().all(|v| *v == 0.0), "nothing after the stop");
        assert!(r[RATE / 4..RATE / 2].iter().any(|v| v.abs() > 0.0));
        stopped.clear();
        assert!(!clean_capture(&cap, 10_000, &stopped).touched);
    }

    #[test]
    fn the_reference_keeps_the_last_four_pieces_and_places_each_on_the_captures_clock() {
        let mut r = Reference::default();
        for i in 0..6u64 {
            r.add(i * 1000, vec![0.5; 16]);
        }
        assert_eq!(r.segs.len(), 4);
        let f = r.for_capture(4_000, 16 * 1000 * 2);
        assert!(f[0] > 0.0 && f[16] == 0.0, "the piece at 4 s begins the capture; the gap after 1 ms is silent");
        assert!(r.for_capture(0, 16 * 1000)[..16].iter().all(|v| *v == 0.0), "the pieces before 2 s were dropped");
        // a piece that began before the capture still reaches into it
        let mut s = Reference::default();
        s.add(1_000, vec![0.25; 16 * 100]);
        assert!(s.for_capture(1_050, 16 * 100)[..16 * 50].iter().all(|v| (*v - 0.25).abs() < 1e-6));
    }

    // ---- streaming (synthetic signals only: a generated far end through a generated room, not a recording of a real one)

    fn streamed(echo_db: f32, person: Option<(usize, usize)>) -> (f32, f32, f32) {
        // the model's voice, 3 s of noise, 24 kHz as Gemini sends it, played from sample 0 of the clock
        let far24: Vec<i16> = noise(108_000, 21).iter().map(|v| (v * 20_000.0) as i16).collect();
        let far16 = resample_24k_to_16k(&far24);
        let gain = 10f32.powf(-echo_db / 20.0);
        let room: Vec<(usize, f32)> = ROOM.iter().map(|(d, g)| (*d, g * gain)).collect();
        let echo = echo_of(&far16, &room);
        let tone = |i: usize| 0.3 * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin();
        let mic: Vec<f32> = (0..echo.len()).map(|i| echo[i] + person.filter(|(a, b)| (*a..*b).contains(&i)).map_or(0.0, |_| tone(i))).collect();
        let reference = std::sync::Arc::new(std::sync::Mutex::new(Reference::default()));
        reference.lock().unwrap().add(1000, far16.clone());
        let mut sc = StreamCleaner::new(reference);
        let mut out: Vec<i16> = Vec::new();
        let mut last_level = 0.0;
        for (k, chunk) in to_i16(&mic).chunks(1600).enumerate() {
            // the chunk ends (k+1)*100 ms after the speaker started, on the same clock
            let (o, level) = sc.process(chunk, 1000 + (k as u64 + 1) * 100);
            out.extend(o);
            last_level = level;
        }
        let n = mic.len();
        let before = rms(&mic[n - 8000..]);
        let after = rms(&out[n - 8000..].iter().map(|s| f32::from(*s) / 32768.0).collect::<Vec<_>>());
        (before, after, last_level)
    }

    #[test]
    fn a_streamed_microphone_loses_the_echo_of_a_known_signal_by_a_wide_margin_at_each_echo_strength() {
        for echo_db in [0.0f32, 6.0, 12.0, 20.0] {
            let (before, after, level) = streamed(echo_db, None);
            let erle = 20.0 * ((before + 1e-9) / (after + 1e-9)).log10();
            assert!(erle > 20.0, "an echo {echo_db} dB below the voice is taken {erle:.1} dB further down");
            assert!(level < 0.1, "what is left does not look like a person ({level})");
        }
    }

    #[test]
    fn a_person_talking_over_the_streamed_voice_is_kept_and_stands_out() {
        // the person speaks (a tone) during the last second
        let (_, after, level) = streamed(6.0, Some((56_000, 72_000)));
        assert!(level > 0.5, "the person is plainly louder than the left-over echo ({level})");
        assert!(after > 0.15, "the tone (about 0.21 rms) is not cancelled: {after}");
    }

    #[test]
    fn the_resampler_keeps_the_length_ratio_and_a_tone_inside_the_band() {
        let tone: Vec<i16> = (0..2400).map(|i| (10_000.0 * (i as f32 * 2.0 * std::f32::consts::PI * 300.0 / 24_000.0).sin()) as i16).collect();
        let r = resample_24k_to_16k(&tone);
        assert_eq!(r.len(), 1600);
        let want = 10_000.0 / 32768.0 / 2f32.sqrt();
        assert!((rms(&r) - want).abs() / want < 0.05, "{} vs {want}", rms(&r));
        assert!(resample_24k_to_16k(&[]).is_empty() && resample_24k_to_16k(&[5]).is_empty());
    }
}
