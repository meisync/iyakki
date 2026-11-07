//! Settings -> Firewall (administrators): the state in big letters, what is allowed in, the 60-second "Keep these rules?"
//! after a change, the last blocked attempts, presets and import/export. By default every new incoming connection is blocked.
//! The rules, the kernel writer and the timers are in the `iyakki-fw` crate; this page only edits a draft and hands it over.
//! Design and recovery: `docs/FIREWALL.md`.

use std::sync::Arc;
use std::time::Instant;

use iyakki_fw::blocked::{Blocked, BlockLog};
use iyakki_fw::engine::{RuleState, Status, View};
use iyakki_fw::model::{Config, Kind as FwKind, Ping, Preset, Proto, Rule};
use iyakki_fw::service::Handle;

use super::*;

pub(super) use crate::fx::AMBER;

/// What the page keeps between frames.
pub(super) struct FwPanel {
    pub(super) handle: Handle,
    pub(super) log: Arc<BlockLog>,
    /// Changes not applied yet (the page edits this; Apply hands it to the firewall).
    pub(super) draft: Option<Config>,
    /// False in a window on someone else's computer: the page shows, and changes nothing.
    pub(super) writable: bool,
    /// The simple view is the default; Advanced is the page with every rule, preset, the log and import/export.
    pub(super) advanced: bool,
    pub(super) counters: (u64, u64),
    pub(super) counters_at: Option<Instant>,
}

impl FwPanel {
    pub(super) fn counters_due(&self) -> bool {
        self.counters_at.is_none_or(|t| t.elapsed().as_secs() >= 3)
    }
}

impl Default for FwPanel {
    fn default() -> FwPanel {
        FwPanel { handle: Handle::Global, log: iyakki_fw::service::block_log(), draft: None, writable: !hf_on(), advanced: false, counters: (0, 0), counters_at: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FwForm {
    /// A new rule, or the rule with this id.
    Rule(Option<u32>),
    Presets,
    /// Simple view: a custom port, or a computer / network by name or address.
    Port,
    Address,
}

const KINDS: [FwKind; 4] = FwKind::ALL;
const PROTOS: [&str; 3] = ["TCP and UDP", "TCP", "UDP"];
const EXPIRIES: [(&str, i64); 3] = [("Never", 0), ("1 hour", 3600), ("1 day", 86400)];

fn proto_idx(p: Proto) -> usize {
    match p {
        Proto::Both => 0,
        Proto::Tcp => 1,
        Proto::Udp => 2,
    }
}

fn proto_of(i: usize) -> Proto {
    match i {
        1 => Proto::Tcp,
        2 => Proto::Udp,
        _ => Proto::Both,
    }
}

pub(super) fn rule_form(r: Option<&Rule>) -> Form {
    let kinds: Vec<&str> = KINDS.iter().map(|k| k.label()).collect();
    let mut exp: Vec<&str> = EXPIRIES.iter().map(|e| e.0).collect();
    let keep = r.is_some_and(|r| r.expires != 0);
    if keep {
        exp.push("Keep the current end time");
    }
    let d = Rule::new(FwKind::Host, "");
    let r = r.unwrap_or(&d);
    Form::new(if r.id == 0 { "Allow incoming connections from..." } else { "Edit rule" })
        .choice("Type", &kinds, KINDS.iter().position(|k| *k == r.kind).unwrap_or(0))
        .text("Value", &r.value, "host name (nas.local; only as trustworthy as your DNS), IP address, range 192.168.1.10-192.168.1.50, or network 192.168.1.0/24")
        .choice("Protocol", &PROTOS, proto_idx(r.proto))
        .text("Ports", &r.ports, "empty: every port. Otherwise 22 or 80,443 or 8000-8100")
        .text("Interface", &r.iface, "empty: any. Otherwise only packets arriving on this one, for example eth0")
        .text("Note", &r.comment, "for you; up to 80 characters")
        .choice("Allow for", &exp, if keep { 3 } else { 0 })
}

/// Read the rule form back (the checks that need the whole rule happen in `check_rule`).
pub(super) fn rule_of_form(f: &Form, old: Option<&Rule>, now: i64) -> Result<Rule, String> {
    let kind = KINDS.get(f.choice_idx(0)).copied().unwrap_or(FwKind::Host);
    let mut r = Rule::new(kind, &f.value(1));
    r.proto = proto_of(f.choice_idx(2));
    r.ports = f.value(3);
    r.iface = f.value(4);
    r.comment = f.value(5);
    r.expires = match f.choice_idx(6) {
        3 => old.map_or(0, |o| o.expires),
        i => EXPIRIES.get(i).map_or(0, |e| if e.1 == 0 { 0 } else { now + e.1 }),
    };
    if let Some(o) = old {
        r.id = o.id;
        r.enabled = o.enabled;
    }
    iyakki_fw::validate::check_rule(&r)?;
    Ok(r)
}

pub(super) fn presets_form(c: &Config) -> Form {
    let mut f = Form::new("Presets and basics");
    for p in Preset::ALL {
        f = f.choice(p.label(), &["off", "on"], usize::from(c.has(p)));
    }
    f = f.choice("Ping (echo request)", &["not answered", "from the local network only", "from anywhere"], match c.ping {
        Ping::Off => 0,
        Ping::Local => 1,
        Ping::Anyone => 2,
    });
    f = f.choice("Cluster and container networks on this machine", &["blocked like the rest", "trusted (recommended with k3s or podman)"], usize::from(c.trust_containers));
    f = f.text("Look up host names again every (minutes)", &c.refresh_minutes.to_string(), "1 to 1440; also when the network changes");
    for (i, p) in Preset::ALL.iter().enumerate() {
        f.fields[i].hint = p.explain().to_string();
    }
    f
}

pub(super) fn apply_presets_form(f: &Form, c: &mut Config) -> Result<(), String> {
    for (i, p) in Preset::ALL.iter().enumerate() {
        c.set_preset(*p, f.choice_idx(i) == 1);
    }
    let n = Preset::ALL.len();
    c.ping = match f.choice_idx(n) {
        0 => Ping::Off,
        2 => Ping::Anyone,
        _ => Ping::Local,
    };
    c.trust_containers = f.choice_idx(n + 1) == 1;
    c.refresh_minutes = f.value(n + 2).parse::<u32>().ok().filter(|m| (1..=1440).contains(m)).ok_or("Look-up interval: a whole number of minutes from 1 to 1440.")?;
    Ok(())
}

fn expiry_text(r: &Rule, now: i64) -> String {
    match r.expires {
        0 => "no end".into(),
        e if e <= now => "ended".into(),
        e => {
            let l = e - now;
            if l >= 86400 {
                format!("ends in {} d {} h", l / 86400, l % 86400 / 3600)
            } else if l >= 3600 {
                format!("ends in {} h {} min", l / 3600, l % 3600 / 60)
            } else {
                format!("ends in {} min", (l / 60).max(1))
            }
        }
    }
}

/// Title and detail line of a rule row.
pub(super) fn rule_lines(r: &Rule, v: &View, now: i64) -> (String, String) {
    let title = format!("{}  {}", r.kind.label(), r.value);
    let ports = if r.ports.trim().is_empty() { "all ports".to_string() } else { format!("ports {}", r.ports) };
    let mut parts = vec![format!("{} {}", r.proto.label(), ports), if r.iface.is_empty() { "any interface".into() } else { format!("on {}", r.iface) }, expiry_text(r, now)];
    if r.kind == FwKind::Host {
        let n = iyakki_fw::validate::normalize_host(&r.value);
        if let Some(h) = v.hosts.iter().find(|h| h.name == n) {
            parts.push(if h.addrs.is_empty() { "no address yet".to_string() } else { format!("= {}", h.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")) });
        }
    }
    if !r.comment.is_empty() {
        parts.push(r.comment.clone());
    }
    if iyakki_fw::devrules::is_managed(r) {
        parts.push("made by developer mode so the developer computer can connect; it goes away with developer mode, or Remove it".to_string());
    }
    (title, parts.join(" · "))
}

pub(super) fn state_chip(s: &RuleState) -> (&'static str, [u8; 4]) {
    match s {
        RuleState::Active => ("allowed", OK),
        RuleState::Disabled => ("off", DIM),
        RuleState::Expired => ("ended", ERR),
        RuleState::WaitingForDns => ("no address", AMBER),
        RuleState::Invalid(_) => ("invalid", ERR),
    }
}

/// The words under the big status, most important first.
pub(super) fn status_lines(v: &View, draft_changes: usize, writable: bool) -> Vec<(String, [u8; 4])> {
    let mut out: Vec<(String, [u8; 4])> = Vec::new();
    match &v.status {
        Status::On => out.push(("Incoming connections are blocked unless allowed below.".into(), TEXT)),
        Status::Pending { secs_left } => {
            out.push((format!("Keep these rules? {secs_left} s left, then they are taken back by themselves. k keeps them, v reverts now."), AMBER));
            out.push(("Incoming connections are blocked unless allowed below.".into(), TEXT));
        }
        Status::Off => out.push(("The firewall is off: anything on this machine that listens can be reached from the network. o switches it on.".into(), AMBER)),
        Status::OffThisBoot => out.push(("Off for this boot only (kernel parameter iyakki.firewall=off). It comes back at the next restart.".into(), AMBER)),
        Status::NotProtected(e) => {
            out.push((format!("NOT PROTECTED: {e}. Incoming connections are NOT blocked."), ERR));
        }
        Status::Unavailable(e) => out.push((format!("{e}. Incoming connections are NOT blocked."), ERR)),
    }
    if v.status.protected() || matches!(v.status, Status::Off | Status::OffThisBoot) {
        out.push(("Always allowed: replies to connections this machine started, this machine talking to itself, cluster and container networks on this machine, DHCP and IPv6 neighbour discovery. Outgoing traffic is not filtered (planned).".into(), MUTED));
    }
    if v.ssh_boot {
        out.push(("Remote diagnostics (boot parameter iyakki.ssh=1) is on, so SSH (port 22) is allowed from the local network.".into(), AMBER));
    }
    if let Some(e) = &v.last_error {
        if !matches!(v.status, Status::NotProtected(_) | Status::Unavailable(_)) {
            out.push((format!("The last change failed and was not applied: {e}"), ERR));
        }
    }
    if draft_changes > 0 {
        out.push((format!("{draft_changes} change(s) not applied yet. y applies them (then you have 60 s to keep them), u discards them."), AMBER));
    }
    if !writable {
        out.push(("This window runs on another computer: the page shows IYAKKI's firewall settings but changes nothing here.".into(), MUTED));
    }
    out
}

pub(super) fn presets_line(c: &Config) -> String {
    let on: Vec<&str> = Preset::ALL.iter().filter(|p| c.has(**p)).map(|p| p.label()).collect();
    format!("Presets on: {}. Ping: {}. p changes them.", if on.is_empty() { "none".to_string() } else { on.join(", ") }, c.ping.label())
}

pub(super) fn blocked_line(b: &Blocked, now: i64) -> String {
    let ago = (now - b.at).max(0);
    let when = if ago < 60 { format!("{ago} s ago") } else if ago < 3600 { format!("{} min ago", ago / 60) } else { format!("{} h ago", ago / 3600) };
    format!("{when}  {}  {}{}{}", b.src, b.proto_name(), if b.dport != 0 { format!(" port {}", b.dport) } else { String::new() }, if b.count > 1 { format!("  x{}", b.count) } else { String::new() })
}

/// How many entries differ between the draft and what is in force (a rough count for the banner).
pub(super) fn change_count(draft: &Config, base: &Config) -> usize {
    let mut n = 0;
    n += draft.rules.iter().filter(|r| !base.rules.contains(r)).count();
    n += base.rules.iter().filter(|r| !draft.rules.contains(r)).count();
    n += Preset::ALL.iter().filter(|p| draft.has(**p) != base.has(**p)).count();
    n += usize::from(draft.ping != base.ping) + usize::from(draft.enabled != base.enabled) + usize::from(draft.trust_containers != base.trust_containers) + usize::from(draft.refresh_minutes != base.refresh_minutes);
    n
}

/// Does the firewall keep other computers on the local network out of a share's port? The words for the Shared folders page.
pub(super) fn blocked_note_for(eng: &iyakki_fw::Engine, port: u16) -> Option<String> {
    if !eng.managed() || eng.would_allow_tcp("192.168.0.2".parse().ok()?, port) {
        return None;
    }
    Some(format!("The firewall blocks port {port} from the network, so other devices cannot open this share. Press f to open the shared-folder ports (8081-8180) to the local network; you then confirm on the Firewall page."))
}

pub(super) fn share_blocked_note(port: u16) -> Option<String> {
    let e = iyakki_fw::service::try_engine()?;
    blocked_note_for(&e, port)
}

impl SettingsApp {
    /// The Shared folders page's "f": switch the Shared folders preset on (applied now, confirmed on the Firewall page).
    pub(super) fn fw_open_shares(&mut self, cx: &AppCx) {
        self.fw_open_shares_impl(cx, true)
    }

    /// The same, answered "yes" to the card that came up when a share started: the person stays on the Shared folders page.
    pub(super) fn fw_open_shares_here(&mut self, cx: &AppCx) {
        self.fw_open_shares_impl(cx, false)
    }

    fn fw_open_shares_impl(&mut self, cx: &AppCx, go_to_firewall: bool) {
        if !self.admin(cx) {
            return;
        }
        if !self.fw.writable {
            return self.fail("This window runs on another computer; it cannot change IYAKKI's firewall.");
        }
        let mut c = self.fw.handle.lock().config().clone();
        if c.has(Preset::SharedFolders) {
            return self.ok("The firewall already lets the local network reach shared folders.");
        }
        c.set_preset(Preset::SharedFolders, true);
        let user = cx.session.user.clone();
        let (r, kept) = self.propose_and_keep_if_only_opening(c, &user);
        match r {
            Ok(()) => {
                cx.audit.record(&user, "firewall.preset", "shares on, from Shared folders");
                if go_to_firewall {
                    self.open_item("tab:firewall", cx);
                }
                self.ok(if kept { "Opened the firewall for shared folders. It stays open until you close it." } else { "Opened the firewall for shared folders. Press k within 60 seconds to keep it, or v to go back." });
            }
            Err(e) => self.fail(format!("Not applied: {e}")),
        }
    }

    /// Apply `c`; when it only lets more in (nothing can lock the person out) and no earlier change was waiting for Keep, keep it at once, so it does not
    /// disappear after 60 seconds. Anything that tightens keeps the confirmation. Returns the result and whether it was kept.
    pub(super) fn propose_and_keep_if_only_opening(&self, c: Config, user: &str) -> (Result<(), String>, bool) {
        let mut e = self.fw.handle.lock();
        let before = e.config().clone();
        let waiting = matches!(e.status(), Status::Pending { .. });
        let opens = !waiting && c.only_opens_from(&before);
        let r = e.propose(c, user);
        let kept = r.is_ok() && opens && matches!(e.status(), Status::Pending { .. }) && e.keep(user).is_ok();
        (r, kept)
    }

    pub(super) fn fw_base(&self) -> Config {
        self.fw.handle.lock().config().clone()
    }

    pub(super) fn fw_cfg(&self) -> Config {
        self.fw.draft.clone().unwrap_or_else(|| self.fw_base())
    }

    pub(super) fn fw_len(&self) -> usize {
        if !self.fw.advanced {
            return self.fw_items().len();
        }
        self.fw_cfg().rules.len()
    }

    /// Edit the draft; a draft that equals what is in force is dropped.
    fn fw_edit(&mut self, f: impl FnOnce(&mut Config)) {
        let mut c = self.fw_cfg();
        f(&mut c);
        let base = self.fw_base();
        self.fw.draft = if c == base { None } else { Some(c) };
    }

    pub(super) fn fw_buttons(&self) -> Vec<Btn> {
        if !self.fw.advanced {
            let mut v = vec![("Add…", ACCENT, 'a'), ("Remove", ERR, 'd')];
            if matches!(self.fw.handle.lock().status(), Status::Pending { .. }) {
                v.insert(0, ("Undo", ERR, 'v'));
                v.insert(0, ("Keep", OK, 'k'));
            }
            if self.fw.draft.is_some() {
                v.extend([("Apply", ACCENT, 'y'), ("Discard", TEXT, 'u')]);
            }
            v.push(("Advanced…", TEXT, 'A'));
            return v;
        }
        let mut v = vec![("Add rule…", ACCENT, 'a'), ("Edit", TEXT, 'e'), ("Allow on/off", TEXT, 'x'), ("Delete", ERR, 'd'), ("Presets…", TEXT, 'p'), ("Apply", ACCENT, 'y')];
        if matches!(self.fw.handle.lock().status(), Status::Pending { .. }) {
            v.insert(0, ("Revert", ERR, 'v'));
            v.insert(0, ("Keep", OK, 'k'));
        }
        // discard, look names up, copy, import and clear the log have keys too (the hint line lists them) and buttons for touch
        if self.fw.draft.is_some() {
            v.push(("Discard", TEXT, 'u'));
        }
        v.extend([("Look up names", TEXT, 'r'), ("Copy rules", TEXT, 'c'), ("Import", TEXT, 'i'), ("Clear log", TEXT, 'b')]);
        v.extend([("On/off", TEXT, 'o'), ("Disable now", ERR, 'z'), ("Simple view", TEXT, 'A')]);
        v
    }

    pub(super) fn handle_firewall(&mut self, k: &Key, cx: &AppCx) {
        if *k == Key::Char('A') {
            self.fw.advanced = !self.fw.advanced;
            self.sel[self.tab] = 0;
            self.ok(if self.fw.advanced { "Advanced: every rule, preset and the log. Choose Simple view to go back." } else { "Simple view." });
            return;
        }
        if !self.fw.advanced && self.handle_fw_simple(k, cx) {
            return;
        }
        let len = self.fw_len();
        let sel = self.sel[self.tab].min(len.saturating_sub(1));
        let changes = matches!(k, Key::Char('a' | 'e' | 'x' | 'd' | 'p' | 'y' | 'k' | 'v' | 'o' | 'u' | 'i' | 'z' | 'r' | 'b'));
        match k {
            Key::Up => return self.move_sel(-1, len),
            Key::Down => return self.move_sel(1, len),
            Key::Char('c') => {
                crate::ui::clipboard::copy(&self.fw_cfg().to_json());
                self.ok("Copied the rules as JSON (also the presets). Import them on another machine with i.");
                return;
            }
            _ => {}
        }
        if !changes {
            return;
        }
        if !self.admin(cx) {
            return;
        }
        if !self.fw.writable {
            return self.fail("This window runs on another computer; it cannot change IYAKKI's firewall.");
        }
        let user = cx.session.user.clone();
        let rule = self.fw_cfg().rules.get(sel).cloned();
        match k {
            Key::Char('a') => self.open_form(FormKind::Fw(FwForm::Rule(None)), rule_form(None)),
            Key::Char('e') => match rule {
                Some(r) => self.open_form(FormKind::Fw(FwForm::Rule(Some(r.id))), rule_form(Some(&r))),
                None => self.fail("Add a rule first (a)."),
            },
            Key::Char('x') => match rule {
                Some(r) => {
                    let on = !r.enabled;
                    self.fw_edit(|c| {
                        if let Some(x) = c.rule_mut(r.id) {
                            x.enabled = on;
                        }
                    });
                    cx.audit.record(&user, "firewall.rule.toggle", &format!("{} {} {}", r.kind.label(), r.value, if on { "on" } else { "off" }));
                    self.ok(format!("{} is {} (not applied yet: y).", r.value, if on { "allowed again" } else { "switched off" }));
                }
                None => self.fail("Choose a rule first (Up/Down)."),
            },
            Key::Char('d') => match rule {
                Some(r) => {
                    self.fw_edit(|c| {
                        c.remove(r.id);
                    });
                    cx.audit.record(&user, "firewall.rule.delete", &format!("{} {}", r.kind.label(), r.value));
                    self.ok(format!("Removed the rule for {} (not applied yet: y).", r.value));
                }
                None => self.fail("Choose a rule first (Up/Down)."),
            },
            Key::Char('p') => {
                let c = self.fw_cfg();
                self.open_form(FormKind::Fw(FwForm::Presets), presets_form(&c));
            }
            Key::Char('o') => {
                let on = !self.fw_cfg().enabled;
                self.fw_edit(|c| c.enabled = on);
                self.ok(if on { "The firewall will be switched on (not applied yet: y)." } else { "The firewall will be switched off (not applied yet: y)." });
            }
            Key::Char('u') => {
                self.fw.draft = None;
                self.ok("Changes discarded.");
            }
            Key::Char('y') => match self.fw.draft.clone() {
                None => self.fail("There is nothing to apply: the rules in force are what you see."),
                Some(c) => {
                    let (r, kept) = self.propose_and_keep_if_only_opening(c, &user);
                    match r {
                        Ok(()) => {
                            self.fw.draft = None;
                            cx.audit.record(&user, if kept { "firewall.keep" } else { "firewall.apply" }, if kept { "from Settings: only opens, kept at once" } else { "from Settings" });
                            let st = self.fw.handle.lock().status();
                            self.ok(if matches!(st, Status::Pending { .. }) { "Applied. Press k within 60 seconds to keep these rules, or v to go back. If nothing is pressed they are taken back." } else { "Applied." });
                        }
                        Err(e) => self.fail(format!("Not applied: {e}")),
                    }
                }
            },
            Key::Char('k') => {
                let r = self.fw.handle.lock().keep(&user);
                match r {
                    Ok(()) => {
                        cx.audit.record(&user, "firewall.keep", "");
                        self.ok("Kept: these rules are saved and stay after a restart.")
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('v') => {
                let r = self.fw.handle.lock().revert(&format!("{user} pressed Revert"));
                match r {
                    Ok(()) => {
                        cx.audit.record(&user, "firewall.revert", "");
                        self.ok("Reverted to the rules you last kept.")
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('z') => {
                let r = self.fw.handle.lock().disable_now(&user);
                match r {
                    Ok(()) => {
                        self.fw.draft = None;
                        cx.audit.record(&user, "firewall.disable", "emergency off from Settings");
                        self.ok("The firewall is off. Switch it on again with o and y when you are ready.")
                    }
                    Err(e) => self.fail(format!("Could not switch the firewall off: {e}")),
                }
            }
            Key::Char('r') => {
                self.fw.handle.lock().request_dns_refresh();
                self.ok("Host names will be looked up again in a moment.");
            }
            Key::Char('b') => {
                self.fw.log.clear();
                self.ok("The list of blocked attempts is cleared.");
            }
            Key::Char('i') => match crate::ui::clipboard::latest() {
                Some(t) => self.fw_import(cx, &t),
                None => self.fail("Copy the exported rules first, then press i."),
            },
            _ => {}
        }
    }

    /// Take exported rules as unapplied changes.
    pub(super) fn fw_import(&mut self, cx: &AppCx, text: &str) {
        match Config::from_json(text) {
            Ok(c) => {
                let n = c.rules.len();
                self.fw_edit(|d| *d = c);
                cx.audit.record(&cx.session.user, "firewall.import", &format!("{n} rules"));
                self.ok(format!("Imported {n} rule(s) as changes (not applied yet: y)."));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn fw_submit(&mut self, cx: &AppCx, kind: &FwForm, form: &mut Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Err("Only administrators can change the firewall.".into());
        }
        let user = cx.session.user.clone();
        match kind {
            FwForm::Rule(id) => {
                let old = id.and_then(|i| self.fw_cfg().rules.into_iter().find(|r| r.id == i));
                let r = rule_of_form(form, old.as_ref(), iyakki_agent::clock::now_unix())?;
                let warn = iyakki_fw::validate::check_rule(&r)?.warnings.into_iter().find(|w| !w.starts_with("A name is only")).unwrap_or_default();
                let detail = format!("{} {} {} {}", r.kind.label(), r.value, r.proto.label(), r.ports);
                self.fw_edit(|c| match id {
                    Some(i) => {
                        if let Some(x) = c.rule_mut(*i) {
                            *x = r.clone();
                        }
                    }
                    None => {
                        c.add(r.clone());
                    }
                });
                cx.audit.record(&user, if id.is_some() { "firewall.rule.edit" } else { "firewall.rule.add" }, &detail);
                Ok(format!("{} (not applied yet: y).{}", if id.is_some() { "Saved the rule" } else { "Added the rule" }, if warn.is_empty() { String::new() } else { format!(" Note: {warn}") }))
            }
            FwForm::Port | FwForm::Address => self.fw_simple_submit(cx, kind, form),
            FwForm::Presets => {
                let mut c = self.fw_cfg();
                apply_presets_form(form, &mut c)?;
                self.fw_edit(|d| *d = c);
                cx.audit.record(&user, "firewall.presets", "changed");
                Ok("Presets changed (not applied yet: y).".into())
            }
        }
    }

    pub(super) fn render_firewall(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if !self.fw.advanced {
            return self.render_firewall_simple(g, a, s, cx);
        }
        let v = self.fw.handle.lock().view();
        let now = iyakki_agent::clock::now_unix();
        let base = self.fw_base();
        let cfg = self.fw_cfg();
        let changes = self.fw.draft.as_ref().map_or(0, |d| change_count(d, &base));
        let bottom = a.y + a.h;
        let mut y = a.y;
        // status in big letters
        let (word, col) = match &v.status {
            Status::On | Status::Pending { .. } => ("ON", OK),
            Status::Off | Status::OffThisBoot => ("OFF", AMBER),
            Status::NotProtected(_) | Status::Unavailable(_) => ("NOT PROTECTED", ERR),
        };
        let size = if a.w < 330.0 * s { 26.0 * s } else { 34.0 * s };
        let head = g.fit(size, &format!("FIREWALL {word}"), Face::Bold, a.w);
        g.text(a.x, y, size, col, &head, Face::Bold);
        y += size * 1.35;
        let line = |g: &mut Gfx, y: &mut f32, t: &str, c: [u8; 4], sz: f32, face: Face| {
            for l in g.wrap(sz, t, Face::Sans, a.w) {
                if *y + sz * 1.3 > bottom {
                    return;
                }
                g.text(a.x, *y, sz, c, &l, face);
                *y += sz * 1.4;
            }
            *y += 3.0 * s;
        };
        for (t, c) in status_lines(&v, changes, self.fw.writable) {
            line(g, &mut y, &t, c, 13.0 * s, Face::Sans);
        }
        line(g, &mut y, &presets_line(&cfg), MUTED, 13.0 * s, Face::Sans);
        if cfg.rules.iter().any(|r| r.kind == FwKind::Host) {
            line(g, &mut y, &format!("Host names are looked up every {} min and when the network changes; the addresses found are shown on each rule. A name is only as trustworthy as the DNS server that answers for it. Wildcard names are not supported.", cfg.refresh_minutes), MUTED, 12.0 * s, Face::Sans);
        }
        for w in v.warnings.iter().take(2) {
            line(g, &mut y, w, AMBER, 12.0 * s, Face::Sans);
        }
        // blocked attempts
        if self.fw.counters_at.is_none_or(|t| t.elapsed().as_secs() >= 3) {
            if let Some(c) = self.fw.handle.lock().counters() {
                self.fw.counters = c;
            }
            self.fw.counters_at = Some(Instant::now());
        }
        let recent = self.fw.log.recent();
        let shown = ((bottom - y) / (18.0 * s)).max(0.0) as usize;
        let tail_rows = if shown >= 12 { 3 } else if shown >= 8 { 2 } else { 0 };
        line(g, &mut y, &format!("Blocked connections since the rules were applied: {}. Last {} attempts:", self.fw.counters.0, recent.len().min(50)), TEXT, 13.0 * s, Face::Bold);
        for b in recent.iter().take(tail_rows) {
            line(g, &mut y, &blocked_line(b, now), DIM, 12.0 * s, Face::Mono);
        }
        // the rules
        let rules = &cfg.rules;
        if rules.is_empty() {
            if y + 20.0 * s < bottom {
                g.text(a.x, y + 4.0 * s, 14.0 * s, DIM, "No rules yet: nothing is allowed in except the presets. Press a to add one.", Face::Sans);
            }
            return;
        }
        let sel = self.sel[self.tab].min(rules.len() - 1);
        let rh = 60.0 * s;
        let fit = (((bottom - y - 4.0 * s) / rh) as usize).max(1);
        let first = if sel < fit { 0 } else { sel + 1 - fit }.min(rules.len().saturating_sub(fit));
        if y + rh > bottom + 0.5 {
            return;
        }
        for (i, r) in rules.iter().enumerate().skip(first).take(fit) {
            if y + rh > bottom + 0.5 {
                break;
            }
            let st = if self.fw.draft.is_some() && !base.rules.contains(r) { RuleState::Active } else { v.rule_states.iter().find(|x| x.0 == r.id).map(|x| x.1.clone()).unwrap_or(RuleState::Active) };
            let st = if !r.enabled { RuleState::Disabled } else { st };
            let (title, sub) = rule_lines(r, &v, now);
            let (chip, col) = state_chip(&st);
            let mut chips = vec![(chip, col)];
            if self.fw.draft.is_some() && !base.rules.contains(r) {
                chips.push(("not applied", AMBER));
            }
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &title, &sub, &chips);
            y += rh;
        }
    }
}
