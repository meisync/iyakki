//! Draws every app screen to a PNG and prints how long each took:
//! `cargo run -q --release -p iyakki-shell --example screens [outdir] [WxH]` (default /tmp/iyakki-screens, 1280x720).
//! Signs in as root on a throwaway data folder, opens each app through the shell (so the header is the real one) and
//! walks the pages of the apps that have tabs.

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::{Event, Key};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    let d = std::env::temp_dir().join(format!("iyakki-screens-{}", std::process::id()));
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

/// Draw twice (the first frame settles hover and caches), time the second, save it.
fn shot(sh: &mut Shell, g: &mut Gfx, dir: &str, name: &str, rows: &mut Vec<(String, f64)>) {
    for _ in 0..2 {
        sh.handle(&Event::Tick);
    }
    sh.render(g);
    let mut best = f64::MAX;
    for _ in 0..15 {
        let t = Instant::now();
        sh.render(g);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    g.save_png(std::path::Path::new(&format!("{dir}/{name}.png"))).unwrap();
    rows.push((name.to_string(), best));
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/iyakki-screens".into());
    let (w, h) = std::env::args().nth(2).and_then(|d| d.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280), b.parse().unwrap_or(720)))).unwrap_or((1280, 720));
    std::fs::create_dir_all(&dir).unwrap();
    let only: Option<String> = std::env::var("ONLY").ok();
    let mut sh = Shell::new(env());
    let mut g = Gfx::new(w, h);
    let mut rows = Vec::new();
    sh.render(&mut g);
    shot(&mut sh, &mut g, &dir, "00-login", &mut rows);
    type_text(&mut sh, "root");
    sh.handle(&Event::Key(Key::Tab));
    type_text(&mut sh, "rootpass12");
    sh.handle(&Event::Key(Key::Enter));
    sh.render(&mut g);
    shot(&mut sh, &mut g, &dir, "01-launcher", &mut rows);
    let apps = ["settings", "system", "users", "rag", "runs", "mcp", "skills", "calendar", "scan", "mesh", "usage", "inbox", "cli", "edit", "radio", "browser", "chat", "media"];
    for id in apps {
        if only.as_deref().is_some_and(|o| o != id) {
            continue;
        }
        if !sh.open_for_demo(id) {
            println!("could not open {id}");
            continue;
        }
        sh.render(&mut g);
        shot(&mut sh, &mut g, &dir, id, &mut rows);
        // pages: click each tab-like hit once (Event::Tab(n)) and draw it
        let tabs: Vec<(usize, String)> = sh.hits().iter().filter_map(|hh| if let Event::Tab(n) = hh.ev { if n < 40 { Some((n, hh.label.clone())) } else { None } } else { None }).collect();
        let mut seen = std::collections::HashSet::new();
        for (n, label) in tabs.into_iter().filter(|(n, _)| *n > 0).take(14) {
            if !seen.insert(n) {
                continue;
            }
            sh.handle(&Event::Tab(n));
            sh.render(&mut g);
            let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
            shot(&mut sh, &mut g, &dir, &format!("{id}-{n:02}-{safe}"), &mut rows);
        }
    }
    println!("{:<34} {:>8}", "screen", "ms");
    for (n, ms) in &rows {
        println!("{n:<34} {ms:>8.2}");
    }
}
