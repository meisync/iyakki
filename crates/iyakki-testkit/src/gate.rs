//! Stub child processes that live exactly as long as the test needs. Never `sleep N` stubs: a child that lives for N seconds is gone when
//! a loaded machine makes the test slower than N, and a child that lives longer slows the test down.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Kills and reaps a child when dropped (also when the test panics).
pub struct ChildGuard(pub Option<Child>);

impl ChildGuard {
    /// The child's process id.
    pub fn id(&self) -> u32 {
        self.0.as_ref().map(|c| c.id()).unwrap_or(0)
    }
    /// Has it ended? (`Some(success)`)
    pub fn try_finished(&mut self) -> Option<bool> {
        self.0.as_mut().and_then(|c| c.try_wait().ok().flatten()).map(|s| s.success())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// A child that stays alive until [`GatedChild::release`] (it reads its standard input until that is closed) and is reaped on drop.
pub struct GatedChild {
    child: ChildGuard,
    stdin: Option<std::process::ChildStdin>,
}

impl GatedChild {
    /// Starts `cat` as the stub: it lives until the gate is released.
    pub fn start() -> GatedChild {
        GatedChild::start_command(Command::new("cat"))
    }

    /// Starts a prepared command (it must read its standard input to the end before it exits, like `cat` or a script's `cat >/dev/null`).
    pub fn start_command(mut cmd: Command) -> GatedChild {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("testkit: start the gated child");
        let stdin = child.stdin.take();
        GatedChild { child: ChildGuard(Some(child)), stdin }
    }

    /// The child's process id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Is it still running?
    pub fn alive(&mut self) -> bool {
        self.child.try_finished().is_none()
    }

    /// Lets the child end (closes its input) and waits for it, up to the default budget.
    pub fn release(&mut self) {
        if let Some(mut s) = self.stdin.take() {
            let _ = s.flush();
        }
        let budget = crate::wait::BUDGET;
        let c = &mut self.child;
        crate::wait::wait_until(budget, || c.try_finished().is_some());
    }
}

/// A shell snippet for stub SCRIPTS (a fake `kubectl` that must "run for a while"): it waits until `gate` exists. The test creates the gate
/// file when the script may end. Example: `format!("{}\necho done", wait_gate_snippet(&gate))`.
pub fn wait_gate_snippet(gate: &Path) -> String {
    format!("until [ -e '{}' ]; do sleep 0.02; done", gate.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gated_child_lives_until_released_and_is_reaped() {
        let mut g = GatedChild::start();
        assert!(g.alive());
        g.release();
        assert!(!g.alive());
        let pid;
        {
            let g2 = GatedChild::start();
            pid = g2.id();
        }
        // after the drop the process is gone (reaped)
        crate::wait::wait_until(crate::wait::BUDGET, || !Path::new(&format!("/proc/{pid}")).exists() || !Path::new("/proc/self").exists());
    }
}
