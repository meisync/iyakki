//! The sessions a paired computer has open on the device (a shell, a forwarded port, a followed log): kept in memory per data folder,
//! listed in Settings → Developer mode with a Disconnect button, and ended at once when the computer is revoked, a scope is taken
//! away or developer mode goes off. Whoever runs a session holds a [`Guard`]; it checks [`Guard::ended`] while it works.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::scopes::Scope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u64,
    pub computer: String,
    pub account: String,
    /// The scope it runs under (`shell`, `forward`, `debug`...).
    pub scope: &'static str,
    /// `shell`, `forward 6443`, `dmesg --follow`...
    pub what: String,
    pub started: i64,
}

struct Entry {
    info: SessionInfo,
    ended: Arc<AtomicBool>,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Vec<Entry>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Vec<Entry>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Vec<Entry>>> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

/// An open session; dropping it removes it from the list.
pub struct Guard {
    data: PathBuf,
    id: u64,
    ended: Arc<AtomicBool>,
}

impl Guard {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Has the administrator (or a revoke, or the switch-off) ended this session?
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(v) = lock().get_mut(&self.data) {
            v.retain(|e| e.info.id != self.id);
        }
    }
}

/// Most sessions one computer may have open at once.
pub const MAX_PER_COMPUTER: usize = 16;

/// Register a session. Refused when the computer already has [`MAX_PER_COMPUTER`].
pub fn open(data: &Path, computer: &str, account: &str, scope: Scope, what: &str, now: i64) -> Result<Guard, String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut r = lock();
    let v = r.entry(data.to_path_buf()).or_default();
    if v.iter().filter(|e| e.info.computer == computer).count() >= MAX_PER_COMPUTER {
        return Err(format!("{computer} has {MAX_PER_COMPUTER} sessions open already: close some first"));
    }
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let ended = Arc::new(AtomicBool::new(false));
    v.push(Entry { info: SessionInfo { id, computer: computer.into(), account: account.into(), scope: scope.id(), what: what.chars().filter(|c| !c.is_control()).take(80).collect(), started: now }, ended: ended.clone() });
    Ok(Guard { data: data.to_path_buf(), id, ended })
}

/// The open sessions, oldest first.
pub fn list(data: &Path) -> Vec<SessionInfo> {
    lock().get(data).map(|v| v.iter().map(|e| e.info.clone()).collect()).unwrap_or_default()
}

fn end_where(data: &Path, f: impl Fn(&SessionInfo) -> bool) -> usize {
    let mut n = 0;
    if let Some(v) = lock().get(data) {
        for e in v.iter().filter(|e| f(&e.info)) {
            e.ended.store(true, Ordering::SeqCst);
            n += 1;
        }
    }
    n
}

/// Disconnect one session (the Disconnect button).
pub fn end(data: &Path, id: u64) -> bool {
    end_where(data, |s| s.id == id) > 0
}

pub fn end_computer(data: &Path, computer: &str) -> usize {
    end_where(data, |s| s.computer == computer)
}

pub fn end_scope(data: &Path, computer: &str, scope: Scope) -> usize {
    end_where(data, |s| s.computer == computer && (s.scope == scope.id() || (scope == Scope::Shell && s.scope == Scope::Power.id())))
}

pub fn end_all(data: &Path) -> usize {
    end_where(data, |_| true)
}
