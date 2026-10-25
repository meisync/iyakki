//! The SHELL GRANT: running commands is the most dangerous power a paired computer can ask for, so holding the `shell` scope only
//! lets it ASK. Each time, the computer files a request ([`DevMode::shell_request`]), the administrator sees it on the device's
//! screen (Settings → Developer mode, and a sign in the top bar) and approves it for N minutes or for one session
//! ([`DevMode::shell_approve`]), or denies it. While a grant is valid the top bar shows `SHELL ACCESS by <computer>`. Requests wait
//! two minutes. A grant never outlives its time, a revoke, a scope change or developer mode being switched off.

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use super::{DevKey, DevMode};

/// How long a request waits for the person at the screen.
pub const REQUEST_SECS: i64 = 120;
/// The longest a computer may ask for, and the longest a "once" grant lasts.
pub const MAX_MINUTES: u32 = 240;
pub const ONCE_MAX_SECS: i64 = 3600;

fn kind_text(kind: &str) -> &'static str {
    match kind {
        "power" => "restart or switch off the device",
        "patch" => "patch the running system (iyakki dev patch)",
        _ => "run commands",
    }
}

fn word_of(kind: &str) -> &'static str {
    match kind {
        "power" => "POWER",
        "patch" => "PATCH",
        _ => "SHELL",
    }
}

fn shell_kind() -> String {
    "shell".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ShellRequest {
    /// `shell` (run commands) or `power` (restart / switch off only).
    #[serde(default = "shell_kind")]
    pub kind: String,
    pub computer: String,
    pub account: String,
    /// Asked to run as root (only an account named root can get it).
    pub as_root: bool,
    pub minutes: u32,
    pub why: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ShellGrant {
    #[serde(default = "shell_kind")]
    pub kind: String,
    pub computer: String,
    pub account: String,
    pub as_root: bool,
    /// For one session: ends when the first session of this grant closes.
    pub once: bool,
    pub expires: i64,
    pub by: String,
    pub since: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct ShellState {
    pending: Vec<ShellRequest>,
    grants: Vec<ShellGrant>,
    /// Computers whose last request was denied, and when (so the waiting computer hears it).
    denied: Vec<(String, i64)>,
}

/// Where a computer's request stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellStatus {
    None,
    Pending,
    Denied,
    Granted(ShellGrant),
}

impl DevMode {
    fn shell_read(&self) -> ShellState {
        let mut st: ShellState = self.read("shell.json");
        let now = self.now();
        st.pending.retain(|r| now - r.at < REQUEST_SECS);
        st.grants.retain(|g| g.expires > now);
        st.denied.retain(|(_, t)| now - *t < REQUEST_SECS);
        st
    }

    fn shell_write(&self, st: &ShellState) -> Result<(), String> {
        self.write("shell.json", st)
    }

    /// A computer asks to run commands. Its `shell` scope has been checked by the caller.
    pub fn shell_request(&self, k: &DevKey, as_root: bool, minutes: u32, why: &str) -> Result<ShellStatus, String> {
        self.grant_request(k, "shell", as_root, minutes, why)
    }

    /// Ask for a grant of `kind` (`shell`, or `power` for a computer that may only restart or switch off the device).
    pub fn grant_request(&self, k: &DevKey, kind: &str, as_root: bool, minutes: u32, why: &str) -> Result<ShellStatus, String> {
        if !matches!(kind, "shell" | "power" | "patch") {
            return Err("kind: shell, power or patch".into());
        }
        let _one_at_a_time = super::keys_lock();
        let mut st = self.shell_read();
        if let Some(g) = st.grants.iter().find(|g| g.computer == k.name) {
            return Ok(ShellStatus::Granted(g.clone()));
        }
        st.denied.retain(|(c, _)| c != &k.name);
        st.pending.retain(|r| r.computer != k.name);
        if st.pending.len() >= 8 {
            return Err("too many shell requests are waiting on the device's screen".into());
        }
        let why: String = why.chars().map(|c| if c.is_control() { ' ' } else { c }).take(120).collect();
        st.pending.push(ShellRequest { kind: kind.to_string(), computer: k.name.clone(), account: k.account.clone(), as_root: as_root && k.account == "root", minutes: minutes.clamp(1, MAX_MINUTES), why: why.clone(), at: self.now() });
        self.shell_write(&st)?;
        self.log(&k.name, &k.account, "dev.shell.request", &format!("asks to {} for {} min{}{}", kind_text(kind), minutes.clamp(1, MAX_MINUTES), if as_root && k.account == "root" { " as root" } else { "" }, if why.is_empty() { String::new() } else { format!(": {why}") }), true);
        Ok(ShellStatus::Pending)
    }

    pub fn shell_status(&self, computer: &str) -> ShellStatus {
        let st = self.shell_read();
        if let Some(g) = st.grants.iter().find(|g| g.computer == computer) {
            return ShellStatus::Granted(g.clone());
        }
        if st.pending.iter().any(|r| r.computer == computer) {
            return ShellStatus::Pending;
        }
        if st.denied.iter().any(|(c, _)| c == computer) {
            return ShellStatus::Denied;
        }
        ShellStatus::None
    }

    /// The valid grant of a computer, if any.
    pub fn shell_grant(&self, computer: &str) -> Option<ShellGrant> {
        match self.shell_status(computer) {
            ShellStatus::Granted(g) => Some(g),
            _ => None,
        }
    }

    /// Requests waiting for the person at the screen.
    pub fn shell_pending(&self) -> Vec<ShellRequest> {
        self.shell_read().pending
    }

    /// Grants in force now.
    pub fn shell_grants(&self) -> Vec<ShellGrant> {
        self.shell_read().grants
    }

    /// Approve the waiting request of `computer` (administrators): for `minutes` (1 to 240), or 0 for one session.
    pub fn shell_approve(&self, policy: &Policy, user: &str, computer: &str, minutes: u32) -> Result<ShellGrant, String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut st = self.shell_read();
        let pos = st.pending.iter().position(|r| r.computer == computer).ok_or_else(|| format!("{computer} has no request waiting (it may have timed out: ask again from the computer)"))?;
        let r = st.pending.remove(pos);
        let key = self.keys().into_iter().find(|k| k.name == computer).ok_or("that computer is no longer paired")?;
        let need = match r.kind.as_str() {
            "power" => super::scopes::Scope::Power,
            "patch" => super::scopes::Scope::Update,
            _ => super::scopes::Scope::Shell,
        };
        if !key.has(need) {
            return Err(format!("{computer} does not hold the {} scope any more", need.id()));
        }
        let now = self.now();
        let once = minutes == 0;
        let secs = if once { ONCE_MAX_SECS } else { minutes.min(MAX_MINUTES) as i64 * 60 };
        let g = ShellGrant { kind: r.kind.clone(), computer: computer.into(), account: r.account, as_root: r.as_root, once, expires: now + secs, by: user.into(), since: now };
        st.grants.retain(|x| x.computer != computer);
        st.grants.push(g.clone());
        self.shell_write(&st)?;
        if g.kind == "patch" {
            // the first approval is remembered for this computer: later patches need no card (wide paths and free steps still do)
            self.patch_trust(computer);
        }
        self.log(user, user, "devmode.shell.grant", &format!("{computer} may {} {}{}", kind_text(&g.kind), if once { "for one session".to_string() } else { format!("for {} min", secs / 60) }, if g.as_root { " as root" } else { "" }), true);
        Ok(g)
    }

    pub fn shell_deny(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut st = self.shell_read();
        st.pending.retain(|r| r.computer != computer);
        st.denied.retain(|(c, _)| c != computer);
        st.denied.push((computer.into(), self.now()));
        self.shell_write(&st)?;
        self.log(user, user, "devmode.shell.deny", &format!("{computer} was refused shell access"), true);
        Ok(())
    }

    /// End a grant early and close the computer's shell sessions (administrators).
    pub fn shell_end(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        self.shell_drop(computer);
        super::sessions::end_scope(&self.data, computer, super::scopes::Scope::Shell);
        self.log(user, user, "devmode.shell.end", &format!("{computer}: shell access ended"), true);
        Ok(())
    }

    fn shell_drop(&self, computer: &str) {
        let _one_at_a_time = super::keys_lock();
        let mut st = self.shell_read();
        st.grants.retain(|g| g.computer != computer);
        st.pending.retain(|r| r.computer != computer);
        let _ = self.shell_write(&st);
    }

    /// A shell session of `computer` closed: a one-session grant ends with it.
    pub fn shell_session_closed(&self, computer: &str) {
        if self.shell_grant(computer).is_some_and(|g| g.once) {
            self.shell_drop(computer);
            self.log("system", "", "devmode.shell.end", &format!("{computer}: the one-session grant ended with its session"), true);
        }
    }

    pub(super) fn shell_forget(&self, computer: &str) {
        self.shell_drop(computer);
        self.patch_untrust(computer);
    }

    pub(super) fn shell_forget_all(&self) {
        let _one_at_a_time = super::keys_lock();
        let _ = std::fs::remove_file(self.dir().join("shell.json"));
        let _ = std::fs::remove_file(self.dir().join("patch-trust.json"));
    }

    /// `SHELL ACCESS by laptop` while a grant is in force (for the top bar), `SHELL REQUEST from laptop` while one waits.
    pub fn shell_banner(&self) -> Option<String> {
        let st = self.shell_read();
        if let Some(g) = st.grants.first() {
            let word = word_of(&g.kind);
            return Some(if st.grants.len() > 1 { format!("{word} ACCESS by {} computers", st.grants.len()) } else { format!("{word} ACCESS by {}", g.computer) });
        }
        st.pending.first().map(|r| format!("{} REQUEST from {}", word_of(&r.kind), r.computer))
    }
}

impl DevMode {
    /// A one-time power grant is used up by the action it was given for.
    pub fn power_used(&self, computer: &str) {
        self.shell_session_closed(computer);
    }
}

impl DevMode {
    /// The denial was reported to the computer: forget it.
    pub fn shell_clear_denied(&self, computer: &str) {
        let _one_at_a_time = super::keys_lock();
        let mut st = self.shell_read();
        st.denied.retain(|(c, _)| c != computer);
        let _ = self.shell_write(&st);
    }
}

impl DevMode {
    /// Has an administrator approved patching for this computer once (the approval card is shown the first time only)?
    pub fn patch_trusted(&self, computer: &str) -> bool {
        self.read::<Vec<String>>("patch-trust.json").iter().any(|c| c == computer)
    }

    /// Called with the lock held (from the approval).
    fn patch_trust(&self, computer: &str) {
        let mut v: Vec<String> = self.read("patch-trust.json");
        if !v.iter().any(|c| c == computer) {
            v.push(computer.into());
            let _ = self.write("patch-trust.json", &v);
        }
    }

    /// Revoking a computer (or switching developer mode off) forgets the approval: a new computer with the same name must be approved again.
    fn patch_untrust(&self, computer: &str) {
        let _one_at_a_time = super::keys_lock();
        let mut v: Vec<String> = self.read("patch-trust.json");
        v.retain(|c| c != computer);
        let _ = self.write("patch-trust.json", &v);
    }
}
