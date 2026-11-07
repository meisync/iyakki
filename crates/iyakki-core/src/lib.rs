//! IYAKKI supervisor core: host detection, logging, signals, agent task running.

pub mod boot;
pub mod bootconfirm;
pub mod bootlog;
pub use iyakki_storage::crashlog;
pub mod coldplug;
pub mod homebind;
pub mod display;
pub mod camera;
pub mod hotplug;
pub mod storageloss;
pub mod web;
pub mod console;
pub mod keepalive;
#[cfg(feature = "shell")]
pub mod host;
#[cfg(feature = "shell")]
pub mod browsertest;
#[cfg(feature = "shell")]
pub mod svc;
pub mod net;
pub mod cloudnode;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_sec::{Capability, CapabilityStore};
use iyakki_vfs::MemFs;
use iyakki_wasm::{RunOutcome, Sandbox, SandboxConfig, WasmError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostEnv {
    Docker,
    MicroVm,
    BareMetal,
}

impl std::fmt::Display for HostEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HostEnv::Docker => "docker",
            HostEnv::MicroVm => "microvm",
            HostEnv::BareMetal => "baremetal",
        })
    }
}

/// Pure classifier so it can be unit tested without touching /proc.
pub fn classify_env(cmdline: &str, cgroup: &str, has_dockerenv: bool, sys_vendor: &str) -> HostEnv {
    // Explicit override on the kernel command line wins: `iyakki.env=docker|microvm|baremetal`.
    for tok in cmdline.split_whitespace() {
        match tok.strip_prefix("iyakki.env=") {
            Some("docker") => return HostEnv::Docker,
            Some("microvm") => return HostEnv::MicroVm,
            Some("baremetal") => return HostEnv::BareMetal,
            _ => {}
        }
    }
    if has_dockerenv || ["docker", "kubepods", "containerd", "/lxc/"].iter().any(|m| cgroup.contains(m)) {
        return HostEnv::Docker;
    }
    let v = sys_vendor.to_ascii_lowercase();
    if cmdline.contains("firecracker") || v.contains("firecracker") || v.contains("cloud hypervisor") || v.contains("qemu") {
        return HostEnv::MicroVm;
    }
    HostEnv::BareMetal
}

pub fn detect_env() -> HostEnv {
    // Orchestrators set these in every container, even where cgroup/dockerenv hints are absent (cgroup v2, containerd).
    if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() || std::env::var_os("container").is_some() {
        return HostEnv::Docker;
    }
    let rd = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    classify_env(
        &rd("/proc/cmdline"),
        &rd("/proc/1/cgroup"),
        std::path::Path::new("/.dockerenv").exists(),
        &rd("/sys/class/dmi/id/sys_vendor"),
    )
}

struct StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let line = console::format_log(r.level(), r.args());
            // the kernel log reaches every console (including the screen); stderr is best effort and never panics
            if !console::kmsg(console::level_to_kernel(r.level()), &line) {
                console::err_write(&format!("{line}\n"));
            }
        }
    }
    fn flush(&self) {}
}

/// Install the logger (kernel log, falling back to stderr). Level from `IYAKKI_LOG` (error|warn|info|debug|trace), default info.
pub fn init_logger() {
    let lvl = std::env::var("IYAKKI_LOG").ok().and_then(|s| s.parse().ok()).unwrap_or(log::LevelFilter::Info);
    if log::set_logger(&StderrLogger).is_ok() {
        log::set_max_level(lvl);
    }
}

static SHUTDOWN: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

extern "C" fn on_shutdown_signal(_sig: libc::c_int) {
    // only an atomic load and store: both are async-signal-safe
    if let Some(f) = SHUTDOWN.get() {
        f.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Register SIGTERM/SIGINT; returns a flag that flips to true on shutdown request.
/// Calling it again returns the same flag.
pub fn install_signal_handlers() -> std::io::Result<Arc<AtomicBool>> {
    let flag = SHUTDOWN.get_or_init(|| Arc::new(AtomicBool::new(false))).clone();
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: `act` is fully initialised (zeroed, then handler/flags/mask set) and outlives the call.
        let rc = unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_shutdown_signal as extern "C" fn(libc::c_int) as usize;
            act.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut act.sa_mask);
            libc::sigaction(sig, &act, std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(flag)
}

/// As PID 1 we must reap orphaned children or they pile up as zombies.
pub fn reap_children() {
    // SAFETY: waitpid with WNOHANG on any child is always safe to call.
    unsafe {
        while libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) > 0 {}
    }
}

/// What an agent task is allowed to do.
#[derive(Clone, Debug, Default)]
pub struct Grants {
    pub read: Vec<String>,
    pub write: Vec<String>,
    /// Hosts the task may reach (only effective if the supervisor has a network backend).
    pub net: Vec<String>,
}

pub struct Supervisor {
    pub env: HostEnv,
    pub vfs: Arc<Mutex<MemFs>>,
    pub caps: CapabilityStore,
    sandbox: Sandbox,
}

impl Supervisor {
    pub fn new(env: HostEnv, cfg: SandboxConfig) -> Result<Self, WasmError> {
        Ok(Self {
            env,
            vfs: Arc::new(Mutex::new(MemFs::new())),
            caps: CapabilityStore::new(),
            sandbox: Sandbox::new(cfg)?,
        })
    }

    /// Route guest network calls through `net` (normally an `iyakki_proxy::EgressProxy`).
    pub fn with_net(mut self, net: Arc<dyn iyakki_wasm::NetBackend>) -> Self {
        self.sandbox = self.sandbox.clone().with_net(net);
        self
    }

    /// Run a guest task: issue its tokens, execute, then revoke them.
    pub fn run_task(&self, wasm: &[u8], entry: &str, grants: &Grants) -> Result<RunOutcome, WasmError> {
        let mut tokens = Vec::new();
        for p in &grants.read {
            tokens.push(self.caps.issue(Capability::FsRead(p.clone())).map_err(|e| WasmError::Load(e.to_string()))?);
        }
        for p in &grants.write {
            tokens.push(self.caps.issue(Capability::FsWrite(p.clone())).map_err(|e| WasmError::Load(e.to_string()))?);
        }
        for h in &grants.net {
            tokens.push(self.caps.issue(Capability::NetConnect(h.to_ascii_lowercase())).map_err(|e| WasmError::Load(e.to_string()))?);
        }
        let res = self.sandbox.run(wasm, entry, self.vfs.clone(), self.caps.clone(), tokens.clone());
        for t in &tokens {
            self.caps.revoke(t);
        }
        res
    }

    /// Idle loop for PID 1 / daemon mode until a shutdown signal arrives.
    pub fn serve(&self, shutdown: &AtomicBool) {
        log::info!("supervisor up (env={})", self.env);
        // the kernel's `quiet` hides log lines; this one still reaches the console (serial or screen) for people and tests
        console::out_write(&format!("IYAKKI-BOOT supervisor up (env={})\n", self.env));
        while !shutdown.load(Ordering::Relaxed) {
            reap_children();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        log::info!("shutdown requested, exiting");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sigterm_sets_the_shutdown_flag_and_the_flag_is_shared() {
        let f = super::install_signal_handlers().unwrap();
        assert!(!f.load(std::sync::atomic::Ordering::SeqCst));
        // SAFETY: raising a signal that we just installed a handler for.
        unsafe { libc::raise(libc::SIGTERM) };
        assert!(f.load(std::sync::atomic::Ordering::SeqCst), "the handler ran and set the flag");
        assert!(std::sync::Arc::ptr_eq(&f, &super::install_signal_handlers().unwrap()), "one flag per process");
    }

    use super::*;

    #[test]
    fn env_classification() {
        assert_eq!(classify_env("quiet iyakki.env=microvm", "", true, ""), HostEnv::MicroVm);
        assert_eq!(classify_env("", "0::/docker/abc", false, ""), HostEnv::Docker);
        assert_eq!(classify_env("", "", true, ""), HostEnv::Docker);
        assert_eq!(classify_env("console=ttyS0 firecracker", "", false, ""), HostEnv::MicroVm);
        assert_eq!(classify_env("", "0::/", false, "Dell Inc."), HostEnv::BareMetal);
    }

    #[test]
    fn supervisor_runs_task_and_revokes_tokens() {
        let sup = Supervisor::new(HostEnv::BareMetal, SandboxConfig::default()).unwrap();
        let wat = r#"
        (module
          (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
          (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 64) "/ws/a")
          (data (i32.const 128) "ok")
          (func (export "main") (result i32)
            (drop (call $tg (i32.const 0) (i32.const 0)))
            (call $fw (i32.const 0) (i32.const 64) (i32.const 5) (i32.const 128) (i32.const 2))))
        "#;
        let g = Grants { write: vec!["/ws".into()], ..Default::default() };
        let out = sup.run_task(wat.as_bytes(), "main", &g).unwrap();
        assert_eq!(out.result, Some(0));
        assert_eq!(sup.vfs.lock().unwrap().read("/ws/a").unwrap(), b"ok");
    }
}
