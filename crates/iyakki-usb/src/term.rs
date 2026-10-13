//! The raw-terminal glue: single key presses without echo, and the terminal width.

use std::io::Read;

/// While alive the terminal delivers key presses immediately and without echo; restored on drop.
pub struct Raw {
    saved: libc::termios,
}

impl Raw {
    pub fn enter() -> Option<Raw> {
        // SAFETY: tcgetattr/tcsetattr on stdin with a zero-initialised termios that tcgetattr fills.
        unsafe {
            if libc::isatty(0) == 0 {
                return None;
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return None;
            }
            print!("\x1b[?25l"); // hide the cursor
            Some(Raw { saved })
        }
    }

    /// Wait for a key press and return the bytes it sent (an arrow key sends three).
    pub fn read_bytes(&self) -> Vec<u8> {
        let mut buf = [0u8; 16];
        let n = std::io::stdin().read(&mut buf).unwrap_or(0);
        let mut out = buf[..n].to_vec();
        // an Escape on its own, or the start of an arrow sequence: see whether more bytes follow
        if out == [0x1b] {
            let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the duration of the call.
            if unsafe { libc::poll(&mut fds, 1, 40) } > 0 {
                let m = std::io::stdin().read(&mut buf).unwrap_or(0);
                out.extend_from_slice(&buf[..m]);
            }
        }
        out
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: restoring the attributes saved in `enter`.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.saved) };
        print!("\x1b[?25h");
    }
}

pub fn width() -> usize {
    // SAFETY: TIOCGWINSZ fills a winsize.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 20 { ws.ws_col as usize } else { 80 }
    }
}

pub fn is_tty() -> bool {
    // SAFETY: isatty takes a file descriptor.
    unsafe { libc::isatty(0) != 0 && libc::isatty(1) != 0 }
}
