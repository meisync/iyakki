//! What the apps need from the running system. Implemented by `iyakki-core` (the supervisor) and by mocks and `FakeOs` in tests.
//! Moved here from `iyakki-agent`, which re-exports it (`iyakki_agent::HostApi` is unchanged for every caller).

use iyakki_sec::policy::Policy;

pub trait HostApi: Send + Sync {
    fn detect_env(&self) -> String;
    fn net_status(&self) -> String;
    /// Run a WASM module for `user` with that user's limits and capability tokens.
    fn run_wasm(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String>;
    fn power_off(&self);
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
