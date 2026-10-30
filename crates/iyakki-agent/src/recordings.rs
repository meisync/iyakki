//! Voice recordings: the WAV files of the VOICE tile's Record mode, kept in the person's own `Recordings` folder. Pure file
//! logic (name, save, list, rename, delete) so it is tested without a screen; the sound itself is captured and played through
//! the shared entry points (`voicedesk::Recorders`, `audiosink`), never opened a second way here.

use crate::mixer::SourceKind;
use crate::story::{Playing, Sink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder under the person's home.
pub const FOLDER: &str = "Recordings";
/// Longest name a person gives a recording (characters, without `.wav`).
pub const MAX_NAME: usize = 80;

/// One saved recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The name shown (the file name without `.wav`).
    pub name: String,
    pub bytes: u64,
    pub secs: f32,
    /// Modification time, seconds since 1970 (0 when unknown).
    pub modified: u64,
}

impl Entry {
    pub fn file(&self) -> String {
        format!("{}.wav", self.name)
    }
}

/// `<home>/Recordings`.
pub fn folder(home: &Path) -> PathBuf {
    home.join(FOLDER)
}

/// A name the person typed made safe for a file name: trimmed, no folders, no control characters, no leading dot.
pub fn clean_name(input: &str) -> Result<String, String> {
    let t = input.trim();
    let t = t.strip_suffix(".wav").or_else(|| t.strip_suffix(".WAV")).unwrap_or(t).trim();
    if t.is_empty() {
        return Err("Type a name for the recording.".into());
    }
    if t.chars().count() > MAX_NAME {
        return Err(format!("That name is too long (most {MAX_NAME} characters)."));
    }
    if t.starts_with('.') {
        return Err("A name cannot start with a dot.".into());
    }
    if t.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("A name cannot contain / \\ : * ? \" < > | or control characters.".into());
    }
    Ok(t.to_string())
}

fn file_for(dir: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(dir.join(format!("{}.wav", clean_name(name)?)))
}

/// `stem.wav`, or `stem-2.wav` and so on: nothing is ever overwritten.
fn unique(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.wav"));
    if !first.exists() {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem}-{n}.wav"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Write `pcm` (16-bit mono at `rate`) as a WAV named after `stem`. The file appears whole or not at all. Returns the entry.
pub fn save(dir: &Path, stem: &str, pcm: &[i16], rate: u32) -> Result<Entry, String> {
    if pcm.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    let stem = clean_name(stem)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
    let path = unique(dir, &stem);
    let tmp = path.with_extension("wav.part");
    std::fs::write(&tmp, crate::voice::encode_wav(pcm, rate)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not save the recording: {e}")
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not save the recording: {e}")
    })?;
    entry_of(&path).ok_or_else(|| "The recording was saved but cannot be read back.".to_string())
}

fn entry_of(path: &Path) -> Option<Entry> {
    let meta = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    let name = path.file_name()?.to_str()?.strip_suffix(".wav")?.to_string();
    // the rate is in the header; the length follows from the size
    let mut head = [0u8; 44];
    let rate = std::fs::File::open(path).ok().and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head).ok()).map(|_| u32::from_le_bytes([head[24], head[25], head[26], head[27]])).filter(|r| *r > 0).unwrap_or(crate::voice::SAMPLE_RATE);
    let secs = meta.len().saturating_sub(44) as f32 / (rate as f32 * 2.0);
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Some(Entry { name, bytes: meta.len(), secs, modified })
}

/// The recordings of `dir`, newest first (empty when the folder does not exist).
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut v: Vec<Entry> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| entry_of(&e.path())).collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
    v
}

/// Rename a recording. A name that is taken is refused (nothing is overwritten). Returns the new name.
pub fn rename(dir: &Path, old: &str, new: &str) -> Result<String, String> {
    let to = file_for(dir, new)?;
    let from = file_for(dir, old)?;
    if !from.is_file() {
        return Err("That recording is gone.".into());
    }
    let new = clean_name(new)?;
    if from == to {
        return Ok(new);
    }
    if to.exists() {
        return Err(format!("A recording called {new} already exists."));
    }
    std::fs::rename(&from, &to).map_err(|e| format!("Could not rename it: {e}"))?;
    Ok(new)
}

/// Delete a recording.
pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    let p = file_for(dir, name)?;
    std::fs::remove_file(&p).map_err(|e| if p.exists() { format!("Could not delete it: {e}") } else { "That recording is gone.".to_string() })
}

/// The bytes of a recording, for playing it.
pub fn read(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(file_for(dir, name)?).map_err(|e| format!("Could not open the recording: {e}"))
}

/// A sound on the speaker through the shared audio entry point (the mixer, or the play command of Settings -> Voice).
struct OnSpeaker(crate::audiosink::Output);

impl Playing for OnSpeaker {
    fn is_playing(&mut self) -> bool {
        self.0.is_playing()
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

/// Plays WAV bytes with `Services::play_wav_output` (the one way sound reaches the speaker).
pub struct ServicesSink(pub Arc<crate::services::Services>);

impl Sink for ServicesSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        Ok(Box::new(OnSpeaker(self.0.play_wav_output(SourceKind::Media, "Voice recording", wav)?)))
    }
}

#[cfg(test)]
#[path = "recordings_tests.rs"]
mod tests;
