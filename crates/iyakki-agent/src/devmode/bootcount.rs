//! The boot counter of a system update (F-UPD-02): a new set of boot files is tried once, and when it was never marked good the boot
//! menu starts the previous version by itself.
//!
//! It is the boot menu's own mechanism and nothing more: the menu file (`grub.cfg`) gets a small guard that reads the variable
//! `iyakki_trial` from `boot/grub/grubenv` on the boot partition.
//!
//! * `1`: an update or a rollback was just written. The guard changes it to `2` and starts the normal entry (the new set).
//! * `2`: the new set was tried and nobody marked it good (it did not get as far as the first picture). The guard sets `0` and makes
//!   the "IYAKKI (previous version)" entry the default for this start.
//! * `0` or none: nothing to do.
//!
//! The running system sets `0` ([`mark_good`]) once the data volume is mounted and the shell has drawn its first picture. The guard is
//! only ever written together with the previous-version entry it falls back to, and [`arm`] refuses to count when the menu has no
//! guard or no such entry, so a machine is never left with a counter and nothing to fall back to. The environment file is the
//! 1024 byte block GRUB reads (`# GRUB Environment Block`, `name=value` lines, padded with `#`), replaced as a whole.
//! The GRUB build of the image must have the `loadenv` module; that is not yet checked on a real image.

use std::path::{Path, PathBuf};

use super::update::PREV_TITLE;

/// The environment block, relative to the boot partition.
pub const ENV_FILE: &str = "boot/grub/grubenv";
/// The variable the guard and the running system share.
pub const VAR: &str = "iyakki_trial";
const BEGIN: &str = "# iyakki boot counter: begin";
const END: &str = "# iyakki boot counter: end";
const HEADER: &str = "# GRUB Environment Block\n";
const BLOCK: usize = 1024;

/// What the counter says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing is being tried.
    Idle,
    /// Written by an update; the next start tries the new set.
    Armed,
    /// The new set was started and has not been marked good.
    Trying,
}

fn state_of(v: Option<&str>) -> State {
    match v {
        Some("1") => State::Armed,
        Some("2") => State::Trying,
        _ => State::Idle,
    }
}

/// The variables of an environment block, in file order.
pub fn parse_env(bytes: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The block for these variables: header, lines, `#` padding to exactly 1024 bytes. `None` when they do not fit.
pub fn format_env(vars: &[(String, String)]) -> Option<Vec<u8>> {
    let mut s = String::from(HEADER);
    for (k, v) in vars {
        s.push_str(&format!("{k}={v}\n"));
    }
    if s.len() > BLOCK {
        return None;
    }
    let mut b = s.into_bytes();
    b.resize(BLOCK, b'#');
    Some(b)
}

fn env_path(root: &Path) -> PathBuf {
    root.join(ENV_FILE)
}

/// The counter on the boot partition mounted at `root`.
pub fn read_state(root: &Path) -> State {
    let vars = std::fs::read(env_path(root)).map(|b| parse_env(&b)).unwrap_or_default();
    state_of(vars.iter().find(|(k, _)| k == VAR).map(|(_, v)| v.as_str()))
}

fn write_state(root: &Path, value: &str) -> Result<(), String> {
    let p = env_path(root);
    let mut vars = std::fs::read(&p).map(|b| parse_env(&b)).unwrap_or_default();
    vars.retain(|(k, _)| k != VAR);
    vars.push((VAR.to_string(), value.to_string()));
    let bytes = format_env(&vars).ok_or("the boot environment block is full")?;
    let tmp = p.with_file_name("grubenv.new");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::File::open(&tmp).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    if let Some(d) = p.parent() {
        if let Ok(f) = std::fs::File::open(d) {
            let _ = f.sync_all();
        }
    }
    Ok(())
}

/// Position of the previous-version entry among the menu entries (what GRUB's `default` counts), if the menu has one.
fn prev_index(cfg: &str) -> Option<usize> {
    let at = cfg.find(&format!("menuentry \"{PREV_TITLE}"))?;
    Some(cfg[..at].lines().filter(|l| l.starts_with("menuentry ")).count())
}

fn guard(index: usize) -> String {
    format!(
        "{BEGIN}\n# a new set is tried once; when it was never marked good the previous version starts (docs/UPDATES.md)\nload_env {VAR}\nif [ \"${{{VAR}}}\" = \"1\" ]; then\n\tset {VAR}=2\n\tsave_env {VAR}\nelif [ \"${{{VAR}}}\" = \"2\" ]; then\n\tset {VAR}=0\n\tsave_env {VAR}\n\tset default={index}\nfi\n{END}\n"
    )
}

/// True when the menu has the guard.
pub fn has_guard(cfg: &str) -> bool {
    cfg.contains(BEGIN) && cfg.contains(END)
}

/// `cfg` with the guard before the first entry (an older guard is replaced). The menu must already have the previous-version entry:
/// without it there would be nothing to fall back to, so this is an error and the menu is left as it was.
pub fn with_guard(cfg: &str) -> Result<String, String> {
    let mut text = without_guard(cfg);
    let index = prev_index(&text).ok_or("the boot menu has no previous-version entry to fall back to, so no boot counter is added")?;
    let at = text.lines().scan(0usize, |pos, l| {
        let at = *pos;
        *pos += l.len() + 1;
        Some((at, l))
    }).find(|(_, l)| l.starts_with("menuentry ")).map(|(at, _)| at).ok_or("the boot menu has no entries")?;
    text.insert_str(at, &guard(index));
    Ok(text)
}

/// `cfg` without the guard.
pub fn without_guard(cfg: &str) -> String {
    match (cfg.find(BEGIN), cfg.find(END)) {
        (Some(a), Some(b)) if b > a => {
            let end = cfg[b..].find('\n').map_or(cfg.len(), |n| b + n + 1);
            format!("{}{}", &cfg[..a], &cfg[end..])
        }
        _ => cfg.to_string(),
    }
}

/// Start counting on the next start. Refused unless the menu on this partition has the guard and the previous-version entry.
pub fn arm(root: &Path) -> Result<(), String> {
    let cfg_path = find_cfg(root).ok_or("the boot menu is missing from the boot partition")?;
    let cfg = std::fs::read_to_string(cfg_path).map_err(|e| e.to_string())?;
    if !has_guard(&cfg) || prev_index(&cfg).is_none() {
        return Err("the boot menu has no boot counter and previous-version entry, so a failed start would not fall back by itself".into());
    }
    write_state(root, "1")
}

/// The running system is good: stop counting. Returns whether anything was waiting. Does nothing when the counter is idle.
pub fn mark_good(root: &Path) -> Result<bool, String> {
    if read_state(root) == State::Idle {
        return Ok(false);
    }
    write_state(root, "0")?;
    Ok(true)
}

fn find_cfg(root: &Path) -> Option<PathBuf> {
    super::update::find_ci(root, "boot/grub/grub.cfg")
}
