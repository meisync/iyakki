//! A copy of the kernel log on the data volume during a lean boot (docs/BOOT-SPEED.md), so a start that ends in a blank screen can be read
//! afterwards by plugging the stick into another computer: `iyakki/boot-lean.log` on the data volume (`/var/lib/iyakki/boot-lean.log`).
//! The first stage writes its own log to `boot/stage1.log` on the boot volume; this one starts where that one ends.
//! The file is rewritten every few seconds for the first two minutes (the whole log so far, at most 256 KB, newest kept), so it shows how far
//! IYAKKI got even when it never reaches the sign-in screen. Failures are silent: a log must never stop the boot.

use std::io::Read;
use std::path::PathBuf;

/// Largest copy kept.
pub const MAX_BYTES: usize = 256 * 1024;

/// One `/dev/kmsg` record (`pri,seq,usec,flags;text`) as `[ s.uuuuuu] text`; continuation lines are dropped.
pub fn format_record(rec: &str) -> Option<String> {
    let first = rec.lines().next()?;
    let (head, text) = first.split_once(';')?;
    let usec: u64 = head.split(',').nth(2)?.parse().ok()?;
    Some(format!("[{:>5}.{:06}] {}", usec / 1_000_000, usec % 1_000_000, text))
}

/// The newest `max` bytes of `text`, starting at a line.
pub fn keep_tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut cut = text.len() - max;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    match text[cut..].find('\n') {
        Some(i) => &text[cut + i + 1..],
        None => &text[cut..],
    }
}

/// The kernel log as text (non-blocking read of /dev/kmsg from its start).
#[cfg(target_os = "linux")]
pub fn read_kmsg() -> String {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/kmsg") else { return String::new() };
    let mut out = String::new();
    let mut buf = vec![0u8; 8192];
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            break;
        }
        if let Some(l) = format_record(&String::from_utf8_lossy(&buf[..n])) {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub fn read_kmsg() -> String {
    String::new()
}

fn write_copy(path: &std::path::Path) {
    use std::io::Write;
    let text = read_kmsg();
    let text = keep_tail(&text, MAX_BYTES);
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path) {
        let _ = f.write_all(text.as_bytes());
        let _ = f.sync_all();
    }
}

/// Start the copy (lean boot only): now, then every 2 s for 12 s, then every 5 s up to two minutes.
pub fn start(path: PathBuf) {
    let _ = std::thread::Builder::new().name("iyakki-bootlog".into()).spawn(move || {
        write_copy(&path);
        for i in 0..26u64 {
            std::thread::sleep(std::time::Duration::from_secs(if i < 6 { 2 } else { 5 }));
            write_copy(&path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_become_log_lines_and_the_copy_stays_bounded() {
        assert_eq!(format_record("6,12,1500000,-;usb 2-1: new SuperSpeed USB device\n SUBSYSTEM=usb\n").as_deref(), Some("[    1.500000] usb 2-1: new SuperSpeed USB device"));
        assert_eq!(format_record("nonsense"), None);
        let t = "aaaa\nbbbb\ncccc\n";
        assert_eq!(keep_tail(t, 100), t);
        assert_eq!(keep_tail(t, 8), "cccc\n");
        assert!(keep_tail(&t.repeat(1000), MAX_BYTES).len() <= MAX_BYTES);
    }

    #[test]
    fn the_lean_log_starts_only_when_the_first_stage_mounted_the_data_volume() {
        let src = include_str!("boot.rs");
        let body = src.split("pub fn mount_data_volume").nth(1).unwrap().split("let roots = iyakki_storage::Roots::real();").next().unwrap();
        assert!(body.contains("crate::bootlog::start("), "started in the lean branch");
        assert!(!src.split("let roots = iyakki_storage::Roots::real();").nth(1).unwrap().split("pub fn dir_is_mount_point").next().unwrap().contains("bootlog"), "not in the normal path");
    }
}
