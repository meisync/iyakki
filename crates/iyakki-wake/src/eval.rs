//! Evaluation: misses on the phrase and false accepts per hour on background audio, over a sweep of thresholds, using the same
//! streaming scorer as the live spotter (energy gate, windows every 100 ms, smoothing, cool-down). Numbers from synthetic data are
//! synthetic: `Report::synthetic_only` carries that into the printout.

use crate::augment;
use crate::features::SAMPLE_RATE;
use crate::model::Net;
use crate::rng::Rng;
use crate::spotter::{Score, Scorer};

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub threshold: f32,
    pub miss_rate: f32,
    pub false_accepts_per_hour: f32,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub rows: Vec<Row>,
    pub positives: usize,
    pub background_hours: f32,
    pub synthetic_only: bool,
}

/// Smoothed scores over a whole stream.
pub fn trace(net: &Net, audio: &[f32]) -> Vec<Score> {
    let mut sc = Scorer::new(net.clone());
    let mut out = Vec::new();
    for chunk in audio.chunks(1600) {
        sc.feed(chunk, &mut |s| out.push(s));
    }
    out
}

/// Wakes a threshold and cool-down would give on a trace.
pub fn wakes(trace: &[Score], threshold: f32, cooldown_ms: u64) -> usize {
    let mut quiet = 0;
    let mut n = 0;
    for s in trace {
        if s.smoothed >= threshold && s.at_ms >= quiet {
            n += 1;
            quiet = s.at_ms + cooldown_ms;
        }
    }
    n
}

/// A positive clip inside a quiet-room stream: lead-in, the clip (mixed with `bg` at `snr_db` when given), a tail.
pub fn embed(clip: &[f32], bg: Option<&[f32]>, snr_db: f32, rng: &mut Rng) -> Vec<f32> {
    let mut c = clip.to_vec();
    if let Some(b) = bg {
        augment::add_noise(&mut c, b, snr_db, rng);
    }
    let mut x: Vec<f32> = (0..2 * SAMPLE_RATE).map(|_| rng.normal() * 0.0003).collect();
    x.extend(c);
    x.extend((0..SAMPLE_RATE * 3 / 2).map(|_| rng.normal() * 0.0003));
    x
}

pub fn sweep(net: &Net, positives: &[Vec<f32>], background: &[Vec<f32>], snr_db: f32, thresholds: &[f32], cooldown_ms: u64, synthetic_only: bool) -> Report {
    let mut rng = Rng::new(77);
    let pos_traces: Vec<Vec<Score>> = positives.iter().map(|p| trace(net, &embed(p, background.first().map(|v| v.as_slice()), snr_db, &mut rng))).collect();
    let bg_traces: Vec<Vec<Score>> = background.iter().map(|b| trace(net, b)).collect();
    let samples: usize = background.iter().map(|b| b.len()).sum();
    let hours = samples as f32 / SAMPLE_RATE as f32 / 3600.0;
    let rows = thresholds
        .iter()
        .map(|&t| {
            let missed = pos_traces.iter().filter(|tr| !tr.iter().any(|s| s.smoothed >= t)).count();
            let fa: usize = bg_traces.iter().map(|tr| wakes(tr, t, cooldown_ms)).sum();
            Row { threshold: t, miss_rate: if positives.is_empty() { f32::NAN } else { missed as f32 / positives.len() as f32 }, false_accepts_per_hour: if hours > 0.0 { fa as f32 / hours } else { f32::NAN } }
        })
        .collect();
    Report { rows, positives: positives.len(), background_hours: hours, synthetic_only }
}

impl Report {
    pub fn table(&self) -> String {
        let mut s = String::new();
        if self.synthetic_only {
            s.push_str("SYNTHETIC DATA ONLY: these numbers describe the formant stub's sounds, not real voices or rooms.\n");
        }
        s.push_str(&format!("positives: {}   background: {:.3} h\n", self.positives, self.background_hours));
        s.push_str("threshold   miss rate   false accepts / hour\n");
        for r in &self.rows {
            s.push_str(&format!("{:>9.2}   {:>8.1}%   {:>12.1}\n", r.threshold, r.miss_rate * 100.0, r.false_accepts_per_hour));
        }
        s
    }

    /// The lowest threshold whose false accepts per hour are at most `max_fa` (None if none is).
    pub fn pick_threshold(&self, max_fa: f32) -> Option<f32> {
        self.rows.iter().filter(|r| r.false_accepts_per_hour <= max_fa).map(|r| r.threshold).fold(None, |a: Option<f32>, t| Some(a.map_or(t, |x| x.min(t))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(v: &[(u64, f32)]) -> Vec<Score> {
        v.iter().map(|(a, b)| Score { at_ms: *a, smoothed: *b }).collect()
    }

    #[test]
    fn wake_counting_follows_the_threshold_and_the_cool_down() {
        let t = tr(&[(100, 0.9), (200, 0.95), (3000, 0.9), (3100, 0.2), (6000, 0.85)]);
        assert_eq!(wakes(&t, 0.8, 2500), 3);
        assert_eq!(wakes(&t, 0.8, 100), 4, "0.2 never counts; with a short cool-down 200 ms is after 100+100");
        assert_eq!(wakes(&t, 0.99, 2500), 0);
    }

    #[test]
    fn the_sweep_counts_misses_and_false_accepts_per_hour_by_hand() {
        // an always-yes network: every loud positive is found, every loud background is a false accept
        let mut net = Net::zeros();
        let n = Net::param_count();
        net.params[n - 1] = 9.0;
        let mut r = Rng::new(1);
        let loud = |len: usize, r: &mut Rng| -> Vec<f32> { (0..len).map(|_| r.normal() * 0.2).collect() };
        let positives = vec![loud(16_000, &mut r), loud(14_000, &mut r)];
        let quiet_bg: Vec<f32> = (0..SAMPLE_RATE * 20).map(|_| r.normal() * 0.0002).collect();
        let mut loud_bg = quiet_bg.clone();
        loud_bg.extend(loud(SAMPLE_RATE, &mut r));
        loud_bg.extend(vec![0.0002; SAMPLE_RATE * 20]);
        let rep = sweep(&net, &positives, &[quiet_bg, loud_bg], 100.0, &[0.5, 0.999999], 2500, true);
        assert_eq!(rep.rows[0].miss_rate, 0.0);
        let hours = (20.0 + 41.0) / 3600.0;
        assert!((rep.background_hours - hours).abs() < 1e-4);
        assert!((rep.rows[0].false_accepts_per_hour - 1.0 / hours).abs() < 1.0, "one burst, one false accept");
        assert_eq!(rep.rows[1].miss_rate, 1.0, "nothing reaches 0.999999 after smoothing... or the threshold is above all scores");
        assert_eq!(rep.rows[1].false_accepts_per_hour, 0.0);
        assert!(rep.table().starts_with("SYNTHETIC DATA ONLY"));
        assert_eq!(rep.pick_threshold(0.0), Some(0.999999));
    }
}
