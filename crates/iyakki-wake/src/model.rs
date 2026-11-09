//! The wake model: a small depthwise-separable CNN over about 1.5 s of normalised log-mel frames, f32, pure Rust, with its backward pass
//! (used only by training). About 16 000 weights (65 KB).
//!
//! ```text
//! input [148 frames][40 mel]  ->  channels 40, time 148
//! conv 40->32 k5 stride 2, ReLU                      (74)
//! depthwise k5 + pointwise 32->48, ReLU              (74)
//! depthwise k5 stride 2 + pointwise 48->64, ReLU     (37)
//! depthwise k5 + pointwise 64->64, ReLU              (37)
//! global average and global max over time -> 128
//! dense 128 -> 2 (not a phrase, the phrase)
//! ```

use crate::features::{N_MEL, WINDOW_FRAMES};
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Full 1-D convolution over time: weights `[out][in][k]`.
    Conv { cin: usize, cout: usize, k: usize, stride: usize },
    /// One filter per channel: weights `[c][k]`.
    Depth { c: usize, k: usize, stride: usize },
    Relu,
    /// Global average then global max over time: `c` channels in, `2c` out.
    Pool { c: usize },
    Dense { cin: usize, cout: usize },
}

#[derive(Clone, Debug)]
struct Layer {
    kind: Kind,
    w_off: usize,
    b_off: usize,
    n_w: usize,
    n_b: usize,
    t_in: usize,
    t_out: usize,
}

fn out_len(t: usize, k: usize, stride: usize) -> usize {
    (t + 2 * (k / 2) - k) / stride + 1
}

fn spec() -> Vec<Kind> {
    use Kind::*;
    vec![
        Conv { cin: N_MEL, cout: 32, k: 5, stride: 2 },
        Relu,
        Depth { c: 32, k: 5, stride: 1 },
        Conv { cin: 32, cout: 48, k: 1, stride: 1 },
        Relu,
        Depth { c: 48, k: 5, stride: 2 },
        Conv { cin: 48, cout: 64, k: 1, stride: 1 },
        Relu,
        Depth { c: 64, k: 5, stride: 1 },
        Conv { cin: 64, cout: 64, k: 1, stride: 1 },
        Relu,
        Pool { c: 64 },
        Dense { cin: 128, cout: 2 },
    ]
}

/// Buffers reused between calls (activations of every layer).
#[derive(Default)]
pub struct Workspace {
    acts: Vec<Vec<f32>>,
}

#[derive(Clone, Debug)]
pub struct Net {
    pub params: Vec<f32>,
    layers: Vec<Layer>,
}

impl Net {
    fn layout() -> (Vec<Layer>, usize) {
        let mut layers = Vec::new();
        let mut off = 0;
        let mut t = WINDOW_FRAMES;
        for kind in spec() {
            let (n_w, n_b, t_out) = match kind {
                Kind::Conv { cin, cout, k, stride } => (cout * cin * k, cout, out_len(t, k, stride)),
                Kind::Depth { c, k, stride } => (c * k, c, out_len(t, k, stride)),
                Kind::Relu => (0, 0, t),
                Kind::Pool { .. } => (0, 0, 1),
                Kind::Dense { cin, cout } => (cin * cout, cout, 1),
            };
            layers.push(Layer { kind, w_off: off, b_off: off + n_w, n_w, n_b, t_in: t, t_out });
            off += n_w + n_b;
            t = t_out;
        }
        (layers, off)
    }

    /// The number of weights (and biases).
    pub fn param_count() -> usize {
        Self::layout().1
    }

    /// Zeros (a valid but useless network).
    pub fn zeros() -> Net {
        let (layers, n) = Self::layout();
        Net { params: vec![0.0; n], layers }
    }

    /// He-initialised weights from a seed.
    pub fn random(seed: u64) -> Net {
        let mut net = Net::zeros();
        let mut rng = Rng::new(seed);
        for l in &net.layers {
            let fan_in = match l.kind {
                Kind::Conv { cin, k, .. } => cin * k,
                Kind::Depth { k, .. } => k,
                Kind::Dense { cin, .. } => cin,
                _ => continue,
            };
            let std = (2.0 / fan_in as f32).sqrt() * if matches!(l.kind, Kind::Dense { .. }) { 0.5 } else { 1.0 };
            for w in &mut net.params[l.w_off..l.w_off + l.n_w] {
                *w = rng.normal() * std;
            }
        }
        net
    }

    pub fn from_params(params: Vec<f32>) -> Result<Net, String> {
        let mut net = Net::zeros();
        if params.len() != net.params.len() {
            return Err(format!("the weights have {} numbers, this model needs {}", params.len(), net.params.len()));
        }
        if params.iter().any(|v| !v.is_finite()) {
            return Err("the weights contain a number that is not finite".into());
        }
        net.params = params;
        Ok(net)
    }

    /// Logits `[not the phrase, the phrase]` for one normalised window `[frame][mel]` (`WINDOW_FRAMES * N_MEL` numbers).
    pub fn forward(&self, x: &[f32], ws: &mut Workspace) -> [f32; 2] {
        assert_eq!(x.len(), WINDOW_FRAMES * N_MEL, "a window is {WINDOW_FRAMES} frames of {N_MEL} mel");
        ws.acts.resize(self.layers.len() + 1, Vec::new());
        // frame-major to channel-major
        let a0 = &mut ws.acts[0];
        a0.clear();
        a0.resize(x.len(), 0.0);
        for t in 0..WINDOW_FRAMES {
            for m in 0..N_MEL {
                a0[m * WINDOW_FRAMES + t] = x[t * N_MEL + m];
            }
        }
        for (i, l) in self.layers.iter().enumerate() {
            let (before, after) = ws.acts.split_at_mut(i + 1);
            let (inp, out) = (&before[i], &mut after[0]);
            self.layer_forward(l, inp, out);
        }
        let o = &ws.acts[self.layers.len()];
        [o[0], o[1]]
    }

    fn layer_forward(&self, l: &Layer, inp: &[f32], out: &mut Vec<f32>) {
        let w = &self.params[l.w_off..l.w_off + l.n_w];
        let b = &self.params[l.b_off..l.b_off + l.n_b];
        match l.kind {
            Kind::Conv { cin, cout, k, stride } => {
                out.clear();
                out.resize(cout * l.t_out, 0.0);
                let pad = k / 2;
                for o in 0..cout {
                    let y = &mut out[o * l.t_out..(o + 1) * l.t_out];
                    y.iter_mut().for_each(|v| *v = b[o]);
                    for i in 0..cin {
                        let x = &inp[i * l.t_in..(i + 1) * l.t_in];
                        for kk in 0..k {
                            let wv = w[(o * cin + i) * k + kk];
                            for (t, yv) in y.iter_mut().enumerate() {
                                let idx = (t * stride + kk) as isize - pad as isize;
                                if idx >= 0 && (idx as usize) < l.t_in {
                                    *yv += wv * x[idx as usize];
                                }
                            }
                        }
                    }
                }
            }
            Kind::Depth { c, k, stride } => {
                out.clear();
                out.resize(c * l.t_out, 0.0);
                let pad = k / 2;
                for ch in 0..c {
                    let x = &inp[ch * l.t_in..(ch + 1) * l.t_in];
                    let y = &mut out[ch * l.t_out..(ch + 1) * l.t_out];
                    for (t, yv) in y.iter_mut().enumerate() {
                        let mut s = b[ch];
                        for kk in 0..k {
                            let idx = (t * stride + kk) as isize - pad as isize;
                            if idx >= 0 && (idx as usize) < l.t_in {
                                s += w[ch * k + kk] * x[idx as usize];
                            }
                        }
                        *yv = s;
                    }
                }
            }
            Kind::Relu => {
                out.clear();
                out.extend(inp.iter().map(|v| v.max(0.0)));
            }
            Kind::Pool { c } => {
                out.clear();
                out.resize(2 * c, 0.0);
                for ch in 0..c {
                    let x = &inp[ch * l.t_in..(ch + 1) * l.t_in];
                    out[ch] = x.iter().sum::<f32>() / l.t_in as f32;
                    out[c + ch] = x.iter().cloned().fold(f32::MIN, f32::max);
                }
            }
            Kind::Dense { cin, cout } => {
                out.clear();
                out.resize(cout, 0.0);
                for o in 0..cout {
                    out[o] = b[o] + (0..cin).map(|i| w[o * cin + i] * inp[i]).sum::<f32>();
                }
            }
        }
    }

    /// Probability of "the phrase" for a normalised window.
    pub fn score(&self, x: &[f32], ws: &mut Workspace) -> f32 {
        let l = self.forward(x, ws);
        softmax2(l)[1]
    }

    /// Backward pass after `forward` on the same workspace: adds the gradient of the loss with respect to every weight into `grads`,
    /// given the gradient with respect to the two logits.
    pub fn backward(&self, ws: &Workspace, dlogits: [f32; 2], grads: &mut [f32]) {
        assert_eq!(grads.len(), self.params.len());
        let mut dy: Vec<f32> = dlogits.to_vec();
        for (i, l) in self.layers.iter().enumerate().rev() {
            let inp = &ws.acts[i];
            let w = &self.params[l.w_off..l.w_off + l.n_w];
            let mut dx = vec![0.0f32; inp.len()];
            match l.kind {
                Kind::Conv { cin, cout, k, stride } => {
                    let pad = k / 2;
                    for o in 0..cout {
                        let g = &dy[o * l.t_out..(o + 1) * l.t_out];
                        grads[l.b_off + o] += g.iter().sum::<f32>();
                        for ci in 0..cin {
                            let x = &inp[ci * l.t_in..(ci + 1) * l.t_in];
                            for kk in 0..k {
                                let wi = (o * cin + ci) * k + kk;
                                let mut gw = 0.0;
                                for (t, gv) in g.iter().enumerate() {
                                    let idx = (t * stride + kk) as isize - pad as isize;
                                    if idx >= 0 && (idx as usize) < l.t_in {
                                        gw += gv * x[idx as usize];
                                        dx[ci * l.t_in + idx as usize] += gv * w[wi];
                                    }
                                }
                                grads[l.w_off + wi] += gw;
                            }
                        }
                    }
                }
                Kind::Depth { c, k, stride } => {
                    let pad = k / 2;
                    for ch in 0..c {
                        let g = &dy[ch * l.t_out..(ch + 1) * l.t_out];
                        grads[l.b_off + ch] += g.iter().sum::<f32>();
                        for kk in 0..k {
                            let mut gw = 0.0;
                            for (t, gv) in g.iter().enumerate() {
                                let idx = (t * stride + kk) as isize - pad as isize;
                                if idx >= 0 && (idx as usize) < l.t_in {
                                    gw += gv * inp[ch * l.t_in + idx as usize];
                                    dx[ch * l.t_in + idx as usize] += gv * w[ch * k + kk];
                                }
                            }
                            grads[l.w_off + ch * k + kk] += gw;
                        }
                    }
                }
                Kind::Relu => {
                    for (j, v) in dx.iter_mut().enumerate() {
                        *v = if inp[j] > 0.0 { dy[j] } else { 0.0 };
                    }
                }
                Kind::Pool { c } => {
                    for ch in 0..c {
                        let x = &inp[ch * l.t_in..(ch + 1) * l.t_in];
                        let avg_g = dy[ch] / l.t_in as f32;
                        let mut arg = 0;
                        for (t, v) in x.iter().enumerate() {
                            dx[ch * l.t_in + t] += avg_g;
                            if *v > x[arg] {
                                arg = t;
                            }
                        }
                        dx[ch * l.t_in + arg] += dy[c + ch];
                    }
                }
                Kind::Dense { cin, cout } => {
                    for o in 0..cout {
                        grads[l.b_off + o] += dy[o];
                        for ii in 0..cin {
                            grads[l.w_off + o * cin + ii] += dy[o] * inp[ii];
                            dx[ii] += dy[o] * w[o * cin + ii];
                        }
                    }
                }
            }
            dy = dx;
        }
    }
}

pub fn softmax2(l: [f32; 2]) -> [f32; 2] {
    let m = l[0].max(l[1]);
    let (a, b) = ((l[0] - m).exp(), (l[1] - m).exp());
    [a / (a + b), b / (a + b)]
}

/// Cross-entropy of the logits for `label` (0 or 1) and the gradient with respect to the logits.
pub fn cross_entropy(l: [f32; 2], label: usize) -> (f32, [f32; 2]) {
    let p = softmax2(l);
    let loss = -(p[label].max(1e-12)).ln();
    let mut g = p;
    g[label] -= 1.0;
    (loss, g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(seed: u64) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..WINDOW_FRAMES * N_MEL).map(|_| r.normal()).collect()
    }

    #[test]
    fn the_model_is_small_and_deterministic() {
        let n = Net::param_count();
        assert!(n * 4 < 100 * 1024, "{} KB", n * 4 / 1024);
        assert!(n > 5000);
        let (a, b) = (Net::random(3), Net::random(3));
        assert_eq!(a.params, b.params);
        let x = input(1);
        let (mut w1, mut w2) = (Workspace::default(), Workspace::default());
        assert_eq!(a.forward(&x, &mut w1), b.forward(&x, &mut w2));
        assert_ne!(Net::random(4).params, a.params);
    }

    #[test]
    fn a_zero_network_is_undecided_and_bad_weights_are_refused() {
        let z = Net::zeros();
        let p = z.score(&input(2), &mut Workspace::default());
        assert!((p - 0.5).abs() < 1e-6);
        assert!(Net::from_params(vec![0.0; 10]).is_err());
        let mut v = vec![0.0; Net::param_count()];
        v[3] = f32::NAN;
        assert!(Net::from_params(v).is_err());
    }

    #[test]
    fn the_hand_computed_conv_and_dense_values() {
        // a dense layer alone: logits = W x + b
        let (layers, _) = Net::layout();
        let d = layers.last().unwrap();
        let mut net = Net::zeros();
        net.params[d.w_off + 5] = 2.0; // output 0 , input 5
        net.params[d.w_off + 128 + 6] = -3.0; // output 1, input 6
        net.params[d.b_off] = 0.25;
        let mut out = Vec::new();
        let mut inp = vec![0.0; 128];
        inp[5] = 1.5;
        inp[6] = 2.0;
        net.layer_forward(d, &inp, &mut out);
        assert!((out[0] - 3.25).abs() < 1e-6 && (out[1] + 6.0).abs() < 1e-6);
        // a stride-2 depthwise filter [0,0,1,0,0] with pad 2 copies every second sample
        let dw = layers.iter().find(|l| matches!(l.kind, Kind::Depth { stride: 2, .. })).unwrap();
        net.params[dw.w_off + 2] = 1.0; // channel 0 centre tap
        let mut x = vec![0.0f32; 48 * dw.t_in];
        for t in 0..dw.t_in {
            x[t] = t as f32;
        }
        net.layer_forward(dw, &x, &mut out);
        assert_eq!(dw.t_out, 37);
        for t in 0..dw.t_out {
            assert!((out[t] - (2 * t) as f32).abs() < 1e-6, "t {t}");
        }
    }

    #[test]
    fn the_backward_pass_matches_numerical_gradients() {
        let net = Net::random(11);
        let x = input(5);
        let label = 1;
        let mut ws = Workspace::default();
        let l = net.forward(&x, &mut ws);
        let (_, g) = cross_entropy(l, label);
        let mut grads = vec![0.0; net.params.len()];
        net.backward(&ws, g, &mut grads);
        let loss_at = |n: &Net| {
            let l = n.forward(&x, &mut Workspace::default());
            cross_entropy(l, label).0
        };
        let mut r = Rng::new(9);
        let mut checked = 0;
        for _ in 0..80 {
            let i = r.below(net.params.len());
            let eps = 1e-2f32;
            let (mut a, mut b) = (net.clone(), net.clone());
            a.params[i] += eps;
            b.params[i] -= eps;
            let num = (loss_at(&a) - loss_at(&b)) / (2.0 * eps);
            let tol = 2e-2 * num.abs().max(grads[i].abs()).max(0.05);
            assert!((num - grads[i]).abs() <= tol, "param {i}: numeric {num} analytic {}", grads[i]);
            checked += 1;
        }
        assert_eq!(checked, 80);
    }
}
