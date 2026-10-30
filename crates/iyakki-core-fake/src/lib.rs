//! `FakeOs`: IYAKKI core in memory, for tests. Canned answers in, every call recorded, nothing touches the machine.
//! Implements [`Sysinfo`] and [`HostApi`] today; each slice of docs/development/UI-OS-SPLIT-PLAN.md adds its trait here.

pub mod contract;

use iyakki_core_api::{ApiError, ApiResult, Capabilities, HostApi, Identity, LoadAvg, MemInfo, Sysinfo, Uptime};
use iyakki_sec::policy::Policy;
use std::sync::Mutex;

/// What the fake answers with. `None` makes the matching call fail with `Unsupported`.
#[derive(Clone, Debug)]
pub struct Canned {
    pub cmdline: Option<String>,
    pub meminfo: Option<MemInfo>,
    pub uptime: Option<Uptime>,
    pub loadavg: Option<LoadAvg>,
    pub product_name: Option<String>,
    pub cpu_count: Option<u32>,
    pub env: String,
    pub net: String,
    pub wasm_result: Result<String, String>,
    pub can_power: bool,
    pub capabilities: Capabilities,
    pub identity: Identity,
}

impl Default for Canned {
    fn default() -> Canned {
        Canned {
            cmdline: Some("quiet iyakki.test=1".into()),
            meminfo: Some(MemInfo { total_kb: 8_000_000, free_kb: 2_000_000, available_kb: 4_000_000, swap_total_kb: 0, swap_free_kb: 0 }),
            uptime: Some(Uptime { up_secs: 3600.0, idle_secs: 14000.0 }),
            loadavg: Some(LoadAvg { one: 0.25, five: 0.2, fifteen: 0.1 }),
            product_name: Some("Test Machine".into()),
            cpu_count: Some(4),
            env: "baremetal".into(),
            net: "eth0 Ethernet".into(),
            wasm_result: Ok("ok".into()),
            can_power: true,
            capabilities: Capabilities::all(),
            identity: Identity::local("tester", true),
        }
    }
}

#[derive(Default)]
pub struct FakeOs {
    canned: Mutex<Canned>,
    calls: Mutex<Vec<String>>,
}

impl FakeOs {
    pub fn new() -> FakeOs {
        FakeOs::default()
    }
    pub fn with(canned: Canned) -> FakeOs {
        FakeOs { canned: Mutex::new(canned), calls: Mutex::new(vec![]) }
    }
    /// Change the canned answers while a test runs.
    pub fn set(&self, f: impl FnOnce(&mut Canned)) {
        f(&mut self.canned.lock().unwrap());
    }
    /// Every call so far, in order, as `method` or `method(argument)`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }
    pub fn capabilities(&self) -> Capabilities {
        self.canned.lock().unwrap().capabilities
    }
    pub fn identity(&self) -> Identity {
        self.canned.lock().unwrap().identity.clone()
    }
    fn rec(&self, what: impl Into<String>) {
        self.calls.lock().unwrap().push(what.into());
    }
    fn answer<T: Clone>(&self, name: &str, pick: impl FnOnce(&Canned) -> Option<T>) -> ApiResult<T> {
        self.rec(name);
        pick(&self.canned.lock().unwrap()).ok_or_else(|| ApiError::Unsupported(format!("{name} is not available on this fake")))
    }
}

impl Sysinfo for FakeOs {
    fn cmdline(&self) -> ApiResult<String> {
        self.answer("cmdline", |c| c.cmdline.clone())
    }
    fn meminfo(&self) -> ApiResult<MemInfo> {
        self.answer("meminfo", |c| c.meminfo)
    }
    fn uptime(&self) -> ApiResult<Uptime> {
        self.answer("uptime", |c| c.uptime)
    }
    fn loadavg(&self) -> ApiResult<LoadAvg> {
        self.answer("loadavg", |c| c.loadavg)
    }
    fn product_name(&self) -> ApiResult<String> {
        self.answer("product_name", |c| c.product_name.clone())
    }
    fn cpu_count(&self) -> ApiResult<u32> {
        self.answer("cpu_count", |c| c.cpu_count)
    }
}

impl HostApi for FakeOs {
    fn detect_env(&self) -> String {
        self.rec("detect_env");
        self.canned.lock().unwrap().env.clone()
    }
    fn net_status(&self) -> String {
        self.rec("net_status");
        self.canned.lock().unwrap().net.clone()
    }
    fn run_wasm(&self, user: &str, _policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
        self.rec(format!("run_wasm({user},{} bytes,{entry})", wasm.len()));
        self.canned.lock().unwrap().wasm_result.clone()
    }
    fn power_off(&self) {
        self.rec("power_off");
    }
    fn reboot(&self) {
        self.rec("reboot");
    }
    fn can_power(&self) -> bool {
        self.canned.lock().unwrap().can_power
    }
}

/// In-memory accounts and audit for the web service's tests: a plain list of (name, password, role) and a list of audit lines.
pub struct FakeAccounts {
    users: std::sync::Mutex<Vec<(String, String, iyakki_sec::policy::Role)>>,
    audit: std::sync::Mutex<Vec<String>>,
}

impl FakeAccounts {
    pub fn new(users: &[(&str, &str, iyakki_sec::policy::Role)]) -> FakeAccounts {
        FakeAccounts { users: std::sync::Mutex::new(users.iter().map(|(n, p, r)| (n.to_string(), p.to_string(), *r)).collect()), audit: Default::default() }
    }
    pub fn remove(&self, name: &str) {
        self.users.lock().unwrap_or_else(|e| e.into_inner()).retain(|u| u.0 != name);
    }
    /// Every audit line so far, as `user event detail`.
    pub fn lines(&self) -> Vec<String> {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn principal(name: &str, role: iyakki_sec::policy::Role) -> iyakki_core_api::Principal {
        iyakki_core_api::Principal { user: name.into(), role: role.name().into(), admin: role == iyakki_sec::policy::Role::Admin, policy: role.template() }
    }
}

impl iyakki_core_api::Authenticator for FakeAccounts {
    fn login(&self, name: &str, password: &str) -> ApiResult<iyakki_core_api::Principal> {
        let g = self.users.lock().unwrap_or_else(|e| e.into_inner());
        match g.iter().find(|u| u.0 == name && u.1 == password) {
            Some(u) => Ok(FakeAccounts::principal(&u.0, u.2)),
            None => Err(ApiError::Denied("invalid username or password".into())),
        }
    }
    fn lookup(&self, user: &str) -> Option<iyakki_core_api::Principal> {
        let g = self.users.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().find(|u| u.0 == user).map(|u| FakeAccounts::principal(&u.0, u.2))
    }
}

impl iyakki_core_api::AuditSink for FakeAccounts {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{user} {event} {detail}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn sysinfo_answers_from_the_canned_data_and_records_each_call() {
        let os = FakeOs::new();
        assert_eq!(os.cmdline().unwrap(), "quiet iyakki.test=1");
        assert_eq!(os.meminfo().unwrap().total_kb, 8_000_000);
        assert_eq!(os.cpu_count().unwrap(), 4);
        assert_eq!(os.calls(), ["cmdline", "meminfo", "cpu_count"]);
        os.clear_calls();
        assert!(os.calls().is_empty());
    }

    #[test]
    fn a_missing_answer_is_unsupported_and_still_recorded() {
        let os = FakeOs::new();
        os.set(|c| c.product_name = None);
        let e = os.product_name().unwrap_err();
        assert_eq!(e.code(), "unsupported");
        assert_eq!(os.calls(), ["product_name"]);
    }

    #[test]
    fn canned_data_can_change_while_a_test_runs() {
        let os = FakeOs::new();
        os.set(|c| c.uptime = Some(Uptime { up_secs: 5.0, idle_secs: 1.0 }));
        assert_eq!(os.uptime().unwrap().up_secs, 5.0);
    }

    #[test]
    fn the_host_api_is_recorded_and_power_calls_do_nothing() {
        let os = FakeOs::new();
        let p = Role::Admin.template();
        assert_eq!(os.detect_env(), "baremetal");
        assert_eq!(os.net_status(), "eth0 Ethernet");
        assert_eq!(os.run_wasm("amy", &p, b"abc", "main"), Ok("ok".into()));
        os.power_off();
        os.reboot();
        assert!(os.can_power());
        assert_eq!(os.calls(), ["detect_env", "net_status", "run_wasm(amy,3 bytes,main)", "power_off", "reboot"]);
        os.set(|c| {
            c.can_power = false;
            c.wasm_result = Err("trap".into());
        });
        assert!(!os.can_power());
        assert_eq!(os.run_wasm("amy", &p, b"", "main"), Err("trap".into()));
    }

    #[test]
    fn it_works_as_a_trait_object_for_both_traits() {
        let os = std::sync::Arc::new(FakeOs::new());
        let h: std::sync::Arc<dyn HostApi> = os.clone();
        let s: std::sync::Arc<dyn Sysinfo> = os.clone();
        assert_eq!(h.detect_env(), "baremetal");
        assert!(s.loadavg().is_ok());
        assert_eq!(os.capabilities(), Capabilities::all());
        assert_eq!(os.identity().user, "tester");
    }
}
