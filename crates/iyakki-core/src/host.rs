//! The running system as seen by the shell's apps (`iyakki_agent::HostApi`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use iyakki_agent::HostApi;
use iyakki_sec::policy::Policy;
use iyakki_sec::CapabilityStore;
use iyakki_vfs::MemFs;
use iyakki_wasm::{Sandbox, SandboxConfig};

use crate::{detect_env, net};

/// The WebAssembly tasks running now. A task is listed from the moment `run_wasm` starts until it returns, whatever way it ends.
static TASKS: Mutex<Vec<iyakki_core_api::host::WasmTask>> = Mutex::new(Vec::new());
static NEXT_TASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) struct TaskGuard(u64);

impl TaskGuard {
    pub(crate) fn begin(user: &str, entry: &str, bytes: usize) -> TaskGuard {
        let id = NEXT_TASK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let t = iyakki_core_api::host::WasmTask { id, user: user.into(), entry: entry.chars().take(64).collect(), bytes, started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) };
        TASKS.lock().unwrap_or_else(|e| e.into_inner()).push(t);
        TaskGuard(id)
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        TASKS.lock().unwrap_or_else(|e| e.into_inner()).retain(|t| t.id != self.0);
    }
}

#[derive(Default)]
pub struct CoreHost {
    /// One isolated in-memory filesystem per user, sized by that user's quota.
    vfs: Mutex<HashMap<String, Arc<Mutex<MemFs>>>>,
}

impl CoreHost {
    pub fn new() -> CoreHost {
        CoreHost::default()
    }

    fn vfs_for(&self, user: &str, quota_mb: u32) -> Arc<Mutex<MemFs>> {
        let mut m = self.vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        m.entry(user.to_string()).or_insert_with(|| Arc::new(Mutex::new(MemFs::with_quota((quota_mb as usize) << 20)))).clone()
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

impl HostApi for CoreHost {
    fn detect_env(&self) -> String {
        detect_env().to_string()
    }

    fn net_status(&self) -> String {
        let ifaces = net::list_ifaces(Path::new(crate::net::SYS_CLASS_NET));
        let mut s: Vec<String> = ifaces.iter().map(|i| format!("{:<14}{:?}", i.name, i.kind)).collect();
        if let Ok(o) = std::process::Command::new("ip").args(["-br", "-4", "addr"]).output() {
            s.push(String::new());
            s.extend(String::from_utf8_lossy(&o.stdout).lines().map(str::to_string));
        }
        // read-only: the private MAC address mode and the addresses in use (changing it is for administrators, in Settings)
        let mac = iyakki_devctl::mac::MacEnv::real();
        let lines = iyakki_devctl::mac::status_lines_from_files(&mac.roots, &mac.config());
        if lines.len() > 1 {
            s.push(String::new());
            s.extend(lines);
        }
        s.join("\n")
    }

    fn wasm_tasks(&self) -> Vec<iyakki_core_api::host::WasmTask> {
        TASKS.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn run_wasm(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
        self.run_wasm_io(user, policy, wasm, entry, &iyakki_core_api::host::WasmIo::default())
    }

    fn run_wasm_io(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str, io: &iyakki_core_api::host::WasmIo) -> Result<String, String> {
        let _task = TaskGuard::begin(user, entry, wasm.len());
        let cfg = SandboxConfig { fuel: policy.max_fuel, max_memory_bytes: (policy.max_memory_mb as usize) << 20 };
        let sandbox = Sandbox::new(cfg).map_err(|e| e.to_string())?.with_stdin(io.stdin.clone()).with_cancel(io.cancel.clone());
        let caps = CapabilityStore::new();
        let tokens = policy.grant_tokens(&caps, user, &[]).map_err(|e| e.to_string())?;
        let vfs = self.vfs_for(user, policy.vfs_quota_mb);
        let out = sandbox.run(wasm, entry, vfs, caps, tokens).map_err(|e| e.to_string())?;
        let log = String::from_utf8_lossy(&out.log).trim_end().to_string();
        let tail = format!("[exit {:?}, {} instructions]", out.result, out.fuel_used);
        Ok(if log.is_empty() { tail } else { format!("{log}\n{tail}") })
    }

    fn can_power(&self) -> bool {
        is_root()
    }

    fn power_off(&self) {
        if is_root() {
            crate::boot::power_off();
        } else {
            log::warn!("power off requested but not running as root; ignoring");
        }
    }

    fn reboot(&self) {
        if is_root() {
            crate::boot::reboot();
        } else {
            log::warn!("reboot requested but not running as root; ignoring");
        }
    }

    fn can_sleep(&self) -> bool {
        is_root() && crate::boot::sleep_state_available()
    }

    fn sleep(&self) {
        if is_root() {
            crate::boot::sleep();
        } else {
            log::warn!("sleep requested but not running as root; ignoring");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    const WRITE: &str = r#"(module
      (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
      (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
      (import "iyakki" "log" (func $log (param i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 64) "/home/alice/f")
      (data (i32.const 128) "x")
      (func (export "_start")
        ;; tokens are issued in policy order: read scopes first, then write scopes
        (drop (call $tg (i32.const 2) (i32.const 0)))
        (if (i32.eq (call $fw (i32.const 0) (i32.const 64) (i32.const 13) (i32.const 128) (i32.const 1)) (i32.const 0))
          (then (call $log (i32.const 128) (i32.const 1))))))"#;

    #[test]
    fn runs_with_user_limits_and_isolated_filesystems() {
        let host = CoreHost::new();
        let p = Role::User.template(); // read: /home/{u}, /shared  write: /home/{u}
        let out = host.run_wasm("alice", &p, WRITE.as_bytes(), "_start").unwrap();
        assert!(out.starts_with("x\n[exit None"), "{out}");
        assert!(host.vfs_for("alice", 64).lock().unwrap().exists("/home/alice/f"));
        assert!(!host.vfs_for("bob", 64).lock().unwrap().exists("/home/alice/f"), "per-user filesystems");
        // a guest cannot write outside the user's scope: bob's token does not cover /home/alice
        let out = host.run_wasm("bob", &p, WRITE.as_bytes(), "_start").unwrap();
        assert!(!out.starts_with("x"), "{out}");
    }

    #[test]
    fn fuel_limit_from_policy_is_enforced() {
        let host = CoreHost::new();
        let mut p = Role::Guest.template();
        p.max_fuel = 5_000;
        let e = host.run_wasm("g", &p, br#"(module (func (export "_start") (loop br 0)))"#, "_start").unwrap_err();
        assert!(e.contains("fuel"), "{e}");
    }

    #[test]
    fn describes_the_machine() {
        let h = CoreHost::new();
        assert!(!h.detect_env().is_empty());
        assert!(h.net_status().contains("Loopback"));
    }

    #[test]
    fn a_wasm_task_is_listed_while_it_runs_and_gone_after_however_it_ends() {
        let host = CoreHost::new();
        let mine = |u: &str| host.wasm_tasks().into_iter().filter(|t| t.user == u).count();
        assert_eq!(mine("lister"), 0);
        {
            let _g = TaskGuard::begin("lister", "_start", 10);
            assert_eq!(mine("lister"), 1);
        }
        assert_eq!(mine("lister"), 0, "the guard removes it");
        // a run that fails for fuel is not left behind either
        let mut p = Role::Guest.template();
        p.max_fuel = 5_000;
        let _ = host.run_wasm("lister", &p, br#"(module (func (export "_start") (loop br 0)))"#, "_start");
        assert_eq!(mine("lister"), 0);
    }

    #[test]
    fn a_run_gets_its_input_and_a_stop_flag_ends_a_loop_for_real() {
        let host = CoreHost::new();
        let p = Role::User.template();
        let echo = r#"(module
          (import "iyakki" "stdin_read" (func $rd (param i32 i32) (result i32)))
          (import "iyakki" "log" (func $log (param i32 i32)))
          (memory (export "memory") 1)
          (func (export "_start") (call $log (i32.const 0) (call $rd (i32.const 0) (i32.const 64)))))"#;
        let io = iyakki_core_api::host::WasmIo { stdin: b"ping\n".to_vec(), ..Default::default() };
        let out = host.run_wasm_io("typist", &p, echo.as_bytes(), "_start", &io).unwrap();
        assert!(out.starts_with("ping\n[exit None"), "{out}");
        assert!(host.run_wasm("typist", &p, echo.as_bytes(), "_start").unwrap().starts_with("[exit None"), "no input means an empty read");
        let mut big = Role::User.template();
        big.max_fuel = u64::MAX / 4;
        let io = iyakki_core_api::host::WasmIo::default();
        let flag = io.cancel.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let t = std::time::Instant::now();
        let e = host.run_wasm_io("typist", &big, br#"(module (func (export "_start") (loop br 0)))"#, "_start", &io).unwrap_err();
        h.join().unwrap();
        assert!(e.contains("stopped") && t.elapsed() < std::time::Duration::from_secs(10), "{e}");
        assert_eq!(host.wasm_tasks().iter().filter(|t| t.user == "typist").count(), 0, "a stopped run is not left in the task list");
    }
}
