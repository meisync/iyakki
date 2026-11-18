//! `input` scope: events a paired computer sends to the shell on the screen (`iyakki dev input tap|key|text|swipe`). They go into a
//! small queue the SHELL empties on its next beat and turns into its own pointer and key events: nothing is injected below the
//! shell (no `/dev/uinput`), so only what the shell would do for a person at the keyboard happens. While a password field has
//! focus (the shell says so with [`set_secret_focus`]) events are refused unless the computer also holds `input:password`.
//! While an approval card is up on the screen (developer-mode shell/reboot/update ask, the power cards, a flow's permission ask) the
//! shell says so with [`set_approval_card`] and the events are refused with [`APPROVAL_SENTENCE`]: those are answered by a person
//! at the device, never by a paired computer. The shell also drops, and logs, anything that was queued before the card came up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub use iyakki_shared::devmode::input::{canonical_key, valid_key, InputEv};

/// Biggest queue between the listener and the shell.
pub const MAX_QUEUE: usize = 256;
pub const MAX_TEXT: usize = 1000;
/// Coordinates beyond this are nonsense on any screen.
pub const MAX_COORD: i32 = 16384;

#[derive(Default)]
struct Hand {
    queue: HashMap<PathBuf, Vec<InputEv>>,
    secret: HashMap<PathBuf, bool>,
    approval: HashMap<PathBuf, bool>,
}

fn hand() -> std::sync::MutexGuard<'static, Hand> {
    static H: OnceLock<Mutex<Hand>> = OnceLock::new();
    H.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
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

/// What a paired computer is told when its input meets an approval card.
pub use iyakki_shared::devmode::input::APPROVAL_SENTENCE;

/// The shell side: is an approval card (dev ask, power card, permission ask) up right now?
pub fn set_approval_card(data_dir: &Path, on: bool) {
    hand().approval.insert(data_dir.to_path_buf(), on);
}

pub fn approval_card(data_dir: &Path) -> bool {
    hand().approval.get(data_dir).copied().unwrap_or(false)
}
