//! What the apps need from the running system. Implemented by `iyakki-core` (the supervisor) and by mocks and `FakeOs` in tests.
//! Moved here from `iyakki-agent`, which re-exports it (`iyakki_agent::HostApi` is unchanged for every caller).

use iyakki_sec::policy::Policy;

/// A WebAssembly task the supervisor is running right now (SYSTEM lists them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmTask {
    pub id: u64,
    pub user: String,
    pub entry: String,
    /// Size of the module in bytes.
    pub bytes: usize,
    /// When it started (unix seconds).
    pub started: i64,
}

/// What a WebAssembly run may be given besides the module: the input the program reads (`iyakki.stdin_read`) and a flag that stops
/// the run for real when it becomes true (the editor's Stop button and the chat tool).
#[derive(Clone, Debug, Default)]
pub struct WasmIo {
    pub stdin: Vec<u8>,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub trait HostApi: Send + Sync {
    fn detect_env(&self) -> String;
    fn net_status(&self) -> String;
    /// Run a WASM module for `user` with that user's limits and capability tokens.
    fn run_wasm(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String>;
    /// Like `run_wasm` with input and a stop flag (additive). The default only checks the flag before it starts and drops the input:
    /// hosts that cannot feed a program or stop it half way say so by not overriding it.
    fn run_wasm_io(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str, io: &WasmIo) -> Result<String, String> {
        if io.cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("stopped before it started".into());
        }
        self.run_wasm(user, policy, wasm, entry)
    }
    fn power_off(&self);
    /// The WebAssembly tasks running now (additive: hosts that do not track them list none).
    fn wasm_tasks(&self) -> Vec<WasmTask> {
        Vec::new()
    }
    fn reboot(&self);
    /// Whether `power_off` and `reboot` can really act here (false in windowed development mode and tests that say so); the
    /// login screen's power card tells the person instead of doing nothing.
    fn can_power(&self) -> bool {
        true
    }
    /// Whether the machine can go to sleep (suspend) and wake again here. False by default; the power cards show a Sleep
    /// choice only when this is true.
    fn can_sleep(&self) -> bool {
        false
    }
    /// Suspend the machine. Returns after it wakes. The caller locks the session first.
    fn sleep(&self) {}
}
