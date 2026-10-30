//! Settings → Developer mode (administrators): switch it on or off with an optional automatic switch-off, pair a computer (the code and
//! the certificate fingerprint are shown here, with the addresses to type), the paired computers (revoke one), and the activity log of
//! everything they did. The rules and the work are in `iyakki_agent::devmode`; the listener is `iyakki_remote::devserver`.

use super::*;
use iyakki_agent::devmode::scopes::Scope;
use iyakki_agent::devmode::{self as dm, DevMode};

/// The automatic switch-off choices, in hours (0: stays on until switched off).
pub(super) const HOURS: [u32; 6] = [4, 1, 8, 24, 72, 0];
/// What the "Extend" button adds to the switch-off time.
pub(super) const EXTEND_HOURS: u32 = 4;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct DevPanel {
    /// Index into [`HOURS`] for the next switch-on.
    hours: usize,
    /// Show the activity log instead of the computers.
    log: bool,
    /// Whether it was on when the page was last drawn (for the buttons).
    on: bool,
    /// The scope the switches act on (index into `Scope::ALL`).
    scope: usize,
    /// The "Connect over USB-C" page (key `u`) is showing instead of the computers list.
    pub(super) usb_open: bool,
    pub(super) usb: super::usbc_page::UsbPanel,
}

fn hours_text(h: u32) -> String {
    if h == 0 { "never (until switched off)".into() } else { format!("after {h} h") }
}

impl SettingsApp {
    fn devmode(cx: &AppCx) -> DevMode {
        DevMode::open(cx.services.dir())
    }

    pub(super) fn devmode_keys(cx: &AppCx) -> Vec<dm::DevKey> {
        Self::devmode(cx).keys()
    }

    pub(super) fn handle_developer(&mut self, k: &Key, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        if self.devel.usb_open {
            self.handle_usb(k, cx);
            return;
        }
        let d = Self::devmode(cx);
        let p = &cx.session.policy;
        let user = cx.session.user.clone();
        match k {
            Key::Char('u') => self.devel.usb_open = true,
            Key::Char('e') | Key::Enter if !d.is_on() => match d.enable(p, &user, HOURS[self.devel.hours]) {
                Ok(_) => {
                    cx.audit.record(&user, "devmode.on", &hours_text(HOURS[self.devel.hours]));
                    self.ok(format!("Developer mode is on (port {}). It switches off {}. Pair a computer with p.", d.state().port(), hours_text(HOURS[self.devel.hours])));
                }
                Err(e) => self.fail(e),
            },
            Key::Char('e') => match d.disable(p, &user) {
                Ok(()) => {
                    cx.audit.record(&user, "devmode.off", "switched off in Settings");
                    self.ok("Developer mode is off: the port is closed and every request is refused.");
                }
                Err(e) => self.fail(e),
            },
            Key::Char('h') => {
                self.devel.hours = (self.devel.hours + 1) % HOURS.len();
                let h = HOURS[self.devel.hours];
                if d.is_on() {
                    // apply to the running session at once
                    match d.enable(p, &user, h) {
                        Ok(_) => self.ok(format!("Switches off {} from now.", hours_text(h))),
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.ok(format!("When switched on it will switch off {}.", hours_text(h)));
                }
            }
            Key::Char('p') => match d.start_pairing(p, &user) {
                Ok(_) => {
                    cx.audit.record(&user, "devmode.pair.open", "pairing code shown");
                    self.ok("Type the code on the computer: iyakki dev pair <address> (good for 5 minutes).");
                }
                Err(e) => self.fail(e),
            },
            // let the firewall through: the pairing (one time) when a code is on the screen, else the computer the hint names
            Key::Char('f') => {
                let hint_src = self.dev_fw_hint(&d).and_then(|h| h.split(" from ").nth(1).map(|x| x.trim_end_matches(": Allow").to_string()));
                if d.pairing().is_some() {
                    match d.allow_pairing_through_firewall(p, &user) {
                        Ok(_) => {
                            cx.audit.record(&user, "devmode.firewall.request", "pairing (one time)");
                            self.ok("The firewall lets the local network reach the developer port for ten minutes, or until a computer has paired. The pairing code and the fingerprint are still what protects it.");
                        }
                        Err(e) => self.fail(e),
                    }
                } else if let Some(src) = hint_src {
                    match d.allow_peer(p, &user, &src) {
                        Ok(()) => {
                            cx.audit.record(&user, "devmode.firewall.request", &format!("{src} while developer mode is on"));
                            self.ok(format!("The firewall lets {src} reach the developer port while developer mode is on."));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.fail("Nothing to allow: press p to show a pairing code first, then f.".to_string());
                }
            }
            Key::Char('n') => {
                let now_on = !d.network_wide();
                match d.set_network_wide(p, &user, now_on) {
                    Ok(()) => self.ok(if now_on { "The firewall now lets the whole network of each paired computer reach the developer port (only paired keys can act)." } else { "The firewall lets only the address of each paired computer in." }),
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('c') => {
                d.cancel_pairing();
                self.ok("The pairing code is cancelled.");
            }
            Key::Char('x') | Key::Delete => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(k) = rows.get(sel) {
                    match d.revoke(p, &user, &k.name) {
                        Ok(k) => {
                            cx.audit.record(&user, "devmode.revoke", &k.name);
                            self.ok(format!("{} can no longer reach this device.", k.name));
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            Key::Char('l') => self.devel.log = !self.devel.log,
            // push the automatic switch-off 4 h further (the timer of "h" starts it again from now)
            Key::Char('t') => match d.extend(p, &user, EXTEND_HOURS) {
                Ok(st) => {
                    cx.audit.record(&user, "devmode.extend", &format!("{EXTEND_HOURS} h"));
                    self.ok(format!("Developer mode now switches off at {}.", dm::stamp(st.until)));
                }
                Err(e) => self.fail(e),
            },
            // every diagnostic scope for the selected computer, in one go: look, files, screen, cluster, shell (still approved here)
            Key::Char('G') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(row) = rows.get(sel) {
                    match d.grant_diagnostic(p, &user, &row.name) {
                        Ok(nk) => {
                            cx.audit.record(&user, "devmode.scope.grant", &format!("{} diagnostic scopes", nk.name));
                            self.ok(format!("{} may now help diagnose: {}. A shell still needs your approval each time; updates and power stay off.", nk.name, nk.scope_text()));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.fail("No computer is paired yet.".to_string());
                }
            }
            Key::Char('s') => self.devel.scope = (self.devel.scope + 1) % Scope::ALL.len(),
            Key::Char(' ') | Key::Char('g') => {
                let sc = Scope::ALL[self.devel.scope % Scope::ALL.len()];
                if matches!(k, Key::Char('g')) {
                    // the computer that is about to pair
                    let on = !d.pairing().is_some_and(|p| p.scopes.iter().any(|x| x == sc.id()));
                    match d.set_pairing_scope(p, &user, sc, on) {
                        Ok(l) => self.ok(format!("The computer that pairs will get: {}.", l.join(", "))),
                        Err(e) => self.fail(e),
                    }
                } else {
                    let rows = Self::devmode_keys(cx);
                    let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                    if let Some(row) = rows.get(sel) {
                        let on = !row.has(sc);
                        match d.set_scope(p, &user, &row.name, sc, on) {
                            Ok(nk) => {
                                cx.audit.record(&user, if on { "devmode.scope.grant" } else { "devmode.scope.revoke" }, &format!("{} {}", nk.name, sc.id()));
                                self.ok(format!("{} now holds: {}.", nk.name, nk.scope_text()));
                            }
                            Err(e) => self.fail(e),
                        }
                    }
                }
            }
            Key::Char('a') | Key::Char('o') | Key::Char('d') | Key::Char('y') => {
                let Some(req) = d.shell_pending().into_iter().next() else {
                    self.fail("No computer is asking for shell access right now.".to_string());
                    return;
                };
                let r = match k {
                    Key::Char('a') => d.shell_approve(p, &user, &req.computer, 10).map(|g| format!("{} may run commands for 10 minutes{}. The top bar shows it.", g.computer, if g.as_root { " as root" } else { "" })),
                    Key::Char('o') => d.shell_approve(p, &user, &req.computer, 0).map(|g| format!("{} may run commands for one session.", g.computer)),
                    // approve this one and stop asking for a while: a plain shell only (never as root, power or patching)
                    Key::Char('y') if req.as_root => Err("A request to run as root is always asked each time: press a or o.".to_string()),
                    Key::Char('y') => d.shell_approve(p, &user, &req.computer, 10).and_then(|g| d.shell_auto_on(p, &user, &req.computer, iyakki_agent::devmode::grants::AUTO_DEFAULT_HOURS).map(|_| format!("{} may run commands; its next requests are approved by themselves for {} hours (never as root). w stops it.", g.computer, iyakki_agent::devmode::grants::AUTO_DEFAULT_HOURS))),
                    _ => d.shell_deny(p, &user, &req.computer).map(|_| format!("{} was refused.", req.computer)),
                };
                match r {
                    Ok(m) => {
                        cx.audit.record(&user, "devmode.shell", &m);
                        self.ok(m)
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('w') => {
                let trusted = d.shell_auto_list();
                if trusted.is_empty() {
                    self.fail("No computer is approved automatically right now.".to_string());
                    return;
                }
                let mut m = String::new();
                for a in &trusted {
                    if let Err(e) = d.shell_auto_off(p, &user, &a.computer) {
                        self.fail(e);
                        return;
                    }
                    m = format!("{}{}{}", m, if m.is_empty() { "" } else { ", " }, a.computer);
                }
                cx.audit.record(&user, "devmode.shell.auto_off", &m);
                self.ok(format!("{m} must ask for approval again."));
            }
            Key::Char('k') => {
                // end the selected computer's shell access and disconnect all its sessions
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(row) = rows.get(sel) {
                    let _ = d.shell_end(p, &user, &row.name);
                    let n = dm::sessions::end_computer(cx.services.dir(), &row.name);
                    cx.audit.record(&user, "devmode.disconnect", &row.name);
                    self.ok(format!("{} is disconnected ({n} session(s) ended, shell access ended).", row.name));
                }
            }
            _ => {}
        }
    }

    /// "the firewall blocks the developer port ...: Allow" while developer mode is on, the firewall is in force and nothing lets it in.
    pub(super) fn dev_fw_hint(&self, d: &DevMode) -> Option<String> {
        let blocked = iyakki_fw::service::block_log().recent();
        iyakki_fw::devrules::hint(&self.fw.handle.lock(), &blocked, d.state().port(), d.is_on())
    }

    pub(super) fn developer_buttons(&self) -> Vec<Btn> {
        if self.devel.usb_open {
            return self.usb_buttons();
        }
        let on = self.devel.on;
        let mut v: Vec<Btn> = vec![(if on { "Switch off" } else { "Switch on" }, if on { ERR } else { ACCENT }, 'e'), ("Switch-off time", TEXT, 'h')];
        if on {
            v.push(("Pair a computer", ACCENT, 'p'));
            v.push(("Allow in the firewall", ACCENT, 'f'));
            v.push(("Whole network on/off", TEXT, 'n'));
            v.push(("Cancel code", TEXT, 'c'));
        }
        if on {
            v.push(("Extend 4 h", TEXT, 't'));
        }
        v.push(("Grant all diagnostic scopes", ACCENT, 'G'));
        v.push(("Next scope", TEXT, 's'));
        v.push(("Scope on/off", TEXT, ' '));
        if on {
            v.push(("Approve shell", ERR, 'a'));
            v.push(("Shell once", ERR, 'o'));
            v.push(("Deny shell", TEXT, 'd'));
        }
        v.push(("Disconnect", ERR, 'k'));
        v.push(("Revoke", ERR, 'x'));
        v.push(("Activity log", TEXT, 'l'));
        v.push(("Connect over USB-C", ACCENT, 'u'));
        v
    }

    pub(super) fn render_developer(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if self.devel.usb_open {
            self.render_usb(g, a, s, cx);
            return;
        }
        let d = Self::devmode(cx);
        let on = d.is_on();
        self.devel.on = on;
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Install and run apps from your own computer");
        for l in [
            "Like adb for IYAKKI: a paired computer can install, upgrade and remove tile apps, skills, MCP servers and extensions, read their logs, open them on this screen and take a screenshot (iyakki dev ...).".to_string(),
            "What it installs is labelled Developer (local), goes through the STORE's checks and is never updated from a store. Driver packs and kernel modules cannot be installed this way. Off by default; every action is logged.".to_string(),
        ] {
            for w in g.wrap(13.5 * s, &l, Face::Sans, a.w) {
                g.text(a.x, y, 13.5 * s, MUTED, &w, Face::Sans);
                y += 19.0 * s;
            }
        }
        y += 6.0 * s;
        let st = d.state();
        let (word, col) = if on { (d.status_text(), rgb(255, 196, 94)) } else { ("Off: nothing listens, every request is refused".to_string(), OK) };
        let head = g.fit(16.0 * s, &format!("Developer mode: {word}"), Face::Bold, a.w);
        g.text(a.x, y, 16.0 * s, col, &head, Face::Bold);
        y += 26.0 * s;
        g.text(a.x, y, 13.0 * s, MUTED, &format!("Port {} (TLS) · next switch-on switches off {}", st.port(), hours_text(HOURS[self.devel.hours])), Face::Sans);
        y += 24.0 * s;
        if let Some(p) = d.pairing().filter(|_| on) {
            y += controls::section_title(g, a.x, y, s, "Pair a computer");
            g.text(a.x, y, 30.0 * s, ACCENT, &p.code, Face::Mono);
            y += 42.0 * s;
            let fp = dm::cert_fingerprint(cx.services.dir()).unwrap_or_else(|| "(the certificate is being made: look again in a moment)".into());
            let half = fp.len().div_ceil(2).min(48);
            for part in [&fp[..half.min(fp.len())], &fp[half.min(fp.len())..]] {
                if !part.is_empty() {
                    g.text(a.x, y, 13.0 * s, TEXT, part.trim_start_matches(':'), Face::Mono);
                    y += 18.0 * s;
                }
            }
            let addrs: Vec<String> = iyakki_devctl::ipv4_addresses().into_iter().filter(|(_, ip)| !ip.starts_with("127.")).map(|(_, ip)| format!("{ip}:{}", st.port())).collect();
            let line = format!("On the computer: iyakki dev pair {} — then type this code. The fingerprint above must match what it prints. The computer will act as {} ({} left).", addrs.first().cloned().unwrap_or_else(|| format!("<this device's address>:{}", st.port())), p.account, {
                let left = (p.expires - d.now()).max(0);
                format!("{}:{:02}", left / 60, left % 60)
            });
            for w in g.wrap(13.0 * s, &line, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 18.0 * s;
            }
            let granted = p.scopes.join(", ");
            let line = format!("This computer will be allowed: {granted}. (s picks a scope, g adds or removes it for the new computer; more can be granted after pairing.)");
            for w in g.wrap(13.0 * s, &line, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), &w, Face::Sans);
                y += 18.0 * s;
            }
            y += 8.0 * s;
        }
        if on {
            let w = d.fw_want();
            let mut lines: Vec<(String, [u8; 4])> = Vec::new();
            if let Some(h) = self.dev_fw_hint(&d) {
                lines.push((format!("{h} (f)"), rgb(255, 196, 94)));
            }
            if w.pair_until > 0 {
                lines.push((format!("Firewall: the local network may reach the developer port until {} or until a computer has paired (rule: developer mode pairing (one time)).", dm::stamp(w.pair_until)), TEXT));
            }
            lines.push((if w.network_wide {
                "Firewall: paired computers are allowed from their whole network (/24). Trade-off: any computer on this network can reach the developer port, though only paired keys can act. n turns it off.".to_string()
            } else {
                "Firewall: only the address of each paired computer is let in (default). If its address changes, press f when the hint shows, or n to allow its whole network (any computer there could then reach the port; only paired keys can act).".to_string()
            }, MUTED));
            for (t, c) in lines {
                for w in g.wrap(13.0 * s, &t, Face::Sans, a.w) {
                    g.text(a.x, y, 13.0 * s, c, &w, Face::Sans);
                    y += 18.0 * s;
                }
            }
            y += 6.0 * s;
            for t in d.shell_auto_list() {
                let line = format!("{} runs commands without asking until {} (never as root). w: ask each time again", t.computer, dm::stamp(t.until));
                for w in g.wrap(13.0 * s, &line, Face::Sans, a.w) {
                    g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                    y += 18.0 * s;
                }
            }
            for r in d.shell_pending() {
                let line = format!("{} asks to RUN COMMANDS on this device as {}{}{}. a: 10 minutes · o: one session · y: yes, and stop asking for 4 hours · d: refuse", r.computer, r.account, if r.as_root { " (as root)" } else { "" }, if r.why.is_empty() { String::new() } else { format!(" — {}", r.why) });
                for w in g.wrap(14.0 * s, &line, Face::Bold, a.w) {
                    g.text(a.x, y, 14.0 * s, ERR, &w, Face::Bold);
                    y += 20.0 * s;
                }
                y += 6.0 * s;
            }
            let sessions = dm::sessions::list(cx.services.dir());
            if !sessions.is_empty() {
                y += controls::section_title(g, a.x, y, s, "Open sessions (k disconnects the selected computer)");
                for x in sessions.iter().take(8) {
                    let t = g.fit(13.0 * s, &format!("{} ({}) · {} · since {}", x.computer, x.account, x.what, dm::stamp(x.started)), Face::Mono, a.w);
                    g.text(a.x, y, 13.0 * s, TEXT, &t, Face::Mono);
                    y += 18.0 * s;
                }
                y += 6.0 * s;
            }
        }
        if self.devel.log {
            y += controls::section_title(g, a.x, y, s, "Activity log (newest first)");
            let acts = d.activity(60);
            if acts.is_empty() {
                g.text(a.x, y, 14.0 * s, DIM, "Nothing yet.", Face::Sans);
            }
            for x in acts {
                if y > a.y + a.h - 20.0 * s {
                    break;
                }
                let t = g.fit(12.5 * s, &x.line(), Face::Mono, a.w);
                g.text(a.x, y, 12.5 * s, if x.ok { TEXT } else { ERR }, &t, Face::Mono);
                y += 18.0 * s;
            }
            return;
        }
        y += controls::section_title(g, a.x, y, s, "Paired computers");
        let rows = Self::devmode_keys(cx);
        if rows.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, if on { "None yet. Press p to show a pairing code." } else { "None. Switch developer mode on (e), then pair a computer (p)." }, Face::Sans);
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        for (i, k) in rows.iter().enumerate() {
            if y + 64.0 * s > a.y + a.h {
                break;
            }
            let used = if k.last_used == 0 { "never used".to_string() } else { format!("last used {}", dm::stamp(k.last_used)) };
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, i == sel, &k.name, &format!("key {} · acts as {} · paired {} by {} · {used} · scopes: {}", k.short(), k.account, dm::stamp(k.added), k.added_by, k.scope_text()), &[(if on { "can connect" } else { "mode off" }, if on { rgb(255, 196, 94) } else { DIM })]);
            y += 66.0 * s;
        }
        // the switches of the selected computer: one line per scope, the cursor on the one s/space act on
        if let Some(k) = rows.get(sel) {
            y += controls::section_title(g, a.x, y, s, &format!("What {} may do (s: next, space: on/off, G: all diagnostic scopes)", k.name));
            let note = "Grant all diagnostic scopes gives: debug, files, input, kube and shell (a shell still needs your approval each time). It never gives updates, restart/power, port forwarding, system files, password fields or settings.";
            for w in g.wrap(12.5 * s, note, Face::Sans, a.w) {
                g.text(a.x, y, 12.5 * s, MUTED, &w, Face::Sans);
                y += 17.0 * s;
            }
            for (i, sc) in Scope::ALL.into_iter().enumerate() {
                if y + 20.0 * s > a.y + a.h {
                    break;
                }
                let held = k.has(sc);
                let cursor = if i == self.devel.scope % Scope::ALL.len() { "▶" } else { " " };
                let col = if held && sc.dangerous() { ERR } else if held { rgb(255, 196, 94) } else { DIM };
                let t = g.fit(13.0 * s, &format!("{cursor} [{}] {:<16} {}", if held { "x" } else { " " }, sc.id(), sc.describe()), Face::Mono, a.w);
                g.text(a.x, y, 13.0 * s, col, &t, Face::Mono);
                y += 18.0 * s;
            }
        }
    }
}
