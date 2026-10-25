//! DISKS: see every drive of the machine, mount and unmount partitions, eject safely, and (administrators, behind "Advanced")
//! change partitions (docs/DISKS.md).
//!
//! The engine is `iyakki_agent::disks` (listing, superblock probing, mount and eject) and `iyakki_agent::disks_part` (the partition
//! manager with its safety rules). This file is the state and the keys; `disks_tile_ui.rs` draws it. Nothing blocks the screen:
//! scanning, mounting, ejecting and partition changes run on short threads whose answers are picked up on the heartbeat, and the
//! tile looks for new drives with two small `/proc` reads every couple of seconds while it is on screen.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iyakki_agent::disks::{may_use, Actor, Disk, Disks, MountOpts, MountPlan, Part, Snapshot, Table};
use iyakki_agent::disks_part::{check_confirmation, Confirm, FormatFs, PartOp};
use iyakki_sec::policy::Role;

use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::safeclamp::SafeClamp;
use crate::scrolllist::ScrollList;
use crate::ui::{clipboard, Event, Key, TextField, R};

pub const ACCENT: [u8; 4] = [205, 158, 112, 255];

// click codes (Event::Tab)
pub(super) const T_MOUNT: usize = 1;
pub(super) const T_UNMOUNT: usize = 2;
pub(super) const T_EJECT: usize = 3;
pub(super) const T_OPEN: usize = 4;
pub(super) const T_PROPS: usize = 5;
pub(super) const T_ADV: usize = 6;
pub(super) const T_REFRESH: usize = 7;
pub(super) const T_RO: usize = 8;
pub(super) const T_EXEC: usize = 9;
pub(super) const T_BACK: usize = 10;
pub(super) const T_CLEAR_FILTER: usize = 11;
pub(super) const T_ADV_CLOSE: usize = 12;
pub(super) const T_DISMISS: usize = 13;
pub(super) const T_NEWTABLE: usize = 20;
pub(super) const T_ADD: usize = 21;
pub(super) const T_DELETE: usize = 22;
pub(super) const T_FORMAT: usize = 23;
pub(super) const T_RENAME: usize = 24;
pub(super) const T_RESIZE: usize = 25;
pub(super) const T_REREAD: usize = 26;
pub(super) const T_FS: usize = 30; // 30..35: the file system chips of a form
pub(super) const T_GPT: usize = 36;
pub(super) const T_MBR: usize = 37;
pub(super) const T_REVIEW: usize = 38;
pub(super) const T_CANCEL: usize = 39;
pub(super) const T_APPLY: usize = 40;
pub(super) const T_EJECT_ROW: usize = 1000; // 1000..: the eject button of drive n
pub(super) const T_PART: usize = 2000; // 2000..: partition n of the chosen drive

pub(super) const A_LIST: usize = 62_501;
pub(super) const A_DETAIL: usize = 62_502;
pub(super) const F_FILTER: usize = 62_510;
pub(super) const F_CONFIRM: usize = 62_511;
pub(super) const F_SIZE: usize = 62_512;
pub(super) const F_LABEL: usize = 62_513;

/// What the form of an operation offers as a file system: (what to format, the chip's text).
pub(super) const FS_CHOICES: [(Option<FormatFs>, &str); 5] = [(Some(FormatFs::Ext4), "ext4"), (Some(FormatFs::Fat32), "FAT32"), (Some(FormatFs::Exfat), "exFAT (not yet)"), (Some(FormatFs::Ntfs), "NTFS (not yet)"), (None, "No file system")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    List,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    Drives,
    Parts,
    Filter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormKind {
    NewTable,
    Add,
    Format,
    Rename,
}

#[derive(Clone, Debug)]
pub(super) struct Form {
    pub kind: FormKind,
    pub number: u32,
    pub size: TextField,
    pub label: TextField,
    pub fs: usize,
    pub table: Table,
    /// 0 = size, 1 = label.
    pub focus: usize,
    pub error: String,
}

#[derive(Clone, Debug)]
pub(super) struct Review {
    pub op: PartOp,
    pub disk: String,
    /// The plan, as the engine rendered it.
    pub lines: Vec<String>,
    pub field: TextField,
    pub scroll: usize,
    pub error: String,
    /// 0 = the typed name, 1 = Cancel, 2 = Apply.
    pub focus: usize,
}

#[derive(Clone, Debug)]
pub(super) enum Modal {
    None,
    Form(Form),
    Review(Review),
}

type Slot<T> = Arc<Mutex<Option<T>>>;

pub(super) struct Running {
    pub what: String,
    pub slot: Slot<Result<String, String>>,
}

pub struct DisksApp {
    pub(super) engine: Arc<Disks>,
    pub(super) who: Actor,
    pub(super) snap: Snapshot,
    pub(super) loaded: bool,
    pub(super) finger: u64,
    pub(super) ticks: u32,
    /// Index into `snap.disks`.
    pub(super) sel: usize,
    /// Index into the chosen drive's partitions.
    pub(super) psel: usize,
    pub(super) focus: Focus,
    pub(super) page: Page,
    pub(super) filter: TextField,
    pub(super) list: ScrollList,
    pub(super) detail: ScrollList,
    pub(super) msg: Option<(String, bool)>,
    pub(super) props: bool,
    pub(super) read_only: bool,
    pub(super) allow_exec: bool,
    pub(super) manager: bool,
    pub(super) modal: Modal,
    pub(super) job: Option<Running>,
    scan: Option<Slot<(Snapshot, u64)>>,
    seen: HashSet<String>,
    /// What mounting the chosen partition would do (or why not).
    pub(super) plan: Result<MountPlan, String>,
    /// Where to look: how often (heartbeats) the machine is polled; tests set it to 1.
    pub(super) poll_every: u32,
    /// Run the engine's work on the calling thread (tests).
    pub(super) inline: bool,
    /// Something changed without a key or click: the shell draws again.
    dirty: bool,
    /// The last height of the right side's content (for its scroll bar).
    pub(super) detail_h: f32,
}

/// The owner when the account has no passwd entry: the user IYAKKI itself runs as.
#[cfg(not(test))]
fn process_owner() -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
}

impl DisksApp {
    pub fn new(cx: &AppCx) -> DisksApp {
        // the shell's tests (and the monkey) never see a real drive: they get the made-up machine
        #[cfg(test)]
        let engine = Arc::new(iyakki_agent::disks_fake::machine().disks());
        #[cfg(not(test))]
        let engine = Arc::new(Disks::real());
        DisksApp::with_engine(cx, engine, false)
    }

    pub fn with_engine(cx: &AppCx, engine: Arc<Disks>, inline: bool) -> DisksApp {
        // the drives go into the signed-in person's own home, owned by that account (its passwd entry; the IYAKKI process's own
        // user when the account has none)
        let user = cx.session.user.clone();
        #[cfg(test)]
        let (home, uid, gid) = (format!("/home/{user}"), 1000, 1000);
        #[cfg(not(test))]
        let (home, (uid, gid)) = (iyakki_agent::scope::home_root(&user, std::path::Path::new("/var/lib/iyakki")).display().to_string(), iyakki_agent::disks::account_owner(&user, "/etc/passwd").unwrap_or_else(process_owner));
        let mut a = DisksApp {
            engine,
            who: Actor { user, role: cx.session.role, uid, gid, home },
            snap: Snapshot::default(),
            loaded: false,
            finger: 0,
            ticks: 0,
            sel: 0,
            psel: 0,
            focus: Focus::Drives,
            page: Page::List,
            filter: TextField::new("Filter drives (/)", false),
            list: ScrollList::default(),
            detail: ScrollList::default(),
            msg: None,
            props: false,
            read_only: false,
            allow_exec: false,
            manager: false,
            modal: Modal::None,
            job: None,
            scan: None,
            seen: HashSet::new(),
            plan: Err(String::new()),
            poll_every: 60,
            inline,
            dirty: false,
            detail_h: 0.0,
        };
        a.filter.max = 40;
        a.start_scan();
        a
    }

    // ----------------------------------------------------------------------------------------------------------------
    // what is chosen
    // ----------------------------------------------------------------------------------------------------------------

    pub(super) fn is_admin(&self) -> bool {
        self.who.role == Role::Admin
    }

    pub(super) fn disk(&self) -> Option<&Disk> {
        self.snap.disks.get(self.sel)
    }

    pub(super) fn part(&self) -> Option<&Part> {
        self.disk().and_then(|d| d.parts.get(self.psel))
    }

    /// The drives that match the filter, as indexes into `snap.disks`.
    pub(super) fn visible(&self) -> Vec<usize> {
        let f = self.filter.value.trim().to_lowercase();
        self.snap
            .disks
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                f.is_empty() || {
                    let mut hay = vec![d.title(), d.name.clone(), d.serial.clone(), d.transport.name().to_string()];
                    hay.extend(d.parts.iter().flat_map(|p| [p.label.clone(), p.fs.name().to_string(), p.name.clone()]));
                    hay.iter().any(|h| h.to_lowercase().contains(&f))
                }
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.msg = Some((m.into(), ok));
    }

    /// Recompute what depends on the choice: the mount plan.
    pub(super) fn derive(&mut self) {
        let n = self.snap.disks.len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
        let parts = self.disk().map_or(0, |d| d.parts.len());
        if self.psel >= parts {
            self.psel = parts.saturating_sub(1);
        }
        self.plan = match self.part() {
            Some(p) => self.engine.plan_mount(&self.snap, &self.who, &p.name, &MountOpts { read_only: self.read_only, allow_exec: self.allow_exec }),
            None => Err(String::new()),
        };
        if !self.is_admin() {
            self.manager = false;
        }
    }

    pub(super) fn select(&mut self, i: usize) {
        if i < self.snap.disks.len() && i != self.sel {
            self.sel = i;
            self.psel = 0;
            self.props = false;
            self.manager = false;
            self.detail.to_top();
            self.derive();
        }
    }

    pub(super) fn select_part(&mut self, j: usize) {
        if self.disk().is_some_and(|d| j < d.parts.len()) {
            self.psel = j;
            self.derive();
        }
    }

    // ----------------------------------------------------------------------------------------------------------------
    // scanning and the work threads
    // ----------------------------------------------------------------------------------------------------------------

    pub(super) fn start_scan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let slot: Slot<(Snapshot, u64)> = Arc::new(Mutex::new(None));
        let (e, s2) = (self.engine.clone(), slot.clone());
        let work = move || {
            let fp = e.fingerprint();
            let snap = e.scan();
            if let Ok(mut g) = s2.lock() {
                *g = Some((snap, fp));
            }
        };
        if self.inline {
            work();
        } else {
            std::thread::spawn(work);
        }
        self.scan = Some(slot);
        if self.inline {
            self.poll_scan();
        }
    }

    fn poll_scan(&mut self) {
        let Some(slot) = &self.scan else { return };
        let done = slot.lock().ok().and_then(|mut g| g.take());
        let Some((snap, fp)) = done else { return };
        self.scan = None;
        self.dirty = true;
        self.finger = fp;
        let first = !self.loaded;
        let old = self.disk().map(|d| d.name.clone());
        let part = self.part().map(|p| p.name.clone());
        self.snap = snap;
        self.loaded = true;
        // keep the same drive and partition chosen when they are still there
        if let Some(n) = old {
            if let Some(i) = self.snap.disks.iter().position(|d| d.name == n) {
                self.sel = i;
            }
        }
        if let (Some(p), Some(d)) = (part, self.disk()) {
            if let Some(j) = d.parts.iter().position(|q| q.name == p) {
                self.psel = j;
            }
        }
        // a drive that was not there a moment ago
        let names: Vec<(String, String, bool)> = self.snap.disks.iter().map(|d| (d.name.clone(), format!("{} ({})", d.title(), iyakki_storage::human(d.size_bytes)), d.removable_media())).collect();
        if !first {
            for (n, t, removable) in &names {
                if !self.seen.contains(n) && *removable {
                    let msg = format!("{} drive connected: {t}", if self.snap.disk(n).is_some_and(|d| d.transport == iyakki_agent::disks::Transport::Usb) { "USB" } else { "Storage" });
                    clipboard::toast(&msg);
                    self.say(format!("{msg}. It is selected on the left."), true);
                    if let Some(i) = self.snap.disks.iter().position(|d| d.name == *n) {
                        self.select(i);
                    }
                }
            }
        }
        self.seen = names.into_iter().map(|(n, _, _)| n).collect();
        self.derive();
    }

    fn audit_fn(cx: &AppCx) -> impl Fn(&str, &str) + Send + 'static {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        move |a: &str, d: &str| audit.record(&user, a, d)
    }

    /// Run `f` (a mount, an eject, a partition change) off the screen's thread; the answer is picked up on a heartbeat.
    fn run(&mut self, what: &str, cx: &AppCx, f: impl FnOnce(&Disks, &dyn Fn(&str, &str)) -> Result<String, String> + Send + 'static) {
        if self.job.is_some() {
            self.say("Wait for the current job to finish.", false);
            return;
        }
        let slot: Slot<Result<String, String>> = Arc::new(Mutex::new(None));
        let (e, s2, audit) = (self.engine.clone(), slot.clone(), Self::audit_fn(cx));
        let work = move || {
            let r = f(&e, &audit);
            if let Ok(mut g) = s2.lock() {
                *g = Some(r);
            }
        };
        self.job = Some(Running { what: what.to_string(), slot });
        if self.inline {
            work();
            self.poll_job();
        } else {
            std::thread::spawn(work);
        }
    }

    fn poll_job(&mut self) {
        let Some(j) = &self.job else { return };
        let done = j.slot.lock().ok().and_then(|mut g| g.take());
        let Some(r) = done else { return };
        self.job = None;
        self.dirty = true;
        match r {
            Ok(m) => self.say(m, true),
            Err(e) => self.say(e, false),
        }
        self.modal = Modal::None;
        self.engine.forget();
        self.start_scan();
    }

    /// One heartbeat: pick up finished work and look for changes every couple of seconds.
    pub(super) fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        self.poll_job();
        self.poll_scan();
        if self.scan.is_none() && self.job.is_none() && self.ticks % self.poll_every.max(1) == 0 && self.engine.fingerprint() != self.finger {
            self.start_scan();
        }
    }

    // ----------------------------------------------------------------------------------------------------------------
    // actions
    // ----------------------------------------------------------------------------------------------------------------

    pub(super) fn can_unmount(&self) -> bool {
        match (self.disk(), self.part()) {
            (Some(d), Some(p)) => p.mounted() && !p.system && may_use(&self.who, d).is_ok(),
            _ => false,
        }
    }

    pub(super) fn can_eject(&self) -> bool {
        self.disk().is_some_and(|d| !d.system && may_use(&self.who, d).is_ok())
    }

    pub(super) fn do_mount(&mut self, cx: &AppCx) {
        let Some(p) = self.part().map(|p| p.name.clone()) else { return };
        if let Err(e) = &self.plan {
            let e = e.clone();
            self.say(e, false);
            return;
        }
        let (who, opts) = (self.who.clone(), MountOpts { read_only: self.read_only, allow_exec: self.allow_exec });
        self.run("Opening the drive", cx, move |d, a| {
            d.mount(&who, &p, &opts, a).map(|m| {
                let mut t = format!("Opened at {}{}.", m.target, if m.read_only { " (read-only)" } else { "" });
                for n in m.notes {
                    t.push(' ');
                    t.push_str(&n);
                }
                t
            })
        });
    }

    pub(super) fn do_unmount(&mut self, cx: &AppCx) {
        let Some(p) = self.part().map(|p| p.name.clone()) else { return };
        let who = self.who.clone();
        self.run("Closing the drive", cx, move |d, a| d.unmount(&who, &p, a));
    }

    pub(super) fn do_eject(&mut self, cx: &AppCx, disk: Option<String>) {
        let Some(n) = disk.or_else(|| self.disk().map(|d| d.name.clone())) else { return };
        let who = self.who.clone();
        self.run("Ejecting the drive", cx, move |d, a| d.eject(&who, &n, a));
    }

    /// There is no Files app: the folder's path goes to the clipboard, where the file picker's path box takes it.
    pub(super) fn do_open(&mut self) {
        let Some(mp) = self.part().and_then(|p| p.mountpoints.first().cloned()) else {
            self.say("Open the drive first (Mount).", false);
            return;
        };
        clipboard::copy(&mp);
        self.say(format!("The folder is {mp}. Its path is copied: paste it into the file picker's path box (Ctrl+L in a picker)."), true);
    }

    pub(super) fn open_manager(&mut self) {
        if !self.is_admin() {
            self.say("Only an administrator can change partitions.", false);
            return;
        }
        match self.disk() {
            Some(d) if d.system => {
                let m = format!("{} {}. The drive that holds the running system is never changed.", d.name, d.system_why);
                self.say(m, false);
            }
            Some(_) => {
                self.manager = true;
                self.props = false;
                self.detail.to_top();
            }
            None => {}
        }
    }

    pub(super) fn begin(&mut self, kind: FormKind) {
        let Some(d) = self.disk() else { return };
        if let Some(r) = self.engine.refusal(&self.who, d) {
            self.say(r, false);
            return;
        }
        let number = self.part().map_or(0, |p| p.number);
        if matches!(kind, FormKind::Format | FormKind::Rename) && number == 0 {
            let m = if d.parts.is_empty() { "This drive has no partitions yet." } else { "This drive has a file system but no partition table. Make a new partition table first (it erases the drive), then add partitions." };
            self.say(m, false);
            return;
        }
        let mut f = Form { kind, number, size: TextField::new("empty = all free space (e.g. 8 GB, 500 MB)", false), label: TextField::new("label (optional)", false), fs: 0, table: Table::Gpt, focus: if kind == FormKind::Rename { 1 } else { 0 }, error: String::new() };
        f.size.max = 16;
        f.label.max = 16;
        if kind == FormKind::Rename {
            if let Some(p) = self.part() {
                let l = p.label.clone();
                f.label.set(&l);
            }
        }
        self.modal = Modal::Form(f);
    }

    pub(super) fn begin_delete(&mut self) {
        let Some(p) = self.part().map(|p| p.number) else {
            self.say("Choose a partition first.", false);
            return;
        };
        if p == 0 {
            self.say("This drive has a file system but no partition table, so there is no partition to delete. Make a new partition table (it erases the drive).", false);
            return;
        }
        self.review(PartOp::Delete { number: p });
    }

    pub(super) fn begin_reread(&mut self, cx: &AppCx) {
        let (who, n) = (self.who.clone(), self.disk().map(|d| d.name.clone()));
        let Some(n) = n else { return };
        self.run("Reading the partition table", cx, move |d, a| d.apply_partition(&who, &n, &PartOp::Reread, &Confirm::default(), a));
    }

    /// The dry run: ask the engine for the plan and show it with the drive's name, size and serial.
    pub(super) fn review(&mut self, op: PartOp) {
        let Some(name) = self.disk().map(|d| d.name.clone()) else { return };
        match self.engine.plan_partition(&self.who, &name, &op) {
            Ok(plan) => {
                let lines: Vec<String> = plan.render().lines().map(str::to_string).collect();
                let mut field = TextField::new("type the drive's name or the last 4 of its serial", false);
                field.max = 40;
                self.modal = Modal::Review(Review { op, disk: name, lines, field, scroll: 0, error: String::new(), focus: 0 });
            }
            Err(e) => match &mut self.modal {
                Modal::Form(f) => f.error = e,
                _ => self.say(e, false),
            },
        }
    }

    /// The form's "Review the plan" button.
    pub(super) fn submit_form(&mut self) {
        let Modal::Form(f) = &self.modal else { return };
        let (kind, number, label, table) = (f.kind, f.number, f.label.value.trim().to_string(), f.table);
        let fs = FS_CHOICES.get(f.fs).and_then(|c| c.0);
        let size = f.size.value.trim().to_string();
        let op = match kind {
            FormKind::NewTable => PartOp::NewTable(table),
            FormKind::Add => match parse_size(&size) {
                Ok(s) => PartOp::Add { size_bytes: s, fs, label },
                Err(e) => return self.form_error(e),
            },
            FormKind::Format => match fs {
                Some(fs) => PartOp::Format { number, fs, label },
                None => return self.form_error("Choose a file system.".into()),
            },
            FormKind::Rename => PartOp::Rename { number, label },
        };
        self.review(op);
    }

    fn form_error(&mut self, e: String) {
        if let Modal::Form(f) = &mut self.modal {
            f.error = e;
        }
    }

    /// The typed name matches (what the engine will check again, with the second confirmation).
    pub(super) fn review_typed_ok(&self) -> bool {
        match (&self.modal, self.disk()) {
            (Modal::Review(r), Some(d)) => check_confirmation(d, &Confirm { typed: r.field.value.clone(), second: true }).is_ok(),
            _ => false,
        }
    }

    /// The "Apply" button: the second confirmation. The engine checks everything again before it writes a byte.
    pub(super) fn apply(&mut self, cx: &AppCx) {
        let Modal::Review(r) = &mut self.modal else { return };
        let Some(d) = self.snap.disk(&r.disk).cloned() else {
            r.error = "The drive is gone.".into();
            return;
        };
        let confirm = Confirm { typed: r.field.value.clone(), second: true };
        if let Err(e) = check_confirmation(&d, &confirm) {
            r.error = e;
            r.focus = 0;
            return;
        }
        let (who, op, name) = (self.who.clone(), r.op.clone(), r.disk.clone());
        self.run("Changing the drive", cx, move |e, a| e.apply_partition(&who, &name, &op, &confirm, a));
    }

    // ----------------------------------------------------------------------------------------------------------------
    // events
    // ----------------------------------------------------------------------------------------------------------------

    pub(super) fn click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_MOUNT => self.do_mount(cx),
            T_UNMOUNT => self.do_unmount(cx),
            T_EJECT => self.do_eject(cx, None),
            T_OPEN => self.do_open(),
            T_PROPS => self.props = !self.props,
            T_ADV => self.open_manager(),
            T_ADV_CLOSE => self.manager = false,
            T_REFRESH => {
                self.engine.forget();
                self.start_scan();
                self.say("Looking at the drives again.", true);
            }
            T_RO => {
                self.read_only = !self.read_only;
                self.derive();
            }
            T_EXEC => {
                if self.is_admin() {
                    self.allow_exec = !self.allow_exec;
                    self.derive();
                }
            }
            T_BACK => self.page = Page::List,
            T_DISMISS => self.msg = None,
            T_CLEAR_FILTER => {
                self.filter.clear();
                self.focus = Focus::Drives;
            }
            T_NEWTABLE => self.begin(FormKind::NewTable),
            T_ADD => self.begin(FormKind::Add),
            T_DELETE => self.begin_delete(),
            T_FORMAT => self.begin(FormKind::Format),
            T_RENAME => self.begin(FormKind::Rename),
            T_RESIZE => self.say("Resizing is not supported yet: IYAKKI has no safe, in-house way to grow or shrink a file system, so it does not pretend. Copy the files off, delete the partition and add a new one.", false),
            T_REREAD => self.begin_reread(cx),
            T_REVIEW => self.submit_form(),
            T_CANCEL => self.modal = Modal::None,
            T_APPLY => self.apply(cx),
            T_GPT | T_MBR => {
                if let Modal::Form(f) = &mut self.modal {
                    f.table = if n == T_GPT { Table::Gpt } else { Table::Mbr };
                }
            }
            n if (T_FS..T_FS + FS_CHOICES.len()).contains(&n) => {
                if let Modal::Form(f) = &mut self.modal {
                    f.fs = n - T_FS;
                }
            }
            n if n >= T_PART => {
                self.select_part(n - T_PART);
                self.focus = Focus::Parts;
            }
            n if n >= T_EJECT_ROW => {
                let i = n - T_EJECT_ROW;
                if let Some(name) = self.snap.disks.get(i).map(|d| d.name.clone()) {
                    self.select(i);
                    self.do_eject(cx, Some(name));
                }
            }
            _ => {}
        }
    }

    fn modal_key(&mut self, k: &Key, cx: &AppCx) {
        let typed_ok = self.review_typed_ok();
        match &mut self.modal {
            Modal::Form(f) => match k {
                Key::Esc => self.modal = Modal::None,
                Key::Tab | Key::BackTab => f.focus = 1 - f.focus.min(1),
                Key::Enter => return self.submit_form(),
                Key::Left | Key::Right if f.kind == FormKind::NewTable => f.table = if f.table == Table::Gpt { Table::Mbr } else { Table::Gpt },
                Key::Ctrl('f') if matches!(f.kind, FormKind::Add | FormKind::Format) => f.fs = (f.fs + 1) % FS_CHOICES.len(),
                other => {
                    let field = if f.focus == 0 && f.kind == FormKind::Add { &mut f.size } else { &mut f.label };
                    if f.kind != FormKind::NewTable {
                        field.handle(other);
                    }
                    f.error.clear();
                }
            },
            Modal::Review(r) => match k {
                Key::Esc => self.modal = Modal::None,
                Key::Tab => r.focus = (r.focus + 1) % 3,
                Key::BackTab => r.focus = (r.focus + 2) % 3,
                Key::Enter => match r.focus {
                    1 => self.modal = Modal::None,
                    2 => self.apply(cx),
                    _ => r.focus = if typed_ok { 2 } else { 0 },
                },
                Key::PageUp | Key::Up if r.focus != 0 => r.scroll = r.scroll.saturating_sub(3),
                Key::PageDown | Key::Down if r.focus != 0 => r.scroll += 3,
                other => {
                    if r.focus == 0 {
                        r.field.handle(other);
                        r.error.clear();
                    }
                }
            },
            Modal::None => {}
        }
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        if !matches!(self.modal, Modal::None) {
            self.modal_key(k, cx);
            return Flow::Stay;
        }
        if self.focus == Focus::Filter {
            match k {
                Key::Esc => {
                    self.focus = Focus::Drives;
                    if self.filter.value.is_empty() {
                        return Flow::Stay;
                    }
                    self.filter.clear();
                }
                Key::Enter | Key::Down => self.focus = Focus::Drives,
                other => {
                    self.filter.handle(other);
                }
            }
            let vis = self.visible();
            if let Some(first) = vis.first() {
                if !vis.contains(&self.sel) {
                    self.select(*first);
                }
            }
            return Flow::Stay;
        }
        let vis = self.visible();
        let pos = vis.iter().position(|i| *i == self.sel);
        match k {
            Key::Esc => {
                if self.job.is_some() {
                    return Flow::Stay;
                }
                if self.manager {
                    self.manager = false;
                } else if self.props {
                    self.props = false;
                } else if self.focus == Focus::Parts {
                    self.focus = Focus::Drives;
                } else if self.page == Page::Detail {
                    self.page = Page::List;
                } else if !self.filter.value.is_empty() {
                    self.filter.clear();
                } else {
                    return Flow::Back;
                }
            }
            Key::Char('/') => self.focus = Focus::Filter,
            Key::Up | Key::Down => {
                let d: isize = if matches!(k, Key::Up) { -1 } else { 1 };
                if self.focus == Focus::Parts {
                    let n = self.disk().map_or(0, |d| d.parts.len());
                    if n > 0 {
                        let j = (self.psel as isize + d).clamp_safe(0, n as isize - 1) as usize;
                        self.select_part(j);
                    }
                } else if !vis.is_empty() {
                    let p = pos.unwrap_or(0) as isize;
                    let q = (p + d).clamp_safe(0, vis.len() as isize - 1) as usize;
                    self.select(vis[q]);
                }
            }
            Key::Right | Key::Enter if self.focus == Focus::Drives && self.disk().is_some_and(|d| !d.parts.is_empty()) => {
                self.focus = Focus::Parts;
                self.page = Page::Detail;
            }
            Key::Enter => {
                if self.plan.is_ok() {
                    self.do_mount(cx);
                } else if self.part().is_some_and(Part::mounted) {
                    self.do_open();
                }
            }
            Key::Left => {
                if self.focus == Focus::Parts {
                    self.focus = Focus::Drives;
                } else {
                    self.page = Page::List;
                }
            }
            Key::Char('m') => self.do_mount(cx),
            Key::Char('u') => self.do_unmount(cx),
            Key::Char('e') => self.do_eject(cx, None),
            Key::Char('o') => self.do_open(),
            Key::Char('p') => self.props = !self.props,
            Key::Char('r') if !self.manager => self.click(T_REFRESH, cx),
            Key::Char('a') if !self.manager => self.open_manager(),
            Key::Char('n') if self.manager => self.begin(FormKind::NewTable),
            Key::Char('a') if self.manager => self.begin(FormKind::Add),
            Key::Char('d') if self.manager => self.begin_delete(),
            Key::Char('f') if self.manager => self.begin(FormKind::Format),
            Key::Char('r') if self.manager => self.begin(FormKind::Rename),
            Key::Char('l') if self.manager => self.begin_reread(cx),
            other => {
                if !self.detail.key(other) {
                    self.list.key(other);
                }
            }
        }
        Flow::Stay
    }

    pub(super) fn handle_event(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Tick => {
                self.tick();
                return Flow::Stay;
            }
            Event::Key(k) => return self.key(k, cx),
            Event::Paste(t) => {
                match (&mut self.modal, self.focus) {
                    (Modal::Review(r), _) if r.focus == 0 => {
                        r.field.paste(t);
                    }
                    (Modal::Form(f), _) => {
                        let field = if f.focus == 0 && f.kind == FormKind::Add { &mut f.size } else { &mut f.label };
                        field.paste(t);
                    }
                    (Modal::None, Focus::Filter) => {
                        self.filter.paste(t);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Event::Tab(n) => self.click(*n, cx),
            Event::Row(i) => {
                self.select(*i);
                self.focus = Focus::Drives;
                self.page = Page::Detail;
            }
            Event::FocusAt(id, x) => match *id {
                F_FILTER => {
                    self.focus = Focus::Filter;
                    self.filter.click(*x);
                }
                F_CONFIRM => {
                    if let Modal::Review(r) = &mut self.modal {
                        r.focus = 0;
                        r.field.click(*x);
                    }
                }
                F_SIZE | F_LABEL => {
                    if let Modal::Form(f) = &mut self.modal {
                        f.focus = if *id == F_SIZE { 0 } else { 1 };
                        if *id == F_SIZE { f.size.click(*x) } else { f.label.click(*x) }
                    }
                }
                _ => {}
            },
            other => {
                if let Modal::Review(r) = &mut self.modal {
                    if let Event::Scroll(d) = other {
                        r.scroll = (r.scroll as i32 - d * 3).max(0) as usize;
                    }
                    return Flow::Stay;
                }
                if !self.list.event(other, A_LIST) {
                    self.detail.event(other, A_DETAIL);
                }
            }
        }
        Flow::Stay
    }
}


/// "8 GB", "500MB", "2 GiB", "1.5 G", "" (all free space). Powers of 1024: partitions are sized in MiB.
pub(super) fn parse_size(t: &str) -> Result<Option<u64>, String> {
    let t = t.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().map_err(|_| format!("\"{t}\" is not a size. Write something like 8 GB or 500 MB, or leave it empty for all free space."))?;
    let mult: f64 = match unit.trim().to_lowercase().trim_end_matches('b').trim_end_matches('i') {
        "" | "m" => 1024.0 * 1024.0,
        "k" => 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("\"{t}\": the unit must be MB, GB or TB.")),
    };
    let bytes = n * mult;
    if !bytes.is_finite() || bytes < 1024.0 * 1024.0 {
        return Err("A partition is at least 1 MB.".into());
    }
    Ok(Some(bytes as u64))
}

impl App for DisksApp {
    fn title(&self) -> &'static str {
        "Disks"
    }
    fn icon(&self) -> Icon {
        Icon::Disks
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "↑↓ drives · → partitions · Enter open · m mount · u unmount · e eject · / filter · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "Up / Down · choose a drive (or a partition, after Right or Enter)",
            "Right or Enter · move to the drive's partitions · Left or Esc · back",
            "m · mount the chosen partition (read-only with the switch)   u · unmount   e · eject the whole drive safely",
            "o · copy the folder's path (paste it into a file picker)   p · properties   r · look at the drives again",
            "/ · filter the drives by name, size type, serial or label",
            "a · Advanced: change partitions (administrators). There: n new table, a add, d delete, f format, r rename, l read the table again",
            "Every change asks for the drive's name or the last 4 characters of its serial, shows the plan first and needs a second press.",
            "The drive that holds the running system is never unmounted, ejected or changed.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.handle_event(ev, cx)
    }
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        // a dialog open: its own whole-notch scrolling
        matches!(self.modal, Modal::None) && (self.list.wheel_px(px) || self.detail.wheel_px(px))
    }
    fn wants_redraw(&mut self) -> bool {
        // a finished job or scan; while a job runs the "working" line animates
        std::mem::take(&mut self.dirty) || self.job.is_some()
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}
