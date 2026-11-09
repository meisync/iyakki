//! iyakki-wake-gen --out DIR [--voices N] [--seed N] [--tts-command "TEMPLATE"]
//! Writes synthetic training data (positive/, negative/, background/). Without --tts-command it uses the built-in formant stub, which is NOT
//! real speech. A template may hold {text} {out} {voice} {speed}; the tool must write a WAV to {out}.
#[path = "args.rs"]
mod args;
use iyakki_wake::gen::{generate, GenConfig};
use iyakki_wake::synth::{CommandTts, FormantTts, Tts};
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let m = args::parse(&a);
    let Some(out) = m.get("out") else {
        eprintln!("usage: iyakki-wake-gen --out DIR [--voices N] [--seed N] [--tts-command \"TEMPLATE\"]");
        std::process::exit(2);
    };
    let out = PathBuf::from(out);
    if let Err(e) = iyakki_wake::refuse_tmp(&out) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    let mut cfg = GenConfig::default();
    if let Some(v) = m.get("voices").and_then(|v| v.parse().ok()) {
        cfg.voices = v;
    }
    if let Some(v) = m.get("seed").and_then(|v| v.parse().ok()) {
        cfg.seed = v;
    }
    let tts: Box<dyn Tts> = match m.get("tts-command") {
        Some(t) if !t.is_empty() => Box::new(CommandTts { template: t.clone(), work_dir: out.clone(), voice_count: cfg.voices }),
        _ => Box::new(FormantTts { seed: cfg.seed }),
    };
    match generate(tts.as_ref(), &out, &cfg) {
        Ok(r) => {
            println!("{r:?}");
            if !r.real_speech {
                println!("NOTE: made with the formant stub. This is NOT real speech; a model trained on it has not heard a real voice.");
            }
        }
        Err(e) => {
            eprintln!("generation failed: {e}");
            std::process::exit(1);
        }
    }
}
