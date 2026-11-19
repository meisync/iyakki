//! The STANDING RESTART GRANT: the administrator ticks "Allow restarts from this computer without asking" for one paired computer
//! (Settings, Developer mode, or the chat tool `restart_grant`). From then on that computer may restart or switch off the device
//! without a card, also while the sign-in screen shows (nobody has to be there to answer).
//!
//! * **Time.** The grant has no time of its own: it lasts exactly as long as the developer connection (developer mode's own switch-off
//!   time, `State::until`; with no limit set, until developer mode is switched off). It ends with it: automatic switch-off, switching
//!   off by hand, revoking the computer, or the computer losing the `power` scope.
//! * **Keep until I switch it off (opt-in, `persist`).** The administrator may tick a second box. That grant does not end with the
//!   automatic switch-off: ticking it clears developer mode's switch-off time (the developer connection must stay up for the computer
//!   to reach the device at all, and the page says so in plain words), and [`DevMode::is_on`] never lets the timer end it. It ends only
//!   by hand: unticking, disconnecting/removing the computer, the computer losing `power`, or developer mode switched off by hand
//!   (which forgets every grant). When it ends and developer mode is still on, the default switch-off time (4 h) is set again so an
//!   ended grant never leaves an open-ended connection behind. Same scope, audit and restart-loop guard as above. See
//!   docs/development/RESTART-GRANT-PERSIST.md.
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
use super::{DevMode, State};
pub use iyakki_shared::devmode_types::restartgrant::{RestartView};

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
    /// Kept until the administrator switches it off (not ended by developer mode's automatic switch-off). Old files: false.
    #[serde(default)]
    pub persist: bool,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub(super) struct RestartFile {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    pub(super) grants: Vec<RestartGrant>,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}


/// The automatic switch-off set again when a persistent grant ends while developer mode stays on.
pub const FALLBACK_AUTO_OFF_HOURS: i64 = 4;

impl DevMode {
    pub(super) fn restart_load(&self) -> RestartFile {
        iyakki_schema::load_typed::<RestartFile>(&RESTART_SPEC, &self.dir().join(FILE), iyakki_schema::now_secs()).1
    }

    fn restart_save(&self, f: &RestartFile) -> Result<(), String> {
        iyakki_schema::save_typed(&RESTART_SPEC, &self.dir().join(FILE), f, 0o600)
    }

    pub(super) fn restart_load_has(&self, computer: &str) -> bool {
        self.restart_load().grants.iter().any(|g| g.computer == computer)
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
            .map(|g| RestartView { until: if g.persist { 0 } else { until }, persist: g.persist, computer: g.computer, by: g.by, awaiting_start: g.awaiting_start })
            .collect()
    }

    pub fn restart_grant_active(&self, computer: &str) -> bool {
        self.restart_grant_list().iter().any(|g| g.computer == computer)
    }

    /// True when developer mode is enabled and a persistent grant is in force (its computer still paired and holding `power`). Never
    /// calls [`DevMode::is_on`] (which asks this), so it is safe inside it.
    pub(super) fn restart_persist_valid(&self) -> bool {
        if !self.state().enabled {
            return false;
        }
        let keys = self.keys();
        self.restart_load().grants.iter().any(|g| g.persist && keys.iter().any(|k| k.name == g.computer && k.has(Scope::Power)))
    }

    /// Tick the box for `computer` (administrators, developer mode on, the computer holds `power`). Ends with the developer connection.
    /// Ticking this on a grant that was kept until switched off makes it an ordinary one again (the default switch-off time returns).
    pub fn restart_grant_on(&self, policy: &Policy, user: &str, computer: &str) -> Result<RestartView, String> {
        self.restart_grant_set(policy, user, computer, false)
    }

    /// Tick the box "keep until I switch it off": the grant, and the developer connection with it, stay until switched off by hand.
    pub fn restart_grant_persist(&self, policy: &Policy, user: &str, computer: &str) -> Result<RestartView, String> {
        self.restart_grant_set(policy, user, computer, true)
    }

    fn restart_grant_set(&self, policy: &Policy, user: &str, computer: &str, persist: bool) -> Result<RestartView, String> {
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
        let was_persist = keep.as_ref().is_some_and(|g| g.persist);
        f.grants.retain(|g| g.computer != computer);
        let mut g = keep.unwrap_or(RestartGrant { computer: computer.into(), by: user.into(), since: self.now(), ..Default::default() });
        g.persist = persist;
        f.grants.push(g);
        // the grant and the switch-off time are changed together: the time first, so a failure leaves no persistent grant
        let st = self.state();
        if persist && st.until != 0 {
            self.write("state.json", &State { until: 0, ..st })?;
        } else if !persist && was_persist {
            self.restart_restore_timer();
        }
        self.restart_save(&f)?;
        let until = self.state().until;
        let what = if persist { "until you switch it off (developer mode stays on until you switch it off)".to_string() } else if until > 0 { format!("until {}", super::stamp(until)) } else { "until developer mode is switched off".into() };
        self.log(user, user, if persist { "devmode.restart.grant_persist" } else { "devmode.restart.grant_on" }, &format!("{computer} may restart or switch off this device without asking {what}"), true);
        Ok(RestartView { computer: computer.into(), by: user.into(), until, awaiting_start: false, persist })
    }

    /// Developer mode is on with no switch-off time because a persistent grant asked for that, and no persistent grant is left: the
    /// default time comes back (from now).
    pub(super) fn restart_restore_timer(&self) {
        if self.trusted_kept_valid() {
            return;
        }
        let st = self.state();
        if st.enabled && st.until == 0 {
            let until = self.now() + FALLBACK_AUTO_OFF_HOURS * 3600;
            let _ = self.write("state.json", &State { until, ..st });
            self.log("system", "", "devmode.restart.timer_back", &format!("the kept restart grant ended: developer mode switches off by itself again at {}", super::stamp(until)), true);
        }
    }

    /// Persistent grants whose computer lost `power` (or is gone) end now; the switch-off time returns when none is left. Called with
    /// the keys lock NOT held.
    pub(super) fn restart_prune(&self) {
        let _one_at_a_time = super::keys_lock();
        let keys = self.keys();
        let mut f = self.restart_load();
        let n = f.grants.len();
        f.grants.retain(|g| !g.persist || keys.iter().any(|k| k.name == g.computer && k.has(Scope::Power)));
        if f.grants.len() != n {
            let _ = self.restart_save(&f);
            if !f.grants.iter().any(|g| g.persist) {
                self.restart_restore_timer();
            }
            self.log("system", "", "devmode.restart.grant_ended", "a kept restart grant ended: its computer no longer holds the power scope", true);
        }
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
        let was_persist = f.grants.iter().any(|g| g.computer == computer && g.persist);
        f.grants.retain(|g| g.computer != computer);
        if f.grants.len() != n {
            let _ = self.restart_save(&f);
            if was_persist && !f.grants.iter().any(|g| g.persist) {
                self.restart_restore_timer();
            }
        }
    }

    /// Called with the keys lock held (from `shell_forget_all`).
    pub(super) fn restart_forget_all(&self) {
        let _ = std::fs::remove_file(self.dir().join(FILE));
    }
}
