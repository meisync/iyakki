//! Whole-session terminals for apps that embed one (`Terminal::pty_*` of the core contract, F-SDK-103): a real shell on a pseudo-terminal as
//! the person's own system user, with the screen kept here (`iyakki-vt`: parser, cell grid, keys) and handed out as a frame.
//!
//! The rule (docs/development/ALPHA-7-DECISIONS.md, D-TERM-1): a session is not checked line by line, so the person says yes once for the
//! whole session; nothing starts before the yes, only an account whose terminal level is Full may ask, and a person who answered no gets
//! nothing. The audit holds the open, the answer, the end and the close, never what was typed or shown. The program is never started as
//! the core's own user (root): [`SystemShell`] resolves the person's system user the way the developer shell does and refuses to run as
//! root.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_core_api::terminal::{self as t, PtyCell, PtyColor, PtyFrame, PtyKey, PtyMods, PtyOpen, TermApproval};
use iyakki_vt::{Key, Link, Mods, Terminal as VtTerminal};

/// Starts the program of a session. The real one is [`SystemShell`]; tests give a scripted link.
pub trait ShellSpawner: Send + Sync {
    fn spawn(&self, user: &str, cols: usize, rows: usize) -> Result<Box<dyn Link + Send>, String>;
    /// `kubectl exec -it` into a pod (F-K8S-26), as the person's system user, with the arguments the core built from a checked [`t::PtyExec`].
    fn spawn_exec(&self, _user: &str, _cols: usize, _rows: usize, _argv: &[String]) -> Result<Box<dyn Link + Send>, String> {
        Err("a pod terminal is not available here".into())
    }
}

/// The person's own shell on a pseudo-terminal, as their system user, in their home folder, with a clean environment.
pub struct SystemShell {
    pub data_dir: PathBuf,
}

impl SystemShell {
    /// The person's system user, their folder and a clean environment (`TERM` set), and the program started on a pseudo-terminal there. Never as root.
    #[cfg(unix)]
    fn start(&self, user: &str, cols: usize, rows: usize, program: &str, args: &[String], what: &str) -> Result<Box<dyn Link + Send>, String> {
        use crate::devmode::shellexec as sx;
        use iyakki_vt::pty::SpawnOpts;
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid(2) takes no arguments and cannot fail.
        let euid = unsafe { libc::geteuid() };
        let home = crate::scope::home_root_for_writes(user, &self.data_dir);
        let owner = std::fs::metadata(&home).ok().map(|m| (m.uid(), m.gid()));
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let ident = sx::identity_with_owner(user, false, &passwd, euid, &home, owner)?;
        if ident.root || (euid == 0 && ident.uid.is_none_or(|u| u == 0)) {
            return Err("a terminal does not run as root".into());
        }
        let dir = sx::start_dir(&ident, &home, &sx::real_stat);
        let mut env = sx::command_env(&dir, &ident.user);
        env.retain(|(k, _)| !matches!(*k, "TERM" | "IYAKKI_DEV"));
        env.push(("TERM", "xterm-256color".to_string()));
        let env: Vec<(String, String)> = env.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let opts = SpawnOpts { cwd: Some(dir), uid: ident.uid, gid: ident.gid };
        let link = iyakki_vt::session::PtyLink::spawn_with(program, args, &env, cols, rows, &opts).map_err(|e| format!("could not start the terminal: {e}"))?;
        // the program is the person's in SYSTEM's "My processes" for as long as the session lives (KI-131)
        let pid = link.pid();
        Ok(claimed(Box::new(link), pid, user, what))
    }
}

impl ShellSpawner for SystemShell {
    #[cfg(unix)]
    fn spawn(&self, user: &str, cols: usize, rows: usize) -> Result<Box<dyn Link + Send>, String> {
        self.start(user, cols, rows, "/bin/sh", &["-i".to_string()], "terminal")
    }

    #[cfg(unix)]
    fn spawn_exec(&self, user: &str, cols: usize, rows: usize, argv: &[String]) -> Result<Box<dyn Link + Send>, String> {
        self.start(user, cols, rows, "kubectl", argv, "kubectl exec")
    }

    #[cfg(not(unix))]
    fn spawn(&self, _user: &str, _cols: usize, _rows: usize) -> Result<Box<dyn Link + Send>, String> {
        Err("a terminal needs a Unix system".into())
    }
}

/// A link whose program is claimed for a person (`procowner`) until the link is dropped.
pub struct ClaimedLink {
    inner: Box<dyn Link + Send>,
    _claim: crate::procowner::ClaimGuard,
}

pub fn claimed(inner: Box<dyn Link + Send>, pid: u32, user: &str, what: &str) -> Box<dyn Link + Send> {
    Box::new(ClaimedLink { inner, _claim: crate::procowner::claim(pid, user, what) })
}

impl Link for ClaimedLink {
    fn read(&mut self, out: &mut Vec<u8>) -> iyakki_vt::LinkState {
        self.inner.read(out)
    }
    fn write(&mut self, bytes: &[u8]) {
        self.inner.write(bytes)
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        self.inner.resize(cols, rows)
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.inner.exit_code()
    }
    fn close(&mut self) {
        self.inner.close()
    }
}

enum State {
    /// Asked, no answer yet: nothing runs.
    Asking { call: String, cols: usize, rows: usize, exec: Option<Box<t::PtyExec>> },
    Live(Box<VtTerminal<Box<dyn Link + Send>>>),
}

struct Session {
    app: String,
    state: State,
}

pub struct PtyBook {
    sessions: Mutex<HashMap<(String, String), Session>>,
    next: std::sync::atomic::AtomicU64,
    spawner: Mutex<Arc<dyn ShellSpawner>>,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn key_of(k: PtyKey) -> Key {
    match k {
        PtyKey::Char(c) => Key::Char(c),
        PtyKey::Enter => Key::Enter,
        PtyKey::Tab => Key::Tab,
        PtyKey::BackTab => Key::BackTab,
        PtyKey::Backspace => Key::Backspace,
        PtyKey::Esc => Key::Esc,
        PtyKey::Up => Key::Up,
        PtyKey::Down => Key::Down,
        PtyKey::Left => Key::Left,
        PtyKey::Right => Key::Right,
        PtyKey::Home => Key::Home,
        PtyKey::End => Key::End,
        PtyKey::PageUp => Key::PageUp,
        PtyKey::PageDown => Key::PageDown,
        PtyKey::Insert => Key::Insert,
        PtyKey::Delete => Key::Delete,
        PtyKey::F(n) => Key::F(n),
    }
}

fn color_of(c: iyakki_vt::Color) -> PtyColor {
    match c {
        iyakki_vt::Color::Default => PtyColor::Default,
        iyakki_vt::Color::Indexed(i) => PtyColor::Indexed(i),
        iyakki_vt::Color::Rgb(r, g, b) => PtyColor::Rgb(r, g, b),
    }
}

fn cell_of(c: iyakki_vt::Cell) -> PtyCell {
    PtyCell { ch: c.ch, fg: color_of(c.fg), bg: color_of(c.bg), attrs: c.attrs, tail: c.wide_tail }
}

impl PtyBook {
    pub fn new(data_dir: PathBuf) -> PtyBook {
        PtyBook { sessions: Mutex::new(HashMap::new()), next: std::sync::atomic::AtomicU64::new(1), spawner: Mutex::new(Arc::new(SystemShell { data_dir })) }
    }

    /// Tests only: another program for the sessions (a scripted link).
    #[cfg(test)]
    pub(crate) fn set_spawner(&self, s: Arc<dyn ShellSpawner>) {
        *lk(&self.spawner) = s;
    }

    /// Ask for a session; `audit(what, word)` records the steps. The account's level must already be Full (checked by the caller).
    pub fn open(&self, user: &str, app: &str, cols: usize, rows: usize, audit: &dyn Fn(&str, &str)) -> Result<PtyOpen, String> {
        t::check_size(cols, rows)?;
        let mut g = lk(&self.sessions);
        if g.keys().filter(|(u, _)| u == user).count() >= t::PTY_MAX_SESSIONS {
            return Err(format!("at most {} full terminals can be open at once", t::PTY_MAX_SESSIONS));
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (id, call) = (format!("p{n}"), format!("pc{n}"));
        let app: String = app.chars().take(40).collect();
        g.insert((user.to_string(), id.clone()), Session { app: app.clone(), state: State::Asking { call: call.clone(), cols, rows, exec: None } });
        drop(g);
        audit("pty.asked", &app);
        Ok(PtyOpen { id, approval: TermApproval { call, summary: t::PTY_QUESTION.to_string() } })
    }

    /// Ask for a session in a pod (`kubectl exec -it`); the question names the pod. The audit holds the ask, the answer and the close, with the
    /// namespace and pod name, never what is typed.
    pub fn open_exec(&self, user: &str, app: &str, cols: usize, rows: usize, exec: &t::PtyExec, audit: &dyn Fn(&str, &str)) -> Result<PtyOpen, String> {
        exec.check()?;
        t::check_size(cols, rows)?;
        let mut g = lk(&self.sessions);
        if g.keys().filter(|(u, _)| u == user).count() >= t::PTY_MAX_SESSIONS {
            return Err(format!("at most {} full terminals can be open at once", t::PTY_MAX_SESSIONS));
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (id, call) = (format!("p{n}"), format!("pc{n}"));
        let app: String = app.chars().take(40).collect();
        g.insert((user.to_string(), id.clone()), Session { app: app.clone(), state: State::Asking { call: call.clone(), cols, rows, exec: Some(Box::new(exec.clone())) } });
        drop(g);
        audit("pty.asked", &format!("{app} exec {}/{}", exec.namespace, exec.pod));
        Ok(PtyOpen { id, approval: TermApproval { call, summary: exec.question() } })
    }

    pub fn decide(&self, user: &str, session: &str, call: &str, approve: bool, audit: &dyn Fn(&str, &str)) -> Result<(), String> {
        let mut g = lk(&self.sessions);
        let key = (user.to_string(), session.to_string());
        let Some(s) = g.get_mut(&key) else { return Err("that terminal is not open".into()) };
        let (cols, rows, exec) = match &s.state {
            State::Asking { call: c, cols, rows, exec } if c == call => (*cols, *rows, exec.clone()),
            _ => return Err("there is no such question waiting".into()),
        };
        let app = s.app.clone();
        if !approve {
            g.remove(&key);
            drop(g);
            audit("pty.denied", &app);
            return Ok(());
        }
        let spawner = lk(&self.spawner).clone();
        let started = match &exec {
            Some(e) => e.argv().and_then(|argv| spawner.spawn_exec(user, cols, rows, &argv)),
            None => spawner.spawn(user, cols, rows),
        };
        match started {
            Ok(link) => {
                if let Some(s) = g.get_mut(&key) {
                    s.state = State::Live(Box::new(VtTerminal::new(link, cols, rows)));
                }
                drop(g);
                audit("pty.approved", &app);
                Ok(())
            }
            Err(e) => {
                g.remove(&key);
                drop(g);
                audit("pty.failed", &app);
                Err(e)
            }
        }
    }

    fn live<R>(&self, user: &str, session: &str, f: impl FnOnce(&mut VtTerminal<Box<dyn Link + Send>>) -> R) -> Result<R, String> {
        let mut g = lk(&self.sessions);
        match g.get_mut(&(user.to_string(), session.to_string())) {
            None => Err("that terminal is not open".into()),
            Some(Session { state: State::Asking { .. }, .. }) => Err("the terminal waits for your yes".into()),
            Some(Session { state: State::Live(term), .. }) => Ok(f(term)),
        }
    }

    pub fn key(&self, user: &str, session: &str, k: PtyKey, m: PtyMods) -> Result<(), String> {
        t::check_key(k)?;
        self.live(user, session, |term| term.key(key_of(k), Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift }))
    }

    pub fn text(&self, user: &str, session: &str, text: &str) -> Result<(), String> {
        if text.len() > t::PTY_MAX_PASTE {
            return Err(format!("at most {} bytes can be pasted at once", t::PTY_MAX_PASTE));
        }
        self.live(user, session, |term| term.paste(text))
    }

    pub fn resize(&self, user: &str, session: &str, cols: usize, rows: usize) -> Result<(), String> {
        t::check_size(cols, rows)?;
        self.live(user, session, |term| term.resize(cols, rows))
    }

    pub fn scroll(&self, user: &str, session: &str, lines: isize) -> Result<(), String> {
        self.live(user, session, |term| term.scroll_back(lines.clamp(-10_000, 10_000)))
    }

    pub fn frame(&self, user: &str, session: &str) -> Result<PtyFrame, String> {
        self.live(user, session, |term| {
            term.poll();
            let exited = if term.finished() { term.exit_code() } else { None };
            let s = &term.screen;
            let back = term.back.min(s.scrollback_len());
            let cells = (0..s.rows()).map(|y| s.view_row(back, y).into_iter().map(cell_of).collect::<Vec<_>>()).collect();
            // the view and its history change the picture too, so they are part of the sequence
            PtyFrame { seq: s.generation.wrapping_mul(1024).wrapping_add(back as u64), cols: s.cols(), rows: s.rows(), cells, cursor: (s.x, s.y), cursor_visible: s.cursor_visible && back == 0, title: s.title.chars().take(120).collect(), back, history: s.scrollback_len(), exited }
        })
    }

    /// The end of a person's session (D-SESSION-END): every full terminal of `user` is closed (a live shell is hung up, a question still
    /// waiting is dropped) and audited like a close by hand. Returns how many.
    pub fn close_user(&self, user: &str, audit: &dyn Fn(&str, &str)) -> usize {
        let ids: Vec<String> = lk(&self.sessions).keys().filter(|(u, _)| u == user).map(|(_, id)| id.clone()).collect();
        for id in &ids {
            self.close(user, id, audit);
        }
        ids.len()
    }

    pub fn close(&self, user: &str, session: &str, audit: &dyn Fn(&str, &str)) {
        let removed = lk(&self.sessions).remove(&(user.to_string(), session.to_string()));
        if let Some(Session { app, state }) = removed {
            if let State::Live(mut term) = state {
                term.close();
            }
            audit("pty.close", &app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A program that is a script: it answers each write with a canned output and records what it was sent.
    struct Script {
        out: Arc<Mutex<Vec<u8>>>,
        sent: Arc<Mutex<Vec<u8>>>,
        size: Arc<Mutex<(u16, u16)>>,
        done: Arc<Mutex<Option<i32>>>,
    }

    impl Link for Script {
        fn read(&mut self, out: &mut Vec<u8>) -> iyakki_vt::LinkState {
            out.append(&mut lk(&self.out));
            if lk(&self.done).is_some() { iyakki_vt::LinkState::Closed } else { iyakki_vt::LinkState::Open }
        }
        fn write(&mut self, bytes: &[u8]) {
            lk(&self.sent).extend_from_slice(bytes);
            // an echo, like a shell
            lk(&self.out).extend_from_slice(bytes);
        }
        fn resize(&mut self, cols: u16, rows: u16) {
            *lk(&self.size) = (cols, rows);
        }
        fn exit_code(&mut self) -> Option<i32> {
            *lk(&self.done)
        }
        fn close(&mut self) {
            *lk(&self.done) = Some(129);
        }
    }

    #[test]
    fn ki131_a_terminal_shell_is_claimed_for_the_person_while_the_session_lives() {
        let who = "ki131-term-user";
        let link = iyakki_vt::session::PtyLink::spawn("/bin/sh", &["-c".to_string(), "read x".to_string()], &[("PATH".to_string(), "/usr/bin:/bin".to_string())], 80, 24).unwrap();
        let pid = link.pid();
        let mut boxed = claimed(Box::new(link), pid, who, "terminal");
        let mine = crate::procowner::claims().into_iter().filter(|c| c.user == who).collect::<Vec<_>>();
        assert_eq!(mine.len(), 1);
        assert_eq!((mine[0].pid, mine[0].what.as_str()), (pid, "terminal"));
        boxed.close();
        drop(boxed);
        assert!(crate::procowner::claims().iter().all(|c| c.user != who), "the claim ended with the session");
    }

    /// Runs `f(gate)` as `who` on another thread and returns the claims `who` had while it ran (polled until one shows, never a fixed wait).
    /// The program `f` starts must wait for the `gate` file (`iyakki_testkit::wait_gate_snippet`): it is created once a claim was seen.
    fn claims_seen_while(who: &str, f: impl FnOnce(&str) + Send + 'static) -> Vec<crate::procowner::Claim> {
        let w = who.to_string();
        let gate = crate::testdir::unique("claims", who).join("open");
        let g = gate.to_string_lossy().to_string();
        let t = std::thread::spawn(move || crate::procowner::as_user(&w, move || f(&g)));
        let mut seen = Vec::new();
        while !t.is_finished() && seen.is_empty() {
            seen = crate::procowner::claims().into_iter().filter(|c| c.user == who).collect();
            std::thread::yield_now();
        }
        std::fs::write(&gate, b"").unwrap();
        t.join().unwrap();
        seen
    }

    #[test]
    fn ki131_the_store_install_path_claims_its_program_while_it_runs() {
        let who = "ki131-install-user";
        let seen = claims_seen_while(who, |gate| {
            let script = iyakki_testkit::wait_gate_snippet(std::path::Path::new(gate));
            let _ = crate::clis::run_quiet("/bin/sh", &["-c", &script], 20);
        });
        assert_eq!(seen.first().map(|c| c.what.as_str()), Some("sh"));
        assert!(crate::procowner::claims().iter().all(|c| c.user != who), "a finished program leaves no claim");
    }

    #[test]
    fn ki131_the_cluster_call_path_claims_kubectl_while_it_runs() {
        let who = "ki131-kube-user";
        let seen = claims_seen_while(who, |gate| {
            let k = crate::sandbox::kube::Kubectl { program: "/bin/sh".into(), kubeconfig: None, timeout: std::time::Duration::from_secs(20) };
            let script = iyakki_testkit::wait_gate_snippet(std::path::Path::new(gate));
            let _ = k.run(&["-c", &script], None, 1024);
        });
        assert_eq!(seen.first().map(|c| c.what.as_str()), Some("kubectl"));
        assert!(crate::procowner::claims().iter().all(|c| c.user != who));
    }

    #[derive(Default)]
    struct Spawns {
        count: AtomicUsize,
        out: Arc<Mutex<Vec<u8>>>,
        sent: Arc<Mutex<Vec<u8>>>,
        size: Arc<Mutex<(u16, u16)>>,
        done: Arc<Mutex<Option<i32>>>,
        fail: bool,
        /// The arguments of every pod session started.
        execs: Mutex<Vec<Vec<String>>>,
    }

    impl ShellSpawner for Spawns {
        fn spawn_exec(&self, _user: &str, cols: usize, rows: usize, argv: &[String]) -> Result<Box<dyn Link + Send>, String> {
            if self.fail {
                return Err("no kubectl here".into());
            }
            lk(&self.execs).push(argv.to_vec());
            *lk(&self.size) = (cols as u16, rows as u16);
            Ok(Box::new(Script { out: self.out.clone(), sent: self.sent.clone(), size: self.size.clone(), done: self.done.clone() }))
        }

        fn spawn(&self, _user: &str, cols: usize, rows: usize) -> Result<Box<dyn Link + Send>, String> {
            if self.fail {
                return Err("no shell here".into());
            }
            self.count.fetch_add(1, Ordering::SeqCst);
            *lk(&self.size) = (cols as u16, rows as u16);
            Ok(Box::new(Script { out: self.out.clone(), sent: self.sent.clone(), size: self.size.clone(), done: self.done.clone() }))
        }
    }

    fn book(sp: Arc<Spawns>) -> PtyBook {
        let b = PtyBook::new(std::env::temp_dir());
        b.set_spawner(sp);
        b
    }

    fn quiet(_: &str, _: &str) {}

    fn pod() -> t::PtyExec {
        t::PtyExec { kubeconfig: String::new(), context: "dev".into(), namespace: "shop".into(), pod: "web-1".into(), container: String::new(), shell: String::new() }
    }

    #[test]
    fn a_pod_session_asks_naming_the_pod_starts_kubectl_exec_only_after_the_yes_and_is_audited_without_what_was_typed() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let log = Mutex::new(Vec::<String>::new());
        let au = |w: &str, word: &str| lk(&log).push(format!("{w} {word}"));
        let o = b.open_exec("alice", "k8s", 80, 24, &pod(), &au).unwrap();
        assert!(o.approval.summary.contains("pod web-1") && o.approval.summary.contains("cluster dev"), "{}", o.approval.summary);
        assert!(lk(&sp.execs).is_empty(), "nothing before the yes");
        assert!(b.frame("alice", &o.id).unwrap_err().contains("waits for your yes"));
        b.decide("alice", &o.id, &o.approval.call, true, &au).unwrap();
        assert_eq!(lk(&sp.execs).as_slice(), [vec!["--context=dev", "exec", "-it", "--namespace=shop", "web-1", "--", "sh"]]);
        assert_eq!(sp.count.load(Ordering::SeqCst), 0, "not the person's own shell");
        b.text("alice", &o.id, "ls\n").unwrap();
        b.close("alice", &o.id, &au);
        let l = lk(&log).join("|");
        assert_eq!(l, "pty.asked k8s exec shop/web-1|pty.approved k8s|pty.close k8s");
        assert!(!l.contains("ls"));
        // a no starts nothing; a bad pod is refused before anything is asked; a failed start says why and leaves no session
        let o2 = b.open_exec("alice", "k8s", 80, 24, &pod(), &au).unwrap();
        b.decide("alice", &o2.id, &o2.approval.call, false, &au).unwrap();
        assert_eq!(lk(&sp.execs).len(), 1);
        assert!(b.open_exec("alice", "k8s", 80, 24, &t::PtyExec { pod: "a b".into(), ..pod() }, &au).is_err());
        assert!(b.open_exec("alice", "k8s", 0, 24, &pod(), &au).is_err());
        let b2 = book(Arc::new(Spawns { fail: true, ..Spawns::default() }));
        let o3 = b2.open_exec("alice", "k8s", 80, 24, &pod(), &au).unwrap();
        assert!(b2.decide("alice", &o3.id, &o3.approval.call, true, &au).unwrap_err().contains("no kubectl"));
        assert!(b2.frame("alice", &o3.id).unwrap_err().contains("not open"));
        // the same limit of sessions as every full terminal
        let b3 = book(Arc::new(Spawns::default()));
        for _ in 0..t::PTY_MAX_SESSIONS {
            b3.open_exec("alice", "k8s", 80, 24, &pod(), &au).unwrap();
        }
        assert!(b3.open_exec("alice", "k8s", 80, 24, &pod(), &au).unwrap_err().contains("at most"));
    }

    #[test]
    fn the_end_of_a_session_closes_every_terminal_of_that_person_and_no_one_elses_and_audits_each() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let log = Mutex::new(Vec::<String>::new());
        let au = |w: &str, word: &str| lk(&log).push(format!("{w} {word}"));
        let live = b.open("alice", "sample", 40, 10, &quiet).unwrap();
        b.decide("alice", &live.id, &live.approval.call, true, &quiet).unwrap();
        let asking = b.open("alice", "sample", 40, 10, &quiet).unwrap();
        let theirs = b.open("bob", "sample", 40, 10, &quiet).unwrap();
        b.decide("bob", &theirs.id, &theirs.approval.call, true, &quiet).unwrap();
        assert_eq!(b.close_user("alice", &au), 2, "the live shell and the question still waiting");
        assert!(b.frame("alice", &live.id).unwrap_err().contains("not open") && b.decide("alice", &asking.id, &asking.approval.call, true, &quiet).is_err());
        assert!(b.frame("bob", &theirs.id).is_ok(), "another person's terminal stays");
        assert!(*lk(&sp.done) == Some(129), "the live shell was hung up");
        assert_eq!(lk(&log).iter().filter(|l| l.as_str() == "pty.close sample").count(), 2);
        assert_eq!(b.close_user("alice", &au), 0, "nothing left");
    }

    #[test]
    fn a_session_that_is_not_a_pod_session_never_gets_kubectl_even_by_a_stale_call() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let o = b.open("alice", "sample", 40, 10, &quiet).unwrap();
        b.decide("alice", &o.id, &o.approval.call, true, &quiet).unwrap();
        assert_eq!((sp.count.load(Ordering::SeqCst), lk(&sp.execs).len()), (1, 0));
    }

    #[test]
    fn nothing_starts_before_the_yes_and_a_no_drops_the_session() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let log = Mutex::new(Vec::<String>::new());
        let au = |w: &str, word: &str| lk(&log).push(format!("{w} {word}"));
        let o = b.open("alice", "sample", 40, 10, &au).unwrap();
        assert!(o.approval.summary.contains("not checked line by line"));
        assert_eq!(sp.count.load(Ordering::SeqCst), 0, "no program before the answer");
        assert!(b.frame("alice", &o.id).unwrap_err().contains("waits for your yes"));
        assert!(b.key("alice", &o.id, PtyKey::Enter, PtyMods::default()).is_err());
        // another person, a wrong call, a wrong session: refused
        assert!(b.decide("bob", &o.id, &o.approval.call, true, &au).is_err());
        assert!(b.decide("alice", &o.id, "other", true, &au).is_err());
        assert!(b.decide("alice", "p999", &o.approval.call, true, &au).is_err());
        b.decide("alice", &o.id, &o.approval.call, false, &au).unwrap();
        assert_eq!(sp.count.load(Ordering::SeqCst), 0);
        assert!(b.frame("alice", &o.id).unwrap_err().contains("not open"));
        assert_eq!(lk(&log).as_slice(), ["pty.asked sample", "pty.denied sample"]);
    }

    #[test]
    fn after_the_yes_keys_text_resize_and_frames_reach_the_program_and_the_screen_comes_back() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let o = b.open("alice", "sample", 40, 10, &quiet).unwrap();
        b.decide("alice", &o.id, &o.approval.call, true, &quiet).unwrap();
        assert_eq!(sp.count.load(Ordering::SeqCst), 1);
        lk(&sp.out).extend_from_slice(b"\x1b[1;31mhi\x1b[0m there\r\n$ ");
        let f = b.frame("alice", &o.id).unwrap();
        assert_eq!((f.cols, f.rows), (40, 10));
        assert!(f.text().starts_with("hi there\n$"), "{:?}", f.text());
        assert_eq!(f.cells[0][0].fg, PtyColor::Indexed(1));
        assert_eq!(f.cells[0][0].attrs & 1, 1);
        assert_eq!(f.cursor, (2, 1));
        // keys become the bytes the program expects; text is pasted; ctrl+c is 0x03
        b.key("alice", &o.id, PtyKey::Char('l'), PtyMods::default()).unwrap();
        b.key("alice", &o.id, PtyKey::Up, PtyMods::default()).unwrap();
        b.key("alice", &o.id, PtyKey::Char('c'), PtyMods { ctrl: true, ..PtyMods::default() }).unwrap();
        b.text("alice", &o.id, "ls\n").unwrap();
        assert_eq!(lk(&sp.sent).as_slice(), b"l\x1b[A\x03ls\r", "a pasted newline is the Enter key");
        assert!(b.key("alice", &o.id, PtyKey::F(13), PtyMods::default()).is_err());
        assert!(b.key("alice", &o.id, PtyKey::Char('\u{7}'), PtyMods::default()).is_err());
        assert!(b.text("alice", &o.id, &"x".repeat(t::PTY_MAX_PASTE + 1)).is_err());
        // the size reaches the program and the frame
        b.resize("alice", &o.id, 60, 12).unwrap();
        assert_eq!(*lk(&sp.size), (60, 12));
        let f2 = b.frame("alice", &o.id).unwrap();
        assert_eq!((f2.cols, f2.rows), (60, 12));
        assert!(f2.seq != f.seq);
        assert!(b.resize("alice", &o.id, 0, 5).is_err() && b.resize("alice", &o.id, 5, t::PTY_MAX_ROWS + 1).is_err());
        // the same frame twice has the same sequence number
        assert_eq!(b.frame("alice", &o.id).unwrap().seq, b.frame("alice", &o.id).unwrap().seq);
        // another person has no access
        assert!(b.frame("bob", &o.id).is_err() && b.text("bob", &o.id, "x").is_err());
    }

    #[test]
    fn the_program_ending_is_shown_scrollback_works_and_close_audits_without_what_was_typed() {
        let sp = Arc::new(Spawns::default());
        let b = book(sp.clone());
        let log = Mutex::new(Vec::<String>::new());
        let au = |w: &str, word: &str| lk(&log).push(format!("{w} {word}"));
        let o = b.open("alice", "sample", 20, 3, &au).unwrap();
        b.decide("alice", &o.id, &o.approval.call, true, &au).unwrap();
        lk(&sp.out).extend_from_slice(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let f = b.frame("alice", &o.id).unwrap();
        assert_eq!((f.text().as_str(), f.history), ("three\nfour\nfive", 2));
        b.scroll("alice", &o.id, 2).unwrap();
        let back = b.frame("alice", &o.id).unwrap();
        assert_eq!((back.text().as_str(), back.back, back.cursor_visible), ("one\ntwo\nthree", 2, false));
        b.text("alice", &o.id, "SECRET-WORDS").unwrap();
        *lk(&sp.done) = Some(3);
        let ended = b.frame("alice", &o.id).unwrap();
        assert_eq!(ended.exited, Some(3));
        b.close("alice", &o.id, &au);
        b.close("alice", &o.id, &au);
        assert!(b.frame("alice", &o.id).is_err());
        let log = lk(&log).join("\n");
        assert_eq!(log, "pty.asked sample\npty.approved sample\npty.close sample");
        assert!(!log.contains("SECRET"));
    }

    #[test]
    fn a_person_may_hold_two_full_sessions_and_a_failed_start_says_so_and_leaves_nothing() {
        let b = book(Arc::new(Spawns::default()));
        let a = b.open("alice", "x", 10, 5, &quiet).unwrap();
        let _b2 = b.open("alice", "x", 10, 5, &quiet).unwrap();
        assert!(b.open("alice", "x", 10, 5, &quiet).unwrap_err().contains("at most 2"));
        assert!(b.open("bob", "x", 10, 5, &quiet).is_ok(), "limits are per person");
        assert!(b.open("bob", "x", 0, 5, &quiet).is_err());
        b.close("alice", &a.id, &quiet);
        assert!(b.open("alice", "x", 10, 5, &quiet).is_ok());
        let bad = book(Arc::new(Spawns { fail: true, ..Spawns::default() }));
        let o = bad.open("alice", "x", 10, 5, &quiet).unwrap();
        assert_eq!(bad.decide("alice", &o.id, &o.approval.call, true, &quiet).unwrap_err(), "no shell here");
        assert!(bad.frame("alice", &o.id).unwrap_err().contains("not open"));
    }

    /// A real shell on a real pseudo-terminal, as this test's own user (the system shell refuses root, so this test skips there).
    #[cfg(unix)]
    #[test]
    fn a_real_shell_runs_a_command_and_its_output_is_on_the_screen() {
        // SAFETY: geteuid(2) takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = std::env::temp_dir().join(format!("iyakki-pty-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let b = PtyBook::new(d.clone());
        let o = b.open("alice", "x", 80, 10, &quiet).unwrap();
        b.decide("alice", &o.id, &o.approval.call, true, &quiet).unwrap();
        // type only when the shell has shown its prompt: text typed before that is echoed by the terminal first and the prompt then lands on
        // the line of the answer ("$ pty-42"); a shell that never shows a prompt is typed at after the deadline
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < end && b.frame("alice", &o.id).unwrap().text().trim().is_empty() {
            std::thread::yield_now();
        }
        b.text("alice", &o.id, "echo pty-$((6*7))\n").unwrap();
        // the shell answers when it is ready: look at the screen until it says so (a failed run ends at the deadline)
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut seen = String::new();
        while std::time::Instant::now() < end {
            seen = b.frame("alice", &o.id).unwrap().text();
            if seen.lines().any(|l| l.trim() == "pty-42") {
                break;
            }
            std::thread::yield_now();
        }
        assert!(seen.lines().any(|l| l.trim() == "pty-42"), "{seen:?}");
        b.close("alice", &o.id, &quiet);
        let _ = std::fs::remove_dir_all(&d);
    }
}
