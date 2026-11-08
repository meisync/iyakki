//! Settings -> System update: the Update centre for a person who is not a developer (docs/UPDATES.md).
//!
//! Shows the installed version and the one kept before it, finds update packs in a folder or on a USB stick, says in plain words
//! what an install changes and what needs a yes (another variant, an older version, no signature), installs with the previous
//! version kept (the engine is `iyakki_agent::devmode::centre`, which is the developer-mode update with its safety rules), rolls back
//! to the kept version, and tells what the first start after an update found. It writes only the device's own boot files and the
//! engine's folders on the data volume, and it never restarts the computer: it says when to.

use super::*;
use iyakki_agent::devmode::centre as uc;
use iyakki_agent::store::updatecheck as chk;

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
    /// The last update check as read from the data folder (re-read every two seconds while the page shows).
    pub(super) check: Option<(std::time::Instant, Option<chk::Report>)>,
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
            Key::Char('c') | Key::Char('m') | Key::Char('u') | Key::Char('t') => self.upd_check_key(k, cx),
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
        let mut v = vec![("Install", ACCENT, 'i'), ("Roll back", TEXT, 'r'), ("Choose folder", TEXT, 'f'), ("Look again", TEXT, 's'), ("Check for updates", TEXT, 'c'), ("How often", TEXT, 'm'), ("Update list address", TEXT, 'u'), ("Use a store", TEXT, 't')];
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
        for (text, col, face) in self.upd_check_lines(_cx) {
            line(g, &mut y, 13.0, col, face, &text);
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

// -------------------------------------------------------------------------------------------- the update check (F-UPD-04)

impl SettingsApp {
    fn upd_report(&mut self, cx: &AppCx) -> Option<chk::Report> {
        if let Some((at, r)) = &self.upd.check {
            if at.elapsed() < std::time::Duration::from_secs(2) {
                return r.clone();
            }
        }
        let r = chk::Report::load(Self::svc(cx).dir());
        self.upd.check = Some((std::time::Instant::now(), r.clone()));
        r
    }

    fn upd_check_settings(cx: &AppCx) -> chk::CheckSettings {
        Self::svc(cx).settings.lock().map(|s| s.get().update_check.clone()).unwrap_or_default()
    }

    fn upd_check_save(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut chk::CheckSettings)) -> bool {
        let r = Self::svc(cx).settings.lock().map(|mut s| s.update(|s| f(&mut s.update_check)));
        match r {
            Ok(Ok(_)) => {
                cx.audit.record(&cx.session.user, "settings.update_check", what);
                true
            }
            Ok(Err(e)) => {
                self.fail(e.to_string());
                false
            }
            Err(_) => {
                self.fail("The settings are busy; try again.");
                false
            }
        }
    }

    /// (name, announcement address) of every enabled store that was added with its key checked.
    fn upd_stores(cx: &AppCx) -> Vec<(String, String)> {
        let store = iyakki_agent::store::Store::for_services(Self::svc(cx));
        store.config().sources.into_iter().filter(|s| s.enabled && !s.pin.is_empty() && s.kind == iyakki_agent::store::catalog::SourceKind::Url).map(|s| (s.name.clone(), iyakki_agent::store::sysannounce::url_for_catalog(&s.location))).collect()
    }

    /// `c` check now, `m` how often, `u` the address of the system update list. Administrators only.
    pub(super) fn upd_check_key(&mut self, k: &Key, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        let cur = Self::upd_check_settings(cx);
        match k {
            Key::Char('m') => {
                let mode = cur.mode.next();
                if self.upd_check_save(cx, &format!("mode={mode:?}"), |s| s.mode = mode) {
                    self.ok(format!("Checking for updates: {}.", mode.label()));
                }
            }
            Key::Char('t') => {
                // the stores added with their key checked: each announces IYAKKI at <store>/v1/system.json, signed with that key
                let stores = Self::upd_stores(cx);
                if stores.is_empty() {
                    return self.fail("No store with a checked key is added yet. Add your private store in the STORE (Settings) first.");
                }
                let at = stores.iter().position(|(_, url)| *url == cur.system_url).map_or(0, |i| (i + 1) % stores.len());
                let (name, url) = stores[at].clone();
                if self.upd_check_save(cx, &format!("system_url={url} (store {name})"), |s| s.system_url = url.clone()) {
                    self.upd.check = None;
                    self.ok(format!("The update list is now {name}'s announcement{}. Press Check for updates to try it. Nothing is downloaded from it.", if stores.len() > 1 { " (press Use a store again for the next store)" } else { "" }));
                }
            }
            Key::Char('u') => {
                let f = Form::new("Address of the update list").with_note("Where the newest IYAKKI version is announced (a small JSON file with the version). Leave it empty to check apps only. Nothing is downloaded from it; it only says what is new.").text("Address", &cur.system_url, "https://updates.example.org/iyakki/index.json");
                self.open_form(FormKind::UpdateSource, f);
            }
            _ => {
                if cur.mode == chk::Mode::Off {
                    return self.fail("Checking for updates is off. Press m to turn it on.");
                }
                let v = self.upd.versions.clone().unwrap_or_else(|| self.upd_centre().versions());
                let (svc, policy, user) = (cx.services.clone(), cx.session.policy.clone(), cx.session.user.clone());
                cx.audit.record(&user, "system.update.check", "now");
                self.upd.check = None;
                self.start("Checking for updates", move || {
                    let store = iyakki_agent::store::Store::for_services(&svc);
                    let fetch = iyakki_agent::store::catalog::HttpFetch(svc.quick_upstream());
                    let rep = chk::run_and_keep(&store, &svc, &policy, &user, &fetch, &cur, &v.running, &v.variant);
                    JobOut::Msg(Ok(match (rep.count(), rep.errors.len()) {
                        (0, 0) => "Everything is up to date.".to_string(),
                        (0, e) => format!("Nothing new found, but {e} source(s) could not be reached."),
                        (n, 0) => format!("{n} update(s) available. Nothing was downloaded or installed."),
                        (n, e) => format!("{n} update(s) available; {e} source(s) could not be reached. Nothing was downloaded or installed."),
                    }))
                });
            }
        }
    }

    pub(super) fn upd_source_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Ok(String::new());
        }
        let url = chk::check_url(&form.value(0))?;
        if self.upd_check_save(cx, "system_url", |s| s.system_url = url.clone()) {
            Ok(if url.is_empty() { "No system update list: only apps are checked.".to_string() } else { "Address saved. Press Check for updates to try it.".to_string() })
        } else {
            Err("Could not save.".into())
        }
    }

    /// The lines under the versions: how often, when it last looked, what it found, and what it does not do.
    pub(super) fn upd_check_lines(&mut self, cx: &AppCx) -> Vec<(String, [u8; 4], Face)> {
        let cfg = Self::upd_check_settings(cx);
        let mut out = vec![(format!("Checking for updates: {}.{}", cfg.mode.label(), if cfg.system_url.is_empty() { " Apps in your store sources are checked; no system update list is set." } else { " Your apps and the system update list are checked." }), TEXT, Face::Bold)];
        if let Some((name, _)) = Self::upd_stores(cx).into_iter().find(|(_, url)| *url == cfg.system_url) {
            out.push((format!("The update list is the announcement of {name} (signed with the key you confirmed)."), MUTED, Face::Sans));
        }
        if cfg.mode == chk::Mode::Daily {
            out.push(("This computer cannot tell a metered connection yet, so a daily check also runs on one.".into(), MUTED, Face::Sans));
        }
        match self.upd_report(cx) {
            None => out.push(("Not checked yet. Press Check for updates.".into(), MUTED, Face::Sans)),
            Some(r) => {
                let ago = iyakki_agent::clock::now_unix().saturating_sub(r.checked_at);
                let when = match ago {
                    0..=89 => "just now".to_string(),
                    90..=5399 => format!("{} minutes ago", ago / 60),
                    5400..=129_599 => format!("{} hours ago", (ago + 1800) / 3600),
                    _ => format!("{} days ago", ago / 86_400),
                };
                out.push((format!("Last checked {when}. {}", match r.count() { 0 => "Everything is up to date.".to_string(), n => format!("{n} update(s) available. Nothing is downloaded or installed until you do it yourself.") }), if r.count() > 0 { crate::fx::AMBER } else { OK }, Face::Sans));
                for u in r.updates.iter().take(6) {
                    let what = match u.kind {
                        chk::Kind::System => format!("System: IYAKKI {} -> {}. Get the update pack and install it from a folder or stick below.{}", u.from, u.to, if u.note.is_empty() { String::new() } else { format!(" {}", u.note) }),
                        chk::Kind::App => format!("App: {} {} -> {} ({}).{}", u.title, u.from, u.to, u.source, if u.needs_review() { format!(" Asks for new permissions: {}. Read them in the STORE before you update.", u.new_permissions.join(", ")) } else { String::new() }),
                    };
                    out.push((what, if u.needs_review() { crate::fx::AMBER } else { TEXT }, Face::Sans));
                }
                for (src, why) in r.errors.iter().take(3) {
                    out.push((format!("{src}: {why}"), ERR, Face::Sans));
                }
            }
        }
        out
    }
}
