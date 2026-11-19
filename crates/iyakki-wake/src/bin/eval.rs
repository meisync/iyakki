//! iyakki-wake-eval --weights FILE (--data DIR | --positives DIR --background DIR) [--snr DB] [--all]
//! iyakki-wake-eval --weights FILE --bench
//! Prints the miss rate and the false accepts per hour for a threshold sweep, or the CPU cost of the streaming spotter.


use iyakki_wake::dataset::DataSet;
use iyakki_wake::eval::sweep;
use iyakki_wake::features::SAMPLE_RATE;
use iyakki_wake::spotter::{Scorer, Settings, Spotter, WakeSpotter};
use iyakki_wake::weights;
use std::path::{Path, PathBuf};

fn load_wavs(dir: &Path) -> Vec<Vec<f32>> {
    DataSet::load_plain(dir).unwrap_or_default()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let m = iyakki_wake::cli::parse(&a);
    let Some(w) = m.get("weights") else {
        eprintln!("usage: iyakki-wake-eval --weights FILE (--data DIR | --positives DIR --background DIR) [--snr DB] [--all] | --bench");
        std::process::exit(2);
    };
    let bytes = std::fs::read(w).unwrap_or_else(|e| {
        eprintln!("cannot read {w}: {e}");
        std::process::exit(1)
    });
    let (net, header) = weights::load_bytes(&bytes).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    println!("model: {}", iyakki_wake::describe_weights(&header));
    if m.contains_key("bench") {
        bench(net);
        return;
    }
    let snr: f32 = m.get("snr").and_then(|v| v.parse().ok()).unwrap_or(15.0);
    let (mut positives, mut background, mut synthetic) = (Vec::new(), Vec::new(), header.data_kind == weights::DATA_SYNTHETIC);
    if let Some(d) = m.get("data") {
        let ds = DataSet::load(&PathBuf::from(d)).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
        let ds = if m.contains_key("all") || ds.positives.len() < 5 { ds.clone() } else { ds.split(5).1 };
        synthetic = synthetic && !ds.any_real();
        positives = ds.positives.iter().map(|c| c.samples.clone()).collect();
        background = ds.backgrounds.iter().map(|c| c.samples.clone()).collect();
        // held-out speech and TV as one long stream with half-second gaps
        let mut stream = Vec::new();
        for c in &ds.negatives {
            stream.extend_from_slice(&c.samples);
            stream.extend(std::iter::repeat(0.0).take(SAMPLE_RATE / 2));
        }
        if !stream.is_empty() {
            background.push(stream);
        }
    }
    if let Some(p) = m.get("positives") {
        positives = load_wavs(&PathBuf::from(p));
        synthetic = false;
    }
    if let Some(b) = m.get("background") {
        background = load_wavs(&PathBuf::from(b));
        synthetic = false;
    }
    let thresholds: Vec<f32> = (10..=19).map(|i| i as f32 * 0.05).chain([0.97, 0.99]).collect();
    let rep = sweep(&net, &positives, &background, snr, &thresholds, 2500, synthetic);
    print!("{}", rep.table());
    println!("(positives are mixed with room sound at {snr} dB SNR; the cool-down is 2.5 s)");
}

fn bench(net: iyakki_wake::model::Net) {
    use iyakki_wake::rng::Rng;
    let mut rng = Rng::new(5);
    let secs = 60;
    let mk = |level: f32, rng: &mut Rng| -> Vec<f32> { (0..secs * SAMPLE_RATE).map(|i| { let on = if (i / 8000) % 2 == 0 { 1.0 } else { 0.01 }; level * on * (rng.normal() * 0.5 + (i as f32 * 0.07).sin() + (i as f32 * 0.31).sin()) }).collect() };
    let quiet = mk(0.0003, &mut rng);
    let loud = mk(0.15, &mut rng);
    let run = |x: &[f32]| {
        let mut sp = Spotter::new(net.clone(), Settings { threshold: 2.0, cooldown_ms: 0 });
        sp.arm();
        let t = std::time::Instant::now();
        for c in x.chunks(1600) {
            sp.feed(c);
        }
        let e = t.elapsed().as_secs_f64();
        (e / secs as f64 * 100.0, sp.stats())
    };
    let (q, qs) = run(&quiet);
    let (l, ls) = run(&loud);
    println!("CPU, one core, {secs} s of audio each (wall time of the feed loop on this PC, build profile of this binary):");
    println!("  quiet room (gate shut):        {q:.3} %   ({} windows scored)", qs.windows_scored);
    println!("  loud all the time (gate open): {l:.3} %   ({} windows scored)  <- worst case", ls.windows_scored);
    let _ = Scorer::new(net.clone());
}
