//! Loads addresses in the BROWSER tile over the real network with the built-in engine and saves a PNG of each:
//! `cargo run -q --release -p iyakki-shell --example browse_shot -- outdir 'cats' 'https://www.youtube.com/'`
//! Each argument is typed into the address bar. Developer check only (needs the network).

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::{Event, Key};
use std::sync::{Arc, Mutex};

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
        "eth0 Ethernet".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

fn env() -> ShellEnv {
    let d = std::env::temp_dir().join(format!("iyakki-browse-shot-{}", std::process::id()));
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
        web: Arc::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default()),
        hostname: "iyakki-demo".into(),
        data_volatile: false,
        hwcheck: false,
    }
}

fn type_text(sh: &mut Shell, s: &str) {
    for c in s.chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).unwrap();
    let inputs: Vec<String> = args.collect();
    let mut sh = Shell::new(env());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    type_text(&mut sh, "root");
    sh.handle(&Event::Key(Key::Tab));
    type_text(&mut sh, "rootpass12");
    sh.handle(&Event::Key(Key::Enter));
    for (i, input) in inputs.iter().enumerate() {
        if !sh.open_for_demo("browser") {
            println!("could not open the browser");
            return;
        }
        sh.handle(&Event::Key(Key::Ctrl('l')));
        type_text(&mut sh, input);
        sh.handle(&Event::Key(Key::Enter));
        for _ in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            sh.handle(&Event::Tick);
        }
        sh.render(&mut g);
        let name = format!("{dir}/{:02}.png", i + 1);
        g.save_png(std::path::Path::new(&name)).unwrap();
        println!("{input} -> {name}");
    }
}
