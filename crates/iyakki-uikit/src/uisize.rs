//! The size of text and buttons a person chose (Settings → Display), kept for them and applied when they sign in.

use std::path::PathBuf;

/// The choices, as a name and a factor.
pub const LEVELS: [(&str, f32); 5] = [("Small", 0.85), ("Normal", 1.0), ("Large", 1.25), ("Larger", 1.5), ("Huge", 2.0)];
pub const NORMAL: usize = 1;

fn path(dir: &std::path::Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

/// The size is a choice of the COMPUTER (a small screen and a big one want other sizes): the program installs these two hooks at start
/// (`iyakki_agent::hwprofile`). `get` says `None` = use the person's file (no per-computer profile, or the category is shared),
/// `Some(None)` = this computer has no choice yet (Normal), `Some(Some(i))` = its choice. `set` records a choice for this computer.
pub type SizeGet = fn(&std::path::Path, &str) -> Option<Option<usize>>;
pub type SizeSet = fn(&std::path::Path, &str, usize);
static HOOKS: std::sync::OnceLock<(SizeGet, SizeSet)> = std::sync::OnceLock::new();

pub fn set_computer_hooks(get: SizeGet, set: SizeSet) {
    let _ = HOOKS.set((get, set));
}

/// The saved choice (an index into [`LEVELS`]), "Normal" when there is none.
pub fn load(dir: &std::path::Path, user: &str) -> usize {
    if let Some((get, _)) = HOOKS.get() {
        if let Some(per_computer) = get(dir, user) {
            return per_computer.map_or(NORMAL, |i| i.min(LEVELS.len() - 1));
        }
    }
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["size"].as_u64()).map_or(NORMAL, |i| (i as usize).min(LEVELS.len() - 1))
}

fn read(dir: &std::path::Path, user: &str) -> serde_json::Value {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}))
}

fn write(dir: &std::path::Path, user: &str, v: &serde_json::Value) {
    let p = path(dir, user);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn save(dir: &std::path::Path, user: &str, level: usize) {
    let mut v = read(dir, user);
    v["size"] = serde_json::json!(level.min(LEVELS.len() - 1));
    write(dir, user, &v);
    if let Some((_, set)) = HOOKS.get() {
        set(dir, user, level.min(LEVELS.len() - 1));
    }
}

/// The launcher group the person last used (its id), if any.
pub fn load_group(dir: &std::path::Path, user: &str) -> Option<String> {
    read(dir, user)["group"].as_str().map(str::to_string)
}

pub fn save_group(dir: &std::path::Path, user: &str, group: &str) {
    let mut v = read(dir, user);
    v["group"] = serde_json::json!(group);
    write(dir, user, &v);
}

/// The AI IDE's density (0 Comfortable, 1 Compact, 2 Dense), Compact when the person never chose.
pub fn load_ide_density(dir: &std::path::Path, user: &str) -> usize {
    read(dir, user)["ide_density"].as_u64().map_or(1, |i| (i as usize).min(2))
}

pub fn save_ide_density(dir: &std::path::Path, user: &str, level: usize) {
    let mut v = read(dir, user);
    v["ide_density"] = serde_json::json!(level.min(2));
    write(dir, user, &v);
}

/// The person's clock choices: (12-hour clock, seconds shown). The zone and network time are the machine's, in Settings.
pub fn load_clock(dir: &std::path::Path, user: &str) -> (bool, bool) {
    let v = read(dir, user);
    (v["hour12"].as_bool().unwrap_or(false), v["seconds"].as_bool().unwrap_or(false))
}

pub fn save_clock(dir: &std::path::Path, user: &str, hour12: bool, seconds: bool) {
    let mut v = read(dir, user);
    v["hour12"] = serde_json::json!(hour12);
    v["seconds"] = serde_json::json!(seconds);
    write(dir, user, &v);
}

/// Make the program draw at the person's size (or at normal size with `None`).
pub fn apply(level: Option<usize>) {
    crate::ui::set_ui_size(level.map_or(1.0, |l| LEVELS[l.min(LEVELS.len() - 1)].1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_kept_per_person_and_unknown_or_missing_means_normal() {
        let d = std::env::temp_dir().join(format!("iyakki-uisize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(load(&d, "ann"), NORMAL);
        save(&d, "ann", 3);
        save(&d, "bob", 0);
        assert_eq!((load(&d, "ann"), load(&d, "bob"), load(&d, "cy")), (3, 0, NORMAL));
        std::fs::write(path(&d, "ann"), "{\"size\": 99}").unwrap();
        assert_eq!(load(&d, "ann"), LEVELS.len() - 1, "an out-of-range value is brought back in range");
        std::fs::write(path(&d, "ann"), "garbage").unwrap();
        assert_eq!(load(&d, "ann"), NORMAL);
        assert_eq!(LEVELS[NORMAL].1, 1.0);
        // the launcher group lives in the same file and neither choice wipes the other
        assert_eq!(load_group(&d, "ann"), None);
        save_group(&d, "bob", "work");
        save(&d, "bob", 2);
        assert_eq!((load(&d, "bob"), load_group(&d, "bob").as_deref()), (2, Some("work")));
        // the IDE's density is kept per person next to them, and does not disturb them
        assert_eq!(load_ide_density(&d, "bob"), 1, "Compact by default");
        save_ide_density(&d, "bob", 2);
        assert_eq!((load_ide_density(&d, "bob"), load(&d, "bob"), load_group(&d, "bob").as_deref()), (2, 2, Some("work")));
        assert_eq!(load_ide_density(&d, "ann"), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
