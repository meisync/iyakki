//! The person's vault of one-time codes and the two terminals, served over the UI socket (F-SPLIT-03 part 3, D-SPLIT-03-SERVED).
//!
//! * `code_vault(user)`: the book of one-time codes with every secret hidden (`*`): the UI sees names, groups and order, the core computes the
//!   codes (`codes_now`), and a save that carries `*` keeps what the core holds for that entry (only a secret the person typed is written).
//! * `terminal(user, policy, host)`: the line terminal of the CLI tile. The policy the UI named is ignored (the account's own is read in the core,
//!   the UI cannot ask for a bigger one), the host is the core's own, and a long SSH command is run by the core on a thread and polled.
//! * `core_terminal()`: the terminal of the core contract with its whole-session pty: the core keeps the screen, the UI pulls frames (a frame
//!   that has not changed since the one the UI has is not sent again) and sends keys and text.
//!
//! An answer that may not fit a line (a screen, 64 KiB of output) comes inline when small and as a download object when not.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use iyakki_agent_api::{Terminal, TerminalCall};
use iyakki_core_api::terminal::{PtyFrame, PtyKey, PtyMods, PtyOpen, TermOpen, TermReply};
use iyakki_shared::authcodes::{Book, CodeVault, VaultProblem};
use iyakki_shared::cli::{Action, CliOut};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::served::{call, Served, NOT_SERVED};
use crate::served_hosts::{down_bytes, read_all};
use crate::{de, ser, Obj, Registry, RemoteServicesHandle, Rpc, ServicesRoot};

/// What stands in the place of a secret on the wire.
pub const HIDDEN: &str = "*";
/// An answer longer than this goes as a download object (a line is at most 64 KiB).
const INLINE_MAX: usize = 40_000;

/// An answer that may be long: inline when it fits a line, else a download object the UI reads.
fn big<T: serde::Serialize>(reg: &Registry<'_>, v: &T) -> Result<Value, String> {
    let v = ser(v)?;
    let text = v.to_string();
    if text.len() <= INLINE_MAX {
        Ok(json!({ "inline": v }))
    } else {
        Ok(json!({ "download": down_bytes(reg, text.into_bytes()) }))
    }
}

fn call_big<T: DeserializeOwned>(rpc: &Rpc, iface: &str, obj: u64, method: &str, args: Value) -> Result<T, String> {
    let r: Value = rpc.call(iface, obj, method, args)?;
    if let Some(v) = r.get("inline") {
        return serde_json::from_value(v.clone()).map_err(|e| format!("the answer to {iface}::{method} has the wrong shape: {e}"));
    }
    let id = r.get("download").and_then(Value::as_u64).ok_or_else(|| format!("the answer to {iface}::{method} has neither a value nor a download"))?;
    serde_json::from_slice(&read_all(rpc, id)?).map_err(|e| format!("the answer to {iface}::{method} has the wrong shape: {e}"))
}

// ---------------------------------------------------------------------------------------------------------------- the core's end

pub(crate) fn serve_person(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: Value) -> Option<Result<Value, String>> {
    let t: &dyn iyakki_agent_api::ServicesHandle = root.svc.as_ref();
    Some(match method {
        "code_vault" => de::<(String,)>(a).and_then(|(user,)| ser(&reg.add(Obj::Served(Arc::new(CodeVaultObj { svc: root.svc.clone(), user }))))),
        "terminal" => (|| {
            let (user,): (String,) = de(a)?;
            let accounts = crate::served::ServedParts::need(&root.parts.accounts, "accounts")?;
            let host = crate::served::ServedParts::need(&root.parts.host, "host")?;
            let policy = accounts.get_public(&user).ok_or_else(|| format!("{NOT_SERVED}: no account {user}"))?.policy();
            let term = t.terminal(&user, policy, host);
            ser(&reg.add(Obj::Served(Arc::new(TerminalObj(Mutex::new(term))))))
        })(),
        "core_terminal" => crate::served::ServedParts::need(&root.parts.core_terminal, "core terminal").and_then(|c| ser(&reg.add(Obj::Served(Arc::new(CoreTerminalObj(c)))))),
        _ => return None,
    })
}

/// The book of codes of one account; each call opens the real vault again, so a locked vault answers locked and nothing is kept here.
struct CodeVaultObj {
    svc: Arc<dyn iyakki_agent_api::ServicesHandle>,
    user: String,
}
impl CodeVaultObj {
    fn vault(&self) -> Box<dyn CodeVault> {
        self.svc.code_vault(&self.user)
    }
}
impl Served for CodeVaultObj {
    fn iface(&self) -> &'static str {
        "CodeVault"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "load" => {
                let loaded = self.vault().load().map(|mut b| {
                    for e in b.entries.iter_mut() {
                        e.secret = HIDDEN.to_string();
                    }
                    b
                });
                big(reg, &loaded)
            }
            "save" => {
                let (mut book,): (Book,) = de(a)?;
                let v = self.vault();
                ser(&(|| -> Result<(), VaultProblem> {
                    if book.entries.iter().any(|e| e.secret == HIDDEN) {
                        let now = v.load()?;
                        for e in book.entries.iter_mut().filter(|e| e.secret == HIDDEN) {
                            match now.entry(e.id) {
                                Some(kept) => e.secret = kept.secret.clone(),
                                None => return Err(VaultProblem::Other("an entry with a hidden secret is not in the core's book".into())),
                            }
                        }
                    }
                    v.save(&book)
                })())
            }
            "codes_now" => {
                let (unix,): (i64,) = de(a)?;
                ser(&self.vault().load().map(|b| b.entries.iter().filter_map(|e| e.code_at(unix).map(|c| (e.id, c))).collect::<Vec<(u64, String)>>()))
            }
            other => Err(format!("CodeVault has no call {other}")),
        }
    }
}

struct TerminalObj(Mutex<Box<dyn Terminal>>);
impl Served for TerminalObj {
    fn iface(&self) -> &'static str {
        "Terminal"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let mut t = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match method {
            "prompt" => ser(&t.prompt()),
            "full" => ser(&t.full()),
            "exec" => {
                let (line,): (String,) = de(a)?;
                let out = t.exec(&line);
                big(reg, &(out.text, out.action))
            }
            "ssh_call" => {
                let (line,): (String,) = de(a)?;
                ser(&t.ssh_call(&line).map(|c| (c.command().to_string(), reg.add(Obj::Served(Arc::new(CallObj::new(c)))))))
            }
            other => Err(format!("Terminal has no call {other}")),
        }
    }
}

/// What a running SSH command has said and how it ended.
#[derive(Default)]
struct Progress {
    lines: Vec<String>,
    done: Option<Result<String, String>>,
}
/// One SSH command of the terminal: started on a thread of the core, its lines pulled by the UI as they come.
struct CallObj {
    call: Mutex<Option<Box<dyn TerminalCall>>>,
    state: Arc<(Mutex<Progress>, Condvar)>,
}
impl CallObj {
    fn new(c: Box<dyn TerminalCall>) -> CallObj {
        CallObj { call: Mutex::new(Some(c)), state: Arc::new((Mutex::new(Progress::default()), Condvar::new())) }
    }
}
impl Served for CallObj {
    fn iface(&self) -> &'static str {
        "TerminalCall"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, _a: Value) -> Result<Value, String> {
        match method {
            "start" => {
                let call = self.call.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or("that command was started already")?;
                let state = self.state.clone();
                std::thread::spawn(move || {
                    let sink_state = state.clone();
                    let sink: iyakki_shared::exec::Sink = Arc::new(move |l| {
                        let (m, cv) = &*sink_state;
                        m.lock().unwrap_or_else(PoisonError::into_inner).lines.push(l.to_string());
                        cv.notify_all();
                    });
                    let r = call.run(Some(sink));
                    let (m, cv) = &*state;
                    m.lock().unwrap_or_else(PoisonError::into_inner).done = Some(r);
                    cv.notify_all();
                });
                ser(&true)
            }
            "poll" => {
                let (m, cv) = &*self.state;
                let g = m.lock().unwrap_or_else(PoisonError::into_inner);
                // waits for news (a line or the end), at most a quarter of a second, so a quiet command does not hold the line
                let (mut g, _) = cv.wait_timeout_while(g, Duration::from_millis(250), |p| p.lines.is_empty() && p.done.is_none()).unwrap_or_else(PoisonError::into_inner);
                let lines = std::mem::take(&mut g.lines);
                let done = if g.lines.is_empty() { g.done.clone() } else { None };
                ser(&(lines, done))
            }
            other => Err(format!("TerminalCall has no call {other}")),
        }
    }
}

struct CoreTerminalObj(Arc<dyn iyakki_core_api::Terminal>);
impl Served for CoreTerminalObj {
    fn iface(&self) -> &'static str {
        "CoreTerminal"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let t = &self.0;
        match method {
            "open" => {
                let (u, app): (String, String) = de(a)?;
                ser(&t.open(&u, &app))
            }
            "run" => {
                let (u, s, line): (String, String, String) = de(a)?;
                big(reg, &t.run(&u, &s, &line))
            }
            "decide" => {
                let (u, s, c, yes): (String, String, String, bool) = de(a)?;
                big(reg, &t.decide(&u, &s, &c, yes))
            }
            "close" => {
                let (u, s): (String, String) = de(a)?;
                {
                    t.close(&u, &s);
                    ser(&true)
                }
            }
            "pty_open" => {
                let (u, app, cols, rows): (String, String, usize, usize) = de(a)?;
                ser(&t.pty_open(&u, &app, cols, rows))
            }
            "pty_open_exec" => {
                let (u, app, cols, rows, exec): (String, String, usize, usize, iyakki_core_api::terminal::PtyExec) = de(a)?;
                ser(&t.pty_open_exec(&u, &app, cols, rows, &exec))
            }
            "pty_decide" => {
                let (u, s, c, yes): (String, String, String, bool) = de(a)?;
                ser(&t.pty_decide(&u, &s, &c, yes))
            }
            "pty_key" => {
                let (u, s, k, m): (String, String, PtyKey, PtyMods) = de(a)?;
                ser(&t.pty_key(&u, &s, k, m))
            }
            "pty_text" => {
                let (u, s, text): (String, String, String) = de(a)?;
                ser(&t.pty_text(&u, &s, &text))
            }
            "pty_resize" => {
                let (u, s, cols, rows): (String, String, usize, usize) = de(a)?;
                ser(&t.pty_resize(&u, &s, cols, rows))
            }
            "pty_scroll" => {
                let (u, s, lines): (String, String, isize) = de(a)?;
                ser(&t.pty_scroll(&u, &s, lines))
            }
            "pty_frame" => {
                let (u, s, after): (String, String, Option<u64>) = de(a)?;
                // a frame the UI already has is not sent again
                match t.pty_frame(&u, &s) {
                    Ok(f) if Some(f.seq) == after => big(reg, &Ok::<Option<PtyFrame>, String>(None)),
                    other => big(reg, &other.map(Some)),
                }
            }
            "pty_close" => {
                let (u, s): (String, String) = de(a)?;
                {
                    t.pty_close(&u, &s);
                    ser(&true)
                }
            }
            other => Err(format!("CoreTerminal has no call {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

impl RemoteServicesHandle {
    /// The person's book of one-time codes: names and order from the core, the secrets stay there.
    pub(crate) fn m_code_vault(&self, user: &str) -> Box<dyn CodeVault> {
        let obj = self.rpc.call::<u64>("ServicesHandle", 0, "code_vault", json!([user])).unwrap_or(u64::MAX);
        Box::new(RemoteCodeVault { rpc: self.rpc.clone(), obj })
    }

    /// The line terminal of the CLI tile: the core's own host and the account's own policy stand in for the ones the UI named.
    pub(crate) fn m_terminal(&self, user: &str, _policy: iyakki_sec::policy::Policy, _host: Arc<dyn iyakki_core_api::HostApi>) -> Box<dyn Terminal> {
        let obj = self.rpc.call::<u64>("ServicesHandle", 0, "terminal", json!([user])).unwrap_or(u64::MAX);
        Box::new(RemoteTerminal { rpc: self.rpc.clone(), obj })
    }

    /// The terminal of the core contract (sessions and whole-session pty), as a remote object.
    pub fn core_terminal(&self) -> Arc<RemoteCoreTerminal> {
        let obj = self.rpc.call::<u64>("ServicesHandle", 0, "core_terminal", json!([])).unwrap_or(u64::MAX);
        Arc::new(RemoteCoreTerminal { rpc: self.rpc.clone(), obj })
    }
}

/// The book as the UI sees it. `codes_now` is the one way to see a code: the secret is never here.
pub struct RemoteCodeVault {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteCodeVault {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl CodeVault for RemoteCodeVault {
    fn load(&self) -> Result<Book, VaultProblem> {
        call_big::<Result<Book, VaultProblem>>(&self.rpc, "CodeVault", self.obj, "load", json!([])).unwrap_or_else(|e| Err(VaultProblem::Other(e)))
    }
    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        call(&self.rpc, "CodeVault", self.obj, "save", json!([book]))
    }
    fn codes_now(&self, unix: i64) -> Option<Vec<(u64, String)>> {
        self.rpc.call::<Result<Vec<(u64, String)>, VaultProblem>>("CodeVault", self.obj, "codes_now", json!([unix])).ok()?.ok()
    }
}

struct RemoteTerminal {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteTerminal {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl Terminal for RemoteTerminal {
    fn prompt(&self) -> String {
        self.rpc.call::<String>("Terminal", self.obj, "prompt", json!([])).unwrap_or_else(|_| "$ ".into())
    }
    fn exec(&mut self, line: &str) -> CliOut {
        match call_big::<(String, Action)>(&self.rpc, "Terminal", self.obj, "exec", json!([line])) {
            Ok((text, action)) => CliOut { text, action },
            Err(e) => CliOut { text: e, action: Action::None },
        }
    }
    fn ssh_call(&self, line: &str) -> Option<Box<dyn TerminalCall>> {
        let (command, obj) = self.rpc.call::<Option<(String, u64)>>("Terminal", self.obj, "ssh_call", json!([line])).ok().flatten()?;
        Some(Box::new(RemoteCall { rpc: self.rpc.clone(), obj, command }))
    }
    fn full(&self) -> bool {
        self.rpc.call::<bool>("Terminal", self.obj, "full", json!([])).unwrap_or(false)
    }
}

struct RemoteCall {
    rpc: Rpc,
    obj: u64,
    command: String,
}
impl Drop for RemoteCall {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl TerminalCall for RemoteCall {
    fn command(&self) -> &str {
        &self.command
    }
    fn run(&self, sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String> {
        self.rpc.call::<bool>("TerminalCall", self.obj, "start", json!([]))?;
        loop {
            let (lines, done): (Vec<String>, Option<Result<String, String>>) = self.rpc.call("TerminalCall", self.obj, "poll", json!([]))?;
            if let Some(s) = &sink {
                for l in &lines {
                    s(l);
                }
            }
            if let Some(r) = done {
                return r;
            }
        }
    }
}

pub struct RemoteCoreTerminal {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteCoreTerminal {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl RemoteCoreTerminal {
    fn c<T: DeserializeOwned + crate::Lost>(&self, m: &str, args: Value) -> T {
        call(&self.rpc, "CoreTerminal", self.obj, m, args)
    }
    fn b<T: DeserializeOwned>(&self, m: &str, args: Value) -> Result<T, String> {
        call_big::<Result<T, String>>(&self.rpc, "CoreTerminal", self.obj, m, args)?
    }
}
impl iyakki_core_api::Terminal for RemoteCoreTerminal {
    fn open(&self, user: &str, app: &str) -> Result<TermOpen, String> {
        self.c("open", json!([user, app]))
    }
    fn run(&self, user: &str, session: &str, line: &str) -> Result<TermReply, String> {
        self.b("run", json!([user, session, line]))
    }
    fn decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<TermReply, String> {
        self.b("decide", json!([user, session, call, approve]))
    }
    fn close(&self, user: &str, session: &str) {
        self.c::<bool>("close", json!([user, session]));
    }
    fn pty_open(&self, user: &str, app: &str, cols: usize, rows: usize) -> Result<PtyOpen, String> {
        self.c("pty_open", json!([user, app, cols, rows]))
    }
    fn pty_open_exec(&self, user: &str, app: &str, cols: usize, rows: usize, exec: &iyakki_core_api::terminal::PtyExec) -> Result<PtyOpen, String> {
        self.c("pty_open_exec", json!([user, app, cols, rows, exec]))
    }
    fn pty_decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<(), String> {
        self.c("pty_decide", json!([user, session, call, approve]))
    }
    fn pty_key(&self, user: &str, session: &str, key: PtyKey, mods: PtyMods) -> Result<(), String> {
        self.c("pty_key", json!([user, session, key, mods]))
    }
    fn pty_text(&self, user: &str, session: &str, text: &str) -> Result<(), String> {
        self.c("pty_text", json!([user, session, text]))
    }
    fn pty_resize(&self, user: &str, session: &str, cols: usize, rows: usize) -> Result<(), String> {
        self.c("pty_resize", json!([user, session, cols, rows]))
    }
    fn pty_scroll(&self, user: &str, session: &str, lines: isize) -> Result<(), String> {
        self.c("pty_scroll", json!([user, session, lines]))
    }
    fn pty_frame(&self, user: &str, session: &str) -> Result<PtyFrame, String> {
        self.b::<Option<PtyFrame>>("pty_frame", json!([user, session, Option::<u64>::None]))?.ok_or_else(|| "the core sent no frame".to_string())
    }
    fn pty_close(&self, user: &str, session: &str) {
        self.c::<bool>("pty_close", json!([user, session]));
    }
}
impl RemoteCoreTerminal {
    /// The screen of a session if it changed since the one with sequence number `after` (`None`: send it whatever it is). The stream of a
    /// session is this call repeated: an unchanged screen costs one small answer.
    pub fn frame_if_changed(&self, user: &str, session: &str, after: Option<u64>) -> Result<Option<PtyFrame>, String> {
        self.b::<Option<PtyFrame>>("pty_frame", json!([user, session, after]))
    }
}

// ---------------------------------------------------------------------------------------------------------------- seams

/// A line terminal as an object the core can serve as the root of the trait `Terminal` (the composition root and the tests use it; the
/// accessor `terminal` builds the same object from the real services).
pub fn serve_terminal(t: Box<dyn Terminal>) -> Obj {
    Obj::Served(Arc::new(TerminalObj(Mutex::new(t))))
}

/// A core terminal as an object the core can serve as the root of the trait `CoreTerminal`.
pub fn serve_core_terminal(t: Arc<dyn iyakki_core_api::Terminal>) -> Obj {
    Obj::Served(Arc::new(CoreTerminalObj(t)))
}

/// The UI's end of a served line terminal (`obj` 0 is a root).
pub fn remote_terminal(rpc: &Rpc, obj: u64) -> Box<dyn Terminal> {
    Box::new(RemoteTerminal { rpc: rpc.clone(), obj })
}

/// The UI's end of a served core terminal.
pub fn remote_core_terminal(rpc: &Rpc, obj: u64) -> Arc<RemoteCoreTerminal> {
    Arc::new(RemoteCoreTerminal { rpc: rpc.clone(), obj })
}
