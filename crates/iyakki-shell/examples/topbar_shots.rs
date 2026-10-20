//! Pictures of the top bar's IYAKKI menu, the installer indicator, the home screen sections and Settings -> Time:
//! `cargo run -q -p iyakki-shell --example topbar_shots [outdir]` (default /tmp/iyakki-topbar-shots). Offline: nothing is downloaded,
//! the install shown is a stand-in script.

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
    let d = std::env::temp_dir().join(format!("iyakki-topbar-shots-{}", std::process::id()));
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


fn save(sh: &mut Shell, g: &mut Gfx, dir: &str, name: &str) {
    for _ in 0..3 {
        sh.handle(&Event::Tick);
    }
    sh.render(g);
    sh.render(g);
    g.save_png(std::path::Path::new(&format!("{dir}/{name}.png"))).unwrap();
    println!("{dir}/{name}.png");
}

fn key(sh: &mut Shell, k: Key) {
    sh.handle(&Event::Key(k));
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/iyakki-topbar-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let e = env();
    let data = e.services.dir().to_path_buf();
    let mut sh = Shell::new(e);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    type_text(&mut sh, "root");
    key(&mut sh, Key::Tab);
    type_text(&mut sh, "rootpass12");
    key(&mut sh, Key::Enter);
    // the four sections of the home screen
    for name in ["home-1-home", "home-2-web-media", "home-3-work", "home-4-system"] {
        save(&mut sh, &mut g, &dir, name);
        key(&mut sh, Key::Tab);
    }
    // the IYAKKI menu and its pages
    key(&mut sh, Key::F(10));
    save(&mut sh, &mut g, &dir, "menu-1-iyakki");
    for (k, name) in [('a', "menu-2-about"), ('q', "menu-3-quick-install"), ('m', "menu-4-media-player"), ('d', "menu-5-mac-drivers")] {
        key(&mut sh, Key::Char(k));
        save(&mut sh, &mut g, &dir, name);
        key(&mut sh, Key::Esc);
    }
    // an install running: a stand-in script prints steps like `iyakki quick-install` (nothing is downloaded)
    use std::os::unix::fs::PermissionsExt;
    let fake = data.join("fake-iyakki");
    std::fs::write(&fake, "#!/bin/sh\necho \"[0/2] installing $2 from the CLI catalogue\"\nsleep 1\necho \"[1/2] downloading 48%\"\nsleep 60\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    sh.set_quick_launch_for_demo(iyakki_agent::quickinstall::Launch { exe: fake, data: data.clone() });
    key(&mut sh, Key::Char('q'));
    for _ in 0..3 {
        key(&mut sh, Key::Down);
    }
    key(&mut sh, Key::Enter);
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Esc);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    save(&mut sh, &mut g, &dir, "installs-1-indicator");
    if let Some(r) = sh.hits().iter().find(|h| h.ev == Event::Tab(iyakki_shell::installpop::INDICATOR)).map(|h| h.rect) {
        sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
        save(&mut sh, &mut g, &dir, "installs-2-list");
        if let Some(r) = sh.hits().iter().find(|h| h.label == "Log").map(|h| h.rect) {
            sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
            for _ in 0..40 {
                sh.handle(&Event::Tick);
            }
            save(&mut sh, &mut g, &dir, "installs-3-log");
        }
    }
    // stop the stand-in job
    for j in sh.services_for_demo().jobs.list("root") {
        let _ = sh.services_for_demo().jobs.stop("root", &j.id, std::time::Duration::from_millis(500));
    }
    // Settings -> Time with Detect automatically
    sh.open_for_demo("settings");
    key(&mut sh, Key::F(7));
    save(&mut sh, &mut g, &dir, "settings-time");
    let _ = std::fs::remove_dir_all(data.parent().unwrap_or(&data));
}
