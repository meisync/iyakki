//! IYAKKI first-stage init (`/init` of the lean initial RAM disk, docs/BOOT-SPEED.md).
//!
//! The lean layout keeps only this program, BusyBox (for an emergency shell) and empty folders in the initial RAM disk, so GRUB reads a few
//! MB instead of the whole system. This program finds the real root on the stick, puts a writable overlay (upper layer on tmpfs, which can be
//! swapped to zram) over it, and switches to it; the files of the system are read from the stick only when they are used.
//! It starts no process (system calls only) and writes nothing to any drive.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("iyakki-stage1 runs as /init on Linux only");
}

#[cfg(target_os = "linux")]
mod linux {
    use iyakki_stage1::*;
    use std::ffi::CString;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const WAIT_FOR_DEVICES: Duration = Duration::from_millis(WAIT_FOR_DEVICES_MS);
    /// How long a failure message stays on the screen before the machine restarts (back to the boot menu, whose first entry is the normal boot).
    const FAIL_SHOWN: Duration = Duration::from_secs(30);

    /// Everything the first stage says, with the kernel uptime in front, kept in memory and written to the stick (`flush_log`) so a boot that
    /// ends in a blank screen can be read afterwards on another computer.
    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static FLUSHED: AtomicUsize = AtomicUsize::new(0);
    static BOOT_UP: AtomicBool = AtomicBool::new(false);
    static DATA_UP: AtomicBool = AtomicBool::new(false);
    const LOG_MAX: usize = 64 * 1024;

    fn uptime() -> f64 {
        std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse().ok())).unwrap_or(0.0)
    }

    fn note(line: &str) {
        if let Ok(mut l) = LOG.lock() {
            l.push(format!("[{:>8.3}] {line}", uptime()));
        }
    }

    fn say(msg: &str) {
        let line = format!("stage1: {msg}");
        note(&line);
        // never `eprintln!`: it PANICS when the write fails, and /dev/console is the last `console=` word (ttyS0), which fails every write on a
        // machine without that serial port (a Mac): the panic would kill PID 1 and leave a blank screen
        let _ = writeln!(std::io::stderr(), "{line}");
        if let Ok(mut k) = std::fs::OpenOptions::new().write(true).open("/dev/kmsg") {
            let _ = k.write_all(format!("<6>{line}\n").as_bytes());
        }
    }

    /// A line the person must see: kernel log level 2, which every console prints even with `quiet loglevel=3` (the screen too). `/dev/console`
    /// is only the LAST `console=` of the command line (the serial port, which a Mac does not have), so plain stderr would never reach the screen.
    fn say_loud(msg: &str) {
        let line = format!("stage1: {msg}");
        note(&line);
        let ok = std::fs::OpenOptions::new().write(true).open("/dev/kmsg").and_then(|mut k| k.write_all(format!("<2>{line}\n").as_bytes())).is_ok();
        if !ok {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }

    /// A short status line straight onto the screen's text console (tty0), whatever the log level: a blank screen can be told apart from a working stage.
    fn screen(msg: &str) {
        note(&format!("screen: {msg}"));
        if let Ok(mut t) = std::fs::OpenOptions::new().write(true).open("/dev/tty0") {
            let _ = t.write_all(format!("  IYAKKI fast start: {msg}\r\n").as_bytes());
        }
    }

    /// The kernel log so far (non-blocking read of /dev/kmsg), the last `max` lines.
    fn kmsg_tail(max: usize) -> Vec<String> {
        let Ok(mut f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/kmsg") else { return Vec::new() };
        let mut out = Vec::new();
        let mut buf = vec![0u8; 8192];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Some(l) = kmsg_line(&String::from_utf8_lossy(&buf[..n])) {
                out.push(l);
            }
        }
        let skip = out.len().saturating_sub(max);
        out.split_off(skip)
    }

    /// Append the new log lines to `boot/stage1.log` on the boot volume (mounted read-only: remounted read-write for this one small write, fsynced,
    /// then read-only again), or to `iyakki/stage1.log` on the data volume when only that one is up. Bounded to 64 KB. Failure is silent: a log must
    /// never stop the boot.
    fn flush_log() {
        let lines: Vec<String> = LOG.lock().map(|l| l.clone()).unwrap_or_default();
        let done = FLUSHED.load(Ordering::SeqCst).min(lines.len());
        if done == lines.len() {
            return;
        }
        let text = lines[done..].join("\n") + "\n";
        let (dir, file, boot) = if BOOT_UP.load(Ordering::SeqCst) {
            ("/s/boot", "/s/boot/boot/stage1.log", true)
        } else if DATA_UP.load(Ordering::SeqCst) {
            ("/s/data", "/s/data/iyakki/stage1.log", false)
        } else {
            return;
        };
        if boot {
            let _ = mnt("", dir, None, libc::MS_REMOUNT | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "");
        }
        let _ = std::fs::create_dir_all(Path::new(file).parent().unwrap_or(Path::new("/")));
        let old = std::fs::read_to_string(file).unwrap_or_default();
        let new = bounded_log(&old, &text, LOG_MAX);
        let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(file).and_then(|mut f| {
            f.write_all(new.as_bytes())?;
            f.sync_all()
        });
        if ok.is_ok() {
            FLUSHED.store(lines.len(), Ordering::SeqCst);
        }
        if boot {
            let _ = mnt("", dir, None, libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "");
        }
    }

    fn c(p: &str) -> CString {
        CString::new(p).unwrap_or_default()
    }

    fn mnt(src: &str, tgt: &str, fs: Option<&str>, flags: libc::c_ulong, data: &str) -> Result<(), String> {
        let (s, t, f, d) = (c(src), c(tgt), fs.map(c), c(data));
        // SAFETY: all pointers are valid NUL-terminated strings that live for the call.
        let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ref().map_or(std::ptr::null(), |x| x.as_ptr()), flags, d.as_ptr().cast()) };
        if rc == 0 { Ok(()) } else { Err(format!("mount {src} on {tgt}: {}", std::io::Error::last_os_error())) }
    }

    fn mkdirs(p: &str) {
        let _ = std::fs::create_dir_all(p);
    }

    /// The patch layers to put over the root (full paths), decided by [`plan_patches`]. Counts this start in `boot-count` (the running system
    /// removes it once it is up); with too many starts in a row that did not get there, writes `disabled` and uses none. Never fails the boot.
    fn patch_layers(root_path: &str) -> Vec<String> {
        let dir = format!("/s/data/{PATCHES_DIR}");
        let Ok(active) = std::fs::read_to_string(format!("{dir}/active")) else { return Vec::new() };
        let attempts = parse_count(&std::fs::read_to_string(format!("{dir}/boot-count")).unwrap_or_default());
        let read = |p: &str| std::fs::read_to_string(format!("/s/data/{p}")).ok();
        let stat = |p: &str| std::fs::symlink_metadata(format!("/s/data/{p}")).ok().map(|m| m.len());
        let plan = plan_patches(&active, attempts, root_id_of(root_path), &read, &stat);
        for (id, why) in &plan.skipped {
            say(&format!("patch {id} skipped: {why}"));
        }
        if plan.guard {
            let _ = std::fs::write(format!("{dir}/disabled"), format!("{attempts} starts in a row with patches did not reach the system; no patch is used until one is applied or removed again\n"));
        }
        if !plan.skipped.is_empty() {
            let list: String = plan.skipped.iter().map(|(i, w)| format!("{i}: {w}\n")).collect();
            let _ = std::fs::write(format!("{dir}/skipped"), list);
        }
        // which patches this start really uses (`iyakki dev patch list` tells live from next-start with it)
        let used: String = plan.layers.iter().filter_map(|l| l.rsplit('/').nth(1)).map(|i| format!("{i}\n")).collect();
        let _ = std::fs::write(format!("{dir}/booted"), used);
        if plan.layers.is_empty() {
            return Vec::new();
        }
        let _ = std::fs::write(format!("{dir}/boot-count"), format!("{}\n", attempts + 1));
        plan.layers.iter().map(|l| format!("/s/data/{l}")).collect()
    }

    fn raw_head(dev: &str, off: u64, len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        let ok = std::fs::File::open(dev).and_then(|mut f| {
            f.seek(SeekFrom::Start(off))?;
            f.read_exact(&mut b)
        });
        if ok.is_ok() { b } else { Vec::new() }
    }

    /// Devices whose read did not answer in time (a hidden or sick disk, e.g. the internal one behind a T2 chip): left alone for 10 s.
    static HUNG: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

    /// Read a few bytes of a device, but never wait for it forever: a read that hangs would leave a blank screen.
    fn head(dev: &str, off: u64, len: usize) -> Vec<u8> {
        if HUNG.lock().map(|h| h.iter().any(|(d, t)| d == dev && t.elapsed() < Duration::from_secs(10))).unwrap_or(false) {
            return Vec::new();
        }
        let (tx, rx) = mpsc::channel();
        let d = dev.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(raw_head(&d, off, len));
        });
        match rx.recv_timeout(Duration::from_millis(2500)) {
            Ok(v) => v,
            Err(_) => {
                note(&format!("{dev}: no answer to a read of {len} bytes in 2.5 s, skipped for 10 s"));
                if let Ok(mut h) = HUNG.lock() {
                    h.retain(|(d, _)| d != dev);
                    h.push((dev.to_string(), Instant::now()));
                }
                Vec::new()
            }
        }
    }

    /// (boot device, data device, one report line per block device) among the block devices that exist now.
    fn scan() -> (Option<String>, Option<String>, Vec<String>) {
        let (mut boot, mut data) = (None, None);
        let mut report = Vec::new();
        let mut names: Vec<String> = std::fs::read_dir("/sys/class/block").into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in names {
            // nvme: the internal disk of a Mac sits behind its T2 chip, where a read can stall for a long time, and a read stuck in the kernel
            // would also stop the final exec; the stick is a `sd` (USB, SATA) or `vd` (virtual) disk
            if ["loop", "ram", "zram", "dm-", "sr", "fd", "nvme", "mmcblk"].iter().any(|p| n.starts_with(p)) {
                continue;
            }
            let dev = format!("/dev/{n}");
            let sectors: u64 = std::fs::read_to_string(format!("/sys/class/block/{n}/size")).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
            let mut what = String::new();
            let sector0 = head(&dev, 0, 512);
            if let Some(l) = fat32_label(&sector0) {
                // the label of the boot sector, else the volume-label entry of the root directory (some formatters write only one of them)
                let mut shown = format!("FAT {l:?}");
                let mut is_boot = is_boot_label(&l);
                if !is_boot {
                    if let Some((off, len)) = fat32_root_dir(&sector0) {
                        if let Some(rl) = fat_label_in_root(&head(&dev, off, len)) {
                            shown = format!("FAT {l:?} (root directory label {rl:?})");
                            is_boot = is_boot_label(&rl);
                        }
                    }
                }
                what = shown;
                if boot.is_none() && is_boot {
                    boot = Some(dev.clone());
                }
            }
            if let Some(l) = ext4_label(&head(&dev, 1024, 1024)) {
                what = format!("ext {l:?}");
                if data.is_none() && l == DATA_LABEL {
                    data = Some(dev.clone());
                }
            }
            if what.is_empty() {
                what = if sector0.is_empty() { "unreadable".to_string() } else { "no known file system".to_string() };
            }
            report.push(format!("{n} {} {what}", human_size(sectors)));
        }
        (boot, data, report)
    }

    /// Last resort when no label matched but a disk looks like ours: a FAT partition (any label) of an `sd` disk that holds `boot/initrd-lean`
    /// (so it is the stick the lean files were put on), followed by the next partition as the data volume (an ext2/3/4 file system, any label).
    /// The FAT volume is looked into read-only and left again; nothing is written.
    fn fallback_by_layout() -> (Option<String>, Option<String>) {
        let mut names: Vec<String> = std::fs::read_dir("/sys/class/block").into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in &names {
            let Some(num) = n.strip_prefix("sd").and_then(|r| r.trim_start_matches(|c: char| c.is_ascii_lowercase()).parse::<u32>().ok()) else { continue };
            let dev = format!("/dev/{n}");
            if fat32_label(&head(&dev, 0, 512)).is_none() {
                continue;
            }
            if mnt(&dev, "/s/boot", Some("vfat"), libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "").is_err() {
                continue;
            }
            let ours = Path::new("/s/boot/boot/initrd-lean").is_file();
            let p = c("/s/boot");
            // SAFETY: valid NUL-terminated path.
            unsafe { libc::umount2(p.as_ptr(), 0) };
            if !ours {
                continue;
            }
            let next = format!("{}{}", n.trim_end_matches(|c: char| c.is_ascii_digit()), num + 1);
            if names.contains(&next) && ext4_label(&head(&format!("/dev/{next}"), 1024, 1024)).is_some() {
                note(&format!("no label matched; by the layout: boot {dev} (has boot/initrd-lean), data /dev/{next}"));
                return (Some(dev), Some(format!("/dev/{next}")));
            }
        }
        (None, None)
    }

    /// Attach a file to a free loop device (read-only) and return the device node.
    fn loop_attach(file: &Path) -> Result<String, String> {
        const LOOP_SET_FD: libc::c_ulong = 0x4C00;
        const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4C82;
        let ctl = std::fs::OpenOptions::new().read(true).write(true).open("/dev/loop-control").map_err(|e| format!("/dev/loop-control: {e}"))?;
        // SAFETY: plain ioctl on an open fd, no pointer argument.
        let n = unsafe { libc::ioctl(ctl.as_raw_fd(), LOOP_CTL_GET_FREE as _) };
        if n < 0 {
            return Err(format!("no free loop device: {}", std::io::Error::last_os_error()));
        }
        let node = format!("/dev/loop{n}");
        for _ in 0..50 {
            if Path::new(&node).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let backing = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let dev = std::fs::OpenOptions::new().read(true).open(&node).map_err(|e| format!("{node}: {e}"))?;
        // SAFETY: both fds are open; the argument is an fd number.
        let rc = unsafe { libc::ioctl(dev.as_raw_fd(), LOOP_SET_FD as _, backing.as_raw_fd()) };
        if rc < 0 {
            return Err(format!("LOOP_SET_FD {node}: {}", std::io::Error::last_os_error()));
        }
        Ok(node)
    }

    fn volume_dir(v: Volume) -> &'static str {
        match v {
            Volume::Data => "/s/data",
            Volume::Boot => "/s/boot",
        }
    }

    /// Present one candidate at /lower (read-only).
    fn present(cand: &Candidate) -> Result<(), String> {
        let src = PathBuf::from(volume_dir(cand.volume)).join(&cand.path);
        match cand.kind {
            Kind::Tree => {
                if !src.is_dir() {
                    return Err(format!("{} is not a folder", src.display()));
                }
                mnt(&src.to_string_lossy(), "/lower", None, libc::MS_BIND, "")?;
                mnt("", "/lower", None, libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY, "")
            }
            Kind::Image => {
                let len = std::fs::metadata(&src).map_err(|e| format!("{}: {e}", src.display()))?.len();
                if let Some(want) = cand.size {
                    if want != len {
                        return Err(format!("{} is {len} bytes, the pointer says {want}: an unfinished update", src.display()));
                    }
                }
                let node = loop_attach(&src)?;
                mnt(&node, "/lower", Some("squashfs"), libc::MS_RDONLY, "")
            }
        }
    }

    /// The file on the boot volume that tells GRUB the lean boot did not work (`boot/grub/iyakki-lean-failed`): while it is there the menu's
    /// first entry starts the full boot instead of the lean one, so a broken lean tree never makes the machine restart in a loop. Written by
    /// [`fail`], removed here by the first lean start that gets as far as switching root. The boot volume is remounted read-write only when
    /// there is something to write or remove. A failure to do either is silent (the log is the only thing that must never stop the boot).
    const LEAN_FAILED: &str = "/s/boot/boot/grub/iyakki-lean-failed";

    fn lean_marker(failed_because: Option<&str>) {
        if !BOOT_UP.load(Ordering::SeqCst) {
            return;
        }
        let exists = Path::new(LEAN_FAILED).exists();
        if failed_because.is_none() && !exists {
            return;
        }
        let rw = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        let _ = mnt("", "/s/boot", None, libc::MS_REMOUNT | rw, "");
        match failed_because {
            Some(why) => {
                let _ = std::fs::write(LEAN_FAILED, format!("{why}\n"));
            }
            None => {
                let _ = std::fs::remove_file(LEAN_FAILED);
            }
        }
        // SAFETY: sync takes no pointers.
        unsafe { libc::sync() };
        let _ = mnt("", "/s/boot", None, libc::MS_REMOUNT | libc::MS_RDONLY | rw, "");
    }

    /// A failure the person can read: says what is wrong and how to get the normal boot back, then restarts after a pause (the marker file makes
    /// the boot menu's default the normal full boot, see [`lean_marker`]). `iyakki.stage1=shell` gives a shell instead. Never hangs silently.
    fn fail(why: &str) -> ! {
        for l in [
            format!("The fast start (lean boot) cannot start: {why}"),
            "Only a small log (boot/stage1.log) was written to the drive. The next start uses the full boot by itself (the entry named IYAKKI); it does not use the lean files.".to_string(),
            format!("This machine restarts by itself in {} seconds. To look around instead, add iyakki.stage1=shell to the entry's command line.", FAIL_SHOWN.as_secs()),
        ] {
            say_loud(&l);
        }
        note("outcome: failed, restarting in 30 s");
        lean_marker(Some(&why.replace('\n', " ")));
        flush_log();
        std::thread::sleep(FAIL_SHOWN);
        say_loud("restarting now");
        // SAFETY: sync and reboot take no pointers; reboot only returns on failure.
        unsafe {
            libc::sync();
            libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn shell(why: &str) -> ! {
        say_loud(&format!("{why}; starting an emergency shell (BusyBox) on the screen and the keyboard"));
        // the shell talks to the screen (tty0), like a person at a Mac needs; the serial console is only the last console= word
        for tty in ["/dev/tty0", "/dev/console"] {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(tty) {
                let fd = f.as_raw_fd();
                // SAFETY: dup2 of an open fd onto the standard descriptors.
                unsafe {
                    libc::dup2(fd, 0);
                    libc::dup2(fd, 1);
                    libc::dup2(fd, 2);
                }
                break;
            }
        }
        let sh = c("/bin/busybox");
        let args = [c("sh"), c("-i")];
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());
        // SAFETY: argv is NULL-terminated and its strings outlive the call; execv only returns on failure.
        unsafe { libc::execv(sh.as_ptr(), argv.as_ptr()) };
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn redirect_console() {
        if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/console") {
            let fd = f.as_raw_fd();
            // SAFETY: dup2 of an open fd onto the standard descriptors.
            unsafe {
                libc::dup2(fd, 0);
                libc::dup2(fd, 1);
                libc::dup2(fd, 2);
            }
        }
    }

    /// A small helper process that outlives this program's exec: for 3 minutes it copies the kernel log to `stage1-kmsg.log` in the data volume's top
    /// folder (through a directory handle opened before the volume was moved, so it needs no path), so a start that stops before the system
    /// writes its own log can still be read on another computer. It is the only process this program starts, and only on a lean boot.
    fn start_log_child(dirfd: i32, kfd: i32) {
        // SAFETY: fork in a program with no other running threads of consequence; the child only uses system calls and its own memory.
        let pid = unsafe { libc::fork() };
        if pid != 0 {
            return;
        }
        // SAFETY: plain system calls in the child.
        unsafe { libc::setsid() };
        let mut lines: Vec<String> = Vec::new();
        let mut buf = vec![0u8; 8192];
        let name = c("stage1-kmsg.log");
        for _ in 0..90 {
            loop {
                // SAFETY: buf is a valid writable buffer of the given length; kfd is an open descriptor.
                let n = unsafe { libc::read(kfd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPIPE) {
                    continue; // records were overwritten: go on with the oldest left
                }
                if n <= 0 {
                    break;
                }
                if let Some(l) = kmsg_line(&String::from_utf8_lossy(&buf[..n as usize])) {
                    lines.push(l);
                }
            }
            let skip = lines.len().saturating_sub(4000);
            let text = bounded_log("", &(lines[skip..].join("\n") + "\n"), 256 * 1024);
            // SAFETY: openat on the directory handle with a valid NUL-terminated name.
            let fd = unsafe { libc::openat(dirfd, name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600) };
            if fd >= 0 {
                let b = text.as_bytes();
                // SAFETY: b is a valid slice; fd is open.
                unsafe {
                    libc::write(fd, b.as_ptr().cast(), b.len());
                    libc::fsync(fd);
                    libc::close(fd);
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        // SAFETY: leaves the child without running destructors of the parent's state.
        unsafe { libc::_exit(0) };
    }

    /// Remove what is left of the initial RAM disk's own files (they sit in non-swappable memory), except the mount points in use.
    fn free_old_root() {
        for e in std::fs::read_dir("/").into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if ["newroot", "s", "lower", "upper", "dev", "proc", "sys"].contains(&n.as_str()) {
                continue;
            }
            let p = e.path();
            let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }

    pub fn run() {
        let t0 = Instant::now();
        // a panic of the main thread must not leave a dead PID 1 and a blank screen: say so (screen, log on the stick) and restart
        std::panic::set_hook(Box::new(|info| {
            if std::thread::current().name() == Some("main") {
                fail(&format!("internal error in the first stage: {info}"));
            }
        }));
        for d in ["/dev", "/proc", "/sys", "/s/boot", "/s/data", "/lower", "/upper", "/newroot"] {
            mkdirs(d);
        }
        let _ = mnt("devtmpfs", "/dev", Some("devtmpfs"), 0, "mode=0755");
        let _ = mnt("proc", "/proc", Some("proc"), 0, "");
        let _ = mnt("sysfs", "/sys", Some("sysfs"), 0, "");
        // the kernel limits what programs may write to its log to about 10 lines per 5 seconds ("ratelimit", the default): the first stage's
        // own lines and the system's boot lines would be dropped, and a remote check could not read the timeline. Switch the limit off.
        let _ = std::fs::write("/proc/sys/kernel/printk_devkmsg", "on\n");
        // the kernel could not open /dev/console for PID 1 (the RAM disk had no device node): messages go to the screen from here on
        redirect_console();
        let opts = options(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default());
        say("started; looking for the IYAKKI-BOOT and IYAKKI-DATA volumes");
        if opts.shell {
            shell("iyakki.stage1=shell");
        }
        // 1. the stick's partitions (USB disks appear some time after the drivers: the drivers are built into the kernel)
        let start = Instant::now();
        let (mut boot, mut data, mut report);
        let mut first_seen: Option<Instant> = None;
        let mut shown = 0u64;
        let mut tried_layout = false;
        screen("looking for the IYAKKI drive...");
        loop {
            (boot, data, report) = scan();
            // the disk shows up some time after the kernel starts (a USB SSD: 0.5 to 3 s, behind a Mac's chain maybe later); the decision is
            // in the library (tested)
            if (boot.is_some() || data.is_some()) && first_seen.is_none() {
                first_seen = Some(Instant::now());
            }
            let since_first = first_seen.map(|t| t.elapsed().as_millis() as u64);
            if wait_decision(boot.is_some(), data.is_some(), since_first, start.elapsed().as_millis() as u64) != Wait::Keep {
                break;
            }
            let secs = start.elapsed().as_secs();
            if secs >= 10 && !tried_layout {
                // no label matched in 10 s: look at the layout (a FAT volume with boot/initrd-lean and the partition after it)
                tried_layout = true;
                (boot, data) = fallback_by_layout();
                if boot.is_some() || data.is_some() {
                    break;
                }
            }
            if secs >= 5 && secs / 5 > shown {
                shown = secs / 5;
                screen(&format!("still waiting for the IYAKKI drive ({secs} s)"));
                note(&format!("block devices now: {}", if report.is_empty() { "none".to_string() } else { report.join("; ") }));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        note(&format!("block devices: {}", if report.is_empty() { "none".to_string() } else { report.join("; ") }));
        if boot.is_none() && data.is_none() {
            (boot, data) = fallback_by_layout();
        }
        if boot.is_none() && data.is_none() {
            let why = format!("no IYAKKI-BOOT or IYAKKI-DATA volume appeared in {} s (block devices seen: {})", WAIT_FOR_DEVICES.as_secs(), if report.is_empty() { "none".to_string() } else { report.join("; ") });
            for l in kmsg_tail(120) {
                note(&format!("kernel: {l}"));
            }
            if opts.shell {
                shell(&why);
            }
            fail(&why);
        }
        let t_dev = t0.elapsed().as_millis();
        say(&format!("volumes found at {t_dev} ms (boot: {}, data: {})", boot.as_deref().unwrap_or("none"), data.as_deref().unwrap_or("none")));
        screen("found the drive");
        let mut data_ok = false;
        if let (Some(d), false) = (&data, opts.boot_only) {
            match mnt(d, "/s/data", Some("ext4"), libc::MS_NOSUID | libc::MS_NODEV, "") {
                Ok(()) => {
                    data_ok = true;
                    DATA_UP.store(true, Ordering::SeqCst);
                    say("data volume mounted");
                }
                Err(e) => say(&e),
            }
        }
        let mut boot_ok = false;
        if let Some(b) = &boot {
            match mnt(b, "/s/boot", Some("vfat"), libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "") {
                Ok(()) => {
                    boot_ok = true;
                    BOOT_UP.store(true, Ordering::SeqCst);
                    say("boot volume mounted");
                    // the log goes onto the stick as soon as it can: if anything below hangs, this much is readable afterwards
                    note(&format!("cmdline: {}", std::fs::read_to_string("/proc/cmdline").unwrap_or_default().trim()));
                    flush_log();
                }
                Err(e) => say(&e),
            }
        }
        // 2. the root: updates on the data volume first, then the seed on the boot volume
        let read_ptr = |name: &str| if data_ok { std::fs::read_to_string(format!("/s/data/{ROOTS_DIR}/{name}")).ok().and_then(|t| Pointer::parse(&t)) } else { None };
        let (cur, prev) = (read_ptr("current"), read_ptr("previous"));
        let kind_of = |base: &str| {
            let b = Path::new("/s/data").join(base);
            if b.join("tree").is_dir() { Some(Kind::Tree) } else if b.join("root.sfs").is_file() { Some(Kind::Image) } else { None }
        };
        let list = candidates(cur.as_ref(), prev.as_ref(), &kind_of, boot_ok);
        let mut chosen: Option<Candidate> = None;
        for cand in list {
            match present(&cand) {
                Ok(()) => {
                    chosen = Some(cand);
                    break;
                }
                Err(e) => say(&format!("skipping {}: {e}", cand.path)),
            }
        }
        let Some(chosen) = chosen else {
            let why = &format!(
                "no usable system root on the drive (boot volume {}, data volume {}; no iyakki/root/current tree on the data volume, and no seed image)",
                if boot_ok { "mounted" } else { "not found or not mountable" },
                if data_ok { "mounted" } else { "not found or not mountable" }
            );
            for l in kmsg_tail(120) {
                note(&format!("kernel: {l}"));
            }
            if opts.shell {
                shell(why);
            }
            fail(why);
        };
        let t_root = t0.elapsed().as_millis();
        say(&format!("root {} mounted at {t_root} ms", chosen.path));
        screen("starting IYAKKI...");
        // 3. writable overlay: upper layer on tmpfs (swappable)
        if let Err(e) = mnt("tmpfs", "/upper", Some("tmpfs"), 0, "mode=0755") {
            fail(&e);
        }
        mkdirs("/upper/u");
        mkdirs("/upper/w");
        // patch layers (iyakki dev patch) go over the root when they are whole; any trouble leaves them out, never the boot
        let patches = if data_ok { patch_layers(chosen.path.as_str()) } else { Vec::new() };
        let mut mounted = false;
        if !patches.is_empty() {
            match mnt("overlay", "/newroot", Some("overlay"), 0, &overlay_options_patched(&patches, "/lower", "/upper/u", "/upper/w")) {
                Ok(()) => {
                    mounted = true;
                    say(&format!("{} patch layer(s) over the root", patches.len()));
                }
                Err(e) => {
                    say(&format!("patch layers could not be mounted ({e}); starting without them"));
                    let _ = std::fs::write(format!("/s/data/{PATCHES_DIR}/last-failure"), format!("overlay with patch layers failed: {e}\n"));
                }
            }
        }
        if !mounted {
            if let Err(e) = mnt("overlay", "/newroot", Some("overlay"), 0, &overlay_options("/lower", "/upper/u", "/upper/w")) {
                fail(&e);
            }
        }
        for l in kmsg_tail(80) {
            note(&format!("kernel: {l}"));
        }
        note("outcome so far: root chosen, switching to the system (if the log ends here, look at /var/lib/iyakki/boot-lean.log)");
        flush_log();
        // handles for the log helper (opened now: the mounts move below, the handles stay valid)
        let log_dir = if data_ok { std::fs::File::open("/s/data").ok() } else { None };
        let log_kmsg = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/kmsg").ok();
        // the volume the root is read from stays mounted inside the new root:
        // - the data volume goes to its place (/var/lib/iyakki; the system sees it is mounted and skips its own search)
        // - the boot volume goes to /.stage1/boot (a read-only mount; the update code finds it in the mount table and remounts it)
        lean_marker(None);
        let moves = [(Volume::Boot, "/newroot/.stage1/boot"), (Volume::Data, "/newroot/var/lib/iyakki")];
        for (v, to) in moves {
            let from = volume_dir(v);
            if v == chosen.volume {
                mkdirs(to);
                if let Err(e) = mnt(from, to, None, libc::MS_MOVE, "") {
                    say(&e);
                }
            } else {
                let p = c(from);
                // SAFETY: valid NUL-terminated path; a failure only leaves the mount where it is.
                unsafe { libc::umount2(p.as_ptr(), libc::MNT_DETACH) };
            }
        }
        say(&format!(
            "root: {} ({}) on the {} volume; volumes found in {t_dev} ms, root presented in {} ms, overlay and switch at {} ms",
            chosen.path,
            if chosen.kind == Kind::Image { "image" } else { "extracted tree" },
            if chosen.volume == Volume::Data { "data" } else { "boot" },
            t_root - t_dev,
            t0.elapsed().as_millis()
        ));
        say("switching root");
        // 4. switch: /dev moves along, /proc and /sys are mounted again by the system itself (it must find /proc empty to know it is PID 1)
        let _ = mnt("/dev", "/newroot/dev", None, libc::MS_MOVE, "");
        for p in ["/proc", "/sys"] {
            let cp = c(p);
            // SAFETY: valid NUL-terminated path.
            unsafe { libc::umount2(cp.as_ptr(), libc::MNT_DETACH) };
        }
        free_old_root();
        let nr = c("/newroot");
        let dot = c(".");
        let slash = c("/");
        // SAFETY: the standard switch_root sequence on valid paths; every step is checked.
        unsafe {
            if libc::chdir(nr.as_ptr()) != 0 {
                fail("chdir /newroot failed");
            }
            if libc::mount(dot.as_ptr(), slash.as_ptr(), std::ptr::null(), libc::MS_MOVE, std::ptr::null()) != 0 {
                fail(&format!("moving the new root failed: {}", std::io::Error::last_os_error()));
            }
            if libc::chroot(dot.as_ptr()) != 0 {
                fail("chroot failed");
            }
            libc::chdir(slash.as_ptr());
        }
        redirect_console();
        if let (Some(d), Some(k)) = (&log_dir, &log_kmsg) {
            start_log_child(d.as_raw_fd(), k.as_raw_fd());
        }
        let init = c("/sbin/init");
        let args = [c("/sbin/init")];
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());
        // SAFETY: argv is NULL-terminated; execv only returns on failure.
        unsafe { libc::execv(init.as_ptr(), argv.as_ptr()) };
        fail(&format!("cannot start /sbin/init: {}", std::io::Error::last_os_error()));
    }

}
