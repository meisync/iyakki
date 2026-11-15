//! The computer side of `iyakki dev shell --tty` (F-DEV-16): the keyboard and the screen of this terminal carried to a terminal command on
//! the device. The terminal goes into raw mode (every key, Ctrl+C included, is sent as a byte and the device's terminal acts on it), the
//! output is written as it comes, the window size is sent when it changes, and **Ctrl+]** ends the session on this side, as in telnet, in
//! case the device does not answer. The terminal settings are put back when the session ends, also after an error.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::client::DevClient;

/// Ctrl+]: ends the session on this side.
pub const ESCAPE: u8 = 0x1d;

/// What to forward of the keys just read, and whether the escape key was among them (what follows it is dropped).
pub fn split_escape(buf: &[u8]) -> (&[u8], bool) {
    match buf.iter().position(|b| *b == ESCAPE) {
        Some(i) => (&buf[..i], true),
        None => (buf, false),
    }
}

/// The query of the exec request for a terminal.
pub fn exec_query(sid: u64, rows: u16, cols: u16, term: &str) -> Vec<(String, String)> {
    vec![("sid".into(), sid.to_string()), ("tty".into(), "1".into()), ("rows".into(), rows.to_string()), ("cols".into(), cols.to_string()), ("term".into(), term.to_string())]
}

/// Rows and columns of the terminal on `fd`, if it is one.
pub fn window_size(fd: i32) -> Option<(u16, u16)> {
    let mut ws = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes one `winsize` struct through the pointer, which lives for the call.
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut ws) } == 0;
    (ok && ws.ws_row > 0 && ws.ws_col > 0).then_some((ws.ws_row, ws.ws_col))
}

/// This terminal in raw mode until the value is dropped.
pub struct RawMode {
    fd: i32,
    saved: libc::termios,
}

impl RawMode {
    pub fn enter(fd: i32) -> Result<RawMode, String> {
        // SAFETY: isatty(3) only inspects the descriptor.
        if unsafe { libc::isatty(fd) } != 1 {
            return Err("--tty needs a terminal: this program's input is not one".into());
        }
        // SAFETY: tcgetattr fills the struct we pass; an all-zero termios is a valid value to be overwritten.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return Err("could not read the terminal settings".into());
        }
        let mut raw = saved;
        // SAFETY: cfmakeraw edits the struct we own; tcsetattr applies it to the descriptor.
        unsafe {
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err("could not switch the terminal to raw mode".into());
            }
        }
        Ok(RawMode { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: puts back the settings read in `enter` on the same descriptor.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

/// Wait up to `ms` for input on `fd`.
fn readable(fd: i32, ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: poll reads and writes one `pollfd` that lives for the call.
    unsafe { libc::poll(&mut p, 1, ms) > 0 && p.revents & libc::POLLIN != 0 }
}

/// Run `line` (empty: an interactive shell) on a terminal of the device in session `sid`, until it ends. Returns its exit status.
pub fn run(c: &DevClient, sid: u64, line: &str, note: &mut dyn FnMut(&str)) -> Result<i32, String> {
    let (rows, cols) = window_size(0).unwrap_or((24, 80));
    let term = std::env::var("TERM").unwrap_or_default();
    let q = exec_query(sid, rows, cols, &term);
    let qr: Vec<(&str, &str)> = q.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let id = c.call("POST", "/v1/dev/shell/exec", &qr, line.as_bytes())?["id"].as_u64().ok_or("the device gave no command number")?;
    let (sid_s, id_s) = (sid.to_string(), id.to_string());
    let raw = RawMode::enter(0);
    let raw = match raw {
        Ok(r) => r,
        Err(e) => {
            let _ = c.call("POST", "/v1/dev/shell/signal", &[("sid", &sid_s), ("id", &id_s), ("sig", "kill")], b"");
            return Err(e);
        }
    };
    note("Connected to a terminal on the device. Ctrl+] ends the session on this side.\r");
    let stop = Arc::new(AtomicBool::new(false));
    let input = {
        let (c, stop, sid_s, id_s) = (c.clone(), stop.clone(), sid_s.clone(), id_s.clone());
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 1024];
            while !stop.load(Ordering::SeqCst) {
                if !readable(0, 200) {
                    continue;
                }
                let n = match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let (keys, quit) = split_escape(&buf[..n]);
                if !keys.is_empty() && c.call("POST", "/v1/dev/shell/stdin", &[("sid", &sid_s), ("id", &id_s)], keys).is_err() {
                    break;
                }
                if quit {
                    stop.store(true, Ordering::SeqCst);
                    break;
                }
            }
            stop.store(true, Ordering::SeqCst);
        })
    };
    let (mut since, mut size, mut code, mut ended) = (0u64, (rows, cols), 0, false);
    let mut stdout = std::io::stdout();
    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        let since_s = since.to_string();
        match c.call("GET", "/v1/dev/shell/read", &[("sid", &sid_s), ("id", &id_s), ("since", &since_s), ("wait", "300"), ("enc", "b64")], b"") {
            Ok(v) => {
                if let Some(bytes) = v["b64"].as_str().and_then(crate::sshkey::unb64) {
                    let _ = stdout.write_all(&bytes);
                    let _ = stdout.flush();
                }
                since = v["next"].as_u64().unwrap_or(since);
                if v["done"].as_bool() == Some(true) {
                    code = v["code"].as_i64().unwrap_or(0) as i32;
                    ended = true;
                    break Ok(());
                }
            }
            Err(e) => break Err(e),
        }
        if let Some(now) = window_size(0) {
            if now != size {
                size = now;
                let (r, k) = (now.0.to_string(), now.1.to_string());
                let _ = c.call("POST", "/v1/dev/shell/resize", &[("sid", &sid_s), ("id", &id_s), ("rows", &r), ("cols", &k)], b"");
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    stop.store(true, Ordering::SeqCst);
    if !ended {
        let _ = c.call("POST", "/v1/dev/shell/signal", &[("sid", &sid_s), ("id", &id_s), ("sig", "kill")], b"");
    }
    let _ = input.join();
    drop(raw);
    result.map(|_| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_right_bracket_ends_the_session_and_drops_what_follows() {
        assert_eq!(split_escape(b"ls\r"), (&b"ls\r"[..], false));
        assert_eq!(split_escape(b"ab\x1dcd"), (&b"ab"[..], true));
        assert_eq!(split_escape(b"\x1d"), (&b""[..], true));
        assert_eq!(split_escape(b""), (&b""[..], false));
        // Ctrl+C (3) and Ctrl+D (4) are ordinary keys here: the device's terminal acts on them
        assert_eq!(split_escape(&[3, 4]), (&[3u8, 4][..], false));
    }

    #[test]
    fn the_exec_query_carries_the_terminal_and_its_size() {
        let q = exec_query(7, 30, 100, "xterm");
        assert_eq!(q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(), ["sid=7", "tty=1", "rows=30", "cols=100", "term=xterm"]);
    }

    #[test]
    fn raw_mode_refuses_a_descriptor_that_is_not_a_terminal() {
        let f = std::fs::File::open("/dev/null").unwrap();
        use std::os::fd::AsRawFd;
        assert!(RawMode::enter(f.as_raw_fd()).err().is_some_and(|e| e.contains("needs a terminal")));
        assert_eq!(window_size(f.as_raw_fd()), None);
    }
}
