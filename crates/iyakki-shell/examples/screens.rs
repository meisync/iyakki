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
    let e = env();
    // the AI IDE shows this repository as its project, with a fake cluster that has a small shop in it
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    {
        let mut reg = iyakki_agent::workspace::Registry::load(e.services.dir(), "root");
        if let Ok(other) = std::fs::create_dir_all(std::env::temp_dir().join("iyakki-screens-shop")).map(|_| std::env::temp_dir().join("iyakki-screens-shop")) {
            let _ = reg.add("Shop Front", &other);
        }
        let _ = reg.add("iyakki", &repo);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        iyakki_shell::apps::ide::use_kube(Some(Arc::new(iyakki_agent::k8s::sample_cluster(now))));
    }
    let mut sh = Shell::new(e);
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
    let apps = ["settings", "system", "users", "rag", "runs", "mcp", "skills", "calendar", "scan", "mesh", "usage", "inbox", "cli", "edit", "radio", "browser", "chat", "media", "tales", "data", "dash", "flows", "store"];
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
    if only.as_deref().is_none_or(|o| o == "ide") {
        ide_tour(&mut sh, &mut g, &dir, &mut rows);
    }
    println!("{:<34} {:>8}", "screen", "ms");
    for (n, ms) in &rows {
        println!("{n:<34} {ms:>8.2}");
    }
}

/// The AI IDE: the main layout, the board in its three views, the cluster view, the switcher.
fn ide_tour(sh: &mut Shell, g: &mut Gfx, dir: &str, rows: &mut Vec<(String, f64)>) {
    let settle = |sh: &mut Shell| {
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            sh.handle(&Event::Tick);
        }
    };
    if !sh.open_for_demo("ide") {
        println!("could not open ide");
        return;
    }
    // DENSITY=comfortable|dense draws the tour in that IDE density (Compact is the default)
    match std::env::var("DENSITY").ok().as_deref() {
        Some("comfortable") => {
            sh.handle(&Event::Key(Key::Ctrl('=')));
        }
        Some("dense") => {
            sh.handle(&Event::Key(Key::Ctrl('-')));
        }
        _ => {}
    }
    settle(sh);
    sh.render(g);
    shot(sh, g, dir, "ide-01-main", rows);
    let key = |sh: &mut Shell, k: Key| {
        sh.handle(&Event::Key(k));
    };
    // open a few files through Ctrl+P
    for name in ["agent.rs", "README.md"] {
        key(sh, Key::Ctrl('p'));
        for c in name.chars() {
            key(sh, Key::Char(c));
        }
        key(sh, Key::Enter);
    }
    sh.render(g);
    shot(sh, g, dir, "ide-02-editor", rows);
    key(sh, Key::Ctrl('p'));
    for c in "ide".chars() {
        key(sh, Key::Char(c));
    }
    sh.render(g);
    shot(sh, g, dir, "ide-03-quick-open", rows);
    key(sh, Key::Esc);
    key(sh, Key::Ctrl('B'));
    settle(sh);
    sh.render(g);
    shot(sh, g, dir, "ide-04-board", rows);
    key(sh, Key::Char('v'));
    sh.render(g);
    shot(sh, g, dir, "ide-05-board-full", rows);
    key(sh, Key::Down);
    key(sh, Key::Down);
    key(sh, Key::Enter);
    sh.render(g);
    shot(sh, g, dir, "ide-06-board-drawer", rows);
    key(sh, Key::Esc);
    key(sh, Key::Tab);
    sh.render(g);
    shot(sh, g, dir, "ide-07-board-table", rows);
    key(sh, Key::Tab);
    sh.render(g);
    shot(sh, g, dir, "ide-08-board-phases", rows);
    key(sh, Key::Tab);
    key(sh, Key::Char('v'));
    key(sh, Key::Ctrl('K'));
    settle(sh);
    sh.render(g);
    shot(sh, g, dir, "ide-09-k8s", rows);
    for _ in 0..3 {
        key(sh, Key::Tab);
    }
    for _ in 0..4 {
        key(sh, Key::Down);
    }
    key(sh, Key::Enter);
    settle(sh);
    sh.render(g);
    shot(sh, g, dir, "ide-10-k8s-pod", rows);
    key(sh, Key::Char('l'));
    settle(sh);
    sh.render(g);
    shot(sh, g, dir, "ide-11-k8s-logs", rows);
    key(sh, Key::Esc);
    key(sh, Key::Ctrl('K'));
    key(sh, Key::Ctrl('W'));
    sh.render(g);
    shot(sh, g, dir, "ide-12-workspaces", rows);
    key(sh, Key::Esc);
}
