//! A local Ollama server, started and stopped from the CLI tile once Ollama has been installed there. It is a background
//! process of IYAKKI listening on this machine only (127.0.0.1), its models kept in the data folder, and when it is ready it
//! is added to Models as a provider so the chat can use it at once. An Ollama that is already running on the port (started by
//! someone else) is used as it is and never stopped from here.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::clis::CliManager;

pub const DEFAULT_PORT: u16 = 11434;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OllamaState {
    Stopped,
    Starting,
    Running {
        /// Started by someone else: used, never stopped from here.
        external: bool,
        version: String,
    },
    Stopping,
    Failed(String),
}

pub struct OllamaServer {
    dir: PathBuf,
    clis: Arc<CliManager>,
    port: std::sync::atomic::AtomicU16,
    state: Mutex<OllamaState>,
    child: Mutex<Option<Child>>,
    gen: std::sync::atomic::AtomicU64,
    on_ready: Mutex<Option<Box<dyn Fn(&str) + Send + Sync>>>,
}

fn io_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// `GET /api/version` against a local port: the version string, or `None` when nothing (Ollama) answers.
pub fn probe(port: u16) -> Option<String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(b"GET /api/version HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
    let mut text = String::new();
    s.take(8192).read_to_string(&mut text).ok()?;
    let body = text.split_once("\r\n\r\n")?.1;
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// The names of the models the Ollama on a local port lists (`GET /api/tags`).
pub fn list_models(port: u16) -> Result<Vec<String>, String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|e| format!("nothing answers on port {port}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).map_err(io_err)?;
    s.set_write_timeout(Some(Duration::from_millis(800))).map_err(io_err)?;
    s.write_all(b"GET /api/tags HTTP/1.0\r\nHost: localhost\r\n\r\n").map_err(io_err)?;
    let mut text = String::new();
    s.take(4 << 20).read_to_string(&mut text).map_err(io_err)?;
    let body = text.split_once("\r\n\r\n").ok_or("no answer from Ollama")?.1;
    parse_tags(body)
}

/// The model names in an `/api/tags` answer.
pub fn parse_tags(body: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).map_err(|e| format!("not an Ollama answer: {e}"))?;
    let list = v.get("models").and_then(|m| m.as_array()).ok_or("the answer has no model list")?;
    Ok(list.iter().filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect())
}

impl OllamaServer {
    pub fn new(dir: &Path, clis: Arc<CliManager>) -> OllamaServer {
        OllamaServer { dir: dir.into(), clis, port: std::sync::atomic::AtomicU16::new(DEFAULT_PORT), state: Mutex::new(OllamaState::Stopped), child: Mutex::new(None), gen: std::sync::atomic::AtomicU64::new(0), on_ready: Mutex::new(None) }
    }

    /// For tests: another port.
    pub fn with_port(self, port: u16) -> OllamaServer {
        self.set_port(port);
        self
    }

    /// Listen on another port (the default is Ollama's own 11434).
    pub fn set_port(&self, port: u16) {
        self.port.store(port, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn port(&self) -> u16 {
        self.port.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Called with the server's address each time it becomes ready (the services add it to Models there).
    pub fn set_on_ready(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.on_ready.lock().unwrap() = Some(Box::new(f));
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    pub fn models_dir(&self) -> PathBuf {
        self.dir.join("models")
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join("server.log")
    }

    pub fn state(&self) -> OllamaState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, s: OllamaState) {
        *self.state.lock().unwrap() = s;
    }

    pub fn installed(&self) -> bool {
        self.clis.is_installed("ollama").is_some()
    }

    pub fn log_tail(&self, lines: usize) -> String {
        let t = std::fs::read_to_string(self.log_path()).unwrap_or_default();
        let all: Vec<&str> = t.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }

    /// What stands in the way of starting (empty when nothing).
    pub fn requirements(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.installed() {
            v.push("Ollama is not installed (install it in the CLIs tab)".to_string());
        }
        v
    }

    fn ready(&self, version: &str, external: bool) {
        self.set_state(OllamaState::Running { external, version: version.to_string() });
        if let Some(f) = self.on_ready.lock().unwrap().as_ref() {
            f(&self.url());
        }
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        if matches!(self.state(), OllamaState::Starting | OllamaState::Running { .. }) {
            return Err("Ollama is already running".into());
        }
        // an Ollama someone else started on this port is simply used
        if let Some(v) = probe(self.port()) {
            self.ready(&v, true);
            return Ok(());
        }
        let missing = self.requirements();
        if !missing.is_empty() {
            return Err(format!("Ollama cannot start here: {}", missing.join("; ")));
        }
        std::fs::create_dir_all(self.models_dir()).map_err(io_err)?;
        std::fs::create_dir_all(self.dir.join("home")).map_err(io_err)?;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.log_path()).map_err(io_err)?;
        let log2 = log.try_clone().map_err(io_err)?;
        use std::os::unix::process::CommandExt;
        let child = Command::new(self.clis.bin_dir().join("ollama"))
            .arg("serve")
            .env_clear()
            .env("PATH", format!("{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", self.clis.bin_dir().display()))
            .env("HOME", self.dir.join("home"))
            .env("OLLAMA_HOST", format!("127.0.0.1:{}", self.port()))
            .env("OLLAMA_MODELS", self.models_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2))
            .process_group(0)
            .spawn()
            .map_err(|e| format!("could not start ollama: {e}"))?;
        *self.child.lock().unwrap() = Some(child);
        self.set_state(OllamaState::Starting);
        let gen = self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let me = self.clone();
        std::thread::Builder::new().name("iyakki-ollama-monitor".into()).spawn(move || me.monitor(gen)).map_err(io_err)?;
        Ok(())
    }

    fn current(&self, gen: u64) -> bool {
        self.gen.load(std::sync::atomic::Ordering::SeqCst) == gen
    }

    fn monitor(&self, gen: u64) {
        let began = Instant::now();
        while self.current(gen) {
            {
                let mut c = self.child.lock().unwrap();
                match c.as_mut() {
                    Some(ch) => {
                        let life = iyakki_core_api::child_status(ch);
                        if life.is_over() {
                            let status = life.describe();
                            *c = None;
                            drop(c);
                            if self.current(gen) {
                                self.set_state(OllamaState::Failed(format!("ollama exited ({status}); the end of its log: {}", self.log_tail(3).replace('\n', " | "))));
                            }
                            return;
                        }
                    }
                    None => return,
                }
            }
            if self.state() == OllamaState::Starting {
                if let Some(v) = probe(self.port()) {
                    if self.current(gen) {
                        self.ready(&v, false);
                    }
                }
            }
            if began.elapsed() > Duration::from_secs(120) && self.state() == OllamaState::Starting {
                self.set_state(OllamaState::Failed("Ollama did not answer within two minutes".into()));
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Stop the server this program started (SIGTERM, then SIGKILL after ten seconds). One someone else started is left alone.
    pub fn stop(&self) -> Result<(), String> {
        if matches!(self.state(), OllamaState::Running { external: true, .. }) {
            self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.set_state(OllamaState::Stopped);
            return Err("this Ollama was not started from here, so it is left running (it is simply not used as the local one any more)".into());
        }
        let child = self.child.lock().unwrap().take();
        self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(mut child) = child else {
            self.set_state(OllamaState::Stopped);
            return Err("Ollama is not running".into());
        };
        self.set_state(OllamaState::Stopping);
        // SAFETY: signalling the process group we created with `process_group(0)`.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
        let t = Instant::now();
        loop {
            match iyakki_core_api::child_status(&mut child) {
                s if s.is_over() => break,
                _ if t.elapsed() > Duration::from_secs(10) => {
                    // SAFETY: as above.
                    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        self.set_state(OllamaState::Stopped);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    /// A stand-in Ollama server: answers `/api/version` on `port` until the returned flag is set.
    fn answer_version(port: u16, version: &'static str) -> Arc<std::sync::atomic::AtomicBool> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
        l.set_nonblocking(true).unwrap();
        let s = stop.clone();
        std::thread::spawn(move || {
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                match l.accept() {
                    Ok((mut c, _)) => {
                        let mut b = [0u8; 512];
                        let _ = c.set_read_timeout(Some(Duration::from_millis(200)));
                        let _ = c.read(&mut b);
                        let body = format!("{{\"version\":\"{version}\"}}");
                        let _ = c.write_all(format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        stop
    }

    fn manager(tag: &str, script: &str) -> (Arc<CliManager>, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-ollama-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let clis = Arc::new(CliManager::new(&d.join("cli")));
        crate::clis::write_fake_installed(&clis, "ollama", script);
        (clis, d)
    }

    fn until(mut f: impl FnMut() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("it never happened");
    }

    #[test]
    fn the_probe_reads_the_version_and_says_nothing_for_a_port_with_nothing_on_it() {
        let p = free_port();
        assert_eq!(probe(p), None);
        let stop = answer_version(p, "0.40.3");
        assert_eq!(probe(p).as_deref(), Some("0.40.3"));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn the_server_starts_when_it_answers_is_added_to_models_and_stops_cleanly() {
        let port = free_port();
        let (clis, d) = manager("run", "#!/bin/sh\nexec sleep 300\n");
        let server = Arc::new(OllamaServer::new(&d.join("ollama"), clis).with_port(port));
        let ready: Arc<Mutex<Vec<String>>> = Default::default();
        let r2 = ready.clone();
        server.set_on_ready(move |url| r2.lock().unwrap().push(url.to_string()));
        assert_eq!(server.state(), OllamaState::Stopped);
        server.start().unwrap();
        assert_eq!(server.state(), OllamaState::Starting);
        // nothing answers yet; once something does on the port, it is ready
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(server.state(), OllamaState::Starting);
        let stop = answer_version(port, "0.40.3");
        until(|| matches!(server.state(), OllamaState::Running { .. }));
        assert_eq!(server.state(), OllamaState::Running { external: false, version: "0.40.3".into() });
        assert_eq!(*ready.lock().unwrap(), vec![format!("http://127.0.0.1:{port}")]);
        assert!(server.start().unwrap_err().contains("already running"));
        server.stop().unwrap();
        assert_eq!(server.state(), OllamaState::Stopped);
        assert!(server.stop().unwrap_err().contains("not running"));
        assert!(server.models_dir().exists(), "models live in the data folder");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_server_that_dies_is_reported_with_the_end_of_its_log_and_a_missing_install_is_explained() {
        let port = free_port();
        let (clis, d) = manager("die", "#!/bin/sh\necho 'cuda driver too old' >&2\nexit 3\n");
        let server = Arc::new(OllamaServer::new(&d.join("ollama"), clis).with_port(port));
        server.start().unwrap();
        until(|| matches!(server.state(), OllamaState::Failed(_)));
        let OllamaState::Failed(why) = server.state() else { unreachable!() };
        assert!(why.contains("exited") && why.contains("cuda driver too old"), "{why}");
        let none = Arc::new(OllamaServer::new(&d.join("o2"), Arc::new(CliManager::new(&d.join("cli2")))).with_port(free_port()));
        assert!(none.start().unwrap_err().contains("not installed"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_ollama_somebody_else_started_is_used_and_never_stopped_from_here() {
        let port = free_port();
        let stop = answer_version(port, "0.39.0");
        let (clis, d) = manager("ext", "#!/bin/sh\nexit 1\n");
        let server = Arc::new(OllamaServer::new(&d.join("ollama"), clis).with_port(port));
        server.start().unwrap();
        assert_eq!(server.state(), OllamaState::Running { external: true, version: "0.39.0".into() });
        assert!(server.stop().unwrap_err().contains("not started from here"));
        assert_eq!(probe(port).as_deref(), Some("0.39.0"), "it is still there");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod gone_tests {
    use super::*;

    /// The PID-1 reaper took the status of the ollama process: the monitor must still notice that it is gone.
    #[test]
    fn a_server_whose_exit_status_was_taken_by_the_reaper_is_seen_as_exited() {
        let d = crate::archive::tests::tmp("ol-gone");
        let o = OllamaServer::new(&d, Arc::new(CliManager::new(&d.join("cli"))));
        let c = std::process::Command::new("true").spawn().unwrap();
        iyakki_core_api::steal_status(c.id());
        *o.child.lock().unwrap() = Some(c);
        o.set_state(OllamaState::Starting);
        o.monitor(0);
        match o.state() {
            OllamaState::Failed(why) => assert!(why.contains("ollama exited") && why.contains("not available"), "{why}"),
            s => panic!("a dead server was not noticed: {s:?}"),
        }
        assert!(o.child.lock().unwrap().is_none());
    }

    #[test]
    fn stop_does_not_hang_on_a_server_whose_status_was_taken() {
        let d = crate::archive::tests::tmp("ol-gone-stop");
        let o = OllamaServer::new(&d, Arc::new(CliManager::new(&d.join("cli"))));
        let c = std::process::Command::new("true").spawn().unwrap();
        iyakki_core_api::steal_status(c.id());
        *o.child.lock().unwrap() = Some(c);
        let t = Instant::now();
        assert!(o.stop().is_ok());
        assert!(t.elapsed() < Duration::from_secs(2));
        assert_eq!(o.state(), OllamaState::Stopped);
    }
}
