//! Settings -> System update: the Update centre for a person who is not a developer (docs/UPDATES.md).
//!
//! Shows the installed version and the one kept before it, finds update packs in a folder or on a USB stick, says in plain words
//! what an install changes and what needs a yes (another variant, an older version, no signature), installs with the previous
//! version kept (the engine is `iyakki_agent::devmode::centre`, which is the developer-mode update with its safety rules), rolls back
//! to the kept version, and tells what the first start after an update found. It writes only the device's own boot files and the
//! engine's folders on the data volume, and it never restarts the computer: it says when to.

use super::*;
use iyakki_agent::devmode::centre as uc;

/// What the next `y` confirms.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum UpdAsk {
    #[default]
    None,
    Install,
    Rollback,
}

#[derive(Default)]
pub(super) struct UpdPanel {
    pub(super) centre: Option<Arc<uc::Centre>>,
    /// Folders to look in instead of the usual mount points (tests), and the folder the person typed.
    pub(super) roots: Option<Vec<std::path::PathBuf>>,
    pub(super) folder: String,
    pub(super) found: Vec<uc::Found>,
    pub(super) sel: usize,
    pub(super) preview: Option<Result<uc::Preview, String>>,
    pub(super) ask: UpdAsk,
    pub(super) versions: Option<uc::Versions>,
    pub(super) notice: Option<String>,
    pub(super) loaded: bool,
    /// An install or a rollback is running in the background job.
    pub(super) running: bool,
    pub(super) boot_id: Option<String>,
}

/// Where USB sticks and other drives appear when they are mounted.
fn mount_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let subs = |p: &str| -> Vec<std::path::PathBuf> { std::fs::read_dir(p).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect() };
    for base in ["/media", "/mnt"] {
        out.extend(subs(base));
    }
    for u in subs("/run/media") {
        out.extend(subs(&u.to_string_lossy()));
    }
    out.truncate(16);
    out
}

impl SettingsApp {
    /// Use another engine, places to look and boot id (tests).
    #[cfg(test)]
    pub(crate) fn with_updates(mut self, centre: uc::Centre, roots: Vec<std::path::PathBuf>, boot_id: &str) -> SettingsApp {
        self.storage.data_dir = centre.data().to_path_buf();
        self.upd.centre = Some(Arc::new(centre));
        self.upd.roots = Some(roots);
        self.upd.boot_id = Some(boot_id.into());
        self
    }

    fn upd_centre(&mut self) -> Arc<uc::Centre> {
        let data = self.storage.data_dir.clone();
        self.upd.centre.get_or_insert_with(|| Arc::new(uc::Centre::real(&data))).clone()
    }

    /// Read the versions, the note about the last start and the packs. Done once when the page opens and again on "Look again".
    fn upd_load(&mut self) {
        let c = self.upd_centre();
        let v = c.versions();
        let boot_id = self.upd.boot_id.clone().unwrap_or_else(uc::boot_id_now);
        if let Some(n) = uc::after_boot(c.data(), &v.running, &boot_id) {
            self.upd.notice = Some(n);
        }
        self.upd.versions = Some(c.versions());
        let mut roots = self.upd.roots.clone().unwrap_or_else(mount_dirs);
        if !self.upd.folder.is_empty() {
            roots.insert(0, std::path::PathBuf::from(&self.upd.folder));
        }
        let mut found = Vec::new();
        for r in roots {
            for f in uc::find_packs(&r, c.key()) {
                if !found.iter().any(|x: &uc::Found| x.dir == f.dir) {
                    found.push(f);
                }
            }
        }
        self.upd.found = found;
        self.upd.sel = self.upd.sel.min(self.upd.found.len().saturating_sub(1));
        self.upd.loaded = true;
        self.upd.ask = UpdAsk::None;
        self.upd_preview();
    }

    pub(super) fn upd_preview(&mut self) {
        let c = self.upd_centre();
        self.upd.preview = self.upd.found.get(self.upd.sel).map(|f| c.preview(f));
    }

    pub(super) fn upd_len(&self) -> usize {
        self.upd.found.len()
    }

    pub(super) fn upd_folder_submit(&mut self, form: &Form) -> Result<String, String> {
        let p = form.value(0).trim().to_string();
        if !p.is_empty() && !std::path::Path::new(&p).is_dir() {
            return Err(format!("{p} is not a folder"));
        }
        self.upd.folder = p;
        self.upd_load();
        Ok(format!("{} update(s) found.", self.upd.found.len()))
    }

    pub(super) fn handle_updates(&mut self, k: &Key, cx: &AppCx) {
        if !self.upd.loaded {
            self.upd_load();
        }
        // a finished install or rollback: read everything again
        if self.upd.running && self.job.is_none() {
            self.upd.running = false;
            self.upd_load();
        }
        match k {
            Key::Up | Key::Down => {
                let n = self.upd.found.len();
                if n > 0 {
                    self.upd.sel = if matches!(k, Key::Up) { (self.upd.sel + n - 1) % n } else { (self.upd.sel + 1) % n };
                    self.upd.ask = UpdAsk::None;
                    self.upd_preview();
                }
            }
            Key::Char('s') => {
                self.upd_load();
                self.ok(format!("Looked again: {} update(s) found.", self.upd.found.len()));
            }
            Key::Char('f') => {
                if !self.admin(cx) {
                    return;
                }
                let f = Form::new("Folder with an update").path("Folder (a USB stick shows up under /media or /run/media)", &self.upd.folder, "the folder that holds update.json, or a folder with such folders in it", crate::filepicker::PathKind::Folder, &[]);
                self.open_form(FormKind::UpdateFolder, f);
            }
            Key::Char('n') => {
                self.upd.ask = UpdAsk::None;
                self.ok("Nothing was changed.");
            }
            Key::Char('i') | Key::Char('r') | Key::Char('y') => {
                if !self.admin(cx) {
                    return;
                }
                self.upd_act(k, cx);
            }
            _ => {}
        }
    }

    fn upd_act(&mut self, k: &Key, cx: &AppCx) {
        if self.job.is_some() {
            return self.fail("An update is being written. Do not remove the drive or switch the computer off.");
        }
        let c = self.upd_centre();
        let user = cx.session.user.clone();
        match (k, self.upd.ask) {
            (Key::Char('i'), _) => {
                let Some(f) = self.upd.found.get(self.upd.sel) else { return self.fail("Choose an update first (f to pick a folder, s to look again).") };
                match &self.upd.preview {
                    Some(Ok(pv)) => {
                        self.upd.ask = UpdAsk::Install;
                        let extra = if pv.confirm.is_empty() { String::new() } else { format!(" It needs your yes: {}.", pv.confirm.join("; ")) };
                        self.ok(format!("Install IYAKKI {} over {}?{extra} Press y to do it, n to cancel.", f.manifest.version, pv.from));
                    }
                    Some(Err(e)) => self.fail(format!("This update cannot be installed: {e}")),
                    None => {}
                }
            }
            (Key::Char('r'), _) => {
                let v = c.versions();
                match v.previous {
                    Some(p) => {
                        self.upd.ask = UpdAsk::Rollback;
                        self.ok(format!("Roll back to IYAKKI {p}? The version on the boot partition ({}) is kept. Your files are not touched and nothing restarts by itself. Press y to do it, n to cancel.", v.on_disk));
                    }
                    None => self.fail("No previous version is kept on this device, so there is nothing to roll back to."),
                }
            }
            (Key::Char('y'), UpdAsk::Install) => {
                let Some(f) = self.upd.found.get(self.upd.sel).cloned() else { return };
                self.upd.ask = UpdAsk::None;
                self.upd.running = true;
                cx.audit.record(&user, "system.update.install", &format!("{} from {}", f.manifest.version, f.dir.display()));
                self.start("Installing the update", move || JobOut::Msg(c.install(&f, true, &user)));
            }
            (Key::Char('y'), UpdAsk::Rollback) => {
                self.upd.ask = UpdAsk::None;
                self.upd.running = true;
                cx.audit.record(&user, "system.update.rollback", "to the previous version");
                self.start("Rolling back", move || JobOut::Msg(c.rollback(&user)));
            }
            (Key::Char('y'), UpdAsk::None) => self.fail("There is nothing to confirm. Press i to install or r to roll back."),
            _ => {}
        }
    }

    pub(super) fn updates_buttons(&self) -> Vec<Btn> {
        let mut v = vec![("Install", ACCENT, 'i'), ("Roll back", TEXT, 'r'), ("Choose folder", TEXT, 'f'), ("Look again", TEXT, 's')];
        if self.upd.ask != UpdAsk::None {
            v.insert(0, ("Yes, do it", ERR, 'y'));
            v.insert(1, ("No", TEXT, 'n'));
        }
        v
    }

    pub(super) fn render_updates(&mut self, g: &mut Gfx, a: R, s: f32, _cx: &AppCx) {
        if !self.upd.loaded || (self.upd.running && self.job.is_none()) {
            self.upd.running = false;
            self.upd_load();
        }
        let mut y = a.y;
        let bottom = a.y + a.h;
        let line = |g: &mut Gfx, y: &mut f32, size: f32, col: [u8; 4], face: Face, text: &str| {
            for l in g.wrap(size * s, text, face, a.w) {
                if *y + size * s * 1.3 > bottom {
                    return;
                }
                g.text(a.x, *y, size * s, col, &l, face);
                *y += size * s * 1.45;
            }
        };
        if let Some(v) = self.upd.versions.clone() {
            line(g, &mut y, 15.0, TEXT, Face::Bold, &format!("Installed: IYAKKI {} ({})", v.on_disk, if v.variant.is_empty() { "variant unknown" } else { &v.variant }));
            if v.on_disk != v.running {
                line(g, &mut y, 13.0, crate::fx::AMBER, Face::Sans, &format!("Running now: {}. Restart to use {}; the computer does not restart by itself.", v.running, v.on_disk));
            }
            line(g, &mut y, 13.0, MUTED, Face::Sans, &match &v.previous {
                Some(p) => format!("Kept before it: IYAKKI {p} (Roll back puts it back; the boot menu also lists it as \"IYAKKI (previous version)\")."),
                None => "No previous version is kept yet. The first update keeps the one you have now.".to_string(),
            });
            if let Some(t) = &v.interrupted {
                line(g, &mut y, 13.0, crate::fx::AMBER, Face::Bold, t);
            }
        }
        if let Some(n) = self.upd.notice.clone() {
            line(g, &mut y, 13.0, OK, Face::Sans, &n);
        }
        if self.job.is_some() {
            line(g, &mut y, 14.0, crate::fx::AMBER, Face::Bold, "Writing the update. Do not remove the drive or switch the computer off.");
        }
        y += 6.0 * s;
        if self.upd.found.is_empty() {
            line(g, &mut y, 14.0, MUTED, Face::Sans, "No update found. Put the update folder or USB stick in, then press Look again, or Choose folder.");
        } else {
            line(g, &mut y, 13.0, DIM, Face::Sans, "Updates found (Up/Down to choose):");
            for (i, f) in self.upd.found.clone().iter().enumerate() {
                if y + 20.0 * s > bottom {
                    break;
                }
                let trust = match &f.trust {
                    uc::Trust::Signed => "signed",
                    uc::Trust::Unsigned => "not signed",
                    uc::Trust::Bad(_) => "WRONG signature",
                };
                let t = g.fit(14.0 * s, &format!("{} IYAKKI {} ({}, {trust}) {}", if i == self.upd.sel { ">" } else { " " }, f.manifest.version, f.manifest.variant, f.dir.display()), if i == self.upd.sel { Face::Bold } else { Face::Sans }, a.w);
                g.text(a.x, y, 14.0 * s, if i == self.upd.sel { ACCENT } else { TEXT }, &t, if i == self.upd.sel { Face::Bold } else { Face::Sans });
                y += 20.0 * s;
            }
            y += 4.0 * s;
            match self.upd.preview.clone() {
                Some(Ok(pv)) => {
                    for l in pv.lines() {
                        let col = if l.starts_with("Needs your yes") { crate::fx::AMBER } else { MUTED };
                        line(g, &mut y, 13.0, col, Face::Sans, &l);
                    }
                }
                Some(Err(e)) => line(g, &mut y, 13.0, ERR, Face::Sans, &format!("Cannot be installed: {e}")),
                None => {}
            }
        }
    }
}
