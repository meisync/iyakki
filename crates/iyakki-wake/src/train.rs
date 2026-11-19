//! Training: Adam on cross-entropy over freshly augmented examples, repeatable with a seed (the result does not depend on how many
//! threads are used: examples are made in order and gradients are added in a fixed order). Runs on the dev PC, never on the device.

use crate::dataset::{make_example, pick_source, DataSet, Source};
use crate::features::{LogMel, N_MEL, WINDOW_FRAMES};
use crate::model::{cross_entropy, softmax2, Net, Workspace};
use crate::rng::Rng;

#[derive(Clone, Debug)]
pub struct Config {
    pub epochs: usize,
    pub examples_per_epoch: usize,
    pub batch: usize,
    pub lr: f32,
    pub seed: u64,
    pub threads: usize,
    pub val_examples: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { epochs: 20, examples_per_epoch: 4000, batch: 32, lr: 3e-3, seed: 1, threads: 4, val_examples: 600 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct EpochReport {
    pub epoch: usize,
    pub train_loss: f32,
    pub val_loss: f32,
    pub val_accuracy: f32,
    pub val_recall_at_half: f32,
    pub val_false_rate_at_half: f32,
}

struct Adam {
    m: Vec<f32>,
    v: Vec<f32>,
    t: i32,
}

impl Adam {
    fn step(&mut self, p: &mut [f32], g: &[f32], lr: f32) {
        self.t += 1;
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        let c1 = 1.0 - b1.powi(self.t);
        let c2 = 1.0 - b2.powi(self.t);
        for i in 0..p.len() {
            self.m[i] = b1 * self.m[i] + (1.0 - b1) * g[i];
            self.v[i] = b2 * self.v[i] + (1.0 - b2) * g[i] * g[i];
            p[i] -= lr * (self.m[i] / c1) / ((self.v[i] / c2).sqrt() + eps);
        }
    }
}

type Example = (Vec<f32>, usize);

/// Loss, gradient sum over a group of examples (a fixed partition keeps the sum order the same for any thread count).
fn group_grad(net: &Net, group: &[Example]) -> (Vec<f32>, f32) {
    let mut grads = vec![0.0f32; net.params.len()];
    let mut ws = Workspace::default();
    let mut loss = 0.0;
    for (x, y) in group {
        let l = net.forward(x, &mut ws);
        let (ls, g) = cross_entropy(l, *y);
        loss += ls;
        net.backward(&ws, g, &mut grads);
    }
    (grads, loss)
}

pub fn evaluate(net: &Net, ex: &[Example]) -> EpochReport {
    let mut ws = Workspace::default();
    let (mut loss, mut ok, mut pos, mut hit, mut neg, mut fa) = (0.0, 0, 0, 0, 0, 0);
    for (x, y) in ex {
        let l = net.forward(x, &mut ws);
        loss += cross_entropy(l, *y).0;
        let p = softmax2(l)[1];
        if (p >= 0.5) == (*y == 1) {
            ok += 1;
        }
        if *y == 1 {
            pos += 1;
            hit += (p >= 0.5) as usize;
        } else {
            neg += 1;
            fa += (p >= 0.5) as usize;
        }
    }
    let n = ex.len().max(1) as f32;
    EpochReport { epoch: 0, train_loss: 0.0, val_loss: loss / n, val_accuracy: ok as f32 / n, val_recall_at_half: hit as f32 / pos.max(1) as f32, val_false_rate_at_half: fa as f32 / neg.max(1) as f32 }
}

pub fn make_validation(ds: &DataSet, n: usize, seed: u64) -> Vec<Example> {
    let lm = LogMel::new();
    let mut rng = Rng::new(seed ^ 0xBAD5EED);
    (0..n)
        .map(|i| {
            let src = match i % 4 {
                0 | 1 => Source::Phrase,
                2 => if ds.negatives.is_empty() { Source::Background } else { Source::Negative },
                _ => if i % 8 == 3 { Source::Fragment } else { Source::Background },
            };
            make_example(ds, &lm, src, &mut rng)
        })
        .collect()
}

/// Train a new network. `progress` is called after every epoch.
pub fn train(train_ds: &DataSet, val_ds: &DataSet, cfg: &Config, progress: &mut dyn FnMut(&EpochReport)) -> (Net, Vec<EpochReport>) {
    let lm = LogMel::new();
    let mut net = Net::random(cfg.seed);
    let mut adam = Adam { m: vec![0.0; net.params.len()], v: vec![0.0; net.params.len()], t: 0 };
    let mut rng = Rng::new(cfg.seed.wrapping_mul(31).wrapping_add(7));
    let val = make_validation(val_ds, cfg.val_examples, cfg.seed);
    let mut reports = Vec::new();
    let steps = (cfg.examples_per_epoch / cfg.batch).max(1);
    let total_steps = (steps * cfg.epochs).max(1);
    let mut step_no = 0usize;
    for epoch in 1..=cfg.epochs {
        let mut epoch_loss = 0.0;
        for _ in 0..steps {
            let batch: Vec<Example> = (0..cfg.batch).map(|_| make_example(train_ds, &lm, pick_source(train_ds, &mut rng), &mut rng)).collect();
            let groups: Vec<&[Example]> = batch.chunks(4).collect();
            let results: Vec<(Vec<f32>, f32)> = if cfg.threads <= 1 {
                groups.iter().map(|g| group_grad(&net, g)).collect()
            } else {
                let net_ref = &net;
                std::thread::scope(|s| {
                    let per = groups.len().div_ceil(cfg.threads);
                    let handles: Vec<_> = groups.chunks(per).map(|part| s.spawn(move || part.iter().map(|g| group_grad(net_ref, g)).collect::<Vec<_>>())).collect();
                    handles.into_iter().flat_map(|h| h.join().expect("training thread")).collect()
                })
            };
            let mut grads = vec![0.0f32; net.params.len()];
            for (g, l) in &results {
                grads.iter_mut().zip(g).for_each(|(a, b)| *a += b);
                epoch_loss += l;
            }
            let inv = 1.0 / cfg.batch as f32;
            grads.iter_mut().for_each(|g| *g *= inv);
            let norm = grads.iter().map(|g| g * g).sum::<f32>().sqrt();
            if norm > 5.0 {
                grads.iter_mut().for_each(|g| *g *= 5.0 / norm);
            }
            let cos = 0.5 * (1.0 + (std::f32::consts::PI * step_no as f32 / total_steps as f32).cos());
            let warm = ((step_no + 1) as f32 / 20.0).min(1.0);
            adam.step(&mut net.params, &grads, cfg.lr * (0.05 + 0.95 * cos) * warm);
            step_no += 1;
        }
        let mut rep = evaluate(&net, &val);
        rep.epoch = epoch;
        rep.train_loss = epoch_loss / (steps * cfg.batch) as f32;
        progress(&rep);
        reports.push(rep);
    }
    (net, reports)
}

/// A starting threshold from the validation windows: the lowest one (0.6 to 0.98) that lets at most 0.5 % of the not-phrase windows
/// through. The streaming sweep in `eval` is the better judge; this only seeds the weights header.
pub fn suggest_threshold(net: &Net, ex: &[Example]) -> f32 {
    let mut ws = Workspace::default();
    let neg: Vec<f32> = ex.iter().filter(|(_, y)| *y == 0).map(|(x, _)| net.score(x, &mut ws)).collect();
    let mut t = 0.6f32;
    while t < 0.98 {
        if neg.iter().filter(|p| **p >= t).count() as f32 <= 0.005 * neg.len() as f32 {
            break;
        }
        t += 0.02;
    }
    t
}

/// The input size the model expects, for the tools that print it.
pub fn input_shape() -> (usize, usize) {
    (WINDOW_FRAMES, N_MEL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Clip;

    /// Two clearly different synthetic sounds: the "phrase" is a rising chirp, the negatives are steady tones.
    fn easy() -> DataSet {
        let chirp = |len: usize, f0: f32| -> Vec<f32> { (0..len).map(|i| 0.4 * (f0 * i as f32 + 1.5e-5 * (i * i) as f32).sin()).collect() };
        let tone = |len: usize, f: f32| -> Vec<f32> { (0..len).map(|i| 0.4 * (f * i as f32).sin()).collect() };
        let c = |n: &str, s: Vec<f32>| Clip { name: n.into(), samples: s, real: false };
        DataSet {
            positives: (0..4).map(|i| c(&format!("p{i}"), chirp(12_000 + i * 500, 0.1 + 0.01 * i as f32))).collect(),
            negatives: (0..4).map(|i| c(&format!("n{i}"), tone(12_000 + i * 500, 0.3 + 0.1 * i as f32))).collect(),
            backgrounds: vec![],
        }
    }

    #[test]
    fn training_learns_an_easy_split_and_is_repeatable_for_any_thread_count() {
        let ds = easy();
        let cfg = Config { epochs: 3, examples_per_epoch: 64, batch: 16, lr: 5e-3, seed: 3, threads: 1, val_examples: 32 };
        let (a, ra) = train(&ds, &ds, &cfg, &mut |_| {});
        let (b, _) = train(&ds, &ds, &Config { threads: 3, ..cfg.clone() }, &mut |_| {});
        assert_eq!(a.params, b.params, "same seed, same weights, whatever the threads");
        assert!(ra[2].train_loss < ra[0].train_loss, "loss falls: {:?}", ra.iter().map(|r| r.train_loss).collect::<Vec<_>>());
        let (c, _) = train(&ds, &ds, &Config { seed: 4, threads: 1, ..cfg }, &mut |_| {});
        assert_ne!(a.params, c.params);
    }

    #[test]
    fn adam_moves_toward_a_minimum() {
        let mut a = Adam { m: vec![0.0; 1], v: vec![0.0; 1], t: 0 };
        let mut p = [5.0f32];
        for _ in 0..500 {
            let g = [2.0 * p[0]];
            a.step(&mut p, &g, 0.1);
        }
        assert!(p[0].abs() < 0.2, "{}", p[0]);
    }
}
