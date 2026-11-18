//! The person's type choices beyond the family (F-SKIN-04): line height, letter spacing and tabular figures, kept beside the family
//! (`fonts`), the theme and the UI size in `ui-<user>.json` (keys `line_height`, `tracking`, `tabular`). The family itself is
//! [`crate::fonts`]; this file only keeps the words and the numbers they mean. A missing or unknown value is the normal one.

use std::path::{Path, PathBuf};

/// Distance between baselines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineHeight {
    Tight,
    #[default]
    Normal,
    Loose,
}

/// Extra space after each letter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tracking {
    #[default]
    Normal,
    Wide,
    Wider,
}

pub const LINE_ALL: [LineHeight; 3] = [LineHeight::Tight, LineHeight::Normal, LineHeight::Loose];
pub const TRACKING_ALL: [Tracking; 3] = [Tracking::Normal, Tracking::Wide, Tracking::Wider];

impl LineHeight {
    pub fn key(self) -> &'static str {
        match self {
            LineHeight::Tight => "tight",
            LineHeight::Normal => "normal",
            LineHeight::Loose => "loose",
        }
    }
    pub fn from_key(k: &str) -> Option<LineHeight> {
        LINE_ALL.into_iter().find(|l| l.key() == k.trim().to_lowercase())
    }
    /// Baseline to baseline, in text sizes.
    pub fn factor(self) -> f32 {
        match self {
            LineHeight::Tight => 1.2,
            LineHeight::Normal => 1.35,
            LineHeight::Loose => 1.6,
        }
    }
}

impl Tracking {
    pub fn key(self) -> &'static str {
        match self {
            Tracking::Normal => "normal",
            Tracking::Wide => "wide",
            Tracking::Wider => "wider",
        }
    }
    pub fn from_key(k: &str) -> Option<Tracking> {
        TRACKING_ALL.into_iter().find(|l| l.key() == k.trim().to_lowercase())
    }
    /// Extra space after each letter, in text sizes.
    pub fn em(self) -> f32 {
        match self {
            Tracking::Normal => 0.0,
            Tracking::Wide => 0.04,
            Tracking::Wider => 0.08,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Prefs {
    pub line: LineHeight,
    pub tracking: Tracking,
    pub tabular: bool,
}

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

fn read(dir: &Path, user: &str) -> serde_json::Value {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}))
}

pub fn load(dir: &Path, user: &str) -> Prefs {
    let v = read(dir, user);
    Prefs {
        line: v["line_height"].as_str().and_then(LineHeight::from_key).unwrap_or_default(),
        tracking: v["tracking"].as_str().and_then(Tracking::from_key).unwrap_or_default(),
        tabular: v["tabular"].as_bool().unwrap_or(false),
    }
}

/// Keep the choice, leaving every other field of the file alone.
pub fn save(dir: &Path, user: &str, p: Prefs) {
    let file = path(dir, user);
    let mut v = read(dir, user);
    v["line_height"] = serde_json::json!(p.line.key());
    v["tracking"] = serde_json::json!(p.tracking.key());
    v["tabular"] = serde_json::json!(p.tabular);
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
}

thread_local! {
    static CURRENT: std::cell::Cell<Prefs> = const { std::cell::Cell::new(Prefs { line: LineHeight::Normal, tracking: Tracking::Normal, tabular: false }) };
}

/// The choices now in force on this thread (the shell's, like the font family and the theme).
pub fn current() -> Prefs {
    CURRENT.with(std::cell::Cell::get)
}

pub fn set_current(p: Prefs) {
    CURRENT.with(|c| c.set(p));
}

pub fn describe(p: Prefs) -> String {
    format!("line height {}, letter spacing {}, tabular figures {}", p.line.key(), p.tracking.key(), if p.tabular { "on" } else { "off" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_are_kept_beside_the_other_display_settings_and_a_bad_value_is_the_normal_one() {
        let d = std::env::temp_dir().join(format!("iyakki-typeset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(load(&d, "ann"), Prefs::default());
        std::fs::write(d.join("ui-ann.json"), r#"{"theme":"light","line_height":"huge","tracking":7}"#).unwrap();
        assert_eq!(load(&d, "ann"), Prefs::default());
        save(&d, "ann", Prefs { line: LineHeight::Loose, tracking: Tracking::Wide, tabular: true });
        assert_eq!(load(&d, "ann"), Prefs { line: LineHeight::Loose, tracking: Tracking::Wide, tabular: true });
        assert_eq!(load(&d, "bob"), Prefs::default());
        let t = std::fs::read_to_string(d.join("ui-ann.json")).unwrap();
        assert!(t.contains("\"theme\":\"light\""), "other fields stay: {t}");
        assert!(LineHeight::from_key("Loose") == Some(LineHeight::Loose) && Tracking::from_key("nope").is_none());
        assert!(LINE_ALL.windows(2).all(|w| w[0].factor() < w[1].factor()) && TRACKING_ALL.windows(2).all(|w| w[0].em() < w[1].em()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
