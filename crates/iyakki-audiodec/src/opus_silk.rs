//! The SILK decoder of Opus (RFC 6716 section 4.2), written in-house: the speech layer of an Opus stream, in 8, 12 or 16 kHz internal
//! rate, mono or mid/side stereo, with frames of 10 or 20 ms (a packet carries up to three of them).
//!
//! A frame is read as: the side information (signal type, gains, the line spectral frequencies in two stages with an interpolation
//! factor, the pitch lag and contour, the long-term prediction filter), then the excitation pulses (shell coded in blocks of 16, with
//! extra low bits and signs). The sound is made by running the excitation through the long-term (pitch) predictor and the short-term
//! (LPC) synthesis filter, then resampled to the output rate. All of this is fixed-point integer arithmetic, written here with the same
//! rounding as the standard's decoder, because the stage after it (the line spectral frequency to filter conversion, the gain steps)
//! must give the same numbers on every machine. Lost frames are concealed by repeating the pitch cycle with fading noise, and the
//! first good frame after a loss is faded in (section 4.2.8).
//!
//! The constant tables are in `opus_silk_tables.rs` (data from the reference decoder, BSD licence, see legal/); the rest is this
//! project's own code.

use crate::opus::RangeDecoder;
use crate::opus_silk_tables as t;

const MAX_LPC: usize = 16;
const LTP_ORDER: usize = 5;
const MAX_FRAME: usize = 320;
const MAX_SUBFR: usize = 80;
const MAX_NB_SUBFR: usize = 4;
const N_LEVELS_QGAIN: i32 = 64;
const MIN_DELTA_GAIN: i32 = -4;
const MAX_DELTA_GAIN: i32 = 36;
const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;
const TYPE_NO_VOICE: i32 = 0;
const TYPE_VOICED: i32 = 2;
const CODE_INDEPENDENTLY: i32 = 0;
const CODE_INDEPENDENTLY_NO_LTP_SCALING: i32 = 1;
const CODE_CONDITIONALLY: i32 = 2;
const BWE_AFTER_LOSS_Q16: i32 = 63570;
const OUT_BUF: usize = MAX_FRAME + 2 * MAX_SUBFR;

// ---- fixed-point helpers (the standard's macros, with wrapping arithmetic so that damaged streams cannot panic)

fn smulwb(a: i32, b: i32) -> i32 {
    ((a as i64 * (b as i16 as i64)) >> 16) as i32
}
fn smlawb(acc: i32, a: i32, b: i32) -> i32 {
    acc.wrapping_add(smulwb(a, b))
}
fn smulbb(a: i32, b: i32) -> i32 {
    (a as i16 as i32).wrapping_mul(b as i16 as i32)
}
fn rshift_round(a: i32, s: u32) -> i32 {
    if s == 1 {
        (a >> 1).wrapping_add(a & 1)
    } else {
        (a >> (s - 1)).wrapping_add(1) >> 1
    }
}
fn rshift_round64(a: i64, s: u32) -> i64 {
    if s == 1 {
        (a >> 1) + (a & 1)
    } else {
        ((a >> (s - 1)) + 1) >> 1
    }
}
fn smulww(a: i32, b: i32) -> i32 {
    smulwb(a, b).wrapping_add(a.wrapping_mul(rshift_round(b, 16)))
}
fn smlaww(acc: i32, a: i32, b: i32) -> i32 {
    acc.wrapping_add(smulww(a, b))
}
fn smmul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 32) as i32
}
fn sat16(a: i32) -> i32 {
    a.clamp(-32768, 32767)
}
fn clz32(x: i32) -> i32 {
    (x as u32).leading_zeros() as i32
}
fn shl(a: i32, s: i32) -> i32 {
    a.wrapping_shl(s as u32)
}
fn lshift_sat32(a: i32, s: i32) -> i32 {
    shl(a.clamp(i32::MIN >> s, i32::MAX >> s), s)
}
fn rand(seed: i32) -> i32 {
    907_633_515i32.wrapping_add(seed.wrapping_mul(196_314_165))
}
fn div32(a: i32, b: i32) -> i32 {
    if b == 0 {
        0
    } else {
        a.wrapping_div(b)
    }
}

fn clz_frac(x: i32) -> (i32, i32) {
    let lz = clz32(x);
    let frac = (x as u32).rotate_right(((24 - lz) & 31) as u32) & 0x7f;
    (lz, frac as i32)
}

fn sqrt_approx(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let (lz, frac) = clz_frac(x);
    let mut y: i32 = if lz & 1 != 0 { 32768 } else { 46214 };
    y >>= lz >> 1;
    smlawb(y, y, smulbb(213, frac))
}

fn div32_varq(a32: i32, b32: i32, qres: i32) -> i32 {
    if b32 == 0 {
        return 0;
    }
    let a_head = clz32(a32.wrapping_abs()) - 1;
    let mut a_nrm = shl(a32, a_head);
    let b_head = clz32(b32.wrapping_abs()) - 1;
    let b_nrm = shl(b32, b_head);
    let b_inv = div32(i32::MAX >> 2, b_nrm >> 16);
    let mut result = smulwb(a_nrm, b_inv);
    a_nrm = a_nrm.wrapping_sub(shl(smmul(b_nrm, result), 3));
    result = smlawb(result, a_nrm, b_inv);
    let lshift = 29 + a_head - b_head - qres;
    if lshift < 0 {
        lshift_sat32(result, -lshift)
    } else if lshift < 32 {
        result >> lshift
    } else {
        0
    }
}

fn inverse32_varq(b32: i32, qres: i32) -> i32 {
    if b32 == 0 {
        return i32::MAX;
    }
    let b_head = clz32(b32.wrapping_abs()) - 1;
    let b_nrm = shl(b32, b_head);
    let b_inv = div32(i32::MAX >> 2, b_nrm >> 16);
    let mut result = shl(b_inv, 16);
    let err_q32 = shl((1 << 29) - smulwb(b_nrm, b_inv), 3);
    result = smlaww(result, err_q32, b_inv);
    let lshift = 61 - b_head - qres;
    if lshift <= 0 {
        lshift_sat32(result, -lshift)
    } else if lshift < 32 {
        result >> lshift
    } else {
        0
    }
}

fn log2lin(x: i32) -> i32 {
    if x < 0 {
        return 0;
    }
    let mut out = shl(1, x >> 7);
    let frac = x & 0x7f;
    let k = smlawb(frac, smulbb(frac, 128 - frac), -174);
    if x < 2048 {
        out = out.wrapping_add(out.wrapping_mul(k) >> 7);
    } else {
        out = out.wrapping_add((out >> 7).wrapping_mul(k));
    }
    out
}

fn bwexpander(ar: &mut [i16], d: usize, mut chirp_q16: i32) {
    let minus_one = chirp_q16 - 65536;
    for a in ar.iter_mut().take(d.saturating_sub(1)) {
        *a = rshift_round(chirp_q16.wrapping_mul(*a as i32), 16) as i16;
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(chirp_q16.wrapping_mul(minus_one), 16));
    }
    if d > 0 {
        ar[d - 1] = rshift_round(chirp_q16.wrapping_mul(ar[d - 1] as i32), 16) as i16;
    }
}

fn bwexpander_32(ar: &mut [i32], d: usize, mut chirp_q16: i32) {
    let minus_one = chirp_q16 - 65536;
    for a in ar.iter_mut().take(d.saturating_sub(1)) {
        *a = smulww(chirp_q16, *a);
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(chirp_q16.wrapping_mul(minus_one), 16));
    }
    if d > 0 {
        ar[d - 1] = smulww(chirp_q16, ar[d - 1]);
    }
}

/// The inverse of the prediction gain of an LPC filter, Q30; 0 if the filter is unstable.
fn lpc_inverse_pred_gain(a_q12: &[i16], order: usize) -> i32 {
    const QA: u32 = 24;
    const A_LIMIT: i32 = 16_773_022; // 0.99975 in Q24
    let mut a = [[0i32; MAX_LPC]; 2];
    let mut dc = 0i32;
    for k in 0..order {
        dc += a_q12[k] as i32;
        a[order & 1][k] = (a_q12[k] as i32) << (QA - 12);
    }
    if dc >= 4096 {
        return 0;
    }
    let mut cur = order & 1;
    let mut inv_gain: i32 = 1 << 30;
    for k in (1..order).rev() {
        if a[cur][k] > A_LIMIT || a[cur][k] < -A_LIMIT {
            return 0;
        }
        let rc_q31 = -shl(a[cur][k], 31 - QA as i32);
        let rc_mult1 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
        let mult2q = 32 - clz32(rc_mult1.wrapping_abs());
        let rc_mult2 = inverse32_varq(rc_mult1, mult2q + 30);
        inv_gain = shl(smmul(inv_gain, rc_mult1), 2);
        let next = k & 1;
        for n in 0..k {
            let tmp = a[cur][n].wrapping_sub(rshift_round64(a[cur][k - n - 1] as i64 * rc_q31 as i64, 31) as i32);
            a[next][n] = rshift_round64(tmp as i64 * rc_mult2 as i64, mult2q as u32) as i32;
        }
        cur = next;
    }
    if a[cur][0] > A_LIMIT || a[cur][0] < -A_LIMIT {
        return 0;
    }
    let rc_q31 = -shl(a[cur][0], 31 - QA as i32);
    let rc_mult1 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
    shl(smmul(inv_gain, rc_mult1), 2)
}

/// Short-term prediction error filter (whitening): `out[i] = in[i] - sum B[j] in[i-1-j]` for `i >= d`, zero before.
fn lpc_analysis_filter(out: &mut [i16], inp: &[i16], b: &[i16], len: usize, d: usize) {
    for ix in d..len {
        let mut acc = 0i32;
        for j in 0..d {
            acc = acc.wrapping_add(smulbb(inp[ix - 1 - j] as i32, b[j] as i32));
        }
        let o = ((inp[ix] as i32) << 12).wrapping_sub(acc);
        out[ix] = sat16(rshift_round(o, 12)) as i16;
    }
    for o in out.iter_mut().take(d) {
        *o = 0;
    }
}

fn sum_sqr_shift(x: &[i16]) -> (i32, i32) {
    let len = x.len() as i32 - 1;
    let (mut nrg, mut shft) = (0i32, 0i32);
    let mut i = 0i32;
    while i < len {
        nrg = nrg.wrapping_add(smulbb(x[i as usize] as i32, x[i as usize] as i32)).wrapping_add(smulbb(x[i as usize + 1] as i32, x[i as usize + 1] as i32));
        if nrg < 0 {
            nrg = ((nrg as u32) >> 2) as i32;
            shft = 2;
            break;
        }
        i += 2;
    }
    while i < len {
        let tmp = smulbb(x[i as usize] as i32, x[i as usize] as i32).wrapping_add(smulbb(x[i as usize + 1] as i32, x[i as usize + 1] as i32));
        nrg = (nrg as u32).wrapping_add((tmp as u32) >> shft) as i32;
        if nrg < 0 {
            nrg = ((nrg as u32) >> 2) as i32;
            shft += 2;
        }
        i += 2;
    }
    if i == len {
        let tmp = smulbb(x[i as usize] as i32, x[i as usize] as i32);
        nrg = (nrg as u32).wrapping_add((tmp as u32) >> shft) as i32;
    }
    if nrg & 0xC000_0000u32 as i32 != 0 {
        nrg = ((nrg as u32) >> 2) as i32;
        shft += 2;
    }
    (nrg, shft)
}

// ---- line spectral frequencies

struct NlsfCb {
    order: usize,
    step_q16: i32,
    cb1_q8: &'static [u8],
    cb1_icdf: &'static [u8],
    pred_q8: &'static [u8],
    ec_sel: &'static [u8],
    cb2_icdf: &'static [u8],
    delta_min: &'static [i16],
}

const CB_NB_MB: NlsfCb = NlsfCb { order: 10, step_q16: 11796, cb1_q8: &t::NLSF_CB1_NB_MB_Q8, cb1_icdf: &t::NLSF_CB1_ICDF_NB_MB, pred_q8: &t::NLSF_PRED_NB_MB_Q8, ec_sel: &t::NLSF_CB2_SELECT_NB_MB, cb2_icdf: &t::NLSF_CB2_ICDF_NB_MB, delta_min: &t::NLSF_DELTA_MIN_NB_MB_Q15 };
const CB_WB: NlsfCb = NlsfCb { order: 16, step_q16: 9830, cb1_q8: &t::NLSF_CB1_WB_Q8, cb1_icdf: &t::NLSF_CB1_ICDF_WB, pred_q8: &t::NLSF_PRED_WB_Q8, ec_sel: &t::NLSF_CB2_SELECT_WB, cb2_icdf: &t::NLSF_CB2_ICDF_WB, delta_min: &t::NLSF_DELTA_MIN_WB_Q15 };

fn nlsf_unpack(cb: &NlsfCb, cb1_index: usize) -> ([i32; MAX_LPC], [i32; MAX_LPC]) {
    let mut ec_ix = [0i32; MAX_LPC];
    let mut pred = [0i32; MAX_LPC];
    let sel = &cb.ec_sel[cb1_index * cb.order / 2..];
    let mut i = 0;
    while i < cb.order {
        let entry = sel[i / 2] as i32;
        ec_ix[i] = ((entry >> 1) & 7) * (2 * NLSF_QUANT_MAX_AMPLITUDE + 1);
        pred[i] = cb.pred_q8[i + (entry as usize & 1) * (cb.order - 1)] as i32;
        ec_ix[i + 1] = ((entry >> 5) & 7) * (2 * NLSF_QUANT_MAX_AMPLITUDE + 1);
        pred[i + 1] = cb.pred_q8[i + ((entry as usize >> 4) & 1) * (cb.order - 1) + 1] as i32;
        i += 2;
    }
    (ec_ix, pred)
}

fn nlsf_stabilize(nlsf: &mut [i16], delta_min: &[i16], l: usize) {
    let mut loops = 0;
    while loops < 20 {
        let mut min_diff = nlsf[0] as i32 - delta_min[0] as i32;
        let mut idx = 0usize;
        for i in 1..l {
            let diff = nlsf[i] as i32 - (nlsf[i - 1] as i32 + delta_min[i] as i32);
            if diff < min_diff {
                min_diff = diff;
                idx = i;
            }
        }
        let diff = (1 << 15) - (nlsf[l - 1] as i32 + delta_min[l] as i32);
        if diff < min_diff {
            min_diff = diff;
            idx = l;
        }
        if min_diff >= 0 {
            return;
        }
        if idx == 0 {
            nlsf[0] = delta_min[0];
        } else if idx == l {
            nlsf[l - 1] = ((1 << 15) - delta_min[l] as i32) as i16;
        } else {
            let mut min_center = 0i32;
            for k in 0..idx {
                min_center += delta_min[k] as i32;
            }
            min_center += (delta_min[idx] as i32) >> 1;
            let mut max_center = 1i32 << 15;
            for k in (idx + 1..=l).rev() {
                max_center -= delta_min[k] as i32;
            }
            max_center -= (delta_min[idx] as i32) >> 1;
            let center = rshift_round(nlsf[idx - 1] as i32 + nlsf[idx] as i32, 1).clamp(min_center, max_center) as i16;
            nlsf[idx - 1] = center - (delta_min[idx] >> 1);
            nlsf[idx] = nlsf[idx - 1] + delta_min[idx];
        }
        loops += 1;
    }
    // too many rounds: sort and force the minimum distances
    nlsf[..l].sort_unstable();
    nlsf[0] = nlsf[0].max(delta_min[0]);
    for i in 1..l {
        nlsf[i] = nlsf[i].max(((nlsf[i - 1] as i32 + delta_min[i] as i32).min(32767)) as i16);
    }
    nlsf[l - 1] = nlsf[l - 1].min(((1 << 15) - delta_min[l] as i32) as i16);
    for i in (0..l - 1).rev() {
        nlsf[i] = nlsf[i].min((nlsf[i + 1] as i32 - delta_min[i + 1] as i32) as i16);
    }
}

fn nlsf_vq_weights_laroia(w: &mut [i32], nlsf: &[i16], d: usize) {
    let inv = |x: i32| div32(1 << 17, x.max(1));
    let mut tmp1 = inv(nlsf[0] as i32);
    let mut tmp2 = inv(nlsf[1] as i32 - nlsf[0] as i32);
    w[0] = (tmp1 + tmp2).min(32767);
    let mut k = 1;
    while k < d - 1 {
        tmp1 = inv(nlsf[k + 1] as i32 - nlsf[k] as i32);
        w[k] = (tmp1 + tmp2).min(32767);
        tmp2 = inv(nlsf[k + 2] as i32 - nlsf[k + 1] as i32);
        w[k + 1] = (tmp1 + tmp2).min(32767);
        k += 2;
    }
    tmp1 = inv((1 << 15) - nlsf[d - 1] as i32);
    w[d - 1] = (tmp1 + tmp2).min(32767);
}

fn nlsf_decode(cb: &NlsfCb, indices: &[i32]) -> [i16; MAX_LPC] {
    let order = cb.order;
    let mut nlsf = [0i16; MAX_LPC];
    let base = &cb.cb1_q8[indices[0] as usize * order..];
    for i in 0..order {
        nlsf[i] = (base[i] as i16) << 7;
    }
    let (_, pred) = nlsf_unpack(cb, indices[0] as usize);
    // residual
    let mut res_q10 = [0i32; MAX_LPC];
    let mut out_q10 = 0i32;
    for i in (0..order).rev() {
        let pred_q10 = smulbb(out_q10, pred[i]) >> 8;
        out_q10 = shl(indices[i + 1], 10);
        if out_q10 > 0 {
            out_q10 -= 102;
        } else if out_q10 < 0 {
            out_q10 += 102;
        }
        out_q10 = smlawb(pred_q10, out_q10, cb.step_q16);
        res_q10[i] = out_q10 as i16 as i32;
    }
    let mut w = [0i32; MAX_LPC];
    nlsf_vq_weights_laroia(&mut w, &nlsf, order);
    for i in 0..order {
        let w_q9 = sqrt_approx(shl(w[i], 16));
        let tmp = (nlsf[i] as i32).wrapping_add(div32(shl(res_q10[i], 14), w_q9));
        nlsf[i] = tmp.clamp(0, 32767) as i16;
    }
    nlsf_stabilize(&mut nlsf, cb.delta_min, order);
    nlsf
}

fn lsf_to_poly(out: &mut [i32], c_lsf: &[i32], stride_off: usize, dd: usize) {
    const QA: u32 = 16;
    out[0] = 1 << QA;
    out[1] = -c_lsf[stride_off];
    for k in 1..dd {
        let ftmp = c_lsf[stride_off + 2 * k];
        out[k + 1] = (out[k - 1] << 1).wrapping_sub(rshift_round64(ftmp as i64 * out[k] as i64, QA) as i32);
        for n in (2..=k).rev() {
            out[n] = out[n].wrapping_add(out[n - 2]).wrapping_sub(rshift_round64(ftmp as i64 * out[n - 1] as i64, QA) as i32);
        }
        out[1] = out[1].wrapping_sub(ftmp);
    }
}

/// The LPC filter (Q12) from the line spectral frequencies (Q15).
fn nlsf2a(a_q12: &mut [i16], nlsf: &[i16], d: usize) {
    const QA: u32 = 16;
    const ORDERING16: [usize; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];
    const ORDERING10: [usize; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
    let ordering: &[usize] = if d == 16 { &ORDERING16 } else { &ORDERING10 };
    let mut cos_lsf = [0i32; MAX_LPC];
    for k in 0..d {
        let f_int = (nlsf[k] as i32) >> 8;
        let f_frac = nlsf[k] as i32 - (f_int << 8);
        let f_int = f_int.clamp(0, 127) as usize;
        let cos_val = t::LSF_COS_TAB_Q12[f_int] as i32;
        let delta = t::LSF_COS_TAB_Q12[f_int + 1] as i32 - cos_val;
        cos_lsf[ordering[k]] = rshift_round((cos_val << 8) + delta * f_frac, 20 - QA);
    }
    let dd = d >> 1;
    let (mut p, mut q) = ([0i32; MAX_LPC / 2 + 1], [0i32; MAX_LPC / 2 + 1]);
    lsf_to_poly(&mut p, &cos_lsf, 0, dd);
    lsf_to_poly(&mut q, &cos_lsf, 1, dd);
    let mut a32 = [0i32; MAX_LPC];
    for k in 0..dd {
        let ptmp = p[k + 1].wrapping_add(p[k]);
        let qtmp = q[k + 1].wrapping_sub(q[k]);
        a32[k] = (-qtmp).wrapping_sub(ptmp);
        a32[d - k - 1] = qtmp.wrapping_sub(ptmp);
    }
    let mut i = 0;
    while i < 10 {
        let (mut maxabs, mut idx) = (0i32, 0usize);
        for (k, &v) in a32.iter().enumerate().take(d) {
            let av = v.wrapping_abs();
            if av > maxabs {
                maxabs = av;
                idx = k;
            }
        }
        maxabs = rshift_round(maxabs, QA + 1 - 12);
        if maxabs > 32767 {
            maxabs = maxabs.min(163_838);
            let sc_q16 = 65470 - div32(shl(maxabs - 32767, 14), maxabs.wrapping_mul(idx as i32 + 1) >> 2);
            bwexpander_32(&mut a32, d, sc_q16);
        } else {
            break;
        }
        i += 1;
    }
    if i == 10 {
        for k in 0..d {
            a_q12[k] = sat16(rshift_round(a32[k], QA + 1 - 12)) as i16;
            a32[k] = shl(a_q12[k] as i32, (QA + 1 - 12) as i32);
        }
    } else {
        for k in 0..d {
            a_q12[k] = rshift_round(a32[k], QA + 1 - 12) as i16;
        }
    }
    for i in 0..16 {
        if lpc_inverse_pred_gain(a_q12, d) < 107_374 {
            bwexpander_32(&mut a32, d, 65536 - (2 << i));
            for k in 0..d {
                a_q12[k] = rshift_round(a32[k], QA + 1 - 12) as i16;
            }
        } else {
            break;
        }
    }
}

// ---- state

#[derive(Clone, Default)]
struct Indices {
    signal_type: i32,
    quant_offset_type: i32,
    gains: [i32; MAX_NB_SUBFR],
    nlsf: [i32; MAX_LPC + 1],
    nlsf_interp_q2: i32,
    lag_index: i32,
    contour_index: i32,
    per_index: i32,
    ltp_index: [i32; MAX_NB_SUBFR],
    ltp_scale_index: i32,
    seed: i32,
}

#[derive(Clone, Default)]
struct Ctrl {
    pitch_l: [i32; MAX_NB_SUBFR],
    gains_q16: [i32; MAX_NB_SUBFR],
    pred_coef_q12: [[i16; MAX_LPC]; 2],
    ltp_coef_q14: [i16; LTP_ORDER * MAX_NB_SUBFR],
    ltp_scale_q14: i32,
}

#[derive(Clone, Default)]
struct Plc {
    pitch_l_q8: i32,
    ltp_coef_q14: [i16; LTP_ORDER],
    prev_lpc_q12: [i16; MAX_LPC],
    last_frame_lost: bool,
    rand_seed: i32,
    rand_scale_q14: i16,
    conc_energy: i32,
    conc_energy_shift: i32,
    prev_ltp_scale_q14: i32,
    prev_gain_q16: [i32; 2],
    fs_khz: i32,
    nb_subfr: usize,
    subfr_length: usize,
}

#[derive(Clone)]
struct Cng {
    exc_buf_q14: [i32; MAX_FRAME],
    smth_nlsf_q15: [i16; MAX_LPC],
    synth_state: [i32; MAX_LPC],
    smth_gain_q16: i32,
    rand_seed: i32,
    fs_khz: i32,
}

/// The resampler from the internal rate to 48 kHz: a 2x all-pass upsampler followed by a 12-phase FIR interpolation, as the standard's.
#[derive(Clone)]
struct Resampler {
    iir: [i32; 6],
    fir: [i16; 8],
    delay_buf: [i16; 48],
    in_khz: usize,
    out_khz: usize,
    batch: usize,
    inv_ratio_q16: i32,
    input_delay: usize,
    two_x: bool,
}

impl Resampler {
    fn new(fs_in: i32, fs_out: i32) -> Resampler {
        let delay = match (fs_in, fs_out) {
            (12000, 48000) => 4,
            (16000, 48000) => 7,
            _ => 0,
        };
        let two_x = fs_out == 2 * fs_in;
        let up2x = if two_x { 0 } else { 1 };
        let mut inv = shl(div32(shl(fs_in, 14 + up2x), fs_out), 2);
        while smulww(inv, fs_out) < shl(fs_in, up2x) {
            inv += 1;
        }
        Resampler { iir: [0; 6], fir: [0; 8], delay_buf: [0; 48], in_khz: (fs_in / 1000) as usize, out_khz: (fs_out / 1000) as usize, batch: (fs_in / 1000) as usize * 10, inv_ratio_q16: inv, input_delay: delay, two_x }
    }

    fn up2(iir: &mut [i32; 6], out: &mut [i16], inp: &[i16]) {
        for (k, &s) in inp.iter().enumerate() {
            let in32 = (s as i32) << 10;
            for (half, coef, base) in [(0usize, &t::RESAMPLER_UP2_HQ_0, 0usize), (1usize, &t::RESAMPLER_UP2_HQ_1, 3usize)] {
                let mut y = in32.wrapping_sub(iir[base]);
                let mut x = smulwb(y, coef[0] as i32);
                let o1 = iir[base].wrapping_add(x);
                iir[base] = in32.wrapping_add(x);
                y = o1.wrapping_sub(iir[base + 1]);
                x = smulwb(y, coef[1] as i32);
                let o2 = iir[base + 1].wrapping_add(x);
                iir[base + 1] = o1.wrapping_add(x);
                y = o2.wrapping_sub(iir[base + 2]);
                x = smlawb(y, y, coef[2] as i32);
                let o3 = iir[base + 2].wrapping_add(x);
                iir[base + 2] = o2.wrapping_add(x);
                out[2 * k + half] = sat16(rshift_round(o3, 10)) as i16;
            }
        }
    }

    fn iir_fir(&mut self, out: &mut Vec<i16>, inp: &[i16]) {
        let mut buf = vec![0i16; 2 * self.batch + 16];
        buf[..8].copy_from_slice(&self.fir);
        let mut rest = inp;
        let mut n_in;
        loop {
            n_in = rest.len().min(self.batch);
            Self::up2(&mut self.iir, &mut buf[8..8 + 2 * n_in], &rest[..n_in]);
            let max_index = (n_in as i32) << 17;
            let mut index = 0i32;
            while index < max_index {
                let ti = smulwb(index & 0xFFFF, 12) as usize;
                let p = &buf[(index >> 16) as usize..];
                let f = |row: usize, col: usize| t::RESAMPLER_FRAC_FIR_12[row * 4 + col] as i32;
                let mut r = smulbb(p[0] as i32, f(ti, 0));
                r = r.wrapping_add(smulbb(p[1] as i32, f(ti, 1)));
                r = r.wrapping_add(smulbb(p[2] as i32, f(ti, 2)));
                r = r.wrapping_add(smulbb(p[3] as i32, f(ti, 3)));
                r = r.wrapping_add(smulbb(p[4] as i32, f(11 - ti, 3)));
                r = r.wrapping_add(smulbb(p[5] as i32, f(11 - ti, 2)));
                r = r.wrapping_add(smulbb(p[6] as i32, f(11 - ti, 1)));
                r = r.wrapping_add(smulbb(p[7] as i32, f(11 - ti, 0)));
                out.push(sat16(rshift_round(r, 15)) as i16);
                index = index.wrapping_add(self.inv_ratio_q16);
            }
            rest = &rest[n_in..];
            if !rest.is_empty() {
                let mv: Vec<i16> = buf[2 * n_in..2 * n_in + 8].to_vec();
                buf[..8].copy_from_slice(&mv);
            } else {
                break;
            }
        }
        let tail: Vec<i16> = buf[2 * n_in..2 * n_in + 8].to_vec();
        self.fir.copy_from_slice(&tail);
    }

    fn run_part(&mut self, out: &mut Vec<i16>, inp: &[i16]) {
        if self.two_x {
            let mut tmp = vec![0i16; 2 * inp.len()];
            Self::up2(&mut self.iir, &mut tmp, inp);
            out.extend(tmp);
        } else {
            self.iir_fir(out, inp);
        }
    }

    /// Resamples `inp` (at least 1 ms): the first millisecond goes through the delay buffer, which makes the delay equal for all rates.
    fn process(&mut self, inp: &[i16]) -> Vec<i16> {
        let mut out = Vec::with_capacity(inp.len() * self.out_khz / self.in_khz.max(1) + 8);
        let n_samples = self.in_khz - self.input_delay;
        let d = self.input_delay;
        self.delay_buf[d..d + n_samples].copy_from_slice(&inp[..n_samples]);
        let first: Vec<i16> = self.delay_buf[..self.in_khz].to_vec();
        self.run_part(&mut out, &first);
        let tail_len = inp.len().saturating_sub(self.in_khz);
        self.run_part(&mut out, &inp[n_samples..n_samples + tail_len]);
        let l = inp.len();
        self.delay_buf[..d].copy_from_slice(&inp[l - d..l]);
        out
    }
}

struct Channel {
    fs_khz: i32,
    nb_subfr: usize,
    frame_length: usize,
    subfr_length: usize,
    ltp_mem_length: usize,
    lpc_order: usize,
    prev_nlsf_q15: [i16; MAX_LPC],
    exc_q14: [i32; MAX_FRAME],
    slpc_q14_buf: [i32; MAX_LPC],
    out_buf: [i16; OUT_BUF],
    lag_prev: i32,
    last_gain_index: i32,
    indices: Indices,
    vad_flags: [bool; 3],
    lbrr_flag: bool,
    lbrr_flags: [bool; 3],
    frames_decoded: usize,
    frames_per_packet: usize,
    ec_prev_signal_type: i32,
    ec_prev_lag_index: i32,
    prev_signal_type: i32,
    first_frame_after_reset: bool,
    loss_cnt: i32,
    prev_gain_q16: i32,
    plc: Plc,
    cng: Cng,
    resampler: Option<Resampler>,
    resampler_fs: i32,
}

impl Channel {
    fn new() -> Channel {
        let mut c = Channel {
            fs_khz: 0,
            nb_subfr: 0,
            frame_length: 0,
            subfr_length: 0,
            ltp_mem_length: 0,
            lpc_order: 0,
            prev_nlsf_q15: [0; MAX_LPC],
            exc_q14: [0; MAX_FRAME],
            slpc_q14_buf: [0; MAX_LPC],
            out_buf: [0; OUT_BUF],
            lag_prev: 0,
            last_gain_index: 0,
            indices: Indices::default(),
            vad_flags: [false; 3],
            lbrr_flag: false,
            lbrr_flags: [false; 3],
            frames_decoded: 0,
            frames_per_packet: 0,
            ec_prev_signal_type: 0,
            ec_prev_lag_index: 0,
            prev_signal_type: 0,
            first_frame_after_reset: true,
            loss_cnt: 0,
            prev_gain_q16: 65536,
            plc: Plc::default(),
            cng: Cng { exc_buf_q14: [0; MAX_FRAME], smth_nlsf_q15: [0; MAX_LPC], synth_state: [0; MAX_LPC], smth_gain_q16: 0, rand_seed: 0, fs_khz: 0 },
            resampler: None,
            resampler_fs: 0,
        };
        c.cng_reset();
        c.plc_reset();
        c
    }

    fn cng_reset(&mut self) {
        let step = div32(32767, self.lpc_order as i32 + 1);
        let mut acc = 0i32;
        for i in 0..self.lpc_order {
            acc += step;
            self.cng.smth_nlsf_q15[i] = acc as i16;
        }
        self.cng.smth_gain_q16 = 0;
        self.cng.rand_seed = 3_176_576;
    }

    fn plc_reset(&mut self) {
        self.plc.pitch_l_q8 = (self.frame_length as i32) << 7;
        self.plc.prev_gain_q16 = [65536, 65536];
        self.plc.subfr_length = 20;
        self.plc.nb_subfr = 2;
    }

    /// Sets the internal rate and frame layout (at the first frame of a packet).
    fn set_fs(&mut self, fs_khz: i32, nb_subfr: usize) {
        self.nb_subfr = nb_subfr;
        self.subfr_length = 5 * fs_khz as usize;
        let frame_length = nb_subfr * self.subfr_length;
        if self.resampler.is_none() || self.fs_khz != fs_khz || self.resampler_fs != 48000 {
            self.resampler = Some(Resampler::new(fs_khz * 1000, 48000));
            self.resampler_fs = 48000;
        }
        if self.fs_khz != fs_khz {
            self.ltp_mem_length = 20 * fs_khz as usize;
            self.lpc_order = if fs_khz == 16 { 16 } else { 10 };
            self.first_frame_after_reset = true;
            self.lag_prev = 100;
            self.last_gain_index = 10;
            self.prev_signal_type = TYPE_NO_VOICE;
            self.out_buf = [0; OUT_BUF];
            self.slpc_q14_buf = [0; MAX_LPC];
        }
        self.fs_khz = fs_khz;
        self.frame_length = frame_length;
    }

    fn cb(&self) -> &'static NlsfCb {
        if self.fs_khz == 16 {
            &CB_WB
        } else {
            &CB_NB_MB
        }
    }

    fn decode_indices(&mut self, dec: &mut RangeDecoder<'_>, frame_index: usize, decode_lbrr: bool, cond: i32) {
        let ix = if decode_lbrr || self.vad_flags[frame_index] { dec.icdf(&t::TYPE_OFFSET_VAD_ICDF, 8) as i32 + 2 } else { dec.icdf(&t::TYPE_OFFSET_NO_VAD_ICDF, 8) as i32 };
        let idx = &mut self.indices;
        idx.signal_type = ix >> 1;
        idx.quant_offset_type = ix & 1;
        if cond == CODE_CONDITIONALLY {
            idx.gains[0] = dec.icdf(&t::DELTA_GAIN_ICDF, 8) as i32;
        } else {
            idx.gains[0] = (dec.icdf(&t::GAIN_ICDF[idx.signal_type as usize * 8..], 8) as i32) << 3;
            idx.gains[0] += dec.icdf(&t::UNIFORM8_ICDF, 8) as i32;
        }
        for i in 1..self.nb_subfr {
            idx.gains[i] = dec.icdf(&t::DELTA_GAIN_ICDF, 8) as i32;
        }
        let cb = if self.fs_khz == 16 { &CB_WB } else { &CB_NB_MB };
        idx.nlsf[0] = dec.icdf(&cb.cb1_icdf[(idx.signal_type as usize >> 1) * 32..], 8) as i32;
        let (ec_ix, _) = nlsf_unpack(cb, idx.nlsf[0] as usize);
        for i in 0..cb.order {
            let mut v = dec.icdf(&cb.cb2_icdf[ec_ix[i] as usize..], 8) as i32;
            if v == 0 {
                v -= dec.icdf(&t::NLSF_EXT_ICDF, 8) as i32;
            } else if v == 2 * NLSF_QUANT_MAX_AMPLITUDE {
                v += dec.icdf(&t::NLSF_EXT_ICDF, 8) as i32;
            }
            idx.nlsf[i + 1] = v - NLSF_QUANT_MAX_AMPLITUDE;
        }
        idx.nlsf_interp_q2 = if self.nb_subfr == MAX_NB_SUBFR { dec.icdf(&t::NLSF_INTERP_FACTOR_ICDF, 8) as i32 } else { 4 };
        if idx.signal_type == TYPE_VOICED {
            let mut absolute = true;
            if cond == CODE_CONDITIONALLY && self.ec_prev_signal_type == TYPE_VOICED {
                let delta = dec.icdf(&t::PITCH_DELTA_ICDF, 8) as i32;
                if delta > 0 {
                    idx.lag_index = self.ec_prev_lag_index + delta - 9;
                    absolute = false;
                }
            }
            if absolute {
                idx.lag_index = dec.icdf(&t::PITCH_LAG_ICDF, 8) as i32 * (self.fs_khz >> 1);
                let low: &[u8] = match self.fs_khz {
                    16 => &t::UNIFORM8_ICDF,
                    12 => &t::UNIFORM6_ICDF,
                    _ => &t::UNIFORM4_ICDF,
                };
                idx.lag_index += dec.icdf(low, 8) as i32;
            }
            self.ec_prev_lag_index = idx.lag_index;
            let contour: &[u8] = match (self.fs_khz == 8, self.nb_subfr == MAX_NB_SUBFR) {
                (true, true) => &t::PITCH_CONTOUR_NB_ICDF,
                (true, false) => &t::PITCH_CONTOUR_10MS_NB_ICDF,
                (false, true) => &t::PITCH_CONTOUR_ICDF,
                (false, false) => &t::PITCH_CONTOUR_10MS_ICDF,
            };
            idx.contour_index = dec.icdf(contour, 8) as i32;
            idx.per_index = dec.icdf(&t::LTP_PER_INDEX_ICDF, 8) as i32;
            let ltp_icdf: &[u8] = match idx.per_index {
                0 => &t::LTP_GAIN_ICDF_0,
                1 => &t::LTP_GAIN_ICDF_1,
                _ => &t::LTP_GAIN_ICDF_2,
            };
            for k in 0..self.nb_subfr {
                idx.ltp_index[k] = dec.icdf(ltp_icdf, 8) as i32;
            }
            idx.ltp_scale_index = if cond == CODE_INDEPENDENTLY { dec.icdf(&t::LTP_SCALE_ICDF, 8) as i32 } else { 0 };
        }
        self.ec_prev_signal_type = idx.signal_type;
        idx.seed = dec.icdf(&t::UNIFORM4_ICDF, 8) as i32;
    }

    fn decode_pulses(&self, dec: &mut RangeDecoder<'_>, pulses: &mut [i32; MAX_FRAME + 16]) {
        let signal_type = self.indices.signal_type as usize;
        let rate_level = dec.icdf(&t::RATE_LEVELS_ICDF[(signal_type >> 1) * 9..], 8);
        let mut iter = self.frame_length >> 4;
        if iter * 16 < self.frame_length {
            iter += 1;
        }
        let mut sum_pulses = [0i32; 21];
        let mut nl_shifts = [0usize; 21];
        let cdf = &t::PULSES_PER_BLOCK_ICDF[rate_level * 18..];
        for i in 0..iter {
            sum_pulses[i] = dec.icdf(cdf, 8) as i32;
            while sum_pulses[i] == 17 {
                nl_shifts[i] += 1;
                let off = 9 * 18 + (nl_shifts[i] == 10) as usize;
                sum_pulses[i] = dec.icdf(&t::PULSES_PER_BLOCK_ICDF[off..], 8) as i32;
            }
        }
        for p in pulses.iter_mut() {
            *p = 0;
        }
        for i in 0..iter {
            if sum_pulses[i] > 0 {
                shell_decode(&mut pulses[i * 16..i * 16 + 16], dec, sum_pulses[i] as usize);
            }
        }
        for i in 0..iter {
            if nl_shifts[i] > 0 {
                let nls = nl_shifts[i];
                for k in 0..16 {
                    let mut abs_q = pulses[i * 16 + k];
                    for _ in 0..nls {
                        abs_q = (abs_q << 1) + dec.icdf(&t::LSB_ICDF, 8) as i32;
                    }
                    pulses[i * 16 + k] = abs_q;
                }
                sum_pulses[i] |= (nls as i32) << 5;
            }
        }
        // signs
        let icdf_base = 7 * (self.indices.quant_offset_type as usize + ((signal_type) << 1));
        let blocks = (self.frame_length + 8) >> 4;
        for i in 0..blocks {
            let p = sum_pulses[i];
            if p > 0 {
                let icdf = [t::SIGN_ICDF[icdf_base + ((p & 0x1f).min(6) as usize)], 0u8];
                for j in 0..16 {
                    let q = &mut pulses[i * 16 + j];
                    if *q > 0 {
                        *q *= 2 * dec.icdf(&icdf, 8) as i32 - 1;
                    }
                }
            }
        }
    }

    fn decode_parameters(&mut self, ctrl: &mut Ctrl, cond: i32) {
        // gains
        let conditional = cond == CODE_CONDITIONALLY;
        for k in 0..self.nb_subfr {
            if k == 0 && !conditional {
                self.last_gain_index = self.indices.gains[k].max(self.last_gain_index - 16);
            } else {
                let ind_tmp = self.indices.gains[k] + MIN_DELTA_GAIN;
                let threshold = 2 * MAX_DELTA_GAIN - N_LEVELS_QGAIN + self.last_gain_index;
                if ind_tmp > threshold {
                    self.last_gain_index += (ind_tmp << 1) - threshold;
                } else {
                    self.last_gain_index += ind_tmp;
                }
            }
            self.last_gain_index = self.last_gain_index.clamp(0, N_LEVELS_QGAIN - 1);
            ctrl.gains_q16[k] = log2lin((smulwb(1_907_825, self.last_gain_index) + 2090).min(3967));
        }
        let cb = self.cb();
        let nlsf = nlsf_decode(cb, &self.indices.nlsf);
        let order = self.lpc_order;
        nlsf2a(&mut ctrl.pred_coef_q12[1], &nlsf, order);
        if self.first_frame_after_reset {
            self.indices.nlsf_interp_q2 = 4;
        }
        if self.indices.nlsf_interp_q2 < 4 {
            let mut nlsf0 = [0i16; MAX_LPC];
            for i in 0..order {
                nlsf0[i] = (self.prev_nlsf_q15[i] as i32 + ((self.indices.nlsf_interp_q2 * (nlsf[i] as i32 - self.prev_nlsf_q15[i] as i32)) >> 2)) as i16;
            }
            nlsf2a(&mut ctrl.pred_coef_q12[0], &nlsf0, order);
        } else {
            ctrl.pred_coef_q12[0] = ctrl.pred_coef_q12[1];
        }
        self.prev_nlsf_q15[..order].copy_from_slice(&nlsf[..order]);
        if self.loss_cnt > 0 {
            bwexpander(&mut ctrl.pred_coef_q12[0], order, BWE_AFTER_LOSS_Q16);
            bwexpander(&mut ctrl.pred_coef_q12[1], order, BWE_AFTER_LOSS_Q16);
        }
        if self.indices.signal_type == TYPE_VOICED {
            // pitch lags
            let (table, size): (&[i8], usize) = match (self.fs_khz == 8, self.nb_subfr == MAX_NB_SUBFR) {
                (true, true) => (&t::CB_LAGS_STAGE2, 11),
                (true, false) => (&t::CB_LAGS_STAGE2_10MS, 3),
                (false, true) => (&t::CB_LAGS_STAGE3, 34),
                (false, false) => (&t::CB_LAGS_STAGE3_10MS, 12),
            };
            let min_lag = 2 * self.fs_khz;
            let max_lag = 18 * self.fs_khz;
            let lag = min_lag + self.indices.lag_index;
            for k in 0..self.nb_subfr {
                let c = table.get(k * size + self.indices.contour_index as usize).copied().unwrap_or(0) as i32;
                ctrl.pitch_l[k] = (lag + c).clamp(min_lag, max_lag);
            }
            let (vq, n): (&[i8], usize) = match self.indices.per_index {
                0 => (&t::LTP_GAIN_VQ_0, 8),
                1 => (&t::LTP_GAIN_VQ_1, 16),
                _ => (&t::LTP_GAIN_VQ_2, 32),
            };
            for k in 0..self.nb_subfr {
                let ix = (self.indices.ltp_index[k] as usize).min(n - 1);
                for i in 0..LTP_ORDER {
                    ctrl.ltp_coef_q14[k * LTP_ORDER + i] = (vq[ix * LTP_ORDER + i] as i16) << 7;
                }
            }
            ctrl.ltp_scale_q14 = t::LTP_SCALES_Q14[(self.indices.ltp_scale_index as usize).min(2)] as i32;
        } else {
            ctrl.pitch_l = [0; MAX_NB_SUBFR];
            ctrl.ltp_coef_q14 = [0; LTP_ORDER * MAX_NB_SUBFR];
            self.indices.per_index = 0;
            ctrl.ltp_scale_q14 = 0;
        }
    }

    /// The synthesis: excitation, pitch prediction, LPC prediction.
    fn decode_core(&mut self, ctrl: &mut Ctrl, xq: &mut [i16], pulses: &[i32]) {
        let offset_q10 = t::QUANTIZATION_OFFSETS_Q10[(self.indices.signal_type as usize >> 1) * 2 + self.indices.quant_offset_type as usize] as i32;
        let interp_flag = self.indices.nlsf_interp_q2 < 4;
        let mut rand_seed = self.indices.seed;
        for i in 0..self.frame_length {
            rand_seed = rand(rand_seed);
            let mut e = shl(pulses[i], 14);
            if e > 0 {
                e -= 80 << 4;
            } else if e < 0 {
                e += 80 << 4;
            }
            e += offset_q10 << 4;
            if rand_seed < 0 {
                e = -e;
            }
            self.exc_q14[i] = e;
            rand_seed = rand_seed.wrapping_add(pulses[i]);
        }
        let mut slpc = [0i32; MAX_SUBFR + MAX_LPC];
        slpc[..MAX_LPC].copy_from_slice(&self.slpc_q14_buf);
        let mut sltp = [0i16; MAX_FRAME];
        let mut sltp_q15 = [0i32; 2 * MAX_FRAME];
        let mut sltp_buf_idx = self.ltp_mem_length;
        let order = self.lpc_order;
        let subfr = self.subfr_length;
        let mut res_q14 = [0i32; MAX_SUBFR];
        let mut pexc = 0usize;
        let mut pxq = 0usize;
        for k in 0..self.nb_subfr {
            let a_q12 = ctrl.pred_coef_q12[k >> 1];
            let mut b_q14 = [0i16; LTP_ORDER];
            b_q14.copy_from_slice(&ctrl.ltp_coef_q14[k * LTP_ORDER..k * LTP_ORDER + LTP_ORDER]);
            let mut signal_type = self.indices.signal_type;
            let gain_q10 = ctrl.gains_q16[k] >> 6;
            let mut inv_gain_q31 = inverse32_varq(ctrl.gains_q16[k], 47);
            let gain_adj_q16;
            if ctrl.gains_q16[k] != self.prev_gain_q16 {
                gain_adj_q16 = div32_varq(self.prev_gain_q16, ctrl.gains_q16[k], 16);
                for v in slpc.iter_mut().take(MAX_LPC) {
                    *v = smulww(gain_adj_q16, *v);
                }
            } else {
                gain_adj_q16 = 1 << 16;
            }
            self.prev_gain_q16 = ctrl.gains_q16[k];
            if self.loss_cnt > 0 && self.prev_signal_type == TYPE_VOICED && self.indices.signal_type != TYPE_VOICED && k < MAX_NB_SUBFR / 2 {
                b_q14 = [0; LTP_ORDER];
                b_q14[LTP_ORDER / 2] = 4096;
                signal_type = TYPE_VOICED;
                ctrl.ltp_coef_q14[k * LTP_ORDER..k * LTP_ORDER + LTP_ORDER].copy_from_slice(&b_q14);
                ctrl.pitch_l[k] = self.lag_prev;
            }
            let mut lag = 0usize;
            if signal_type == TYPE_VOICED {
                lag = ctrl.pitch_l[k].clamp(1, 18 * self.fs_khz) as usize;
                if k == 0 || (k == 2 && interp_flag) {
                    let start_idx = (self.ltp_mem_length as i32 - lag as i32 - order as i32 - LTP_ORDER as i32 / 2).max(0) as usize;
                    if k == 2 {
                        let n = 2 * subfr;
                        self.out_buf[self.ltp_mem_length..self.ltp_mem_length + n].copy_from_slice(&xq[..n]);
                    }
                    let src_off = start_idx + k * subfr;
                    let len = self.ltp_mem_length - start_idx;
                    let (src, dst) = (&self.out_buf[src_off..], &mut sltp[start_idx..]);
                    lpc_analysis_filter(dst, src, &a_q12, len, order);
                    if k == 0 {
                        inv_gain_q31 = shl(smulwb(inv_gain_q31, ctrl.ltp_scale_q14), 2);
                    }
                    for i in 0..lag + LTP_ORDER / 2 {
                        sltp_q15[sltp_buf_idx - i - 1] = smulwb(inv_gain_q31, sltp[self.ltp_mem_length - i - 1] as i32);
                    }
                } else if gain_adj_q16 != 1 << 16 {
                    for i in 0..lag + LTP_ORDER / 2 {
                        sltp_q15[sltp_buf_idx - i - 1] = smulww(gain_adj_q16, sltp_q15[sltp_buf_idx - i - 1]);
                    }
                }
            }
            let use_res;
            if signal_type == TYPE_VOICED {
                let mut pl = sltp_buf_idx - lag + LTP_ORDER / 2;
                for i in 0..subfr {
                    let mut ltp = 2i32;
                    for (j, &b) in b_q14.iter().enumerate() {
                        ltp = smlawb(ltp, sltp_q15[pl - j], b as i32);
                    }
                    pl += 1;
                    res_q14[i] = self.exc_q14[pexc + i].wrapping_add(shl(ltp, 1));
                    sltp_q15[sltp_buf_idx] = shl(res_q14[i], 1);
                    sltp_buf_idx += 1;
                }
                use_res = true;
            } else {
                use_res = false;
            }
            for i in 0..subfr {
                let mut pred = (order >> 1) as i32;
                for j in 0..order {
                    pred = smlawb(pred, slpc[MAX_LPC + i - 1 - j], a_q12[j] as i32);
                }
                let r = if use_res { res_q14[i] } else { self.exc_q14[pexc + i] };
                slpc[MAX_LPC + i] = r.wrapping_add(shl(pred, 4));
                xq[pxq + i] = sat16(rshift_round(smulww(slpc[MAX_LPC + i], gain_q10), 8)) as i16;
            }
            let mut next = [0i32; MAX_LPC];
            next.copy_from_slice(&slpc[subfr..subfr + MAX_LPC]);
            slpc[..MAX_LPC].copy_from_slice(&next);
            pexc += subfr;
            pxq += subfr;
        }
        self.slpc_q14_buf.copy_from_slice(&slpc[..MAX_LPC]);
    }

    fn plc_update(&mut self, ctrl: &Ctrl) {
        self.prev_signal_type = self.indices.signal_type;
        let mut ltp_gain_q14 = 0i32;
        let plc = &mut self.plc;
        if self.indices.signal_type == TYPE_VOICED {
            let pitch_last = ctrl.pitch_l[self.nb_subfr - 1];
            let mut j = 0usize;
            while ((j * self.subfr_length) as i32) < pitch_last {
                if j == self.nb_subfr {
                    break;
                }
                let mut tmp = 0i32;
                for i in 0..LTP_ORDER {
                    tmp += ctrl.ltp_coef_q14[(self.nb_subfr - 1 - j) * LTP_ORDER + i] as i32;
                }
                if tmp > ltp_gain_q14 {
                    ltp_gain_q14 = tmp;
                    plc.ltp_coef_q14.copy_from_slice(&ctrl.ltp_coef_q14[(self.nb_subfr - 1 - j) * LTP_ORDER..(self.nb_subfr - j) * LTP_ORDER]);
                    plc.pitch_l_q8 = ctrl.pitch_l[self.nb_subfr - 1 - j] << 8;
                }
                j += 1;
            }
            plc.ltp_coef_q14 = [0; LTP_ORDER];
            plc.ltp_coef_q14[LTP_ORDER / 2] = ltp_gain_q14 as i16;
            const MIN_Q14: i32 = 11469;
            const MAX_Q14: i32 = 15565;
            if ltp_gain_q14 < MIN_Q14 {
                let scale_q10 = div32(MIN_Q14 << 10, ltp_gain_q14.max(1));
                for c in plc.ltp_coef_q14.iter_mut() {
                    *c = (smulbb(*c as i32, scale_q10) >> 10) as i16;
                }
            } else if ltp_gain_q14 > MAX_Q14 {
                let scale_q14 = div32(MAX_Q14 << 14, ltp_gain_q14.max(1));
                for c in plc.ltp_coef_q14.iter_mut() {
                    *c = (smulbb(*c as i32, scale_q14) >> 14) as i16;
                }
            }
        } else {
            plc.pitch_l_q8 = (smulbb(self.fs_khz, 18)) << 8;
            plc.ltp_coef_q14 = [0; LTP_ORDER];
        }
        plc.prev_lpc_q12 = ctrl.pred_coef_q12[1];
        plc.prev_ltp_scale_q14 = ctrl.ltp_scale_q14;
        plc.prev_gain_q16 = [ctrl.gains_q16[self.nb_subfr - 2], ctrl.gains_q16[self.nb_subfr - 1]];
        plc.subfr_length = self.subfr_length;
        plc.nb_subfr = self.nb_subfr;
    }

    /// Makes a frame for a lost packet: the last pitch cycle, a fading copy, with noise.
    fn plc_conceal(&mut self, ctrl: &mut Ctrl, frame: &mut [i16]) {
        const HARM_ATT_Q15: [i32; 2] = [32440, 31130];
        const RAND_ATT_V_Q15: [i32; 2] = [31130, 26214];
        const RAND_ATT_UV_Q15: [i32; 2] = [32440, 29491];
        const RAND_BUF: usize = 128;
        let order = self.lpc_order;
        let prev_gain_q10 = [self.plc.prev_gain_q16[0] >> 6, self.plc.prev_gain_q16[1] >> 6];
        if self.first_frame_after_reset {
            self.plc.prev_lpc_q12 = [0; MAX_LPC];
        }
        let sub = self.plc.subfr_length.max(1);
        let nbs = self.plc.nb_subfr.max(2);
        let mut exc_buf = [0i16; 2 * MAX_SUBFR];
        for k in 0..2 {
            for i in 0..sub.min(MAX_SUBFR) {
                let at = i + (k + nbs - 2) * sub;
                let v = self.exc_q14.get(at).copied().unwrap_or(0);
                exc_buf[k * sub + i] = sat16(smulww(v, prev_gain_q10[k]) >> 8) as i16;
            }
        }
        let (energy1, shift1) = sum_sqr_shift(&exc_buf[..sub]);
        let (energy2, shift2) = sum_sqr_shift(&exc_buf[sub..2 * sub]);
        let rand_off = if (energy1 >> shift2.min(31)) < (energy2 >> shift1.min(31)) { ((nbs - 1) * sub).saturating_sub(RAND_BUF) } else { (nbs * sub).saturating_sub(RAND_BUF) };
        let mut b_q14 = self.plc.ltp_coef_q14;
        let mut rand_scale_q14 = self.plc.rand_scale_q14;
        let loss = (self.loss_cnt.max(0) as usize).min(1);
        let harm_gain_q15 = HARM_ATT_Q15[loss];
        let mut rand_gain_q15 = if self.prev_signal_type == TYPE_VOICED { RAND_ATT_V_Q15[loss] } else { RAND_ATT_UV_Q15[loss] };
        let mut prev_lpc = self.plc.prev_lpc_q12;
        bwexpander(&mut prev_lpc, order, 64881);
        self.plc.prev_lpc_q12 = prev_lpc;
        let a_q12 = prev_lpc;
        if self.loss_cnt == 0 {
            rand_scale_q14 = 1 << 14;
            if self.prev_signal_type == TYPE_VOICED {
                for &b in b_q14.iter() {
                    rand_scale_q14 = rand_scale_q14.wrapping_sub(b);
                }
                rand_scale_q14 = rand_scale_q14.max(3277);
                rand_scale_q14 = (smulbb(rand_scale_q14 as i32, self.plc.prev_ltp_scale_q14) >> 14) as i16;
            } else {
                let inv_gain_q30 = lpc_inverse_pred_gain(&self.plc.prev_lpc_q12, order);
                let mut down = ((1i32 << 30) >> 3).min(inv_gain_q30);
                down = ((1i32 << 30) >> 8).max(down);
                down = shl(down, 3);
                rand_gain_q15 = smulwb(down, rand_gain_q15) >> 14;
            }
        }
        let mut rand_seed = self.plc.rand_seed;
        let mut lag = rshift_round(self.plc.pitch_l_q8, 8);
        let mut sltp_buf_idx = self.ltp_mem_length;
        let mut sltp = [0i16; MAX_FRAME];
        let mut sltp_q14 = [0i32; 2 * MAX_FRAME];
        let idx0 = (self.ltp_mem_length as i32 - lag - order as i32 - LTP_ORDER as i32 / 2).max(0) as usize;
        let len = self.ltp_mem_length - idx0;
        let (src, dst) = (&self.out_buf[idx0..], &mut sltp[idx0..]);
        lpc_analysis_filter(dst, src, &a_q12, len, order);
        let inv_gain_q30 = inverse32_varq(self.plc.prev_gain_q16[1], 46).min(i32::MAX >> 1);
        for i in idx0 + order..self.ltp_mem_length {
            sltp_q14[i] = smulwb(inv_gain_q30, sltp[i] as i32);
        }
        for _ in 0..self.nb_subfr {
            let lag_u = lag.clamp(1, 18 * self.fs_khz) as usize;
            let mut pl = sltp_buf_idx - lag_u + LTP_ORDER / 2;
            for _ in 0..self.subfr_length {
                let mut ltp = 2i32;
                for (j, &b) in b_q14.iter().enumerate() {
                    ltp = smlawb(ltp, sltp_q14[pl - j], b as i32);
                }
                pl += 1;
                rand_seed = rand(rand_seed);
                let idx = ((rand_seed >> 25) & (RAND_BUF as i32 - 1)) as usize;
                let r = self.exc_q14.get(rand_off + idx).copied().unwrap_or(0);
                sltp_q14[sltp_buf_idx] = shl(smlawb(ltp, r, rand_scale_q14 as i32), 2);
                sltp_buf_idx += 1;
            }
            for b in b_q14.iter_mut() {
                *b = (smulbb(harm_gain_q15, *b as i32) >> 15) as i16;
            }
            rand_scale_q14 = (smulbb(rand_scale_q14 as i32, rand_gain_q15) >> 15) as i16;
            self.plc.pitch_l_q8 = smlawb(self.plc.pitch_l_q8, self.plc.pitch_l_q8, 655);
            self.plc.pitch_l_q8 = self.plc.pitch_l_q8.min(shl(smulbb(18, self.fs_khz), 8));
            lag = rshift_round(self.plc.pitch_l_q8, 8);
        }
        // LPC synthesis
        let base = self.ltp_mem_length - MAX_LPC;
        sltp_q14[base..base + MAX_LPC].copy_from_slice(&self.slpc_q14_buf);
        for i in 0..self.frame_length {
            let mut pred = (order >> 1) as i32;
            for j in 0..order {
                pred = smlawb(pred, sltp_q14[base + MAX_LPC + i - 1 - j], a_q12[j] as i32);
            }
            let at = base + MAX_LPC + i;
            sltp_q14[at] = sltp_q14[at].wrapping_add(shl(pred, 4));
            frame[i] = sat16(sat16(rshift_round(smulww(sltp_q14[at], prev_gain_q10[1]), 8))) as i16;
        }
        let end = base + self.frame_length;
        let mut st = [0i32; MAX_LPC];
        st.copy_from_slice(&sltp_q14[end..end + MAX_LPC]);
        self.slpc_q14_buf = st;
        self.plc.rand_seed = rand_seed;
        self.plc.rand_scale_q14 = rand_scale_q14;
        for p in ctrl.pitch_l.iter_mut() {
            *p = lag;
        }
    }

    /// Fades in a good frame after concealed ones so the energy does not jump.
    fn plc_glue(&mut self, frame: &mut [i16]) {
        let length = self.frame_length;
        if self.loss_cnt > 0 {
            let (e, s) = sum_sqr_shift(&frame[..length]);
            self.plc.conc_energy = e;
            self.plc.conc_energy_shift = s;
            self.plc.last_frame_lost = true;
        } else {
            if self.plc.last_frame_lost {
                let (mut energy, energy_shift) = sum_sqr_shift(&frame[..length]);
                if energy_shift > self.plc.conc_energy_shift {
                    self.plc.conc_energy >>= (energy_shift - self.plc.conc_energy_shift).min(31);
                } else if energy_shift < self.plc.conc_energy_shift {
                    energy >>= (self.plc.conc_energy_shift - energy_shift).min(31);
                }
                if energy > self.plc.conc_energy {
                    let lz = clz32(self.plc.conc_energy) - 1;
                    self.plc.conc_energy = shl(self.plc.conc_energy, lz);
                    energy >>= (24 - lz).max(0).min(31);
                    let frac_q24 = div32(self.plc.conc_energy, energy.max(1));
                    let mut gain_q16 = shl(sqrt_approx(frac_q24), 4);
                    let mut slope_q16 = div32((1 << 16) - gain_q16, length as i32);
                    slope_q16 = shl(slope_q16, 2);
                    for f in frame.iter_mut().take(length) {
                        *f = smulwb(gain_q16, *f as i32) as i16;
                        gain_q16 = gain_q16.wrapping_add(slope_q16);
                        if gain_q16 > 1 << 16 {
                            break;
                        }
                    }
                }
            }
            self.plc.last_frame_lost = false;
        }
    }

    /// Comfort noise: tracks the background during quiet frames and plays it when frames are lost.
    fn cng_apply(&mut self, ctrl: &Ctrl, frame: &mut [i16]) {
        let length = self.frame_length;
        let order = self.lpc_order;
        if self.fs_khz != self.cng.fs_khz {
            self.cng_reset();
            self.cng.fs_khz = self.fs_khz;
        }
        if self.loss_cnt == 0 && self.prev_signal_type == TYPE_NO_VOICE {
            for i in 0..order {
                self.cng.smth_nlsf_q15[i] = (self.cng.smth_nlsf_q15[i] as i32 + smulwb(self.prev_nlsf_q15[i] as i32 - self.cng.smth_nlsf_q15[i] as i32, 16348)) as i16;
            }
            let (mut max_gain, mut subfr) = (0i32, 0usize);
            for i in 0..self.nb_subfr {
                if ctrl.gains_q16[i] > max_gain {
                    max_gain = ctrl.gains_q16[i];
                    subfr = i;
                }
            }
            let sl = self.subfr_length;
            self.cng.exc_buf_q14.copy_within(0..(self.nb_subfr - 1) * sl, sl);
            let src: Vec<i32> = self.exc_q14[subfr * sl..subfr * sl + sl].to_vec();
            self.cng.exc_buf_q14[..sl].copy_from_slice(&src);
            for i in 0..self.nb_subfr {
                self.cng.smth_gain_q16 = self.cng.smth_gain_q16.wrapping_add(smulwb(ctrl.gains_q16[i].wrapping_sub(self.cng.smth_gain_q16), 4634));
            }
        }
        if self.loss_cnt > 0 {
            let mut sig = [0i32; MAX_FRAME + MAX_LPC];
            let mut exc_mask = 255usize;
            while exc_mask > length {
                exc_mask >>= 1;
            }
            let mut seed = self.cng.rand_seed;
            for i in 0..length {
                seed = rand(seed);
                let idx = ((seed >> 24) as usize) & exc_mask;
                sig[MAX_LPC + i] = sat16(smulww(self.cng.exc_buf_q14[idx], self.cng.smth_gain_q16 >> 4));
            }
            self.cng.rand_seed = seed;
            let mut a_q12 = [0i16; MAX_LPC];
            nlsf2a(&mut a_q12, &self.cng.smth_nlsf_q15, order);
            sig[..MAX_LPC].copy_from_slice(&self.cng.synth_state);
            for i in 0..length {
                let mut sum_q6 = (order >> 1) as i32;
                for j in 0..order {
                    sum_q6 = smlawb(sum_q6, sig[MAX_LPC + i - 1 - j], a_q12[j] as i32);
                }
                sig[MAX_LPC + i] = sig[MAX_LPC + i].wrapping_add(shl(sum_q6, 4));
                frame[i] = sat16(frame[i] as i32 + rshift_round(sum_q6, 6)) as i16;
            }
            let mut st = [0i32; MAX_LPC];
            st.copy_from_slice(&sig[length..length + MAX_LPC]);
            self.cng.synth_state = st;
        } else {
            self.cng.synth_state = [0; MAX_LPC];
        }
    }

    /// Decodes (or conceals) one frame; returns its length.
    fn decode_frame(&mut self, dec: &mut RangeDecoder<'_>, out: &mut [i16], lost: bool, cond: i32) -> usize {
        let l = self.frame_length;
        let mut ctrl = Ctrl::default();
        if !lost {
            let mut pulses = [0i32; MAX_FRAME + 16];
            let fd = self.frames_decoded;
            self.decode_indices(dec, fd.min(2), false, cond);
            self.decode_pulses(dec, &mut pulses);
            self.decode_parameters(&mut ctrl, cond);
            self.decode_core(&mut ctrl, out, &pulses);
            if self.plc.fs_khz != self.fs_khz {
                self.plc_reset();
                self.plc.fs_khz = self.fs_khz;
            }
            self.plc_update(&ctrl);
            self.loss_cnt = 0;
            self.prev_signal_type = self.indices.signal_type;
            self.first_frame_after_reset = false;
        } else {
            if self.plc.fs_khz != self.fs_khz {
                self.plc_reset();
                self.plc.fs_khz = self.fs_khz;
            }
            self.plc_conceal(&mut ctrl, out);
            self.loss_cnt += 1;
        }
        let mv = self.ltp_mem_length - l;
        self.out_buf.copy_within(l..l + mv, 0);
        self.out_buf[mv..mv + l].copy_from_slice(&out[..l]);
        self.plc_glue(out);
        self.cng_apply(&ctrl, out);
        self.lag_prev = ctrl.pitch_l[self.nb_subfr - 1];
        l
    }
}

/// Reads the 16 pulse amplitudes of one shell block whose sum is `total`, by halving the sum down a binary tree.
fn shell_decode(pulses0: &mut [i32], dec: &mut RangeDecoder<'_>, total: usize) {
    let (mut p3, mut p2, mut p1) = ([0i32; 2], [0i32; 4], [0i32; 8]);
    let split = |p: i32, table: &[u8], dec: &mut RangeDecoder<'_>| -> (i32, i32) {
        if p > 0 {
            let c1 = dec.icdf(&table[t::SHELL_CODE_TABLE_OFFSETS[p as usize] as usize..], 8) as i32;
            (c1, p - c1)
        } else {
            (0, 0)
        }
    };
    (p3[0], p3[1]) = split(total as i32, &t::SHELL_CODE_TABLE3, dec);
    (p2[0], p2[1]) = split(p3[0], &t::SHELL_CODE_TABLE2, dec);
    (p1[0], p1[1]) = split(p2[0], &t::SHELL_CODE_TABLE1, dec);
    (pulses0[0], pulses0[1]) = split(p1[0], &t::SHELL_CODE_TABLE0, dec);
    (pulses0[2], pulses0[3]) = split(p1[1], &t::SHELL_CODE_TABLE0, dec);
    (p1[2], p1[3]) = split(p2[1], &t::SHELL_CODE_TABLE1, dec);
    (pulses0[4], pulses0[5]) = split(p1[2], &t::SHELL_CODE_TABLE0, dec);
    (pulses0[6], pulses0[7]) = split(p1[3], &t::SHELL_CODE_TABLE0, dec);
    (p2[2], p2[3]) = split(p3[1], &t::SHELL_CODE_TABLE2, dec);
    (p1[4], p1[5]) = split(p2[2], &t::SHELL_CODE_TABLE1, dec);
    (pulses0[8], pulses0[9]) = split(p1[4], &t::SHELL_CODE_TABLE0, dec);
    (pulses0[10], pulses0[11]) = split(p1[5], &t::SHELL_CODE_TABLE0, dec);
    (p1[6], p1[7]) = split(p2[3], &t::SHELL_CODE_TABLE1, dec);
    (pulses0[12], pulses0[13]) = split(p1[6], &t::SHELL_CODE_TABLE0, dec);
    (pulses0[14], pulses0[15]) = split(p1[7], &t::SHELL_CODE_TABLE0, dec);
}

// ---- stereo

fn stereo_decode_pred(dec: &mut RangeDecoder<'_>) -> [i32; 2] {
    let n = dec.icdf(&t::STEREO_PRED_JOINT_ICDF, 8) as i32;
    let mut ix = [[0i32; 3]; 2];
    ix[0][2] = n / 5;
    ix[1][2] = n - 5 * ix[0][2];
    for n in 0..2 {
        ix[n][0] = dec.icdf(&t::UNIFORM3_ICDF, 8) as i32;
        ix[n][1] = dec.icdf(&t::UNIFORM5_ICDF, 8) as i32;
    }
    let mut pred = [0i32; 2];
    for n in 0..2 {
        ix[n][0] += 3 * ix[n][2];
        let low = t::STEREO_PRED_QUANT_Q13[ix[n][0] as usize] as i32;
        let step = smulwb(t::STEREO_PRED_QUANT_Q13[ix[n][0] as usize + 1] as i32 - low, 6554);
        pred[n] = low.wrapping_add(smulbb(step, 2 * ix[n][1] + 1));
    }
    pred[0] -= pred[1];
    pred
}

#[derive(Default)]
struct StereoState {
    pred_prev_q13: [i32; 2],
    s_mid: [i16; 2],
    s_side: [i16; 2],
}

fn stereo_ms_to_lr(st: &mut StereoState, x1: &mut [i16], x2: &mut [i16], pred_q13: [i32; 2], fs_khz: i32, frame_length: usize) {
    x1[..2].copy_from_slice(&st.s_mid);
    x2[..2].copy_from_slice(&st.s_side);
    st.s_mid.copy_from_slice(&x1[frame_length..frame_length + 2]);
    st.s_side.copy_from_slice(&x2[frame_length..frame_length + 2]);
    let (mut pred0, mut pred1) = (st.pred_prev_q13[0], st.pred_prev_q13[1]);
    let denom_q16 = div32(1 << 16, 8 * fs_khz);
    let delta0 = rshift_round(smulbb(pred_q13[0] - st.pred_prev_q13[0], denom_q16), 16);
    let delta1 = rshift_round(smulbb(pred_q13[1] - st.pred_prev_q13[1], denom_q16), 16);
    let interp = (8 * fs_khz) as usize;
    for n in 0..interp.min(frame_length) {
        pred0 += delta0;
        pred1 += delta1;
        let mut sum = (x1[n] as i32 + x1[n + 2] as i32 + ((x1[n + 1] as i32) << 1)) << 9;
        sum = smlawb((x2[n + 1] as i32) << 8, sum, pred0);
        sum = smlawb(sum, (x1[n + 1] as i32) << 11, pred1);
        x2[n + 1] = sat16(rshift_round(sum, 8)) as i16;
    }
    pred0 = pred_q13[0];
    pred1 = pred_q13[1];
    for n in interp..frame_length {
        let mut sum = (x1[n] as i32 + x1[n + 2] as i32 + ((x1[n + 1] as i32) << 1)) << 9;
        sum = smlawb((x2[n + 1] as i32) << 8, sum, pred0);
        sum = smlawb(sum, (x1[n + 1] as i32) << 11, pred1);
        x2[n + 1] = sat16(rshift_round(sum, 8)) as i16;
    }
    st.pred_prev_q13 = pred_q13;
    for n in 0..frame_length {
        let sum = x1[n + 1] as i32 + x2[n + 1] as i32;
        let diff = x1[n + 1] as i32 - x2[n + 1] as i32;
        x1[n + 1] = sat16(sum) as i16;
        x2[n + 1] = sat16(diff) as i16;
    }
}

// ---- the decoder

/// What the Opus layer tells the SILK decoder about the packet.
#[derive(Clone, Copy, Debug)]
pub struct SilkControl {
    /// Channels in the output (1 or 2).
    pub channels_api: usize,
    /// Channels coded in the stream (1 or 2).
    pub channels_internal: usize,
    /// 8000, 12000 or 16000.
    pub internal_rate: i32,
    /// Length of the packet's SILK part in ms: 10, 20, 40 or 60 (0 for a lost packet, taken as 10).
    pub payload_ms: i32,
}

pub struct SilkDecoder {
    ch: [Channel; 2],
    stereo: StereoState,
    channels_api: usize,
    channels_internal: usize,
    prev_decode_only_middle: bool,
    /// Pitch lag of the last frame at 48 kHz (0 if not voiced), for the concealment of the CELT part.
    pub prev_pitch_lag: i32,
}

impl Default for SilkDecoder {
    fn default() -> Self {
        SilkDecoder::new()
    }
}

impl SilkDecoder {
    pub fn new() -> SilkDecoder {
        SilkDecoder { ch: [Channel::new(), Channel::new()], stereo: StereoState::default(), channels_api: 0, channels_internal: 0, prev_decode_only_middle: false, prev_pitch_lag: 0 }
    }

    /// Decodes the part of the packet that one call covers (a 10 or 20 ms frame): returns the interleaved 48 kHz sound. `lost` conceals
    /// instead of reading. `new_packet` is true for the first call of a packet.
    pub fn decode(&mut self, ctl: SilkControl, lost: bool, new_packet: bool, dec: &mut RangeDecoder<'_>) -> Vec<i16> {
        let n_int = ctl.channels_internal.clamp(1, 2);
        if new_packet {
            for n in 0..n_int {
                self.ch[n].frames_decoded = 0;
            }
        }
        if n_int > self.channels_internal {
            self.ch[1] = Channel::new();
        }
        let stereo_to_mono = n_int == 1 && self.channels_internal == 2 && ctl.internal_rate == 1000 * self.ch[0].fs_khz;
        if self.ch[0].frames_decoded == 0 {
            for n in 0..n_int {
                let (fpp, nb) = match ctl.payload_ms {
                    0 | 10 => (1, 2),
                    20 => (1, 4),
                    40 => (2, 4),
                    _ => (3, 4),
                };
                self.ch[n].frames_per_packet = fpp;
                let fs = (ctl.internal_rate >> 10) + 1;
                self.ch[n].set_fs(fs.clamp(8, 16), nb);
            }
        }
        if ctl.channels_api == 2 && n_int == 2 && (self.channels_api == 1 || self.channels_internal == 1) {
            self.stereo.pred_prev_q13 = [0; 2];
            self.stereo.s_side = [0; 2];
            let r = self.ch[0].resampler.clone();
            self.ch[1].resampler = r;
        }
        self.channels_api = ctl.channels_api;
        self.channels_internal = n_int;
        let mut ms_pred = [0i32; 2];
        let mut decode_only_middle = false;
        if !lost && self.ch[0].frames_decoded == 0 {
            for n in 0..n_int {
                let fpp = self.ch[n].frames_per_packet;
                for i in 0..fpp {
                    self.ch[n].vad_flags[i] = dec.bit_logp(1);
                }
                self.ch[n].lbrr_flag = dec.bit_logp(1);
            }
            for n in 0..n_int {
                self.ch[n].lbrr_flags = [false; 3];
                if self.ch[n].lbrr_flag {
                    let fpp = self.ch[n].frames_per_packet;
                    if fpp == 1 {
                        self.ch[n].lbrr_flags[0] = true;
                    } else {
                        let icdf: &[u8] = if fpp == 2 { &t::LBRR_FLAGS_2_ICDF } else { &t::LBRR_FLAGS_3_ICDF };
                        let sym = dec.icdf(icdf, 8) + 1;
                        for i in 0..fpp {
                            self.ch[n].lbrr_flags[i] = (sym >> i) & 1 != 0;
                        }
                    }
                }
            }
            // regular decoding skips the redundant (LBRR) data
            let fpp0 = self.ch[0].frames_per_packet;
            for i in 0..fpp0 {
                for n in 0..n_int {
                    if self.ch[n].lbrr_flags[i] {
                        let mut pulses = [0i32; MAX_FRAME + 16];
                        if n_int == 2 && n == 0 {
                            let _ = stereo_decode_pred(dec);
                            if !self.ch[1].lbrr_flags[i] {
                                let _ = dec.icdf(&t::STEREO_ONLY_CODE_MID_ICDF, 8);
                            }
                        }
                        let cond = if i > 0 && self.ch[n].lbrr_flags[i - 1] { CODE_CONDITIONALLY } else { CODE_INDEPENDENTLY };
                        self.ch[n].decode_indices(dec, i, true, cond);
                        self.ch[n].decode_pulses(dec, &mut pulses);
                    }
                }
            }
        }
        if n_int == 2 {
            if !lost {
                ms_pred = stereo_decode_pred(dec);
                let fd = self.ch[0].frames_decoded;
                if !self.ch[1].vad_flags[fd.min(2)] {
                    decode_only_middle = dec.icdf(&t::STEREO_ONLY_CODE_MID_ICDF, 8) != 0;
                }
            } else {
                ms_pred = self.stereo.pred_prev_q13;
            }
        }
        if n_int == 2 && !decode_only_middle && self.prev_decode_only_middle {
            let c = &mut self.ch[1];
            c.out_buf = [0; OUT_BUF];
            c.slpc_q14_buf = [0; MAX_LPC];
            c.lag_prev = 100;
            c.last_gain_index = 10;
            c.prev_signal_type = TYPE_NO_VOICE;
            c.first_frame_after_reset = true;
        }
        let has_side = if !lost { !decode_only_middle } else { !self.prev_decode_only_middle };
        let mut tmp: [Vec<i16>; 2] = [vec![0i16; MAX_FRAME + 2], vec![0i16; MAX_FRAME + 2]];
        let mut n_dec = self.ch[0].frame_length;
        for n in 0..n_int {
            if n == 0 || has_side {
                let frame_index = self.ch[0].frames_decoded as i32 - n as i32;
                let cond = if frame_index <= 0 {
                    CODE_INDEPENDENTLY
                } else if n > 0 && self.prev_decode_only_middle {
                    CODE_INDEPENDENTLY_NO_LTP_SCALING
                } else {
                    CODE_CONDITIONALLY
                };
                let (a, _) = tmp[n].split_at_mut(2);
                let _ = a;
                let mut out = vec![0i16; MAX_FRAME];
                n_dec = self.ch[n].decode_frame(dec, &mut out, lost, cond);
                tmp[n][2..2 + n_dec].copy_from_slice(&out[..n_dec]);
            } else {
                for v in tmp[n][2..2 + n_dec].iter_mut() {
                    *v = 0;
                }
            }
            self.ch[n].frames_decoded += 1;
        }
        if ctl.channels_api == 2 && n_int == 2 {
            let (a, b) = tmp.split_at_mut(1);
            let fs = self.ch[0].fs_khz;
            stereo_ms_to_lr(&mut self.stereo, &mut a[0], &mut b[0], ms_pred, fs, n_dec);
        } else {
            let save = [tmp[0][n_dec], tmp[0][n_dec + 1]];
            tmp[0][0] = self.stereo.s_mid[0];
            tmp[0][1] = self.stereo.s_mid[1];
            self.stereo.s_mid = save;
        }
        let mut outs: Vec<Vec<i16>> = Vec::new();
        for n in 0..ctl.channels_api.min(n_int) {
            let r = self.ch[n].resampler.as_mut().unwrap();
            outs.push(r.process(&tmp[n][1..1 + n_dec]));
        }
        let n_out = outs[0].len();
        let mut pcm = vec![0i16; n_out * ctl.channels_api];
        if ctl.channels_api == 2 {
            for (n, o) in outs.iter().enumerate() {
                for i in 0..n_out {
                    pcm[2 * i + n] = o[i];
                }
            }
            if n_int == 1 {
                if stereo_to_mono {
                    let r = self.ch[1].resampler.as_mut().unwrap();
                    let o = r.process(&tmp[0][1..1 + n_dec]);
                    for i in 0..n_out {
                        pcm[2 * i + 1] = o[i];
                    }
                } else {
                    for i in 0..n_out {
                        pcm[2 * i + 1] = pcm[2 * i];
                    }
                }
            }
        } else {
            pcm.copy_from_slice(&outs[0]);
        }
        self.prev_pitch_lag = if self.ch[0].prev_signal_type == TYPE_VOICED { self.ch[0].lag_prev * (48 / self.ch[0].fs_khz) } else { 0 };
        if lost {
            for c in self.ch.iter_mut().take(n_int) {
                c.last_gain_index = 10;
            }
        } else {
            self.prev_decode_only_middle = decode_only_middle;
        }
        pcm
    }

    /// Frames of the current packet already decoded.
    pub fn frames_decoded(&self) -> usize {
        self.ch[0].frames_decoded
    }

    /// Internal frames in the packet (1 to 3) as set by the last first call.
    pub fn frames_per_packet(&self) -> usize {
        self.ch[0].frames_per_packet
    }
}

impl SilkDecoder {
    /// Decodes one whole Opus frame of SILK (10, 20, 40 or 60 ms), which is one to three SILK frames, from a range decoder at its start.
    /// Returns interleaved 16-bit sound at 48 kHz and `channels_api` channels. `frame_samples` is the Opus frame length at 48 kHz.
    pub fn decode_opus_frame(&mut self, dec: &mut RangeDecoder<'_>, internal_rate: i32, channels_internal: usize, channels_api: usize, frame_samples: usize, lost: bool) -> Vec<i16> {
        let payload_ms = ((frame_samples * 1000 / 48000) as i32).max(10);
        let ctl = SilkControl { channels_api, channels_internal, internal_rate, payload_ms };
        let mut out: Vec<i16> = Vec::with_capacity(frame_samples * channels_api);
        let mut done = 0usize;
        while done < frame_samples {
            let pcm = self.decode(ctl, lost, done == 0, dec);
            let n = pcm.len() / channels_api;
            if n == 0 {
                break;
            }
            done += n;
            out.extend(pcm);
        }
        out
    }
}

/// Decodes the packets of an Ogg Opus stream that are all SILK-only, to 48 kHz floats at full scale one (a test and measuring helper;
/// the decoder that handles every mode is in `opus_dec.rs`).
pub fn decode_silk_packets(head: &crate::opus::Head, packets: &[Vec<u8>]) -> Result<Vec<f32>, crate::opus::OpusError> {
    use crate::opus::{parse_packet, Bandwidth, Mode, OpusError};
    let mut dec = SilkDecoder::new();
    let mut out = Vec::new();
    for p in packets {
        let pk = parse_packet(p)?;
        if pk.toc.mode != Mode::Silk {
            return Err(OpusError::NotDecodedYet("hybrid or CELT in a SILK-only helper"));
        }
        let rate = match pk.toc.bandwidth {
            Bandwidth::Narrow => 8000,
            Bandwidth::Medium => 12000,
            _ => 16000,
        };
        for f in &pk.frames {
            let mut rd = RangeDecoder::new(f);
            let pcm = dec.decode_opus_frame(&mut rd, rate, if pk.toc.stereo { 2 } else { 1 }, head.channels as usize, pk.toc.frame_samples(), false);
            out.extend(pcm.iter().map(|&v| v as f32 / 32768.0));
        }
    }
    let skip = head.pre_skip as usize * head.channels as usize;
    Ok(out.split_off(skip.min(out.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (crate::opus::Head, Vec<Vec<u8>>, Vec<i16>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus");
        let bytes = std::fs::read(dir.join(format!("{name}.opus"))).unwrap();
        let raw = std::fs::read(dir.join(format!("{name}.ref.s16"))).unwrap();
        let pcm: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let mut d = crate::ogg::Demuxer::new();
        let mut packets: Vec<Vec<u8>> = d.push(&bytes).into_iter().flat_map(|p| p.packets).collect();
        let head = crate::opus::parse_head(&packets[0]).unwrap();
        let audio = packets.split_off(2);
        (head, audio, pcm)
    }

    fn compare(got: &[f32], want: &[i16]) -> (f64, f64) {
        let n = got.len().min(want.len());
        let (mut s, mut e, mut gg, mut gw) = (0f64, 0f64, 0f64, 0f64);
        for i in 0..n {
            let (g, w) = (got[i] as f64 * 32768.0, want[i] as f64);
            s += w * w;
            e += (g - w) * (g - w);
            gg += g * g;
            gw += g * w;
        }
        (10.0 * (s / e.max(1e-9)).log10(), gw / (gg * s).sqrt().max(1e-9))
    }

    #[test]
    fn silk_fixtures_match_the_reference_decode() {
        for name in ["silk-mono-wb-20ms", "silk-mono-nb-20ms", "silk-mono-mb-20ms", "silk-mono-wb-10ms", "silk-mono-wb-60ms", "silk-stereo-wb-20ms"] {
            let (head, audio, want) = fixture(name);
            let got = decode_silk_packets(&head, &audio).unwrap();
            let (snr, corr) = compare(&got, &want);
            eprintln!("{name}: {} samples, want {}, snr {snr:.1} dB, corr {corr:.5}", got.len(), want.len());
            assert!(got.len() >= want.len(), "{name}: too few samples");
            assert!(snr > 60.0 && corr > 0.9999, "{name}: snr {snr:.1} corr {corr:.5}");
        }
    }
}

