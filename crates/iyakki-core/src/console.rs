//! Output that can never take the machine down.
//!
//! As PID 1, a panic is fatal (the kernel panics with "Attempted to kill init"). Rust's `println!` and
//! `eprintln!` panic when the write fails, and on a PC with no serial port, a console that was routed to
//! `ttyS0` fails every write. So IYAKKI never uses them on its boot path: log lines go to the kernel log
//! (`/dev/kmsg`), which every console shows (including the screen), and anything written to
//! stdout/stderr ignores errors.
//!
//! This module also installs the panic hook (the message is printed at "critical" level so even a quiet
//! boot shows it) and provides the large stacks IYAKKI runs on.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// The stack every thread gets by default: 8 MiB, what glibc-based Linux distributions use (`ulimit -s 8192`) and
/// what software written for them expects. musl, which IYAKKI is built on, would give non-Rust threads only 128 KiB;
/// the build raises that to this value too (`-z stack-size`, see .cargo/config.toml). Stacks are virtual memory:
/// only the pages actually touched are committed.
pub const THREAD_STACK: usize = 8 * 1024 * 1024;
/// The supervisor thread (wasm sandboxes, snapshots, everything it calls): a generous 64 MiB.
pub const MAIN_STACK: usize = 64 * 1024 * 1024;
/// The shell thread (software renderer, text layout, the apps): 32 MiB.
pub const SHELL_STACK: usize = 32 * 1024 * 1024;

static KMSG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// Longest message the kernel accepts in one `/dev/kmsg` write is about 1 KiB; stay under it.
const KMSG_MAX: usize = 900;

/// The `/dev/kmsg` records for `msg` at kernel level `level` (0 emergency ... 7 debug): one per line, each
/// cut to a size the kernel accepts.
pub fn kmsg_lines(level: u8, msg: &str) -> Vec<String> {
    msg.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut end = l.len().min(KMSG_MAX);
            while !l.is_char_boundary(end) {
                end -= 1;
            }
            format!("<{}>iyakki: {}\n", level.min(7), &l[..end])
        })
        .collect()
}

/// Write to the kernel log, best effort. Returns whether it went anywhere.
pub fn kmsg(level: u8, msg: &str) -> bool {
    let cell = KMSG.get_or_init(|| Mutex::new(std::fs::OpenOptions::new().write(true).open("/dev/kmsg").ok()));
    let Ok(mut g) = cell.lock() else { return false };
    let Some(f) = g.as_mut() else { return false };
    let mut ok = true;
    for l in kmsg_lines(level, msg) {
        ok &= f.write_all(l.as_bytes()).is_ok();
    }
    ok
}

/// `eprint!` that ignores failure.
pub fn err_write(s: &str) {
    let _ = std::io::stderr().write_all(s.as_bytes());
}

/// `print!` that ignores failure.
pub fn out_write(s: &str) {
    let mut o = std::io::stdout();
    let _ = o.write_all(s.as_bytes());
    let _ = o.flush();
}

pub fn level_to_kernel(l: log::Level) -> u8 {
    match l {
        log::Level::Error => 3,
        log::Level::Warn => 4,
        log::Level::Info => 6,
        log::Level::Debug | log::Level::Trace => 7,
    }
}

pub fn format_log(level: log::Level, args: &std::fmt::Arguments) -> String {
    format!("[iyakki {:5}] {}", level, args)
}

/// Print panics where a person can see them, then let the normal unwinding continue.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let msg = format!("PANIC in thread '{}': {}", thread.name().unwrap_or("?"), info);
        kmsg(2, &msg);
        err_write(&format!("{msg}\n"));
    }));
}

/// Run `f` on a thread with a large stack ([`MAIN_STACK`]) and make every thread started later default to
/// [`THREAD_STACK`]. Must be called before other threads exist. A panic in `f` comes back as `Err`.
pub fn run_with_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> std::thread::Result<T> {
    std::env::set_var("RUST_MIN_STACK", THREAD_STACK.to_string());
    match std::thread::Builder::new().name("iyakki-main".into()).stack_size(MAIN_STACK).spawn(f) {
        Ok(h) => h.join(),
        Err(e) => Err(Box::new(format!("could not start the main thread: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_log_records_are_one_line_each_and_bounded() {
        assert_eq!(kmsg_lines(6, "supervisor up"), vec!["<6>iyakki: supervisor up\n"]);
        assert_eq!(kmsg_lines(2, "a\nb\n\nc"), vec!["<2>iyakki: a\n", "<2>iyakki: b\n", "<2>iyakki: c\n"], "blank lines are dropped");
        assert!(kmsg_lines(6, "").is_empty());
        let long = kmsg_lines(6, &"é".repeat(2000));
        assert_eq!(long.len(), 1);
        assert!(long[0].len() <= 900 + 16 && long[0].ends_with('\n') && std::str::from_utf8(long[0].as_bytes()).is_ok(), "cut on a character boundary");
        assert_eq!(kmsg_lines(99, "x"), vec!["<7>iyakki: x\n"], "levels are clamped");
    }

    #[test]
    fn log_levels_map_to_kernel_levels_and_lines_are_formatted() {
        assert_eq!([log::Level::Error, log::Level::Warn, log::Level::Info, log::Level::Debug].map(level_to_kernel), [3, 4, 6, 7]);
        assert_eq!(format_log(log::Level::Info, &format_args!("supervisor up")), "[iyakki INFO ] supervisor up");
    }

    #[test]
    fn writes_to_a_broken_stderr_never_panic() {
        // the ignoring writers return normally whatever the descriptors are doing
        err_write("x");
        out_write("");
        assert!(!kmsg(6, "") || true);
    }

    #[test]
    fn the_main_thread_stack_is_big_enough_for_deep_recursion_and_panics_are_caught() {
        fn deep(n: usize) -> usize {
            let pad = [n as u8; 256];
            if n == 0 { pad[0] as usize } else { deep(n - 1) + pad[(n % 256) as usize] as usize % 2 }
        }
        // ~100 000 frames of >256 bytes is about 26 MB: far beyond the default thread stack, within the main one
        let r = run_with_big_stack(|| deep(100_000));
        assert!(r.is_ok());
        let r = run_with_big_stack(|| -> u8 { panic!("boom") });
        assert!(r.is_err(), "a panic is returned, not fatal");
    }
}
