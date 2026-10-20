//! Runs the few programs the AI IDE needs, without a shell: `kubectl` (read-only views), `git init`, and the commands of a
//! project (its test and run commands, the terminal panel). Arguments are an array, the environment is cut down, time and
//! output are capped, and the working folder is the workspace.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// None when the program could not be started or was killed.
    pub code: Option<i32>,
    /// Standard output, then standard error, capped.
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    /// The program was not found.
    pub missing: bool,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
    pub fn combined(&self) -> String {
        if self.stderr.is_empty() { self.stdout.clone() } else if self.stdout.is_empty() { self.stderr.clone() } else { format!("{}\n{}", self.stdout, self.stderr) }
    }
}

/// Split `cargo test -p "my crate"` into words (single and double quotes group; no expansion of any kind).
pub fn split_command(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for c in s.chars() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                q = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Where a running program's output is copied as it arrives (for a live view); capped.
pub type Sink = Arc<Mutex<String>>;
const SINK_CAP: usize = 256 * 1024;

fn read_capped<R: Read + Send + 'static>(mut r: R, cap: usize, sink: Option<Sink>) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let (mut buf, mut chunk, mut cut) = (Vec::new(), [0u8; 8192], false);
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Some(sk) = &sink {
                        let mut g = sk.lock().unwrap_or_else(|e| e.into_inner());
                        if g.len() < SINK_CAP {
                            g.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        }
                    }
                    if buf.len() < cap {
                        let take = n.min(cap - buf.len());
                        buf.extend_from_slice(&chunk[..take]);
                        cut |= take < n;
                    } else {
                        cut = true;
                    }
                }
            }
        }
        (buf, cut)
    })
}

/// Run `argv` in `cwd`. `env` is added to a minimal environment (`PATH`, `HOME`, `LANG`); output is cut at `cap` bytes per stream.
pub fn run_argv(cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize) -> Out {
    run_argv_sink(cwd, argv, env, timeout, cap, None)
}

/// Start a program. A program written an instant ago (an installer's output, a script a test just made) can be refused with
/// "text file busy" while a sibling thread's fork still holds a copy of the descriptor it was written through, until that child
/// has exec'd (microseconds). That is not a failure of the program: ask again, without sleeping, until the other child has let go.
pub fn spawn_exec(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 20_000 => {
                tries += 1;
                std::thread::yield_now();
            }
            r => return r,
        }
    }
}

/// Like [`run_argv`], also copying the output into `sink` while the program runs.
pub fn run_argv_sink(cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<Sink>) -> Out {
    let Some(prog) = argv.first() else { return Out { stderr: "no command".into(), ..Default::default() } };
    let mut cmd = Command::new(prog);
    cmd.args(&argv[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env_clear();
    for k in ["PATH", "HOME", "LANG", "TERM", "USER", "TMPDIR", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = match spawn_exec(&mut cmd) {
        Ok(c) => c,
        Err(e) => return Out { stderr: format!("{prog}: {e}"), missing: e.kind() == std::io::ErrorKind::NotFound, ..Default::default() },
    };
    let (so, se) = (child.stdout.take().map(|r| read_capped(r, cap, sink.clone())), child.stderr.take().map(|r| read_capped(r, cap, sink.clone())));
    let start = Instant::now();
    let (mut code, mut timed_out) = (None, false);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                code = st.code();
                break;
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break,
        }
    }
    let (a, ca) = so.and_then(|h| h.join().ok()).unwrap_or_default();
    let (b, cb) = se.and_then(|h| h.join().ok()).unwrap_or_default();
    Out { code, stdout: String::from_utf8_lossy(&a).into_owned(), stderr: String::from_utf8_lossy(&b).into_owned(), timed_out, truncated: ca || cb, missing: false }
}

/// The one way the dev-cluster code starts programs (`kubectl`, `podman`, `docker`, `k3s`, `curl`): argument arrays, a
/// cut-down environment plus the `env` given, a timeout and an output cap. Tests use [`FakeRunner`].
pub trait Runner: Send + Sync {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<Sink>) -> Out;
    fn has(&self, program: &str) -> bool;
}

static MANAGED_K3S: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// The `k3s` program of the cluster IYAKKI manages on this machine (`k3sdev.rs`). While it is set and exists, a `kubectl` that is not on
/// the machine is run as `k3s kubectl` (k3s is one program that answers to both names), so nothing in IYAKKI needs a separate kubectl.
pub fn register_managed_k3s(p: Option<std::path::PathBuf>) {
    *MANAGED_K3S.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

pub fn managed_k3s() -> Option<std::path::PathBuf> {
    MANAGED_K3S.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|p| p.is_file())
}

/// `kubectl ...` as `k3s kubectl ...` when the machine has no kubectl and `k3s` is the managed cluster's program; None: run it as given.
pub fn kubectl_rewrite(argv: &[String], kubectl_on_path: bool, k3s: Option<&Path>) -> Option<Vec<String>> {
    if argv.first().map(String::as_str) != Some("kubectl") || kubectl_on_path {
        return None;
    }
    let k3s = k3s?;
    let mut a = vec![k3s.display().to_string(), "kubectl".to_string()];
    a.extend(argv[1..].iter().cloned());
    Some(a)
}

/// The real thing.
pub struct System;

impl Runner for System {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<Sink>) -> Out {
        // a kubeconfig held in a vault (`vault:x` / `uvault:x`) is written to a 0600 temp file for this one child and removed after
        let rewritten = kubectl_rewrite(argv, on_path("kubectl"), managed_k3s().as_deref());
        let argv = rewritten.as_deref().unwrap_or(argv);
        match crate::kubeprofiles::prepare(argv, env) {
            Ok(None) => run_argv_sink(cwd, argv, env, timeout, cap, sink),
            Ok(Some((a, e, _guards))) => run_argv_sink(cwd, &a, &e, timeout, cap, sink),
            Err(e) => Out { code: Some(1), stderr: e, ..Default::default() },
        }
    }
    fn has(&self, program: &str) -> bool {
        on_path(program) || (program == "kubectl" && managed_k3s().is_some())
    }
}

/// One call a [`FakeRunner`] saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
}

impl Call {
    pub fn line(&self) -> String {
        self.argv.join(" ")
    }
}

/// A scripted runner: the first rule whose text is contained in the joined command line answers; no rule means a failure.
pub struct FakeRunner {
    rules: Mutex<Vec<(String, Out)>>,
    calls: Mutex<Vec<Call>>,
    programs: Mutex<Vec<String>>,
}

impl FakeRunner {
    pub fn new(programs: &[&str]) -> FakeRunner {
        FakeRunner { rules: Mutex::new(vec![]), calls: Mutex::new(vec![]), programs: Mutex::new(programs.iter().map(|s| s.to_string()).collect()) }
    }
    pub fn answer(&self, contains: &str, stdout: &str) -> &Self {
        self.rules.lock().unwrap().push((contains.into(), Out { code: Some(0), stdout: stdout.into(), ..Default::default() }));
        self
    }
    pub fn fail(&self, contains: &str, code: i32, stderr: &str) -> &Self {
        self.rules.lock().unwrap().push((contains.into(), Out { code: Some(code), stderr: stderr.into(), ..Default::default() }));
        self
    }
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    pub fn lines(&self) -> Vec<String> {
        self.calls().iter().map(Call::line).collect()
    }
}

impl Runner for FakeRunner {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], _t: Duration, _cap: usize, sink: Option<Sink>) -> Out {
        self.calls.lock().unwrap().push(Call { argv: argv.to_vec(), env: env.to_vec(), cwd: cwd.display().to_string() });
        let line = argv.join(" ");
        let out = self.rules.lock().unwrap().iter().find(|(k, _)| line.contains(k.as_str())).map(|(_, o)| o.clone());
        let out = out.unwrap_or_else(|| Out { code: Some(1), stderr: format!("fake runner: no answer for `{line}`"), ..Default::default() });
        if let Some(sk) = sink {
            sk.lock().unwrap().push_str(&out.combined());
        }
        out
    }
    fn has(&self, program: &str) -> bool {
        self.programs.lock().unwrap().iter().any(|p| p == program)
    }
}

/// Is a program of this name on `PATH`?
pub fn on_path(name: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| !d.is_empty() && Path::new(d).join(name).is_file())
}

pub fn git_available() -> bool {
    on_path("git")
}

/// `git init` in a new workspace folder.
pub fn git_init(dir: &Path) -> Result<(), String> {
    if !git_available() {
        return Err("git is not installed".into());
    }
    let o = run_argv(dir, &["git".into(), "init".into(), "-q".into()], &[], Duration::from_secs(10), 4096);
    if o.ok() { Ok(()) } else { Err(o.combined()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_without_a_shell() {
        assert_eq!(split_command("cargo test -p \"my crate\" 'a b'"), vec!["cargo", "test", "-p", "my crate", "a b"]);
        assert_eq!(split_command("  x   y "), vec!["x", "y"]);
        assert_eq!(split_command("echo \"\""), vec!["echo", ""]);
        assert_eq!(split_command("echo $HOME; rm -rf /"), vec!["echo", "$HOME;", "rm", "-rf", "/"], "no expansion, no operators");
    }

    #[test]
    fn runs_with_caps_timeouts_and_missing_programs() {
        let d = std::env::temp_dir();
        let o = run_argv(&d, &["echo".into(), "hi".into()], &[], Duration::from_secs(5), 1000);
        assert!(o.ok() && o.stdout == "hi\n", "{o:?}");
        let o = run_argv(&d, &["sh".into(), "-c".into(), "yes | head -c 100000".into()], &[], Duration::from_secs(5), 1000);
        assert!(o.truncated && o.stdout.len() == 1000, "{}", o.stdout.len());
        let o = run_argv(&d, &["sleep".into(), "5".into()], &[], Duration::from_millis(100), 1000);
        assert!(o.timed_out && o.code.is_none());
        let o = run_argv(&d, &["no-such-program-xyz".into()], &[], Duration::from_secs(1), 10);
        assert!(o.missing, "{o:?}");
        let o = run_argv(&d, &["pwd".into()], &[], Duration::from_secs(2), 100);
        assert!(o.ok());
        let o = run_argv(&d, &["sh".into(), "-c".into(), "echo $SECRET_TOKEN".into()], &[], Duration::from_secs(2), 100);
        assert_eq!(o.stdout, "\n", "the environment is cut down");
    }

    #[test]
    fn the_process_kubeconfig_is_not_inherited_by_runner_children() {
        std::env::set_var("KUBECONFIG", "/home/someone/.kube/config");
        let o = run_argv(&std::env::temp_dir(), &["sh".into(), "-c".into(), "echo [$KUBECONFIG]".into()], &[("KUBECONFIG".into(), "/etc/rancher/k3s/k3s.yaml".into())], Duration::from_secs(5), 4096);
        let none = run_argv(&std::env::temp_dir(), &["sh".into(), "-c".into(), "echo [$KUBECONFIG]".into()], &[], Duration::from_secs(5), 4096);
        std::env::remove_var("KUBECONFIG");
        assert_eq!(o.stdout.trim(), "[/etc/rancher/k3s/k3s.yaml]");
        assert_eq!(none.stdout.trim(), "[]", "no inherited cluster");
    }
}
