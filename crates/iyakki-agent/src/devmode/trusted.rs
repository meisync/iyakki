//! The TRUSTED DEV COMPUTER switch (owner decision 2026-11-16: "a dev machine should have all permissions that can be approved; waiting
//! for a human wastes time"). FOR DEV AND TEST MACHINES ONLY.
//!
//! One switch per paired computer, set by an administrator ON THE DEVICE (Settings, Developer mode, or the chat tool
//! `trusted_computer`, which is a hard ask: the person taps the card on the device; a remote request never switches it on). While it is on,
//! every approval card that this computer's requests would raise is answered yes by itself:
//!
//! | approval | kind | what it opens |
//! |---|---|---|
//! | run commands | `shell` | a shell grant (never as root: a request to run as root still gets its card) |
//! | patch / push-apps | `patch` | the files `iyakki dev patch`, `patch apply` and `push-apps` may change (apps program, tile apps, layers) |
//! | restart / switch off | `power` | `/v1/dev/power/*` |
//!
//! The update, forward, files, config and log routes have no approval card of their own: the scope list decides them, and the mode never
//! adds a scope. Not covered, because they are not approvals: the `input:password` scope, the vault and keys, anything the scopes list does
//! not grant to the computer, and a request to run as root.
//!
//! Two levels share the one store: [`TrustLevel::Everything`] (the switch above) and [`TrustLevel::Patches`] ("Patches without asking":
//! only the `patch` approval, nothing else). Off by default.
//!
//! * **Time.** `Kept` (the default offered: "keep until I switch it off"; clears developer mode's automatic switch-off like the kept restart
//!   grant, because the computer can only reach the device while developer mode is on) or 4 h / 12 h from now. A timed grant also ends with
//!   developer mode.
//! * **Ends.** Switching it off, removing the computer (its pairing; also pairing again under the same name), developer mode switched off
//!   by hand (forgets it) and expiry (logged). Each approval still checks the scope the computer holds, so losing a scope ends that part.
//! * **Audit.** Switching on and off, every auto-approval (scope and approval word only, never arguments) and expiry go to the developer-mode
//!   activity log. The top bar shows `DEV · TRUSTED COMPUTER <name>` ([`DevMode::trusted_banner`]).
//!
//! Stored in `devmode/trusted-computer.json`, a versioned store; unknown fields of a newer release are kept.

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use super::scopes::Scope;
use super::{DevMode, State};
pub use iyakki_shared::devmode_types::trusted::{TrustLevel, TrustFor, TrustedView};

pub const FILE: &str = "trusted-computer.json";

pub const TRUSTED_SPEC: iyakki_schema::Spec = iyakki_schema::Spec { name: "developer mode trusted computer", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the first layout", run: |_| {} }] };




#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Trusted {
    pub computer: String,
    pub by: String,
    pub since: i64,
    /// 0: kept until switched off.
    #[serde(default)]
    pub expires: i64,
    #[serde(default)]
    pub level: TrustLevel,
    #[serde(default)]
    pub uses: u32,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub(super) struct TrustedFile {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    pub(super) grants: Vec<Trusted>,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}



pub const MAX_HOURS: u32 = 24;

impl DevMode {
    pub(super) fn trusted_load(&self) -> TrustedFile {
        iyakki_schema::load_typed::<TrustedFile>(&TRUSTED_SPEC, &self.dir().join(FILE), iyakki_schema::now_secs()).1
    }

    fn trusted_save(&self, f: &TrustedFile) -> Result<(), String> {
        iyakki_schema::save_typed(&TRUSTED_SPEC, &self.dir().join(FILE), f, 0o600)
    }

    fn trusted_live(&self, f: TrustedFile) -> Vec<Trusted> {
        let now = self.now();
        let keys = self.keys();
        f.grants.into_iter().filter(|g| (g.expires == 0 || g.expires > now) && keys.iter().any(|k| k.name == g.computer)).collect()
    }

    /// The switches in force: developer mode on, the computer still paired, not expired.
    pub fn trusted_list(&self) -> Vec<TrustedView> {
        if !self.is_on() {
            return Vec::new();
        }
        self.trusted_live(self.trusted_load()).into_iter().map(|g| TrustedView { computer: g.computer, by: g.by, level: g.level, until: g.expires }).collect()
    }

    /// The switch of `computer` when it covers the approval `kind` (`shell`, `patch`, `power`).
    pub fn trusted_for(&self, computer: &str, kind: &str) -> Option<TrustedView> {
        self.trusted_list().into_iter().find(|t| t.computer == computer && t.level.covers(kind))
    }

    pub fn trusted_active(&self, computer: &str) -> bool {
        self.trusted_list().iter().any(|t| t.computer == computer)
    }

    /// A kept switch is in force: developer mode must not switch itself off. Never calls [`DevMode::is_on`] (which asks this).
    pub(super) fn trusted_kept_valid(&self) -> bool {
        if !self.state().enabled {
            return false;
        }
        self.trusted_live(self.trusted_load()).iter().any(|g| g.expires == 0)
    }

    /// Switch on (administrators, developer mode on, the computer paired). The caller is the hard ask: the device's own screen or the
    /// chat card. Replaces an earlier setting of the same computer.
    pub fn trusted_on(&self, policy: &Policy, user: &str, computer: &str, level: TrustLevel, choice: TrustFor) -> Result<TrustedView, String> {
        Self::need_admin(policy)?;
        if !self.is_on() {
            return Err("Developer mode is off: switch it on first; the trusted-computer switch lasts as long as it does.".into());
        }
        if let TrustFor::Hours(h) = choice {
            if h == 0 || h > MAX_HOURS {
                return Err(format!("the time is 1 to {MAX_HOURS} hours"));
            }
        }
        let _one_at_a_time = super::keys_lock();
        let key = self.keys().into_iter().find(|k| k.name == computer).ok_or("that computer is not paired")?;
        if level == TrustLevel::Everything && !key.has(Scope::Shell) && !key.has(Scope::Power) && !key.has(Scope::Update) {
            return Err(format!("{computer} holds no scope that an approval could open (shell, power or update)"));
        }
        let now = self.now();
        let expires = match choice {
            TrustFor::Kept => 0,
            TrustFor::Hours(h) => now + h as i64 * 3600,
        };
        let mut f = self.trusted_load();
        let had = f.grants.iter().find(|g| g.computer == computer).cloned();
        f.grants.retain(|g| g.computer != computer);
        f.grants.push(Trusted { computer: computer.into(), by: user.into(), since: now, expires, level, uses: had.map_or(0, |g| g.uses), ..Default::default() });
        // the connection must stay for a kept switch: the time first, so a failure leaves no kept switch
        let st = self.state();
        if choice == TrustFor::Kept && st.until != 0 {
            self.write("state.json", &State { until: 0, ..st })?;
        }
        self.trusted_save(&f)?;
        if choice != TrustFor::Kept {
            self.trusted_timer_back();
        }
        let what = match choice {
            TrustFor::Kept => "until you switch it off (developer mode stays on until you switch it off)".to_string(),
            TrustFor::Hours(h) => format!("for {h} h"),
        };
        let word = if level == TrustLevel::Patches { "patches" } else { "every approval" };
        self.log(user, user, "devmode.trusted.on", &format!("{computer}: {word} answered yes by itself {what}"), true);
        Ok(TrustedView { computer: computer.into(), by: user.into(), level, until: expires })
    }

    pub fn trusted_off(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        if self.trusted_forget(computer) {
            self.log(user, user, "devmode.trusted.off", &format!("{computer}: approvals are asked again"), true);
        }
        Ok(())
    }

    /// Removes the switch of `computer` (pairing removed, switched off). True when there was one.
    pub(super) fn trusted_forget(&self, computer: &str) -> bool {
        let _one_at_a_time = super::keys_lock();
        self.trusted_forget_locked(computer)
    }

    /// The same with the keys lock already held.
    pub(super) fn trusted_forget_locked(&self, computer: &str) -> bool {
        let mut f = self.trusted_load();
        let n = f.grants.len();
        f.grants.retain(|g| g.computer != computer);
        if f.grants.len() == n {
            return false;
        }
        let _ = self.trusted_save(&f);
        self.trusted_timer_back();
        true
    }

    /// Called with the keys lock held (developer mode switched off).
    pub(super) fn trusted_forget_all(&self) {
        let _ = std::fs::remove_file(self.dir().join(FILE));
    }

    /// No kept switch (and no kept restart grant) is left while developer mode is on: the default switch-off time comes back.
    fn trusted_timer_back(&self) {
        if !self.trusted_kept_valid() && !self.restart_persist_valid() {
            self.restart_restore_timer();
        }
    }

    /// An approval of `kind` was answered by the switch: one audit line (names only: the scope and the approval word, never arguments).
    pub fn trusted_note_use(&self, computer: &str, kind: &str, word: &str) {
        let _one_at_a_time = super::keys_lock();
        self.trusted_note_use_locked(computer, kind, word);
    }

    /// The same with the keys lock already held.
    pub(super) fn trusted_note_use_locked(&self, computer: &str, kind: &str, word: &str) {
        let mut f = self.trusted_load();
        if let Some(g) = f.grants.iter_mut().find(|g| g.computer == computer) {
            g.uses = g.uses.saturating_add(1);
            let _ = self.trusted_save(&f);
        }
        self.log(computer, "", "dev.trusted.auto", &format!("{kind} approval answered yes by the trusted-computer switch ({word})"), true);
    }

    /// The top-bar line: `TRUSTED COMPUTER laptop` (the bar shows `DEV · TRUSTED COMPUTER laptop`); only the patch level says `PATCHES
    /// WITHOUT ASKING`. Expired switches are swept (and logged) here.
    pub fn trusted_banner(&self) -> Option<String> {
        if !self.state().enabled {
            return None;
        }
        self.trusted_sweep();
        let l = self.trusted_list();
        let names = |lv: TrustLevel| l.iter().filter(|t| t.level == lv).map(|t| t.computer.clone()).collect::<Vec<_>>();
        let (all, patch) = (names(TrustLevel::Everything), names(TrustLevel::Patches));
        if !all.is_empty() {
            Some(format!("TRUSTED COMPUTER {}", all.join(", ")))
        } else if !patch.is_empty() {
            Some(format!("PATCHES WITHOUT ASKING {}", patch.join(", ")))
        } else {
            None
        }
    }

    fn trusted_sweep(&self) {
        let now = self.now();
        let f = self.trusted_load();
        if !f.grants.iter().any(|g| g.expires != 0 && g.expires <= now) {
            return;
        }
        let _one_at_a_time = super::keys_lock();
        let mut f = self.trusted_load();
        let gone: Vec<String> = f.grants.iter().filter(|g| g.expires != 0 && g.expires <= now).map(|g| g.computer.clone()).collect();
        f.grants.retain(|g| g.expires == 0 || g.expires > now);
        let _ = self.trusted_save(&f);
        for c in gone {
            self.log("system", "", "devmode.trusted.expired", &format!("{c}: the trusted-computer time is over; approvals are asked again"), true);
        }
    }

    /// One line for the status text of the device.
    pub fn trusted_status_line(&self) -> Option<String> {
        let l = self.trusted_list();
        if l.is_empty() {
            return None;
        }
        Some(l.iter().map(|t| format!("{} {}", if t.level == TrustLevel::Patches { "patches without asking:" } else { "TRUSTED COMPUTER:" }, if t.kept() { format!("{} (until switched off)", t.computer) } else { format!("{} (until {})", t.computer, super::stamp(t.until)) })).collect::<Vec<_>>().join("; "))
    }
}
