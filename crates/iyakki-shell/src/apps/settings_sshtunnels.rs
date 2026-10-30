//! Settings -> Network -> SSH tunnels (administrators): see a web page that sits behind a front-facing SSH host. A list of tunnels, each
//! with its own state, that can be added, edited, enabled, disabled, deleted (after asking), tested and retried. A tunnel listens on
//! 127.0.0.1 only; the page shows the address the Browser should open. The rules, the command line and the manager are in
//! `iyakki_agent::{sshtunnel, sshtunnel_mgr}`; the model is explained in `docs/SSH-TUNNELS.md`.
//!
//! The key of a tunnel is a name from Settings -> Git and SSH keys (or one made here for the tunnel): the private half stays in the
//! vault and reaches the client through a key service in memory. A host is trusted only after its fingerprint was shown and
//! confirmed; a changed host key is refused.

use std::sync::Arc;

use iyakki_agent::sshtunnel::{self as tun, Hop, Kind as TKind, Tunnel};
use iyakki_agent::sshtunnel_mgr::{Manager, State, TestReport};

use super::*;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct TunPanel {
    /// Tests: a manager with a fake client instead of the person's real one.
    pub(super) mgr: Option<Arc<Manager>>,
    /// Tests: the clock (Unix seconds).
    pub(super) now: Option<u64>,
    /// A Test in flight: the tunnel's id and its answer.
    testing: Option<(String, Receiver<TestReport>)>,
}

/// The forms of this page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TunForm {
    /// A new tunnel, or the tunnel with this id.
    Edit(Option<String>),
    /// Trust the key of this host (its fingerprint is in the form).
    Trust(String, u16),
}

const KINDS: [&str; 2] = ["One page (local forward)", "Any site (SOCKS proxy)"];

impl SettingsApp {
    fn tun_now(&self) -> u64 {
        self.tun.now.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }

    pub(super) fn tun_mgr(&self, cx: &AppCx) -> Arc<Manager> {
        if let Some(m) = &self.tun.mgr {
            return m.clone();
        }
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        Manager::shared(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }

    pub(super) fn tun_len(&self, cx: &AppCx) -> usize {
        self.tun_mgr(cx).tunnels().len()
    }

    pub(super) fn tun_buttons(&self) -> Vec<Btn> {
        vec![("Add", ACCENT, 'a'), ("Edit", TEXT, 'e'), ("On / Off", TEXT, 'x'), ("Delete", ERR, 'd'), ("Test", TEXT, 'T'), ("Retry", TEXT, 'r'), ("Copy URL", TEXT, 'c'), ("Copy key", TEXT, 'k')]
    }

    /// The heartbeat: collects the answer of a Test, and (only when the supervisor is not doing it) keeps the clients going.
    pub(super) fn tun_tick(&mut self, cx: &AppCx) {
        if self.tab() == Tab::SshTunnels && self.ticks % 20 == 0 {
            self.tun_mgr(cx).tick(self.tun_now());
        }
        let Some((id, rx)) = &self.tun.testing else { return };
        let Ok(r) = rx.try_recv() else { return };
        let id = id.clone();
        self.tun.testing = None;
        let mgr = self.tun_mgr(cx);
        let name = mgr.get(&id).map(|t| t.name).unwrap_or_default();
        if r.ok {
            self.ok(format!("{name}: {}", r.lines.join(" ")));
            return;
        }
        self.fail(format!("{name}: {}", r.lines.join(" ")));
        if let (Some((host, port, keys)), None) = (r.untrusted.first(), &self.modal) {
            let fp = keys.first().map(|k| k.fingerprint()).unwrap_or_default();
            self.open_form(FormKind::SshTun(TunForm::Trust(host.clone(), *port)), Self::tun_trust_form(host, *port, &fp, &iyakki_agent::ssh::describe_keys(keys)));
        }
    }

    fn tun_trust_form(host: &str, port: u16, fp: &str, shown: &[String]) -> Form {
        Form::new(&format!("Trust the host {}?", iyakki_agent::ssh::pattern(host, port)))
            .text("Fingerprint it presents (compare with what its owner gave you)", fp, &shown.join(", "))
            .choice("Trust this host key", &["No, do not trust it", "Yes, it matches"], 0)
    }

    fn tun_row(&self, cx: &AppCx) -> Option<iyakki_agent::sshtunnel_mgr::Row> {
        let rows = self.tun_mgr(cx).rows();
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        rows.into_iter().nth(sel)
    }

    pub(super) fn handle_tun(&mut self, k: &Key, cx: &AppCx) {
        let mgr = self.tun_mgr(cx);
        let len = mgr.tunnels().len();
        let row = self.tun_row(cx);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('a') => self.tun_open_form(cx, None),
            Key::Char('e') => match row {
                Some(r) => self.tun_open_form(cx, Some(r.tunnel)),
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('x') => match row {
                Some(r) => {
                    let on = !r.tunnel.enabled;
                    match mgr.set_enabled(&r.tunnel.id, on) {
                        Ok(t) if on => {
                            mgr.ensure_supervised();
                            mgr.tick(self.tun_now());
                            self.ok(format!("{} is enabled: it starts with this device and reconnects by itself.", t.name));
                        }
                        Ok(t) => self.ok(format!("{} is disabled: it is stopped and kept, and will not start until you enable it.", t.name)),
                        Err(e) => self.fail(e),
                    }
                }
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('d') | Key::Delete => match row {
                Some(r) => {
                    let text = format!("Delete the tunnel “{}”? {}This cannot be undone.", r.tunnel.name, if r.tunnel.own_key { "Its key is deleted from your vault too. " } else { "" });
                    self.modal = Some(Modal::Confirm(ConfirmKind::SshTunnelDelete(r.tunnel.id), text));
                }
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('T') => match row {
                Some(_) if self.tun.testing.is_some() => self.fail("A test is still running."),
                Some(r) => {
                    let (id, now, m) = (r.tunnel.id.clone(), self.tun_now(), mgr.clone());
                    let (tx, rx) = channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(m.test(&id, now));
                    });
                    self.tun.testing = Some((r.tunnel.id, rx));
                    self.ok(format!("Testing {}…", r.tunnel.name));
                }
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('r') => match row {
                Some(r) => {
                    mgr.retry(&r.tunnel.id);
                    mgr.tick(self.tun_now());
                    self.ok(format!("Trying {} again.", r.tunnel.name));
                }
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('c') => match row {
                Some(r) => {
                    crate::ui::clipboard::copy(&r.tunnel.address());
                    self.ok(format!("Copied {}.", r.tunnel.address()));
                }
                None => self.fail("Choose a tunnel first (Up/Down)."),
            },
            Key::Char('k') => match row.and_then(|r| mgr.keys().key(&r.tunnel.key)) {
                Some(info) => {
                    crate::ui::clipboard::copy(&info.public);
                    self.ok("Copied the public key. Add it to the user's authorized keys on the SSH host.");
                }
                None => self.fail("Choose a tunnel whose key exists first (Up/Down)."),
            },
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------------ add and edit

    fn tun_open_form(&mut self, cx: &AppCx, edit: Option<Tunnel>) {
        let keys: Vec<String> = self.tun_mgr(cx).keys().keys().into_iter().map(|k| k.id).collect();
        let own = edit.as_ref().is_some_and(|t| t.own_key);
        let mut opts: Vec<String> = vec![if own { "Keep the key made for this tunnel".into() } else { "Make a new key for this tunnel".into() }];
        opts.extend(keys.iter().cloned());
        let opt_refs: Vec<&str> = opts.iter().map(String::as_str).collect();
        let t = edit.clone().unwrap_or_else(|| Tunnel { port: 22, ..Default::default() });
        let key_sel = if own { 0 } else { keys.iter().position(|k| *k == t.key).map_or(0, |i| i + 1) };
        let jumps = t.jump.iter().map(|h| format!("{}@{}:{}", h.user, h.host, h.port)).collect::<Vec<_>>().join(", ");
        let f = Form::new(if edit.is_some() { "Edit SSH tunnel" } else { "Add an SSH tunnel" })
            .text("Name", &t.name, "for example: Intranet wiki")
            .text("SSH host (the front-facing one)", &t.host, "bastion.example.org")
            .text("SSH port", &t.port.to_string(), "22")
            .text("User name on that host", &t.user, "your user on the host")
            .text("Jump hosts before it (optional)", &jumps, "user@gate.example.org:22, user@gate2.example.org")
            .choice("What to reach", &KINDS, if t.kind == TKind::DynamicSocks { 1 } else { 0 })
            .text("Page host (one page only)", &t.target_host, "wiki.internal.example.org, as the SSH host sees it")
            .text("Page port (one page only)", &if t.target_port == 0 { String::new() } else { t.target_port.to_string() }, "80, 443, 8080")
            .text("Port on this device", &if t.local_port == 0 { String::new() } else { t.local_port.to_string() }, "empty: a free one is chosen (1024 or above)")
            .choice("Key", &opt_refs, key_sel);
        self.open_form(FormKind::SshTun(TunForm::Edit(edit.map(|t| t.id))), f);
    }

    pub(super) fn tun_submit(&mut self, cx: &AppCx, kind: &TunForm, form: &mut Form) -> Result<String, String> {
        let mgr = self.tun_mgr(cx);
        match kind {
            TunForm::Trust(host, port) => {
                if form.choice_idx(1) != 1 {
                    return Err("Nothing was trusted. Choose Yes only when the fingerprint matches the one the host's owner gave you.".into());
                }
                mgr.trust(host, *port, form.value(0).trim())
            }
            TunForm::Edit(id) => {
                let old = id.as_ref().and_then(|i| mgr.get(i));
                if id.is_some() && old.is_none() {
                    return Err("That tunnel is gone.".into());
                }
                let user = form.value(3).trim().to_string();
                let socks = form.choice_idx(5) == 1;
                let num = |i: usize, what: &str, empty_ok: bool| -> Result<u16, String> {
                    let v = form.value(i);
                    let v = v.trim();
                    if v.is_empty() && empty_ok {
                        return Ok(0);
                    }
                    v.parse::<u16>().map_err(|_| format!("{what} is a number from 1 to 65535"))
                };
                let key_idx = form.choice_idx(9);
                let keys: Vec<String> = mgr.keys().keys().into_iter().map(|k| k.id).collect();
                let own_kept = old.as_ref().is_some_and(|t| t.own_key);
                let (key, new_key) = match (key_idx, &old) {
                    (0, Some(t)) if own_kept => (t.key.clone(), false),
                    (0, _) => (String::new(), true),
                    (n, _) => (keys.get(n - 1).cloned().ok_or("Choose a key.")?, false),
                };
                let t = Tunnel {
                    id: old.as_ref().map(|t| t.id.clone()).unwrap_or_default(),
                    name: form.value(0).trim().to_string(),
                    host: form.value(1).trim().to_string(),
                    port: num(2, "The SSH port", false)?,
                    jump: parse_jumps(&form.value(4), &user)?,
                    user,
                    key,
                    own_key: own_kept && !new_key,
                    kind: if socks { TKind::DynamicSocks } else { TKind::LocalForward },
                    local_port: num(8, "The port on this device", true)?,
                    target_host: if socks { String::new() } else { form.value(6).trim().to_string() },
                    target_port: if socks { 0 } else { num(7, "The page port", false)? },
                    enabled: old.as_ref().is_some_and(|t| t.enabled),
                };
                let saved = mgr.save(t, new_key)?;
                if new_key {
                    if let Some(info) = mgr.keys().key(&saved.key) {
                        crate::ui::clipboard::copy(&info.public);
                    }
                }
                if saved.enabled {
                    mgr.ensure_supervised();
                }
                Ok(format!(
                    "{} saved. Open {} in the Browser once it is connected.{}{}",
                    saved.name,
                    saved.address(),
                    if new_key { " A new key was made and its public half copied: add it to the user's authorized keys on the host." } else { "" },
                    if saved.enabled { "" } else { " It is disabled: press Test, then Enable." }
                ))
            }
        }
    }

    pub(super) fn tun_delete(&mut self, cx: &AppCx, id: &str) {
        match self.tun_mgr(cx).remove(id) {
            Ok(done) => {
                self.ok(format!("Deleted {}.", done.join(", ")));
                let len = self.tun_len(cx);
                self.sel[self.tab] = self.sel[self.tab].min(len.saturating_sub(1));
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    pub(super) fn render_tun(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mgr = self.tun_mgr(cx);
        let rows = mgr.rows();
        let now = self.tun_now();
        let mut y = a.y;
        intro(g, a, s, &mut y, &["Open a web page that sits behind an SSH host: each tunnel listens on this device only (127.0.0.1), so no other computer can use it. You bring your own host and key; nothing is built in.", "Add the tunnel, press Test (a new host's fingerprint is shown and confirmed once; a changed key is refused), then Enable. Enabled tunnels start with this device and reconnect by themselves."], "Opens a page behind an SSH host, on this device only (127.0.0.1). Add, Test, then Enable.", if rows.is_empty() { 40.0 * s } else { 112.0 * s + 60.0 * s + 16.0 * s });
        if rows.is_empty() {
            y += 6.0 * s;
            for w in g.wrap(14.0 * s, "No tunnels yet. Press a to add one.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, DIM, &w, Face::Sans);
                y += 20.0 * s;
            }
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        let detail_h = 112.0 * s;
        let rh = 60.0 * s;
        let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
        let heights = vec![rh; rows.len()];
        let first = self.win[self.tab].layout(sel, &heights, room);
        let fit = self.win[self.tab].fit();
        for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
            let t = &r.tunnel;
            let (state, col) = tun_state(r, now);
            let kind = if t.kind == TKind::DynamicSocks { "SOCKS" } else { "page" };
            let chips: Vec<(&str, [u8; 4])> = vec![(state.as_str(), col), (kind, MUTED)];
            let sub = match t.kind {
                TKind::LocalForward => format!("127.0.0.1:{} to {}:{} via {}", t.local_port, t.target_host, t.target_port, t.route_text()),
                TKind::DynamicSocks => format!("127.0.0.1:{} (SOCKS) via {}", t.local_port, t.route_text()),
            };
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &t.name, &sub, &chips);
            y += rh;
        }
        if rows.len() > fit {
            g.text(a.x + a.w - 120.0 * s, a.y + a.h - detail_h - 6.0 * s, 11.0 * s, DIM, &format!("{}-{} of {}", first + 1, (first + fit).min(rows.len()), rows.len()), Face::Sans);
        }
        // the chosen tunnel: the address to open, why it is not up, what it can reach
        let r = &rows[sel];
        let t = &r.tunnel;
        let top = a.y + a.h - detail_h;
        g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
        let mut lines: Vec<(String, [u8; 4])> = vec![(
            match t.kind {
                TKind::LocalForward => format!("Open {} in the Browser (c copies it).", t.address()),
                TKind::DynamicSocks => format!("SOCKS5 proxy for the Browser: 127.0.0.1:{} (c copies {}).", t.local_port, t.address()),
            },
            TEXT,
        )];
        lines.push((format!("Status: {}.", r.status.line(t.enabled, now)), MUTED));
        if let Some(e) = &r.status.error {
            lines.push((e.clone(), if r.status.needs_you { ERR } else { crate::fx::AMBER }));
        }
        lines.push((format!("Key {} (kept in your vault; its private half never leaves memory). Listens on 127.0.0.1 only.", t.key), MUTED));
        lines.push(("Choosing which sites use a tunnel (by domain or address) comes later; for now open the address above.".into(), DIM));
        let mut ty = top + 4.0 * s;
        'draw: for (l, col) in lines {
            for w in g.wrap(12.0 * s, &l, Face::Sans, a.w) {
                if ty + 16.0 * s > a.y + a.h {
                    break 'draw;
                }
                g.text(a.x, ty, 12.0 * s, col, &w, Face::Sans);
                ty += 16.0 * s;
            }
        }
    }
}

/// The chip of a row: the state in a word and its colour.
fn tun_state(r: &iyakki_agent::sshtunnel_mgr::Row, now: u64) -> (String, [u8; 4]) {
    let amber = crate::fx::AMBER;
    match (r.status.state, r.tunnel.enabled) {
        (State::Connected { since }, _) => (format!("● connected {}", tun::ago(now.saturating_sub(since))), OK),
        (State::Connecting, _) => ("● connecting".into(), amber),
        (State::Stopped, false) => ("○ disabled".into(), DIM),
        (State::Stopped, true) if r.status.needs_you => ("● needs you".into(), ERR),
        (State::Stopped, true) if r.status.error.is_some() => ("● retrying".into(), amber),
        (State::Stopped, true) => ("○ starting".into(), DIM),
    }
}

/// The explanation at the top of a list page: wrapped to the width in full when there is room for it and for what must stay below
/// (`keep`, in pixels); otherwise one short line, fitted to the width, and when not even that fits nothing (the page's own subtitle says the same), so the page never runs into its buttons or its list.
pub(super) fn intro(g: &mut Gfx, a: R, s: f32, y: &mut f32, paragraphs: &[&str], short: &str, keep: f32) {
    let wrapped: Vec<Vec<String>> = paragraphs.iter().map(|p| g.wrap(13.0 * s, p, Face::Sans, a.w)).collect();
    let total: f32 = wrapped.iter().map(|w| w.len() as f32 * 18.0 * s + 4.0 * s).sum();
    if *y + total + keep <= a.y + a.h {
        for para in &wrapped {
            for w in para {
                g.text(a.x, *y, 13.0 * s, MUTED, w, Face::Sans);
                *y += 18.0 * s;
            }
            *y += 4.0 * s;
        }
    } else if *y + 22.0 * s + keep <= a.y + a.h {
        let line = g.fit(13.0 * s, short, Face::Sans, a.w);
        g.text(a.x, *y, 13.0 * s, MUTED, &line, Face::Sans);
        *y += 22.0 * s;
    }
}

/// `user@host:port, user@host` into hops; the user of the tunnel stands in for a missing one, 22 for a missing port.
fn parse_jumps(text: &str, default_user: &str) -> Result<Vec<Hop>, String> {
    let mut out = vec![];
    for w in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let (user, rest) = w.split_once('@').map_or((default_user, w), |(u, r)| (u, r));
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("`{p}` is not a port in the jump host `{w}`"))?),
            None => (rest, 22),
        };
        out.push(Hop { host: host.to_string(), port, user: user.to_string() });
    }
    Ok(out)
}
