//! The line terminal with its SSH call, and the terminal of the core contract with its whole-session pty, served over a real loopback socket
//! against the reference server (F-SPLIT-03 part 3, D-SPLIT-03-SERVED): a command that outlives a call is polled for its lines, an answer longer
//! than a line comes as a download, and a screen that did not change is not sent again. Test level: real transport, fake machine.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::{Terminal, TerminalCall};
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::terminal::{PtyKey, PtyMods, Terminal as CoreTerminal};
use iyakki_shared::cli::{Action, CliOut};
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{remote_core_terminal, remote_terminal, serve_core_terminal, serve_terminal, Host, Rpc};

/// One SSH command of the fake: it says three lines, then ends as it was told to.
struct Call {
    end: Result<String, String>,
}
impl TerminalCall for Call {
    fn command(&self) -> &str {
        "ssh box uptime"
    }
    fn run(&self, sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String> {
        if let Some(s) = sink {
            for l in ["up 3 days", "load 0.1", "done"] {
                s(l);
            }
        }
        self.end.clone()
    }
}

/// A line terminal that echoes, answers a very long line for one command, and hands out SSH calls for lines that start with `ssh`.
struct Lines {
    ran: Arc<Mutex<Vec<String>>>,
    ssh_end: Result<String, String>,
}
impl Terminal for Lines {
    fn prompt(&self) -> String {
        "ann@box$ ".into()
    }
    fn exec(&mut self, line: &str) -> CliOut {
        self.ran.lock().unwrap().push(line.to_string());
        match line {
            "clear" => CliOut { text: String::new(), action: Action::Clear },
            "long" => CliOut { text: "x".repeat(100_000), action: Action::None },
            other => CliOut { text: format!("ran: {other}"), action: Action::None },
        }
    }
    fn ssh_call(&self, line: &str) -> Option<Box<dyn TerminalCall>> {
        line.starts_with("ssh").then(|| Box::new(Call { end: self.ssh_end.clone() }) as Box<dyn TerminalCall>)
    }
    fn full(&self) -> bool {
        true
    }
}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn machine(host: Host) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served {
        hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        rpc: Some(Arc::new(host)),
        appevents: None,
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn rpc(m: &Machine) -> Rpc {
    Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap())
}

#[test]
fn a_line_terminal_runs_lines_in_the_core_and_a_long_answer_comes_as_a_download() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let m = machine(Host::new().with_root(serve_terminal(Box::new(Lines { ran: ran.clone(), ssh_end: Ok("ok".into()) }))));
    let mut t = remote_terminal(&rpc(&m), 0);
    assert_eq!(t.prompt(), "ann@box$ ");
    assert!(t.full());
    assert_eq!(t.exec("ls"), CliOut { text: "ran: ls".into(), action: Action::None });
    assert_eq!(t.exec("clear").action, Action::Clear);
    // 100 kB does not fit a line: it comes as a download read in chunks, and it is whole
    let long = t.exec("long");
    assert_eq!((long.text.len(), long.text.chars().all(|c| c == 'x')), (100_000, true));
    assert_eq!(ran.lock().unwrap().as_slice(), ["ls", "clear", "long"], "every line ran on the core's terminal, in order");
}

#[test]
fn a_long_ssh_command_is_started_on_the_core_and_its_lines_arrive_as_they_come() {
    let m = machine(Host::new().with_root(serve_terminal(Box::new(Lines { ran: Default::default(), ssh_end: Ok("exit 0".into()) }))));
    let t = remote_terminal(&rpc(&m), 0);
    assert!(t.ssh_call("ls").is_none(), "a line that is not an SSH command has no call");
    let call = t.ssh_call("ssh box uptime").expect("an SSH line has a call");
    assert_eq!(call.command(), "ssh box uptime");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let r = call.run(Some(Arc::new(move |l: &str| s2.lock().unwrap().push(l.to_string()))));
    assert_eq!(r, Ok("exit 0".to_string()));
    assert_eq!(seen.lock().unwrap().as_slice(), ["up 3 days", "load 0.1", "done"], "every line, once, in order");
    // a command that fails ends with the core's words
    let m2 = machine(Host::new().with_root(serve_terminal(Box::new(Lines { ran: Default::default(), ssh_end: Err("connection refused".into()) }))));
    let t2 = remote_terminal(&rpc(&m2), 0);
    assert_eq!(t2.ssh_call("ssh x").unwrap().run(None), Err("connection refused".to_string()));
    // the same call cannot be started twice
    let again = t.ssh_call("ssh box uptime").unwrap();
    assert!(again.run(None).is_ok());
}

#[test]
fn the_core_terminal_runs_sessions_and_a_whole_session_pty_whose_screen_is_pulled_and_not_resent_when_unchanged() {
    let fake = Arc::new(iyakki_core_fake::FakeTerminal::default());
    let m = machine(Host::new().with_root(serve_core_terminal(fake.clone())));
    let t = remote_core_terminal(&rpc(&m), 0);
    // sessions of lines: the question is asked and answered by the person, and nothing runs before the answer
    let open = t.open("ann", "sample").unwrap();
    let asked = t.run("ann", &open.id, "rm file").unwrap();
    let q = asked.approval.expect("a line that changes something waits");
    assert!(fake.log().is_empty(), "nothing ran before the answer");
    assert_eq!(t.decide("ann", &open.id, &q.call, true).unwrap().text, "ran: rm file");
    assert!(t.run("ann", "no-such-session", "ls").is_err(), "the core's refusal comes back in the core's words");
    t.close("ann", &open.id);
    // the whole-session pty: open (a question), answer, type, read the screen
    let p = t.pty_open("ann", "sample", 100, 40).unwrap();
    t.pty_decide("ann", &p.id, &p.approval.call, true).unwrap();
    t.pty_text("ann", &p.id, "echo hi").unwrap();
    t.pty_key("ann", &p.id, PtyKey::Enter, PtyMods::default()).unwrap();
    // a screen of 100 x 40 cells is far longer than a line: it comes as a download and it is the core's screen cell for cell
    let frame = t.pty_frame("ann", &p.id).unwrap();
    assert_eq!((frame.cols, frame.rows), (100, 40));
    assert_eq!(frame, fake.pty_frame("ann", &p.id).unwrap());
    assert!(frame.text().contains("ran: echo hi"));
    // a screen that did not change is not sent again; a changed one is
    assert_eq!(t.frame_if_changed("ann", &p.id, Some(frame.seq)).unwrap(), None);
    t.pty_key("ann", &p.id, PtyKey::Char('x'), PtyMods::default()).unwrap();
    let next = t.frame_if_changed("ann", &p.id, Some(frame.seq)).unwrap().expect("the screen changed");
    assert!(next.seq > frame.seq);
    t.pty_resize("ann", &p.id, 80, 24).unwrap();
    assert_eq!((t.pty_frame("ann", &p.id).unwrap().cols, t.pty_frame("ann", &p.id).unwrap().rows), (80, 24));
    assert!(t.pty_resize("ann", &p.id, 0, 24).is_err(), "a size the core refuses is refused in its words");
    t.pty_close("ann", &p.id);
    assert!(t.pty_frame("ann", &p.id).is_err());
}

#[test]
fn a_lost_core_answers_errors_and_quiet_values_and_never_panics() {
    let closed = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let t = remote_core_terminal(&closed, u64::MAX);
    assert!(t.open("ann", "sample").is_err());
    assert!(t.pty_frame("ann", "p1").is_err());
    assert_eq!(t.frame_if_changed("ann", "p1", None).unwrap_err().is_empty(), false);
    let mut l = remote_terminal(&closed, u64::MAX);
    assert_eq!(l.prompt(), "$ ", "the plain prompt of a terminal that cannot be reached");
    assert!(!l.full());
    assert!(l.exec("ls").text.contains("closed"), "the reason is shown, nothing is made up");
    assert!(l.ssh_call("ssh x").is_none());
}
