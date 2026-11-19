//! The OpenVPN backend of the VPN manager (F-VPN-06). The OpenVPN program is never shipped: the person downloads it by their own
//! click (`clis`, entry `openvpn`, licence text shown first) and this module only runs it, with the fixed argument list of
//! `iyakki_vpn::ovpnrun`, as a child of the agent. The program never applies an address or a route and never runs a script: it
//! creates the tunnel interface and reports what the server pushed; IYAKKI reads that report, runs it through the same route and
//! DNS checks as a WireGuard profile and applies it itself through `Sys`.
//!
//! The process is behind two traits so every test here runs with a scripted fake: [`OvpnProc`] (one running program) and
//! [`OvpnLauncher`] (starts one). [`RealLauncher`] is the real one.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use iyakki_vpn::net::{Cidr, Context};
use iyakki_vpn::ovpnrun::{self, Checked, Event, Pushed};

/// Seconds (one per idle turn) to wait for the tunnel to come up before giving up.
pub const UP_WAIT_TURNS: u32 = 90;
/// Log lines read before the program is judged to be talking without making progress.
pub const MAX_LINES: u32 = 20_000;

/// What a running program gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Text(String),
    /// Nothing arrived in about a second.
    Idle,
    /// The program ended and its output is read to the end.
    Closed,
}

pub trait OvpnProc: Send {
    fn next(&mut self) -> Line;
    fn alive(&mut self) -> bool;
    /// End the program (SIGTERM, then kill) and remove its config file.
    fn stop(&mut self);
}

pub trait OvpnLauncher: Send + Sync {
    /// Is the program there (downloaded by the person)? The text is the plain-words reason when it is not.
    fn available(&self) -> Result<(), String>;
    /// Start it with the profile text (secret blocks included) and the tunnel interface name.
    fn launch(&self, config: &str, ifname: &str) -> Result<Box<dyn OvpnProc>, String>;
}

/// Why the tunnel did not come up, and whether trying again would be pointless.
#[derive(Debug, PartialEq, Eq)]
pub struct UpError {
    pub msg: String,
    pub hard: bool,
}

fn fail(msg: &str, hard: bool) -> UpError {
    UpError { msg: msg.into(), hard }
}

/// Read the program's log until the tunnel is up. Returns what the server pushed, checked. Refuses (hard) a bad password or
/// certificate (retrying with the same ones only gets the account locked), (soft) a server that did not answer in time.
pub fn await_up(p: &mut dyn OvpnProc, profile_routes: &[Cidr], profile_redirect: bool, full: bool, ctx: &Context) -> Result<Checked, UpError> {
    let mut pushed: Option<Pushed> = None;
    let (mut idle, mut lines) = (0u32, 0u32);
    loop {
        match p.next() {
            Line::Idle => {
                idle += 1;
                if idle >= UP_WAIT_TURNS {
                    return Err(fail("no answer from the OpenVPN server: check the server address and that the port is not blocked", false));
                }
                if !p.alive() {
                    return Err(fail("the OpenVPN program ended before the tunnel was up", false));
                }
            }
            Line::Closed => return Err(fail("the OpenVPN program ended before the tunnel was up", false)),
            Line::Text(t) => {
                lines += 1;
                if lines > MAX_LINES {
                    return Err(fail("the OpenVPN program talked a long time without bringing the tunnel up", false));
                }
                match ovpnrun::classify_line(&t) {
                    Event::Push(text) => match ovpnrun::parse_push_reply(&text) {
                        Ok(v) => pushed = Some(v),
                        Err(e) => return Err(fail(&format!("the server's settings were refused: {e}"), true)),
                    },
                    Event::Up => {
                        let Some(v) = pushed else { return Err(fail("the tunnel came up but the server's settings were not seen (the program's log level is too low)", true)) };
                        return ovpnrun::check_pushed(&v, profile_routes, profile_redirect, full, ctx).map_err(|e| fail(&format!("the server's settings were refused: {e}"), true));
                    }
                    Event::AuthFailed => return Err(fail("the OpenVPN server refused the user name or password", true)),
                    Event::TlsFailed => return Err(fail("the secure handshake with the OpenVPN server failed: check the certificates in the profile", false)),
                    Event::Ended => return Err(fail("the OpenVPN program gave up", false)),
                    Event::Restarting | Event::Other => {}
                }
            }
        }
    }
}

// ------------------------------------------------------------------ the real program

/// Starts the downloaded program. `binary` is where the click put it; `run_dir` is a private folder (tmpfs on the image) for the
/// config file, which holds secrets, is written 0600 and removed when the tunnel is up or the program stops.
pub struct RealLauncher {
    pub binary: PathBuf,
    pub run_dir: PathBuf,
}

impl OvpnLauncher for RealLauncher {
    fn available(&self) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(&self.binary) {
            Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 => Ok(()),
            _ => Err("OpenVPN profiles need the OpenVPN program, which you download yourself (Settings -> Downloads, OpenVPN). It is not downloaded.".into()),
        }
    }

    fn launch(&self, config: &str, ifname: &str) -> Result<Box<dyn OvpnProc>, String> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        self.available()?;
        std::fs::create_dir_all(&self.run_dir).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(&self.run_dir, std::fs::Permissions::from_mode(0o700));
        let path = self.run_dir.join(format!("ovpn-{}.conf", std::process::id()));
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
            f.write_all(config.as_bytes()).map_err(|e| e.to_string())?;
        }
        let args = match iyakki_vpn::ovpnrun::command_args(&path, ifname) {
            Ok(a) => a,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(e.to_string());
            }
        };
        let mut child = match std::process::Command::new(&self.binary)
            .args(&args)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!("could not start the OpenVPN program: {e}"));
            }
        };
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        for out in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let (tx, path) = (tx.clone(), path.clone());
            std::thread::spawn(move || {
                for l in BufReader::new(out).lines().map_while(Result::ok) {
                    if l.contains("Initialization Sequence Completed") {
                        let _ = std::fs::remove_file(&path);
                    }
                    if tx.send(l).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Box::new(RealProc { child, rx, path }))
    }
}

struct RealProc {
    child: std::process::Child,
    rx: Receiver<String>,
    path: PathBuf,
}

impl OvpnProc for RealProc {
    fn next(&mut self) -> Line {
        match self.rx.recv_timeout(Duration::from_secs(1)) {
            Ok(l) => Line::Text(l),
            Err(RecvTimeoutError::Timeout) => Line::Idle,
            Err(RecvTimeoutError::Disconnected) => Line::Closed,
        }
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stop(&mut self) {
        if self.alive() {
            // the program ends its tunnel cleanly on SIGTERM; `kill` is the last resort
            let _ = std::process::Command::new("kill").args(["-TERM", &self.child.id().to_string()]).status();
            for _ in 0..50 {
                if !self.alive() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for RealProc {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
pub mod fakes {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A scripted program: its lines in order, then Closed (or Idle forever when `hang`).
    pub struct ScriptProc {
        pub lines: VecDeque<String>,
        pub hang: bool,
        pub up: bool,
        pub stopped: Arc<Mutex<bool>>,
    }

    impl OvpnProc for ScriptProc {
        fn next(&mut self) -> Line {
            match self.lines.pop_front() {
                Some(l) => Line::Text(l),
                None if self.hang => Line::Idle,
                None => Line::Closed,
            }
        }
        fn alive(&mut self) -> bool {
            self.up && !*self.stopped.lock().unwrap()
        }
        fn stop(&mut self) {
            *self.stopped.lock().unwrap() = true;
        }
    }

    pub struct ScriptLauncher {
        pub lines: Vec<String>,
        pub present: bool,
        pub hang: bool,
        pub stopped: Arc<Mutex<bool>>,
        /// The config texts it was started with (so a test can check what reached the program).
        pub configs: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl ScriptLauncher {
        pub fn new(lines: &[&str]) -> ScriptLauncher {
            ScriptLauncher { lines: lines.iter().map(|s| s.to_string()).collect(), present: true, hang: false, stopped: Arc::new(Mutex::new(false)), configs: Arc::new(Mutex::new(Vec::new())) }
        }
    }

    impl OvpnLauncher for ScriptLauncher {
        fn available(&self) -> Result<(), String> {
            if self.present { Ok(()) } else { Err("not downloaded".into()) }
        }
        fn launch(&self, config: &str, ifname: &str) -> Result<Box<dyn OvpnProc>, String> {
            self.available()?;
            self.configs.lock().unwrap().push((config.to_string(), ifname.to_string()));
            Ok(Box::new(ScriptProc { lines: self.lines.iter().cloned().collect(), hang: self.hang, up: true, stopped: self.stopped.clone() }))
        }
    }

    pub const PUSH: &str = "PUSH: Received control message: 'PUSH_REPLY,route 10.8.0.0 255.255.255.0,dhcp-option DNS 10.8.0.1,redirect-gateway def1,ifconfig 10.8.0.6 255.255.255.0'";
    pub const UP: &str = "Initialization Sequence Completed";
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;

    fn run(lines: &[&str], hang: bool) -> Result<Checked, UpError> {
        let mut l = ScriptLauncher::new(lines);
        l.hang = hang;
        let mut p = l.launch("client\n", "iyvpn0").unwrap();
        await_up(&mut *p, &[], false, true, &Context::default())
    }

    #[test]
    fn a_normal_start_gives_the_checked_settings() {
        let c = run(&["OpenVPN 2.6 starting", "TLS: Initial packet from [AF_INET]x", PUSH, UP], false).unwrap();
        assert_eq!(c.addresses, vec![Cidr::parse("10.8.0.6/24").unwrap()]);
        assert!(c.allowed.iter().any(Cidr::is_default) && c.dns.len() == 1);
    }

    #[test]
    fn every_way_to_fail_is_named_and_a_bad_password_is_not_retried() {
        let e = run(&["AUTH: Received control message: AUTH_FAILED"], false).unwrap_err();
        assert!(e.hard && e.msg.contains("user name or password"));
        let e = run(&["VERIFY ERROR: depth=0, error=certificate has expired"], false).unwrap_err();
        assert!(!e.hard && e.msg.contains("certificates"));
        let e = run(&["Exiting due to fatal error"], false).unwrap_err();
        assert!(e.msg.contains("gave up"));
        let e = run(&["hello"], false).unwrap_err();
        assert!(e.msg.contains("ended"), "{e:?}");
        let e = run(&[], true).unwrap_err();
        assert!(!e.hard && e.msg.contains("no answer"), "an idle program is given up on after the bounded number of turns");
        let e = run(&[UP], false).unwrap_err();
        assert!(e.hard && e.msg.contains("not seen"), "up without the pushed settings is refused");
        let e = run(&["PUSH: Received control message: 'PUSH_REPLY,route 10.0.0.0 255.0.255.0'", UP], false).unwrap_err();
        assert!(e.hard && e.msg.contains("refused"));
    }

    #[test]
    fn the_real_launcher_reports_a_missing_program_in_plain_words_and_runs_one_with_a_closed_environment() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::testdir::outside_project("ovpn", "real");
        let l = RealLauncher { binary: d.join("openvpn"), run_dir: d.join("run") };
        assert!(l.available().unwrap_err().contains("download yourself"));
        assert!(l.launch("client\n", "iyvpn0").is_err());
        // a stand-in program that prints what the real one prints and shows its arguments and environment
        std::fs::write(&l.binary, "#!/bin/sh\nfor a in \"$@\"; do echo \"arg $a\"; done\necho \"env $(env | wc -l)\"\necho \"PUSH: Received control message: 'PUSH_REPLY,ifconfig 10.8.0.2 255.255.255.0,route 10.9.0.0 255.255.0.0'\"\necho 'Initialization Sequence Completed'\n").unwrap();
        std::fs::set_permissions(&l.binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut p = l.launch("client\nremote x 1194\n", "iyvpn0").unwrap();
        let mut seen = Vec::new();
        let up = loop {
            match p.next() {
                Line::Text(t) => {
                    if matches!(ovpnrun::classify_line(&t), Event::Up) {
                        break true;
                    }
                    seen.push(t);
                }
                Line::Closed => break false,
                Line::Idle => {}
            }
        };
        assert!(up);
        assert!(seen.iter().any(|l| l == "arg --script-security") && seen.iter().any(|l| l == "arg 0"));
        assert!(seen.iter().any(|l| l.starts_with("env ") && l.trim_start_matches("env ").trim().parse::<u32>().unwrap() <= 3), "the environment is closed: {seen:?}");
        assert!(std::fs::read_dir(d.join("run")).unwrap().next().is_none(), "the config file with the secrets is removed once the tunnel is up");
        p.stop();
    }
}
