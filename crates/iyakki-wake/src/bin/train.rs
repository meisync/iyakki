//! iyakki-wake-train --data DIR --out FILE [--epochs N] [--examples N] [--batch N] [--lr F] [--seed N] [--threads N]
//! Reads DIR/positive, DIR/negative, DIR/background (WAV), trains, writes the weights file (versioned header, checksum).
#[path = "args.rs"]
mod args;
use iyakki_wake::{dataset::DataSet, train, weights};
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let m = args::parse(&a);
    let (Some(data), Some(out)) = (m.get("data"), m.get("out")) else {
        eprintln!("usage: iyakki-wake-train --data DIR --out FILE [--epochs N] [--examples N] [--batch N] [--lr F] [--seed N] [--threads N]");
        std::process::exit(2);
    };
    let (data, out) = (PathBuf::from(data), PathBuf::from(out));
    for p in [&data, &out] {
        if let Err(e) = iyakki_wake::refuse_tmp(p) {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
    let num = |k: &str, d: f64| m.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
    let cfg = train::Config {
        epochs: num("epochs", 20.0) as usize,
        examples_per_epoch: num("examples", 4000.0) as usize,
        batch: num("batch", 32.0) as usize,
        lr: num("lr", 3e-3) as f32,
        seed: num("seed", 1.0) as u64,
        threads: num("threads", 4.0) as usize,
        val_examples: 600,
    };
    let ds = match DataSet::load(&data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read the data: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = ds.check_trainable() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    let (tr, va) = if ds.positives.len() >= 5 { ds.split(5) } else { (ds.clone(), ds.clone()) };
    let real = ds.any_real();
    println!(
        "data: {} positives, {} negatives, {} backgrounds ({} real recordings included: {}); validation holds out whole speakers: {} positives, {} negatives",
        ds.positives.len(), ds.negatives.len(), ds.backgrounds.len(), "any", real, va.positives.len(), va.negatives.len()
    );
    if !real {
        println!("NOTE: every clip is synthetic. The model will not have heard a real voice.");
    }
    let started = std::time::Instant::now();
    let (net, reports) = train::train(&tr, &va, &cfg, &mut |r| {
        println!(
            "epoch {:>3}  train loss {:.4}  val loss {:.4}  val acc {:.3}  recall@0.5 {:.3}  false@0.5 {:.3}",
            r.epoch, r.train_loss, r.val_loss, r.val_accuracy, r.val_recall_at_half, r.val_false_rate_at_half
        );
    });
    let val = train::make_validation(&va, cfg.val_examples, cfg.seed);
    let thr = train::suggest_threshold(&net, &val);
    let header = weights::Header {
        data_kind: if real { weights::DATA_REAL_INCLUDED } else { weights::DATA_SYNTHETIC },
        threshold: thr,
        seed: cfg.seed,
        epochs: reports.len() as u32,
        notes: format!("{} positives, {} negatives, {} backgrounds", ds.positives.len(), ds.negatives.len(), ds.backgrounds.len()),
    };
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&out, weights::save_bytes(&net, &header)) {
        eprintln!("cannot write {}: {e}", out.display());
        std::process::exit(1);
    }
    println!("wrote {} ({} bytes), suggested threshold {:.2}, {:.0} s", out.display(), std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0), thr, started.elapsed().as_secs_f32());
}
