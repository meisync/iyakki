//! Training data: a folder with `positive/`, `negative/` and `background/` of WAV files, and the making of augmented examples.
//!
//! File names: `real_<speaker>_<nn>.wav` marks a real recording (the weights header then says real voices were included); anything else
//! is treated as synthetic. Positives are the phrase said once, negatives are anything else (similar phrases, speech, TV), backgrounds are
//! long recordings of the room (noise, fan, TV, music) that are mixed in and also cropped as negatives.

use crate::augment::{self, Recipe};
use crate::features::{window_of, LogMel, WINDOW_SAMPLES};
use crate::rng::Rng;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub samples: Vec<f32>,
    pub real: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DataSet {
    pub positives: Vec<Clip>,
    pub negatives: Vec<Clip>,
    pub backgrounds: Vec<Clip>,
}

fn load_folder(dir: &Path, trim: bool) -> Result<Vec<Clip>, String> {
    let mut names: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))).collect(),
        Err(_) => return Ok(Vec::new()),
    };
    names.sort();
    let mut out = Vec::new();
    for p in names {
        let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut s = crate::wav::decode(&bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        if trim {
            s = augment::trim_silence(&s, -40.0, 800);
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        out.push(Clip { real: name.starts_with("real_"), name, samples: s });
    }
    Ok(out)
}

impl DataSet {
    pub fn load(dir: &Path) -> Result<DataSet, String> {
        Ok(DataSet {
            positives: load_folder(&dir.join("positive"), true)?,
            negatives: load_folder(&dir.join("negative"), false)?,
            backgrounds: load_folder(&dir.join("background"), false)?,
        })
    }

    /// Every WAV in a folder as plain samples (for the evaluation tool).
    pub fn load_plain(dir: &Path) -> Result<Vec<Vec<f32>>, String> {
        Ok(load_folder(dir, false)?.into_iter().map(|c| c.samples).collect())
    }

    pub fn any_real(&self) -> bool {
        self.positives.iter().chain(&self.negatives).any(|c| c.real)
    }

    /// Hold out whole speakers: the group of a file is the second part of its name (`syn_v03_...` is voice `v03`, `real_amy_01` is `amy`).
    /// Every `every`th group (in sorted order) goes to validation, so validation voices are never trained on. With fewer than three
    /// groups, every `every`th file goes instead.
    pub fn split(&self, every: usize) -> (DataSet, DataSet) {
        fn key(name: &str) -> String {
            let stem = name.trim_end_matches(".wav");
            stem.split('_').nth(1).filter(|_| stem.contains('_')).unwrap_or(stem).to_string()
        }
        let mut groups: Vec<String> = self.positives.iter().chain(&self.negatives).map(|c| key(&c.name)).collect();
        groups.sort();
        groups.dedup();
        let by_group = groups.len() >= 3;
        let held = |g: &str| groups.iter().position(|x| x == g).is_some_and(|i| i % every == every - 1);
        let pick = |v: &Vec<Clip>, val: bool| -> Vec<Clip> {
            v.iter().enumerate().filter(|(i, c)| (if by_group { held(&key(&c.name)) } else { i % every == every - 1 }) == val).map(|(_, c)| c.clone()).collect()
        };
        // background recordings: every `every`th file by index
        let bg = |val: bool| -> Vec<Clip> { self.backgrounds.iter().enumerate().filter(|(i, _)| (i % every == every - 1) == val).map(|(_, c)| c.clone()).collect() };
        let tr = DataSet { positives: pick(&self.positives, false), negatives: pick(&self.negatives, false), backgrounds: bg(false) };
        let va = DataSet { positives: pick(&self.positives, true), negatives: pick(&self.negatives, true), backgrounds: bg(true) };
        (tr, va)
    }

    pub fn check_trainable(&self) -> Result<(), String> {
        if self.positives.is_empty() {
            return Err("no positive clips (put WAV files of the phrase in positive/)".into());
        }
        if self.negatives.is_empty() && self.backgrounds.is_empty() {
            return Err("no negative or background clips".into());
        }
        Ok(())
    }
}

/// What an example is made from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Phrase,
    /// A cut-off piece of the phrase (label: not the phrase).
    Fragment,
    Negative,
    Background,
}

fn noise_for(ds: &DataSet, rng: &mut Rng) -> Vec<f32> {
    if !ds.backgrounds.is_empty() && rng.chance(0.75) {
        ds.backgrounds[rng.below(ds.backgrounds.len())].samples.clone()
    } else {
        augment::synthetic_noise(rng.below(4), 16_000 * 4, rng)
    }
}

fn degrade(clip: &[f32], r: &Recipe, rng: &mut Rng) -> Vec<f32> {
    let mut x = augment::speed(clip, r.speed);
    if let Some(rt) = r.rt60 {
        let h = augment::synthetic_rir(rt, rng);
        let before = augment::rms(&x);
        x = augment::convolve(&x, &h);
        let after = augment::rms(&x);
        if after > 1e-9 {
            x.iter_mut().for_each(|v| *v *= before / after);
        }
    }
    augment::gain(&mut x, r.gain_db);
    x
}

/// One augmented model window (audio) and its label (1 = the phrase is wholly inside).
pub fn make_audio(ds: &DataSet, src: Source, rng: &mut Rng) -> (Vec<f32>, usize) {
    let r = Recipe::random(rng);
    let (mut audio, label) = match src {
        Source::Phrase | Source::Fragment => {
            let c = &ds.positives[rng.below(ds.positives.len())].samples;
            let mut clip = degrade(c, &r, rng);
            let mut label = 1;
            if src == Source::Fragment {
                let keep = rng.range(0.3, 0.65);
                let n = ((clip.len() as f32 * keep) as usize).max(800).min(clip.len());
                clip = if rng.chance(0.5) { clip[..n].to_vec() } else { clip[clip.len() - n..].to_vec() };
                label = 0;
            } else if !ds.negatives.is_empty() && rng.chance(0.25) {
                // some speech just before the phrase, as in a room
                let n = &ds.negatives[rng.below(ds.negatives.len())].samples;
                let room = WINDOW_SAMPLES.saturating_sub(clip.len() + 800);
                let take = n.len().min(room).min(rng.below(room.max(1)) + 1);
                if take > 1000 {
                    let mut pre = degrade(&n[..take], &r, rng);
                    pre.extend(std::iter::repeat(0.0).take(rng.below(1500)));
                    pre.extend_from_slice(&clip);
                    clip = pre;
                }
            }
            (augment::place_in_window(&clip, rng).0, label)
        }
        Source::Negative if !ds.negatives.is_empty() => {
            let c = &ds.negatives[rng.below(ds.negatives.len())].samples;
            let mut clip = degrade(c, &r, rng);
            if rng.chance(0.3) {
                let c2 = &ds.negatives[rng.below(ds.negatives.len())].samples;
                clip.extend(std::iter::repeat(0.0).take(rng.below(3000)));
                clip.extend(degrade(c2, &r, rng));
            }
            (augment::place_in_window(&clip, rng).0, 0)
        }
        _ => (vec![0.0; WINDOW_SAMPLES], 0),
    };
    let noise = noise_for(ds, rng);
    let signal_level = augment::rms(&audio);
    if signal_level < 1e-6 {
        // nothing but room: the background itself at a random level
        let n = noise_for(ds, rng);
        let start = rng.below(n.len().max(1));
        audio = (0..WINDOW_SAMPLES).map(|i| n[(start + i) % n.len().max(1)]).collect();
        augment::gain(&mut audio, rng.range(-30.0, 0.0));
    } else {
        augment::add_noise(&mut audio, &noise, r.snr_db, rng);
    }
    for v in audio.iter_mut() {
        *v += 1e-4 * rng.normal();
    }
    (audio, label)
}

/// An example as normalised features.
pub fn make_example(ds: &DataSet, lm: &LogMel, src: Source, rng: &mut Rng) -> (Vec<f32>, usize) {
    let (a, l) = make_audio(ds, src, rng);
    (window_of(lm, &a), l)
}

/// The mix of sources in a training epoch: a third phrases, the rest hard to easy negatives.
pub fn pick_source(ds: &DataSet, rng: &mut Rng) -> Source {
    let p = rng.f32();
    if p < 0.34 {
        Source::Phrase
    } else if p < 0.42 {
        Source::Fragment
    } else if p < 0.82 && !ds.negatives.is_empty() {
        Source::Negative
    } else {
        Source::Background
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::N_MEL;

    pub fn tiny() -> DataSet {
        let mut r = Rng::new(1);
        let mk = |n: &str, f: f32, len: usize| Clip {
            name: n.into(),
            samples: (0..len).map(|i| 0.4 * (i as f32 * f).sin() * (1.0 - (i as f32 / len as f32 - 0.5).abs() * 1.6)).collect(),
            real: false,
        };
        let _ = &mut r;
        DataSet {
            positives: vec![mk("syn_a", 0.1, 14_000), mk("syn_b", 0.11, 15_000), mk("real_c", 0.09, 13_000)],
            negatives: vec![mk("n1", 0.3, 12_000), mk("n2", 0.5, 20_000)],
            backgrounds: vec![],
        }
    }

    #[test]
    fn examples_have_the_label_the_source_says_and_repeat_with_the_seed() {
        let ds = tiny();
        let lm = LogMel::new();
        let (mut a, mut b) = (Rng::new(4), Rng::new(4));
        let (x1, l1) = make_example(&ds, &lm, Source::Phrase, &mut a);
        let (x2, l2) = make_example(&ds, &lm, Source::Phrase, &mut b);
        assert_eq!((l1, x1.len()), (1, WINDOW_FRAMES_N * N_MEL));
        assert_eq!(l2, 1);
        assert_eq!(x1, x2);
        assert_eq!(make_example(&ds, &lm, Source::Fragment, &mut a).1, 0);
        assert_eq!(make_example(&ds, &lm, Source::Negative, &mut a).1, 0);
        assert_eq!(make_example(&ds, &lm, Source::Background, &mut a).1, 0);
        assert!(ds.any_real());
    }

    const WINDOW_FRAMES_N: usize = crate::features::WINDOW_FRAMES;

    #[test]
    fn the_split_holds_out_whole_speakers_and_keeps_files_whole() {
        let ds = tiny();
        let (tr, va) = ds.split(3);
        assert_eq!(tr.positives.len() + va.positives.len(), 3);
        assert_eq!(va.positives.len(), 1);
        assert!(tr.positives.iter().all(|c| va.positives.iter().all(|d| d.name != c.name)));
        assert!(ds.check_trainable().is_ok());
        assert!(DataSet::default().check_trainable().is_err());
    }

    #[test]
    fn a_folder_loads_with_real_files_marked() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wake-test-scratch").join(format!("ds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("positive")).unwrap();
        std::fs::create_dir_all(dir.join("negative")).unwrap();
        let tone: Vec<f32> = (0..16_000).map(|i| 0.3 * (i as f32 * 0.2).sin()).collect();
        std::fs::write(dir.join("positive/real_amy_01.wav"), crate::wav::encode(&tone)).unwrap();
        std::fs::write(dir.join("positive/syn_0.wav"), crate::wav::encode(&tone)).unwrap();
        std::fs::write(dir.join("negative/x.wav"), crate::wav::encode(&tone)).unwrap();
        std::fs::write(dir.join("negative/notes.txt"), b"x").unwrap();
        let ds = DataSet::load(&dir).unwrap();
        assert_eq!((ds.positives.len(), ds.negatives.len(), ds.backgrounds.len()), (2, 1, 0));
        assert!(ds.positives[0].real && !ds.positives[1].real && ds.any_real());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
