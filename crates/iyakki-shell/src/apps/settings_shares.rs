//! Settings -> Shared folders: share a folder of yours over HTTP on the local network (browse and download, optionally
//! upload), or serve it as a small website. Everything is off until a share is made here, read-only until uploads are
//! switched on, and stops at sign-out. The rules and the server are in `iyakki_agent::{sharedfolder, sharedsite}`; the model
//! is explained in `docs/SHARED-FOLDERS.md`.

use std::sync::Arc;

use iyakki_agent::sharedfolder::{self as sf, Access, Draft, Mode, ShareInfo, Shares, EXPIRIES};

use super::*;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct SharePanel {
    first: usize,
    fit: usize,
    /// Show the QR code of the chosen share's first address.
    qr: bool,
    /// A folder asked for by "Share this folder..." in the IDE; the form opens with it on the next frame.
    pub(super) pending: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ShareForm {
    New,
    Edit(String),
    Delete(String),
}

const MODES: [&str; 2] = ["Files: browse and download", "Website: serve index.html (no list)"];
const ACCESSES: [&str; 4] = ["Secret link (recommended)", "Password (sent in clear over HTTP)", "Anyone on the local network", "This machine only"];
const UPLOADS: [&str; 4] = ["Read only", "Visitors may upload new files", "Uploads and new folders", "Uploads, folders and replacing files"];

fn lan_options() -> Vec<String> {
    let mut v = vec!["all network interfaces".to_string()];
    v.extend(sf::lan_addresses());
    v
}

/// The form for a share. `d` has the values to start from.
pub(super) fn share_form(d: &Draft, edit: bool) -> Form {
    let lan = lan_options();
    let lan_refs: Vec<&str> = lan.iter().map(String::as_str).collect();
    let bind = lan.iter().position(|a| *a == d.bind).unwrap_or(0);
    let exp: Vec<&str> = EXPIRIES.iter().map(|(l, _)| *l).collect();
    let up = if !d.upload {
        0
    } else if d.overwrite {
        3
    } else if d.mkdir {
        2
    } else {
        1
    };
    Form::new(if edit { "Edit shared folder" } else { "Share a folder on the network" })
        .text("Name", &d.name, "shown to visitors; letters, digits, space - _ .")
        .path("Folder", &d.folder, "full path of a folder of yours, for example your home folder or a workspace", crate::filepicker::PathKind::Folder, &[])
        .choice("Show it as", &MODES, if d.mode == Mode::Website { 1 } else { 0 })
        .choice("Who can open it", &ACCESSES, match d.access {
            Access::Token => 0,
            Access::Password => 1,
            Access::Open => 2,
            Access::Local => 3,
        })
        .secret("Password (password mode only)", if edit { "8 or more characters; leave empty to keep the current one" } else { "8 or more characters" })
        .text("Type OPEN to confirm", if d.access == Access::Open { "OPEN" } else { "" }, "only when you chose 'Anyone on the local network': then everyone on this network can see these files")
        .choice("Visitors may", &UPLOADS, up)
        .text("Largest upload (MB)", &if d.max_upload_mb == 0 { "25".to_string() } else { d.max_upload_mb.to_string() }, "1 to 512; uploads are kept in memory while they arrive")
        .choice("Hidden files (.name)", &["stay hidden", "are shown"], usize::from(d.show_hidden))
        .choice("Stops after", &exp, EXPIRIES.iter().position(|(_, s)| *s == d.expiry_secs).unwrap_or(1))
        .choice("Listen on", &lan_refs, bind)
        .text("Port", &if d.port == 0 { String::new() } else { d.port.to_string() }, "empty: the next free one from 8081; it stays the same for this share")
        .choice("After I sign out", &["it stops", "it starts again when I sign in"], usize::from(d.keep_running))
}

/// Read the form back (the checks that need the file system happen in `Shares::create`).
pub(super) fn draft_of_form(f: &Form) -> Result<Draft, String> {
    let lan = lan_options();
    let access = match f.choice_idx(3) {
        0 => Access::Token,
        1 => Access::Password,
        2 => Access::Open,
        _ => Access::Local,
    };
    let up = f.choice_idx(6);
    let port = match f.value(11).trim() {
        "" => 0,
        t => t.parse::<u16>().map_err(|_| "The port is a number from 1024 to 65535.".to_string())?,
    };
    let mb = f.value(7).trim().parse::<u64>().map_err(|_| "The largest upload is a whole number of megabytes (1 to 512).".to_string())?;
    if !(1..=512).contains(&mb) {
        return Err("The largest upload is a whole number of megabytes (1 to 512).".into());
    }
    Ok(Draft {
        name: f.value(0).trim().to_string(),
        folder: f.value(1).trim().to_string(),
        mode: if f.choice_idx(2) == 1 { Mode::Website } else { Mode::Files },
        access,
        password: f.raw(4),
        confirm_open: f.value(5).trim() == "OPEN",
        upload: up >= 1,
        mkdir: up >= 2,
        overwrite: up >= 3,
        max_upload_mb: mb,
        show_hidden: f.choice_idx(8) == 1,
        expiry_secs: EXPIRIES.get(f.choice_idx(9)).map_or(86400, |(_, s)| *s),
        bind: if f.choice_idx(10) == 0 { "all".into() } else { lan.get(f.choice_idx(10)).cloned().unwrap_or_else(|| "all".into()) },
        port,
        keep_running: f.choice_idx(12) == 1,
    })
}

fn left_text(secs: i64) -> String {
    match secs {
        s if s >= 86400 => format!("{} d {} h left", s / 86400, s % 86400 / 3600),
        s if s >= 3600 => format!("{} h {} min left", s / 3600, s % 3600 / 60),
        s => format!("{} min left", s.max(60) / 60),
    }
}

/// One line about who may open a share, for the list and the docs of the page.
pub(super) fn who(i: &ShareInfo) -> String {
    let c = &i.cfg;
    format!("{} · {}{}", c.access.label(), if c.mode == Mode::Website { "website" } else if c.upload { "files, uploads on" } else { "files, read only" }, if c.show_hidden { " · hidden files shown" } else { "" })
}

impl SettingsApp {
    pub(super) fn shares(cx: &AppCx) -> Arc<Shares> {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let a2 = user.clone();
        let allow = sf::allow_fn(cx.session.policy.clone(), &user, cx.services.dir());
        sf::for_user(cx.services.dir(), &user, Arc::new(move |e, d| audit.record(&a2, e, d)), allow)
    }

    pub(super) fn shares_rows(cx: &AppCx) -> Vec<ShareInfo> {
        let m = Self::shares(cx);
        m.tick();
        m.list()
    }

    pub(super) fn shares_len(cx: &AppCx) -> usize {
        Self::shares_rows(cx).len()
    }

    pub(super) fn shares_buttons(&self) -> Vec<Btn> {
        vec![("New share…", ACCENT, 'n'), ("Start/Stop", TEXT, 's'), ("Copy link", TEXT, 'c'), ("QR", TEXT, 'q'), ("Edit", TEXT, 'e'), ("New secret", TEXT, 't'), ("Delete", ERR, 'd'), ("Stop all", ERR, 'x')]
    }

    /// Open the form for a new share with `folder` filled in ("Share this folder…" in the IDE and EDIT).
    pub(super) fn shares_new_for(&mut self, folder: &str) {
        let d = Draft { folder: folder.to_string(), name: std::path::Path::new(folder).file_name().map(|n| n.to_string_lossy().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')).collect::<String>()).unwrap_or_default(), mode: if std::path::Path::new(folder).join("index.html").is_file() { Mode::Website } else { Mode::Files }, access: Access::Token, expiry_secs: 86400, max_upload_mb: 25, bind: "all".into(), ..Default::default() };
        self.open_form(FormKind::Share(ShareForm::New), share_form(&d, false));
    }

    pub(super) fn handle_shares(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::shares_rows(cx);
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        let m = Self::shares(cx);
        match k {
            Key::Up => self.move_sel(-1, rows.len()),
            Key::Down => self.move_sel(1, rows.len()),
            Key::Char('n') => {
                let home = iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir());
                self.shares_new_for(&home.to_string_lossy());
            }
            Key::Char('s') | Key::Enter => match rows.get(sel) {
                Some(i) if i.running => {
                    m.stop(&i.cfg.id);
                    self.ok(format!("Stopped sharing {}.", i.cfg.name));
                }
                Some(i) => match m.start(&i.cfg.id) {
                    Ok(()) => self.ok(format!("{} is shared now.", i.cfg.name)),
                    Err(e) => self.fail(e),
                },
                None => self.fail("Make a share first (n)."),
            },
            Key::Char('c') => match rows.get(sel).and_then(|i| i.urls.first().map(|u| (i, u.clone()))) {
                Some((i, u)) => {
                    crate::ui::clipboard::copy(&u);
                    self.ok(format!("Copied the link of {}{}.", i.cfg.name, if i.running { "" } else { " (it is stopped: press s to start it)" }));
                }
                None => self.fail("Choose a share first (Up/Down)."),
            },
            Key::Char('q') => {
                self.shares.qr = !self.shares.qr;
            }
            Key::Char('e') => match rows.get(sel) {
                Some(i) => self.open_form(FormKind::Share(ShareForm::Edit(i.cfg.id.clone())), share_form(&sf::draft_of(&i.cfg), true)),
                None => self.fail("Choose a share first (Up/Down)."),
            },
            Key::Char('t') => match rows.get(sel) {
                Some(i) if i.cfg.access == Access::Token => match m.new_token(&i.cfg.id) {
                    Ok(()) => self.ok("The link has a new secret; the old address stopped working. Copy the new link (c)."),
                    Err(e) => self.fail(e),
                },
                Some(_) => self.fail("Only a share with a secret link has a secret to change."),
                None => self.fail("Choose a share first (Up/Down)."),
            },
            Key::Char('d') | Key::Delete => match rows.get(sel) {
                Some(i) => {
                    let f = Form::new(&format!("Delete the share {}", i.cfg.name)).text("Type DELETE to stop and remove it (your files are not touched)", "", "DELETE");
                    self.open_form(FormKind::Share(ShareForm::Delete(i.cfg.id.clone())), f);
                }
                None => self.fail("Choose a share first (Up/Down)."),
            },
            Key::Char('f') => self.fw_open_shares(cx),
            Key::Char('x') => {
                let n = sf::stop_everything();
                self.ok(if n == 0 { "Nothing was being shared.".to_string() } else { format!("Stopped {n} share(s). Nothing is shared now.") });
            }
            _ => {}
        }
    }

    pub(super) fn shares_submit(&mut self, cx: &AppCx, kind: &ShareForm, form: &mut Form) -> Result<String, String> {
        let m = Self::shares(cx);
        match kind {
            ShareForm::New => {
                let d = draft_of_form(form)?;
                let id = m.create(&d)?;
                let c = m.get(&id);
                Ok(format!("Made the share {}. It is stopped: choose it and press s to start sharing{}.", d.name, c.map(|c| format!(" (port {})", c.port)).unwrap_or_default()))
            }
            ShareForm::Edit(id) => {
                let d = draft_of_form(form)?;
                m.update(id, &d)?;
                Ok(format!("Saved {}.", d.name))
            }
            ShareForm::Delete(id) => {
                if form.value(0).trim() != "DELETE" {
                    return Err("Type DELETE exactly to remove the share.".into());
                }
                m.delete(id)?;
                Ok("The share is removed.".into())
            }
        }
    }

    pub(super) fn render_shares(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if let Some(f) = self.shares.pending.take() {
            self.shares_new_for(&f);
        }
        let rows = Self::shares_rows(cx);
        let mut y = a.y;
        let running = rows.iter().filter(|r| r.running).count();
        for (l, col) in [
            ("Share a folder of yours with other devices on this network: browse and download, or serve a small website. Nothing is shared until you make a share and start it; it is read-only unless you allow uploads; everything stops when you sign out.", MUTED),
            ("A secret link is the safest choice here: anyone who has the link can open it, nobody else. A password is sent in clear over plain HTTP, so someone on the same network could read it. 'Anyone on the local network' needs no link or password.", MUTED),
        ] {
            for w in g.wrap(13.0 * s, l, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, col, &w, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        if running > 0 {
            g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), &format!("{running} share(s) running now. 'Stop all sharing' (x) closes them at once."), Face::Bold);
            y += 22.0 * s;
        }
        if rows.is_empty() {
            g.text(a.x, y + 6.0 * s, 14.0 * s, DIM, "No shares yet. Press n to share a folder.", Face::Sans);
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        let detail_h = 150.0 * s;
        let rh = 60.0 * s;
        let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
        let fit = ((room / rh) as usize).max(1);
        self.shares.fit = fit;
        if sel < self.shares.first {
            self.shares.first = sel;
        } else if sel >= self.shares.first + fit {
            self.shares.first = sel + 1 - fit;
        }
        let first = self.shares.first.min(rows.len().saturating_sub(fit));
        self.shares.first = first;
        for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
            let mut chips: Vec<(&str, [u8; 4])> = vec![];
            chips.push(if r.running { ("running", OK) } else { ("stopped", DIM) });
            if r.cfg.access == Access::Open {
                chips.push(("open to all", ERR));
            }
            let exp = match (r.running, r.left) {
                (true, Some(l)) => left_text(l),
                _ => EXPIRIES.iter().find(|(_, s)| *s == r.cfg.expiry_secs).map_or(String::new(), |(l, _)| if r.cfg.expiry_secs == 0 { "no end".to_string() } else { format!("stops after {l}") }),
            };
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("{}  :{}", r.cfg.name, r.cfg.port), &format!("{} · {} · {} · {} visitor(s)", r.cfg.folder, who(r), exp, r.clients), &chips);
            y += rh;
        }
        if rows.len() > fit {
            g.text(a.x + a.w - 120.0 * s, a.y + a.h - detail_h - 6.0 * s, 11.0 * s, DIM, &format!("{}-{} of {}", first + 1, (first + fit).min(rows.len()), rows.len()), Face::Sans);
        }
        // the chosen share: every address it answers to, why it might not run, and the QR code
        let r = &rows[sel];
        let top = a.y + a.h - detail_h;
        g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
        let qr = if self.shares.qr { r.urls.first().and_then(|u| iyakki_agent::qr::encode(u.as_bytes())) } else { None };
        let qr_px = qr.as_ref().map_or(0.0, |q| (q.size as f32 + 4.0) * 3.0 * s);
        let tw = a.w - qr_px - 16.0 * s;
        g.text(a.x, top + 4.0 * s, 13.0 * s, TEXT, &format!("Addresses of {} (type one into a browser on another device)", r.cfg.name), Face::Bold);
        // the firewall blocks incoming connections unless allowed: say so when it would keep other devices out of this share
        let fw_note = if r.running && r.cfg.access != Access::Local { super::firewall_page::share_blocked_note(r.cfg.port) } else { None };
        let mut ty = top + 24.0 * s;
        for u in r.urls.iter().take(4) {
            for l in g.wrap(12.0 * s, u, Face::Mono, tw).iter().take(2) {
                g.text(a.x, ty, 12.0 * s, if r.running { OK } else { DIM }, l, Face::Mono);
                ty += 16.0 * s;
            }
        }
        if let Some(e) = &r.error {
            for l in g.wrap(12.0 * s, e, Face::Sans, tw).iter().take(2) {
                g.text(a.x, ty, 12.0 * s, ERR, l, Face::Sans);
                ty += 16.0 * s;
            }
        } else if let Some(n) = fw_note {
            for l in g.wrap(12.0 * s, &n, Face::Sans, tw).iter().take(3) {
                g.text(a.x, ty, 12.0 * s, rgb(255, 196, 94), l, Face::Sans);
                ty += 16.0 * s;
            }
        } else if r.cfg.access == Access::Password {
            g.text(a.x, ty, 12.0 * s, MUTED, "Any user name works; the password goes with every request in clear text.", Face::Sans);
        }
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
        } else {
            g.text(a.x, top + detail_h - 26.0 * s, 12.0 * s, MUTED, "c copies the link · q shows a QR code to scan with a phone", Face::Sans);
        }
    }
}
