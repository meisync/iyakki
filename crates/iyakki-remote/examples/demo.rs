//! A throwaway server over a temporary data folder, for trying the browser front end by hand:
//! `cargo run -p iyakki-remote --example demo -- 127.0.0.1:18080` (sign in as root / rootpass12).

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::screen::ShellGate;
use iyakki_remote::server::{Config, Server};
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Offline;
impl Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

fn env(tag: &str) -> ShellEnv {
    let d = std::env::temp_dir().join(format!("iyakki-remote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    ShellEnv {
        store: Arc::new(Mutex::new(store)),
        audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
        host: Arc::new(Host),
        cfg: iyakki_agent::config::AiConfig::parse("").unwrap(),
        chat: Arc::new(iyakki_agent::chat::Unconfigured),
        mcp_servers: vec![],
        services: iyakki_agent::services::Services::open(&d.join("svc"), up.clone(), up).unwrap(),
        web: Arc::new(iyakki_web::fetch::NoNetwork),
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    }
}


fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:18080".into());
    let server = Server::start(Config { addr, tls: None }, Arc::new(ShellGate::new(env("demo")))).expect("listen");
    println!("listening on http://{}  (root / rootpass12)", server.local_addr());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
