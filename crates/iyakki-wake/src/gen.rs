//! The data generator: synthesises the phrase in several voices and speeds, hard negatives (sounds-alike phrases), general speech,
//! TV-like speech with music, and background noise, into `positive/`, `negative/` and `background/` folders of 16-bit WAV files.
//! File names: `syn_v<voice>_s<speed percent>_<n>.wav` (the voice is the group that validation holds out).
//!
//! These are SYNTHETIC files made on the dev PC for training. With `FormantTts` they are not real speech (see `synth.rs`); with an
//! external engine (`CommandTts`) they are as real as that engine is.

use crate::augment::{self, rms};
use crate::features::SAMPLE_RATE;
use crate::rng::Rng;
use crate::synth::Tts;
use std::path::Path;

pub const PHRASE: &str = "my iyakki";

/// Phrases that sound a bit like the wake phrase and must not wake it.
pub const HARD_NEGATIVES: &[&str] = &[
    "my yaki", "hey ikki", "my ikki", "hi yaki", "my taxi", "my lucky", "my sticky", "mighty icky", "my kiki", "mickey", "iyakki",
    "my hockey", "hey kiki", "my yakitori", "I like it", "my key", "hi iyakki", "yaki iyakki", "my icky", "by yaki",
];

pub const SENTENCES: &[&str] = &[
    "the weather today is cold and the news is long",
    "turn on the lights in the kitchen",
    "play some music tonight",
    "what is the time",
    "they said that one of the people could come home",
    "she will make water for the first time",
    "open the door and see what is down there",
    "we can find many other ways to write the number",
    "his day was long and the oil was cold",
    "how many people are about to come",
    "call him when you can and look at the part",
    "and then they go up to the home",
    "it was the first day of the long weather",
    "when will you make the music",
    "there is no way he could find them",
    "this is what you said about the news",
    "some of these people have been to the water",
    "go down the road and turn at the first light",
    "I like the way you play",
    "has she had the time to see it",
];

#[derive(Clone, Debug)]
pub struct GenConfig {
    pub voices: usize,
    pub speeds: Vec<f32>,
    pub seed: u64,
    /// Seconds of each synthetic background file, and how many.
    pub background_secs: usize,
    pub background_files: usize,
}

impl Default for GenConfig {
    fn default() -> Self {
        GenConfig { voices: 16, speeds: vec![0.85, 0.95, 1.05, 1.2], seed: 1, background_secs: 20, background_files: 12 }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenReport {
    pub positives: usize,
    pub hard_negatives: usize,
    pub speech: usize,
    pub tv: usize,
    pub backgrounds: usize,
    pub real_speech: bool,
    pub engine: String,
}

fn write(dir: &Path, name: &str, x: &[f32]) -> Result<(), String> {
    std::fs::write(dir.join(name), crate::wav::encode(x)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
}

/// A little music-like bed: a few sustained notes with slow tremolo.
fn music(len: usize, rng: &mut Rng) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    for _ in 0..3 {
        let f = rng.range(150.0, 900.0);
        let tr = rng.range(0.3, 3.0);
        for (i, v) in out.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE as f32;
            *v += (2.0 * std::f32::consts::PI * f * t).sin() * (0.6 + 0.4 * (2.0 * std::f32::consts::PI * tr * t).sin()) * 0.2;
        }
    }
    out
}

/// Write the folders. Returns what was made.
pub fn generate(tts: &dyn Tts, out: &Path, cfg: &GenConfig) -> Result<GenReport, String> {
    for d in ["positive", "negative", "background"] {
        std::fs::create_dir_all(out.join(d)).map_err(|e| e.to_string())?;
    }
    let mut rng = Rng::new(cfg.seed);
    let mut rep = GenReport { real_speech: tts.is_real_speech(), engine: tts.name(), ..Default::default() };
    let nv = cfg.voices.min(tts.voices()).max(1);
    for v in 0..nv {
        for (k, sp) in cfg.speeds.iter().enumerate() {
            let x = tts.synth(PHRASE, v, *sp)?;
            write(&out.join("positive"), &format!("syn_v{v:02}_s{:03}_{k}.wav", (sp * 100.0) as u32), &x)?;
            rep.positives += 1;
        }
    }
    for v in 0..nv {
        for (i, p) in HARD_NEGATIVES.iter().enumerate() {
            let sp = cfg.speeds[(v + i) % cfg.speeds.len()];
            let x = tts.synth(p, v, sp)?;
            write(&out.join("negative"), &format!("syn_v{v:02}_hard{i:02}.wav"), &x)?;
            rep.hard_negatives += 1;
        }
        for k in 0..4 {
            let s = SENTENCES[rng.below(SENTENCES.len())];
            let sp = cfg.speeds[rng.below(cfg.speeds.len())];
            let x = tts.synth(s, v, sp)?;
            write(&out.join("negative"), &format!("syn_v{v:02}_talk{k}.wav"), &x)?;
            rep.speech += 1;
        }
    }
    // TV-like: two or three voices one after the other with music underneath
    for k in 0..(nv * 2) {
        let mut x = Vec::new();
        for _ in 0..(2 + rng.below(2)) {
            let s = SENTENCES[rng.below(SENTENCES.len())];
            x.extend(tts.synth(s, rng.below(nv), rng.range(0.9, 1.2))?);
        }
        let mut m = music(x.len(), &mut rng);
        let g = rms(&x) / rms(&m).max(1e-6) * 10f32.powf(-rng.range(6.0, 16.0) / 20.0);
        m.iter_mut().for_each(|v| *v *= g);
        x.iter_mut().zip(&m).for_each(|(a, b)| *a += b);
        write(&out.join("negative"), &format!("syn_tv{k:02}_mix.wav"), &x)?;
        rep.tv += 1;
    }
    for k in 0..cfg.background_files {
        let len = cfg.background_secs * SAMPLE_RATE;
        let mut x = if k % 5 == 4 {
            // a murmur of several voices talking over each other, at low level
            let mut b = vec![0.0f32; len];
            for j in 0..3 {
                let mut at = 0;
                while at < len {
                    let s = tts.synth(SENTENCES[rng.below(SENTENCES.len())], (k + j) % nv, rng.range(0.9, 1.2))?;
                    for (i, v) in s.iter().enumerate() {
                        if at + i < len {
                            b[at + i] += v * 0.3;
                        }
                    }
                    at += s.len() + rng.below(8000);
                }
            }
            b
        } else {
            augment::synthetic_noise(k, len, &mut rng)
        };
        augment::gain(&mut x, rng.range(-12.0, 0.0));
        write(&out.join("background"), &format!("syn_bg{k:02}.wav"), &x)?;
        rep.backgrounds += 1;
    }
    let note = format!(
        "Generated with: {}\nReal speech engine: {}\n{}\n",
        tts.name(),
        tts.is_real_speech(),
        if tts.is_real_speech() { "" } else { "SYNTHETIC ONLY (formant stub): a model trained on this has NOT heard a real voice.\n" }
    );
    std::fs::write(out.join("GENERATED.txt"), note).map_err(|e| e.to_string())?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::FormantTts;

    #[test]
    fn the_generator_makes_the_folders_with_the_naming_scheme_and_says_it_is_synthetic() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wake-test-scratch").join(format!("gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = GenConfig { voices: 2, speeds: vec![1.0, 1.2], seed: 2, background_secs: 2, background_files: 5 };
        let rep = generate(&FormantTts { seed: 1 }, &dir, &cfg).unwrap();
        assert_eq!((rep.positives, rep.hard_negatives, rep.speech, rep.tv, rep.backgrounds), (4, 40, 8, 4, 5));
        assert!(!rep.real_speech);
        let ds = crate::dataset::DataSet::load(&dir).unwrap();
        assert_eq!(ds.positives.len(), 4);
        assert!(ds.positives.iter().all(|c| c.name.starts_with("syn_v") && !c.real && c.samples.len() > 8000));
        assert_eq!(ds.negatives.len(), 52);
        assert_eq!(ds.backgrounds.len(), 5);
        assert!(!ds.any_real());
        let note = std::fs::read_to_string(dir.join("GENERATED.txt")).unwrap();
        assert!(note.contains("NOT heard a real voice"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_hard_negatives_really_differ_from_the_phrase() {
        let p = crate::synth::to_phonemes(PHRASE);
        for h in HARD_NEGATIVES {
            assert_ne!(crate::synth::to_phonemes(h), p, "{h} must not be the phrase");
        }
    }
}
