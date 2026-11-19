//! What SDK apps keep and the desktop choices they may change, as files of the data folder (the UI SDK's storage, dock and wallpaper
//! ports end here). The one place that opens these files for the UI.
//!
//! * App data: `<data>/appdata/<user>/<app>.json`, one JSON object of text values. Limits are the SDK's: keys up to 128 characters, no
//!   control characters, a value up to 64 KB, 256 KB of keys and values together per app and person. The user and app names are checked
//!   (letters, digits, `-`, `_`, at most 48), so a name can never walk out of the folder.
//! * Desktop: the `dock_items` and `wallpaper` fields of `<data>/ui-<user>.json`, beside the choices the Settings pages keep there; every
//!   other field is left alone.

use std::path::{Path, PathBuf};

use iyakki_core_api::{AppData, Desktop};

const MAX_KEY: usize = 128;
const MAX_VALUE: usize = 64 * 1024;
const LIMIT: usize = 256 * 1024;
const MAX_DOCK: usize = 24;

pub struct LocalUiState {
    dir: PathBuf,
}

fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 48 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn write_atomic(p: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p).map_err(|e| e.to_string())
}

impl LocalUiState {
    pub fn new(dir: &Path) -> LocalUiState {
        LocalUiState { dir: dir.to_path_buf() }
    }

    fn data_path(&self, user: &str, app: &str) -> Result<PathBuf, String> {
        if !name_ok(user) || !name_ok(app) {
            return Err("that person or app name is not allowed".into());
        }
        Ok(self.dir.join("appdata").join(user).join(format!("{app}.json")))
    }

    fn read_map(&self, user: &str, app: &str) -> std::collections::BTreeMap<String, String> {
        self.data_path(user, app).ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn ui_path(&self, user: &str) -> Result<PathBuf, String> {
        if !name_ok(user) {
            return Err("that person name is not allowed".into());
        }
        Ok(self.dir.join(format!("ui-{user}.json")))
    }

    fn ui_json(&self, user: &str) -> Option<serde_json::Value> {
        self.ui_path(user).ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok())
    }

    /// One field of one screen's choices, under `screens` -> the screen's id, leaving every other field alone.
    fn set_screen_field(&self, user: &str, screen: u32, field: &str, value: &str) -> Result<(), String> {
        let p = self.ui_path(user)?;
        let mut v = self.ui_json(user).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
        if !v["screens"].is_object() {
            v["screens"] = serde_json::json!({});
        }
        if v["screens"].as_object().is_some_and(|m| m.len() >= 16 && !m.contains_key(&screen.to_string())) {
            return Err("too many screens are kept".into());
        }
        if !v["screens"][screen.to_string()].is_object() {
            v["screens"][screen.to_string()] = serde_json::json!({});
        }
        v["screens"][screen.to_string()][field] = serde_json::json!(value);
        write_atomic(&p, &v.to_string())
    }

    fn set_ui_field(&self, user: &str, field: &str, value: serde_json::Value) -> Result<(), String> {
        let p = self.ui_path(user)?;
        let mut v = self.ui_json(user).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
        v[field] = value;
        write_atomic(&p, &v.to_string())
    }
}

impl AppData for LocalUiState {
    fn get(&self, user: &str, app: &str, key: &str) -> Option<String> {
        self.read_map(user, app).get(key).cloned()
    }

    fn set(&self, user: &str, app: &str, key: &str, value: &str) -> Result<(), String> {
        let p = self.data_path(user, app)?;
        if key.is_empty() || key.len() > MAX_KEY || key.chars().any(char::is_control) {
            return Err("the key is empty, too long or has control characters".into());
        }
        if value.len() > MAX_VALUE {
            return Err("the value is too long".into());
        }
        let mut m = self.read_map(user, app);
        let others: usize = m.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.len()).sum();
        if others + key.len() + value.len() > LIMIT {
            return Err("the app would keep more than 256 KB".into());
        }
        m.insert(key.to_string(), value.to_string());
        write_atomic(&p, &serde_json::to_string(&m).map_err(|e| e.to_string())?)
    }

    fn remove(&self, user: &str, app: &str, key: &str) -> Result<(), String> {
        let p = self.data_path(user, app)?;
        let mut m = self.read_map(user, app);
        if m.remove(key).is_some() {
            write_atomic(&p, &serde_json::to_string(&m).map_err(|e| e.to_string())?)?;
        }
        Ok(())
    }

    fn keys(&self, user: &str, app: &str) -> Vec<String> {
        self.read_map(user, app).into_keys().collect()
    }
}

impl Desktop for LocalUiState {
    fn dock(&self, user: &str) -> Option<Vec<String>> {
        self.ui_json(user)?["dock_items"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
    }

    fn set_dock(&self, user: &str, items: &[String]) -> Result<(), String> {
        if items.len() > MAX_DOCK || items.iter().any(|i| !name_ok(i) || i.len() > 32) {
            return Err("the dock list is too long or has a name that is not an app id".into());
        }
        self.set_ui_field(user, "dock_items", serde_json::json!(items))
    }

    fn wallpaper(&self, user: &str) -> Option<String> {
        self.ui_json(user)?["wallpaper"].as_str().map(String::from)
    }

    fn set_wallpaper(&self, user: &str, key: &str) -> Result<(), String> {
        if !name_ok(key) {
            return Err("that is not a wallpaper key".into());
        }
        self.set_ui_field(user, "wallpaper", serde_json::json!(key))
    }

    fn screen_wallpaper(&self, user: &str, screen: u32) -> Option<String> {
        self.ui_json(user)?["screens"][screen.to_string()]["wallpaper"].as_str().map(String::from)
    }

    fn set_screen_wallpaper(&self, user: &str, screen: u32, key: &str) -> Result<(), String> {
        if !name_ok(key) {
            return Err("that is not a wallpaper key".into());
        }
        self.set_screen_field(user, screen, "wallpaper", key)
    }

    fn screen_dock(&self, user: &str, screen: u32) -> Option<String> {
        self.ui_json(user)?["screens"][screen.to_string()]["dock"].as_str().map(String::from)
    }

    fn set_screen_dock(&self, user: &str, screen: u32, position: &str) -> Result<(), String> {
        if !matches!(position, "bottom" | "left" | "right" | "off") {
            return Err("the dock sits at bottom, left, right or off".into());
        }
        self.set_screen_field(user, screen, "dock", position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-uistate-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn data_is_kept_per_person_and_app_and_comes_back() {
        let d = tmp("data");
        let s = LocalUiState::new(&d);
        s.set("alice", "sample", "notes", "0 one\n1 two").unwrap();
        s.set("alice", "sample", "other", "x").unwrap();
        assert_eq!(s.get("alice", "sample", "notes").as_deref(), Some("0 one\n1 two"));
        assert_eq!(s.keys("alice", "sample"), vec!["notes".to_string(), "other".to_string()]);
        assert_eq!(s.get("bob", "sample", "notes"), None, "another person never sees it");
        assert_eq!(s.get("alice", "other-app", "notes"), None, "another app never sees it");
        s.remove("alice", "sample", "other").unwrap();
        assert_eq!(s.keys("alice", "sample"), vec!["notes".to_string()]);
        assert_eq!(LocalUiState::new(&d).get("alice", "sample", "notes").as_deref(), Some("0 one\n1 two"), "it is on disk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_that_walk_out_and_oversize_values_are_refused() {
        let d = tmp("limits");
        let s = LocalUiState::new(&d);
        assert!(s.set("..", "sample", "k", "v").is_err());
        assert!(s.set("alice", "../x", "k", "v").is_err());
        assert!(s.set("alice", "sample", "", "v").is_err());
        assert!(s.set("alice", "sample", "a\nb", "v").is_err());
        assert!(s.set("alice", "sample", "k", &"x".repeat(MAX_VALUE + 1)).is_err());
        for i in 0..4 {
            s.set("alice", "sample", &format!("k{i}"), &"y".repeat(MAX_VALUE - 10)).unwrap();
        }
        assert!(s.set("alice", "sample", "k4", &"y".repeat(MAX_VALUE - 10)).is_err(), "256 KB together is the limit");
        s.set("alice", "sample", "k0", "short").unwrap();
        assert!(!d.join("x.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_screens_own_wallpaper_and_dock_are_kept_beside_the_persons_and_other_screens_are_untouched() {
        let d = tmp("screens");
        let s = LocalUiState::new(&d);
        assert_eq!((s.screen_wallpaper("alice", 2), s.screen_dock("alice", 2)), (None, None));
        s.set_wallpaper("alice", "dusk").unwrap();
        s.set_screen_wallpaper("alice", 2, "meadow").unwrap();
        s.set_screen_dock("alice", 2, "left").unwrap();
        s.set_screen_dock("alice", 3, "off").unwrap();
        assert_eq!(s.screen_wallpaper("alice", 2).as_deref(), Some("meadow"));
        assert_eq!((s.screen_dock("alice", 2).as_deref(), s.screen_dock("alice", 3).as_deref()), (Some("left"), Some("off")));
        assert_eq!(s.screen_wallpaper("alice", 3), None, "screen 3 has no wallpaper of its own");
        assert_eq!(s.wallpaper("alice").as_deref(), Some("dusk"), "the person's own wallpaper is not changed");
        assert!(s.set_screen_dock("alice", 2, "diagonal").is_err());
        assert!(s.set_screen_wallpaper("alice", 2, "../x").is_err());
        assert!(s.set_screen_wallpaper("..", 2, "dusk").is_err());
        for i in 10..30 {
            let _ = s.set_screen_dock("alice", i, "off");
        }
        assert!(s.set_screen_dock("alice", 99, "off").is_err(), "no more than 16 screens are kept");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_dock_and_wallpaper_are_fields_beside_the_other_ui_choices() {
        let d = tmp("desk");
        std::fs::write(d.join("ui-alice.json"), "{\"size\":125,\"theme\":\"light\"}").unwrap();
        let s = LocalUiState::new(&d);
        assert_eq!(s.dock("alice"), None);
        assert_eq!(s.wallpaper("alice"), None);
        s.set_dock("alice", &["chat".into(), "sample".into()]).unwrap();
        s.set_wallpaper("alice", "dusk").unwrap();
        assert_eq!(s.dock("alice"), Some(vec!["chat".to_string(), "sample".to_string()]));
        assert_eq!(s.wallpaper("alice").as_deref(), Some("dusk"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("ui-alice.json")).unwrap()).unwrap();
        assert_eq!((v["size"].as_i64(), v["theme"].as_str()), (Some(125), Some("light")), "the other fields stay");
        assert!(s.set_dock("alice", &["../x".into()]).is_err());
        assert!(s.set_dock("alice", &vec!["a".to_string(); 25]).is_err());
        assert!(s.set_wallpaper("alice", "a/b").is_err());
        assert!(s.set_wallpaper("..", "dusk").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
