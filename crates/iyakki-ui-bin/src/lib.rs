//! The command line and the socket handshake of `iyakki-ui`, apart from `main` so they are tested.
//!
//! ```text
//! iyakki-ui --socket PATH [--fb /dev/fb0] [--restore]     talk to the core over its local UI socket (the ui-layer program)
//! iyakki-ui --in-process [--fb /dev/fb0]                   run the shell with the real services in this program (feature `in-process`)
//! ```

use std::path::PathBuf;
use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_command_line_picks_one_mode_and_defaults_the_screen() {
        assert_eq!(parse(&v(&["--socket", "/run/iyakki/ui/ui.sock", "--restore"])).unwrap(), Args { mode: Mode::Socket("/run/iyakki/ui/ui.sock".into()), fb: "/dev/fb0".into(), restore: true });
        assert_eq!(parse(&v(&["--in-process", "--fb", "/dev/fb1"])).unwrap().fb, "/dev/fb1");
        assert!(parse(&v(&[])).is_err(), "no mode is an error, not a guess");
        assert!(parse(&v(&["--socket"])).is_err());
        assert!(parse(&v(&["--bogus"])).is_err());
    }
}
