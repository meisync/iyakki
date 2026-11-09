//! The My IYAKKI wake phrase spotter, all in-house (docs/development/MY-IYAKKI-PLAN.md, step 4).
//!
//! * `features`, `fft`: 16 kHz samples to normalised log-mel frames.
//! * `model`: a small depthwise-separable CNN, forward and backward in f32.
//! * `train`, `dataset`, `augment`, `rng`, `wav`, `weights`: training on the dev PC from a folder of WAV files; the weights file.
//! * `synth`, `gen`: the data generator, with a formant speech stub (NOT real speech) and a plug-in point for a real engine.
//! * `spotter`: the streaming detector and the `WakeSpotter` trait (arm, mute, indicator, memory only).
//! * `eval`: misses and false accepts per hour over a threshold sweep.
//!
//! No dependencies. Nothing here records, stores or sends audio at run time; the file-touching modules (`wav`, `dataset`, `gen`,
//! `weights` loading by the tools) are for the dev PC's training pipeline only.
//!
//! Honest limits: with no microphone and no speech engine on the build machine, a model trained from `FormantTts` has never heard a real
//! voice. Accuracy on real voices and real rooms is unknown until real takes are recorded and the evaluation is run on them.

pub mod augment;
pub mod dataset;
pub mod eval;
pub mod features;
pub mod fft;
pub mod gen;
pub mod model;
pub mod rng;
pub mod spotter;
pub mod synth;
pub mod train;
pub mod wav;
pub mod weights;

pub use spotter::{FakeSpotter, Settings, Spotter, WakeEvent, WakeSpotter, WakeState};

/// The settings key (off by default, opt-in).
pub const SETTING_KEY: &str = "voice.my_iyakki_wake";

/// Training data and weights belong under the work folder, never in `/tmp` (a RAM disk that is wiped on reboot).
pub fn refuse_tmp(path: &std::path::Path) -> Result<(), String> {
    if path.starts_with("/tmp") {
        Err(format!("{} is under /tmp; use a folder under ~/iyakki-work instead", path.display()))
    } else {
        Ok(())
    }
}

/// What a person is told about the model in the status line, from the weights header.
pub fn describe_weights(h: &weights::Header) -> String {
    if h.data_kind == weights::DATA_REAL_INCLUDED {
        "trained with real recordings included".into()
    } else {
        "trained on synthetic speech only (it has not heard a real voice; accuracy on real voices is unknown until real takes are recorded)".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_is_refused() {
        assert!(refuse_tmp(std::path::Path::new("/tmp/x")).is_err());
        assert!(refuse_tmp(std::path::Path::new("/data/work/x")).is_ok());
    }

    #[test]
    fn the_weights_description_never_hides_synthetic_data() {
        let h = weights::Header { data_kind: 0, threshold: 0.8, seed: 1, epochs: 1, notes: String::new() };
        assert!(describe_weights(&h).contains("synthetic speech only"));
        let h = weights::Header { data_kind: 1, ..h };
        assert!(describe_weights(&h).contains("real recordings"));
    }
}
