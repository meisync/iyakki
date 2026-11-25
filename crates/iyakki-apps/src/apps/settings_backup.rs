//! Settings -> Backup and restore: one file with a person's own data, written to a folder or stick they choose, and a restore from
//! such a file. The rules, the format and the checks are in `iyakki_shared::backup`; the page only asks, shows what is included and
//! what a file contains, and runs the work in the background. See `docs/BACKUP.md`.

use super::account_port::BackupPort;
use crate::safeclamp::SafeClamp;
use std::collections::BTreeSet;
use std::path::PathBuf;

use iyakki_shared::backup::{self as bk, AreaInfo, Contents, Created, Layout, RestoreReport};
use iyakki_shared::backup_schedule::Schedule;
use iyakki_shared::backup_share as bk_share;

use super::*;

/// What a background run brings back.
pub(super) enum BackupOut {
    Made(Result<Created, String>),
    /// The file read through and verified: where it is, the passphrase it needed (kept in memory until the file is closed), what is in it.
    Opened(Result<(PathBuf, Option<String>, Contents), String>),
    Restored(Result<RestoreReport, String>),
    /// A run that ends in one sentence (a share, the machine's own data, a chain link).
    Said(Result<String, String>),
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
    /// The scheduled backup: every N days, the hour, the folder, how many to keep, the passphrase file.
    Schedule,
    /// One backup written to a network share that is mounted only for the run (F-BACKUP-04).
    Share,
    /// The machine's own settings and keys, administrators only, always encrypted (F-BACKUP-05).
    System,
    /// One more link of a chain: only what changed since the files named (F-BACKUP-06).
    Link,
    /// Put the machine's own data back from a system backup, administrators only (F-BACKUP-05).
    SystemRestore,
    /// Put areas back from a chain: the full file and its links in order (F-BACKUP-06).
    ChainRestore,
}

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
    /// How far the backup that is being made has come, and the way to stop it (Settings shows a bar and the S key).
    pub(super) progress: Option<std::sync::Arc<bk::Progress>>,
    /// What mounts a network share for one backup (the machine's own mount; tests use a fake).
    mounter: std::sync::Arc<dyn bk_share::Mounter + Send + Sync>,
}

impl Default for BackupPanel {
    fn default() -> BackupPanel {
        BackupPanel { off: Default::default(), pick: Default::default(), preview: None, loaded: None, last_dir: String::new(), first: 0, layout: None, progress: None, mounter: std::sync::Arc::new(bk_share::RealMounter) }
    }
}

const WARN: [u8; 4] = [255, 196, 94, 255];

impl SettingsApp {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn with_backup_layout(mut self, l: Layout) -> SettingsApp {
        self.backup.layout = Some(l);
        self
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn with_backup_mounter(mut self, m: std::sync::Arc<dyn bk_share::Mounter + Send + Sync>) -> SettingsApp {
        self.backup.mounter = m;
        self
    }

    fn backup_layout(&self, p: &dyn BackupPort) -> Layout {
        self.backup.layout.clone().unwrap_or_else(|| Layout { home: p.backup_home(), data: p.data_dir(), user: p.backup_user() })
    }

    fn backup_on(&self) -> BTreeSet<String> {
        bk::all_areas().into_iter().filter(|a| !self.backup.off.contains(a)).collect()
    }

    fn backup_preview(&mut self, p: &dyn BackupPort) -> Result<Vec<AreaInfo>, String> {
        if self.backup.preview.is_none() {
            let l = self.backup_layout(p);
            self.backup.preview = Some(bk::preview(&l, &bk::all_areas()).map_err(|e| e.to_string()));
        }
        self.backup.preview.clone().unwrap_or(Ok(Vec::new()))
    }

    /// The rows of the page: the areas of the opened file, else the areas a backup would take now.
    pub(super) fn backup_rows(&mut self, p: &dyn BackupPort) -> Vec<AreaInfo> {
        match &self.backup.loaded {
            Some(l) => l.contents.areas.clone(),
            None => self.backup_preview(p).unwrap_or_default(),
        }
    }

    pub(super) fn backup_len(&self) -> usize {
        self.backup.loaded.as_ref().map_or(bk::AREAS.len(), |l| l.contents.areas.len())
    }

    pub(super) fn backup_buttons(&self) -> Vec<Btn> {
        if self.backup.loaded.is_some() {
            vec![("Restore chosen…", ACCENT, 'r'), ("Choose / skip", TEXT, ' '), ("Close backup", TEXT, 'c'), ("Open another…", TEXT, 'o')]
        } else {
            let mut v = vec![("Back up…", ACCENT, 'b'), ("Open backup…", TEXT, 'o'), ("Include / leave out", TEXT, ' '), ("Look again", TEXT, 'k'), ("Schedule…", TEXT, 'h'), ("Schedule off", TEXT, 'u'), ("To a network share…", TEXT, 'n'), ("Add a link…", TEXT, 'l'), ("Machine data…", TEXT, 'y'), ("Restore machine data…", TEXT, 'z'), ("Restore a chain…", TEXT, 't')];
            if self.backup.progress.is_some() && self.is_busy() {
                v.insert(0, ("Stop the backup", ERR, 's'));
            }
            v
        }
    }

    pub(super) fn handle_backup(&mut self, k: &Key, p: &dyn BackupPort) {
        let rows = self.backup_rows(p);
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
            Key::Char('s') => match (&self.backup.progress, self.is_busy()) {
                (Some(job), true) => {
                    job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.ok("Stopping the backup: the half file is removed.");
                }
                _ => self.fail("No backup is running."),
            },
            Key::Char('h') if self.backup.loaded.is_none() => {
                let s = Schedule::load(&p.data_dir(), &p.backup_user());
                let f = Form::new("Back up on a schedule")
                    .text("Every how many days (1 to 31)", &s.every_days.to_string(), "1 is every day")
                    .text("From which hour (0 to 23)", &s.hour.to_string(), "a computer that was off runs it when you next sign in")
                    .path("Folder to write to", if s.dest.is_empty() { &self.backup.last_dir } else { &s.dest }, "a folder that exists, for example on your USB stick", crate::filepicker::PathKind::Folder, &[])
                    .text("Keep how many files (1 to 50)", &s.keep.to_string(), "older ones of yours in that folder are removed after a good run")
                    .path("Passphrase file (empty: NOT encrypted)", &s.passphrase_file, "a file you made; its first line is the passphrase (8 or more characters). Nothing is typed here", crate::filepicker::PathKind::File, &[])
                    .text("Network share instead of the folder (optional)", &s.share, "smb://host/share/folder or nfs://host/export; mounted only while the backup runs")
                    .path("Share credentials file (SMB)", &s.credentials_file, "a file you made: user name on the first line, password on the second", crate::filepicker::PathKind::File, &[])
                    .text("Full file after how many files (0: always full)", &s.full_every.to_string(), "0, or 2 to 50: a full file, then small links that hold only what changed");
                self.open_form(FormKind::Backup(BackupForm::Schedule), f);
            }
            Key::Char('u') if self.backup.loaded.is_none() => {
                let mut s = Schedule::load(&p.data_dir(), &p.backup_user());
                if !s.enabled {
                    return self.ok("There is no scheduled backup.");
                }
                s.enabled = false;
                match s.save(&p.data_dir(), &p.backup_user()) {
                    Ok(()) => {
                        p.audit("backup.schedule", "off");
                        self.ok("The scheduled backup is off (its settings are kept).");
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('n') if self.backup.loaded.is_none() => {
                if self.backup_on().is_empty() {
                    return self.fail("Everything is switched off: choose at least one thing to include (Space).");
                }
                let f = Form::new("Back up to a network share")
                    .text("Share address", "", "smb://host/share/folder or nfs://host/export")
                    .path("Credentials file (SMB only)", "", "a file you made: user name on the first line, password on the second; nothing is typed here", crate::filepicker::PathKind::File, &[])
                    .secret("Passphrase (optional)", "empty: the file is NOT encrypted. 8 or more characters to encrypt it")
                    .secret("Repeat the passphrase", "only when you typed one above");
                self.open_form(FormKind::Backup(BackupForm::Share), f);
            }
            Key::Char('y') if self.backup.loaded.is_none() => {
                if !p.may_manage_settings() {
                    return self.fail("Only an administrator may back up the machine's own settings and keys.");
                }
                let f = Form::new("Back up the machine's own settings and keys")
                    .path("Folder or stick to write to", &self.backup.last_dir, "a folder you choose; nothing is chosen for you", crate::filepicker::PathKind::Folder, &[])
                    .secret("Passphrase (required, 8 or more characters)", "these files hold keys and tokens: they are never written without a passphrase")
                    .secret("Repeat the passphrase", "");
                self.open_form(FormKind::Backup(BackupForm::System), f);
            }
            Key::Char('z') if self.backup.loaded.is_none() => {
                if !p.may_manage_settings() {
                    return self.fail("Only an administrator may restore the machine's own settings and keys.");
                }
                let f = Form::new("Restore the machine's own settings and keys")
                    .path("System backup file", "", "a file ending in .iybak made with Machine data", crate::filepicker::PathKind::File, &[bk::EXTENSION])
                    .secret("Passphrase it was made with", "")
                    .secret("Repeat the passphrase", "")
                    .text("Type RESTORE to confirm (what is there now is kept as a copy)", "", "RESTORE");
                self.open_form(FormKind::Backup(BackupForm::SystemRestore), f);
            }
            Key::Char('t') if self.backup.loaded.is_none() => {
                let f = Form::new("Restore from a chain of backups")
                    .text("The backups, in order", "", "the full file first, then each link, separated by a semicolon")
                    .text("What to bring back", "", "area names separated by commas, as shown in the list")
                    .secret("Passphrase the chain was made with (empty: plain files)", "")
                    .text("Type RESTORE to confirm (what is there now is kept as a copy)", "", "RESTORE");
                self.open_form(FormKind::Backup(BackupForm::ChainRestore), f);
            }
            Key::Char('l') if self.backup.loaded.is_none() => {
                if self.backup_on().is_empty() {
                    return self.fail("Everything is switched off: choose at least one thing to include (Space).");
                }
                let f = Form::new("Add a link to a chain of backups")
                    .path("Folder to write the new link to", &self.backup.last_dir, "a folder you choose", crate::filepicker::PathKind::Folder, &[])
                    .text("The backups so far, in order", "", "the full file first, then each link, separated by a semicolon")
                    .secret("Passphrase the chain was made with (empty: plain files)", "");
                self.open_form(FormKind::Backup(BackupForm::Link), f);
            }
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

    pub(super) fn backup_submit(&mut self, p: &dyn BackupPort, kind: &BackupForm, form: &mut Form) -> Result<String, String> {
        let layout = self.backup_layout(p);
        let now = iyakki_schema::now_secs() as i64;
        let user = p.backup_user();
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
                p.audit("backup.create", &format!("{} area(s) to {}, {}", opts.areas.len(), dest, if pass.is_some() { "encrypted" } else { "not encrypted" }));
                let pr = std::sync::Arc::new(bk::Progress::default());
                self.backup.progress = Some(pr.clone());
                self.start("Backing up", move || JobOut::Backup(BackupOut::Made(bk::create_with_progress(&layout, std::path::Path::new(&dest), &opts, &pr).map_err(|e| e.to_string()))));
                Ok("Backing up…".into())
            }
            BackupForm::Schedule => {
                let num = |i: usize, what: &str, lo: u32, hi: u32| -> Result<u32, String> { form.value(i).trim().parse::<u32>().ok().filter(|n| (lo..=hi).contains(n)).ok_or_else(|| format!("{what}: a whole number from {lo} to {hi}.")) };
                let mut s = Schedule::load(&p.data_dir(), &user);
                s.every_days = num(0, "Days", 1, 31)?;
                s.hour = num(1, "Hour", 0, 23)? as u8;
                s.dest = form.value(2).trim().to_string();
                s.keep = num(3, "Keep", 1, 50)?;
                s.passphrase_file = form.value(4).trim().to_string();
                s.share = form.value(5).trim().to_string();
                s.credentials_file = form.value(6).trim().to_string();
                s.full_every = form.value(7).trim().parse::<u32>().map_err(|_| "Full file after how many files: 0, or a whole number from 2 to 50.".to_string())?;
                s.enabled = true;
                s.validate()?;
                if s.share.is_empty() && !std::path::Path::new(&s.dest).is_dir() {
                    return Err("The folder is not there yet: make it, or plug in the stick, then try again.".into());
                }
                s.save(&p.data_dir(), &user)?;
                p.audit("backup.schedule", &format!("on, every {} day(s){}, {}", s.every_days, if s.share.is_empty() { "" } else { ", to a share" }, if s.passphrase_file.is_empty() { "not encrypted" } else { "encrypted" }));
                Ok(s.describe(&iyakki_clock::Zone::system_with(None), now))
            }
            BackupForm::Share => {
                let spec = bk_share::ShareSpec::parse(form.value(0).trim())?;
                let creds = form.value(1).trim().to_string();
                let creds = if creds.is_empty() { None } else { Some(bk_share::parse_credentials(&std::fs::read_to_string(&creds).map_err(|e| format!("The credentials file cannot be read: {e}"))?)?) };
                let pass = form.raw(2);
                if pass != form.raw(3) {
                    return Err("The two passphrases differ.".into());
                }
                let pass = (!pass.is_empty()).then_some(pass);
                if pass.as_ref().is_some_and(|p| p.chars().count() < bk::MIN_PASSPHRASE) {
                    return Err(format!("The passphrase needs at least {} characters (or leave it empty for no encryption).", bk::MIN_PASSPHRASE));
                }
                let work = p.data_dir().join("backup-share-work");
                let opts = bk::CreateOptions { areas: self.backup_on(), passphrase: pass.clone(), now, release: env!("CARGO_PKG_VERSION").to_string() };
                p.audit("backup.share", &format!("{}, {}", spec.source(), if pass.is_some() { "encrypted" } else { "not encrypted" }));
                let mounter = self.backup.mounter.clone();
                self.start("Backing up to the share", move || {
                    let r = (|| {
                        use std::os::unix::fs::DirBuilderExt;
                        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&work).map_err(|x| format!("no private folder for the mount: {x}"))?;
                        bk_share::create_on_share(&layout, &opts, &spec, creds.as_ref().map(|(u, p)| (u.as_str(), p.as_str())), mounter.as_ref(), &work)
                    })();
                    JobOut::Backup(BackupOut::Said(r.map(|(name, bytes)| format!("Wrote {name} ({}) to {}. The share was let go again.{}", bk::human(bytes), spec.source(), if opts.passphrase.is_some() { "" } else { " It is not encrypted: anyone who can read the share can read the file." }))))
                });
                Ok("Backing up to the share…".into())
            }
            BackupForm::System => {
                let proof = bk::AdminProof::of(p.may_manage_settings()).ok_or("Only an administrator may back up the machine's own settings and keys.")?;
                let dest = form.value(0).trim().to_string();
                if dest.is_empty() {
                    return Err("Choose the folder or stick to write to.".into());
                }
                let pass = form.raw(1);
                if pass != form.raw(2) {
                    return Err("The two passphrases differ.".into());
                }
                if pass.chars().count() < bk::MIN_PASSPHRASE {
                    return Err(format!("The passphrase needs at least {} characters: the machine's own data is never written without one.", bk::MIN_PASSPHRASE));
                }
                self.backup.last_dir = dest.clone();
                let opts = bk::CreateOptions { areas: Default::default(), passphrase: Some(pass), now, release: env!("CARGO_PKG_VERSION").to_string() };
                p.audit("backup.system.create", &dest);
                self.start("Backing up the machine's data", move || {
                    JobOut::Backup(BackupOut::Said(bk::create_system(&layout, std::path::Path::new(&dest), &opts, &bk::Progress::default(), proof).map(|c| format!("Wrote {} ({} files, {}), encrypted.", c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.files, bk::human(c.bytes))).map_err(|e| e.to_string())))
                });
                Ok("Backing up the machine's data…".into())
            }
            BackupForm::Link => {
                let dest = form.value(0).trim().to_string();
                let chain: Vec<PathBuf> = form.value(1).split(';').map(str::trim).filter(|x| !x.is_empty()).map(PathBuf::from).collect();
                if dest.is_empty() || chain.is_empty() || chain.len() > 200 {
                    return Err("Name the folder and the backups so far (the full file first, then each link, separated by a semicolon).".into());
                }
                let pass = Some(form.raw(2)).filter(|p| !p.is_empty());
                self.backup.last_dir = dest.clone();
                let opts = bk::CreateOptions { areas: self.backup_on(), passphrase: pass.clone(), now, release: env!("CARGO_PKG_VERSION").to_string() };
                p.audit("backup.incremental", &format!("{} file(s) before", chain.len()));
                self.start("Adding a link", move || {
                    let r = bk::chain_state(&layout, &chain, pass.as_deref()).and_then(|base| bk::create_incremental(&layout, std::path::Path::new(&dest), &opts, &bk::Progress::default(), &base).map(|c| (base.links, c)));
                    JobOut::Backup(BackupOut::Said(r.map(|(n, c)| format!("Wrote {} ({} changed files, {}). Keep it after the {n} file(s) before it, in that order.", c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.files, bk::human(c.bytes))).map_err(|e| e.to_string())))
                });
                Ok("Adding a link…".into())
            }
            BackupForm::SystemRestore => {
                let proof = bk::AdminProof::of(p.may_manage_settings()).ok_or("Only an administrator may restore the machine's own settings and keys.")?;
                let file = PathBuf::from(form.value(0).trim());
                if file.as_os_str().is_empty() {
                    return Err("Choose the system backup file.".into());
                }
                let pass = form.raw(1);
                if pass != form.raw(2) {
                    return Err("The two passphrases differ.".into());
                }
                if pass.is_empty() {
                    return Err("The machine's own data is always encrypted: type the passphrase.".into());
                }
                if form.value(3).trim() != "RESTORE" {
                    return Err("Type RESTORE exactly to go on.".into());
                }
                p.audit("backup.system.restore", &file.display().to_string());
                self.start("Restoring the machine's data", move || {
                    JobOut::Backup(BackupOut::Said(bk::restore_system(&layout, &file, Some(&pass), now, false, proof).map(|r| format!("Restored {}. The old data was kept as: {}.", r.restored.iter().map(|x| x.label.clone()).collect::<Vec<_>>().join(", "), r.kept.iter().filter_map(|k| k.file_name().map(|n| n.to_string_lossy().into_owned())).collect::<Vec<_>>().join(", "))).map_err(|e| e.to_string())))
                });
                Ok("Restoring the machine's data…".into())
            }
            BackupForm::ChainRestore => {
                let chain: Vec<PathBuf> = form.value(0).split(';').map(str::trim).filter(|x| !x.is_empty()).map(PathBuf::from).collect();
                if chain.is_empty() || chain.len() > 200 {
                    return Err("Name the backups (the full file first, then each link, separated by a semicolon).".into());
                }
                let areas: BTreeSet<String> = form.value(1).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                if areas.is_empty() {
                    return Err("Name the areas to bring back, separated by commas.".into());
                }
                if let Some(bad) = areas.iter().find(|a| !bk::AREAS.iter().any(|k| k.0 == a.as_str())) {
                    return Err(format!("\u{201C}{bad}\u{201D} is not a backup area."));
                }
                let pass = Some(form.raw(2)).filter(|p| !p.is_empty());
                if form.value(3).trim() != "RESTORE" {
                    return Err("Type RESTORE exactly to go on.".into());
                }
                let scratch = p.data_dir().join("backup-chain-scratch");
                let req = bk::RestoreRequest { areas, allow_other_account: false, now };
                p.audit("backup.chain.restore", &format!("{} area(s) from {} file(s)", req.areas.len(), chain.len()));
                self.start("Restoring from the chain", move || {
                    let r = std::fs::create_dir_all(&scratch).map_err(|e| e.to_string()).and_then(|()| bk::restore_chain(&layout, &chain, pass.as_deref(), &scratch, &req).map_err(|e| e.to_string()));
                    let _ = std::fs::remove_dir(&scratch);
                    JobOut::Backup(BackupOut::Restored(r))
                });
                Ok("Restoring from the chain…".into())
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
                p.audit("backup.restore", &format!("{} area(s) from {}", req.areas.len(), path.display()));
                self.start("Restoring", move || JobOut::Backup(BackupOut::Restored(bk::restore(&layout, &path, pass.as_deref(), &req).map_err(|e| e.to_string()))));
                Ok("Restoring…".into())
            }
        }
    }

    pub(super) fn backup_job_done(&mut self, out: BackupOut) {
        self.backup.progress = None;
        match out {
            BackupOut::Made(Ok(c)) => {
                self.backup.preview = None;
                self.ok(format!("Backed up {} files ({}) to {}{}", c.files, bk::human(c.bytes), c.path.display(), if c.encrypted { ". Encrypted with your passphrase." } else { ". This file is NOT encrypted: keep it somewhere safe." }));
            }
            BackupOut::Opened(Ok((path, pass, contents))) => {
                let msg = format!("Verified {}: {} files, {}, made by {} on {}.", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), contents.total_files, bk::human(contents.total_bytes), contents.account, iyakki_shared::release::civil_date(contents.created.max(0) as u64));
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
            BackupOut::Said(Ok(m)) => self.ok(m),
            BackupOut::Said(Err(e)) | BackupOut::Made(Err(e)) | BackupOut::Opened(Err(e)) | BackupOut::Restored(Err(e)) => self.fail(e),
        }
    }

    pub(super) fn render_backup(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn BackupPort) {
        let rows = self.backup_rows(p);
        let mut y = a.y;
        if let (Some(job), true) = (&self.backup.progress, self.is_busy()) {
            let (done, total) = (job.done.load(std::sync::atomic::Ordering::Relaxed), job.total.load(std::sync::atomic::Ordering::Relaxed));
            let frac = if total == 0 { 0.0 } else { (done as f32 / total as f32).min(1.0) };
            g.rrect(a.x, y, a.w, 10.0 * s, 5.0 * s, rgb(30, 38, 68));
            g.rrect(a.x, y, (a.w * frac).max(if total == 0 { 0.0 } else { 6.0 * s }), 10.0 * s, 5.0 * s, ACCENT);
            let t = if total == 0 { "Counting the files… (S stops)".to_string() } else { format!("{}% (S stops, the half file is removed)", (frac * 100.0) as u32) };
            let t = g.fit(13.0 * s, &t, Face::Sans, a.w);
            g.text(a.x, y + 14.0 * s, 13.0 * s, TEXT, &t, Face::Sans);
            y += 36.0 * s;
        } else if self.backup.loaded.is_none() {
            let line = Schedule::load(&p.data_dir(), &p.backup_user()).describe(&iyakki_clock::Zone::system_with(None), iyakki_clock::now_unix());
            for w in g.wrap(13.0 * s, &line, Face::Sans, a.w).into_iter().take(2) {
                g.text(a.x, y, 13.0 * s, DIM, &w, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        let intro: Vec<(String, [u8; 4])> = match &self.backup.loaded {
            None => vec![
                ("Makes one file with your own data, to keep on a stick or another disk. It holds the things listed below, and only yours.".into(), MUTED),
                ("The file is NOT encrypted unless you type a passphrase when you back up. Your vault goes in as the encrypted file it already is. Header values, VPN keys and tokens are never written in plain text. Nothing is written anywhere you have not chosen.".into(), WARN),
            ],
            Some(l) => {
                let c = &l.contents;
                vec![
                    (format!("{}: {} files, {}, made {} by {}{}. Every checksum was verified; nothing has been changed yet.", l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.total_files, bk::human(c.total_bytes), iyakki_shared::release::civil_date(c.created.max(0) as u64), c.account, if c.encrypted { " (encrypted)" } else { " (not encrypted)" }), MUTED),
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
            if let Err(e) = self.backup_preview(p) {
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
