//! Themes as data files a person can add (F-SHELL-62), beside the built-in Dark, Light and High contrast.
//!
//! A theme file is plain text, `<name>.theme`, in `<data>/themes/<person>/` (the person's own) or `<data>/themes/` (for everybody who
//! uses this computer; an administrator puts them there). Lines are `key = value`; `#` starts a comment line:
//!
//! ```text
//! name = Midnight blue
//! base = dark              # dark, light or high-contrast: every colour not written here comes from it, and so does the way colours that
//!                          # the apps made up are mapped
//! bg_top = #0b1020
//! text = #f2f4ff
//! ring = 1.5               # optional, 1.0 to 3.0
//! ```
//!
//! The colour keys are those of [`crate::theme::Theme`] (`bg_top bg_bottom card card_hover card_sel border accent text muted dim err ok field
//! panel well amber cyan magenta button button_hot`). A file is refused, with the line and the reason, when it is not understood or when
//! its colours cannot be read: text and muted text need at least 4.5:1 against every ground, the other signal colours at least 3:1, and the
//! border must be told apart from the card. A theme that cannot be read never becomes the active one; the person keeps the one they had.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::theme::{Theme, ThemeId};

/// Most theme files looked at in a folder, and the biggest file.
pub const MAX_FILES: usize = 16;
pub const MAX_BYTES: u64 = 4096;
/// Most different themes that can have been loaded in one run (each is kept for the life of the program).
const MAX_LOADED: usize = 48;

const GROUNDS: [&str; 5] = ["bg_top", "bg_bottom", "card", "field", "panel"];

fn get<'a>(t: &'a mut Theme, key: &str) -> Option<&'a mut [u8; 4]> {
    Some(match key {
        "bg_top" => &mut t.bg_top,
        "bg_bottom" => &mut t.bg_bottom,
        "card" => &mut t.card,
        "card_hover" => &mut t.card_hover,
        "card_sel" => &mut t.card_sel,
        "border" => &mut t.border,
        "accent" => &mut t.accent,
        "text" => &mut t.text,
        "muted" => &mut t.muted,
        "dim" => &mut t.dim,
        "err" => &mut t.err,
        "ok" => &mut t.ok,
        "field" => &mut t.field,
        "panel" => &mut t.panel,
        "well" => &mut t.well,
        "amber" => &mut t.amber,
        "cyan" => &mut t.cyan,
        "magenta" => &mut t.magenta,
        "button" => &mut t.button,
        "button_hot" => &mut t.button_hot,
        _ => return None,
    })
}

fn color(v: &str) -> Option<[u8; 4]> {
    let h = v.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8, 255])
}

fn lum(c: [u8; 4]) -> f64 {
    let f = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// A theme read from a file's text: its name and the theme (its `id` is the base it grew from).
#[derive(Clone, Debug)]
pub struct UserTheme {
    pub name: String,
    pub theme: Theme,
}

fn name_ok(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.chars().count() <= 32 && n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

/// The file name a theme name is kept under and chosen by: lower case, letters and digits, dashes.
pub fn key_of(name: &str) -> String {
    let k: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    k.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn parse(text: &str) -> Result<UserTheme, String> {
    let mut name = String::new();
    let mut base = ThemeId::Dark;
    let mut set: Vec<(String, [u8; 4])> = Vec::new();
    let mut ring: Option<f32> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = i + 1;
        let (k, v) = line.split_once('=').ok_or(format!("line {at}: expected `key = value`"))?;
        let (k, v) = (k.trim(), v.trim().split(" #").next().unwrap_or("").trim());
        let v = if k == "name" { line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("") } else { v };
        match k {
            "name" => {
                if !name_ok(v) {
                    return Err(format!("line {at}: the name is 1 to 32 letters, digits, spaces, dashes, underscores or dots"));
                }
                name = v.trim().to_string();
            }
            "base" => {
                base = match v {
                    "dark" => ThemeId::Dark,
                    "light" => ThemeId::Light,
                    "high-contrast" => ThemeId::HighContrast,
                    _ => return Err(format!("line {at}: base is dark, light or high-contrast")),
                }
            }
            "ring" => {
                let r: f32 = v.parse().map_err(|_| format!("line {at}: ring is a number from 1.0 to 3.0"))?;
                if !(1.0..=3.0).contains(&r) {
                    return Err(format!("line {at}: ring is a number from 1.0 to 3.0"));
                }
                ring = Some(r);
            }
            other => {
                let mut probe = *ThemeId::Dark.theme();
                if get(&mut probe, other).is_none() {
                    return Err(format!("line {at}: `{other}` is not a colour of the look"));
                }
                let c = color(v).ok_or(format!("line {at}: `{other}` is written #rrggbb"))?;
                set.push((other.to_string(), c));
            }
        }
    }
    if name.is_empty() {
        return Err("the file needs a `name = ...` line".into());
    }
    let mut t = *base.theme();
    for (k, c) in &set {
        if let Some(slot) = get(&mut t, k) {
            *slot = *c;
        }
    }
    if let Some(r) = ring {
        t.ring = r;
    }
    check(&t)?;
    Ok(UserTheme { name, theme: t })
}

/// The readability rules (see the module text).
pub fn check(t: &Theme) -> Result<(), String> {
    let mut probe = *t;
    for g in GROUNDS {
        let ground = *get(&mut probe, g).ok_or("internal: ground")?;
        for (k, need) in [("text", 4.5), ("muted", 4.5), ("dim", 3.0), ("accent", 3.0), ("err", 3.0), ("ok", 3.0), ("amber", 3.0), ("cyan", 3.0), ("magenta", 3.0)] {
            let c = *get(&mut probe, k).ok_or("internal: colour")?;
            let r = contrast(c, ground);
            if r < need {
                return Err(format!("`{k}` on `{g}` is {r:.1}:1 and needs at least {need}:1"));
            }
        }
    }
    if contrast(t.border, t.card) < 1.3 {
        return Err("`border` cannot be told apart from `card` (at least 1.3:1)".into());
    }
    Ok(())
}

/// What one folder holds: `(file key, Ok(theme) or the reason it was refused)`.
pub fn list_dir(dir: &Path) -> Vec<(String, Result<UserTheme, String>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "theme")).collect();
    files.sort();
    files.truncate(MAX_FILES);
    files
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let md = std::fs::symlink_metadata(&p);
            let r = match md {
                Ok(m) if m.is_file() && m.len() <= MAX_BYTES => std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| parse(&t)),
                Ok(m) if m.is_file() => Err(format!("the file is bigger than {MAX_BYTES} bytes")),
                _ => Err("not a plain file".into()),
            };
            (stem, r)
        })
        .collect()
}

/// The folders a person's themes are looked for in: their own first, then the shared one.
pub fn folders(data: &Path, user: &str) -> Vec<PathBuf> {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let mut v = Vec::new();
    if !who.is_empty() && !who.starts_with('.') {
        v.push(data.join("themes").join(who));
    }
    v.push(data.join("themes"));
    v
}

/// Every theme this person can choose, by the key they choose it with, the person's own shadowing a shared one of the same key.
pub fn available(data: &Path, user: &str) -> Vec<(String, Result<UserTheme, String>)> {
    let mut out: Vec<(String, Result<UserTheme, String>)> = Vec::new();
    for d in folders(data, user) {
        for (k, r) in list_dir(&d) {
            if !out.iter().any(|(o, _)| *o == k) {
                out.push((k, r));
            }
        }
    }
    out
}

static LOADED: Mutex<Vec<(String, &'static Theme)>> = Mutex::new(Vec::new());

/// A `&'static` theme for the drawing code (the same content is kept once; at most [`MAX_LOADED`] different ones in one run).
pub fn leak(u: &UserTheme) -> Option<&'static Theme> {
    let sig = format!("{:?}", u.theme);
    let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, t)) = g.iter().find(|(s, _)| *s == sig) {
        return Some(t);
    }
    if g.len() >= MAX_LOADED {
        return None;
    }
    let t: &'static Theme = Box::leak(Box::new(u.theme));
    g.push((sig, t));
    Some(t)
}

/// The key a person's saved choice carries for a user theme.
pub const CHOICE_PREFIX: &str = "user:";

/// Make a user theme the active one without keeping the choice. Err when it is not there or cannot be read (the active theme stays).
pub fn activate(data: &Path, user: &str, key: &str) -> Result<String, String> {
    let (_, r) = available(data, user).into_iter().find(|(k, _)| k == key).ok_or_else(|| format!("there is no theme file `{key}.theme`"))?;
    let u = r?;
    let t = leak(&u).ok_or("too many different themes were loaded in this run; sign out and in again")?;
    crate::theme::set_custom(Some(t));
    Ok(u.name)
}

/// Choose a user theme by its key: it becomes the active one and the person's choice. Err leaves everything as it was.
pub fn choose(data: &Path, user: &str, key: &str) -> Result<String, String> {
    let name = activate(data, user, key)?;
    crate::theme::save_choice(data, user, &format!("{CHOICE_PREFIX}{key}"));
    Ok(name)
}

#[cfg(test)]
#[path = "usertheme_tests.rs"]
mod tests;
