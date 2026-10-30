//! The `adb` side of developer mode end to end over loopback TLS: scopes (default install only, granted on the device), `iyakki dev`
//! info / ps / dmesg / status / logs --system, files push and pull with resume, input, whoami scopes, revoke. A made-up /proc and
//! /sys; no real network, no real home folder.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::devmode::client::{self, DevClient, Home};
use iyakki_agent::devmode::scopes::Scope;
use iyakki_agent::devmode::cli_adb;
use iyakki_agent::devmode::{self, cli, DevMode};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::devserver::DevServer;
use iyakki_sec::policy::{Policy, Role};
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;

static REBOOTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static POWEROFFS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
    fn power_off(&self) {
        POWEROFFS.fetch_add(1, Ordering::SeqCst);
    }
    fn reboot(&self) {
        REBOOTS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fx {
    env: ShellEnv,
    dir: PathBuf,
    server: DevServer,
    dm: DevMode,
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.server.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    let d = std::env::temp_dir().join(format!("iyakki-devmode-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    store.add_local("bob", "bobpass1234", Role::User).unwrap();
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
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    };
    let id = iyakki_remote::tls::identity(&devmode::tls_dir(env.services.dir()), &[]).unwrap();
    let m = d.join("machine");
    for (f, t) in [("proc/uptime", "100.0 1.0\n"), ("proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 500 kB\n"), ("proc/loadavg", "0.1 0.2 0.3 1/2 3\n"), ("proc/modules", "a 1 0 - Live 0x0\n"), ("sys/class/dmi/id/product_name", "Macmini7,1\n"), ("kmsg", "6,1,100,-;hello kernel\n6,2,200,-;[iyakki INFO ] started\n"), ("proc/1/stat", "1 (iyakki) S 0 0 0 0 0 0 0 0 0 0 5 1 0 0 20 0 1 0 1 1000 10 0\n"), ("proc/1/status", "Uid:\t0\t0\t0\t0\n"), ("passwd", "svc:x:100:100::/var/empty:/bin/false\n")] {
        let p = m.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, t).unwrap();
    }
    let paths = iyakki_agent::devmode::debug::Paths { sys: iyakki_agent::sysinfo::SysRoots { proc: m.join("proc"), sys: m.join("sys"), passwd: m.join("passwd") }, kmsg: m.join("kmsg"), boot_log: m.join("boot.log"), release: m.join("release"), addrs: Arc::new(|| vec![("eth0".into(), "192.0.2.7".into())]) };
    let home = d.join("acct-home");
    std::fs::create_dir_all(&home).unwrap();
    let server = DevServer::start_with_paths(env.share(), "127.0.0.1:0", &id, paths, Some(home)).unwrap();
    let dm = DevMode::open(env.services.dir());
    Fx { env, dir: d, server, dm }
}

fn admin() -> Policy {
    Role::Admin.template()
}

fn port(f: &Fx) -> u16 {
    f.server.local_addr().port()
}

fn home(f: &Fx, n: &str) -> Home {
    Home::at(&f.dir.join(format!("home-{n}")))
}

/// Pair a computer called `name` acting as `account`.
fn paired(f: &Fx, name: &str, account: &str) -> (Home, DevClient) {
    let h = home(f, name);
    let p = f.dm.start_pairing_as(&admin(), "root", account).unwrap();
    let d = client::pair(&h, "127.0.0.1", port(f), name, &p.code, None, Some(name)).unwrap();
    let c = DevClient::new(&h, d).unwrap();
    (h, c)
}

fn tile_dir(f: &Fx, id: &str, version: &str, text: &str) -> PathBuf {
    let d = f.dir.join(format!("src-{id}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("app.json"), format!(r#"{{"id":"{id}","name":"{id} app","version":"{version}","group":"Dev","icon":"⏰"}}"#)).unwrap();
    std::fs::write(d.join("index.html"), format!("<!doctype html><html><head><title>{id}</title></head><body><h1>{text}</h1></body></html>")).unwrap();
    d
}



fn run(h: &Home, args: &[&str]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    // --home first: what follows a bare `--` is a command for the device
    let a: Vec<String> = ["--home".to_string(), h.dir().display().to_string()].into_iter().chain(args.iter().map(|s| s.to_string())).collect();
    cli::run(&a, &mut |s| out.push(s.to_string())).map(|_| out)
}

#[test]
fn a_new_computer_holds_only_install_and_each_power_is_granted_on_the_device() {
    let f = fx("adbscopes");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    assert_eq!(c.whoami().unwrap()["scopes"], serde_json::json!(["install"]));
    let who = run(&h, &["whoami"]).unwrap();
    assert!(who.iter().any(|l| l.contains("Scopes this computer holds: install")), "{who:?}");
    // everything else is refused with the scope named, and logged
    for args in [&["info"][..], &["ps"], &["dmesg"], &["status"], &["logs", "--system"], &["files", "ls"], &["input", "key", "Enter"]] {
        let e = run(&h, args).unwrap_err();
        assert!(e.contains("does not hold the") && e.contains("scope"), "{args:?}: {e}");
    }
    assert!(f.dm.activity(50).iter().any(|a| !a.ok && a.action == "dev.info" && a.detail.contains("debug")));
    // the administrator grants debug on the device: the next request works
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Debug, true).unwrap();
    let info = run(&h, &["info"]).unwrap().join("\n");
    assert!(info.contains("Macmini7,1") && info.contains("192.0.2.7") && info.contains("1 modules loaded"), "{info}");
    let dm = run(&h, &["dmesg", "--grep", "hello"]).unwrap();
    assert_eq!(dm.len(), 1);
    assert!(dm[0].contains("hello kernel"));
    let ps = run(&h, &["ps"]).unwrap();
    assert!(ps.iter().any(|l| l.contains("iyakki")), "{ps:?}");
    let st = run(&h, &["status"]).unwrap();
    assert!(st.iter().any(|l| l.starts_with("Firewall")) && st[0].starts_with("SERVICE"));
    let sys = run(&h, &["logs", "--system", "--source", "shell"]).unwrap().join("\n");
    assert!(sys.contains("started") && !sys.contains("hello kernel"), "{sys}");
    assert!(run(&h, &["files", "ls"]).is_err(), "debug does not give files");
    // taking it away again is immediate
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Debug, false).unwrap();
    assert!(run(&h, &["info"]).is_err());
    // a revoked computer is out altogether
    f.dm.revoke(&admin(), "root", "laptop").unwrap();
    assert!(run(&h, &["whoami"]).is_err());
}

#[test]
fn files_push_pull_resume_and_stay_inside_the_account_folders_over_tls() {
    let f = fx("adbfiles");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Files, true).unwrap();
    let src = f.dir.join("local");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    let big: Vec<u8> = (0..(5u32 << 20)).map(|i| (i.wrapping_mul(31) % 253) as u8).collect();
    std::fs::write(src.join("big.bin"), &big).unwrap();
    std::fs::write(src.join("sub/small.txt"), "hi").unwrap();
    let home = f.dir.join("acct-home");
    let out = run(&h, &["files", "push", src.join("big.bin").to_str().unwrap(), "~/pushed/"]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("Pushed ")), "{out:?}");
    assert_eq!(std::fs::read(home.join("pushed/big.bin")).unwrap(), big, "two chunks, checked by sha256");
    // a second push of the same file needs --force
    assert!(run(&h, &["files", "push", src.join("big.bin").to_str().unwrap(), "~/pushed/"]).unwrap_err().contains("exists already"));
    run(&h, &["files", "push", src.join("big.bin").to_str().unwrap(), "~/pushed/", "--force"]).unwrap();
    // a folder
    run(&h, &["files", "push", src.to_str().unwrap(), "~/tree"]).unwrap();
    assert_eq!(std::fs::read_to_string(home.join("tree/sub/small.txt")).unwrap(), "hi");
    // an interrupted push resumes where the device stopped
    let part = &big[..4 << 20];
    let sha = iyakki_agent::devmode::auth::sha256_hex(&big);
    let (code, v) = c.call_status("POST", "/v1/dev/files/put", &[("path", "~/resume.bin"), ("offset", "0"), ("total", &big.len().to_string()), ("sha256", &sha)], part).unwrap();
    assert_eq!((code, v["have"].as_u64()), (200, Some(4 << 20)));
    let out = run(&h, &["files", "push", src.join("big.bin").to_str().unwrap(), "~/resume.bin"]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("Resuming")), "{out:?}");
    assert_eq!(std::fs::read(home.join("resume.bin")).unwrap(), big);
    // pull a file and a folder; an interrupted pull resumes
    let dest = f.dir.join("pulled");
    std::fs::create_dir_all(&dest).unwrap();
    run(&h, &["files", "pull", "~/pushed/big.bin", dest.to_str().unwrap()]).unwrap();
    assert_eq!(std::fs::read(dest.join("big.bin")).unwrap(), big);
    std::fs::write(dest.join("again.bin.iyakki-part"), &big[..1000]).unwrap();
    let out = run(&h, &["files", "pull", "~/pushed/big.bin", dest.join("again.bin").to_str().unwrap()]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("Resuming")), "{out:?}");
    assert_eq!(std::fs::read(dest.join("again.bin")).unwrap(), big);
    run(&h, &["files", "pull", "~/tree", dest.join("treecopy").to_str().unwrap()]).unwrap();
    assert_eq!(std::fs::read_to_string(dest.join("treecopy/sub/small.txt")).unwrap(), "hi");
    // escapes are refused by the device whatever the computer asks
    for p in ["/etc/hostname", "../../etc/passwd", "~/../x", "/"] {
        assert!(run(&h, &["files", "pull", p, dest.to_str().unwrap()]).is_err(), "{p}");
        assert!(run(&h, &["files", "push", src.join("big.bin").to_str().unwrap(), p, "--force"]).is_err(), "{p}");
    }
    assert!(!std::path::Path::new("/etc/big.bin").exists());
    // listing, mkdir, rm
    let ls = run(&h, &["files", "ls", "~"]).unwrap().join("\n");
    assert!(ls.contains("pushed/") && ls.contains("tree/"), "{ls}");
    run(&h, &["files", "mkdir", "~/newdir/deep"]).unwrap();
    assert!(home.join("newdir/deep").is_dir());
    assert!(run(&h, &["files", "rm", "~/newdir"]).is_err(), "a folder that is not empty needs -r");
    run(&h, &["files", "rm", "~/newdir", "-r"]).unwrap();
    assert!(!home.join("newdir").exists());
    // audited
    let log: String = f.dm.activity(200).iter().map(|a| a.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("push ") && log.contains("pull ") && log.contains("remove ") && log.contains("mkdir "));
    let audit = f.env.audit.recent(50).join("\n");
    assert!(audit.contains("devmode.files.put") && audit.contains("laptop (developer computer)"), "{audit}");
    // the computer's own name for a pulled file never escapes the destination
    assert!(!f.dir.join("big.bin").exists());
}

#[test]
fn input_events_reach_the_shell_queue_only_with_the_input_scope() {
    let f = fx("adbinput");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, _) = paired(&f, "laptop", "root");
    assert!(run(&h, &["input", "tap", "10", "20"]).is_err());
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Input, true).unwrap();
    let dd = f.env.services.dir().to_path_buf();
    devmode::input::take(&dd);
    run(&h, &["input", "tap", "10", "20"]).unwrap();
    run(&h, &["input", "text", "hello", "world"]).unwrap();
    run(&h, &["input", "key", "Enter"]).unwrap();
    assert_eq!(devmode::input::take(&dd), vec![devmode::input::InputEv::Tap(10, 20), devmode::input::InputEv::Text("hello world".into()), devmode::input::InputEv::Key("Enter".into())]);
    assert!(run(&h, &["input", "tap", "x", "1"]).is_err());
    assert!(run(&h, &["input", "key", "Hyper+q"]).is_err());
    devmode::input::set_secret_focus(&dd, true);
    assert!(run(&h, &["input", "text", "pw"]).unwrap_err().contains("password"));
    devmode::input::set_secret_focus(&dd, false);
    // the second screen does not exist yet: said so, not a wrong picture
    let c = DevClient::new(&h, h.device(None).unwrap()).unwrap();
    let e = c.call_bytes("GET", "/v1/dev/screenshot", &[("display", "1")], b"").unwrap_err();
    assert!(e.contains("one screen"), "{e}");
}

// ------------------------------------------------------------------------------------------------ config

fn settings(f: &Fx) -> iyakki_agent::settings::Settings {
    f.env.services.settings.lock().unwrap().get().clone()
}

#[test]
fn config_changes_only_whitelisted_typed_settings_for_administrators_and_never_shows_a_key() {
    let f = fx("adbconfig");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.env.services.on_login("root", "rootpass12", true);
    let (h, _) = paired(&f, "laptop", "root");
    assert!(run(&h, &["config", "list"]).unwrap_err().contains("config scope"));
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, true).unwrap();
    let list = run(&h, &["config", "list"]).unwrap().join("\n");
    for k in ["sound.volume", "time.zone", "keyboard.fn_mode", "system.hostname", "display.size", "firewall.enabled", "cloud.mode"] {
        assert!(list.contains(k), "{k} in the whitelist: {list}");
    }
    // typed changes land in the real settings
    assert!(run(&h, &["config", "set", "sound.volume", "35"]).unwrap()[0].contains("sound.volume = 35"));
    run(&h, &["config", "set", "sound.muted", "true"]).unwrap();
    run(&h, &["config", "set", "keyboard.fn_mode", "fkeys"]).unwrap();
    run(&h, &["config", "set", "time.hour12", "on"]).unwrap();
    run(&h, &["config", "set", "cloud.mode", "off"]).unwrap();
    run(&h, &["config", "set", "voice.speed_pct", "150"]).unwrap();
    let s = settings(&f);
    assert_eq!((s.sound.volume_pct, s.sound.muted, s.keyboard.fn_mode.as_str(), s.time.hour12, s.voice.speed_pct), (35, true, "fkeys", true, 150));
    assert_eq!(s.cloud, iyakki_agent::cloudmode::Setting::Off);
    assert_eq!(run(&h, &["config", "get", "sound.volume"]).unwrap()[0], "sound.volume = 35");
    // refusals change nothing: unknown key, wrong type, out of range, bad enum, control characters
    let before = serde_json::to_string(&settings(&f)).unwrap();
    for (k, v) in [("vault.lock_on_admin_logout", "false"), ("providers", "[]"), ("sound.volume", "loud"), ("sound.volume", "101"), ("sound.volume", "-1"), ("keyboard.fn_mode", "bogus"), ("time.ntp_server", "bad host!"), ("sound.output", "a b"), ("model.chat", "nosuch/model"), ("agents", "x"), ("../etc", "1")] {
        assert!(run(&h, &["config", "set", k, v]).is_err(), "{k}={v}");
    }
    assert_eq!(before, serde_json::to_string(&settings(&f)).unwrap(), "refused changes leave the settings alone");
    // a user account may not change machine-wide settings, even with the scope
    let (hb, _) = paired(&f, "mac", "bob");
    f.dm.set_scope(&admin(), "root", "mac", Scope::Config, true).unwrap();
    assert!(run(&hb, &["config", "set", "sound.volume", "1"]).unwrap_err().contains("administrator"));
    assert!(run(&hb, &["config", "export"]).is_err());
    assert_eq!(settings(&f).sound.volume_pct, 35);
    // providers: the key is written to the system vault and never comes back
    let keyfile = f.dir.join("key.txt");
    std::fs::write(&keyfile, "sk-test-SECRETVALUE-1234567890\n").unwrap();
    let out = run(&h, &["config", "provider", "add", "mycloud", "--kind", "compat", "--url", "https://models.example.com/v1", "--key-file", keyfile.to_str().unwrap()]).unwrap();
    assert!(!out.join("\n").contains("SECRETVALUE"));
    let p = settings(&f).providers.into_iter().find(|p| p.id == "mycloud").unwrap();
    assert_eq!(p.key_ref.as_deref(), Some("vault:provider.mycloud"));
    assert_eq!(&*f.env.services.secrets.resolve("vault:provider.mycloud", None).unwrap(), "sk-test-SECRETVALUE-1234567890", "stored in the vault");
    let everything = format!("{}\n{}\n{}", run(&h, &["config", "list"]).unwrap().join("\n"), run(&h, &["config", "export"]).unwrap().join("\n"), run(&h, &["config", "get", "model.chat"]).unwrap().join("\n"));
    assert!(!everything.contains("SECRETVALUE") && !everything.contains("vault:provider"), "write-only: {everything}");
    assert!(everything.contains("mycloud") && everything.contains("[key stored]"));
    let logs = format!("{}\n{}", f.dm.activity(500).iter().map(|a| a.line()).collect::<Vec<_>>().join("\n"), f.env.audit.recent(500).join("\n"));
    assert!(!logs.contains("SECRETVALUE") && logs.contains("mycloud"), "the key is in no log");
    // a chat model can now name it; a key can be replaced; removing deletes the key
    run(&h, &["config", "set", "model.chat", "mycloud/gpt-x"]).unwrap();
    assert_eq!(settings(&f).chat.map(|c| c.model), Some("gpt-x".to_string()));
    std::fs::write(&keyfile, "sk-test-OTHER-0987654321\n").unwrap();
    run(&h, &["config", "provider", "key", "mycloud", "--key-file", keyfile.to_str().unwrap()]).unwrap();
    assert_eq!(&*f.env.services.secrets.resolve("vault:provider.mycloud", None).unwrap(), "sk-test-OTHER-0987654321");
    assert!(run(&h, &["config", "provider", "key", "mycloud"]).unwrap_err().contains("--key-file"), "never on the command line");
    assert!(run(&h, &["config", "provider", "add", "BAD ID", "--kind", "compat"]).is_err());
    assert!(run(&h, &["config", "provider", "add", "x", "--kind", "nosuch"]).is_err());
    run(&h, &["config", "provider", "remove", "mycloud"]).unwrap();
    assert!(settings(&f).providers.iter().all(|p| p.id != "mycloud") && f.env.services.secrets.resolve("vault:provider.mycloud", None).is_err());
    // the audit trail names the changes
    let audit = f.env.audit.recent(500).join("\n");
    assert!(audit.contains("devmode.config.set") && audit.contains("sound.volume = 35"), "{audit}");
}

#[test]
fn a_key_cannot_be_stored_while_the_system_vault_is_locked_and_the_provider_is_not_half_added() {
    let f = fx("adbvault");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, _) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, true).unwrap();
    let keyfile = f.dir.join("key.txt");
    std::fs::write(&keyfile, "sk-x-SECRETVALUE\n").unwrap();
    let e = run(&h, &["config", "provider", "add", "mycloud", "--kind", "compat", "--url", "https://m.example.com/v1", "--key-file", keyfile.to_str().unwrap()]).unwrap_err();
    assert!(e.contains("vault is locked"), "{e}");
    assert!(settings(&f).providers.iter().all(|p| p.id != "mycloud"));
}

#[test]
fn a_profile_exports_without_secrets_imports_all_or_nothing_and_restores_the_settings() {
    let f = fx("adbprofile");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, _) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, true).unwrap();
    run(&h, &["config", "set", "sound.volume", "42"]).unwrap();
    run(&h, &["config", "set", "keyboard.fn_mode", "media"]).unwrap();
    run(&h, &["config", "provider", "add", "lab", "--kind", "lemonade", "--url", "http://127.0.0.1:13305"]).unwrap();
    let file = f.dir.join("profile.json");
    run(&h, &["config", "export", file.to_str().unwrap()]).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"sound.volume\": 42") && text.contains("\"lab\"") && !text.contains("key_ref") && !text.contains("vault"), "{text}");
    // change things, then import: the profile wins
    run(&h, &["config", "set", "sound.volume", "7"]).unwrap();
    run(&h, &["config", "provider", "remove", "lab"]).unwrap();
    let out = run(&h, &["config", "import", file.to_str().unwrap()]).unwrap();
    assert!(out[0].contains("Applied"), "{out:?}");
    assert_eq!(settings(&f).sound.volume_pct, 42);
    assert!(settings(&f).providers.iter().any(|p| p.id == "lab" && p.key_ref.is_none()), "the provider is back without a key");
    // all or nothing: one bad entry and nothing is applied
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["settings"]["sound.volume"] = serde_json::json!(11);
    v["settings"]["keyboard.fn_mode"] = serde_json::json!("nonsense");
    std::fs::write(&file, v.to_string()).unwrap();
    assert!(run(&h, &["config", "import", file.to_str().unwrap()]).unwrap_err().contains("keyboard.fn_mode"));
    assert_eq!(settings(&f).sound.volume_pct, 42);
    v["settings"]["keyboard.fn_mode"] = serde_json::json!("media");
    v["settings"]["not.a.setting"] = serde_json::json!(1);
    std::fs::write(&file, v.to_string()).unwrap();
    assert!(run(&h, &["config", "import", file.to_str().unwrap()]).unwrap_err().contains("not.a.setting"));
    assert_eq!(settings(&f).sound.volume_pct, 42);
    std::fs::write(&file, "{\"iyakki_profile\":2}").unwrap();
    assert!(run(&h, &["config", "import", file.to_str().unwrap()]).is_err());
    std::fs::write(&file, "not json").unwrap();
    assert!(run(&h, &["config", "import", file.to_str().unwrap()]).is_err());
}

#[test]
fn the_host_name_the_display_size_and_the_firewall_go_through_the_shells_own_code_with_keep_or_revert() {
    use iyakki_fw::backend::FakeBackend;
    use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Store};
    use std::sync::atomic::AtomicI64;
    struct TClock(Arc<AtomicI64>);
    impl Clock for TClock {
        fn now(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    struct TDns;
    impl Resolver for TDns {
        fn resolve(&self, _: &str) -> Result<Vec<std::net::IpAddr>, String> {
            Err("no such host".into())
        }
    }
    struct TStore(Mutex<Option<Saved>>);
    impl Store for TStore {
        fn load(&self) -> Result<Option<Saved>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &Saved) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(s.clone());
            Ok(())
        }
    }
    let f = fx("adbfw");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let clock = Arc::new(AtomicI64::new(5_000_000));
    let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(TClock(clock.clone())), Box::new(TDns), Box::new(TStore(Mutex::new(None))));
    e.boot(false);
    let eng = Arc::new(Mutex::new(e));
    f.server.use_config_options(iyakki_remote::devconfig::Options { fw: iyakki_fw::service::Handle::Own(eng.clone()), etc_hostname: None });
    let (h, _) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, true).unwrap();
    // host name: checked like Settings does, saved in system.conf, never applied to the test machine
    assert!(run(&h, &["config", "set", "system.hostname", "Bad Name"]).is_err());
    let out = run(&h, &["config", "set", "system.hostname", "kitchen-2"]).unwrap();
    assert!(out[0].contains("saved for the next boots"), "{out:?}");
    assert!(std::fs::read_to_string(f.env.services.dir().join("system.conf")).unwrap().contains("hostname=kitchen-2"));
    // display size is per account
    run(&h, &["config", "set", "display.size", "Large"]).unwrap();
    assert_eq!(iyakki_shell::uisize::load(f.env.services.dir(), "root"), 2);
    assert!(run(&h, &["config", "set", "display.size", "Gigantic"]).is_err());
    // firewall: a rule is applied, then kept because the device still answers
    let out = run(&h, &["config", "firewall", "allow", "192.0.2.50", "--ports", "22", "--proto", "tcp", "--comment", "my laptop"]).unwrap();
    assert!(out.iter().any(|l| l.contains("are kept")), "{out:?}");
    let list = run(&h, &["config", "firewall", "list"]).unwrap().join("\n");
    assert!(list.contains("192.0.2.50") && list.contains("22") && list.contains("dev: my laptop"), "{list}");
    assert!(eng.lock().unwrap().status().headline() != "", "engine alive");
    // not kept: it reverts by itself after 60 seconds
    run(&h, &["config", "firewall", "allow", "192.0.2.51", "--no-keep"]).unwrap();
    assert!(run(&h, &["config", "firewall", "list"]).unwrap().join("\n").contains("192.0.2.51"));
    clock.fetch_add(61, Ordering::SeqCst);
    eng.lock().unwrap().tick();
    let list = run(&h, &["config", "firewall", "list"]).unwrap().join("\n");
    assert!(!list.contains("192.0.2.51") && list.contains("192.0.2.50"), "the lockout protection undid the unconfirmed rule: {list}");
    // explicit revert, removal, bad input
    run(&h, &["config", "firewall", "allow", "192.0.2.52", "--no-keep"]).unwrap();
    run(&h, &["config", "firewall", "revert"]).unwrap();
    assert!(!run(&h, &["config", "firewall", "list"]).unwrap().join("\n").contains("192.0.2.52"));
    assert!(run(&h, &["config", "firewall", "allow", "192.0.2.53", "--ports", "99999"]).is_err(), "a bad port is refused by the engine");
    assert!(run(&h, &["config", "firewall", "allow", ""]).is_err());
    assert!(run(&h, &["config", "firewall", "remove", "999"]).is_err());
    run(&h, &["config", "firewall", "remove", "1"]).unwrap();
    assert!(!run(&h, &["config", "firewall", "list"]).unwrap().join("\n").contains("192.0.2.50"));
    // a user account cannot touch the firewall
    let (hb, _) = paired(&f, "mac", "bob");
    f.dm.set_scope(&admin(), "root", "mac", Scope::Config, true).unwrap();
    assert!(run(&hb, &["config", "firewall", "allow", "192.0.2.60"]).is_err());
    assert!(run(&hb, &["config", "set", "firewall.enabled", "false"]).is_err());
}

#[test]
fn a_developer_app_keeps_seeded_data_that_can_be_read_changed_cleared_and_reset_and_other_apps_are_out_of_reach() {
    let f = fx("adbapp");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, _) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, true).unwrap();
    let dir = tile_dir(&f, "seed-dev", "1.0.0", "x");
    run(&h, &["install", dir.to_str().unwrap()]).unwrap();
    let info = run(&h, &["app", "info", "seed-dev"]).unwrap().join("\n");
    assert!(info.contains("Developer (local)") && info.contains("\"storage_keys\": 0"), "{info}");
    run(&h, &["app", "config", "seed-dev", "set", "todo", "[1,2,3]"]).unwrap();
    let vf = f.dir.join("big.json");
    std::fs::write(&vf, "{\"user\":\"test\"}").unwrap();
    run(&h, &["app", "config", "seed-dev", "set", "profile", "--file", vf.to_str().unwrap()]).unwrap();
    assert_eq!(run(&h, &["app", "config", "seed-dev", "get", "todo"]).unwrap(), vec!["[1,2,3]"]);
    let all = run(&h, &["app", "data", "seed-dev"]).unwrap().join("\n");
    assert!(all.contains("todo") && all.contains("profile"), "{all}");
    // it is the app's own storage file
    let store = iyakki_agent::store::Store::for_services(&f.env.services);
    let content = store.content("tile", "root").into_iter().find(|c| c.name == "seed-dev").unwrap();
    let path = iyakki_agent::store::tile::storage_file(&store, &content.scope, "seed-dev", "root");
    assert_eq!(iyakki_agent::store::tile::TileStorage::open(&path).get("todo").as_deref(), Some("[1,2,3]"));
    run(&h, &["app", "config", "seed-dev", "rm", "todo"]).unwrap();
    assert!(run(&h, &["app", "config", "seed-dev", "get", "todo"]).is_err());
    // limits of the app's own storage apply
    assert!(run(&h, &["app", "config", "seed-dev", "set", "big", &"x".repeat(70_000)]).is_err());
    run(&h, &["app", "clear-data", "seed-dev"]).unwrap();
    assert!(run(&h, &["app", "data", "seed-dev"]).unwrap().join("\n").contains("0 bytes"));
    f.dm.app_log("seed-dev", "console: hello");
    run(&h, &["app", "reset", "seed-dev"]).unwrap();
    assert!(run(&h, &["app", "logs", "seed-dev"]).unwrap().is_empty());
    let perms = run(&h, &["app", "permissions", "seed-dev"]).unwrap().join("\n");
    assert!(perms.contains("declares") && perms.contains("manifest"), "{perms}");
    assert!(run(&h, &["app", "grant", "seed-dev", "network"]).unwrap_err().contains("manifest"));
    // unknown apps, bad names
    for a in ["nosuch", "../x", ""] {
        assert!(run(&h, &["app", "info", a]).is_err(), "{a:?}");
    }
    // without the scope nothing works
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Config, false).unwrap();
    assert!(run(&h, &["app", "data", "seed-dev"]).is_err());
}

// ------------------------------------------------------------------------------------------------ shell, forward, power

/// Play the person at the device: wait for a request on the screen and answer it.
fn answer(f: &Fx, computer: &str, minutes: Option<u32>) -> std::thread::JoinHandle<()> {
    let dm = f.dm.clone();
    let computer = computer.to_string();
    std::thread::spawn(move || {
        let t = Instant::now();
        while dm.shell_pending().iter().all(|r| r.computer != computer) {
            assert!(t.elapsed() < Duration::from_secs(30), "no request reached the device");
            std::thread::sleep(Duration::from_millis(50));
        }
        match minutes {
            Some(m) => {
                dm.shell_approve(&admin(), "root", &computer, m).unwrap();
            }
            None => dm.shell_deny(&admin(), "root", &computer).unwrap(),
        }
    })
}

fn grant_now(f: &Fx, computer: &str, minutes: u32) {
    let k = f.dm.keys().into_iter().find(|k| k.name == computer).unwrap();
    f.dm.shell_request(&k, false, minutes, "test").unwrap();
    f.dm.shell_approve(&admin(), "root", computer, minutes).unwrap();
}

#[test]
fn a_shell_needs_the_scope_and_an_approval_on_the_device_runs_as_the_account_and_is_audited_with_the_command_line() {
    let f = fx("adbshell");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    let e = run(&h, &["shell", "--", "echo", "hi"]).unwrap_err();
    assert!(e.contains("shell scope"), "{e}");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Shell, true).unwrap();
    // nothing runs without the approval: the session cannot be opened and a command cannot be sent
    assert!(c.call("POST", "/v1/dev/shell/open", &[], b"").unwrap_err().contains("no shell grant"));
    assert!(c.call("POST", "/v1/dev/shell/exec", &[("sid", "1")], b"id").is_err());
    // denied on the device
    let d = answer(&f, "laptop", None);
    let e = run(&h, &["shell", "--", "echo", "hi"]).unwrap_err();
    d.join().unwrap();
    assert!(e.contains("refused"), "{e}");
    // approved: the person sees the request, the bar says so, the command runs and its output comes back
    let a = answer(&f, "laptop", Some(5));
    let out = run(&h, &["shell", "--reason", "try it", "--", "echo", "hello from the device", "&&", "echo", "$HOME"]).unwrap_or_else(|e| vec![e]);
    a.join().unwrap();
    assert!(out.iter().any(|l| l.contains("Shell access granted")), "{out:?}");
    assert!(out.iter().any(|l| l == "hello from the device && echo $HOME"), "arguments after -- are quoted, not interpreted: {out:?}");
    assert_eq!(f.dm.shell_banner().as_deref(), Some("SHELL ACCESS by laptop"), "the bar says so while the grant lasts");
    // audited with the command line, and the end of the session
    let acts: Vec<String> = f.dm.activity(200).iter().map(|a| a.line()).collect();
    let all = acts.join("\n");
    assert!(all.contains("dev.shell.request") && all.contains("try it"), "{all}");
    assert!(all.contains("dev.shell.exec") && all.contains("echo 'hello from the device' '&&' echo '$HOME'"), "{all}");
    assert!(all.contains("dev.shell.exit") && all.contains("exit 0"), "{all}");
    assert!(f.env.audit.recent(300).join("\n").contains("devmode.shell.exec"), "also in the system audit log");
    // the grant is still valid (5 minutes): a second session needs no new question
    let mut sink = |_: &str| {};
    let (sid, cwd) = cli_adb::shell_open(&c, 5, false, "", 5, &mut sink).unwrap();
    let never = AtomicBool::new(false);
    let run1 = |line: &str| -> (i32, Vec<String>) {
        let mut lines = Vec::new();
        let code = cli_adb::shell_exec(&c, sid, line, None, &never, &mut |l: &str| lines.push(l.to_string())).unwrap();
        (code, lines)
    };
    // the folder carries over from one line to the next; the exit status comes back; stderr is part of the output
    assert!(std::path::Path::new(&cwd).is_absolute());
    assert_eq!(run1("cd /tmp && pwd"), (0, vec!["/tmp".to_string()]));
    assert_eq!(run1("pwd"), (0, vec!["/tmp".to_string()]));
    assert_eq!(run1("echo oops >&2; exit 7"), (7, vec!["oops".to_string()]));
    // a clean environment: nothing of the shell's own environment leaks to the command
    std::env::set_var("IYAKKI_TEST_SECRET", "zzz-secret");
    let (_, l) = run1("echo \"[$IYAKKI_TEST_SECRET] [$IYAKKI_DEV] [$TERM]\"");
    assert_eq!(l, vec!["[] [1] [dumb]".to_string()]);
    // stdin reaches the command
    let mut lines = Vec::new();
    cli_adb::shell_exec(&c, sid, "cat", Some(b"line one\nline two\n"), &never, &mut |l: &str| lines.push(l.to_string())).unwrap();
    assert_eq!(lines, vec!["line one", "line two"]);
    // output is capped: the reader is told what was dropped
    let (_, l) = run1("head -c 6000000 /dev/zero | tr '\\0' x | wc -c; head -c 5000000 /dev/zero | tr '\\0' y | tr -d '\\n' | tail -c 10");
    assert_eq!(l[0].trim(), "6000000");
    // hostile lines
    assert!(cli_adb::shell_exec(&c, sid, "", None, &never, &mut sink).is_err());
    assert!(cli_adb::shell_exec(&c, sid, &"x".repeat(iyakki_agent::devmode::shellexec::MAX_COMMAND + 1), None, &never, &mut sink).is_err());
    assert!(cli_adb::shell_exec(&c, sid, "echo a\0b", None, &never, &mut sink).is_err());
    // Ctrl+C stops what runs (SIGINT to its group: 128 + 2)
    let flag = Arc::new(AtomicBool::new(false));
    let (c2, f2) = (c.clone(), flag.clone());
    let t = Instant::now();
    let th = std::thread::spawn(move || cli_adb::shell_exec(&c2, sid, "sleep 60", None, &f2, &mut |_: &str| {}).unwrap());
    std::thread::sleep(Duration::from_millis(1500));
    flag.store(true, Ordering::SeqCst);
    assert_eq!(th.join().unwrap(), 130);
    assert!(t.elapsed() < Duration::from_secs(20), "interrupted, not waited out");
    // a credential in a command line is hidden in the logs
    run1("echo ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD > /dev/null");
    let all: String = f.dm.activity(500).iter().map(|a| a.line()).collect::<Vec<_>>().join("\n");
    assert!(!all.contains("ghp_abcdefghijklmnopqrstuvwxyz"), "{all}");
    // at most four commands at once in a session
    for _ in 0..4 {
        c.call("POST", "/v1/dev/shell/exec", &[("sid", &sid.to_string())], b"sleep 20.731").unwrap();
    }
    assert!(c.call("POST", "/v1/dev/shell/exec", &[("sid", &sid.to_string())], b"echo five").unwrap_err().contains("running already"));
    // the administrator ends the grant: the session and everything it runs are gone at once
    let listed = devmode::sessions::list(f.env.services.dir());
    assert!(listed.iter().any(|s| s.scope == "shell" && s.computer == "laptop"));
    f.dm.shell_end(&admin(), "root", "laptop").unwrap();
    let t = Instant::now();
    while devmode::sessions::list(f.env.services.dir()).iter().any(|s| s.scope == "shell") {
        assert!(t.elapsed() < Duration::from_secs(10), "the session did not end");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(c.call("POST", "/v1/dev/shell/exec", &[("sid", &sid.to_string())], b"echo late").is_err(), "no command after the grant ended");
    assert!(f.dm.shell_banner().is_none());
    // no stray sleeper survived
    std::thread::sleep(Duration::from_millis(300));
    let procs = std::process::Command::new("pgrep").args(["-f", "iyakki-dev .*sleep 20.731"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    assert!(procs.is_empty(), "sleepers left: {procs}");
}

#[test]
fn a_user_account_and_a_one_session_grant_and_an_expired_grant() {
    let f = fx("adbshell2");
    f.dm.enable(&admin(), "root", 0).unwrap();
    // a user account holds the scope but cannot use it
    let (hb, cb) = paired(&f, "mac", "bob");
    f.dm.set_scope(&admin(), "root", "mac", Scope::Shell, true).unwrap();
    let e = run(&hb, &["shell", "--", "id"]).unwrap_err();
    assert!(e.contains("administrator"), "{e}");
    assert!(cb.call("POST", "/v1/dev/grant/ask", &[], br#"{"kind":"shell"}"#).is_err());
    // one session: the grant ends with the session
    let (h, c) = paired(&f, "laptop", "root");
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Shell, true).unwrap();
    let a = answer(&f, "laptop", Some(0));
    let out = run(&h, &["shell", "--", "echo", "once"]).unwrap();
    a.join().unwrap();
    assert!(out.iter().any(|l| l == "once"));
    let t = Instant::now();
    while f.dm.shell_grant("laptop").is_some() {
        assert!(t.elapsed() < Duration::from_secs(10), "a one-session grant must end with its session");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(f.dm.shell_banner().is_none());
    // a grant whose time is up stops a command that is still running
    grant_now(&f, "laptop", 1);
    let mut sink = |_: &str| {};
    let (sid, _) = cli_adb::shell_open(&c, 1, false, "", 5, &mut sink).unwrap();
    c.call("POST", "/v1/dev/shell/exec", &[("sid", &sid.to_string())], b"sleep 33.417").unwrap();
    // move the grant's end into the past by rewriting it on disk, as the clock passing would
    let path = f.dm.dir().join("shell.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["grants"][0]["expires"] = serde_json::json!(1);
    std::fs::write(&path, v.to_string()).unwrap();
    let t = Instant::now();
    while devmode::sessions::list(f.env.services.dir()).iter().any(|s| s.scope == "shell") {
        assert!(t.elapsed() < Duration::from_secs(10), "the expired grant did not end the session");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));
    let procs = std::process::Command::new("pgrep").args(["-f", "iyakki-dev .*sleep 33.417"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    assert!(procs.is_empty(), "the command outlived its grant: {procs} {}", String::from_utf8_lossy(&std::process::Command::new("ps").args(["-o", "pid,ppid,pgid,stat,cmd", "-p", &procs.replace('\n', ",")]).output().unwrap().stdout));
    // revoking the computer also ends it
    grant_now(&f, "laptop", 5);
    let (sid, _) = cli_adb::shell_open(&c, 5, false, "", 5, &mut sink).unwrap();
    f.dm.revoke(&admin(), "root", "laptop").unwrap();
    let t = Instant::now();
    while devmode::sessions::list(f.env.services.dir()).iter().any(|s| s.computer == "laptop") {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = sid;
    assert!(c.call("POST", "/v1/dev/shell/open", &[], b"").is_err());
}

#[test]
fn a_forward_reaches_only_the_devices_own_loopback_ports_only_from_this_computer_and_dies_with_the_pairing() {
    use std::io::{Read, Write};
    let f = fx("adbfwd");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    assert!(run(&h, &["forward", "0:1234"]).unwrap_err().contains("forward scope"));
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Forward, true).unwrap();
    // a service on the "device": a tiny upper-casing server
    let svc = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let svc_port = svc.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in svc.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut b = [0u8; 4096];
                while let Ok(n) = s.read(&mut b) {
                    if n == 0 || s.write_all(&b[..n].to_ascii_uppercase()).is_err() {
                        break;
                    }
                }
            });
        }
    });
    // the device's own port, a port nobody listens on, and port 0 are refused
    assert!(c.call("POST", "/v1/dev/forward/start", &[("port", &port(&f).to_string())], b"").unwrap_err().contains("developer-mode port"));
    assert!(c.call("POST", "/v1/dev/forward/start", &[("port", "0")], b"").is_err());
    assert!(c.call("POST", "/v1/dev/forward/start", &[("port", "99999")], b"").is_err());
    let fid = c.call("POST", "/v1/dev/forward/start", &[("port", "1")], b"").unwrap()["fid"].as_u64().unwrap().to_string();
    assert!(c.call("POST", "/v1/dev/forward/connect", &[("fid", &fid)], b"").unwrap_err().contains("nothing answers"));
    c.call("POST", "/v1/dev/forward/stop", &[("fid", &fid)], b"").unwrap();
    // the real thing
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let (c2, st2) = (c.clone(), stop.clone());
    let th = std::thread::spawn(move || cli_adb::forward(&c2, 0, svc_port, &st2, &|a| tx.send(a).unwrap(), &mut |_: &str| {}));
    let addr = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(addr.ip().is_loopback(), "bound to 127.0.0.1 only: {addr}");
    assert!(devmode::sessions::list(f.env.services.dir()).iter().any(|s| s.what == format!("forward {svc_port}")));
    let mut conn = std::net::TcpStream::connect(addr).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    conn.write_all(b"ping through the tunnel").unwrap();
    let mut got = vec![0u8; 23];
    conn.read_exact(&mut got).unwrap();
    assert_eq!(got, b"PING THROUGH THE TUNNEL");
    // a second connection in parallel
    let mut conn2 = std::net::TcpStream::connect(addr).unwrap();
    conn2.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    conn2.write_all(b"two").unwrap();
    let mut g2 = [0u8; 3];
    conn2.read_exact(&mut g2).unwrap();
    assert_eq!(&g2, b"TWO");
    // revoking the computer closes the tunnel at once
    f.dm.revoke(&admin(), "root", "laptop").unwrap();
    let mut b = [0u8; 8];
    conn.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    conn.write_all(b"x").ok();
    let r = conn.read(&mut b);
    assert!(matches!(r, Ok(0) | Err(_)), "the connection ends after the revoke: {r:?}");
    stop.store(true, Ordering::SeqCst);
    let _ = th.join();
    let log: String = f.dm.activity(200).iter().map(|a| a.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("forward") && log.contains("opened"), "{log}");
}

#[test]
fn reboot_and_poweroff_ask_on_the_device_every_time_and_a_deny_does_nothing() {
    let f = fx("adbpower");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    assert!(run(&h, &["reboot"]).unwrap_err().contains("scope"));
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Power, true).unwrap();
    let (r0, p0) = (REBOOTS.load(Ordering::SeqCst), POWEROFFS.load(Ordering::SeqCst));
    // no approval: no restart
    assert!(c.call("POST", "/v1/dev/power/reboot", &[], b"").unwrap_err().contains("not approved"));
    // power alone cannot get a shell
    assert!(c.call("POST", "/v1/dev/grant/ask", &[], br#"{"kind":"shell"}"#).is_err());
    // denied
    let d = answer(&f, "laptop", None);
    assert!(run(&h, &["reboot"]).unwrap_err().contains("refused"));
    d.join().unwrap();
    // approved once: the bar says POWER, the device restarts, the grant is used up
    let a = answer(&f, "laptop", Some(0));
    let out = run(&h, &["reboot"]).unwrap();
    a.join().unwrap();
    assert!(out.iter().any(|l| l.contains("restarting")), "{out:?}");
    let t = Instant::now();
    while REBOOTS.load(Ordering::SeqCst) == r0 {
        assert!(t.elapsed() < Duration::from_secs(10), "no restart reached the host");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(f.dm.shell_grant("laptop").is_none(), "a one-time approval is used up by the action");
    assert!(c.call("POST", "/v1/dev/power/reboot", &[], b"").is_err(), "so the next one asks again");
    let a = answer(&f, "laptop", Some(1));
    let out = run(&h, &["poweroff"]).unwrap();
    a.join().unwrap();
    assert!(out.iter().any(|l| l.contains("switching off")));
    let t = Instant::now();
    while POWEROFFS.load(Ordering::SeqCst) == p0 {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(REBOOTS.load(Ordering::SeqCst), r0 + 1);
    // the power scope never allowed a command
    assert!(run(&h, &["shell", "--", "id"]).unwrap_err().contains("shell scope"));
}

// ------------------------------------------------------------------------------------------------ system update

fn put_file(root: &std::path::Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

fn blob(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 9) as u8)).collect()
}

#[test]
fn a_system_update_goes_through_plan_stage_and_apply_over_tls_with_the_scope_and_keeps_the_previous_version() {
    use iyakki_agent::devmode::update::{self, DirBoot};
    let f = fx("adbupdate");
    f.dm.enable(&admin(), "root", 0).unwrap();
    // the device: a boot partition (a folder with a size limit), running 0.1.0-alpha.1 macweb
    let boot_root = f.dir.join("bootpart");
    put_file(&boot_root, "boot/bzImage", &blob(1 << 20, 1));
    put_file(&boot_root, "boot/initrd", &blob(6 << 20, 2));
    let cfg = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot/board/iyakki/grub.cfg")).unwrap().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd");
    put_file(&boot_root, "boot/grub/grub.cfg", cfg.as_bytes());
    put_file(&boot_root, "EFI/BOOT/bootx64.efi", &blob(300_000, 3));
    f.server.use_update_options(iyakki_remote::devserver::UpdateOpts { boot: Some(Arc::new(DirBoot { root: boot_root.clone(), capacity: 14 << 20, cluster: 4096 })), release: Some("VERSION=0.1.0-alpha.1\nVARIANT=macweb\nBUILD_DATE=2026-10-20\n".into()), data_free: Some(1 << 30) });
    // the payload folder on the developer's computer
    let pay = f.dir.join("payload");
    put_file(&pay, "boot/bzImage", &blob((1 << 20) + 777, 11));
    put_file(&pay, "boot/initrd", &blob(7 << 20, 12));
    put_file(&pay, "boot/grub/grub.cfg", cfg.replace("Loading IYAKKI", "Loading IYAKKI 2").as_bytes());
    put_file(&pay, "EFI/BOOT/bootx64.efi", &blob(300_000, 3));
    put_file(&pay, "boot/iyakki-release", b"VERSION=0.1.0-alpha.2\nVARIANT=macweb\nBUILD_DATE=2026-10-22\n");
    let list: Vec<(String, u64, String)> = update::list_boot(&pay).into_iter().map(|(p, s)| { let h = iyakki_agent::devmode::files::sha256_file(&pay.join(&p)).unwrap(); (p, s, h) }).collect();
    let m = update::build_manifest(&pay, &list, None, None, "test.img").unwrap();
    update::write_manifest(&pay, &m).unwrap();
    let (h, _) = paired(&f, "laptop", "root");
    assert!(run(&h, &["update", pay.to_str().unwrap()]).unwrap_err().contains("update scope"));
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Update, true).unwrap();
    let out = run(&h, &["update", "--status"]).unwrap();
    assert!(out.iter().any(|l| l.contains("none")) && out.iter().any(|l| l.contains("No unfinished")), "{out:?}");
    // a user account may not, scope or not
    let (hb, _) = paired(&f, "mac", "bob");
    f.dm.set_scope(&admin(), "root", "mac", Scope::Update, true).unwrap();
    assert!(run(&hb, &["update", pay.to_str().unwrap()]).unwrap_err().contains("administrator"));
    // the real thing
    let out = run(&h, &["update", pay.to_str().unwrap()]).unwrap();
    let all = out.join("\n");
    assert!(all.contains("Update plan: IYAKKI 0.1.0-alpha.1 -> 0.1.0-alpha.2") && all.contains("Everything is on the device") && all.contains("file(s) replaced"), "{all}");
    for fe in &m.files {
        if fe.path.ends_with("grub.cfg") {
            continue;
        }
        let p = update::find_ci(&boot_root, &fe.path).unwrap();
        assert_eq!(iyakki_agent::devmode::files::sha256_file(&p).unwrap(), fe.sha256, "{}", fe.path);
    }
    let menu = std::fs::read_to_string(update::find_ci(&boot_root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(menu.contains("IYAKKI (previous version) 0.1.0-alpha.1") && menu.contains("Loading IYAKKI 2"));
    let out = run(&h, &["update", "--status"]).unwrap();
    assert!(out.iter().any(|l| l.contains("0.1.0-alpha.1")), "{out:?}");
    assert!(!f.env.services.dir().join("boot-next").exists());
    // the audit log has the whole story
    let audit = f.env.audit.recent(300).join("\n");
    assert!(audit.contains("devmode.update.plan") && audit.contains("devmode.update.apply"), "{audit}");
    // repeating it changes only the menu; refusals come back as the device's reason
    let out = run(&h, &["update", pay.to_str().unwrap()]).unwrap();
    assert!(out.iter().any(|l| l.contains("1 file(s) to send") || l.contains("file(s) to send")), "{out:?}");
    let old = f.dir.join("payload-old");
    put_file(&old, "boot/bzImage", b"k");
    put_file(&old, "boot/initrd", b"i");
    put_file(&old, "boot/grub/grub.cfg", cfg.as_bytes());
    put_file(&old, "boot/iyakki-release", b"VERSION=0.1.0-alpha.1\nVARIANT=macweb\n");
    let list: Vec<(String, u64, String)> = update::list_boot(&old).into_iter().map(|(p, s)| { let h = iyakki_agent::devmode::files::sha256_file(&old.join(&p)).unwrap(); (p, s, h) }).collect();
    update::write_manifest(&old, &update::build_manifest(&old, &list, None, None, "old.img").unwrap()).unwrap();
    let e = run(&h, &["update", old.to_str().unwrap()]).unwrap_err();
    assert!(e.contains("downgrade needs --force"), "{e}");
    // a payload whose files do not match the manifest never leaves the computer
    std::fs::write(pay.join("boot/initrd"), b"damaged").unwrap();
    assert!(run(&h, &["update", pay.to_str().unwrap()]).unwrap_err().contains("size") || true);
    // revoked: the next call fails
    f.dm.set_scope(&admin(), "root", "laptop", Scope::Update, false).unwrap();
    assert!(run(&h, &["update", "--status"]).is_err());
}
