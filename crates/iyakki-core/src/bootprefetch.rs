//! Planner for the background prefetch of the boot-order file list (F-BOOT-30, design in `docs/BOOT-SPEED.md`).
//!
//! After the splash, the files the boot read last time (in that order) are asked into the page cache, so the sign-in screen and the first
//! apps find them warm. This module is the pure part: reading the recorded list, sizing the budget from the memory figures, cutting the
//! list into batches, and a small controller that says what to do next (read a batch, wait because the foreground is reading, or stop
//! because memory got short). The reads themselves go through the [`Advisor`] trait, so tests use a fake and the Linux side
//! (`posix_fadvise(WILLNEED)`) is a few lines on top.

use std::path::PathBuf;

pub const MIB: u64 = 1 << 20;

/// Never fill the page cache with more than this much, whatever the machine has.
pub const BUDGET_CAP: u64 = 1024 * MIB;
/// Same reserve rule as the screen features' memory gate: at most a quarter of what is available, and 512 MiB always left free.
pub const BUDGET_SHARE_PCT: u64 = 25;
pub const MIN_FREE: u64 = 512 * MIB;
/// One batch asks for about this much, so the foreground can get in between batches.
pub const BATCH_BYTES: u64 = 8 * MIB;
/// A single file bigger than this is left to the normal lazy read.
pub const MAX_FILE: u64 = 128 * MIB;
/// The foreground counts as reading when it did so within this many milliseconds.
pub const FOREGROUND_QUIET_MS: u64 = 200;
/// Give up after this long even when nothing went wrong (the boot is over by then and the normal cache is in charge).
pub const DEADLINE_MS: u64 = 120_000;

/// One recorded file: its place in the boot order and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
}

/// Parse the recorded list: one `path` or `path<TAB>size` per line, `#` comments. Relative paths, `..` parts and lines with a bad
/// size are dropped, a path that appears twice keeps its first place. Sizes missing from the list are `unknown_size` (the caller can
/// stat them later).
pub fn parse_list(text: &str, unknown_size: u64) -> Vec<Entry> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (p, size) = match line.split_once('\t') {
            Some((p, s)) => match s.trim().parse::<u64>() {
                Ok(n) => (p.trim(), n),
                Err(_) => continue,
            },
            None => (line, unknown_size),
        };
        if !p.starts_with('/') || p.split('/').any(|c| c == "..") || p.contains('\0') {
            continue;
        }
        if seen.insert(p.to_string()) {
            out.push(Entry { path: PathBuf::from(p), size });
        }
    }
    out
}

/// How many bytes may be prefetched, from `MemAvailable`: a quarter of it, keeping 512 MiB free, capped.
pub fn budget(available: u64) -> u64 {
    let quarter = available / 100 * BUDGET_SHARE_PCT + available % 100 * BUDGET_SHARE_PCT / 100;
    quarter.min(available.saturating_sub(MIN_FREE)).min(BUDGET_CAP)
}

/// The list cut into batches, in boot order, within a byte budget.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Plan {
    pub batches: Vec<Vec<Entry>>,
    /// Bytes the plan asks for.
    pub bytes: u64,
    /// Files left out because they are too big, and files left out because the budget ran out.
    pub too_big: usize,
    pub over_budget: usize,
}

/// Cut `entries` into batches of about [`BATCH_BYTES`], stopping when the budget is used up. A file that does not fit the rest of the budget is
/// skipped and the next smaller one may still fit, but order is kept.
pub fn plan(entries: &[Entry], budget: u64) -> Plan {
    let mut p = Plan::default();
    let mut cur: Vec<Entry> = Vec::new();
    let mut cur_bytes = 0u64;
    for e in entries {
        if e.size > MAX_FILE {
            p.too_big += 1;
            continue;
        }
        if p.bytes + e.size > budget {
            p.over_budget += 1;
            continue;
        }
        if !cur.is_empty() && cur_bytes + e.size > BATCH_BYTES {
            p.batches.push(std::mem::take(&mut cur));
            cur_bytes = 0;
        }
        cur_bytes += e.size;
        p.bytes += e.size;
        cur.push(e.clone());
    }
    if !cur.is_empty() {
        p.batches.push(cur);
    }
    p
}

/// Why the prefetch stopped before the end of the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// `MemAvailable` fell so low that the next batch would leave less than [`MIN_FREE`].
    MemoryShort,
    /// [`DEADLINE_MS`] passed.
    Deadline,
    /// The person (or a setting) switched it off.
    Disabled,
}

/// What to do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask for this batch (index into the plan).
    Read(usize),
    /// The foreground read recently: try again after this many milliseconds.
    Wait(u64),
    Stop(StopReason),
    Done,
}

/// The state between steps.
#[derive(Clone, Debug)]
pub struct Controller {
    plan: Plan,
    next: usize,
    started_ms: u64,
    pub enabled: bool,
}

impl Controller {
    pub fn new(plan: Plan, now_ms: u64) -> Controller {
        Controller { plan, next: 0, started_ms: now_ms, enabled: true }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Decide the next step. `last_foreground_read_ms` is when the foreground last read the disk (`None` = not known / never),
    /// `available` is `MemAvailable` now. The caller does the read for `Action::Read` and calls [`Controller::batch_done`].
    pub fn step(&self, now_ms: u64, last_foreground_read_ms: Option<u64>, available: u64) -> Action {
        if !self.enabled {
            return Action::Stop(StopReason::Disabled);
        }
        if self.next >= self.plan.batches.len() {
            return Action::Done;
        }
        if now_ms.saturating_sub(self.started_ms) > DEADLINE_MS {
            return Action::Stop(StopReason::Deadline);
        }
        let need: u64 = self.plan.batches[self.next].iter().map(|e| e.size).sum();
        if available.saturating_sub(need) < MIN_FREE {
            return Action::Stop(StopReason::MemoryShort);
        }
        if let Some(t) = last_foreground_read_ms {
            let since = now_ms.saturating_sub(t);
            if since < FOREGROUND_QUIET_MS {
                return Action::Wait(FOREGROUND_QUIET_MS - since);
            }
        }
        Action::Read(self.next)
    }

    pub fn batch_done(&mut self) {
        self.next += 1;
    }

    /// Batches read so far.
    pub fn done_batches(&self) -> usize {
        self.next
    }
}

/// How the files are asked for: `posix_fadvise(WILLNEED)` or `readahead` on Linux, a recorder in tests.
pub trait Advisor {
    fn will_need(&mut self, path: &std::path::Path, size: u64) -> std::io::Result<()>;
}

/// Run a plan to the end (or until the controller stops it) with the given clock and measurements. Files that cannot be asked for
/// (gone, permission) are counted and skipped; they never stop the run. Returns (batches read, files that failed, final action).
pub fn run(plan: Plan, advisor: &mut dyn Advisor, mut clock: impl FnMut() -> u64, mut foreground: impl FnMut() -> Option<u64>, mut available: impl FnMut() -> u64, mut sleep: impl FnMut(u64)) -> (usize, usize, Action) {
    let mut c = Controller::new(plan, clock());
    let mut failed = 0;
    loop {
        match c.step(clock(), foreground(), available()) {
            Action::Read(i) => {
                for e in &c.plan().batches[i].clone() {
                    if advisor.will_need(&e.path, e.size).is_err() {
                        failed += 1;
                    }
                }
                c.batch_done();
            }
            Action::Wait(ms) => sleep(ms),
            end => return (c.done_batches(), failed, end),
        }
    }
}


// ------------------------------------------------------------------------------------------------ recording the list

/// Where the recorded list is kept (on the data volume), and the boot options that switch recording and prefetching on. Both are off by
/// default: recording needs root and a kernel with fanotify, prefetching is not measured yet (F-BOOT-30).
pub const LIST_PATH: &str = "/var/lib/iyakki/boot-order.list";
pub const OPT_RECORD: &str = "iyakki.bootrecord";
pub const OPT_PREFETCH: &str = "iyakki.prefetch";
/// The most files a list holds, and the size assumed for a line without one.
pub const MAX_RECORDED: usize = 20_000;
pub const UNKNOWN_SIZE: u64 = 64 * 1024;
/// Paths never recorded: kernel views, devices, run-time state and the data volume (those are not boot-order files of the image).
pub const SKIP_PREFIXES: [&str; 6] = ["/proc/", "/sys/", "/dev/", "/run/", "/tmp/", "/var/lib/iyakki/"];

/// Collects the files opened while booting, in the order of their first open.
#[derive(Default)]
pub struct Recorder {
    seen: std::collections::HashSet<String>,
    entries: Vec<(String, u64)>,
}

impl Recorder {
    /// Offer one opened file. True when it was taken (first time, a regular file with a size, a path worth prefetching).
    pub fn accept(&mut self, path: &str, size: u64, regular: bool) -> bool {
        if !regular || size == 0 || self.entries.len() >= MAX_RECORDED || !path.starts_with('/') || path.contains('\0') || path.contains('\t') || path.contains('\n') {
            return false;
        }
        if SKIP_PREFIXES.iter().any(|p| path.starts_with(p)) || path.ends_with(" (deleted)") || !self.seen.insert(path.to_string()) {
            return false;
        }
        self.entries.push((path.to_string(), size));
        true
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The file [`parse_list`] reads back.
    pub fn render(&self) -> String {
        let mut s = String::from("# boot order list: path <TAB> size, in the order of the first open (iyakki.bootrecord=1)\n");
        for (p, n) in &self.entries {
            s.push_str(&format!("{p}\t{n}\n"));
        }
        s
    }
}

/// `some avg10=1.50 avg60=...` (the first line of an io pressure file) as the 10-second average, in percent.
pub fn parse_pressure_avg10(text: &str) -> Option<f32> {
    text.lines().find(|l| l.starts_with("some "))?.split_whitespace().find_map(|w| w.strip_prefix("avg10="))?.parse().ok()
}

/// The foreground counts as reading when the disk stall average is above this many percent.
pub const IO_BUSY_PCT: f32 = 5.0;

#[cfg(target_os = "linux")]
mod linux_part {
    use super::*;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};

    /// Asks the kernel to read each file into the page cache (`posix_fadvise(WILLNEED)` over the whole file).
    pub struct FadviseAdvisor;

    impl Advisor for FadviseAdvisor {
        fn will_need(&mut self, path: &std::path::Path, _size: u64) -> std::io::Result<()> {
            let f = std::fs::File::open(path)?;
            // SAFETY: a valid descriptor owned by `f` for the duration of the call; offset 0 and length 0 mean "the whole file".
            let r = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_WILLNEED) };
            if r == 0 { Ok(()) } else { Err(std::io::Error::from_raw_os_error(r)) }
        }
    }

    /// Watch every file opened on the root filesystem's mount for `secs` seconds and write the list to `out` (atomically). Needs root and
    /// fanotify; returns the number of files recorded.
    pub fn record_for(secs: u64, out: &std::path::Path) -> std::io::Result<usize> {
        // SAFETY: plain system calls with integer flags and a NUL-terminated path; the descriptor is wrapped in a File at once.
        let file = unsafe {
            let fd = libc::fanotify_init(libc::FAN_CLASS_NOTIF | libc::FAN_CLOEXEC, libc::O_RDONLY as u32);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            std::fs::File::from_raw_fd(fd)
        };
        // SAFETY: the descriptor above, a constant mask and a constant NUL-terminated path.
        let marked = unsafe { libc::fanotify_mark(file.as_raw_fd(), libc::FAN_MARK_ADD | libc::FAN_MARK_MOUNT, libc::FAN_OPEN, libc::AT_FDCWD, c"/".as_ptr()) };
        if marked < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let started = std::time::Instant::now();
        let mut rec = Recorder::default();
        let mut buf = vec![0u8; 64 * 1024];
        let mut reader = &file;
        while started.elapsed().as_secs() < secs {
            let mut p = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd and a short timeout.
            if unsafe { libc::poll(&mut p, 1, 500) } <= 0 {
                continue;
            }
            let n = reader.read(&mut buf)?;
            let mut at = 0usize;
            let meta = std::mem::size_of::<libc::fanotify_event_metadata>();
            while at + meta <= n {
                // SAFETY: the kernel wrote a whole event at `at` (checked against `n`); read it unaligned.
                let ev: libc::fanotify_event_metadata = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
                if (ev.event_len as usize) < meta || at + ev.event_len as usize > n {
                    break;
                }
                if ev.fd >= 0 {
                    // SAFETY: the kernel gave this process the descriptor; File closes it.
                    let f = unsafe { std::fs::File::from_raw_fd(ev.fd) };
                    if let (Ok(path), Ok(m)) = (std::fs::read_link(format!("/proc/self/fd/{}", ev.fd)), f.metadata()) {
                        rec.accept(&path.to_string_lossy(), m.len(), m.is_file());
                    }
                }
                at += ev.event_len as usize;
            }
        }
        let tmp = out.with_extension("list.tmp");
        std::fs::write(&tmp, rec.render())?;
        std::fs::rename(&tmp, out)?;
        Ok(rec.len())
    }
}

#[cfg(target_os = "linux")]
pub use linux_part::*;

/// Off Linux (a build on another computer, the tests there) there is no page cache to warm: the advisor reads nothing and says it went well.
#[cfg(not(target_os = "linux"))]
pub struct FadviseAdvisor;

#[cfg(not(target_os = "linux"))]
impl Advisor for FadviseAdvisor {
    fn will_need(&mut self, _path: &std::path::Path, _size: u64) -> std::io::Result<()> {
        Ok(())
    }
}

/// The prefetch run for a list file, with the real clock, memory figures and disk pressure. Linux only; returns the report line.
#[cfg(target_os = "linux")]
pub fn prefetch_from(list: &std::path::Path, avail: &dyn Fn() -> u64) -> String {
    prefetch_with(list, avail(), avail)
}

/// Like [`prefetch_from`], with the memory the plan is made for given apart from the memory seen while it runs: memory that drops after the plan was made
/// stops the run (`MemoryShort`); with too little memory at planning time nothing is planned at all and the run ends at once as `Done` with no batch.
pub fn prefetch_with(list: &std::path::Path, plan_avail: u64, avail: &dyn Fn() -> u64) -> String {
    let Ok(text) = std::fs::read_to_string(list) else { return format!("boot prefetch: no list at {}", list.display()) };
    let plan = plan(&parse_list(&text, UNKNOWN_SIZE), budget(plan_avail));
    let (files, bytes) = (plan.batches.iter().map(Vec::len).sum::<usize>(), plan.bytes);
    let t0 = std::time::Instant::now();
    let ms = || t0.elapsed().as_millis() as u64;
    let busy = || std::fs::read_to_string("/proc/pressure/io").ok().and_then(|t| parse_pressure_avg10(&t)).filter(|p| *p > IO_BUSY_PCT).map(|_| ms());
    let (batches, failed, end) = run(plan, &mut FadviseAdvisor, ms, busy, avail, |m| std::thread::sleep(std::time::Duration::from_millis(m)));
    format!("boot prefetch: {batches} batch(es) of {files} file(s), {} MiB planned, {failed} unreadable, ended {end:?} after {} ms", bytes / MIB, t0.elapsed().as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn e(p: &str, mib: u64) -> Entry {
        Entry { path: PathBuf::from(p), size: mib * MIB }
    }

    #[test]
    fn the_list_keeps_order_drops_bad_lines_and_duplicates() {
        let t = "# boot order\n/usr/bin/a\t1000\n/usr/lib/b\t2000\nrelative/c\t1\n/etc/../shadow\t5\n/usr/bin/a\t1000\n/x\tnope\n/no/size\n\n";
        let l = parse_list(t, 4096);
        let names: Vec<_> = l.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(names, ["/usr/bin/a", "/usr/lib/b", "/no/size"]);
        assert_eq!(l[2].size, 4096);
        assert_eq!(l[0].size, 1000);
    }

    #[test]
    fn memory_that_drops_after_the_plan_stops_the_run_and_too_little_memory_plans_nothing() {
        let d = std::env::temp_dir().join(format!("iyakki-bootlist-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.bin");
        std::fs::write(&f, vec![1u8; 4096]).unwrap();
        let list = d.join("boot-order.list");
        std::fs::write(&list, format!("{}\t4096\n", f.display())).unwrap();
        let short = prefetch_with(&list, 4096 * MIB, &|| 100 * MIB);
        assert!(short.contains("MemoryShort") && short.contains("0 batch(es) of 1 file(s)"), "{short}");
        let none = prefetch_with(&list, 100 * MIB, &|| 100 * MIB);
        assert!(none.contains("ended Done") && none.contains("0 batch(es) of 0 file(s)"), "{none}");
        let all = prefetch_with(&list, 4096 * MIB, &|| 4096 * MIB);
        assert!(all.contains("1 batch(es) of 1 file(s)") && all.contains("ended Done"), "{all}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_budget_is_a_quarter_keeps_512_mib_free_and_is_capped() {
        assert_eq!(budget(0), 0);
        assert_eq!(budget(512 * MIB), 0);
        assert_eq!(budget(600 * MIB), 88 * MIB, "leaves 512 MiB free when a quarter would not");
        assert_eq!(budget(2048 * MIB), 512 * MIB);
        assert_eq!(budget(64 * 1024 * MIB), BUDGET_CAP);
    }

    #[test]
    fn batches_follow_the_order_and_respect_the_budget() {
        let l = [e("/a", 5), e("/b", 5), e("/big", 300), e("/c", 3), e("/d", 40), e("/e", 1)];
        let p = plan(&l, 20 * MIB);
        let names: Vec<Vec<_>> = p.batches.iter().map(|b| b.iter().map(|x| x.path.to_str().unwrap()).collect()).collect();
        assert_eq!(names, vec![vec!["/a"], vec!["/b", "/c"], vec!["/e"]]);
        assert_eq!((p.bytes, p.too_big, p.over_budget), (14 * MIB, 1, 1));
        assert!(plan(&[], 10).batches.is_empty());
        assert_eq!(plan(&l, 0).bytes, 0);
    }

    #[test]
    fn the_foreground_pauses_it_and_low_memory_stops_it() {
        let c = Controller::new(plan(&[e("/a", 6), e("/b", 6)], 100 * MIB), 1000);
        assert_eq!(c.step(1000, None, 4096 * MIB), Action::Read(0));
        assert_eq!(c.step(1100, Some(1050), 4096 * MIB), Action::Wait(150));
        assert_eq!(c.step(1300, Some(1050), 4096 * MIB), Action::Read(0));
        assert_eq!(c.step(1300, None, 515 * MIB), Action::Stop(StopReason::MemoryShort));
        assert_eq!(c.step(1000 + DEADLINE_MS + 1, None, 4096 * MIB), Action::Stop(StopReason::Deadline));
        let mut off = c.clone();
        off.enabled = false;
        assert_eq!(off.step(1000, None, 4096 * MIB), Action::Stop(StopReason::Disabled));
    }

    struct Rec(Vec<String>, usize);
    impl Advisor for Rec {
        fn will_need(&mut self, path: &std::path::Path, _size: u64) -> std::io::Result<()> {
            if path.to_str() == Some("/gone") {
                self.1 += 1;
                return Err(std::io::ErrorKind::NotFound.into());
            }
            self.0.push(path.to_str().unwrap().to_string());
            Ok(())
        }
    }

    #[test]
    fn a_run_reads_every_batch_in_order_waits_for_the_foreground_and_skips_failures() {
        let p = plan(&[e("/a", 6), e("/gone", 6), e("/b", 6)], 100 * MIB);
        let t = Cell::new(0u64);
        let slept = Cell::new(0u64);
        let mut adv = Rec(vec![], 0);
        let (batches, failed, end) = run(p, &mut adv, || t.get(), || if t.get() < 300 { Some(t.get()) } else { None }, || 4096 * MIB, |ms| {
            slept.set(slept.get() + ms);
            t.set(t.get() + ms);
        });
        assert_eq!((batches, failed, end), (3, 1, Action::Done));
        assert_eq!(adv.0, ["/a", "/b"]);
        assert!(slept.get() >= FOREGROUND_QUIET_MS, "it waited for the quiet period");
    }

    #[test]
    fn a_run_stops_when_memory_drops_mid_way() {
        let p = plan(&[e("/a", 6), e("/b", 6)], 100 * MIB);
        let calls = Cell::new(0);
        let mut adv = Rec(vec![], 0);
        let (batches, _, end) = run(p, &mut adv, || 0, || None, || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 { 4096 * MIB } else { 400 * MIB }
        }, |_| {});
        assert_eq!((batches, end), (1, Action::Stop(StopReason::MemoryShort)));
        assert_eq!(adv.0, ["/a"], "the second batch was not asked for");
    }

    #[test]
    fn the_recorder_keeps_first_opens_of_regular_files_in_order_and_skips_what_is_not_an_image_file() {
        let mut r = Recorder::default();
        assert!(r.accept("/usr/bin/iyakki", 5_000_000, true));
        assert!(!r.accept("/usr/bin/iyakki", 5_000_000, true), "only the first open counts");
        assert!(r.accept("/usr/lib/libc.so.6", 2_000_000, true));
        for (p, n, reg) in [("/proc/self/maps", 1, true), ("/sys/class/net", 1, true), ("/dev/null", 1, true), ("/run/x", 1, true), ("/tmp/a", 1, true), ("/var/lib/iyakki/vault", 1, true), ("/usr/share/dir", 4096, false), ("/usr/empty", 0, true), ("relative/x", 1, true), ("/usr/a\tb", 1, true), ("/usr/gone (deleted)", 1, true)] {
            assert!(!r.accept(p, n, reg), "{p}");
        }
        assert_eq!(r.len(), 2);
        // what it writes is what the planner reads back, in the same order
        let back = parse_list(&r.render(), UNKNOWN_SIZE);
        assert_eq!(back.iter().map(|e| (e.path.to_str().unwrap(), e.size)).collect::<Vec<_>>(), [("/usr/bin/iyakki", 5_000_000), ("/usr/lib/libc.so.6", 2_000_000)]);
        // a list is capped
        let mut big = Recorder::default();
        for i in 0..MAX_RECORDED + 10 {
            big.accept(&format!("/usr/f{i}"), 1, true);
        }
        assert_eq!(big.len(), MAX_RECORDED);
    }

    #[test]
    fn disk_pressure_is_read_from_the_some_line() {
        assert_eq!(parse_pressure_avg10("some avg10=12.50 avg60=3.00 avg300=1.00 total=99\nfull avg10=1.00 avg60=0.00 avg300=0.00 total=1\n"), Some(12.5));
        assert_eq!(parse_pressure_avg10("full avg10=1.00\n"), None);
        assert_eq!(parse_pressure_avg10(""), None);
        assert_eq!(parse_pressure_avg10("some avg10=x"), None);
    }
}
