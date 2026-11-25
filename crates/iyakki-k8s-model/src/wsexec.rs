//! The answer of a program the dev-cluster code started (`Out`), where its output is copied (`Sink`), the one way programs are started (the `Runner`
//! trait) and the scripted `FakeRunner` of the tests; the runner that really starts programs stays in `iyakki-shared::wsexec`, which re-exports these.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// The program ended but its exit status was taken by another reaper (the PID-1 one): `code` is unknown, the output is complete.
    pub status_lost: bool,
}

impl Out {
    pub fn ok(&self) -> bool {
        // a lost status is not a failure by itself: the output is complete; an error text says it did not work
        self.code == Some(0) || (self.status_lost && !self.timed_out && self.stderr.trim().is_empty())
    }
    pub fn combined(&self) -> String {
        if self.stderr.is_empty() { self.stdout.clone() } else if self.stdout.is_empty() { self.stderr.clone() } else { format!("{}\n{}", self.stdout, self.stderr) }
    }
}

/// Where a running program's output is copied as it arrives (for a live view); capped.
pub type Sink = Arc<Mutex<String>>;

/// The one way the dev-cluster code starts programs (`kubectl`, `podman`, `docker`, `k3s`, `curl`): argument arrays, a
/// cut-down environment plus the `env` given, a timeout and an output cap. Tests use [`FakeRunner`].
pub trait Runner: Send + Sync {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<Sink>) -> Out;
    fn has(&self, program: &str) -> bool;
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
        self.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((contains.into(), Out { code: Some(0), stdout: stdout.into(), ..Default::default() }));
        self
    }
    pub fn fail(&self, contains: &str, code: i32, stderr: &str) -> &Self {
        self.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((contains.into(), Out { code: Some(code), stderr: stderr.into(), ..Default::default() }));
        self
    }
    /// Makes a program count as installed.
    pub fn programs_add(&self, p: &str) {
        self.programs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(p.to_string());
    }
    /// Forgets every scripted answer (a test changes what the world says).
    pub fn forget(&self) {
        self.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    pub fn lines(&self) -> Vec<String> {
        self.calls().iter().map(Call::line).collect()
    }
}

impl Runner for FakeRunner {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], _t: Duration, _cap: usize, sink: Option<Sink>) -> Out {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Call { argv: argv.to_vec(), env: env.to_vec(), cwd: cwd.display().to_string() });
        let line = argv.join(" ");
        let out = self.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| line.contains(k.as_str())).map(|(_, o)| o.clone());
        let out = out.unwrap_or_else(|| Out { code: Some(1), stderr: format!("fake runner: no answer for `{line}`"), ..Default::default() });
        if let Some(sk) = sink {
            sk.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push_str(&out.combined());
        }
        out
    }
    fn has(&self, program: &str) -> bool {
        self.programs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|p| p == program)
    }
}
