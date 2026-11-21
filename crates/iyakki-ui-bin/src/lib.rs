//! The command line and the socket handshake of `iyakki-ui`, apart from `main` so they are tested.
//!
//! ```text
//! iyakki-ui --socket PATH [--fb /dev/fb0] [--restore]     talk to the core over its local UI socket (the ui-layer program)
//! iyakki-ui --in-process [--fb /dev/fb0]                   run the shell with the real services in this program (feature `in-process`)
//! ```

use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "socket-shell")]
pub mod socket_shell;

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Services come from the core over the unix socket at this path.
    Socket(PathBuf),
    /// The real services are linked into this program.
    InProcess,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub mode: Mode,
    pub fb: String,
    /// The core started this program again after a crash: restore the apps (F-SHELL-65, developer mode only; the program decides).
    pub restore: bool,
}

pub const USAGE: &str = "usage: iyakki-ui (--socket PATH | --in-process) [--fb /dev/fb0] [--restore]";

pub fn parse(a: &[String]) -> Result<Args, String> {
    let (mut mode, mut fb, mut restore) = (None, "/dev/fb0".to_string(), false);
    let mut it = a.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--socket" => mode = Some(Mode::Socket(it.next().ok_or("--socket needs a path")?.into())),
            "--in-process" => mode = Some(Mode::InProcess),
            "--fb" => fb = it.next().ok_or("--fb needs a path")?.clone(),
            "--restore" => restore = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args { mode: mode.ok_or("one of --socket or --in-process is needed")?, fb, restore })
}

/// The remote objects of the core over its UI socket (the program's services without the agent linked in, UI-RPC-DESIGN.md): every
/// `Remote*` of `iyakki-ui-rpc` calls through this one connection.
pub fn remote_services(path: &std::path::Path) -> Result<iyakki_ui_rpc::Rpc, String> {
    let c = iyakki_ui_client::client::Connection::unix(path, Duration::from_secs(5)).map_err(|e| format!("cannot reach the core at {}: {}", path.display(), e.message))?;
    Ok(iyakki_ui_rpc::Rpc::new(c))
}

/// What the program learned from the core at start: its name and the domains it serves.
#[derive(Debug, PartialEq, Eq)]
pub struct CoreInfo {
    pub machine: String,
    pub domains: Vec<String>,
}

/// Connect to the core's UI socket and say hello (the contract's own negotiation: a core of another major version is refused here).
pub fn handshake(path: &std::path::Path) -> Result<CoreInfo, String> {
    let mut c = iyakki_ui_client::client::Connection::unix(path, Duration::from_secs(5)).map_err(|e| format!("cannot reach the core at {}: {}", path.display(), e.message))?;
    let (hello, n) = c.negotiate(&["settings"]).map_err(|e| format!("the core refused: {}", e.message))?;
    if !n.missing.is_empty() {
        return Err(format!("the core does not serve: {}", n.missing.join(", ")));
    }
    Ok(CoreInfo { machine: hello.machine, domains: hello.domains.iter().map(|d| d.domain.clone()).collect() })
}

/// What the program holds after it connected: what the core said and the services over the socket.
pub struct Connected {
    pub info: CoreInfo,
    pub services: iyakki_ui_rpc::RemoteServicesHandle,
}

/// Why the program cannot show the screen yet, in one sentence per case (the supervisor reads the exit status that goes with it).
#[derive(Debug, PartialEq, Eq)]
pub enum NotReady {
    /// The core answered `hello` but does not serve the object calls (the `rpc` switch of the socket is off, or an older core).
    NoObjectCalls,
}

/// Connect, say hello and check that the core serves the object calls the services come over (`rpc.call`). A core that does not is not taken for
/// the real services (KI-1010): the program stops with a reason instead of drawing a screen over nothing.
pub fn connect(path: &std::path::Path) -> Result<Result<Connected, NotReady>, String> {
    let info = handshake(path)?;
    let services = iyakki_ui_rpc::RemoteServicesHandle::new(remote_services(path)?);
    if !services.is_real() {
        return Ok(Err(NotReady::NoObjectCalls));
    }
    Ok(Ok(Connected { info, services }))
}

/// Exit status of the program when the core stopped answering (the supervisor restarts it with its growing pause, `iyakki_core::uiprocess::policy`).
pub const EXIT_CORE_LOST: i32 = 5;

/// Counts the misses of the core's answer: a screen over a core that is gone draws nothing true, so after `limit` misses in a row the program ends
/// and the supervisor starts it again, which connects to the core as it is then (the person's switch off, a core restart). One answer clears the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreWatch {
    misses: u32,
    limit: u32,
}

impl CoreWatch {
    pub fn new(limit: u32) -> CoreWatch {
        CoreWatch { misses: 0, limit: limit.max(1) }
    }

    /// Note one look at the core; true when the core is lost (the program should end).
    pub fn note(&mut self, answered: bool) -> bool {
        if answered {
            self.misses = 0;
        } else {
            self.misses = self.misses.saturating_add(1);
        }
        self.misses >= self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_program_ends_after_so_many_misses_in_a_row_and_one_answer_clears_them() {
        let mut w = CoreWatch::new(3);
        assert!(!w.note(false) && !w.note(false), "two misses are not yet a lost core");
        assert!(!w.note(true), "an answer clears the count");
        assert!(!w.note(false) && !w.note(false));
        assert!(w.note(false), "three in a row end the program");
        assert!(w.note(false), "and it stays lost");
        assert!(CoreWatch::new(0).note(false), "a limit of nothing means one miss");
    }

    #[test]
    fn the_command_line_picks_one_mode_and_defaults_the_screen() {
        assert_eq!(parse(&v(&["--socket", "/run/iyakki/ui/ui.sock", "--restore"])).unwrap(), Args { mode: Mode::Socket("/run/iyakki/ui/ui.sock".into()), fb: "/dev/fb0".into(), restore: true });
        assert_eq!(parse(&v(&["--in-process", "--fb", "/dev/fb1"])).unwrap().fb, "/dev/fb1");
        assert!(parse(&v(&[])).is_err(), "no mode is an error, not a guess");
        assert!(parse(&v(&["--socket"])).is_err());
        assert!(parse(&v(&["--bogus"])).is_err());
    }

    /// A core that serves one root (the inbox) over a unix socket in a short folder, as the real core does with `rpc` on.
    fn serve_core(tag: &str, with_rpc: bool) -> (PathBuf, std::sync::Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>, PathBuf) {
        use iyakki_ui_client::server::{serve_unix, settings_hello, Served};
        use iyakki_core_api::contract::RateLimit;
        use iyakki_ui_client::{CallContext, Scope, Transport};
        let dir = std::path::PathBuf::from("/tmp").join(format!("iyk-uib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let path = dir.join("ui.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let host = iyakki_ui_rpc::Host::new().with_root(iyakki_ui_rpc::Obj::InboxApi(std::sync::Arc::new(iyakki_shared::inbox::Inbox::new(&data))));
        let served = Served {
            hello: settings_hello("fake-core", RateLimit { calls: 100_000, per_ms: 1000 }),
            settings: None,
            vault: None,
            flows: None,
            assistant: None,
            cameras: None,
            appevents: None,
            rpc: with_rpc.then(|| std::sync::Arc::new(host) as std::sync::Arc<dyn iyakki_ui_client::server::RpcServe>),
            ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
        };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        let t = std::thread::spawn(move || serve_unix(listener, std::sync::Arc::new(served), s2).unwrap());
        (path, stop, t, dir)
    }

    #[test]
    fn the_program_connects_to_a_core_over_the_socket_and_uses_its_services_and_refuses_a_core_without_object_calls() {
        let (path, stop, t, dir) = serve_core("a", true);
        let c = connect(&path).unwrap().ok().expect("a core that serves object calls is ready");
        assert_eq!(c.info.machine, "fake-core");
        assert!(c.services.is_real());
        assert!(c.services.inbox().list("ann").is_empty(), "a call over the socket reaches the core's store");
        drop(c);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // a core that answers hello but serves no object calls is not the real services
        let (path, stop, t, dir) = serve_core("b", false);
        assert_eq!(connect(&path).unwrap().err(), Some(NotReady::NoObjectCalls));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // nothing listening is a plain error with the path in it
        assert!(connect(std::path::Path::new("/tmp/iyk-uib-none/ui.sock")).err().unwrap().contains("/tmp/iyk-uib-none/ui.sock"));
    }
}
