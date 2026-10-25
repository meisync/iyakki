//! The screen saver and screen-off choices: what happens when nobody has touched the machine for a while.
//!
//! Pure data and rules, no drawing and no hardware: the shell (`iyakki-shell`: `saver.rs`, `shell_saver.rs`), the Settings page and the
//! tests all use these types. Everyone keeps their own choices in `screensaver-<user>.json` next to their other interface choices; a
//! guest, and anyone who never chose, gets [`SaverConfig::default`]. The sign-in screen has no person yet: it uses the machine's
//! `screensaver_signin` setting (an administrator changes it in Settings), which starts as [`SaverConfig::signin_default`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the first stage does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum First {
    /// Nothing: the screen stays as it is.
    Nothing,
    /// Start the screen saver (and, after the second delay, turn the screen off).
    #[default]
    Saver,
    /// Turn the screen off at once (dimmed for the last seconds).
    Off,
}

impl First {
    pub const ALL: [First; 3] = [First::Nothing, First::Saver, First::Off];

    pub fn label(self) -> &'static str {
        match self {
            First::Nothing => "Do nothing",
            First::Saver => "Screen saver",
            First::Off => "Turn the screen off",
        }
    }
}

/// What the saver looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// The IYAKKI logo drifting slowly, glowing and breathing.
    #[default]
    Logo,
    /// The same, with the time under the logo.
    LogoClock,
    /// A small dim logo on black that keeps moving (gentle on OLED panels).
    Small,
    /// The logo breathing, with only a very small drift.
    Breathing,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Logo, Style::LogoClock, Style::Small, Style::Breathing];

    pub fn label(self) -> &'static str {
        match self {
            Style::Logo => "IYAKKI logo",
            Style::LogoClock => "Logo and clock",
            Style::Small => "Small logo (OLED friendly)",
            Style::Breathing => "Breathing logo",
        }
    }

    /// The short name on a choice button.
    pub fn short(self) -> &'static str {
        match self {
            Style::Logo => "Logo",
            Style::LogoClock => "Logo + clock",
            Style::Small => "Small logo",
            Style::Breathing => "Breathing",
        }
    }
}

/// Minutes offered for the first delay (0 = never).
pub const AFTER_CHOICES: [u32; 7] = [0, 1, 2, 5, 10, 15, 30];
/// Minutes offered for the second delay (0 = none).
pub const THEN_CHOICES: [u32; 5] = [0, 1, 5, 10, 30];
/// Dim levels offered, in percent of the brightness the person had (0 = do not dim).
pub const DIM_CHOICES: [u32; 4] = [0, 50, 25, 10];

/// One person's (or the sign-in screen's) choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaverConfig {
    /// What happens after [`SaverConfig::after_min`] minutes without input.
    pub first: First,
    /// Minutes of no input before the first stage (0 = never).
    pub after_min: u32,
    /// After the saver ran this many more minutes, turn the screen off (0 = never; only with [`First::Saver`]).
    pub then_off_min: u32,
    /// On battery the delays are halved.
    pub battery_shorter: bool,
    pub style: Style,
    /// Dim the backlight to this percent when the saver starts, or just before the screen goes off (0 = do not dim).
    pub dim_pct: u32,
}

impl Default for SaverConfig {
    fn default() -> SaverConfig {
        SaverConfig { first: First::Saver, after_min: 10, then_off_min: 10, battery_shorter: true, style: Style::Logo, dim_pct: 50 }
    }
}

impl SaverConfig {
    /// The sign-in screen starts with the saver after 5 minutes and the screen off 10 minutes after that.
    pub fn signin_default() -> SaverConfig {
        SaverConfig { after_min: 5, then_off_min: 10, ..SaverConfig::default() }
    }

    /// The same choices with every value snapped to one that is offered (a hand-edited file cannot make a strange delay).
    pub fn normalized(mut self) -> SaverConfig {
        fn snap(v: u32, choices: &[u32]) -> u32 {
            if choices.contains(&v) {
                return v;
            }
            choices.iter().copied().min_by_key(|c| c.abs_diff(v)).unwrap_or(0)
        }
        self.after_min = snap(self.after_min, &AFTER_CHOICES);
        self.then_off_min = snap(self.then_off_min, &THEN_CHOICES);
        self.dim_pct = snap(self.dim_pct, &DIM_CHOICES);
        self
    }

    /// Whether anything ever happens.
    pub fn active(&self) -> bool {
        self.first != First::Nothing && self.after_min > 0
    }

    /// Seconds of no input before the first stage, and before the screen goes off (counted from the first stage's start; only after
    /// a saver). Halved on battery when the person chose that. `None` = never.
    pub fn delays(&self, on_battery: bool) -> (Option<u64>, Option<u64>) {
        if !self.active() {
            return (None, None);
        }
        let k = |m: u32| -> u64 {
            let s = u64::from(m) * 60;
            if on_battery && self.battery_shorter { (s / 2).max(15) } else { s }
        };
        let first = k(self.after_min);
        let off = (self.first == First::Saver && self.then_off_min > 0).then(|| k(self.then_off_min));
        (Some(first), off)
    }
}

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("screensaver-{who}.json"))
}

impl SaverConfig {
    /// The saved choices; a missing or unreadable file gives the defaults.
    pub fn load(dir: &Path, user: &str) -> SaverConfig {
        let got = std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<SaverConfig>(&t).ok());
        got.unwrap_or_default().normalized()
    }

    /// Whether this person has saved choices of their own.
    pub fn has_saved(dir: &Path, user: &str) -> bool {
        path(dir, user).is_file()
    }

    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        let p = path(dir, user);
        let tmp = p.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&self.clone().normalized()).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// A request from the Settings page to the shell that draws the screen (they share nothing but the data folder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Run the saver now for five seconds, or until input.
    Preview,
}

fn request_path(dir: &Path) -> PathBuf {
    dir.join("screensaver-request")
}

/// Ask the shell showing `user` to start the saver (Settings -> Screen saver -> Preview).
pub fn request(dir: &Path, user: &str, what: Request) {
    let Request::Preview = what;
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let _ = std::fs::write(request_path(dir), format!("preview {who}\n"));
}

/// The shell of `user` takes its request (a request for somebody else stays for their shell). Cheap: one small file.
pub fn take_request(dir: &Path, user: &str) -> Option<Request> {
    let p = request_path(dir);
    let text = std::fs::read_to_string(&p).ok()?;
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let mut it = text.split_whitespace();
    let (kind, target) = (it.next()?, it.next().unwrap_or(""));
    if target != who {
        return None;
    }
    let _ = std::fs::remove_file(&p);
    (kind == "preview").then_some(Request::Preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-saver-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    #[test]
    fn the_defaults_are_a_saver_after_ten_minutes_then_off_ten_later_halved_on_battery() {
        let c = SaverConfig::default();
        assert_eq!(c.delays(false), (Some(600), Some(600)));
        assert_eq!(c.delays(true), (Some(300), Some(300)), "halved on battery");
        let off = SaverConfig { battery_shorter: false, ..c };
        assert_eq!(off.delays(true), (Some(600), Some(600)), "the person turned the halving off");
        let signin = SaverConfig::signin_default();
        assert_eq!(signin.delays(false), (Some(300), Some(600)), "sign-in screen: 5 minutes, then off 10 minutes later");
    }

    #[test]
    fn never_and_do_nothing_mean_nothing_happens_and_off_has_no_second_stage() {
        assert_eq!(SaverConfig { after_min: 0, ..SaverConfig::default() }.delays(false), (None, None));
        assert_eq!(SaverConfig { first: First::Nothing, ..SaverConfig::default() }.delays(false), (None, None));
        assert_eq!(SaverConfig { first: First::Off, after_min: 5, ..SaverConfig::default() }.delays(false), (Some(300), None));
        assert_eq!(SaverConfig { then_off_min: 0, ..SaverConfig::default() }.delays(false), (Some(600), None));
    }

    #[test]
    fn the_battery_never_makes_a_delay_shorter_than_fifteen_seconds() {
        let c = SaverConfig { after_min: 1, then_off_min: 1, ..SaverConfig::default() };
        assert_eq!(c.delays(true), (Some(30), Some(30)));
    }

    #[test]
    fn strange_values_snap_to_the_nearest_choice() {
        let c = SaverConfig { after_min: 7, then_off_min: 400, dim_pct: 33, ..SaverConfig::default() }.normalized();
        assert_eq!((c.after_min, c.then_off_min, c.dim_pct), (5, 30, 25));
        assert!(AFTER_CHOICES.contains(&SaverConfig { after_min: u32::MAX, ..SaverConfig::default() }.normalized().after_min));
    }

    #[test]
    fn choices_are_saved_per_person_and_a_broken_file_gives_the_defaults() {
        let d = temp("save");
        assert_eq!(SaverConfig::load(&d, "amy"), SaverConfig::default());
        assert!(!SaverConfig::has_saved(&d, "amy"));
        let c = SaverConfig { first: First::Off, after_min: 2, style: Style::Small, ..SaverConfig::default() };
        c.save(&d, "amy").expect("saved");
        assert!(SaverConfig::has_saved(&d, "amy"));
        assert_eq!(SaverConfig::load(&d, "amy"), c);
        assert_eq!(SaverConfig::load(&d, "bob"), SaverConfig::default(), "bob has his own (none)");
        std::fs::write(d.join("screensaver-amy.json"), "{ not json").expect("write");
        assert_eq!(SaverConfig::load(&d, "amy"), SaverConfig::default());
        // a file from a later version with a field this one does not know, and a missing field
        std::fs::write(d.join("screensaver-amy.json"), r#"{"after_min":15,"future":true}"#).expect("write");
        assert_eq!(SaverConfig::load(&d, "amy").after_min, 15);
        // user names cannot leave the folder
        c.save(&d, "../evil").expect("saved");
        assert!(d.join("screensaver-..evil.json").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_preview_request_reaches_only_the_shell_of_that_person_once() {
        let d = temp("request");
        request(&d, "amy", Request::Preview);
        assert_eq!(take_request(&d, "bob"), None, "not bob's");
        assert_eq!(take_request(&d, "amy"), Some(Request::Preview));
        assert_eq!(take_request(&d, "amy"), None, "taken once");
        assert_eq!(take_request(&d, ""), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
