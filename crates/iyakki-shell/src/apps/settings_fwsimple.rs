//! Settings -> Firewall, the simple view that people see first: one choice of how strict to be, and a short list of what is
//! allowed in with Add and Remove. Everything else (own rules, priorities, ping, host names, the blocked log) lives in the
//! Advanced view, which is the page as it always was. Both views edit the same settings through the same engine, so the
//! default (block every new incoming connection) and the 60-second "Keep these settings?" safety net are the same.

use std::time::Instant;

use iyakki_fw::engine::{Status, View};
use iyakki_fw::model::{Config, Kind as FwKind, Preset, Proto, Rule};
use iyakki_fw::ruleset::{local_networks, Addr};

use super::firewall_page::*;
use super::*;
use crate::ui::{CARD, CARD_HOVER, CARD_SEL};

/// What a firewall change that needs a question first is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FwAct {
    /// Switch to Open: nothing is blocked.
    Open,
    /// Take away a preset and/or these rules (one line of the list).
    Remove(Option<Preset>, Vec<u32>),
}

/// The three ways to run the firewall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Standard,
    Open,
    Locked,
}

impl Mode {
    pub(super) const ALL: [Mode; 3] = [Mode::Standard, Mode::Open, Mode::Locked];
    pub(super) fn title(self) -> &'static str {
        match self {
            Mode::Standard => "Standard",
            Mode::Open => "Open",
            Mode::Locked => "Locked down",
        }
    }
    pub(super) fn key(self) -> char {
        match self {
            Mode::Standard => '1',
            Mode::Open => '2',
            Mode::Locked => '3',
        }
    }
    pub(super) fn explain(self) -> &'static str {
        match self {
            Mode::Standard => "Standard: block everything coming in, allow what you open below. Recommended.",
            Mode::Open => "Open: no blocking. Anything on this computer that listens can be reached from the network.",
            Mode::Locked => "Locked down: also block outgoing connections except what you allow. Not available yet: this firewall only filters what comes in.",
        }
    }
    /// The mode these settings are in. (Locked down does not exist yet, so it is never the current one.)
    pub(super) fn of(c: &Config) -> Mode {
        if c.enabled {
            Mode::Standard
        } else {
            Mode::Open
        }
    }
}

/// One line of "Allowed on this computer": a preset, or the rules one Add made (a custom port is one rule per local network).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub title: String,
    pub detail: String,
    pub preset: Option<Preset>,
    pub rules: Vec<u32>,
}

pub(super) fn preset_words(p: Preset) -> (&'static str, &'static str) {
    match p {
        Preset::Ssh => ("SSH", "Computers on your local network can open a remote command line (port 22)."),
        Preset::RemoteDesktop => ("Remote desktop", "Browsers on your local network can open this computer's desktop."),
        Preset::SharedFolders => ("File sharing", "Your local network can open the folders you share (ports 8081-8180)."),
        Preset::K3sApi => ("Kubernetes (k3s) API", "Your local network can talk to this computer's cluster (port 6443)."),
        Preset::NodePorts => ("Kubernetes services", "Your local network can reach what the cluster publishes (ports 30000-32767)."),
        Preset::StoreNodePort => ("Private app store", "Your local network can open the private app store (port 30301)."),
        Preset::DnsService => ("Name server (DNS)", "Your local network can ask this computer to look up names (port 53)."),
        Preset::WebServer => ("Web server", "Your local network can open this computer's web pages (ports 80 and 443)."),
    }
}

/// The list the simple view shows: presets that are on, then rules (rules made by one custom-port Add are one line).
pub(super) fn items(c: &Config) -> Vec<Item> {
    let mut out: Vec<Item> = Preset::ALL
        .iter()
        .filter(|p| c.has(**p))
        .map(|p| {
            let (title, detail) = preset_words(*p);
            Item { title: title.into(), detail: detail.into(), preset: Some(*p), rules: vec![] }
        })
        .collect();
    let mut grouped: Vec<(String, Vec<u32>)> = Vec::new();
    for r in &c.rules {
        if r.comment.starts_with(PORT_TAG) && r.kind == FwKind::Cidr {
            let key = format!("{}|{}|{}", r.comment, r.ports, r.proto.label());
            match grouped.iter_mut().find(|g| g.0 == key) {
                Some(g) => g.1.push(r.id),
                None => grouped.push((key, vec![r.id])),
            }
        }
    }
    let mut done: Vec<String> = Vec::new();
    for r in &c.rules {
        let off = if r.enabled { "" } else { " (switched off in Advanced)" };
        if r.comment.starts_with(PORT_TAG) && r.kind == FwKind::Cidr {
            let key = format!("{}|{}|{}", r.comment, r.ports, r.proto.label());
            if done.contains(&key) {
                continue;
            }
            done.push(key.clone());
            let ids = grouped.iter().find(|g| g.0 == key).map(|g| g.1.clone()).unwrap_or_default();
            let who = if r.value == "0.0.0.0/0" || r.value == "::/0" { "anyone who can reach this computer" } else { "your local network" };
            out.push(Item { title: r.comment.clone(), detail: format!("{} {} from {who}{off}", r.proto.label(), if r.ports.is_empty() { "all ports".to_string() } else { format!("port {}", r.ports) }), preset: None, rules: ids });
        } else {
            let ports = if r.ports.trim().is_empty() { "every port".to_string() } else { format!("port {}", r.ports) };
            out.push(Item { title: r.value.clone(), detail: format!("{}: {} {}{off}{}", r.kind.label(), r.proto.label(), ports, if r.comment.is_empty() { String::new() } else { format!(" · {}", r.comment) }), preset: None, rules: vec![r.id] });
        }
    }
    out
}

/// Marks the rules a custom-port Add made, so the list can show them as one line.
pub(super) const PORT_TAG: &str = "Port ";

/// What the Add list offers.
pub(super) const SERVICES: [(&str, Option<Preset>); 7] = [
    ("Web server: web pages on ports 80 and 443", Some(Preset::WebServer)),
    ("SSH: a remote command line (port 22)", Some(Preset::Ssh)),
    ("File sharing: the folders you share", Some(Preset::SharedFolders)),
    ("Remote desktop: use this computer from a browser", Some(Preset::RemoteDesktop)),
    ("Kubernetes (k3s): the cluster API (port 6443)", Some(Preset::K3sApi)),
    ("Custom port…", None),
    ("A computer by name or address…", None),
];

pub(super) fn service_items() -> Vec<String> {
    SERVICES.iter().map(|s| s.0.to_string()).collect()
}

pub(super) fn port_form() -> Form {
    Form::new("Allow a port")
        .text("Port", "", "one port such as 8080, several such as 80,443, or a range such as 8000-8100")
        .choice("Protocol", &["TCP and UDP", "TCP", "UDP"], 1)
        .choice("Who may connect", &["Computers on my local network (recommended)", "Anyone who can reach this computer"], 0)
        .text("Note", "", "optional: what it is for (up to 60 characters)")
}

pub(super) fn address_form() -> Form {
    Form::new("Allow a computer or network")
        .text("Name, address or range", "", "a computer name (nas.local), an address (192.168.1.20), a range (192.168.1.10-192.168.1.50) or a network (192.168.1.0/24)")
        .text("Port", "", "empty: every port. Or one such as 22, or several such as 80,443")
        .text("Note", "", "optional: what it is for")
}

/// What kind of source a pasted value is.
pub(super) fn guess_kind(v: &str) -> FwKind {
    let v = v.trim();
    if v.parse::<std::net::IpAddr>().is_ok() {
        FwKind::Ip
    } else if v.contains('/') {
        FwKind::Cidr
    } else if v.split_once('-').is_some_and(|(a, b)| a.trim().parse::<std::net::IpAddr>().is_ok() && b.trim().parse::<std::net::IpAddr>().is_ok()) {
        FwKind::Range
    } else {
        FwKind::Host
    }
}

fn cidr_text(a: &Addr) -> String {
    match a {
        Addr::Cidr(ip, l) => format!("{ip}/{l}"),
        Addr::Exact(ip) => ip.to_string(),
        Addr::Range(a, b) => format!("{a}-{b}"),
    }
}

/// The rules a custom port makes: one per local network, or one pair for anyone. All are checked here.
pub(super) fn port_rules(f: &Form) -> Result<Vec<Rule>, String> {
    let ports = f.value(0).trim().to_string();
    if ports.is_empty() {
        return Err("Type the port to open, for example 8080.".into());
    }
    let proto = match f.choice_idx(1) {
        0 => Proto::Both,
        2 => Proto::Udp,
        _ => Proto::Tcp,
    };
    let note = f.value(3).trim().chars().take(60).collect::<String>();
    let comment = if note.is_empty() { format!("{PORT_TAG}{ports}") } else { format!("{PORT_TAG}{ports}: {note}") };
    let sources: Vec<String> = if f.choice_idx(2) == 1 { vec!["0.0.0.0/0".into(), "::/0".into()] } else { local_networks(true).iter().map(cidr_text).collect() };
    let mut out = Vec::new();
    for s in sources {
        let mut r = Rule::new(FwKind::Cidr, &s);
        r.proto = proto;
        r.ports = ports.clone();
        r.comment = comment.clone();
        iyakki_fw::validate::check_rule(&r).map_err(|e| format!("{e} Check the port and try again."))?;
        out.push(r);
    }
    Ok(out)
}

pub(super) fn address_rule(f: &Form) -> Result<Rule, String> {
    let v = f.value(0);
    if v.trim().is_empty() {
        return Err("Paste or type a computer name, an address, a range or a network.".into());
    }
    let mut r = Rule::new(guess_kind(&v), &v);
    r.ports = f.value(1).trim().to_string();
    r.comment = f.value(2).trim().to_string();
    iyakki_fw::validate::check_rule(&r).map_err(|e| format!("{e} Check what you typed and try again."))?;
    Ok(r)
}

/// The words under the big status in the simple view.
pub(super) fn simple_status(v: &View, changes: usize, writable: bool) -> Vec<(String, [u8; 4])> {
    let mut out: Vec<(String, [u8; 4])> = Vec::new();
    let blocking = "Everything that comes in from the network is blocked, except what is allowed below.";
    match &v.status {
        Status::On => out.push((blocking.into(), TEXT)),
        Status::Pending { secs_left } => {
            out.push((format!("Keep this change? {secs_left} s left, then it is undone by itself."), AMBER));
            out.push((blocking.into(), TEXT));
        }
        Status::Off => out.push(("The firewall is Open: nothing is blocked. Choose Standard to protect this computer.".into(), AMBER)),
        Status::OffThisBoot => out.push(("Open for this start only (it was asked for when the computer started). It goes back to Standard at the next restart.".into(), AMBER)),
        Status::NotProtected(e) => out.push((format!("NOT PROTECTED: {e}. Nothing is being blocked. Restart the computer; if it stays like this, open Advanced for details."), ERR)),
        Status::Unavailable(e) => out.push((format!("{e}. Nothing is being blocked."), ERR)),
    }
    if v.ssh_boot {
        out.push(("Remote diagnostics is on (set when the computer started), so SSH is allowed from the local network.".into(), AMBER));
    }
    if let Some(e) = &v.last_error {
        if !matches!(v.status, Status::NotProtected(_) | Status::Unavailable(_)) {
            out.push((format!("The last change was not applied: {e} Your earlier settings are still in force."), ERR));
        }
    }
    if changes > 0 {
        out.push((format!("{changes} change(s) from Advanced are not applied yet. Apply them (you then have 60 s to keep them) or Discard them."), AMBER));
    }
    if !writable {
        out.push(("This window runs on another computer: this page shows IYAKKI's firewall and changes nothing here.".into(), MUTED));
    }
    out
}

impl SettingsApp {
    pub(super) fn fw_items(&self) -> Vec<Item> {
        items(&self.fw_cfg())
    }

    /// Apply `c` at once (the engine then asks "Keep these settings?" for 60 seconds). A failure leaves it as a draft so the
    /// person sees it and can discard it.
    pub(super) fn fw_commit(&mut self, cx: &AppCx, c: Config, audit: &str, done: &str) {
        let user = cx.session.user.clone();
        let r = self.fw.handle.lock().propose(c.clone(), &user);
        match r {
            Ok(()) => {
                self.fw.draft = None;
                cx.audit.record(&user, "firewall.apply", audit);
                let pending = matches!(self.fw.handle.lock().status(), Status::Pending { .. });
                self.ok(if pending { format!("{done} Choose Keep within 60 seconds, or Undo. If you do nothing it is undone.") } else { done.to_string() });
            }
            Err(e) => {
                self.fw.draft = Some(c);
                self.fail(format!("Not applied: {e} Choose Discard to go back to what is in force."));
            }
        }
    }

    /// A key in the simple view. Returns whether it was used.
    pub(super) fn handle_fw_simple(&mut self, k: &Key, cx: &AppCx) -> bool {
        let n = self.fw_items().len();
        let sel = self.sel[self.tab].min(n.saturating_sub(1));
        match k {
            Key::Up => {
                self.move_sel(-1, n);
                return true;
            }
            Key::Down => {
                self.move_sel(1, n);
                return true;
            }
            Key::Char('a' | 'd' | '1' | '2' | '3') => {}
            Key::Char('e' | 'x' | 'p' | 'o' | 'i' | 'b') => {
                // advanced only: say where it is instead of doing something the list does not show
                self.ok("That is in Advanced. Choose Advanced… below the list.");
                return true;
            }
            _ => return false,
        }
        if !self.admin(cx) {
            return true;
        }
        if !self.fw.writable {
            self.fail("This window runs on another computer; it cannot change IYAKKI's firewall.");
            return true;
        }
        let cfg = self.fw_cfg();
        match k {
            Key::Char('a') => {
                self.modal = Some(Modal::Pick(PickKind::FwService, Picker::new("Allow what?", service_items())));
            }
            Key::Char('d') => match self.fw_items().get(sel).cloned() {
                None => self.fail("There is nothing to remove: nothing is allowed in yet."),
                Some(it) => {
                    let lockout = matches!(it.preset, Some(Preset::Ssh | Preset::RemoteDesktop));
                    if lockout {
                        let what = it.title.clone();
                        self.modal = Some(Modal::Confirm(ConfirmKind::Fw(FwAct::Remove(it.preset, it.rules)), format!("Stop allowing {what}? If you are using this computer through {what} right now, you will lose the connection. The change is undone by itself after 60 seconds unless you choose Keep, so you can get back in.")));
                    } else {
                        self.fw_remove(cx, it.preset, &it.rules);
                    }
                }
            },
            Key::Char('1') => {
                if cfg.enabled {
                    self.ok("Already Standard: everything coming in is blocked except what is allowed below.");
                } else {
                    let mut c = cfg;
                    c.enabled = true;
                    self.fw_commit(cx, c, "mode standard", "Standard is on.");
                }
            }
            Key::Char('2') => {
                if !cfg.enabled {
                    self.ok("Already Open: nothing is blocked.");
                } else {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Fw(FwAct::Open), "Switch the firewall to Open? Nothing coming in will be blocked: anything on this computer that listens can be reached from the network. You can choose Standard again at any time.".into()));
                }
            }
            Key::Char('3') => self.fail("Locked down is not available yet: this firewall only filters connections coming in, it cannot restrict outgoing ones. Standard is the strictest choice today."),
            _ => {}
        }
        true
    }

    pub(super) fn fw_remove(&mut self, cx: &AppCx, preset: Option<Preset>, rules: &[u32]) {
        let mut c = self.fw_cfg();
        if let Some(p) = preset {
            c.set_preset(p, false);
        }
        for id in rules {
            c.remove(*id);
        }
        let what = preset.map_or_else(|| "rule".to_string(), |p| preset_words(p).0.to_string());
        self.fw_commit(cx, c, &format!("removed {what} from Settings"), "Removed.");
        let n = self.fw_items().len();
        self.sel[self.tab] = self.sel[self.tab].min(n.saturating_sub(1));
    }

    pub(super) fn fw_confirm(&mut self, cx: &AppCx, act: FwAct) {
        if !self.admin(cx) || !self.fw.writable {
            return;
        }
        match act {
            FwAct::Open => {
                let mut c = self.fw_cfg();
                c.enabled = false;
                self.fw_commit(cx, c, "mode open", "The firewall is Open.");
            }
            FwAct::Remove(p, ids) => self.fw_remove(cx, p, &ids),
        }
    }

    /// A line of the Add list was chosen.
    pub(super) fn fw_service_pick(&mut self, cx: &AppCx, item: &str) {
        let Some(i) = SERVICES.iter().position(|s| s.0 == item) else { return };
        match SERVICES[i].1 {
            Some(p) => {
                let mut c = self.fw_cfg();
                if c.has(p) {
                    return self.ok(format!("{} is already allowed.", preset_words(p).0));
                }
                c.set_preset(p, true);
                self.fw_commit(cx, c, &format!("allowed {} from Settings", preset_words(p).0), &format!("{} is now allowed from your local network.", preset_words(p).0));
            }
            None if i == 5 => self.open_form(FormKind::Fw(FwForm::Port), port_form()),
            None => self.open_form(FormKind::Fw(FwForm::Address), address_form()),
        }
    }

    /// A custom port or address form was submitted: apply at once.
    pub(super) fn fw_simple_submit(&mut self, cx: &AppCx, kind: &FwForm, form: &Form) -> Result<String, String> {
        let mut c = self.fw_cfg();
        let (audit, done) = match kind {
            FwForm::Port => {
                let rules = port_rules(form)?;
                let ports = form.value(0);
                for r in rules {
                    c.add(r);
                }
                (format!("allowed port {ports}"), format!("Port {ports} is now allowed."))
            }
            _ => {
                let r = address_rule(form)?;
                let what = r.value.clone();
                c.add(r);
                (format!("allowed {what}"), format!("{what} is now allowed."))
            }
        };
        if c.rules.len() > iyakki_fw::model::MAX_RULES {
            return Err(format!("At most {} rules.", iyakki_fw::model::MAX_RULES));
        }
        self.fw_commit(cx, c, &audit, &done);
        Ok(self.status.as_ref().map(|s| s.0.clone()).unwrap_or(done))
    }

    pub(super) fn fw_poll_counters(&mut self) {
        if self.fw.counters_due() {
            if let Some(c) = self.fw.handle.lock().counters() {
                self.fw.counters = c;
            }
            self.fw.counters_at = Some(Instant::now());
        }
    }

    pub(super) fn render_firewall_simple(&mut self, g: &mut Gfx, a: R, s: f32, _cx: &AppCx) {
        let v = self.fw.handle.lock().view();
        let base = self.fw_base();
        let cfg = self.fw_cfg();
        let changes = self.fw.draft.as_ref().map_or(0, |d| change_count(d, &base));
        let bottom = a.y + a.h;
        let mut y = a.y;
        let (word, col) = match &v.status {
            Status::On | Status::Pending { .. } => ("FIREWALL ON", OK),
            Status::Off | Status::OffThisBoot => ("FIREWALL OPEN", AMBER),
            Status::NotProtected(_) | Status::Unavailable(_) => ("NOT PROTECTED", ERR),
        };
        let size = if a.w < 330.0 * s { 24.0 * s } else { 30.0 * s };
        let head = g.fit(size, word, Face::Bold, a.w);
        g.text(a.x, y, size, col, &head, Face::Bold);
        y += size * 1.3;
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
        for (t, c) in simple_status(&v, changes, self.fw.writable) {
            line(g, &mut y, &t, c, 13.0 * s, Face::Sans);
        }
        // the three ways to run it
        y += 4.0 * s;
        let gap = 8.0 * s;
        let cw = ((a.w - 2.0 * gap) / 3.0).max(40.0 * s);
        let ch = 42.0 * s;
        if y + ch > bottom {
            return;
        }
        let cur = Mode::of(&cfg);
        for (i, m) in Mode::ALL.iter().enumerate() {
            let r = R::new(a.x + i as f32 * (cw + gap), y, cw, ch);
            let on = *m == cur;
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, if on { CARD_SEL } else if hot { CARD_HOVER } else { CARD });
            if on {
                g.rrect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, 2.0 * s, ACCENT);
            }
            let t = g.fit(15.0 * s, m.title(), Face::Bold, r.w - 20.0 * s);
            let lh = g.line_height(15.0 * s, Face::Bold);
            let tc = if *m == Mode::Locked { DIM } else if on { TEXT } else { MUTED };
            g.text(r.x + 14.0 * s, r.y + (r.h - lh) / 2.0, 15.0 * s, tc, &t, Face::Bold);
            g.hit(r, Event::Key(Key::Char(m.key())), m.title());
        }
        y += ch + 6.0 * s;
        line(g, &mut y, cur.explain(), MUTED, 12.0 * s, Face::Sans);
        if v.status.protected() {
            line(g, &mut y, "Always allowed: replies to connections this computer started, and this computer talking to itself.", DIM, 12.0 * s, Face::Sans);
        }
        y += 4.0 * s;
        // what is allowed in
        let list = items(&cfg);
        line(g, &mut y, "Allowed on this computer", TEXT, 15.0 * s, Face::Bold);
        if list.is_empty() {
            line(g, &mut y, "Nothing yet, so other computers cannot reach anything on this one. Choose Add to allow something.", DIM, 13.0 * s, Face::Sans);
        }
        let sel = self.sel[self.tab].min(list.len().saturating_sub(1));
        let rh = 60.0 * s;
        let fit = (((bottom - y - 22.0 * s) / rh) as usize).max(1);
        let first = if sel < fit { 0 } else { sel + 1 - fit }.min(list.len().saturating_sub(fit));
        for (i, it) in list.iter().enumerate().skip(first).take(fit) {
            if y + rh > bottom + 0.5 {
                break;
            }
            let in_force = match it.preset {
                Some(p) => base.has(p),
                None => it.rules.iter().all(|id| base.rules.iter().any(|r| r.id == *id && cfg.rules.iter().any(|x| x == r))),
            };
            let chips: Vec<(&str, [u8; 4])> = vec![if in_force { ("allowed", OK) } else { ("not applied", AMBER) }];
            let (t, d) = (it.title.clone(), g.fit(13.0 * s, &it.detail, Face::Sans, a.w - 36.0 * s - 150.0 * s));
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &t, &d, &chips);
            y += rh;
        }
        self.fw_poll_counters();
        if y + 18.0 * s <= bottom {
            let t = g.fit(12.0 * s, &format!("{} connection attempts blocked so far. Details are in Advanced.", self.fw.counters.0), Face::Sans, a.w);
            g.text(a.x, y, 12.0 * s, DIM, &t, Face::Sans);
        }
    }
}
