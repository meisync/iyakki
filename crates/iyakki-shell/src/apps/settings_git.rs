//! Settings → Git and SSH keys: ed25519 keys kept in the person's vault (made here or imported), the public half to copy or scan, which
//! key or token each git host uses, and the hosts whose keys you have already trusted. The rules and the work are in
//! `iyakki_agent::{sshkey, gitkeys, gitagent}`; see `docs/GIT-AND-SSH.md`.
//!
//! The private half of a key is never shown and never written to a file: it lives in the vault and git gets it through a small
//! key service in memory while a clone, fetch or pull runs. A workspace can use another key for a host (the Repositories panel).

use std::sync::Arc;

use iyakki_agent::gitconn::{self, Connection, GitHttp, Method, TestReport};
use iyakki_agent::gitkeys::{shared_session, GitKeys, HostRule, KeyInfo};

use crate::apps::git_wizard::{dot_color, GitWizard, WizEnv, WizOut};
use super::*;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct GitPanel {
    /// The row after which the list starts (so the selected row stays on screen).
    first: usize,
    /// Rows that fitted last time (for the scroll).
    fit: usize,
    /// "Connect Git or Gitea" (add or edit a connection) while it is open.
    pub(super) wiz: Option<Box<GitWizard>>,
    /// A connection test in flight: the connection's id and its answer.
    pub(super) testing: Option<(String, std::sync::mpsc::Receiver<TestReport>)>,
    /// Tests: what answers the host's API instead of the network.
    pub(super) http: Option<Arc<dyn GitHttp>>,
}

/// The forms of this page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GitSetting {
    NewKey,
    ImportKey,
    Passphrase(String),
    HostKey,
    Token,
    DeleteKey(String),
    /// Type the connection's name to remove it (the token goes from the vault).
    RemoveConn(String),
}

pub(super) enum GRow {
    /// A git connection (Connect Git or Gitea).
    Conn(Connection),
    Key(KeyInfo),
    Host(String, HostRule),
    /// (host, port, key type, fingerprint)
    Known(String, u16, String, String),
}

fn split_pattern(p: &str) -> (String, u16) {
    if let Some(rest) = p.strip_prefix('[') {
        if let Some((h, port)) = rest.split_once("]:") {
            return (h.to_string(), port.parse().unwrap_or(22));
        }
    }
    (p.to_string(), 22)
}

impl SettingsApp {
    pub(super) fn git_keys(cx: &AppCx) -> GitKeys {
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        GitKeys::for_services(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)), shared_session())
    }

    pub(super) fn git_rows(cx: &AppCx) -> Vec<GRow> {
        let g = Self::git_keys(cx);
        let mut rows: Vec<GRow> = g.conns().list().into_iter().map(GRow::Conn).collect();
        rows.extend(g.keys().into_iter().map(GRow::Key));
        rows.extend(g.table().hosts.into_iter().map(|(h, r)| GRow::Host(h, r)));
        let mut seen = Vec::new();
        for k in g.known_hosts().read() {
            let (h, p) = split_pattern(&k.pattern);
            if !seen.contains(&(h.clone(), p, k.kind.clone())) {
                seen.push((h.clone(), p, k.kind.clone()));
                rows.push(GRow::Known(h, p, k.kind.clone(), k.fingerprint()));
            }
        }
        rows
    }

    pub(super) fn git_len(cx: &AppCx) -> usize {
        Self::git_rows(cx).len()
    }

    pub(super) fn git_buttons(&self) -> Vec<Btn> {
        vec![("Add connection", ACCENT, 'a'), ("Test", TEXT, 'T'), ("Edit", TEXT, 'e'), ("Make default", TEXT, 'm'), ("Remove connection", ERR, 'R'), ("Generate key", TEXT, 'n'), ("Import key", TEXT, 'i'), ("Copy public key", TEXT, 'c'), ("Unlock", TEXT, 'u'), ("Delete key", ERR, 'd'), ("Host → key", ACCENT, 'h'), ("Token", TEXT, 't'), ("Remove rule", ERR, 'x'), ("Forget host", ERR, 'f')]
    }

    pub(super) fn handle_git(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::git_rows(cx);
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        let user = cx.session.user.clone();
        if self.handle_conn(k, rows.get(sel), cx) {
            return;
        }
        match k {
            Key::Up => self.move_sel(-1, rows.len()),
            Key::Down => self.move_sel(1, rows.len()),
            Key::Char('n') => {
                let f = Form::new("Generate an SSH key (ed25519)").text("Name", "", "lower-case: github, work, gitea").text("Comment", &format!("{user}@iyakki"), "shown at the end of the public key");
                self.open_form(FormKind::Git(GitSetting::NewKey), f);
            }
            Key::Char('i') => {
                let f = Form::new("Import an SSH key")
                    .text("Name", "", "lower-case: github, work, gitea")
                    .text("Private key (paste it, or give the path of the file)", "", "-----BEGIN OPENSSH PRIVATE KEY----- … or /path/to/id_ed25519")
                    .secret("Passphrase of the key (empty if it has none)", "")
                    .choice("A passphrase is", &["asked once per session", "kept in my vault"], 0);
                self.open_form(FormKind::Git(GitSetting::ImportKey), f);
            }
            Key::Char('c') => match rows.get(sel) {
                Some(GRow::Key(info)) => {
                    crate::ui::clipboard::copy(&info.public);
                    self.ok("Copied the public key. Paste it into GitHub (Settings → SSH and GPG keys), Gitea, GitLab or the server's authorized_keys.");
                }
                _ => self.fail("Choose a key first (Up/Down)."),
            },
            Key::Char('u') => match rows.get(sel) {
                Some(GRow::Key(info)) if info.passphrase => {
                    let f = Form::new(&format!("Passphrase of {}", info.id)).secret("Passphrase", "").choice("The passphrase is", &["asked once per session", "kept in my vault"], 0);
                    self.open_form(FormKind::Git(GitSetting::Passphrase(info.id.clone())), f);
                }
                Some(GRow::Key(_)) => self.ok("This key has no passphrase."),
                _ => self.fail("Choose a key first (Up/Down)."),
            },
            Key::Char('d') | Key::Delete => match rows.get(sel) {
                Some(GRow::Key(info)) => {
                    let f = Form::new(&format!("Delete the key {}", info.id)).text(&format!("Type {} to delete it from your vault", info.id), "", &info.id);
                    self.open_form(FormKind::Git(GitSetting::DeleteKey(info.id.clone())), f);
                }
                _ => self.fail("Choose a key first (Up/Down)."),
            },
            Key::Char('h') => {
                let g = Self::git_keys(cx);
                let ids: Vec<String> = g.keys().into_iter().map(|k| k.id).collect();
                if ids.is_empty() {
                    self.fail("Make or import a key first (n or i).");
                    return;
                }
                let mut opts: Vec<&str> = vec!["(no key)"];
                opts.extend(ids.iter().map(String::as_str));
                let host = match rows.get(sel) {
                    Some(GRow::Host(h, _)) | Some(GRow::Known(h, ..)) => h.clone(),
                    _ => String::new(),
                };
                let f = Form::new("Which key does a git host use?").text("Host", &host, "github.com, gitlab.com, git.example.org").choice("Key", &opts, 1.min(opts.len() - 1));
                self.open_form(FormKind::Git(GitSetting::HostKey), f);
            }
            Key::Char('t') => {
                let host = match rows.get(sel) {
                    Some(GRow::Host(h, _)) | Some(GRow::Known(h, ..)) => h.clone(),
                    _ => String::new(),
                };
                let f = Form::new("A token for a git host (https addresses)").text("Host", &host, "github.com, gitlab.com, git.example.org").text("User name", "", "empty: git (GitHub accepts any name with a token)").secret("Token", "a personal access token; it is kept in your vault");
                self.open_form(FormKind::Git(GitSetting::Token), f);
            }
            Key::Char('x') => match rows.get(sel) {
                Some(GRow::Host(h, r)) => {
                    let g = Self::git_keys(cx);
                    let mut done = vec![];
                    if !r.key.is_empty() {
                        if g.set_host_key(h, "").is_ok() {
                            done.push("key rule");
                        }
                    }
                    if r.token && g.clear_token(h).is_ok() {
                        done.push("token");
                    }
                    self.ok(format!("Removed the {} of {h}.", done.join(" and ")));
                }
                _ => self.fail("Choose a host rule first (Up/Down)."),
            },
            Key::Char('f') => match rows.get(sel) {
                Some(GRow::Known(h, p, ..)) => match Self::git_keys(cx).known_hosts().forget(h, *p) {
                    Ok(n) => {
                        cx.audit.record(&user, "git.host.forget", &format!("{h}:{p}"));
                        self.ok(format!("Forgot {n} key(s) of {h}. The next connection shows its fingerprint again and asks."));
                    }
                    Err(e) => self.fail(e),
                },
                _ => self.fail("Choose a known host first (Up/Down)."),
            },
            _ => {}
        }
    }

    pub(super) fn git_submit(&mut self, cx: &AppCx, kind: &GitSetting, form: &mut Form) -> Result<String, String> {
        let g = Self::git_keys(cx);
        match kind {
            GitSetting::NewKey => {
                let info = g.generate(form.value(0).trim(), form.value(1).trim())?;
                Ok(format!("Made the key {}. Copy its public half (c) and add it to GitHub, Gitea or GitLab, then choose where it is used (h).", info.id))
            }
            GitSetting::ImportKey => {
                let given = form.raw(1);
                let text = if given.trim_start().starts_with('/') {
                    let p = given.trim();
                    if !cx.session.policy.allows_read(&cx.session.user, p) {
                        return Err("That file is outside the folders your account may read.".into());
                    }
                    let bytes = std::fs::read(p).map_err(|e| format!("could not read {p}: {e}"))?;
                    if bytes.len() > 16 * 1024 {
                        return Err("That file is too large to be a key.".into());
                    }
                    String::from_utf8(bytes).map_err(|_| "That file is not text.".to_string())?
                } else {
                    given
                };
                let pass = form.raw(2);
                let info = g.import(form.value(0).trim(), &text, if pass.is_empty() { None } else { Some(&pass) }, form.choice_idx(3) == 1)?;
                Ok(format!("Imported the key {} ({}).", info.id, info.fingerprint))
            }
            GitSetting::Passphrase(id) => {
                g.unlock_key(id, &form.raw(0), form.choice_idx(1) == 1)?;
                Ok(format!("The key {id} is unlocked{}.", if form.choice_idx(1) == 1 { " and its passphrase is kept in your vault" } else { " for this session" }))
            }
            GitSetting::HostKey => {
                let ids: Vec<String> = g.keys().into_iter().map(|k| k.id).collect();
                let idx = form.choice_idx(1);
                let key = if idx == 0 { String::new() } else { ids.get(idx - 1).cloned().unwrap_or_default() };
                g.set_host_key(form.value(0).trim(), &key)?;
                Ok(if key.is_empty() { format!("{} uses no key now.", form.value(0).trim()) } else { format!("{} uses the key {key}.", form.value(0).trim()) })
            }
            GitSetting::Token => {
                g.set_token(form.value(0).trim(), form.value(1).trim(), &form.raw(2))?;
                Ok(format!("The token for {} is in your vault; https addresses of that host use it.", form.value(0).trim()))
            }
            GitSetting::RemoveConn(id) => {
                let c = g.conns().get(id).ok_or("That connection is gone.")?;
                if form.value(0).trim() != c.name {
                    return Err(format!("Type {} exactly to remove the connection.", c.name));
                }
                g.conns().remove(id)?;
                Ok(format!("Removed the connection {}. Its token is gone from your vault; repositories already cloned keep working with whatever host rules you set.", c.name))
            }
            GitSetting::DeleteKey(id) => {
                if form.value(0).trim() != id {
                    return Err(format!("Type {id} exactly to delete the key."));
                }
                g.delete_key(id)?;
                Ok(format!("Deleted the key {id} from your vault. Anything that used it has no key now."))
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ connections

    fn wiz_env(&self, cx: &AppCx) -> WizEnv {
        WizEnv::real(cx, Arc::new(Self::git_keys(cx)), self.git.http.clone())
    }

    pub(super) fn git_wizard_open(&self) -> bool {
        self.git.wiz.is_some()
    }

    /// Opens "Connect Git or Gitea" (add) or the same wizard on one connection (edit).
    pub(super) fn git_open_wizard(&mut self, cx: &AppCx, edit: Option<Connection>) {
        let env = self.wiz_env(cx);
        self.git.wiz = Some(Box::new(match edit {
            Some(c) => GitWizard::edit(env, c),
            None => GitWizard::new(env),
        }));
    }

    /// Keys of the wizard (it takes everything while open). True when the event was for it.
    pub(super) fn git_wizard_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        let Some(w) = self.git.wiz.as_mut() else { return false };
        let out = w.event(ev);
        match out {
            WizOut::None => {}
            WizOut::Cancel => {
                self.git.wiz = None;
                self.ok("Nothing was connected or stored.");
            }
            WizOut::OpenPlatform => {
                self.git.wiz = None;
                if let Some(i) = self.tabs.iter().position(|t| *t == Tab::DevCluster) {
                    self.tab = i;
                    self.dc_open_platform(cx, None);
                } else {
                    self.fail("Platform services are in Settings > Dev cluster, which this account does not have.");
                }
            }
            WizOut::QuickInstallGitea => {
                self.git.wiz = None;
                match crate::apps::git_wizard::start_quick_install_gitea(cx) {
                    Ok(m) => self.ok(m),
                    Err(e) => self.fail(e),
                }
            }
            WizOut::Done(c) => {
                self.git.wiz = None;
                let r = Self::git_rows(cx).iter().position(|r| matches!(r, GRow::Conn(x) if x.id == c.id));
                if let Some(i) = r {
                    self.sel[self.tab] = i;
                }
                self.ok(format!("Connected {} ({}{}). {}", c.name, c.host(), if c.account.is_empty() { String::new() } else { format!(", {}", c.account) }, if c.default { "It is your default for new workspaces." } else { "" }));
            }
        }
        true
    }

    pub(super) fn git_wizard_render(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        if let Some(w) = self.git.wiz.as_mut() {
            w.render(g, area, s, ACCENT, blink(tick));
        }
    }

    /// The heartbeat: collects the answer of a connection test.
    pub(super) fn git_tick(&mut self, cx: &AppCx) {
        let Some((id, rx)) = &self.git.testing else { return };
        let Ok(r) = rx.try_recv() else { return };
        let id = id.clone();
        self.git.testing = None;
        let g = Self::git_keys(cx);
        let conns = g.conns();
        let name = conns.get(&id).map(|c| c.name).unwrap_or_default();
        let _ = conns.record_test(&id, &r);
        if r.ok {
            self.ok(format!("{name}: {}.", r.summary()));
        } else if r.tls == gitconn::Tls::Untrusted {
            self.fail(format!("{name}: {} Press e (Edit) and go to the test step to look at it.", r.summary()));
        } else {
            self.fail(format!("{name}: {}", r.summary()));
        }
    }

    /// Starts the test of a connection in the background (the host's own API, with the stored token).
    pub(super) fn git_test_conn(&mut self, cx: &AppCx, c: &Connection) {
        if self.git.testing.is_some() {
            self.fail("A test is still running.");
            return;
        }
        if !c.cluster.is_empty() {
            // inside the cluster the policy of the host list does not apply
        } else if !cx.session.policy.allows_host(&c.host()) {
            let r = TestReport { error: Some(format!("Your account may not reach {} (the network policy of this account).", c.host())), ..TestReport::blank(c) };
            let _ = Self::git_keys(cx).conns().record_test(&c.id, &r);
            self.fail(format!("{}: {}", c.name, r.summary()));
            return;
        }
        let conns = Self::git_keys(cx).conns();
        let token = match conns.token(c) {
            Ok(t) => t.to_string(),
            Err(e) if c.kind.has_api() => {
                self.fail(gitconn::scrub(&e, ""));
                return;
            }
            Err(_) => String::new(),
        };
        let http: Arc<dyn GitHttp> = self.wiz_env(cx).http;
        let cert = conns.pinned_cert(c);
        let c2 = c.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = if token.is_empty() { TestReport { ok: true, notes: vec!["No token to ask the host's API with: the first fetch or push shows if the key works.".into()], ..TestReport::blank(&c2) } } else { gitconn::test_connection(&*http, &c2, &token, cert.as_ref()) };
            let _ = tx.send(r);
        });
        cx.audit.record(&cx.session.user, "git.connect.test.start", &format!("{} {}", c.name, c.host()));
        self.git.testing = Some((c.id.clone(), rx));
        self.ok(format!("Testing {}…", c.name));
    }

    /// Keys of the connection rows. True when the key was taken.
    fn handle_conn(&mut self, k: &Key, row: Option<&GRow>, cx: &AppCx) -> bool {
        match (k, row) {
            (Key::Char('a'), _) => {
                self.git_open_wizard(cx, None);
                true
            }
            (Key::Char('T'), Some(GRow::Conn(c))) => {
                let c = c.clone();
                self.git_test_conn(cx, &c);
                true
            }
            (Key::Char('e'), Some(GRow::Conn(c))) => {
                self.git_open_wizard(cx, Some(c.clone()));
                true
            }
            (Key::Char('m'), Some(GRow::Conn(c))) => {
                match Self::git_keys(cx).conns().set_default(&c.id) {
                    Ok(()) => self.ok(format!("{} is now the default for new workspaces.", c.name)),
                    Err(e) => self.fail(e),
                }
                true
            }
            (Key::Char('R') | Key::Delete, Some(GRow::Conn(c))) => {
                let f = Form::new(&format!("Remove the connection {}", c.name)).text(&format!("Type {} to remove it (its token is deleted from your vault)", c.name), "", &c.name);
                self.open_form(FormKind::Git(GitSetting::RemoveConn(c.id.clone())), f);
                true
            }
            (Key::Char('T' | 'e' | 'm' | 'R'), _) => {
                self.fail("Choose a connection first (Up/Down).");
                true
            }
            _ => false,
        }
    }

    pub(super) fn render_git(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::git_rows(cx);
        let keys = Self::git_keys(cx);
        let mut y = a.y;
        for l in [
            "Connections: a git host you use (GitHub, GitLab, a Gitea or Forgejo server, any other) with how you sign in, tested against the host's own API. The AI IDE offers the same list to connect a project. Press a to add one; tokens live in your vault and are never shown again.",
            "Keys for git over SSH are made here or imported, and kept encrypted in your vault. The private half is never shown and never written to a file: git gets it from a small key service in memory only while a clone, fetch or pull runs, and every use is logged (host and key name).",
            "Add the public half to the git site once. A new host's fingerprint is shown and confirmed before the first connection; a changed host key is refused.",
        ] {
            for w in g.wrap(13.0 * s, l, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        if rows.is_empty() {
            y += 6.0 * s;
            for w in g.wrap(14.0 * s, "No connections or keys yet. Press a to connect Git or Gitea, n to make an SSH key, or i to import one you already use.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, DIM, &w, Face::Sans);
                y += 20.0 * s;
            }
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        let detail_h = match rows.get(sel) {
            Some(GRow::Key(_)) => 150.0 * s,
            Some(GRow::Conn(_)) => 110.0 * s,
            _ => 0.0,
        };
        let rh = 60.0 * s;
        let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
        let fit = ((room / rh) as usize).max(1);
        self.git.fit = fit;
        if sel < self.git.first {
            self.git.first = sel;
        } else if sel >= self.git.first + fit {
            self.git.first = sel + 1 - fit;
        }
        let first = self.git.first.min(rows.len().saturating_sub(fit));
        self.git.first = first;
        for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
            match r {
                GRow::Conn(c) => {
                    let (dot, col) = match &c.last_test {
                        Some(t) if t.ok => ("● ok", dot_color(c)),
                        Some(_) => ("● failed", dot_color(c)),
                        None => ("○ not tested", dot_color(c)),
                    };
                    let mut chips: Vec<(&str, [u8; 4])> = vec![(dot, col), (c.placement().label(), if c.placement() == gitconn::Placement::Internet { MUTED } else { rgb(110, 224, 160) })];
                    if c.default {
                        chips.push(("default", ACCENT));
                    }
                    let test = c.last_test.as_ref().map(|t| t.text.clone()).unwrap_or_else(|| "not tested yet (T)".into());
                    row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("connection  {}", c.name), &format!("{} · {} · {} · {} · {}", c.kind.label(), c.url, if c.account.is_empty() { "account unknown".to_string() } else { c.account.clone() }, c.auth_label(), test), &chips);
                }
                GRow::Key(k) => {
                    let mut chips: Vec<(&str, [u8; 4])> = vec![];
                    if k.passphrase {
                        chips.push((if k.remembered { "passphrase in vault" } else { "passphrase asked" }, rgb(255, 196, 94)));
                    }
                    let used: Vec<String> = keys.table().hosts.iter().filter(|(_, h)| h.key == k.id).map(|(h, _)| h.clone()).collect();
                    row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("key  {}", k.id), &format!("{} · {}", k.fingerprint, if used.is_empty() { "not used by any host".to_string() } else { format!("used for {}", used.join(", ")) }), &chips);
                }
                GRow::Host(h, r) => {
                    let what = match (r.key.is_empty(), r.token) {
                        (false, true) => format!("ssh: key {} · https: token", r.key),
                        (false, false) => format!("ssh: key {}", r.key),
                        (true, true) => "https: token".to_string(),
                        (true, false) => "no key, no token".to_string(),
                    };
                    row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("host {h}"), &what, &[]);
                }
                GRow::Known(h, p, kind, fp) => {
                    row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("trusted  {}", if *p == 22 { h.clone() } else { format!("{h}:{p}") }), &format!("{kind} {fp}"), &[]);
                }
            }
            y += rh;
        }
        if rows.len() > fit {
            g.text(a.x + a.w - 120.0 * s, a.y + a.h - detail_h - 6.0 * s, 11.0 * s, DIM, &format!("{}-{} of {}", first + 1, (first + fit).min(rows.len()), rows.len()), Face::Sans);
        }
        if let Some(GRow::Conn(c)) = rows.get(sel) {
            let top = a.y + a.h - detail_h;
            g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
            let mut ty = top + 4.0 * s;
            let mut lines: Vec<String> = vec![format!("Your code and sign-in go to {} ({}).", c.host(), c.placement().label())];
            if c.url.starts_with("http://") {
                lines.push("This address is http: nothing is encrypted on the way.".into());
            }
            lines.push(match c.method {
                Method::Token => format!("Signs in with a token kept in your vault ({}); it is never shown again, only replaced (e).", if c.cluster.is_empty() { "this connection's own entry" } else { "the entry Platform services keeps for this Gitea" }),
                Method::Ssh => format!("Signs in with the SSH key {}. Add its public half at {}.", c.key, c.kind.key_url(&c.url)),
            });
            if !c.identity_name.is_empty() || !c.identity_email.is_empty() {
                lines.push(format!("Commits as {} <{}>.", c.identity_name, c.identity_email));
            }
            if !c.pinned.is_empty() {
                lines.push(format!("Trusts the certificate {} (this connection only).", c.pinned));
            }
            for l in lines {
                for w in g.wrap(12.0 * s, &l, Face::Sans, a.w) {
                    g.text(a.x, ty, 12.0 * s, MUTED, &w, Face::Sans);
                    ty += 16.0 * s;
                }
            }
        }
        // the public half of the chosen key: text to copy, a short form to read out, and a QR code for a phone
        if let Some(GRow::Key(k)) = rows.get(sel) {
            let top = a.y + a.h - detail_h;
            g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
            let qr = iyakki_agent::qr::encode(k.public.as_bytes());
            let qr_px = qr.as_ref().map_or(0.0, |q| (q.size as f32 + 4.0) * 3.0 * s);
            let tw = a.w - qr_px - 16.0 * s;
            g.text(a.x, top + 4.0 * s, 13.0 * s, TEXT, &format!("Public key of {} (add this to the git site)", k.id), Face::Bold);
            let mut ty = top + 24.0 * s;
            for l in g.wrap(12.0 * s, &k.public, Face::Mono, tw).iter().take(4) {
                g.text(a.x, ty, 12.0 * s, OK, l, Face::Mono);
                ty += 16.0 * s;
            }
            g.text(a.x, top + detail_h - 26.0 * s, 12.0 * s, MUTED, &format!("short form: {}   ·   press c to copy", short_text(k)), Face::Sans);
            if let Some(q) = &qr {
                let m = 3.0 * s;
                let (qx, qy) = (a.x + a.w - qr_px + 2.0 * m, top + 2.0 * s);
                g.rect(qx - 2.0 * m, qy - 2.0 * m, (q.size as f32 + 4.0) * m, (q.size as f32 + 4.0) * m, [255, 255, 255, 255]);
                for yy in 0..q.size {
                    for xx in 0..q.size {
                        if q.get(xx, yy) {
                            g.rect(qx + xx as f32 * m, qy + yy as f32 * m, m, m, [0, 0, 0, 255]);
                        }
                    }
                }
            }
        }
    }
}

/// `ed25519 AbCdEfGh…xYz123` from the fingerprint, short enough to read out or compare.
pub(super) fn short_text(k: &KeyInfo) -> String {
    let body = k.fingerprint.trim_start_matches("SHA256:");
    format!("ed25519 {}…{}", &body[..8.min(body.len())], &body[body.len().saturating_sub(6)..])
}
