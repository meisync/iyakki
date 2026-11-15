//! The STANDING RESTART GRANT: the administrator ticks "Allow restarts from this computer without asking" for one paired computer
//! (Settings, Developer mode, or the chat tool `restart_grant`). From then on that computer may restart or switch off the device
//! without a card, also while the sign-in screen shows (nobody has to be there to answer).
//!
//! * **Time.** The grant has no time of its own: it lasts exactly as long as the developer connection (developer mode's own switch-off
//!   time, `State::until`; with no limit set, until developer mode is switched off). It ends with it: automatic switch-off, switching
//!   off by hand, revoking the computer, or the computer losing the `power` scope.
//! * **Scope.** Restart and switch-off only (`/v1/dev/power/*`). It never runs a command, never gives root, never patches or updates,
//!   and a computer that does not hold the `power` scope cannot be given it.
//! * **Audit.** Ticking, unticking, every use and every refusal go to the developer-mode activity log; the route also writes the
//!   device's audit log line.
//! * **Successful start.** A use marks the grant `awaiting_start`. The next use is refused until the device reports a successful start
//!   ([`DevMode::restart_started`], called when the shell has drawn its first picture after the restart). A restart loop (a start that
//!   never gets as far as the first picture) therefore cannot be driven by the computer.
//!
//! Stored in `devmode/restart-grant.json`, a versioned store ([`RESTART_SPEC`], field `schema`); unknown fields of a newer release are
//! kept (docs/development/CORE-COMPATIBILITY-DESIGN.md).

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use super::scopes::Scope;
use super::DevMode;

pub const FILE: &str = "restart-grant.json";

/// Schema 1: the first layout.
pub const RESTART_SPEC: iyakki_schema::Spec = iyakki_schema::Spec { name: "developer mode restart grant", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the first layout", run: |_| {} }] };

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct RestartGrant {
    pub computer: String,
    pub by: String,
    pub since: i64,
    /// A restart was done under this grant and no successful start has been reported since.
    #[serde(default)]
    pub awaiting_start: bool,
    #[serde(default)]
    pub last_restart: i64,
    #[serde(default)]
    pub uses: u32,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct RestartFile {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    grants: Vec<RestartGrant>,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

/// A grant as shown: with the moment it ends (0: when developer mode is switched off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartView {
    pub computer: String,
    pub by: String,
    pub until: i64,
    pub awaiting_start: bool,
}

impl DevMode {
    fn restart_load(&self) -> RestartFile {
        iyakki_schema::load_typed::<RestartFile>(&RESTART_SPEC, &self.dir().join(FILE), iyakki_schema::now_secs()).1
    }

    fn restart_save(&self, f: &RestartFile) -> Result<(), String> {
        iyakki_schema::save_typed(&RESTART_SPEC, &self.dir().join(FILE), f, 0o600)
    }

    /// The grants in force: developer mode on, the computer still paired and still holding `power`.
    pub fn restart_grant_list(&self) -> Vec<RestartView> {
        if !self.is_on() {
            return Vec::new();
        }
        let until = self.state().until;
        let keys = self.keys();
        self.restart_load()
            .grants
            .into_iter()
            .filter(|g| keys.iter().any(|k| k.name == g.computer && k.has(Scope::Power)))
            .map(|g| RestartView { computer: g.computer, by: g.by, until, awaiting_start: g.awaiting_start })
            .collect()
    }

    pub fn restart_grant_active(&self, computer: &str) -> bool {
        self.restart_grant_list().iter().any(|g| g.computer == computer)
    }

    /// Tick the box for `computer` (administrators, developer mode on, the computer holds `power`). Ends with the developer connection.
    pub fn restart_grant_on(&self, policy: &Policy, user: &str, computer: &str) -> Result<RestartView, String> {
        Self::need_admin(policy)?;
        if !self.is_on() {
            return Err("Developer mode is off: switch it on first; the restart grant lasts as long as it does.".into());
        }
        let _one_at_a_time = super::keys_lock();
        let key = self.keys().into_iter().find(|k| k.name == computer).ok_or("that computer is not paired")?;
        if !key.has(Scope::Power) {
            return Err(format!("{computer} does not hold the power scope"));
        }
        let mut f = self.restart_load();
        let keep = f.grants.iter().find(|g| g.computer == computer).cloned();
        f.grants.retain(|g| g.computer != computer);
        f.grants.push(keep.unwrap_or(RestartGrant { computer: computer.into(), by: user.into(), since: self.now(), ..Default::default() }));
        self.restart_save(&f)?;
        let until = self.state().until;
        self.log(user, user, "devmode.restart.grant_on", &format!("{computer} may restart or switch off this device without asking until {}", if until > 0 { super::stamp(until) } else { "developer mode is switched off".into() }), true);
        Ok(RestartView { computer: computer.into(), by: user.into(), until, awaiting_start: false })
    }

    pub fn restart_grant_off(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        self.restart_forget(computer);
        self.log(user, user, "devmode.restart.grant_off", &format!("{computer}: restarts need approval again"), true);
        Ok(())
    }

    /// A restart or switch-off is asked for under the grant. Ok(who ticked it) marks the grant used; the next use waits for
    /// [`DevMode::restart_started`]. Every refusal is logged too.
    pub fn restart_grant_use(&self, computer: &str, what: &str) -> Result<String, String> {
        let refuse = |why: &str| {
            self.log(computer, "", "devmode.restart.refused", why, false);
            Err(why.to_string())
        };
        // checked before the lock: finding developer mode timed out switches it off, which takes the lock itself
        if !self.restart_grant_active(computer) {
            return refuse("no standing restart grant");
        }
        let _one_at_a_time = super::keys_lock();
        let mut f = self.restart_load();
        let Some(g) = f.grants.iter_mut().find(|g| g.computer == computer) else { return refuse("no standing restart grant") };
        if g.awaiting_start {
            return refuse("the device has not reported a successful start since the last restart under this grant; ask for an approval on the device's screen instead");
        }
        g.awaiting_start = true;
        g.last_restart = self.now();
        g.uses += 1;
        let by = g.by.clone();
        self.restart_save(&f)?;
        self.log(computer, "", "devmode.restart.use", &format!("{what} without a card (standing grant of {by})"), true);
        Ok(by)
    }

    /// The device reports a successful start (the shell drew its first picture): grants waiting for it can be used again.
    pub fn restart_started(&self) {
        let _one_at_a_time = super::keys_lock();
        let mut f = self.restart_load();
        if !f.grants.iter().any(|g| g.awaiting_start) {
            return;
        }
        for g in f.grants.iter_mut().filter(|g| g.awaiting_start) {
            g.awaiting_start = false;
            self.log("system", "", "devmode.restart.started", &format!("{}: the device started successfully after the restart; the grant can be used again", g.computer), true);
        }
        let _ = self.restart_save(&f);
    }

    pub(super) fn restart_forget(&self, computer: &str) {
        let _one_at_a_time = super::keys_lock();
        let mut f = self.restart_load();
        let n = f.grants.len();
        f.grants.retain(|g| g.computer != computer);
        if f.grants.len() != n {
            let _ = self.restart_save(&f);
        }
    }

    /// Called with the keys lock held (from `shell_forget_all`).
    pub(super) fn restart_forget_all(&self) {
        let _ = std::fs::remove_file(self.dir().join(FILE));
    }
}
