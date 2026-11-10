//! `input` scope: events a paired computer sends to the shell on the screen (`iyakki dev input tap|key|text|swipe`). They go into a
//! small queue the SHELL empties on its next beat and turns into its own pointer and key events: nothing is injected below the
//! shell (no `/dev/uinput`), so only what the shell would do for a person at the keyboard happens. While a password field has
//! focus (the shell says so with [`set_secret_focus`]) events are refused unless the computer also holds `input:password`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Biggest queue between the listener and the shell.
pub const MAX_QUEUE: usize = 256;
pub const MAX_TEXT: usize = 1000;
/// Coordinates beyond this are nonsense on any screen.
pub const MAX_COORD: i32 = 16384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEv {
    Tap(i32, i32),
    /// From, to, milliseconds.
    Swipe(i32, i32, i32, i32, u32),
    /// A named key: `Enter`, `Esc`, `Tab`, `Backspace`, `Delete`, `Up`..., `F1`..`F12`, `Ctrl+c`, or one character.
    Key(String),
    Text(String),
}

#[derive(Default)]
struct Hand {
    queue: HashMap<PathBuf, Vec<InputEv>>,
    secret: HashMap<PathBuf, bool>,
}

fn hand() -> std::sync::MutexGuard<'static, Hand> {
    static H: OnceLock<Mutex<Hand>> = OnceLock::new();
    H.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

const NAMED: [&str; 22] = ["Enter", "Esc", "Tab", "Backspace", "Delete", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "BackTab", "Space", "F1", "F2", "F3", "F4", "F5", "F6", "F7"];

/// The one spelling of a key name: names are not case-sensitive (`ctrl+a`, `BACKSPACE`, `f5`, `escape`, `return` all work);
/// a single character keeps its case (`A` is not `a`). `None` when it is no key the shell knows.
pub fn canonical_key(k: &str) -> Option<String> {
    if let Some(n) = NAMED.iter().find(|n| n.eq_ignore_ascii_case(k)) {
        return Some((*n).to_string());
    }
    match k.to_ascii_lowercase().as_str() {
        "escape" => return Some("Esc".into()),
        "return" => return Some("Enter".into()),
        "bksp" | "back" => return Some("Backspace".into()),
        "del" => return Some("Delete".into()),
        "pgup" => return Some("PageUp".into()),
        "pgdn" | "pgdown" => return Some("PageDown".into()),
        _ => {}
    }
    if k.len() > 1 && k.starts_with(['F', 'f']) {
        if let Ok(n) = k[1..].parse::<u8>() {
            return (1..=12).contains(&n).then(|| format!("F{n}"));
        }
    }
    if k.len() > 5 && k[..5].eq_ignore_ascii_case("ctrl+") {
        let mut it = k[5..].chars();
        return match (it.next(), it.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("Ctrl+{}", c.to_ascii_lowercase())),
            _ => None,
        };
    }
    (k.chars().count() == 1 && !k.chars().any(|c| c.is_control())).then(|| k.to_string())
}

/// Is this a key name the shell understands?
pub fn valid_key(k: &str) -> bool {
    canonical_key(k).is_some()
}

/// Check an event before it is queued.
pub fn validate(e: &InputEv) -> Result<(), String> {
    let ok = |v: i32| (0..=MAX_COORD).contains(&v);
    match e {
        InputEv::Tap(x, y) if ok(*x) && ok(*y) => Ok(()),
        InputEv::Swipe(a, b, c, d, ms) if ok(*a) && ok(*b) && ok(*c) && ok(*d) && (1..=5000).contains(ms) => Ok(()),
        InputEv::Tap(..) | InputEv::Swipe(..) => Err(format!("coordinates are 0 to {MAX_COORD} (swipes take 1 to 5000 ms)")),
        InputEv::Key(k) if valid_key(k) => Ok(()),
        InputEv::Key(k) => Err(format!("{k:?} is not a key name (Enter, Esc, Tab, Backspace, Delete, Up, Down, Left, Right, Home, End, PageUp, PageDown, F1..F12, Ctrl+x, or one character)")),
        InputEv::Text(t) if t.chars().count() <= MAX_TEXT && !t.is_empty() => Ok(()),
        InputEv::Text(_) => Err(format!("text is 1 to {MAX_TEXT} characters")),
    }
}

/// Queue events for the shell of `data_dir` (all or none).
pub fn push(data_dir: &Path, evs: &[InputEv]) -> Result<usize, String> {
    for e in evs {
        validate(e)?;
    }
    let mut h = hand();
    let q = h.queue.entry(data_dir.to_path_buf()).or_default();
    if q.len() + evs.len() > MAX_QUEUE {
        return Err("the shell has not caught up with the events already sent: slow down".into());
    }
    q.extend(evs.iter().cloned());
    Ok(evs.len())
}

/// The shell side: everything queued since last time.
pub fn take(data_dir: &Path) -> Vec<InputEv> {
    hand().queue.remove(data_dir).unwrap_or_default()
}

/// The shell side: does a password field have focus right now?
pub fn set_secret_focus(data_dir: &Path, on: bool) {
    hand().secret.insert(data_dir.to_path_buf(), on);
}

pub fn secret_focus(data_dir: &Path) -> bool {
    hand().secret.get(data_dir).copied().unwrap_or(false)
}
