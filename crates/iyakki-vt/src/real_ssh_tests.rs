//! The terminal emulator and the pseudo-terminal session with a REAL OpenSSH client and a REAL OpenSSH `sshd` (ignored by default).
//!
//! `ssh -tt` runs on a pseudo-terminal of this crate, against an `sshd` started as the normal user on a free 127.0.0.1 port (own host key, own `authorized_keys`, key login
//! only; nothing in /etc is read or written). Then the test types into the session like the CLI tile does: a command, a resize, `vi` writing a file, `top` on the alternate
//! screen, Ctrl+C, and `exit`. Needs `ssh`, `sshd`, `ssh-keygen`, `vi`, `top`. Run: `cargo test -p iyakki-vt -- --ignored real_ssh` (`dist/linux-host/real-tools-tests.sh`).
//! The server runs commands as the user, so the test only names files inside its own scratch folder. Row: F-CLI-08.

use crate::session::Session;
use crate::{Key, Mods};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

struct Sshd {
    child: Child,
    dir: std::path::PathBuf,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for(s: &mut Session, what: &str, secs: u64) -> bool {
    for _ in 0..secs * 40 {
        s.poll();
        if s.screen.text().contains(what) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn settle(s: &mut Session, ms: u64) {
    for _ in 0..ms / 25 {
        s.poll();
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn type_line(s: &mut Session, line: &str) {
    for c in line.chars() {
        s.key(Key::Char(c), Mods::default());
    }
    s.key(Key::Enter, Mods::default());
}

#[test]
#[ignore]
fn real_ssh_session_runs_commands_resizes_edits_with_vi_shows_top_and_stops_with_ctrl_c() {
    if !["ssh", "sshd", "ssh-keygen", "vi", "top"].iter().all(|t| sh(&format!("command -v {t} >/dev/null")).0) {
        eprintln!("skipped: needs ssh, sshd, ssh-keygen, vi and top");
        return;
    }
    let dir = std::path::PathBuf::from("/tmp").join(format!("ivt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let d = dir.display().to_string();
    let (ok, out) = sh(&format!("cd {d} && ssh-keygen -q -t ed25519 -N '' -f hostkey && ssh-keygen -q -t ed25519 -N '' -f userkey && cp userkey.pub authorized_keys"));
    assert!(ok, "{out}");
    let conf = format!("Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/hostkey\nPidFile {d}/sshd.pid\nAuthorizedKeysFile {d}/authorized_keys\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nStrictModes no\nPrintMotd no\n");
    std::fs::write(dir.join("sshd_config"), conf).unwrap();
    let child = Command::new("/usr/sbin/sshd").args(["-D", "-e", "-f"]).arg(dir.join("sshd_config")).stdout(Stdio::null()).stderr(Stdio::from(std::fs::File::create(dir.join("sshd.log")).unwrap())).spawn().expect("sshd");
    let _guard = Sshd { child, dir: dir.clone() };
    assert!((0..100).any(|_| TcpStream::connect(("127.0.0.1", port)).is_ok() || { std::thread::sleep(Duration::from_millis(50)); false }), "sshd did not start");

    let user = String::from_utf8_lossy(&Command::new("id").arg("-un").output().unwrap().stdout).trim().to_string();
    let home = std::env::var("HOME").unwrap_or_default(); // home-ok: the real ssh client only needs a HOME to start; nothing is read from it or written to it
    let env = vec![("TERM".into(), "xterm".into()), ("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), home)];
    let args: Vec<String> = ["-tt", "-p", &port.to_string(), "-i", &format!("{d}/userkey"), "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=no", "-o", &format!("UserKnownHostsFile={d}/known_hosts"), "-o", "PasswordAuthentication=no", &format!("{user}@127.0.0.1")]
        .iter().map(|s| s.to_string()).collect();
    let mut s = Session::start("/usr/bin/ssh", &args, &env, 80, 24).unwrap();
    // the login shell of the server: a command whose answer is not in the line that was typed
    type_line(&mut s, "echo sum-$((6*7))-ok");
    assert!(wait_for(&mut s, "sum-42-ok", 20), "no shell prompt or answer through ssh:\n{}\nsshd: {}", s.screen.text(), std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
    // the window size reaches the remote terminal, also after a resize
    type_line(&mut s, "stty size");
    assert!(wait_for(&mut s, "24 80", 10), "{}", s.screen.text());
    s.resize(100, 30);
    settle(&mut s, 300);
    type_line(&mut s, "stty size");
    assert!(wait_for(&mut s, "30 100", 10), "the remote side did not get the new size:\n{}", s.screen.text());
    // vi on the alternate screen writes a file, and the shell screen comes back
    let file = dir.join("vi-out.txt");
    type_line(&mut s, &format!("cd {} && vi vi-out.txt", dir.display()));
    settle(&mut s, 2500);
    assert!(s.screen.text().contains("vi-out.txt"), "vi did not open:\n{}", s.screen.text());
    for c in "ihello from vi over ssh".chars() {
        s.key(Key::Char(c), Mods::default());
    }
    s.key(Key::Esc, Mods::default());
    for c in ":wq".chars() {
        s.key(Key::Char(c), Mods::default());
    }
    s.key(Key::Enter, Mods::default());
    settle(&mut s, 2000); // vi leaves, the shell screen comes back; keys typed meanwhile would be eaten
    type_line(&mut s, "echo vi-$(wc -c < vi-out.txt)-bytes");
    assert!(wait_for(&mut s, "vi-23-bytes", 15), "vi did not write the file (or the shell did not come back):\n{}", s.screen.text());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello from vi over ssh\n");
    // top draws on the alternate screen, `q` leaves it
    type_line(&mut s, "top");
    assert!(wait_for(&mut s, "load average", 15), "top did not draw:\n{}", s.screen.text());
    settle(&mut s, 1500);
    s.key(Key::Char('q'), Mods::default());
    settle(&mut s, 1500);
    assert!(!s.screen.text().contains("load average"), "q did not leave top:\n{}", s.screen.text());
    type_line(&mut s, "echo after-top-ok");
    assert!(wait_for(&mut s, "after-top-ok", 10), "{}", s.screen.text());
    // Ctrl+C stops a running remote program, the session lives on
    type_line(&mut s, "sleep 100");
    settle(&mut s, 500);
    s.key(Key::Char('c'), Mods { ctrl: true, ..Mods::default() });
    type_line(&mut s, "echo after-ctrl-c-ok");
    assert!(wait_for(&mut s, "after-ctrl-c-ok", 10), "{}", s.screen.text());
    // exit ends the session with the remote shell's status
    type_line(&mut s, "exit 3");
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        s.poll();
        if s.finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(s.finished(), "{}", s.screen.text());
    assert_eq!(s.exit_code(), Some(3));
}
