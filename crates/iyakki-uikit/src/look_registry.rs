//! Which Looks a person can choose, and the choice kept per account (F-LOOK-05). No shell wiring yet.
//!
//! The list is the three built-in Looks, then the `.look` files (and, as Looks with only colours, the old `.theme` files) of
//! `<data>/themes/<person>/` and the shared `<data>/themes/`, with the same folder rules and caps as [`crate::usertheme`]. A built-in
//! key cannot be taken by a file; a person's own file shadows a shared one of the same key. A file that cannot be read is listed with
//! its reasons, not hidden. The choice is kept as the key `look` in `ui-<user>.json`, beside the theme, the wallpaper and the UI size;
//! the field is written only when the person chooses a Look, and a file without it keeps working as before.

use std::path::{Path, PathBuf};

use crate::look::{self, Accepted, Look, LookProblem, MAX_BYTES};
use crate::usertheme::{self, MAX_FILES};

/// The key in `ui-<user>.json`.
pub const CHOICE_KEY: &str = "look";

/// One line of the list: the key it is chosen by, and the Look or the reasons it was refused.
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub builtin: bool,
    pub result: Result<Accepted, Vec<LookProblem>>,
}

fn read_file(p: &Path) -> Result<Accepted, Vec<LookProblem>> {
    let one = |m: String| Err(vec![LookProblem { line: 0, message: m }]);
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_file() && m.len() <= MAX_BYTES => match std::fs::read_to_string(p) {
            Ok(t) => look::parse(&t),
            Err(e) => one(e.to_string()),
        },
        Ok(m) if m.is_file() => one(format!("the file is bigger than {MAX_BYTES} bytes")),
        _ => one("not a plain file".into()),
    }
}

fn list_dir(dir: &Path) -> Vec<(String, Result<Accepted, Vec<LookProblem>>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "look" || x == "theme")).collect();
    // `.look` before `.theme` for the same stem, so a Look file wins over an old theme file of the same name.
    files.sort_by(|a, b| (a.file_stem(), a.extension() != Some("look".as_ref())).cmp(&(b.file_stem(), b.extension() != Some("look".as_ref()))));
    files.truncate(MAX_FILES * 2);
    files.into_iter().map(|p| (p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(), read_file(&p))).collect()
}

/// Every Look this person can choose.
pub fn available(data: &Path, user: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Look::builtin()
        .into_iter()
        .map(|l| Entry { key: l.key(), builtin: true, result: look::validate(&l) })
        .collect();
    for (key, result) in crate::skins::shipped() {
        out.push(Entry { key: usertheme::key_of(&key), builtin: true, result });
    }
    for d in usertheme::folders(data, user) {
        for (key, result) in list_dir(&d) {
            if out.iter().any(|e| e.key == key) {
                if out.iter().any(|e| e.key == key && e.builtin) {
                    out.push(Entry {
                        key: format!("{key} (file)"),
                        builtin: false,
                        result: Err(vec![LookProblem { line: 0, message: format!("`{key}` is the name of a built-in Look; rename the file") }]),
                    });
                }
                continue;
            }
            out.push(Entry { key, builtin: false, result });
        }
    }
    out
}

/// One Look by its key, with what was put back; the reasons when it is not there or cannot be read.
pub fn load(data: &Path, user: &str, key: &str) -> Result<Accepted, Vec<LookProblem>> {
    available(data, user)
        .into_iter()
        .find(|e| e.key == key)
        .map(|e| e.result)
        .unwrap_or_else(|| Err(vec![LookProblem { line: 0, message: format!("there is no Look `{key}`") }]))
}

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

fn read_json(p: &Path) -> Option<serde_json::Value> {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
}

/// The key of the Look this person chose, None when they never chose one (then the shell uses [`Look::default`]).
pub fn load_choice(dir: &Path, user: &str) -> Option<String> {
    read_json(&path(dir, user)).and_then(|v| v[CHOICE_KEY].as_str().map(String::from))
}

/// Keep a choice beside the person's other display settings, leaving every other field alone.
pub fn save_choice(dir: &Path, user: &str, key: &str) {
    let p = path(dir, user);
    let mut v = read_json(&p).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v[CHOICE_KEY] = serde_json::json!(key);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// The Look to draw with: the saved choice when it is still there and still readable, else the default Look (a person is never left
/// without one, and a refused file never replaces the Look they had).
pub fn current(dir: &Path, user: &str) -> Accepted {
    load_choice(dir, user)
        .and_then(|k| load(dir, user, &k).ok())
        .unwrap_or_else(|| look::validate(&Look::default()).unwrap_or(Accepted { look: Look::default(), fixes: vec![] }))
}

/// Choose a Look: it is checked first, and only a Look that passes is kept as the choice. Err leaves the saved choice as it was.
pub fn choose(dir: &Path, user: &str, key: &str) -> Result<Accepted, Vec<LookProblem>> {
    let a = load(dir, user, key)?;
    save_choice(dir, user, key);
    Ok(a)
}

/// The key in `ui-<user>.json` that holds the apps the person pinned to the dock, in order.
pub const DOCK_KEY: &str = "dock_items";

/// The apps in the dock of a person who never chose and whose Look names none (the shell and the SDK dock port agree on this list).
pub const DEFAULT_DOCK_IDS: [&str; 6] = ["chat", "browser", "ide", "edit", "cli", "system"];

thread_local! {
    static CHANGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Settings (or a chat tool) changed this person's Look or dock: the shell reads them again at its next heartbeat. Same thread as the shell,
/// like the theme and the wallpaper choices.
pub fn notify_changed() {
    CHANGED.with(|c| c.set(true));
}

/// True once after [`notify_changed`].
pub fn take_changed() -> bool {
    CHANGED.with(|c| c.replace(false))
}

/// Forget the pinned list: the Look's list, or the shell's default set, applies again.
pub fn clear_dock(dir: &Path, user: &str) {
    let p = path(dir, user);
    if let Some(mut v) = read_json(&p).filter(|v| v.is_object()) {
        if let Some(o) = v.as_object_mut() {
            o.remove(DOCK_KEY);
        }
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// The apps this person pinned to the dock (None: they never did, so the Look's list, then the shell's own choice, applies).
pub fn load_dock(dir: &Path, user: &str) -> Option<Vec<String>> {
    read_json(&path(dir, user)).and_then(|v| v[DOCK_KEY].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
}

/// Keep the pinned apps beside the Look choice, leaving every other field alone. At most `look::MAX_LIST` names of the usual id shape.
pub fn save_dock(dir: &Path, user: &str, items: &[String]) {
    let p = path(dir, user);
    let ok: Vec<&String> = items.iter().filter(|i| !i.is_empty() && i.len() <= 32 && i.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))).take(look::MAX_LIST).collect();
    let mut v = read_json(&p).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v[DOCK_KEY] = serde_json::json!(ok);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Keep an edited Look as this person's own file (`<data>/themes/<person>/<key>.look`, the key made from its name) and choose it. The
/// Look is checked first (what it would put back is in the result); nothing is written when it does not pass or the name is a built-in's.
pub fn save_custom(data: &Path, user: &str, l: &Look) -> Result<Accepted, Vec<LookProblem>> {
    let a = look::validate(l)?;
    let key = a.look.key();
    if Look::builtin().iter().any(|b| b.key() == key) || crate::skins::is_shipped(&key) {
        return Err(vec![LookProblem { line: 0, message: format!("`{key}` is the name of a built-in Look; give yours another name") }]);
    }
    let dir = data.join("themes").join(user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect::<String>());
    std::fs::create_dir_all(&dir).map_err(|e| vec![LookProblem { line: 0, message: e.to_string() }])?;
    let file = dir.join(format!("{key}.look"));
    std::fs::write(&file, a.look.to_text()).map_err(|e| vec![LookProblem { line: 0, message: e.to_string() }])?;
    save_choice(data, user, &key);
    Ok(a)
}

#[cfg(test)]
#[path = "look_registry_tests.rs"]
mod tests;
