//! Pictures of the Login button and the sign-in dialog of Claude Code and GitHub Copilot, with stand-in programs that print an
//! address and a code (no vendor is called, no account is used):
//! `cargo run -q -p iyakki-shell --example login_shots [outdir]` (default /tmp/iyakki-login-shots).

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::{Event, Key};
use std::os::unix::fs::PermissionsExt;
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

const URL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=00000000-0000-0000-0000-000000000000&response_type=code&scope=user%3Aprofile+user%3Ainference&code_challenge=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG&state=zyxwvutsrqponmlkjihgfedcba9876543210";

fn script(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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

fn ticks(sh: &mut Shell, ms: u64) {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_millis(ms) {
        sh.handle(&Event::Tick);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/iyakki-login-shots".into());
    std::fs::create_dir_all(&dir).unwrap();
    let d = std::env::temp_dir().join(format!("iyakki-login-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("home")).unwrap();
    // nothing of the real home folder is touched
    std::env::set_var("HOME", d.join("home"));
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    let env = ShellEnv {
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
    };
    // stand-ins: `claude` (signed out until the code is "demo") and `copilot` (a device code, then done)
    let state = d.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let claude = d.join("claude-fake");
    script(&claude, &format!("#!/bin/sh\nS='{s}'\ncase \"$1 $2\" in\n \"auth status\") if [ -f \"$S/in\" ]; then echo '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"email\":\"you@example.com\"}}'; exit 0; fi; echo '{{\"loggedIn\":false,\"authMethod\":\"none\"}}'; exit 1;;\n \"auth login\") printf 'Opening browser to sign in...\\r\\nIf the browser does not open, visit:\\r\\n%s\\r\\nPaste code here if prompted > ' '{URL}'; read c; if [ \"$c\" = demo ]; then touch \"$S/in\"; echo 'Login successful'; exit 0; fi; echo 'The code was not accepted'; exit 1;;\nesac\necho '2.1.7 (Claude Code)'\n", s = state.display()));
    let copilot = d.join("copilot-fake");
    script(&copilot, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '0.0.400'; exit 0; fi\nprintf 'First copy your one-time code: A1B2-C3D4\\r\\nThen open https://github.com/login/device\\r\\nWaiting for authorization...\\r\\n'\nsleep 600\n");
    let svc = env.services.clone();
    iyakki_agent::clis::write_fake_installed(&svc.clis, "claude-code", "#!/bin/sh\nexit 0\n");
    iyakki_agent::clis::write_fake_installed(&svc.clis, "github-copilot", "#!/bin/sh\nexit 0\n");
    std::os::unix::fs::symlink(&claude, svc.clis.bin_dir().join("claude")).unwrap();
    svc.settings.lock().unwrap().update(|s| s.agents.copilot.command = copilot.display().to_string()).unwrap();
    let mut sh = Shell::new(env);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    for c in "root".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Tab));
    for c in "rootpass12".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    // the CLI list: Claude Code installed, not signed in
    sh.open_for_demo("cli");
    sh.handle(&Event::Tab(1));
    let i = iyakki_agent::clis::CATALOG.iter().position(|c| c.id == "claude-code").unwrap();
    sh.handle(&Event::Row(i));
    ticks(&mut sh, 1500);
    save(&mut sh, &mut g, &dir, "login-1-cli-list");
    // Login: the choice
    sh.handle(&Event::Tab(iyakki_shell::agent_login::LOGIN_TAB));
    save(&mut sh, &mut g, &dir, "login-2-choose");
    // the address, the QR code and the field for the code
    // the second step (the phone relay) is offered when the device can serve the page (here: as if the web front end ran on 8443)
    iyakki_agent::extagent::relay::note_web_front_end(8443, true);
    sh.handle(&Event::Key(Key::Enter));
    ticks(&mut sh, 1500);
    save(&mut sh, &mut g, &dir, "login-3-claude-address-and-qr");
    for c in "demo".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    ticks(&mut sh, 1500);
    save(&mut sh, &mut g, &dir, "login-4-signed-in");
    sh.handle(&Event::Key(Key::Esc));
    ticks(&mut sh, 1500);
    save(&mut sh, &mut g, &dir, "login-5-chip-and-logout");
    // GitHub Copilot: a one-time code
    let i = iyakki_agent::clis::CATALOG.iter().position(|c| c.id == "github-copilot").unwrap();
    sh.handle(&Event::Row(i));
    sh.handle(&Event::Tab(iyakki_shell::agent_login::LOGIN_TAB + 1));
    ticks(&mut sh, 1500);
    save(&mut sh, &mut g, &dir, "login-6-copilot-device-code");
    sh.handle(&Event::Key(Key::Esc));
    ticks(&mut sh, 800);
    sh.handle(&Event::Key(Key::Esc));
    // Store -> Setup (the menu's Quick install opens it) lists the same buttons
    sh.handle(&Event::Key(Key::F(10)));
    sh.handle(&Event::Key(Key::Char('q')));
    ticks(&mut sh, 1000);
    save(&mut sh, &mut g, &dir, "login-7-quick-install");
    let _ = std::fs::remove_dir_all(&d);
}
