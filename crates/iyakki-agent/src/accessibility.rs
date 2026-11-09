//! Accessibility choices that are kept for a person: the screen reader, the large pointer, the big focus ring, reduced motion, sticky
//! keys and slow keys (docs/ACCESSIBILITY.md). High contrast and the text size are not stored here: they are the person's theme and UI
//! size in `ui-<name>.json` (Settings, Display), which this module can read and, for the theme, write, so one place stays the truth.
//!
//! Two files in the data folder, nothing else:
//!
//! * `access-<name>.json`: the person's own choices.
//! * `access-machine.json`: only whether the screen reader is on. The sign-in screen has no person yet, so the choice made there (or by
//!   the last person) must live with the computer: a blind person who turned the reader on once finds it on at the next start.
//!
//! The shell and the tools (the chat's `accessibility_set`, Settings) share nothing but these files: [`stamp`] says when any of them
//! changed so the shell applies the new choices within a second.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The slow-keys delays offered, in milliseconds (0 = off).
pub const SLOW_CHOICES: [u32; 5] = [0, 300, 500, 800, 1200];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Access {
    /// Speak what the person moves to, new messages and dialogs (off by default).
    pub screen_reader: bool,
    /// Speak toasts, errors and notices too while the reader is on.
    pub speak_notices: bool,
    /// A bigger mouse pointer.
    pub large_pointer: bool,
    /// A thicker, brighter focus ring.
    pub big_ring: bool,
    /// No animation and no blinking caret.
    pub reduce_motion: bool,
    /// Shift, Ctrl, Alt and Super stay on for the next key (press them one after the other, not together).
    pub sticky_keys: bool,
    /// A key counts only after it was held this long (0 = off, see [`SLOW_CHOICES`]).
    pub slow_keys_ms: u32,
}

impl Default for Access {
    fn default() -> Access {
        Access { screen_reader: false, speak_notices: true, large_pointer: false, big_ring: false, reduce_motion: false, sticky_keys: false, slow_keys_ms: 0 }
    }
}

impl Access {
    /// The same choices with out-of-range numbers brought back (a hand-edited file never breaks the keyboard).
    pub fn sane(mut self) -> Access {
        if !SLOW_CHOICES.contains(&self.slow_keys_ms) {
            self.slow_keys_ms = SLOW_CHOICES.iter().copied().filter(|c| *c >= self.slow_keys_ms.min(1200)).find(|c| *c > 0).unwrap_or(1200);
        }
        self
    }

    /// One line per choice, for the assistant and the status line of Settings.
    pub fn describe(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        format!(
            "screen reader {}; speak notices {}; large pointer {}; big focus ring {}; reduce motion {}; sticky keys {}; slow keys {}",
            on(self.screen_reader),
            on(self.speak_notices),
            on(self.large_pointer),
            on(self.big_ring),
            on(self.reduce_motion),
            on(self.sticky_keys),
            if self.slow_keys_ms == 0 { "off".to_string() } else { format!("{} ms", self.slow_keys_ms) }
        )
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

fn person_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("access-{}.json", who(user)))
}

fn machine_path(dir: &Path) -> PathBuf {
    dir.join("access-machine.json")
}

fn ui_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("ui-{}.json", who(user)))
}

fn write_atomic(p: &Path, text: &str) -> Result<(), String> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("could not save: {e}"))?;
    std::fs::rename(&tmp, p).map_err(|e| format!("could not save: {e}"))
}

/// The person's own choices (the defaults when there is no file or it is broken).
pub fn load(dir: &Path, user: &str) -> Access {
    std::fs::read_to_string(person_path(dir, user)).ok().and_then(|t| serde_json::from_str::<Access>(&t).ok()).unwrap_or_default().sane()
}

/// Whether the computer keeps the screen reader on (what the sign-in screen uses).
pub fn machine_reader(dir: &Path) -> bool {
    std::fs::read_to_string(machine_path(dir)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["screen_reader"].as_bool()).unwrap_or(false)
}

pub fn set_machine_reader(dir: &Path, on: bool) -> Result<(), String> {
    write_atomic(&machine_path(dir), &serde_json::json!({ "screen_reader": on }).to_string())
}

/// What applies while `user` is signed in: their choices, with the screen reader also on when the computer keeps it on.
pub fn effective(dir: &Path, user: &str) -> Access {
    let mut a = load(dir, user);
    a.screen_reader |= machine_reader(dir);
    a
}

/// Keep the person's choices. The screen reader choice is also the computer's, so the next sign-in screen speaks (or stays quiet).
pub fn save(dir: &Path, user: &str, a: &Access) -> Result<(), String> {
    let a = a.clone().sane();
    write_atomic(&person_path(dir, user), &serde_json::to_string(&a).map_err(|e| e.to_string())?)?;
    set_machine_reader(dir, a.screen_reader)
}

/// The person's theme key ("dark", "light", "high-contrast") from `ui-<name>.json`.
pub fn theme_key(dir: &Path, user: &str) -> String {
    std::fs::read_to_string(ui_path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["theme"].as_str().map(str::to_string)).unwrap_or_else(|| "dark".into())
}

pub fn high_contrast(dir: &Path, user: &str) -> bool {
    theme_key(dir, user) == "high-contrast"
}

/// The text size level (index into the UI size list: 0 Small .. 4 Huge, 1 Normal) from `ui-<name>.json`.
pub fn text_level(dir: &Path, user: &str) -> usize {
    std::fs::read_to_string(ui_path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["size"].as_u64()).map_or(1, |i| (i as usize).min(4))
}

/// Switch High contrast on, or back to the theme that was in use before (Dark when it was never anything else). Same file and key as
/// Settings, Display, so both agree.
pub fn set_high_contrast(dir: &Path, user: &str, on: bool) -> Result<(), String> {
    let p = ui_path(dir, user);
    let mut v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let cur = v["theme"].as_str().unwrap_or("dark").to_string();
    if on {
        if cur != "high-contrast" {
            v["theme_before_contrast"] = serde_json::json!(cur);
        }
        v["theme"] = serde_json::json!("high-contrast");
    } else if cur == "high-contrast" {
        let back = v["theme_before_contrast"].as_str().filter(|t| *t != "high-contrast").unwrap_or("dark").to_string();
        v["theme"] = serde_json::json!(back);
    }
    write_atomic(&p, &v.to_string())
}

/// A cheap fingerprint of the three files; the shell compares it once a second and reloads when it changed.
pub fn stamp(dir: &Path, user: &str) -> [Option<std::time::SystemTime>; 3] {
    let m = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    [m(person_path(dir, user)), m(machine_path(dir)), m(ui_path(dir, user))]
}
