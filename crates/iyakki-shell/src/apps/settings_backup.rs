//! Settings -> Backup and restore: one file with a person's own data, written to a folder or stick they choose, and a restore from
//! such a file. The rules, the format and the checks are in `iyakki_agent::backup`; the page only asks, shows what is included and
//! what a file contains, and runs the work in the background. See `docs/BACKUP.md`.

use crate::safeclamp::SafeClamp;
use std::collections::BTreeSet;
use std::path::PathBuf;

use iyakki_agent::backup::{self as bk, AreaInfo, Contents, Created, Layout, RestoreReport};

use super::*;

/// What a background run brings back.
pub(super) enum BackupOut {
    Made(Result<Created, String>),
    /// The file read through and verified: where it is, the passphrase it needed (kept in memory until the file is closed), what is in it.
    Opened(Result<(PathBuf, Option<String>, Contents), String>),
    Restored(Result<RestoreReport, String>),
}

pub(super) struct Loaded {
    path: PathBuf,
    pass: Option<String>,
    contents: Contents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BackupForm {
    Make,
    Open,
    Restore,
}

#[derive(Default)]
pub(super) struct BackupPanel {
    /// Areas to back up (all on until the person switches some off).
    off: BTreeSet<String>,
    /// Areas of an opened file to restore (none until the person switches them on: restoring is always a choice).
    pick: BTreeSet<String>,
    preview: Option<Result<Vec<AreaInfo>, String>>,
    pub(super) loaded: Option<Loaded>,
    last_dir: String,
    first: usize,
    /// Folders to use instead of the real ones (tests).
    layout: Option<Layout>,
}

const WARN: [u8; 4] = [255, 196, 94, 255];

impl SettingsApp {
    #[cfg(test)]
    pub(crate) fn with_backup_layout(mut self, l: Layout) -> SettingsApp {
        self.backup.layout = Some(l);
        self
    }

    fn backup_layout(&self, cx: &AppCx) -> Layout {
        self.backup.layout.clone().unwrap_or_else(|| {
            let data = cx.services.dir().to_path_buf();
            Layout { home: iyakki_agent::scope::home_root(&cx.session.user, &data), data, user: cx.session.user.clone() }
        })
    }

    fn backup_on(&self) -> BTreeSet<String> {
        bk::all_areas().into_iter().filter(|a| !self.backup.off.contains(a)).collect()
    }

    fn backup_preview(&mut self, cx: &AppCx) -> Result<Vec<AreaInfo>, String> {
        if self.backup.preview.is_none() {
            let l = self.backup_layout(cx);
            self.backup.preview = Some(bk::preview(&l, &bk::all_areas()).map_err(|e| e.to_string()));
        }
        self.backup.preview.clone().unwrap_or(Ok(Vec::new()))
    }

    /// The rows of the page: the areas of the opened file, else the areas a backup would take now.
    pub(super) fn backup_rows(&mut self, cx: &AppCx) -> Vec<AreaInfo> {
        match &self.backup.loaded {
            Some(l) => l.contents.areas.clone(),
            None => self.backup_preview(cx).unwrap_or_default(),
        }
    }

    pub(super) fn backup_len(&self) -> usize {
        self.backup.loaded.as_ref().map_or(bk::AREAS.len(), |l| l.contents.areas.len())
    }

    pub(super) fn backup_buttons(&self) -> Vec<Btn> {
        if self.backup.loaded.is_some() {
            vec![("Restore chosen…", ACCENT, 'r'), ("Choose / skip", TEXT, ' '), ("Close backup", TEXT, 'c'), ("Open another…", TEXT, 'o')]
        } else {
            vec![("Back up…", ACCENT, 'b'), ("Open backup…", TEXT, 'o'), ("Include / leave out", TEXT, ' '), ("Look again", TEXT, 'k')]
        }
    }

    pub(super) fn handle_backup(&mut self, k: &Key, cx: &AppCx) {
        let rows = self.backup_rows(cx);
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        match k {
            Key::Up => self.move_sel(-1, rows.len()),
            Key::Down => self.move_sel(1, rows.len()),
            Key::Char(' ') | Key::Enter => {
                let Some(a) = rows.get(sel) else { return };
                let set = if self.backup.loaded.is_some() { &mut self.backup.pick } else { &mut self.backup.off };
                if !set.remove(&a.id) {
                    set.insert(a.id.clone());
                }
            }
            Key::Char('k') => self.backup.preview = None,
            Key::Char('c') if self.backup.loaded.is_some() => {
                self.backup.loaded = None;
                self.backup.pick.clear();
                self.ok("Closed the backup file.");
            }
            Key::Char('b') if self.backup.loaded.is_none() => {
                if self.backup_on().is_empty() {
                    return self.fail("Everything is switched off: choose at least one thing to include (Space).");
                }
                let f = Form::new("Back up your data")
                    .path("Folder or stick to write to", &self.backup.last_dir, "a folder you choose, for example on your USB stick; nothing is chosen for you", crate::filepicker::PathKind::Folder, &[])
                    .secret("Passphrase (optional)", "empty: the file is NOT encrypted. 8 or more characters to encrypt it")
                    .secret("Repeat the passphrase", "only when you typed one above");
                self.open_form(FormKind::Backup(BackupForm::Make), f);
            }
            Key::Char('o') => {
                let f = Form::new("Open a backup file")
                    .path("Backup file", "", "a file ending in .iybak", crate::filepicker::PathKind::File, &[bk::EXTENSION])
                    .secret("Passphrase (only if it was encrypted)", "leave empty for a file that is not encrypted");
                self.open_form(FormKind::Backup(BackupForm::Open), f);
            }
            Key::Char('r') => {
                let Some(l) = &self.backup.loaded else { return self.fail("Open a backup file first (o).") };
                let names: Vec<&str> = l.contents.areas.iter().filter(|a| self.backup.pick.contains(&a.id)).map(|a| a.id.as_str()).collect();
                if names.is_empty() {
                    return self.fail("Choose what to restore first: Up/Down and Space on each thing you want back.");
                }
                let mut f = Form::new(&format!("Restore {}", names.join(", "))).text("Type RESTORE to confirm (what is there now is kept as a copy)", "", "RESTORE");
                if !l.contents.same_account {
                    f = f.text(&format!("This backup belongs to \u{201C}{}\u{201D}: type that name to restore it into your folder", l.contents.account), "", &l.contents.account);
                }
                self.open_form(FormKind::Backup(BackupForm::Restore), f);
            }
            _ => {}
        }
    }

    pub(super) fn backup_submit(&mut self, cx: &AppCx, kind: &BackupForm, form: &mut Form) -> Result<String, String> {
        let layout = self.backup_layout(cx);
        let now = iyakki_schema::now_secs() as i64;
        let user = cx.session.user.clone();
        match kind {
            BackupForm::Make => {
                let dest = form.value(0).trim().to_string();
                if dest.is_empty() {
                    return Err("Choose the folder or stick to write to (use the path box or its folder button).".into());
                }
                let pass = form.raw(1);
                if pass != form.raw(2) {
                    return Err("The two passphrases differ.".into());
                }
                let pass = (!pass.is_empty()).then_some(pass);
                if pass.as_ref().is_some_and(|p| p.chars().count() < bk::MIN_PASSPHRASE) {
                    return Err(format!("The passphrase needs at least {} characters (or leave it empty for no encryption).", bk::MIN_PASSPHRASE));
                }
                self.backup.last_dir = dest.clone();
                let opts = bk::CreateOptions { areas: self.backup_on(), passphrase: pass.clone(), now, release: env!("CARGO_PKG_VERSION").to_string() };
                cx.audit.record(&user, "backup.create", &format!("{} area(s) to {}, {}", opts.areas.len(), dest, if pass.is_some() { "encrypted" } else { "not encrypted" }));
                self.start("Backing up", move || JobOut::Backup(BackupOut::Made(bk::create(&layout, std::path::Path::new(&dest), &opts).map_err(|e| e.to_string()))));
                Ok("Backing up…".into())
            }
            BackupForm::Open => {
                let file = PathBuf::from(form.value(0).trim());
                if file.as_os_str().is_empty() {
                    return Err("Choose the backup file.".into());
                }
                let pass = Some(form.raw(1)).filter(|p| !p.is_empty());
                self.start("Reading the backup", move || {
                    JobOut::Backup(BackupOut::Opened(bk::inspect(&file, pass.as_deref(), &layout).map(|c| (file, pass, c)).map_err(|e| e.to_string())))
                });
                Ok("Reading the backup…".into())
            }
            BackupForm::Restore => {
                let Some(l) = &self.backup.loaded else { return Err("Open a backup file first.".into()) };
                if form.value(0).trim() != "RESTORE" {
                    return Err("Type RESTORE exactly to go on.".into());
                }
                if !l.contents.same_account && form.value(1).trim() != l.contents.account {
                    return Err(format!("Type the account name \u{201C}{}\u{201D} to restore a backup that is not yours.", l.contents.account));
                }
                let req = bk::RestoreRequest { areas: self.backup.pick.clone(), allow_other_account: !l.contents.same_account, now };
                let (path, pass) = (l.path.clone(), l.pass.clone());
                cx.audit.record(&user, "backup.restore", &format!("{} area(s) from {}", req.areas.len(), path.display()));
                self.start("Restoring", move || JobOut::Backup(BackupOut::Restored(bk::restore(&layout, &path, pass.as_deref(), &req).map_err(|e| e.to_string()))));
                Ok("Restoring…".into())
            }
        }
    }

    pub(super) fn backup_job_done(&mut self, out: BackupOut) {
        match out {
            BackupOut::Made(Ok(c)) => {
                self.backup.preview = None;
                self.ok(format!("Backed up {} files ({}) to {}{}", c.files, bk::human(c.bytes), c.path.display(), if c.encrypted { ". Encrypted with your passphrase." } else { ". This file is NOT encrypted: keep it somewhere safe." }));
            }
            BackupOut::Opened(Ok((path, pass, contents))) => {
                let msg = format!("Verified {}: {} files, {}, made by {} on {}.", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), contents.total_files, bk::human(contents.total_bytes), contents.account, iyakki_agent::release::civil_date(contents.created.max(0) as u64));
                self.backup.pick.clear();
                self.backup.loaded = Some(Loaded { path, pass, contents });
                self.sel[self.tab] = 0;
                self.ok(msg);
            }
            BackupOut::Restored(Ok(r)) => {
                self.backup.preview = None;
                let vault = r.restored.iter().any(|a| a.id == "vault");
                self.ok(format!("Restored {} thing(s). The data that was there is kept next to it as \u{201C}…before-restore-…\u{201D}.{}", r.restored.len(), if vault { " Sign out and in again to use the restored vault." } else { "" }));
            }
            BackupOut::Made(Err(e)) | BackupOut::Opened(Err(e)) | BackupOut::Restored(Err(e)) => self.fail(e),
        }
    }

    pub(super) fn render_backup(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = self.backup_rows(cx);
        let mut y = a.y;
        let intro: Vec<(String, [u8; 4])> = match &self.backup.loaded {
            None => vec![
                ("Makes one file with your own data, to keep on a stick or another disk. It holds the things listed below, and only yours.".into(), MUTED),
                ("The file is NOT encrypted unless you type a passphrase when you back up. Your vault goes in as the encrypted file it already is. Header values, VPN keys and tokens are never written in plain text. Nothing is written anywhere you have not chosen.".into(), WARN),
            ],
            Some(l) => {
                let c = &l.contents;
                vec![
                    (format!("{}: {} files, {}, made {} by {}{}. Every checksum was verified; nothing has been changed yet.", l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.total_files, bk::human(c.total_bytes), iyakki_agent::release::civil_date(c.created.max(0) as u64), c.account, if c.encrypted { " (encrypted)" } else { " (not encrypted)" }), MUTED),
                    (if c.same_account { "Choose what to restore with Space, then Restore. What is there now is kept next to it, never overwritten.".into() } else { format!("This backup belongs to \u{201C}{}\u{201D}, not to you. You can still restore it into your own folder, after you confirm.", c.account) }, if c.same_account { MUTED } else { WARN }),
                ]
            }
        };
        // room for the list below: the footer line and one row; the long text when it fits, else a short line per paragraph, else only the last (the warning or the next step)
        let keep = if rows.is_empty() { 30.0 * s } else { 40.0 * s + 52.0 * s };
        let short: Vec<String> = match &self.backup.loaded {
            None => vec!["Backs up your own data to one file.".into(), "Not encrypted unless you type a passphrase.".into()],
            Some(l) => vec!["Checksums verified; nothing changed yet.".into(), if l.contents.same_account { "Space chooses, then Restore.".into() } else { "Belongs to another account.".into() }],
        };
        let lines_of = |g: &mut Gfx, texts: &[&str]| -> Vec<Vec<String>> { texts.iter().map(|t| g.wrap(13.0 * s, t, Face::Sans, a.w)).collect() };
        let height = |w: &[Vec<String>]| -> f32 { w.iter().map(|p| p.len() as f32 * 18.0 * s + 4.0 * s).sum() };
        let full = lines_of(g, &intro.iter().map(|i| i.0.as_str()).collect::<Vec<_>>());
        let shorts = lines_of(g, &short.iter().map(String::as_str).collect::<Vec<_>>());
        let room = a.y + a.h - keep - y;
        let (chosen, from) = if height(&full) <= room {
            (full, 0)
        } else if height(&shorts) <= room {
            (shorts, 0)
        } else if height(&shorts[1..]) <= room {
            (shorts, 1)
        } else {
            (vec![], 0)
        };
        for (i, para) in chosen.iter().enumerate().skip(from) {
            for w in para {
                g.text(a.x, y, 13.0 * s, intro[i].1, w, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        if self.backup.loaded.is_none() {
            if let Err(e) = self.backup_preview(cx) {
                let shown = g.fit(13.0 * s, &e, Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, ERR, &shown, Face::Sans);
                return;
            }
        }
        if rows.is_empty() {
            g.text(a.x, y + 6.0 * s, 14.0 * s, DIM, "Nothing of yours to back up yet.", Face::Sans);
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        let rh = 52.0 * s;
        // the totals line goes first when there is not room for it and one row
        let foot = if a.y + a.h - y >= rh + 40.0 * s { 40.0 * s } else { 0.0 };
        // a window too short for even one row shows none (the buttons above still work on the selected one)
        if a.y + a.h - foot - y < rh {
            return;
        }
        let fit = (((a.y + a.h - foot - y) / rh) as usize).clamp_safe(1, rows.len());
        if sel < self.backup.first {
            self.backup.first = sel;
        } else if sel >= self.backup.first + fit {
            self.backup.first = sel + 1 - fit;
        }
        let first = self.backup.first.min(rows.len() - fit);
        self.backup.first = first;
        let loaded = self.backup.loaded.is_some();
        let (mut files, mut bytes) = (0usize, 0u64);
        for r in &rows {
            if if loaded { self.backup.pick.contains(&r.id) } else { !self.backup.off.contains(&r.id) } {
                files += r.files;
                bytes += r.bytes;
            }
        }
        for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
            let on = if loaded { self.backup.pick.contains(&r.id) } else { !self.backup.off.contains(&r.id) };
            let mut chips: Vec<(&str, [u8; 4])> = vec![if on { (if loaded { "restore" } else { "included" }, OK) } else { (if loaded { "skip" } else { "left out" }, DIM) }];
            if !r.skipped.is_empty() {
                chips.push(("some files left out", WARN));
            }
            let sub = format!("{} file(s) · {}{}", r.files, bk::human(r.bytes), r.skipped.first().map(|x| format!(" · {x}")).unwrap_or_default());
            // the title gives way to the chips on its right (drawn from the right edge), so it never runs under them
            // the row keeps 150 px for its chips; a detail line that has less than that is left out rather than drawn over them
            let room_sub = a.w - 186.0 * s;
            let sub = if room_sub < 40.0 * s { String::new() } else { g.fit(13.0 * s, &sub, Face::Mono, room_sub) };
            let chips_w: f32 = chips.iter().map(|c| g.text_width(12.0 * s, c.0, Face::Bold) + 24.0 * s).sum();
            let title = g.fit(17.0 * s, &format!("[{}] {}", if on { "x" } else { " " }, r.label), Face::Bold, (a.w - 18.0 * s - 14.0 * s - chips_w).max(40.0));
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &title, &sub, &chips);
            y += rh;
        }
        if foot == 0.0 {
            return;
        }
        let fy = a.y + a.h - foot + 10.0 * s;
        let what = if loaded { "to restore" } else { "in the backup" };
        let t = g.fit(13.0 * s, &format!("{files} file(s), {} {what}{}", bk::human(bytes), if rows.len() > fit { format!("   ({}-{} of {})", first + 1, first + fit, rows.len()) } else { String::new() }), Face::Bold, a.w);
        g.text(a.x, fy, 13.0 * s, TEXT, &t, Face::Bold);
    }
}
