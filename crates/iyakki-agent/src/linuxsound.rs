//! Sound out of a Linux sandbox through IYAKKI's mixer (F-LINAPP-18, docs/development/AUDIO-MIXER.md). With the sound switch of a sandbox on
//! and the mixer on, the sandbox is NOT given the card: its programs play into a pipe (ALSA's `file` plugin, a `default` PCM that writes raw
//! 48 kHz stereo 16-bit PCM there, the same mechanism as the browser route in `webmix.rs`), a tap of the agent reads the pipe and hands the
//! sound to the mixer as a media source named "Linux app <sandbox>", which ducks it for speech, applies the master volume and mute and is
//! the one writer to the card. So one program does not hold the card, and the sandbox cannot listen to anything.
//!
//! This file makes the host side (the folder with the pipe and `asound.conf`, and the tap) when a sandbox starts; the sandbox side (the
//! read-only folder bind and `ALSA_CONFIG_PATH`) is `linuxapps_devices::grant`. With the mixer off, `prepare` says so and the sandbox keeps
//! the raw playback nodes as before. Not proven on hardware: that the sandbox's alsa-lib has the `file` plugin and keeps a steady clock.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use iyakki_shared::linuxapps_devices::{SoundRoute, ROUTE_CONF, ROUTE_DST, ROUTE_FIFO};

use crate::webmix::{self, Tap};

/// The taps that are running, by sandbox name (one per sandbox; a sandbox's programs share it).
static TAPS: Mutex<BTreeMap<String, Tap>> = Mutex::new(BTreeMap::new());

/// How a tap is started: the real one asks the global mixer; tests give another.
pub type Starter = fn(&Path, &'static str) -> Result<Tap, String>;

pub struct MixerRoute {
    start: Starter,
}

impl MixerRoute {
    pub fn new() -> MixerRoute {
        MixerRoute { start: webmix::start_labelled }
    }
    pub fn with_starter(start: Starter) -> MixerRoute {
        MixerRoute { start }
    }
}

impl Default for MixerRoute {
    fn default() -> MixerRoute {
        MixerRoute::new()
    }
}

fn label_of(sandbox: &str) -> &'static str {
    // the mixer wants a static label; one copy per sandbox name, bounded
    static POOL: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let text = format!("Linux app {sandbox}");
    let mut p = POOL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(x) = p.iter().find(|x| **x == text) {
        return x;
    }
    if p.len() >= 64 {
        return "Linux app";
    }
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    p.push(leaked);
    leaked
}

impl SoundRoute for MixerRoute {
    fn prepare(&self, sandbox: &str, dir: &Path) -> Result<(), String> {
        if sandbox.is_empty() || !sandbox.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("the sandbox name is not valid".into());
        }
        let fifo = dir.join(ROUTE_FIFO);
        let mut taps = TAPS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if taps.get(sandbox).is_some_and(|t| t.path() == fifo.as_path()) && fifo.exists() && dir.join(ROUTE_CONF).is_file() {
            return Ok(());
        }
        taps.remove(sandbox);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // the conf names the pipe as the sandbox sees it
        let conf = webmix::conf_text(Path::new(&format!("{ROUTE_DST}/{ROUTE_FIFO}"))).ok_or("the pipe path is not valid")?;
        iyakki_schema::write_atomic(&dir.join(ROUTE_CONF), conf.as_bytes(), 0o644)?;
        let tap = (self.start)(&fifo, label_of(sandbox))?;
        taps.insert(sandbox.to_string(), tap);
        Ok(())
    }
}

/// Put the route in place for every sandbox this process starts (called once when the services open).
pub fn install() {
    iyakki_shared::linuxapps_devices::set_sound_route(Some(Arc::new(MixerRoute::new())));
}

#[cfg(test)]
#[path = "linuxsound_tests.rs"]
mod tests;
