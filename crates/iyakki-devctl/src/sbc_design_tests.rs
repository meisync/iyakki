use super::*;
use crate::sbc_tables::{D4_DESIGNED, D8_DESIGNED};

fn table(m: usize) -> &'static [f64] {
    if m == 4 { &D4_DESIGNED } else { &D8_DESIGNED }
}

/// The prototype implied by a table: undo the block signs, scale so that the pass-band amplitude A(0) is 1.
fn proto_of(m: usize, d: &[f64]) -> Vec<f64> {
    let h: Vec<f64> = d.iter().enumerate().map(|(n, v)| if (n / (2 * m)) % 2 == 0 { -v } else { *v }).collect();
    let s: f64 = h.iter().sum();
    h.iter().map(|v| v / s).collect()
}

fn amp(h: &[f64], w: f64) -> f64 {
    let c = h.len() as f64 / 2.0;
    h.iter().enumerate().map(|(n, v)| v * ((n as f64 - c) * w).cos()).sum()
}

#[test]
fn the_tables_are_the_output_of_the_documented_procedure() {
    for m in [4usize, 8] {
        let w = designed_window(m);
        assert_eq!(w.len(), table(m).len());
        let worst = w.iter().zip(table(m)).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        eprintln!("M={m}: largest difference between the recomputed and the stored table {worst:.2e}");
        assert!(worst < 1e-7, "M={m}: table differs from the procedure by {worst:e}; regenerate with print_designed_tables and update docs/development/SBC-TABLES.md");
    }
}

#[test]
fn the_window_is_symmetric_with_the_block_signs_and_peaks_in_the_middle() {
    for m in [4usize, 8] {
        let d = table(m);
        let n = 10 * m;
        assert_eq!(d[0], 0.0);
        for i in 1..n {
            let mirror = d[n - i];
            // D[n-i] = D[i], negated when i is a multiple of 2 m (the block sign flips there)
            let want = if i % (2 * m) == 0 { -d[i] } else { d[i] };
            assert!((mirror - want).abs() < 1e-12, "M={m} i={i}");
        }
        let peak = (0..n).max_by(|&a, &b| d[a].abs().total_cmp(&d[b].abs())).unwrap();
        assert_eq!(peak, n / 2);
        // the prototype is a lowpass: one main lobe, so the magnitude rises to the middle
        let h = proto_of(m, d);
        assert!(h.iter().all(|v| v.is_finite()));
        assert!(h[n / 2] > 0.0 && h[n / 2] == h.iter().cloned().fold(f64::MIN, f64::max));
    }
}

#[test]
fn the_prototype_has_unit_energy_scale_and_power_complementary_bands() {
    for m in [4usize, 8] {
        let h = proto_of(m, table(m));
        // energy: for a power-complementary lowpass of amplitude 1 at DC, sum h^2 = 1 / (2 m) up to the transition-band error
        let e: f64 = h.iter().map(|v| v * v).sum::<f64>() * 2.0 * m as f64;
        let band = std::f64::consts::PI / m as f64;
        let mut pr = 0.0f64;
        let mut stop = 0.0f64;
        let mut pass_dev = 0.0f64;
        for i in 0..2000 {
            let w = band * (i as f64 + 0.5) / 2000.0;
            pr = pr.max((amp(&h, w).powi(2) + amp(&h, band - w).powi(2) - 1.0).abs());
            if w < 0.15 * band {
                pass_dev = pass_dev.max((amp(&h, w) - 1.0).abs());
            }
        }
        for i in 0..4000 {
            let w = 1.1 * band + (std::f64::consts::PI - 1.1 * band) * (i as f64 + 0.5) / 4000.0;
            stop = stop.max(amp(&h, w).abs());
        }
        let stop_db = 20.0 * stop.log10();
        eprintln!("M={m}: energy x2M {e:.4}, power-complementarity error {pr:.2e}, stop band {stop_db:.1} dB, pass band deviation {pass_dev:.2e}");
        assert!((e - 1.0).abs() < 0.005, "M={m} energy {e}");
        assert!(pr < 0.002, "M={m} power complementarity {pr}");
        assert!(stop_db < -55.0, "M={m} stop band {stop_db} dB");
        assert!(pass_dev < 0.005, "M={m} pass band {pass_dev}");
    }
}

#[test]
fn the_filter_bank_reconstructs_with_delay_9_bands_plus_1_gain_one_and_a_small_error() {
    for m in [4usize, 8] {
        let r = measure_loop(m, table(m));
        eprintln!("M={m}: {r:?}");
        // delay: 9 m + 1 samples (the analysis and synthesis halves of the symmetric 10 m tap prototype, less the zero first tap)
        assert_eq!(r.delay, 9 * m + 1);
        assert!((r.gain - 1.0).abs() < 1e-4, "gain {}", r.gain);
        assert!(r.error_db < -62.0, "M={m}: reconstruction error {} dB", r.error_db);
    }
}

#[test]
fn the_design_is_deterministic() {
    assert_eq!(prototype(4), prototype(4));
}

/// A phone's encoder uses the specification's analysis window and our decoder the designed synthesis window (and the other way round): the
/// unquantised mismatch is measured (about -37 dB for 4 bands; see docs/development/SBC-TABLES.md) and must stay below -30 dB, under the noise of a typical bitpool. The legacy window, which matched a real decoder
/// closely, stands in for "a window that behaves like the specification's" here; this test only exists while the legacy window does.
#[cfg(feature = "legacy-sbc-window")]
#[test]
fn designed_and_legacy_windows_interoperate_within_the_quantisation_noise() {
    for m in [4usize, 8] {
        let legacy: &[f64] = if m == 4 { &crate::sbc_tables_legacy::D4_LEGACY } else { &crate::sbc_tables_legacy::D8_LEGACY };
        let des = table(m);
        let a = measure_pair(m, legacy, des);
        let b = measure_pair(m, des, legacy);
        eprintln!("M={m}: legacy encoder -> designed decoder {a:?}; designed encoder -> legacy decoder {b:?}");
        for r in [a, b] {
            assert_eq!(r.delay, 9 * m + 1);
            assert!(r.error_db < -30.0, "M={m}: {r:?}");
        }
    }
}

/// Release guard: a release build (IYAKKI_RELEASE_BUILD=1 in the environment of the test run) must not use the legacy window.
#[test]
fn a_release_build_does_not_use_the_legacy_window() {
    if std::env::var("IYAKKI_RELEASE_BUILD").is_ok_and(|v| v == "1") {
        assert!(crate::sbc_tables::WINDOW_SOURCE.starts_with("designed"), "legacy SBC window selected in a release build: build with --no-default-features");
    }
}
