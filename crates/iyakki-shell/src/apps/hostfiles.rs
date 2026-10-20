//! HOST FILES (window mode only): bring files from the computer the window runs on into the account, share a folder of it
//! read-only (or read-write, on purpose), and save account files back out. [`HostDialog`] is one self-contained dialog
//! (`handle(event)`, `render(...)`) that the DATA viewer embeds and the shell shows over any screen for files dropped on
//! the window. The rules live in `iyakki_agent::hostfiles`; this file is the screens.
//!
//! The host is reached only through this dialog: its browser is rooted at the host `$HOME`, `..` stops there, hidden folders
//! and links that lead out are never shown, nothing is copied over a file without asking, and a shared folder is read-only
//! until writing is turned on for it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_agent::hostfiles as hf;

use super::widgets::{Form, FormEvent, PickEvent, Picker};
use super::AppCx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

/// What the dialog needs to know about the account.
#[derive(Clone)]
pub struct HostEnv {
    pub data_dir: PathBuf,
    pub user: String,
    /// Where imported files land, and how to say so.
    pub dest: PathBuf,
    pub dest_label: String,
    /// `(action, detail)` rows for the audit log.
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl HostEnv {
    /// The same, with another destination folder.
    pub fn with_dest(mut self, dest: PathBuf, label: &str) -> HostEnv {
        self.dest = dest;
        self.dest_label = label.to_string();
        self
    }

    /// Imports go to `<home>/Uploads`.
    pub fn for_cx(cx: &AppCx) -> HostEnv {
        let home = iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir());
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        HostEnv {
            data_dir: cx.services.dir().to_path_buf(),
            user: cx.session.user.clone(),
            dest: home.join("Uploads"),
            dest_label: "your Uploads folder".into(),
            audit: Arc::new(move |a, d| audit.record(&user, a, d)),
        }
    }
}

/// What the dialog tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Stay,
    Close,
    /// Files that were copied into the account.
    Imported(Vec<PathBuf>),
    /// A host folder was shared (its `host:<name>`).
    Linked(String),
    /// A host folder is no longer shared.
    Unlinked(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Import,
    Share,
    /// Choose the host folder to save this account file into.
    SaveTo(PathBuf),
}

enum Step {
    Info(Picker),
    Consent(Picker),
    Menu(Picker),
    Browse { dir: PathBuf, picker: Picker, mode: Mode },
    MountOptions { name: String, picker: Picker },
    FolderChoice { folder: PathBuf, stats: Option<hf::FolderStats>, rx: Receiver<hf::FolderStats>, picker: Picker },
    Conflicts { plan: hf::Plan, then_unlink: Option<String>, picker: Picker },
    SaveName { form: Form, dir: PathBuf, src: PathBuf },
    Running { progress: Arc<AtomicU64>, total: u64, rx: Receiver<Result<Vec<PathBuf>, String>>, label: String, then_unlink: Option<String> },
}

pub struct HostDialog {
    env: HostEnv,
    accent: [u8; 4],
    step: Step,
    selected: Vec<PathBuf>,
    /// Where each step came from (Cancel goes back to the menu).
    pending_out: Option<Out>,
}

const CONSENT: &str = "I understand, continue";

fn media_hint(st: &hf::FolderStats) -> String {
    let mb = st.bytes as f64 / (1 << 20) as f64;
    format!("{}{} files · {} · {}", if st.capped { "over " } else { "" }, st.files, if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }, if st.files > 0 && st.media * 2 > st.files { "mostly music, video or pictures" } else { "mixed files" })
}

impl HostDialog {
    fn base(env: HostEnv, accent: [u8; 4], step: Step) -> HostDialog {
        HostDialog { env, accent, step, selected: Vec::new(), pending_out: None }
    }

    fn consented(env: &HostEnv) -> bool {
        env.data_dir.join(format!("host-consent-{}", env.user)).exists()
    }

    fn info(env: HostEnv, accent: [u8; 4], title: &str, lines: &[&str]) -> HostDialog {
        let mut items: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        items.push("OK".into());
        Self::base(env, accent, Step::Info(Picker::new(title, items)))
    }

    /// The first screen: not available, the first-use explanation, or the menu.
    pub fn open(env: HostEnv, accent: [u8; 4]) -> HostDialog {
        if !hf::enabled() {
            return Self::info(env, accent, "Host files are not available here", &[hf::UNAVAILABLE]);
        }
        if !Self::consented(&env) {
            return Self::consent(env, accent, None);
        }
        let mut d = Self::base(env, accent, Step::Info(Picker::new("", vec![])));
        d.menu();
        d
    }

    fn consent(env: HostEnv, accent: [u8; 4], then: Option<Vec<PathBuf>>) -> HostDialog {
        let home = hf::host_home().map_or("your home folder".to_string(), |h| h.display().to_string());
        let items = vec![
            format!("Host files lets IYAKKI use files from the computer this window runs on ({home})."),
            "Import copies files you choose into your account. Nothing is changed on the computer.".to_string(),
            "Share a folder lets IYAKKI and its assistant read that one folder in place, read-only unless you turn writing on.".to_string(),
            "Nothing else of the computer is reachable: not hidden folders, not other people's folders, not anything you did not choose.".to_string(),
            "You can stop sharing a folder at any time and access ends at once.".to_string(),
            CONSENT.to_string(),
        ];
        let mut d = Self::base(env, accent, Step::Consent(Picker::new("Host files: what you are allowing", items)));
        d.selected = then.unwrap_or_default();
        d
    }

    /// Files and folders dropped on the window (or picked elsewhere): straight to "link or copy" for one folder, otherwise
    /// to the copy.
    pub fn import_paths(env: HostEnv, accent: [u8; 4], paths: Vec<PathBuf>) -> HostDialog {
        if !hf::enabled() {
            return Self::info(env, accent, "Host files are not available here", &[hf::UNAVAILABLE]);
        }
        let paths: Vec<PathBuf> = paths.into_iter().filter_map(|p| std::fs::canonicalize(p).ok()).collect();
        if paths.is_empty() {
            return Self::info(env, accent, "Nothing to import", &["The dropped items could not be read."]);
        }
        if !Self::consented(&env) {
            return Self::consent(env, accent, Some(paths));
        }
        let mut d = Self::base(env, accent, Step::Info(Picker::new("", vec![])));
        d.start_import(paths);
        d
    }

    /// Straight to "share a folder" (Settings → Host access).
    pub fn share(env: HostEnv, accent: [u8; 4]) -> HostDialog {
        let mut d = Self::open(env, accent);
        if matches!(d.step, Step::Menu(_)) {
            if let Some(h) = hf::host_home() {
                d.browse(h, Mode::Share);
            }
        }
        d
    }

    /// The options of one shared folder (mode, convert to a copy, stop sharing, browse it).
    pub fn mount_menu(env: HostEnv, accent: [u8; 4], name: &str) -> HostDialog {
        let mut d = Self::open(env, accent);
        if matches!(d.step, Step::Menu(_)) {
            d.mount_options(name);
        }
        d
    }

    /// **The save API for tiles and apps**: save these bytes on the host computer under a suggested name. Shows the host
    /// folder chooser (rooted at the host home or a writable shared folder), asks for the name, never replaces a file
    /// silently. Apps normally return the bytes from `App::take_host_save` and the shell shows this dialog.
    pub fn save_bytes(env: HostEnv, accent: [u8; 4], name: &str, bytes: &[u8]) -> HostDialog {
        let clean: String = Path::new(name).file_name().map_or("file".to_string(), |n| n.to_string_lossy().into_owned());
        let out = env.data_dir.join("host-out").join(&env.user);
        if let Err(e) = std::fs::create_dir_all(&out).and_then(|_| std::fs::write(out.join(&clean), bytes)) {
            return Self::info(env, accent, "Could not prepare the file", &[&e.to_string()]);
        }
        Self::export_file(env, accent, out.join(&clean))
    }

    /// Save an account file out to the host (a folder chooser, then a name).
    pub fn export_file(env: HostEnv, accent: [u8; 4], src: PathBuf) -> HostDialog {
        if !hf::enabled() {
            return Self::info(env, accent, "Host files are not available here", &[hf::UNAVAILABLE]);
        }
        if !Self::consented(&env) {
            return Self::consent(env, accent, None);
        }
        let mut d = Self::base(env, accent, Step::Info(Picker::new("", vec![])));
        let Some(home) = hf::host_home() else {
            return Self::info(d.env.clone(), accent, "No host home folder", &["The computer's home folder could not be found."]);
        };
        d.browse(home, Mode::SaveTo(src));
        d
    }

    // ------------------------------------------------------------------ what the screens show (tests read these)

    pub fn title(&self) -> String {
        match &self.step {
            Step::Info(p) | Step::Consent(p) | Step::Menu(p) | Step::Browse { picker: p, .. } | Step::MountOptions { picker: p, .. } | Step::FolderChoice { picker: p, .. } | Step::Conflicts { picker: p, .. } => p.title.clone(),
            Step::SaveName { form, .. } => form.title.clone(),
            Step::Running { label, .. } => label.clone(),
        }
    }

    pub fn items(&self) -> Vec<String> {
        match &self.step {
            Step::Info(p) | Step::Consent(p) | Step::Menu(p) | Step::Browse { picker: p, .. } | Step::MountOptions { picker: p, .. } | Step::FolderChoice { picker: p, .. } | Step::Conflicts { picker: p, .. } => p.visible().into_iter().cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// Where an import lands, in words.
    pub fn dest_label(&self) -> &str {
        &self.env.dest_label
    }

    pub fn is_running(&self) -> bool {
        matches!(self.step, Step::Running { .. })
    }

    /// The row of the first item that starts with `prefix` (for tests and keyboard shortcuts).
    pub fn index_of(&self, prefix: &str) -> Option<usize> {
        self.items().iter().position(|i| i.starts_with(prefix))
    }

    // ------------------------------------------------------------------ steps

    fn mounts(&self) -> Vec<hf::HostMount> {
        hf::active_mounts(&self.env.data_dir, &self.env.user)
    }

    fn audit(&self, a: &str, d: &str) {
        (self.env.audit)(a, d);
    }

    fn menu(&mut self) {
        let mounts = self.mounts();
        let mut items = vec!["Import files or folders from your computer…".to_string(), "Share a folder of your computer…".to_string()];
        for m in &mounts {
            items.push(format!("host:{} · {} · {}", m.name, if m.write { "read-write" } else { "read-only" }, m.path));
        }
        let title = if mounts.is_empty() { "Host files".to_string() } else { format!("Host files · access to {} folder{}", mounts.len(), if mounts.len() == 1 { "" } else { "s" }) };
        self.step = Step::Menu(Picker::new(&title, items));
    }

    fn notice(&mut self, title: &str, lines: &[String]) {
        let mut items: Vec<String> = lines.to_vec();
        items.push("OK".into());
        self.step = Step::Info(Picker::new(title, items));
    }

    fn browse(&mut self, dir: PathBuf, mode: Mode) {
        let Some(home) = hf::host_home() else {
            return self.notice("No host home folder", &["The computer's home folder could not be found.".to_string()]);
        };
        // only real folders inside the host home, never a hidden one
        let dir = match std::fs::canonicalize(&dir) {
            Ok(r) if r.is_dir() && hf::inside_home(&home, &r).is_ok() => r,
            _ => home.clone(),
        };
        let mounts = self.mounts();
        let writable_here = |d: &Path| mounts.iter().filter(|m| d.starts_with(&m.path)).max_by_key(|m| m.path.len()).is_none_or(|m| m.write);
        let mut items: Vec<String> = Vec::new();
        match &mode {
            Mode::Import => {
                if !self.selected.is_empty() {
                    items.push(format!("✔ Import the {} selected", self.selected.len()));
                }
                if dir != home {
                    items.push(format!("⬇ This whole folder ({})", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
            }
            Mode::Share => {
                if dir != home {
                    items.push(format!("✔ Share this folder ({})", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
            }
            Mode::SaveTo(_) => {
                if writable_here(&dir) {
                    items.push("✔ Save here…".to_string());
                } else {
                    items.push("(this shared folder is read-only: turn writing on for it first)".to_string());
                }
            }
        }
        items.push("★ Home".to_string());
        for n in ["Documents", "Downloads", "Music", "Pictures", "Videos"] {
            if home.join(n).is_dir() {
                items.push(format!("★ {n}"));
            }
        }
        for m in &mounts {
            if mode != Mode::Share {
                items.push(format!("★ host:{}", m.name));
            }
        }
        if dir != home && dir.parent().is_some_and(|p| hf::inside_home(&home, p).is_ok()) {
            items.push("../".into());
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten().take(5000) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(real) = std::fs::canonicalize(e.path()) else { continue };
                if hf::inside_home(&home, &real).is_err() {
                    continue; // a link that leads out
                }
                match std::fs::metadata(&real) {
                    Ok(m) if m.is_dir() => dirs.push(format!("{name}/")),
                    Ok(m) if m.is_file() && mode == Mode::Import => files.push((name, real)),
                    _ => {}
                }
            }
        }
        dirs.sort();
        files.sort();
        items.extend(dirs);
        for (name, real) in files {
            items.push(format!("{} {name}", if self.selected.contains(&real) { "☑" } else { "☐" }));
        }
        let shown = dir.strip_prefix(&home).map_or_else(|_| dir.display().to_string(), |r| if r.as_os_str().is_empty() { "~".to_string() } else { format!("~/{}", r.display()) });
        let what = match &mode {
            Mode::Import => "Import from your computer",
            Mode::Share => "Share a folder",
            Mode::SaveTo(_) => "Save to your computer",
        };
        self.step = Step::Browse { picker: Picker::new(&format!("{what}: {shown}"), items), dir, mode };
    }

    /// Start bringing in these host paths (already real paths).
    fn start_import(&mut self, paths: Vec<PathBuf>) {
        let single_dir = paths.len() == 1 && paths[0].is_dir();
        if single_dir {
            let folder = paths[0].clone();
            let (tx, rx) = channel();
            let f2 = folder.clone();
            std::thread::spawn(move || {
                let _ = tx.send(hf::folder_stats(&f2));
            });
            let mut d = Step::FolderChoice { folder, stats: None, rx, picker: Picker::new("", vec![]) };
            std::mem::swap(&mut self.step, &mut d);
            self.rebuild_choice();
        } else {
            self.plan_and_copy(paths, None);
        }
    }

    fn rebuild_choice(&mut self) {
        let Step::FolderChoice { folder, stats, picker, .. } = &mut self.step else { return };
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let default = hf::remembered_choice(&self.env.data_dir, &self.env.user, folder).unwrap_or_else(|| stats.as_ref().map_or(hf::Choice::Copy, hf::default_choice));
        let (info, link_tag, copy_tag) = match stats {
            Some(s) => (media_hint(s), if default == hf::Choice::Link { "  (recommended)" } else { "" }, if default == hf::Choice::Copy { "  (recommended)" } else { "" }),
            None => ("counting what is in it…".to_string(), "", ""),
        };
        let items = vec![
            info,
            format!("Link: read-only, no copy{link_tag}"),
            format!("Copy into IYAKKI{copy_tag}"),
            "Cancel".to_string(),
        ];
        *picker = Picker::new(&format!("Bring in “{name}”: link it or copy it?"), items);
    }

    fn plan_and_copy(&mut self, paths: Vec<PathBuf>, then_unlink: Option<String>) {
        let plan = hf::plan_import(&paths, &self.env.dest);
        if plan.items.is_empty() {
            let mut lines = vec!["Nothing was copied.".to_string()];
            lines.extend(plan.refused.iter().take(4).cloned());
            return self.notice("Nothing to copy", &lines);
        }
        if !plan.conflicts.is_empty() {
            let first = plan.conflicts[0].display().to_string();
            let more = if plan.conflicts.len() > 1 { format!(" (and {} more)", plan.conflicts.len() - 1) } else { String::new() };
            let items = vec![format!("{first}{more} already exists in {}.", self.env.dest_label), "Keep both (the new one gets a number)".to_string(), "Replace the existing ones".to_string(), "Cancel".to_string()];
            self.step = Step::Conflicts { plan, then_unlink, picker: Picker::new("Some files exist already", items) };
            return;
        }
        self.run(plan, hf::OnExists::Fail, then_unlink);
    }

    fn run(&mut self, plan: hf::Plan, on: hf::OnExists, then_unlink: Option<String>) {
        let progress = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (p2, dest, pl) = (progress.clone(), self.env.dest.clone(), plan.clone());
        std::thread::spawn(move || {
            let _ = tx.send(hf::run_import(&pl, &dest, on, &p2));
        });
        let label = format!("Copying {} file{} ({} MB)…", plan.items.len(), if plan.items.len() == 1 { "" } else { "s" }, plan.total.div_ceil(1 << 20));
        self.step = Step::Running { progress, total: plan.total, rx, label, then_unlink };
    }

    /// Link: share the folder read-only where it is.
    fn link(&mut self, folder: &Path) {
        let Some(home) = hf::host_home() else { return self.notice("No host home folder", &["The computer's home folder could not be found.".to_string()]) };
        let mut m = hf::HostMounts::load(&self.env.data_dir, &self.env.user);
        match m.add(&home, &folder.to_string_lossy(), None, false) {
            Ok(mount) => {
                self.audit("host.mount.add", &format!("host:{} {} read-only", mount.name, mount.path));
                hf::remember_choice(&self.env.data_dir, &self.env.user, folder, hf::Choice::Link);
                self.pending_out = Some(Out::Linked(mount.name.clone()));
                self.notice("Shared read-only", &[format!("host:{} now shows {}.", mount.name, mount.path), "IYAKKI and the assistant can read it there; nothing is copied and nothing on the computer can be changed.".to_string(), "Stop sharing any time from Host files.".to_string()]);
            }
            Err(e) => self.notice("Could not share it", &[e, "Choose Copy instead, or pick a folder inside your home folder.".to_string()]),
        }
    }

    // ------------------------------------------------------------------ events

    /// Finish whatever runs in the background (a copy, the folder count).
    pub fn poll(&mut self) -> Out {
        if let Step::FolderChoice { stats, rx, .. } = &mut self.step {
            if stats.is_none() {
                if let Ok(s) = rx.try_recv() {
                    *stats = Some(s);
                    self.rebuild_choice();
                }
            }
        }
        let mut result = None;
        if let Step::Running { rx, .. } = &self.step {
            if let Ok(r) = rx.try_recv() {
                result = Some(r);
            }
        }
        if let Some(r) = result {
            let Step::Running { then_unlink, total, .. } = std::mem::replace(&mut self.step, Step::Info(Picker::new("", vec![]))) else { return Out::Stay };
            match r {
                Ok(files) => {
                    self.audit("host.import", &format!("{} files, {} bytes into {}", files.len(), total, self.env.dest.display()));
                    let mut lines = vec![format!("Copied {} file{} into {}.", files.len(), if files.len() == 1 { "" } else { "s" }, self.env.dest_label)];
                    let mut out = Out::Imported(files.clone());
                    if let Some(name) = then_unlink {
                        let mut m = hf::HostMounts::load(&self.env.data_dir, &self.env.user);
                        if m.remove(&name).is_ok() {
                            self.audit("host.mount.remove", &format!("host:{name} (converted to a copy)"));
                            lines.push(format!("host:{name} is no longer shared; the copy does not depend on the computer's folder."));
                            out = Out::Imported(files);
                        }
                    }
                    self.notice("Done", &lines);
                    return out;
                }
                Err(e) => self.notice("The copy stopped", &[e]),
            }
        }
        self.pending_out.take().unwrap_or(Out::Stay)
    }

    pub fn handle(&mut self, ev: &Event) -> Out {
        if matches!(ev, Event::Tick) {
            return self.poll();
        }
        if let Step::Running { .. } = &self.step {
            // a copy cannot be stopped half way from here; Esc only waits
            return Out::Stay;
        }
        let step = std::mem::replace(&mut self.step, Step::Info(Picker::new("", vec![])));
        match step {
            Step::Info(mut p) => match p.handle_event(ev) {
                PickEvent::None => self.step = Step::Info(p),
                _ => {
                    let out = self.pending_out.take().unwrap_or(Out::Close);
                    return if matches!(out, Out::Close) { Out::Close } else { self.after_notice(out) };
                }
            },
            Step::Consent(mut p) => match p.handle_event(ev) {
                PickEvent::Chosen(l) if l == CONSENT => {
                    let _ = std::fs::write(self.env.data_dir.join(format!("host-consent-{}", self.env.user)), "agreed\n");
                    self.audit("host.consent", "the explanation was accepted");
                    let waiting = std::mem::take(&mut self.selected);
                    if waiting.is_empty() { self.menu() } else { self.start_import(waiting) }
                }
                PickEvent::Cancel => return Out::Close,
                _ => self.step = Step::Consent(p),
            },
            Step::Menu(mut p) => match p.handle_event(ev) {
                PickEvent::Cancel => return Out::Close,
                PickEvent::None => self.step = Step::Menu(p),
                PickEvent::Chosen(l) => {
                    self.step = Step::Menu(p);
                    let home = hf::host_home().unwrap_or_default();
                    if l.starts_with("Import") {
                        self.selected.clear();
                        self.browse(home, Mode::Import);
                    } else if l.starts_with("Share") {
                        self.browse(home, Mode::Share);
                    } else if let Some(rest) = l.strip_prefix("host:") {
                        let name = rest.split(' ').next().unwrap_or("").to_string();
                        self.mount_options(&name);
                    }
                }
            },
            Step::Browse { dir, mut picker, mode } => match picker.handle_event(ev) {
                PickEvent::Cancel => self.menu(),
                PickEvent::None => self.step = Step::Browse { dir, picker, mode },
                PickEvent::Chosen(l) => {
                    self.step = Step::Browse { dir: dir.clone(), picker, mode: mode.clone() };
                    self.browse_choice(&l, dir, mode);
                }
            },
            Step::MountOptions { name, mut picker } => match picker.handle_event(ev) {
                PickEvent::Cancel => self.menu(),
                PickEvent::None => self.step = Step::MountOptions { name, picker },
                PickEvent::Chosen(l) => self.mount_choice(&name, &l),
            },
            Step::FolderChoice { folder, stats, rx, mut picker } => match picker.handle_event(ev) {
                PickEvent::Cancel => self.menu(),
                PickEvent::None => self.step = Step::FolderChoice { folder, stats, rx, picker },
                PickEvent::Chosen(l) => {
                    if l.starts_with("Link") {
                        self.link(&folder);
                    } else if l.starts_with("Copy") {
                        hf::remember_choice(&self.env.data_dir, &self.env.user, &folder, hf::Choice::Copy);
                        self.plan_and_copy(vec![folder], None);
                    } else if l == "Cancel" {
                        self.menu();
                    } else {
                        self.step = Step::FolderChoice { folder, stats, rx, picker };
                    }
                }
            },
            Step::Conflicts { plan, then_unlink, mut picker } => match picker.handle_event(ev) {
                PickEvent::Cancel => self.menu(),
                PickEvent::None => self.step = Step::Conflicts { plan, then_unlink, picker },
                PickEvent::Chosen(l) => {
                    if l.starts_with("Keep both") {
                        self.run(plan, hf::OnExists::KeepBoth, then_unlink);
                    } else if l.starts_with("Replace") {
                        self.run(plan, hf::OnExists::Replace, then_unlink);
                    } else if l == "Cancel" {
                        self.menu();
                    } else {
                        self.step = Step::Conflicts { plan, then_unlink, picker };
                    }
                }
            },
            Step::SaveName { mut form, dir, src } => match form.handle_event(ev) {
                FormEvent::Cancel => self.browse(dir, Mode::SaveTo(src)),
                FormEvent::Submit => {
                    let name = form.value(0);
                    match self.save_to(&dir, &name, &src, hf::OnExists::Fail) {
                        Ok(()) => {}
                        Err(e) => {
                            form.error = Some(e);
                            self.step = Step::SaveName { form, dir, src };
                        }
                    }
                }
                _ => self.step = Step::SaveName { form, dir, src },
            },
            Step::Running { .. } => {}
        }
        Out::Stay
    }

    fn after_notice(&mut self, out: Out) -> Out {
        // a notice after a share or a copy: close, and tell the owner what happened
        self.step = Step::Info(Picker::new("", vec![]));
        match out {
            Out::Linked(_) | Out::Unlinked(_) => out,
            _ => Out::Close,
        }
    }

    fn browse_choice(&mut self, label: &str, dir: PathBuf, mode: Mode) {
        let Some(home) = hf::host_home() else { return };
        if label == "../" {
            return self.browse(dir.parent().map(Path::to_path_buf).unwrap_or(home), mode);
        }
        if label == "★ Home" {
            return self.browse(home, mode);
        }
        if let Some(n) = label.strip_prefix("★ host:") {
            let to = self.mounts().into_iter().find(|m| m.name == n).map(|m| PathBuf::from(m.path)).unwrap_or(home);
            return self.browse(to, mode);
        }
        if let Some(n) = label.strip_prefix("★ ") {
            return self.browse(home.join(n), mode);
        }
        if label.starts_with("✔ Import the") {
            let sel = std::mem::take(&mut self.selected);
            return self.start_import_files(sel);
        }
        if label.starts_with("⬇ This whole folder") {
            return self.start_import(vec![dir]);
        }
        if label.starts_with("✔ Share this folder") {
            return self.link(&dir);
        }
        if label.starts_with("✔ Save here") {
            let Mode::SaveTo(src) = mode else { return };
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let form = Form::new("Save to your computer").text("File name", &name, &format!("saved in {}", dir.display()));
            self.step = Step::SaveName { form, dir, src };
            return;
        }
        if let Some(d) = label.strip_suffix('/') {
            return self.browse(dir.join(d), mode);
        }
        if let Some(name) = label.strip_prefix("☐ ").or_else(|| label.strip_prefix("☑ ")) {
            if let Ok(real) = std::fs::canonicalize(dir.join(name)) {
                if hf::inside_home(&home, &real).is_ok() {
                    match self.selected.iter().position(|p| *p == real) {
                        Some(i) => {
                            self.selected.remove(i);
                        }
                        None => self.selected.push(real),
                    }
                }
            }
            return self.browse(dir, mode);
        }
    }

    fn start_import_files(&mut self, files: Vec<PathBuf>) {
        self.plan_and_copy(files, None);
    }

    fn save_to(&mut self, dir: &Path, name: &str, src: &Path, on: hf::OnExists) -> Result<(), String> {
        let home = hf::host_home().ok_or("The computer's home folder could not be found.")?;
        let name = name.trim();
        if name.is_empty() || name.contains('/') || name.starts_with('.') || name == ".." {
            return Err("Type a plain file name (no folders, not starting with a dot).".into());
        }
        let real_dir = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
        hf::inside_home(&home, &real_dir)?;
        if let Some(m) = self.mounts().iter().filter(|m| real_dir.starts_with(&m.path)).max_by_key(|m| m.path.len()) {
            if !m.write {
                return Err(format!("host:{} is shared read-only; turn writing on for it first.", m.name));
            }
        }
        let to = real_dir.join(name);
        let done = hf::export_file(src, &to, on);
        match done {
            Ok(p) => {
                self.audit("host.export", &format!("{} -> {}", src.display(), p.display()));
                self.notice("Saved", &[format!("Saved {}.", p.display())]);
                Ok(())
            }
            Err(e) if e.contains("already exists") => {
                // never replaced silently: ask
                self.audit("host.export.refused", &format!("{} exists", to.display()));
                Err(format!("{name} already exists there. Choose another name (or add a number); nothing was replaced."))
            }
            Err(e) => Err(e),
        }
    }

    fn mount_options(&mut self, name: &str) {
        let Some(m) = self.mounts().into_iter().find(|m| m.name == name) else { return self.menu() };
        let items = vec![
            format!("{}: {}", m.path, if m.write { "read-write (changes are made on the computer)" } else { "read-only" }),
            if m.write { "Make it read-only".to_string() } else { "Allow writing (be careful: changes are made on the computer)".to_string() },
            "Browse it and import files from it…".to_string(),
            "Convert to a copy (copy it into IYAKKI, then stop sharing)".to_string(),
            "Stop sharing (access ends now)".to_string(),
            "Back".to_string(),
        ];
        self.step = Step::MountOptions { name: name.to_string(), picker: Picker::new(&format!("host:{name}"), items) };
    }

    fn mount_choice(&mut self, name: &str, label: &str) {
        let mut m = hf::HostMounts::load(&self.env.data_dir, &self.env.user);
        if label.starts_with("Make it read-only") || label.starts_with("Allow writing") {
            let on = label.starts_with("Allow");
            if m.set_write(name, on).is_ok() {
                self.audit("host.mount.mode", &format!("host:{name} {}", if on { "read-write" } else { "read-only" }));
            }
            return self.mount_options(name);
        }
        if label.starts_with("Stop sharing") {
            if m.remove(name).is_ok() {
                self.audit("host.mount.remove", &format!("host:{name}"));
                self.pending_out = Some(Out::Unlinked(name.to_string()));
                self.notice("Stopped sharing", &[format!("host:{name} is gone: nothing can reach it any more, including tables that were open from it.")]);
            } else {
                self.menu();
            }
            return;
        }
        if label.starts_with("Browse it") {
            self.selected.clear();
            return match self.mounts().into_iter().find(|x| x.name == name) {
                Some(mount) => self.browse(PathBuf::from(mount.path), Mode::Import),
                None => self.menu(),
            };
        }
        if label.starts_with("Convert") {
            let Some(mount) = self.mounts().into_iter().find(|x| x.name == name) else { return self.menu() };
            return self.plan_and_copy(vec![PathBuf::from(mount.path)], Some(name.to_string()));
        }
        self.menu();
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let _ = self.poll();
        let accent = self.accent;
        match &self.step {
            Step::Info(p) | Step::Consent(p) | Step::Menu(p) | Step::Browse { picker: p, .. } | Step::MountOptions { picker: p, .. } | Step::FolderChoice { picker: p, .. } | Step::Conflicts { picker: p, .. } => p.render(g, area, s, accent),
            Step::SaveName { form, .. } => form.render(g, area, s, accent, super::blink(tick)),
            Step::Running { progress, total, label, .. } => {
                let p = crate::fx::modal(g, area, s, 560.0 * s, 190.0 * s, accent, false);
                crate::fx::modal_title(g, p, s, "Copying", accent);
                let t = g.fit(15.0 * s, label, Face::Sans, p.w - 52.0 * s);
                g.text(p.x + 26.0 * s, p.y + 70.0 * s, 15.0 * s, TEXT, &t, Face::Sans);
                let done = progress.load(Ordering::Relaxed);
                let frac = if *total == 0 { 0.0 } else { done as f32 / *total as f32 };
                crate::fx::progress_bar(g, R::new(p.x + 26.0 * s, p.y + 104.0 * s, p.w - 52.0 * s, 12.0 * s), frac, OK, s);
                g.text(p.x + 26.0 * s, p.y + 128.0 * s, 12.5 * s, MUTED, &format!("{} of {} MB", done >> 20, total.div_ceil(1 << 20)), Face::Sans);
                g.text(p.x + 26.0 * s, p.y + 150.0 * s, 12.0 * s, DIM, "You can keep working; this closes by itself when it is done.", Face::Sans);
                let _ = ERR;
            }
        }
    }
}

/// The overlay shown while files are dragged over the window: where a drop will go.
pub fn drop_overlay(g: &mut Gfx, w: f32, h: f32, s: f32, destination: &str, accent: [u8; 4]) {
    g.rect(0.0, 0.0, w, h, [3, 5, 14, 170]);
    let (bw, bh) = ((w - 80.0 * s).min(760.0 * s), 150.0 * s);
    let (x, y) = ((w - bw) / 2.0, (h - bh) / 2.0);
    g.rrect(x, y, bw, bh, 22.0 * s, [17, 22, 42, 235]);
    g.rrect_outline(x, y, bw, bh, 22.0 * s, 3.0 * s, accent);
    // a dashed look: short ticks along the border
    let mut dx = x + 24.0 * s;
    while dx < x + bw - 40.0 * s {
        g.rect(dx, y + 8.0 * s, 16.0 * s, 2.0 * s, accent);
        g.rect(dx, y + bh - 10.0 * s, 16.0 * s, 2.0 * s, accent);
        dx += 32.0 * s;
    }
    let t = g.fit(22.0 * s, &format!("Drop to import into {destination}"), Face::Bold, bw - 48.0 * s);
    let tw = g.text_width(22.0 * s, &t, Face::Bold);
    g.text(x + (bw - tw) / 2.0, y + 44.0 * s, 22.0 * s, TEXT, &t, Face::Bold);
    let sub = "Copied into your account; nothing on the computer changes. A folder can be linked read-only instead.";
    let sub = g.fit(13.5 * s, sub, Face::Sans, bw - 48.0 * s);
    let sw = g.text_width(13.5 * s, &sub, Face::Sans);
    g.text(x + (bw - sw) / 2.0, y + 92.0 * s, 13.5 * s, MUTED, &sub, Face::Sans);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Key;
    use std::sync::Mutex;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-hostdlg-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    struct World {
        host: PathBuf,
        data: PathBuf,
        env: HostEnv,
        log: Arc<Mutex<Vec<(String, String)>>>,
    }

    fn world(tag: &str) -> World {
        let base = tmp(tag);
        let (host, data, dest) = (base.join("host"), base.join("data"), base.join("acct/Uploads"));
        for d in ["Documents", "Music/Album", ".ssh", "Small"] {
            std::fs::create_dir_all(host.join(d)).unwrap();
        }
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(host.join("Documents/a.csv"), "id\n1\n").unwrap();
        std::fs::write(host.join("Documents/b.txt"), "b").unwrap();
        std::fs::write(host.join(".ssh/id_rsa"), "secret").unwrap();
        for i in 0..8 {
            std::fs::write(host.join(format!("Music/Album/t{i}.mp3")), vec![1u8; 100]).unwrap();
        }
        std::fs::write(host.join("Small/n.txt"), "n").unwrap();
        std::fs::write(host.join("Small/m.csv"), "a\n1\n").unwrap();
        std::os::unix::fs::symlink("/etc", host.join("etc")).unwrap();
        hf::set_enabled(true);
        hf::set_host_home(Some(host.clone()));
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let env = HostEnv { data_dir: data.clone(), user: "amy".into(), dest, dest_label: "your Uploads folder".into(), audit: Arc::new(move |a, d| l2.lock().unwrap().push((a.to_string(), d.to_string()))) };
        std::fs::write(data.join("host-consent-amy"), "agreed\n").unwrap();
        World { host: std::fs::canonicalize(host).unwrap(), data, env, log }
    }

    fn audits(w: &World) -> Vec<String> {
        w.log.lock().unwrap().iter().map(|l| l.0.clone()).collect()
    }

    fn pick(d: &mut HostDialog, prefix: &str) -> Out {
        let i = d.index_of(prefix).unwrap_or_else(|| panic!("no item {prefix:?} in {:?}", d.items()));
        d.handle(&Event::Row(i))
    }

    fn settle(d: &mut HostDialog) {
        for _ in 0..2000 {
            d.handle(&Event::Tick);
            if !d.is_running() && !matches!(d.step, Step::FolderChoice { stats: None, .. }) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("still running");
    }

    const ACC: [u8; 4] = [238, 226, 112, 255];

    #[test]
    fn it_says_why_when_host_files_are_off_and_explains_itself_on_first_use() {
        let _g = hf::test_guard();
        let w = world("gate");
        hf::set_enabled(false);
        let d = HostDialog::open(w.env.clone(), ACC);
        assert!(d.title().contains("not available") && d.items()[0].contains("desktop window"), "{:?}", d.items());
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Documents/a.csv")]);
        assert!(d.title().contains("not available"));
        assert_eq!(d.handle(&Event::Key(Key::Enter)), Out::Close);
        hf::set_enabled(true);
        // first use: what is being allowed, in words; nothing happens before the answer
        std::fs::remove_file(w.data.join("host-consent-amy")).unwrap();
        let mut d = HostDialog::open(w.env.clone(), ACC);
        assert!(d.title().contains("what you are allowing"));
        let all = d.items().join(" ");
        assert!(all.contains("Nothing else of the computer is reachable") && all.contains("read-only"), "{all}");
        assert_eq!(d.handle(&Event::Key(Key::Esc)), Out::Close);
        assert!(!w.data.join("host-consent-amy").exists(), "no answer, no grant");
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, CONSENT);
        assert!(d.title().starts_with("Host files"), "{}", d.title());
        assert!(w.data.join("host-consent-amy").exists());
        assert!(audits(&w).contains(&"host.consent".to_string()));
        hf::set_host_home(None);
    }

    #[test]
    fn the_host_browser_starts_at_the_host_home_and_never_leaves_it_or_shows_hidden_folders_or_escaping_links() {
        let _g = hf::test_guard();
        let w = world("browse");
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, "Import");
        let it = d.items();
        assert!(d.title().contains("~"), "{}", d.title());
        assert!(it.contains(&"Documents/".to_string()) && it.contains(&"Music/".to_string()), "{it:?}");
        assert!(!it.iter().any(|i| i.contains(".ssh") || i.contains("etc")), "hidden folders and links out are not shown: {it:?}");
        assert!(!it.contains(&"../".to_string()), "no way up from the host home");
        pick(&mut d, "Documents/");
        assert!(d.items().contains(&"../".to_string()));
        assert!(d.items().iter().any(|i| i.ends_with("a.csv")));
        pick(&mut d, "../");
        assert!(!d.items().contains(&"../".to_string()), "stops at the home");
        hf::set_host_home(None);
    }

    #[test]
    fn several_files_are_selected_and_copied_nothing_is_overwritten_and_it_is_audited() {
        let _g = hf::test_guard();
        let w = world("files");
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, "Import");
        pick(&mut d, "Documents/");
        pick(&mut d, "☐ a.csv");
        pick(&mut d, "☐ b.txt");
        assert!(d.index_of("☑ a.csv").is_some() && d.index_of("✔ Import the 2 selected").is_some(), "{:?}", d.items());
        pick(&mut d, "☑ b.txt");
        assert!(d.index_of("✔ Import the 1 selected").is_some(), "a second press takes it out again");
        pick(&mut d, "☐ b.txt");
        pick(&mut d, "✔ Import the 2");
        settle(&mut d);
        assert!(w.env.dest.join("a.csv").exists() && w.env.dest.join("b.txt").exists());
        assert_eq!(std::fs::read_to_string(w.host.join("Documents/a.csv")).unwrap(), "id\n1\n", "the host file is untouched");
        assert!(d.title() == "Done" && d.items()[0].contains("Copied 2 files into your Uploads folder"), "{:?}", d.items());
        assert!(audits(&w).contains(&"host.import".to_string()));
        // again: they exist, so it asks
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Documents/a.csv")]);
        assert!(d.title().contains("exist already"), "{}", d.title());
        assert!(d.items()[0].contains("a.csv already exists"));
        std::fs::write(w.host.join("Documents/a.csv"), "id\n2\n").unwrap();
        pick(&mut d, "Keep both");
        settle(&mut d);
        assert_eq!(std::fs::read_to_string(w.env.dest.join("a.csv")).unwrap(), "id\n1\n", "the first copy stays");
        assert_eq!(std::fs::read_to_string(w.env.dest.join("a (2).csv")).unwrap(), "id\n2\n");
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Documents/a.csv")]);
        pick(&mut d, "Cancel");
        assert!(d.title().starts_with("Host files"), "cancelling copies nothing");
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Documents/a.csv")]);
        pick(&mut d, "Replace");
        settle(&mut d);
        assert_eq!(std::fs::read_to_string(w.env.dest.join("a.csv")).unwrap(), "id\n2\n", "replaced only when asked");
        hf::set_host_home(None);
    }

    #[test]
    fn a_media_folder_defaults_to_a_read_only_link_that_leaves_the_host_untouched() {
        let _g = hf::test_guard();
        let w = world("link");
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Music/Album")]);
        settle(&mut d);
        let it = d.items();
        assert!(it[0].contains("8 files") && it[0].contains("mostly music"), "{it:?}");
        assert!(it.iter().any(|i| i.starts_with("Link") && i.contains("recommended")) && !it.iter().any(|i| i.starts_with("Copy") && i.contains("recommended")), "{it:?}");
        let before: Vec<_> = std::fs::read_dir(w.host.join("Music/Album")).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().len())).collect();
        pick(&mut d, "Link");
        assert!(d.title() == "Shared read-only", "{}", d.title());
        let m = hf::active_mounts(&w.data, "amy");
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].name.as_str(), m[0].write), ("Album", false));
        assert!(!w.env.dest.exists() || std::fs::read_dir(&w.env.dest).unwrap().count() == 0, "nothing was copied");
        let after: Vec<_> = std::fs::read_dir(w.host.join("Music/Album")).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().len())).collect();
        assert_eq!(before.len(), after.len());
        assert!(audits(&w).contains(&"host.mount.add".to_string()));
        assert_eq!(d.handle(&Event::Key(Key::Enter)), Out::Linked("Album".into()));
        // the choice is remembered for this folder
        assert_eq!(hf::remembered_choice(&w.data, "amy", &w.host.join("Music/Album")), Some(hf::Choice::Link));
        hf::set_host_home(None);
    }

    #[test]
    fn a_small_folder_defaults_to_a_copy_that_does_not_need_the_host_afterwards() {
        let _g = hf::test_guard();
        let w = world("copy");
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Small")]);
        settle(&mut d);
        let it = d.items();
        assert!(it.iter().any(|i| i.starts_with("Copy") && i.contains("recommended")), "{it:?}");
        pick(&mut d, "Copy");
        settle(&mut d);
        assert!(w.env.dest.join("Small/m.csv").exists() && w.env.dest.join("Small/n.txt").exists(), "the folder keeps its shape");
        std::fs::remove_dir_all(w.host.join("Small")).unwrap();
        assert_eq!(std::fs::read_to_string(w.env.dest.join("Small/m.csv")).unwrap(), "a\n1\n", "the copy has no host dependency");
        assert!(hf::active_mounts(&w.data, "amy").is_empty(), "nothing is shared");
        hf::set_host_home(None);
    }

    #[test]
    fn a_folder_outside_the_host_home_can_be_copied_but_not_linked() {
        let _g = hf::test_guard();
        let w = world("outside");
        let other = tmp("outside-other");
        std::fs::write(other.join("x.txt"), "x").unwrap();
        let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![other.clone()]);
        settle(&mut d);
        pick(&mut d, "Link");
        assert_eq!(d.title(), "Could not share it");
        assert!(d.items()[0].contains("outside your host home folder"), "{:?}", d.items());
        assert!(hf::active_mounts(&w.data, "amy").is_empty());
        hf::set_host_home(None);
    }

    #[test]
    fn shared_folders_can_be_made_read_write_converted_to_a_copy_and_stopped() {
        let _g = hf::test_guard();
        let w = world("mounts");
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, "Share");
        pick(&mut d, "Documents/");
        pick(&mut d, "✔ Share this folder (Documents)");
        assert_eq!(d.title(), "Shared read-only");
        d.handle(&Event::Key(Key::Enter));
        let mut d = HostDialog::open(w.env.clone(), ACC);
        assert!(d.title().contains("access to 1 folder"), "{}", d.title());
        pick(&mut d, "host:Documents · read-only");
        assert!(d.items()[0].contains("read-only"));
        pick(&mut d, "Allow writing");
        assert!(d.items()[0].contains("read-write") && d.index_of("Make it read-only").is_some());
        assert!(hf::active_mounts(&w.data, "amy")[0].write);
        pick(&mut d, "Make it read-only");
        assert!(!hf::active_mounts(&w.data, "amy")[0].write);
        // convert to a copy: copies, then stops sharing
        pick(&mut d, "Convert to a copy");
        settle(&mut d);
        assert!(w.env.dest.join("Documents/a.csv").exists());
        assert!(hf::active_mounts(&w.data, "amy").is_empty(), "no longer shared");
        let log = audits(&w);
        assert!(log.contains(&"host.mount.mode".to_string()) && log.contains(&"host.mount.remove".to_string()) && log.contains(&"host.import".to_string()), "{log:?}");
        // share again and stop sharing
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, "Share");
        pick(&mut d, "Music/");
        pick(&mut d, "✔ Share this folder (Music)");
        d.handle(&Event::Key(Key::Enter));
        let mut d = HostDialog::open(w.env.clone(), ACC);
        pick(&mut d, "host:Music");
        pick(&mut d, "Stop sharing");
        assert_eq!(d.title(), "Stopped sharing");
        assert_eq!(d.handle(&Event::Key(Key::Enter)), Out::Unlinked("Music".into()));
        assert!(hf::active_mounts(&w.data, "amy").is_empty());
        hf::set_host_home(None);
    }

    #[test]
    fn saving_to_the_host_never_replaces_silently_and_respects_read_only_shares() {
        let _g = hf::test_guard();
        let w = world("export");
        let src = tmp("export-src").join("result.csv");
        std::fs::write(&src, "a\n1\n").unwrap();
        let mut d = HostDialog::export_file(w.env.clone(), ACC, src.clone());
        pick(&mut d, "★ Documents");
        pick(&mut d, "✔ Save here");
        assert_eq!(d.title(), "Save to your computer");
        // a name that is not plain is refused
        if let Step::SaveName { form, .. } = &mut d.step {
            form.set_value(0, "../x.csv");
        }
        d.handle(&Event::Key(Key::Enter));
        assert!(matches!(&d.step, Step::SaveName { form, .. } if form.error.as_deref().is_some_and(|e| e.contains("plain file name"))));
        if let Step::SaveName { form, .. } = &mut d.step {
            form.set_value(0, "result.csv");
        }
        d.handle(&Event::Key(Key::Enter));
        assert_eq!(std::fs::read_to_string(w.host.join("Documents/result.csv")).unwrap(), "a\n1\n");
        assert!(audits(&w).contains(&"host.export".to_string()));
        // the same name again: refused, nothing replaced
        std::fs::write(&src, "a\n2\n").unwrap();
        let mut d = HostDialog::export_file(w.env.clone(), ACC, src.clone());
        pick(&mut d, "★ Documents");
        pick(&mut d, "✔ Save here");
        d.handle(&Event::Key(Key::Enter));
        assert!(matches!(&d.step, Step::SaveName { form, .. } if form.error.as_deref().is_some_and(|e| e.contains("already exists") && e.contains("nothing was replaced"))));
        assert_eq!(std::fs::read_to_string(w.host.join("Documents/result.csv")).unwrap(), "a\n1\n");
        // a read-only share offers no save
        let mut m = hf::HostMounts::load(&w.data, "amy");
        m.add(&w.host, "~/Documents", None, false).unwrap();
        let mut d = HostDialog::export_file(w.env.clone(), ACC, src.clone());
        pick(&mut d, "★ host:Documents");
        assert!(d.index_of("✔ Save here").is_none() && d.index_of("(this shared folder is read-only").is_some(), "{:?}", d.items());
        hf::set_host_home(None);
    }

    #[test]
    fn the_dialog_and_the_drop_overlay_draw_inside_the_window_at_any_size() {
        let _g = hf::test_guard();
        let w = world("draw");
        for (gw, gh) in [(640u32, 360u32), (1280, 720), (1920, 1080)] {
            let mut g = Gfx::new(gw, gh);
            let mut d = HostDialog::open(w.env.clone(), ACC);
            g.clear([9, 11, 20, 255]);
            d.render(&mut g, R::new(0.0, 0.0, gw as f32, gh as f32), 1.0, 0);
            assert!(g.hits.iter().any(|h| h.label == "Cancel"), "{gw}x{gh}");
            for h in g.hits.iter().filter(|h| h.label != "backdrop") {
                assert!(h.rect.x >= 0.0 && h.rect.y >= 0.0 && h.rect.x + h.rect.w <= gw as f32 + 1.0 && h.rect.y + h.rect.h <= gh as f32 + 1.0, "{} at {gw}x{gh}: {:?}", h.label, h.rect);
            }
            g.clear([9, 11, 20, 255]);
            drop_overlay(&mut g, gw as f32, gh as f32, 1.0, "your Uploads folder", ACC);
            let lit = g.rgba().chunks_exact(4).step_by(53).filter(|p| p[0] > 200).count();
            assert!(lit > 3, "the overlay shows its text at {gw}x{gh}");
            // the progress screen
            let mut d = HostDialog::import_paths(w.env.clone(), ACC, vec![w.host.join("Documents/b.txt")]);
            g.clear([9, 11, 20, 255]);
            d.render(&mut g, R::new(0.0, 0.0, gw as f32, gh as f32), 1.0, 0);
            settle(&mut d);
        }
        hf::set_host_home(None);
    }
}
