//! Surviving the loss of the boot drive (docs/STORAGE-LOSS.md).
//!
//! The root filesystem lives in RAM, so when the USB SSD IYAKKI runs from vanishes for a moment (a loose USB-C cable, a bus
//! glitch, a UAS hiccup) the system keeps running while `/var/lib/iyakki` and `/home` stay mounted on a dead device: every read
//! and write answers EIO. The kernel brings the drive back as a NEW node (`sdb` for the dead `sda`) and nothing remounts it.
//!
//! This module is the pure part: a [`Watcher`] that notices the loss ([`State`]) from the mount table, `/sys/block` and a small
//! read/write probe, the conservative [`Watcher::reattach`] sequence (same UUID, label `IYAKKI-DATA`, not mounted anywhere,
//! stable for 3 s, never fsck), a RAM event ring that is written to the audit log after the drive is back, the plain wording the
//! shell and the apps show, and the USB link facts (speed, hub) that help to tell a cable problem from a hub problem.
//! Everything that touches the machine sits behind [`Env`] and [`Actions`], so the tests use fakes and never touch a device.
//! The real implementations (a thread, `umount2`, `mount`) are in `iyakki-core`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Where the data volume is mounted.
pub const DATA_DIR: &str = "/var/lib/iyakki";
/// `/home` is a bind of `<data>/home`.
pub const HOME_DIR: &str = "/home";
/// A new node is only trusted after it has been there this long without a flap.
pub const STABLE_MS: u64 = 3000;
/// The hidden file the probe writes one byte to.
pub const PROBE_FILE: &str = ".iyakki-probe";
/// Where the new node is mounted first, so a failed mount never leaves the data folder pointing at RAM.
pub const STAGE_DIR: &str = "/run/iyakki-reattach";
/// RAM copy of the watcher's state, for the CLI (`iyakki storage`).
pub const STATUS_FILE: &str = "/run/iyakki/storage-state";
/// The CLI drops this file to ask the watcher to reattach.
pub const REQUEST_FILE: &str = "/run/iyakki/storage-reattach.request";
/// Ring size (events kept in RAM).
pub const RING_MAX: usize = 200;

// ------------------------------------------------------------------------------------------------ states

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Healthy,
    /// The disk behind the mount is gone (or offline).
    Lost,
    /// The disk is there but reads or writes fail (EIO, no answer), or ext4 recorded new errors.
    Errors,
    /// The filesystem was remounted read-only.
    ReadOnly,
    /// The same filesystem (UUID and label) is present again on a new node and is not mounted.
    Reattachable,
    /// A reattach is running.
    Reconnecting,
    /// The automatic or manual reattach failed; the text says why.
    Failed(String),
}

impl State {
    pub fn healthy(&self) -> bool {
        *self == State::Healthy
    }

    pub fn word(&self) -> &'static str {
        match self {
            State::Healthy => "Healthy",
            State::Lost => "Lost",
            State::Errors => "Errors",
            State::ReadOnly => "ReadOnly",
            State::Reattachable => "Reattachable",
            State::Reconnecting => "Reconnecting",
            State::Failed(_) => "Failed",
        }
    }
}

/// The volume that was mounted at the data folder when all was well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    /// Node name, e.g. `sda3`.
    pub dev: String,
    /// `major:minor` of that node.
    pub majmin: String,
    pub uuid: String,
    pub label: String,
    /// ext4 `errors_count` when watching began (only an increase counts).
    pub errors: u64,
    /// Was `/home` bound from the volume?
    pub home_bound: bool,
}

/// A partition that could be the volume again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub majmin: String,
    pub uuid: String,
    pub label: String,
    /// Mounted anywhere?
    pub mounted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    Running,
    /// `/sys/block/<disk>/device/state` says something else (offline, blocked).
    Offline,
    Gone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Ok,
    /// errno of the failed stat or write (5 = EIO).
    Failed(i32),
    /// Nothing came back in time (a hung device).
    TimedOut,
}

/// Everything the watcher looks at.
pub trait Env {
    fn mountinfo(&self) -> String;
    fn disk_state(&self, disk: &str) -> DiskState;
    /// ext4 `errors_count` of a node (0 when unknown).
    fn ext4_errors(&self, dev: &str) -> u64;
    /// A bounded stat and 1-byte fsync'd write of the probe file in the data folder.
    fn probe(&self) -> Probe;
    /// Partitions with an ext4 superblock now.
    fn candidates(&self) -> Vec<Candidate>;
    /// Read the identity (uuid, label) of a node that is mounted now (healthy start).
    fn identity(&self, dev: &str) -> Option<(String, String)>;
    /// `major:minor` of a node.
    fn majmin(&self, dev: &str) -> String;
    fn uptime_secs(&self) -> u64;
}

/// The effects of a reattach.
pub trait Actions {
    /// ext4, `rw,nosuid,nodev,relatime`; the journal is replayed by the mount.
    fn mount(&self, dev: &str, target: &str) -> Result<(), String>;
    /// `umount2(MNT_DETACH)`.
    fn lazy_unmount(&self, target: &str) -> Result<(), String>;
    fn move_mount(&self, from: &str, to: &str) -> Result<(), String>;
    fn bind(&self, src: &str, target: &str) -> Result<(), String>;
    /// Runtime folders the boot makes inside the data folder.
    fn make_dirs(&self) -> Result<(), String>;
    /// Stop what holds files open (k3s); returns the names stopped.
    fn stop_components(&self) -> Vec<String>;
    fn start_components(&self, stopped: &[String]);
}

// ------------------------------------------------------------------------------------------------ the ring

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub uptime: u64,
    pub text: String,
}

/// Events kept in RAM while the audit log (on the dead drive) cannot be written.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    events: VecDeque<Event>,
    next: u64,
    flushed: u64,
}

impl Ring {
    pub fn push(&mut self, uptime: u64, text: impl Into<String>) {
        self.next += 1;
        if self.events.len() >= RING_MAX {
            self.events.pop_front();
        }
        self.events.push_back(Event { seq: self.next, uptime, text: text.into().chars().filter(|c| !c.is_control()).take(300).collect() });
    }

    /// The events not handed out yet (each once).
    pub fn take_unflushed(&mut self) -> Vec<Event> {
        let out: Vec<Event> = self.events.iter().filter(|e| e.seq > self.flushed).cloned().collect();
        self.flushed = self.next;
        out
    }

    pub fn all(&self) -> Vec<Event> {
        self.events.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

// ------------------------------------------------------------------------------------------------ mount table

/// One line of `/proc/self/mountinfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    pub majmin: String,
    pub point: String,
    pub mount_opts: String,
    pub fstype: String,
    pub source: String,
    pub super_opts: String,
}

impl MountEntry {
    pub fn read_only(&self) -> bool {
        self.mount_opts.split(',').chain(self.super_opts.split(',')).any(|o| o == "ro")
    }
}

fn unescape(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")
}

pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let lf: Vec<&str> = left.split_whitespace().collect();
            let rf: Vec<&str> = right.split_whitespace().collect();
            if lf.len() < 6 || rf.len() < 2 {
                return None;
            }
            Some(MountEntry { majmin: lf[2].into(), point: unescape(lf[4]), mount_opts: lf[5].into(), fstype: rf[0].into(), source: unescape(rf[1]), super_opts: rf.get(2).copied().unwrap_or("").into() })
        })
        .collect()
}

/// The newest mount at exactly `dir`.
pub fn mount_at<'a>(table: &'a [MountEntry], dir: &str) -> Option<&'a MountEntry> {
    table.iter().rev().find(|m| m.point == dir)
}

/// `/dev/sda3` or `sda3` to `sda3`.
pub fn node_name(source: &str) -> String {
    source.strip_prefix("/dev/").unwrap_or(source).to_string()
}

/// `sda3` to `sda`; `nvme0n1p3` to `nvme0n1`; `mmcblk0p2` to `mmcblk0`.
pub fn disk_of(node: &str) -> String {
    let digits_end = node.trim_end_matches(|c: char| c.is_ascii_digit());
    let numbered_disk = node.starts_with("nvme") || node.starts_with("mmcblk") || node.starts_with("loop");
    if numbered_disk {
        if let Some(i) = digits_end.strip_suffix('p') {
            if i.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return i.to_string();
            }
        }
        return node.to_string();
    }
    digits_end.to_string()
}

/// ext4 superblock: (uuid, label) of the first 1280 bytes of a partition.
pub fn parse_super(b: &[u8]) -> Option<(String, String)> {
    if b.len() < 1024 + 0x88 || b[1024 + 0x38] != 0x53 || b[1024 + 0x39] != 0xEF {
        return None;
    }
    let u = &b[1024 + 0x68..1024 + 0x78];
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]
    );
    let l = &b[1024 + 0x78..1024 + 0x88];
    let end = l.iter().position(|c| *c == 0).unwrap_or(l.len());
    Some((uuid, String::from_utf8_lossy(&l[..end]).into_owned()))
}

// ------------------------------------------------------------------------------------------------ the watcher

/// What one look changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: State,
    pub to: State,
}

pub struct Watcher {
    pub baseline: Option<Baseline>,
    pub state: State,
    /// Uptime (seconds) when the state was entered.
    pub since: u64,
    /// Uptime (seconds) of the loss, for the one-line event and the banner.
    pub lost_at: Option<u64>,
    pub ring: Ring,
    bad_probes: u32,
    /// The new node being watched for stability: (majmin, first seen ms).
    cand: Option<(String, u64)>,
    /// The last refusal told (not repeated every second).
    refused: Option<String>,
    /// Text for the person after a manual or automatic try.
    pub note: String,
    /// Count of finished reattaches (the shell shows the card once per count).
    pub reattached: u32,
}

impl Default for Watcher {
    fn default() -> Watcher {
        Watcher::new()
    }
}

impl Watcher {
    pub fn new() -> Watcher {
        Watcher { baseline: None, state: State::Healthy, since: 0, lost_at: None, ring: Ring::default(), bad_probes: 0, cand: None, refused: None, note: String::new(), reattached: 0 }
    }

    /// Remember the volume mounted at the data folder (when it is a real ext4 device). False when there is none (RAM only).
    pub fn learn(&mut self, env: &dyn Env) -> bool {
        let table = parse_mountinfo(&env.mountinfo());
        let Some(m) = mount_at(&table, DATA_DIR) else { return false };
        if m.fstype != "ext4" || !m.source.starts_with("/dev/") {
            return false;
        }
        let dev = node_name(&m.source);
        let Some((uuid, label)) = env.identity(&dev) else { return false };
        let home_bound = mount_at(&table, HOME_DIR).is_some();
        self.baseline = Some(Baseline { majmin: env.majmin(&dev), errors: env.ext4_errors(&dev), dev, uuid, label, home_bound });
        true
    }

    fn enter(&mut self, env: &dyn Env, to: State, why: &str) -> Option<Change> {
        if self.state == to {
            return None;
        }
        let from = std::mem::replace(&mut self.state, to.clone());
        self.since = env.uptime_secs();
        if from.healthy() {
            self.lost_at = Some(self.since);
        }
        let up = self.since;
        self.ring.push(up, format!("storage: {} at uptime {up} s ({why})", match &to {
            State::Healthy => "the drive answers again",
            State::Lost => "drive lost",
            State::Errors => "drive errors",
            State::ReadOnly => "drive read-only",
            State::Reattachable => "drive back on a new device",
            State::Reconnecting => "reconnecting",
            State::Failed(_) => "reconnect failed",
        }));
        Some(Change { from, to })
    }

    /// One look (about once a second). `now_ms` is a steady clock. Returns the state change, if any.
    pub fn tick(&mut self, env: &dyn Env, now_ms: u64) -> Option<Change> {
        let base = self.baseline.clone()?;
        if matches!(self.state, State::Reconnecting) {
            return None;
        }
        let table = parse_mountinfo(&env.mountinfo());
        let disk = disk_of(&base.dev);
        let mount = mount_at(&table, DATA_DIR);
        let ds = env.disk_state(&disk);
        // what is wrong now, if anything
        let bad: Option<(State, &str)> = if matches!(ds, DiskState::Gone | DiskState::Offline) {
            Some((State::Lost, if ds == DiskState::Gone { "the device is gone" } else { "the device is offline" }))
        } else if mount.is_none() {
            Some((State::Lost, "nothing is mounted at the data folder"))
        } else if mount.is_some_and(MountEntry::read_only) {
            Some((State::ReadOnly, "the filesystem is mounted read-only"))
        } else {
            match env.probe() {
                Probe::Ok => {
                    self.bad_probes = 0;
                    if env.ext4_errors(&base.dev) > base.errors {
                        Some((State::Errors, "ext4 recorded errors"))
                    } else {
                        None
                    }
                }
                p => {
                    self.bad_probes += 1;
                    // one failure may be a hiccup; two in a row are not
                    (self.bad_probes >= 2).then_some((State::Errors, if p == Probe::TimedOut { "the drive does not answer" } else { "reads or writes fail with an I/O error" }))
                }
            }
        };
        match bad {
            None => {
                self.cand = None;
                match self.state {
                    State::Healthy => None,
                    // a failure state heals by itself only when everything checks out again on the same mount
                    State::Errors | State::ReadOnly | State::Lost | State::Reattachable | State::Failed(_) => self.enter(env, State::Healthy, "same device"),
                    State::Reconnecting => None,
                }
            }
            Some((state, why)) => {
                // a failure already known keeps its state (Reattachable/Failed are refinements of it)
                let mut change = if matches!(self.state, State::Healthy) || (self.state != state && matches!(self.state, State::Errors | State::ReadOnly | State::Lost)) {
                    self.enter(env, state, why)
                } else {
                    None
                };
                if let Some(c) = self.look_for_return(env, &base, now_ms) {
                    change = Some(Change { from: change.map_or(c.from.clone(), |x| x.from), to: c.to });
                }
                change
            }
        }
    }

    /// Is the same filesystem present again? Sets Reattachable (or tells why not).
    fn look_for_return(&mut self, env: &dyn Env, base: &Baseline, now_ms: u64) -> Option<Change> {
        if matches!(self.state, State::Failed(_)) {
            return None;
        }
        let cands = env.candidates();
        let same = cands.iter().find(|c| c.uuid == base.uuid && c.label == base.label && !c.mounted && c.majmin != base.majmin);
        if let Some(c) = same {
            match &self.cand {
                Some((m, _)) if *m == c.majmin => {}
                _ => self.cand = Some((c.majmin.clone(), now_ms)),
            }
            return self.enter(env, State::Reattachable, &format!("{} has the same UUID", c.name));
        }
        self.cand = None;
        // a different disk (even with our label) is never taken
        if let Some(c) = cands.iter().find(|c| c.label == base.label && c.uuid != base.uuid && !c.mounted) {
            let msg = format!("storage: {} carries the label {} but another UUID; not touched", c.name, base.label);
            if self.refused.as_deref() != Some(&msg) {
                self.ring.push(env.uptime_secs(), msg.clone());
                self.refused = Some(msg);
            }
        }
        if matches!(self.state, State::Reattachable) {
            // the new node went away again (a flap): back to the failure it was
            return self.enter(env, State::Lost, "the new device went away again");
        }
        None
    }

    /// Has the new node been steady long enough for an automatic reattach?
    pub fn auto_due(&self, now_ms: u64) -> bool {
        matches!(self.state, State::Reattachable) && self.cand.as_ref().is_some_and(|(_, t)| now_ms.saturating_sub(*t) >= STABLE_MS)
    }

    /// Can the manual button act now (the node is there, any age)?
    pub fn can_reattach(&self) -> bool {
        matches!(self.state, State::Reattachable)
    }

    /// The conservative reattach. `manual`: the person pressed the button (the 3 s wait is not required, everything else is).
    /// `Ok` leaves the state Healthy; `Err` leaves state Failed with the reason, or Lost when the drive is not visible yet.
    pub fn reattach(&mut self, env: &dyn Env, act: &dyn Actions, now_ms: u64, manual: bool) -> Result<(), String> {
        let Some(base) = self.baseline.clone() else { return Err("no drive is being watched".into()) };
        let up = env.uptime_secs();
        let fail = |w: &mut Watcher, e: String| -> Result<(), String> {
            w.ring.push(up, format!("storage: reconnect failed: {e}"));
            w.state = State::Failed(e.clone());
            w.since = up;
            w.note = e.clone();
            Err(e)
        };
        // 1. the exact filesystem, on a new node, mounted nowhere
        let cands = env.candidates();
        let Some(c) = cands.iter().find(|c| c.uuid == base.uuid && c.label == base.label && !c.mounted && c.majmin != base.majmin).cloned() else {
            self.note = "The drive is not visible yet. Check the USB-C cable.".into();
            return Err(self.note.clone());
        };
        if !manual && !self.auto_due(now_ms) {
            return Err("the new device has not been steady for 3 seconds yet".into());
        }
        // the disk behind the old mount must really be gone or failing: never move a healthy mount
        if self.state.healthy() {
            return Err("the drive is healthy; nothing to reconnect".into());
        }
        self.state = State::Reconnecting;
        self.since = up;
        self.ring.push(up, format!("storage: reconnecting {} (UUID {})", c.name, base.uuid));
        let stopped = act.stop_components();
        // 2. mount the new node on a staging folder first (journal replay happens here); nothing else is touched if it fails
        let dev = format!("/dev/{}", c.name);
        if let Err(e) = act.mount(&dev, STAGE_DIR) {
            act.start_components(&stopped);
            return fail(self, format!("the new device could not be mounted ({e})"));
        }
        // 3. lazily detach every stale mount, deepest first
        let table = parse_mountinfo(&env.mountinfo());
        let mut stale: Vec<&MountEntry> = table.iter().filter(|m| m.point == DATA_DIR || m.point.starts_with(&format!("{DATA_DIR}/")) || m.point == HOME_DIR || m.majmin == base.majmin).collect();
        stale.sort_by_key(|m| std::cmp::Reverse(m.point.len()));
        stale.dedup_by(|a, b| a.point == b.point && a.majmin == b.majmin);
        for m in &stale {
            let _ = act.lazy_unmount(&m.point);
        }
        // 4. the staged mount takes the data folder (a direct mount if the move is refused)
        if let Err(e) = act.move_mount(STAGE_DIR, DATA_DIR) {
            let _ = act.lazy_unmount(STAGE_DIR);
            if let Err(e2) = act.mount(&dev, DATA_DIR) {
                return fail(self, format!("the volume could not be put back at the data folder ({e}; {e2})"));
            }
        }
        // 5. /home, runtime folders
        if base.home_bound {
            if let Err(e) = act.bind(&format!("{DATA_DIR}/home"), HOME_DIR) {
                return fail(self, format!("/home could not be bound again ({e})"));
            }
        }
        if let Err(e) = act.make_dirs() {
            return fail(self, format!("the runtime folders could not be made ({e})"));
        }
        // 6. it must really write again
        if env.probe() != Probe::Ok {
            return fail(self, "the drive is mounted again but writing still fails".into());
        }
        act.start_components(&stopped);
        self.baseline = Some(Baseline { dev: c.name.clone(), majmin: c.majmin.clone(), errors: env.ext4_errors(&c.name), ..base });
        self.state = State::Healthy;
        self.since = env.uptime_secs();
        self.cand = None;
        self.bad_probes = 0;
        self.refused = None;
        self.reattached += 1;
        self.note = "Drive reconnected, everything is saved again.".into();
        let up = env.uptime_secs();
        self.ring.push(up, format!("storage: drive reconnected as {} at uptime {up} s", c.name));
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ the published state

/// What the shell and the apps read (a copy, cheap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: State,
    pub since: u64,
    pub lost_at: Option<u64>,
    pub note: String,
    pub reattached: u32,
    /// "Check the drive" result text.
    pub check: String,
    /// Where the drive is plugged in and how fast it runs (for the banner's advice).
    pub link: Option<UsbLink>,
    /// Is the compatible USB mode (no UAS) active in this boot?
    pub compat: bool,
}

impl Default for Snapshot {
    fn default() -> Snapshot {
        Snapshot { state: State::Healthy, since: 0, lost_at: None, note: String::new(), reattached: 0, check: String::new(), link: None, compat: false }
    }
}

struct Published {
    snap: Snapshot,
    ring: Ring,
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);
static COMPAT_ON: AtomicBool = AtomicBool::new(false);
static REQ_REATTACH: AtomicBool = AtomicBool::new(false);
static REQ_CHECK: AtomicBool = AtomicBool::new(false);

fn with_pub<R>(f: impl FnOnce(&mut Published) -> R) -> R {
    let mut g = PUBLISHED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| Published { snap: Snapshot::default(), ring: Ring::default() }))
}

thread_local! {
    static OVERRIDE: std::cell::RefCell<Option<Snapshot>> = const { std::cell::RefCell::new(None) };
}

/// Make [`snapshot`] answer `s` on THIS thread only (tests of the shell and the apps use it; nothing else does). `None` ends it.
pub fn set_thread_override(s: Option<Snapshot>) {
    OVERRIDE.with(|o| *o.borrow_mut() = s);
}

/// The current state (Healthy when nothing is watched).
pub fn snapshot() -> Snapshot {
    if let Some(s) = OVERRIDE.with(|o| o.borrow().clone()) {
        return s;
    }
    with_pub(|p| p.snap.clone())
}

pub fn is_healthy() -> bool {
    snapshot().state.healthy()
}

/// The watcher thread publishes after each look.
pub fn publish(w: &Watcher, check: Option<String>, link: Option<UsbLink>) {
    with_pub(|p| {
        let keep_check = p.snap.check.clone();
        let keep_link = p.snap.link.clone();
        p.snap = Snapshot { state: w.state.clone(), since: w.since, lost_at: w.lost_at, note: w.note.clone(), reattached: w.reattached, check: check.unwrap_or(keep_check), link: link.or(keep_link), compat: COMPAT_ON.load(Ordering::SeqCst) };
        // the ring is the watcher's; the unflushed mark is kept here
        let flushed = p.ring.flushed;
        p.ring = w.ring.clone();
        p.ring.flushed = flushed.min(p.ring.next);
    });
}

/// Tell the snapshot whether this boot runs in the compatible USB mode (read from the kernel command line at start).
pub fn set_compat_active(on: bool) {
    COMPAT_ON.store(on, Ordering::SeqCst);
}

type CompatFn = Box<dyn Fn(bool, &str) -> Result<String, String> + Send + Sync>;
static COMPAT_HOOK: Mutex<Option<CompatFn>> = Mutex::new(None);

/// The real thing that writes or removes the GRUB file on the boot partition (`iyakki-core`).
pub fn register_compat_writer(f: impl Fn(bool, &str) -> Result<String, String> + Send + Sync + 'static) {
    *COMPAT_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

thread_local! {
    static COMPAT_OVERRIDE: std::cell::RefCell<Option<std::rc::Rc<dyn Fn(bool, &str) -> Result<String, String>>>> = std::cell::RefCell::new(None);
}

/// Tests: a fake writer for this thread only.
pub fn set_thread_compat_writer(f: Option<std::rc::Rc<dyn Fn(bool, &str) -> Result<String, String>>>) {
    COMPAT_OVERRIDE.with(|o| *o.borrow_mut() = f);
}

/// Switch the compatible USB mode (no UAS) on or off for the drive `usb_id` (`vvvv:pppp`) for the NEXT start. Only the quirk built
/// by [`compat_quirk`] ever reaches the boot partition.
pub fn set_compat(on: bool, usb_id: &str) -> Result<String, String> {
    if on && compat_quirk(usb_id).is_none() {
        return Err("the drive's USB id is not known, so the mode cannot be set".into());
    }
    if let Some(f) = COMPAT_OVERRIDE.with(|o| o.borrow().clone()) {
        return f(on, usb_id);
    }
    match COMPAT_HOOK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(f) => f(on, usb_id),
        None => Err("only the IYAKKI system can change the boot menu".into()),
    }
}

/// Events to write to the audit log, each once, and only when the drive can take them.
pub fn take_events_for_audit() -> Vec<Event> {
    with_pub(|p| if p.snap.state.healthy() { p.ring.take_unflushed() } else { Vec::new() })
}

pub fn request_reattach() {
    REQ_REATTACH.store(true, Ordering::SeqCst);
}

pub fn take_reattach_request() -> bool {
    REQ_REATTACH.swap(false, Ordering::SeqCst)
}

pub fn request_check() {
    REQ_CHECK.store(true, Ordering::SeqCst);
}

pub fn take_check_request() -> bool {
    REQ_CHECK.swap(false, Ordering::SeqCst)
}

// ------------------------------------------------------------------------------------------------ components that hold the volume open

type Hook = Box<dyn Fn() + Send + Sync>;

struct Component {
    name: String,
    running: Box<dyn Fn() -> bool + Send + Sync>,
    stop: Hook,
    start: Hook,
}

static COMPONENTS: Mutex<Vec<Component>> = Mutex::new(Vec::new());

/// A long-running part that keeps files open on the data volume (the k3s dev cluster): it is stopped before a reattach, when it
/// `running`, and started again after. Registering the same name twice replaces the first.
pub fn register_component(name: &str, running: impl Fn() -> bool + Send + Sync + 'static, stop: impl Fn() + Send + Sync + 'static, start: impl Fn() + Send + Sync + 'static) {
    let mut g = COMPONENTS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|c| c.name != name);
    g.push(Component { name: name.into(), running: Box::new(running), stop: Box::new(stop), start: Box::new(start) });
}

/// Stop the registered components that run; the names stopped.
pub fn stop_registered() -> Vec<String> {
    let g = COMPONENTS.lock().unwrap_or_else(|e| e.into_inner());
    g.iter().filter(|c| (c.running)()).map(|c| {
        (c.stop)();
        c.name.clone()
    }).collect()
}

pub fn start_registered(names: &[String]) {
    let g = COMPONENTS.lock().unwrap_or_else(|e| e.into_inner());
    for c in g.iter().filter(|c| names.contains(&c.name)) {
        (c.start)();
    }
}

// ------------------------------------------------------------------------------------------------ wording

/// What the banner says and offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Reconnect,
    Restart,
    Check,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub title: String,
    pub body: String,
    pub buttons: Vec<Button>,
    /// True while the person may not dismiss it (it can still be minimised).
    pub sticky: bool,
}

/// The banner for a state; None while healthy.
pub fn banner(s: &Snapshot) -> Option<Banner> {
    let advice = link_advice(s.link.as_ref());
    let tail = if advice.is_empty() { String::new() } else { format!(" {advice}") };
    let (title, body, buttons) = match &s.state {
        State::Healthy => return None,
        State::Lost => ("The IYAKKI drive was disconnected", "The IYAKKI drive was disconnected for a moment, so saving is not possible right now. Check the USB-C cable. Anything that was being saved at that moment may be lost.".to_string(), vec![Button::Reconnect, Button::Restart]),
        State::Errors => ("The IYAKKI drive is not answering", "The IYAKKI drive is not answering properly, so saving may fail. Check the USB-C cable. If it stays like this, restart.".to_string(), vec![Button::Reconnect, Button::Restart]),
        State::ReadOnly => ("The IYAKKI drive is read-only", "The IYAKKI drive switched to read-only after an error, so nothing can be saved. Restart to fix it.".to_string(), vec![Button::Restart, Button::Check]),
        State::Reattachable => ("The IYAKKI drive is back", "The IYAKKI drive is back. Reconnecting it in a moment, or press Reconnect now.".to_string(), vec![Button::Reconnect, Button::Restart]),
        State::Reconnecting => ("Reconnecting the drive", "Reconnecting the IYAKKI drive. Please wait, this takes a few seconds.".to_string(), vec![]),
        State::Failed(why) => ("The drive could not be reconnected", format!("IYAKKI could not reconnect the drive by itself: {why}. Restart, or check the drive."), vec![Button::Restart, Button::Check, Button::Reconnect]),
    };
    let mut body = body;
    if !matches!(s.state, State::Reconnecting) {
        body.push_str(&tail);
    }
    if !s.note.is_empty() && !matches!(s.state, State::Failed(_)) {
        body.push(' ');
        body.push_str(&s.note);
    }
    if !s.check.is_empty() {
        body.push_str("\n");
        body.push_str(&s.check);
    }
    Some(Banner { title: title.into(), body, buttons, sticky: true })
}

/// The one-line reason apps give instead of a bare I/O error ("" when healthy).
pub fn unavailable_text(s: &Snapshot) -> String {
    match &s.state {
        State::Healthy => String::new(),
        State::Lost => "The IYAKKI drive is not available: it was disconnected. Check the USB-C cable; saving works again after it reconnects.".into(),
        State::Errors => "The IYAKKI drive is not available: it is not answering properly. Check the USB-C cable.".into(),
        State::ReadOnly => "The IYAKKI drive is not available for saving: it switched to read-only. Restart to fix it.".into(),
        State::Reattachable | State::Reconnecting => "The IYAKKI drive is not available right now: it is being reconnected. Try again in a few seconds.".into(),
        State::Failed(_) => "The IYAKKI drive is not available: it could not be reconnected. Restart IYAKKI.".into(),
    }
}

/// Does a failure message look like the drive talking (I/O error, read-only, dead device)?
fn drive_error_shaped(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["os error 5)", "input/output error", "i/o error", "io error", "os error 30)", "read-only file system", "os error 19)", "no such device", "os error 108", "os error 117", "structure needs cleaning"].iter().any(|p| m.contains(p))
}

fn mentions_data_path(msg: &str) -> bool {
    msg.contains(DATA_DIR) || msg.contains("/home/") || msg.ends_with("/home")
}

/// Reword a failure of something that reads or writes the data volume: while the drive is not healthy, an I/O error (or a
/// "missing" / "failed to load" about a path on the volume) becomes the plain explanation. Healthy: the message is unchanged.
pub fn reword_with(s: &Snapshot, msg: &str) -> String {
    if s.state.healthy() {
        return msg.to_string();
    }
    let lower = msg.to_ascii_lowercase();
    let vague = ["missing", "failed to load", "could not load", "not found", "does not exist", "no such file", "could not save", "could not read", "cannot read", "cannot write", "could not write", "failed"].iter().any(|p| lower.contains(p));
    if drive_error_shaped(msg) || (vague && mentions_data_path(msg)) {
        return unavailable_text(s);
    }
    msg.to_string()
}

/// [`reword_with`] for the live state.
pub fn reword(msg: &str) -> String {
    reword_with(&snapshot(), msg)
}

/// Is `path` on the data volume (or the home folder bound from it)?
pub fn on_data_volume(path: &Path) -> bool {
    path.starts_with(DATA_DIR) || path.starts_with(HOME_DIR)
}

/// The plain explanation for a failed operation on `path`, when the drive is the reason; None otherwise.
pub fn explain_for_path(path: &Path, errno: Option<i32>) -> Option<String> {
    explain_for_path_with(&snapshot(), path, errno)
}

pub fn explain_for_path_with(s: &Snapshot, path: &Path, errno: Option<i32>) -> Option<String> {
    if s.state.healthy() {
        return None;
    }
    (on_data_volume(path) || matches!(errno, Some(5) | Some(19) | Some(30) | Some(108))).then(|| unavailable_text(s))
}

// ------------------------------------------------------------------------------------------------ the USB link

/// Where the drive is plugged in, from sysfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbLink {
    /// e.g. `2-2` or `1-3.4` (bus-port path).
    pub port: String,
    pub speed_mbps: u32,
    /// Behind a hub (the port path has a dot).
    pub behind_hub: bool,
    /// Other non-hub devices on the same hub (0 when not behind a hub).
    pub hub_siblings: u32,
    /// `uas` or `usb-storage`.
    pub driver: String,
    /// `vvvv:pppp`.
    pub id: String,
    pub name: String,
}

impl UsbLink {
    /// "SuperSpeed+ 10 Gbit/s", "SuperSpeed 5 Gbit/s", "High-speed 480 Mbit/s".
    pub fn speed_text(&self) -> String {
        match self.speed_mbps {
            s if s >= 20000 => format!("SuperSpeed+ {} Gbit/s", s / 1000),
            s if s >= 10000 => "SuperSpeed+ 10 Gbit/s".into(),
            s if s >= 5000 => "SuperSpeed 5 Gbit/s".into(),
            480 => "High-speed 480 Mbit/s".into(),
            12 => "Full-speed 12 Mbit/s".into(),
            s => format!("{s} Mbit/s"),
        }
    }

    pub fn slow(&self) -> bool {
        self.speed_mbps > 0 && self.speed_mbps < 5000
    }
}

/// Short advice for the banner and Settings: cable, port, hub. "" when nothing stands out.
pub fn link_advice(l: Option<&UsbLink>) -> String {
    let Some(l) = l else { return String::new() };
    let mut parts = Vec::new();
    if l.slow() {
        parts.push(format!("The drive runs at {} (USB 2 speed), which usually means a worn cable or a USB 2 cable: use the cable that came with the drive.", l.speed_text()));
    }
    if l.behind_hub {
        if l.hub_siblings > 0 {
            parts.push(format!("The drive is behind a USB hub that also serves {} other device(s): plug it straight into the computer.", l.hub_siblings));
        } else {
            parts.push("The drive is behind a USB hub: plug it straight into the computer.".into());
        }
    }
    parts.join(" ")
}

fn read_trim(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Is `name` a USB device directory name (`2-2`, `1-3.4`; not `usb2`, not an interface `2-2:1.0`)?
fn is_usb_dev_name(name: &str) -> bool {
    let Some((bus, port)) = name.split_once('-') else { return false };
    !bus.is_empty() && bus.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit() || c == '.') && !port.starts_with('.') && !port.ends_with('.')
}

/// The USB facts of the disk behind `disk` (`sda`) under `sys` (`/sys`). None when it is not a USB disk.
pub fn usb_link(sys: &Path, disk: &str) -> Option<UsbLink> {
    let real = std::fs::canonicalize(sys.join("block").join(disk)).ok()?;
    // the deepest component that is a USB device name
    let comps: Vec<PathBuf> = real.ancestors().map(Path::to_path_buf).collect();
    let dev = comps.iter().find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_usb_dev_name))?.clone();
    let port = dev.file_name()?.to_str()?.to_string();
    let speed_mbps = read_trim(&dev.join("speed")).parse::<f64>().map(|v| v as u32).unwrap_or(0);
    let behind_hub = port.contains('.');
    // the driver of the mass-storage interface
    let mut driver = String::new();
    if let Ok(rd) = std::fs::read_dir(&dev) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&format!("{port}:")) {
                if let Ok(d) = std::fs::read_link(e.path().join("driver")) {
                    driver = d.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                    break;
                }
            }
        }
    }
    let mut hub_siblings = 0;
    if behind_hub {
        if let Some(parent) = dev.parent() {
            if let Ok(rd) = std::fs::read_dir(parent) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n != port && is_usb_dev_name(&n) && read_trim(&e.path().join("bDeviceClass")) != "09" {
                        hub_siblings += 1;
                    }
                }
            }
        }
    }
    let id = format!("{}:{}", read_trim(&dev.join("idVendor")), read_trim(&dev.join("idProduct")));
    let name = [read_trim(&dev.join("manufacturer")), read_trim(&dev.join("product"))].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    Some(UsbLink { port, speed_mbps, behind_hub, hub_siblings, driver, id, name })
}

/// Keep a USB disk awake: write `on` to `power/control` of the device and of the hubs above it. Returns what was written.
pub fn keep_usb_awake(sys: &Path, disk: &str) -> Vec<String> {
    let mut done = Vec::new();
    let Ok(real) = std::fs::canonicalize(sys.join("block").join(disk)) else { return done };
    for p in real.ancestors() {
        if p.file_name().and_then(|n| n.to_str()).is_some_and(is_usb_dev_name) {
            let ctl = p.join("power/control");
            if ctl.exists() && std::fs::write(&ctl, "on").is_ok() {
                done.push(ctl.display().to_string());
            }
        }
    }
    done
}

/// The one-line diagnostic written when the drive is lost: path, hub, speed, driver.
pub fn link_line(l: Option<&UsbLink>) -> String {
    match l {
        None => "storage: the drive is not a USB disk or its port could not be read".into(),
        Some(l) => format!("storage: drive on USB port {}{} at {} with driver {} ({})", l.port, if l.behind_hub { format!(", behind a hub with {} other device(s)", l.hub_siblings) } else { ", directly on the computer".into() }, l.speed_text(), if l.driver.is_empty() { "unknown" } else { &l.driver }, l.id),
    }
}

// ------------------------------------------------------------------------------------------------ compatible USB mode (no UAS)

/// Name of the file on the boot partition that GRUB sources (`boot/grub/iyakki-usb-compat.cfg`).
pub const COMPAT_FILE: &str = "iyakki-usb-compat.cfg";

/// `usb-storage.quirks=vvvv:pppp:u` for a `vvvv:pppp` id (u = do not use UAS). None for an id that is not 4+4 hex digits.
pub fn compat_quirk(id: &str) -> Option<String> {
    let (v, p) = id.split_once(':')?;
    let hex4 = |s: &str| s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit());
    (hex4(v) && hex4(p)).then(|| format!("usb-storage.quirks={}:{}:u", v.to_ascii_lowercase(), p.to_ascii_lowercase()))
}

/// The GRUB script the switch writes.
pub fn compat_file_text(quirk: &str) -> String {
    format!("# written by IYAKKI Settings, Storage: compatible USB mode (no UAS). Delete this file to go back to the default.\nset iyakki_usbq=\"{quirk}\"\n")
}

/// Is `cmdline` already in the compatible mode?
pub fn compat_active(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|t| t.starts_with("usb-storage.quirks=") && t.rsplit(':').next() == Some("u"))
}

#[cfg(test)]
#[path = "loss_tests.rs"]
mod tests;
