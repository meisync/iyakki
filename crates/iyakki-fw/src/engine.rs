//! The firewall's brain: holds the settings, applies them through a [`Backend`], keeps the 60-second "keep these rules?"
//! promise, refreshes host names, ends expired rules and says plainly whether the machine is protected.
//!
//! Everything it depends on (the kernel, the clock, DNS, the file, the audit log) is a trait, so the tests run it with
//! fakes and a clock they move by hand.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::{Backend, BackendError};
use crate::model::{Config, Kind, Resolved};
use crate::ruleset::{self, Built, Ctx, Packet};
use crate::validate::{check_rule, normalize_host};

/// How long a changed ruleset waits for "Keep" before it is taken back.
pub const CONFIRM_SECS: i64 = 60;
pub const MAX_ADDRS_PER_NAME: usize = 16;

pub trait Clock: Send {
    fn now(&self) -> i64;
}

pub trait Resolver: Send {
    fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String>;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    pub config: Config,
    pub resolved: Resolved,
}

pub trait Store: Send {
    /// `Ok(None)`: nothing saved yet. `Err`: a file that cannot be read (the engine then uses the safe defaults and says so).
    fn load(&self) -> Result<Option<Saved>, String>;
    fn save(&self, s: &Saved) -> Result<(), String>;
}

pub type AuditFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Switched off by the administrator.
    Off,
    /// The boot parameter `iyakki.firewall=off` skipped the rules for this boot.
    OffThisBoot,
    On,
    /// Applied, waiting for Keep.
    Pending { secs_left: i64 },
    /// Wanted but not in force: said in red.
    NotProtected(String),
    /// The kernel cannot do it.
    Unavailable(String),
}

impl Status {
    pub fn headline(&self) -> &'static str {
        match self {
            Status::Off | Status::OffThisBoot => "OFF",
            Status::On | Status::Pending { .. } => "ON",
            Status::NotProtected(_) | Status::Unavailable(_) => "NOT PROTECTED",
        }
    }
    pub fn protected(&self) -> bool {
        matches!(self, Status::On | Status::Pending { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleState {
    Active,
    Disabled,
    Expired,
    /// A name with no address yet.
    WaitingForDns,
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostView {
    pub name: String,
    pub addrs: Vec<IpAddr>,
    pub last_ok: Option<i64>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug)]
pub struct View {
    pub status: Status,
    pub config: Config,
    pub rule_states: Vec<(u32, RuleState)>,
    pub hosts: Vec<HostView>,
    pub warnings: Vec<String>,
    pub ssh_boot: bool,
    pub last_error: Option<String>,
    pub events: Vec<String>,
}

struct Pending {
    deadline: i64,
    previous: Config,
}

pub struct Engine {
    backend: Box<dyn Backend>,
    clock: Box<dyn Clock>,
    resolver: Box<dyn Resolver>,
    store: Box<dyn Store>,
    audit: Option<AuditFn>,
    /// Audit events made before a sink was set.
    backlog: Vec<(String, String)>,
    ctx: Ctx,
    /// The settings in force (maybe not yet confirmed).
    cfg: Config,
    committed: Config,
    resolved: Resolved,
    warnings: BTreeMap<String, String>,
    pending: Option<Pending>,
    /// Is our table in the kernel?
    installed: bool,
    status: Status,
    last_error: Option<String>,
    boot_off: bool,
    next_refresh: i64,
    /// Earliest `expires` among applied rules (0: none).
    next_expiry: i64,
    events: Vec<String>,
    load_note: Option<String>,
    /// Look new names up inside `propose` (tests); the real service does it in its own thread.
    pub sync_dns: bool,
    booted: bool,
}

impl Engine {
    pub fn new(backend: Box<dyn Backend>, clock: Box<dyn Clock>, resolver: Box<dyn Resolver>, store: Box<dyn Store>) -> Engine {
        let mut e = Engine {
            backend,
            clock,
            resolver,
            store,
            audit: None,
            backlog: Vec::new(),
            ctx: Ctx::default(),
            cfg: Config::default(),
            committed: Config::default(),
            resolved: Resolved::default(),
            warnings: BTreeMap::new(),
            pending: None,
            installed: false,
            status: Status::Off,
            last_error: None,
            boot_off: false,
            next_refresh: 0,
            next_expiry: 0,
            events: Vec::new(),
            load_note: None,
            sync_dns: true,
            booted: false,
        };
        match e.store.load() {
            Ok(Some(s)) => {
                e.cfg = s.config.clone();
                e.committed = s.config;
                e.resolved = s.resolved;
            }
            Ok(None) => {}
            Err(err) => e.load_note = Some(format!("The saved firewall settings could not be read ({err}); the safe defaults are used: everything incoming is blocked.")),
        }
        e
    }

    pub fn set_audit(&mut self, f: AuditFn) {
        for (ev, d) in self.backlog.drain(..) {
            f(&ev, &d);
        }
        self.audit = Some(f);
    }

    fn note(&mut self, event: &str, detail: &str) {
        let line = format!("{} {event} {detail}", self.clock.now());
        self.events.push(line);
        if self.events.len() > 50 {
            self.events.remove(0);
        }
        match &self.audit {
            Some(f) => f(event, detail),
            None => self.backlog.push((event.to_string(), detail.to_string())),
        }
    }

    /// Facts about the machine (kernel command line, IPv6, cloud mode, web port). Changing them re-applies.
    pub fn set_ctx(&mut self, ctx: Ctx) {
        self.ctx = ctx;
    }

    /// The engine's clock (unix seconds).
    pub fn now(&self) -> i64 {
        self.clock.now()
    }

        pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    fn build(&self) -> Result<Built, String> {
        let mut ctx = self.ctx.clone();
        ctx.now = self.clock.now();
        ruleset::build(&self.cfg, &self.resolved, &ctx)
    }

    /// Put the settings in `self.cfg` into the kernel (or take the table out when off).
    fn apply_current(&mut self) -> Result<(), String> {
        if !self.cfg.enabled {
            return match self.backend.remove() {
                Ok(()) => {
                    self.installed = false;
                    self.next_expiry = 0;
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            };
        }
        let built = self.build()?;
        self.next_expiry = self.cfg.rules.iter().filter(|r| r.enabled && r.expires > self.clock.now()).map(|r| r.expires).min().unwrap_or(0);
        match self.backend.replace(&built.ruleset) {
            Ok(()) => {
                self.installed = true;
                Ok(())
            }
            Err(BackendError::Unavailable(m)) => {
                self.status = Status::Unavailable(m.clone());
                Err(m)
            }
            Err(BackendError::Failed(m)) => Err(m),
        }
    }

    fn refresh_status(&mut self) {
        let now = self.clock.now();
        if let Status::Unavailable(_) = self.status {
            if self.installed || !self.cfg.enabled {
                self.status = Status::Off;
            } else {
                return;
            }
        }
        self.status = if self.boot_off && !self.installed {
            Status::OffThisBoot
        } else if !self.cfg.enabled && !self.installed {
            Status::Off
        } else if let Some(p) = &self.pending {
            Status::Pending { secs_left: (p.deadline - now).max(0) }
        } else if self.installed {
            Status::On
        } else {
            Status::NotProtected(self.last_error.clone().unwrap_or_else(|| "the rules are not in the kernel".into()))
        };
    }

    /// Boot: apply the saved settings unless the kernel command line says `iyakki.firewall=off`.
    pub fn boot(&mut self, firewall_off: bool) {
        self.booted = true;
        if let Some(n) = self.load_note.take() {
            self.note("firewall.settings_unreadable", &n);
            self.last_error = Some(n);
        }
        if firewall_off {
            self.boot_off = true;
            self.note("firewall.boot_skipped", "iyakki.firewall=off on the kernel command line: no rules this boot");
            self.refresh_status();
            return;
        }
        self.apply_and_report("firewall.boot");
    }

    fn apply_and_report(&mut self, event: &str) {
        match self.apply_current() {
            Ok(()) => {
                self.last_error = None;
                if self.cfg.enabled {
                    let n = self.build().map(|b| b.ruleset.rules.len()).unwrap_or(0);
                    self.note(event, &format!("{n} rules in force"));
                } else {
                    self.note(event, "firewall is off");
                }
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                self.note("firewall.apply_failed", &e);
            }
        }
        self.refresh_status();
    }

    /// The network came up or changed: look the names up again and apply again (an address may have appeared).
    pub fn network_changed(&mut self) {
        self.next_refresh = 0;
        if self.cfg.enabled && !self.boot_off {
            self.apply_and_report("firewall.network");
        }
    }

    /// Validate `new` completely; nothing is changed.
    pub fn check(&self, new: &Config) -> Result<(), String> {
        if new.rules.len() > crate::model::MAX_RULES {
            return Err(format!("At most {} rules.", crate::model::MAX_RULES));
        }
        for r in &new.rules {
            check_rule(r).map_err(|e| format!("Rule {} ({}): {e}", r.id, r.value))?;
        }
        let mut ctx = self.ctx.clone();
        ctx.now = self.clock.now();
        ruleset::build(new, &self.resolved, &ctx).map(|_| ())
    }

    /// Apply changed settings and start the 60-second confirmation. Turning the firewall off or changing nothing in the
    /// rules needs no confirmation.
    pub fn propose(&mut self, new: Config, who: &str) -> Result<(), String> {
        self.check(&new)?;
        let baseline = self.pending.as_ref().map_or_else(|| self.committed.clone(), |p| p.previous.clone());
        let was = std::mem::replace(&mut self.cfg, new);
        // names in the new rules that have no address yet are looked up now
        self.refresh_dns_for_missing();
        let was_installed = self.installed;
        self.boot_off = false;
        if let Err(e) = self.apply_current() {
            // the kernel kept what it had (the change is atomic)
            self.cfg = was;
            self.last_error = Some(e.clone());
            self.note("firewall.apply_failed", &format!("{who}: {e}"));
            self.refresh_status();
            return Err(e);
        }
        let _ = was_installed;
        self.last_error = None;
        if !self.cfg.enabled || self.cfg == baseline {
            self.pending = None;
            self.committed = self.cfg.clone();
            self.persist();
            self.note(if self.cfg.enabled { "firewall.apply" } else { "firewall.disable" }, &format!("{who}: {}", if self.cfg.enabled { "no change" } else { "turned off" }));
        } else {
            self.pending = Some(Pending { deadline: self.clock.now() + CONFIRM_SECS, previous: baseline });
            self.note("firewall.apply", &format!("{who}: {} rules, {} presets; waiting {CONFIRM_SECS} s for Keep", self.cfg.rules.len(), self.cfg.presets.len()));
        }
        self.refresh_status();
        Ok(())
    }

    fn persist(&mut self) {
        let s = Saved { config: self.committed.clone(), resolved: self.resolved.clone() };
        if let Err(e) = self.store.save(&s) {
            self.last_error = Some(format!("could not save the firewall settings: {e}"));
            self.note("firewall.save_failed", &e);
        }
    }

    /// "Keep": the new rules stay and are saved.
    pub fn keep(&mut self, who: &str) -> Result<(), String> {
        if self.pending.take().is_none() {
            return Err("There is nothing waiting to be kept.".into());
        }
        self.committed = self.cfg.clone();
        self.persist();
        self.note("firewall.keep", who);
        self.refresh_status();
        Ok(())
    }

    /// "Revert" (or the timeout): the last kept settings are put back.
    pub fn revert(&mut self, why: &str) -> Result<(), String> {
        let Some(p) = self.pending.take() else { return Err("There is nothing to revert.".into()) };
        self.cfg = p.previous;
        self.apply_and_report("firewall.revert");
        self.note("firewall.revert", why);
        Ok(())
    }

    /// The emergency switch: remove our table and keep the firewall off until it is switched on again.
    pub fn disable_now(&mut self, who: &str) -> Result<(), String> {
        self.pending = None;
        let mut c = self.cfg.clone();
        c.enabled = false;
        self.cfg = c;
        match self.apply_current() {
            Ok(()) => {
                self.committed = self.cfg.clone();
                self.persist();
                self.last_error = None;
                self.note("firewall.disable", &format!("{who}: emergency off"));
                self.refresh_status();
                Ok(())
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                self.note("firewall.disable_failed", &e);
                self.refresh_status();
                Err(e)
            }
        }
    }

    /// Run the timers. Call every few seconds.
    pub fn tick(&mut self) {
        let now = self.clock.now();
        if let Some(p) = &self.pending {
            if now >= p.deadline {
                let _ = self.revert("not confirmed within 60 seconds");
                return;
            }
        }
        if self.boot_off || !self.cfg.enabled {
            self.refresh_status();
            return;
        }
        if self.next_expiry != 0 && now >= self.next_expiry {
            self.apply_and_report("firewall.expire");
        }
        if self.sync_dns && self.cfg.enabled && now >= self.next_refresh && !self.cfg.host_names().is_empty() {
            self.refresh_dns(false);
        }
        self.refresh_status();
    }

    fn refresh_dns_for_missing(&mut self) {
        if !self.sync_dns {
            return;
        }
        let missing: Vec<String> = self.cfg.host_names().into_iter().filter(|n| !self.resolved.addrs.contains_key(n)).collect();
        if !missing.is_empty() {
            let now = self.clock.now();
            self.next_refresh = now + i64::from(self.cfg.refresh_minutes.clamp(1, 1440)) * 60;
        }
        for n in missing {
            self.lookup(&n);
        }
    }

    /// One lookup with the engine's own resolver. Returns true if the stored addresses changed.
    fn lookup(&mut self, name: &str) -> bool {
        let r = self.resolver.resolve(name);
        self.record_lookup(name, r)
    }

    /// Store the answer of a lookup (made by the caller, outside any lock). Returns true if the addresses changed.
    pub fn record_lookup(&mut self, name: &str, answer: Result<Vec<IpAddr>, String>) -> bool {
        let now = self.clock.now();
        match answer {
            Ok(mut a) if !a.is_empty() => {
                a.retain(|x| !x.is_unspecified() && !x.is_multicast());
                a.sort();
                a.dedup();
                a.truncate(MAX_ADDRS_PER_NAME);
                if a.is_empty() {
                    self.warnings.insert(name.to_string(), "the name resolved only to addresses that cannot be used".into());
                    return false;
                }
                self.warnings.remove(name);
                self.resolved.at.insert(name.to_string(), now);
                let changed = self.resolved.addrs.get(name) != Some(&a);
                self.resolved.addrs.insert(name.to_string(), a);
                changed
            }
            Ok(_) => {
                self.warnings.insert(name.to_string(), "the name has no address; the last good addresses are kept".into());
                false
            }
            Err(e) => {
                let kept = self.resolved.addrs.contains_key(name);
                self.warnings.insert(name.to_string(), format!("lookup failed ({e}){}", if kept { "; the last good addresses are kept" } else { "; no address yet, so this rule allows nothing" }));
                false
            }
        }
    }

    /// Names whose lookup is due (the service looks them up outside its lock and hands the answers to [`Engine::finish_dns`]).
    pub fn dns_due(&self) -> Vec<String> {
        if !self.cfg.enabled || self.boot_off {
            return Vec::new();
        }
        let missing: Vec<String> = self.cfg.host_names().into_iter().filter(|n| !self.resolved.addrs.contains_key(n)).collect();
        if self.clock.now() >= self.next_refresh {
            self.cfg.host_names()
        } else {
            missing
        }
    }

    /// The answers for names from [`Engine::dns_due`].
    pub fn finish_dns(&mut self, answers: Vec<(String, Result<Vec<IpAddr>, String>)>) {
        let now = self.clock.now();
        self.next_refresh = now + i64::from(self.cfg.refresh_minutes.clamp(1, 1440)) * 60;
        let mut changed = Vec::new();
        for (n, a) in answers {
            if self.cfg.host_names().contains(&n) && self.record_lookup(&n, a) {
                changed.push(n);
            }
        }
        self.after_dns(changed, false);
    }

    fn after_dns(&mut self, changed: Vec<String>, force: bool) {
        let used = self.cfg.host_names();
        self.resolved.addrs.retain(|k, _| used.contains(k));
        self.resolved.at.retain(|k, _| used.contains(k));
        self.warnings.retain(|k, _| used.contains(k));
        if !changed.is_empty() && self.cfg.enabled && !self.boot_off {
            self.apply_and_report("firewall.dns");
            self.note("firewall.dns_changed", &changed.join(","));
        }
        if !changed.is_empty() || force {
            self.persist();
        }
    }

    /// Look all names up now (with the engine's resolver); apply again if an answer changed.
    pub fn refresh_dns(&mut self, force: bool) {
        let now = self.clock.now();
        self.next_refresh = now + i64::from(self.cfg.refresh_minutes.clamp(1, 1440)) * 60;
        let mut changed: Vec<String> = Vec::new();
        for n in self.cfg.host_names() {
            if self.lookup(&n) {
                changed.push(n);
            }
        }
        self.after_dns(changed, force);
    }

    /// Ask for a new look-up of all names at the next opportunity.
    pub fn request_dns_refresh(&mut self) {
        self.next_refresh = 0;
    }

    pub fn counters(&mut self) -> Option<(u64, u64)> {
        if !self.installed {
            return None;
        }
        self.backend.counters().ok()
    }

    /// Would a new connection from `src` to `port` (tcp) get in? True when the firewall is not in force.
    pub fn would_allow_tcp(&self, src: IpAddr, port: u16) -> bool {
        if !self.installed {
            return true;
        }
        match self.build() {
            Ok(b) => ruleset::eval(&b.ruleset, &Packet::new_tcp("eth0", src, port)),
            Err(_) => false,
        }
    }

    pub fn view(&self) -> View {
        let now = self.clock.now();
        let built = self.build().ok();
        let mut states = Vec::new();
        for r in &self.cfg.rules {
            let s = if !r.enabled {
                RuleState::Disabled
            } else if r.expires != 0 && r.expires <= now {
                RuleState::Expired
            } else {
                match check_rule(r) {
                    Err(e) => RuleState::Invalid(e),
                    Ok(_) if r.kind == Kind::Host && self.resolved.addrs.get(&normalize_host(&r.value)).is_none_or(|a| a.is_empty()) => RuleState::WaitingForDns,
                    Ok(_) => RuleState::Active,
                }
            };
            states.push((r.id, s));
        }
        let hosts = self
            .cfg
            .host_names()
            .into_iter()
            .map(|n| HostView { addrs: self.resolved.addrs.get(&n).cloned().unwrap_or_default(), last_ok: self.resolved.at.get(&n).copied(), warning: self.warnings.get(&n).cloned(), name: n })
            .collect();
        let mut warnings: Vec<String> = Vec::new();
        if let Some(b) = &built {
            warnings.extend(b.problems.iter().map(|(i, e)| format!("Rule {i}: {e}")));
        }
        for r in self.cfg.rules.iter().filter(|r| r.enabled) {
            if let Ok(p) = check_rule(r) {
                for w in p.warnings.iter().filter(|w| !w.starts_with("A name is only")) {
                    warnings.push(format!("Rule {}: {w}", r.id));
                }
            }
        }
        View { status: self.status.clone(), config: self.cfg.clone(), rule_states: states, hosts, warnings, ssh_boot: self.ctx.ssh_boot, last_error: self.last_error.clone(), events: self.events.clone() }
    }

    /// Has boot applied (or skipped) the rules in this process? A window on someone else's computer never does.
    pub fn managed(&self) -> bool {
        self.booted
    }

    pub fn status(&self) -> Status {
        self.status.clone()
    }

    /// The ruleset as nft-like text (for the page).
    pub fn ruleset_text(&self) -> String {
        match self.build() {
            Ok(b) => b.ruleset.to_text(),
            Err(e) => format!("# {e}\n"),
        }
    }
}
