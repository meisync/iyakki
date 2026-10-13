use std::process::ExitCode;

use iyakki_core::{detect_env, init_logger, install_signal_handlers, Grants, Supervisor};
use iyakki_wasm::SandboxConfig;

const USAGE: &str = "\
iyakki — An OS for Agents

USAGE:
  iyakki shell [--fb /dev/fb0]          show the login screen and app tiles on a framebuffer
  iyakki net <up|status>                bring up / list network interfaces (/etc/iyakki/net.conf)
  iyakki detect                         print detected host environment
  iyakki hwreport                       hardware report: firmware, display, keyboard/mouse/touch, network, …
  iyakki storage                        where data lives now and which disks could hold it
  iyakki persist <disk> [--force]       make a disk the IYAKKI data volume (erases it)
  iyakki run <module.wasm|.wat> [opts]  run a guest in the sandbox
      --entry <name>   exported function to call (default: _start)
      --fuel <n>       instruction budget (default: 100000000)
      --mem <MiB>      guest memory cap (default: 64)
      --allow-read <vfs-path>    (repeatable)
      --allow-write <vfs-path>   (repeatable)
      --allow-net <host>         (repeatable; hosts need an egress policy, see iyakki-proxy)
  iyakki serve [--swarm] [--web[=ADDR]] run as supervisor daemon / PID 1 (--swarm: join the LAN swarm;
                                      --web: also serve the desktop to browsers, see below)
  iyakki user add NAME [--role R] [--if-missing] --password-stdin|--password-file F
                                      add an account (no screen needed); iyakki user list
  iyakki release keygen FILE | sign DIR --key FILE | verify DIR --pubkey HEX
                                      sums and signs the images of a release, writes the download page, checks a download
  iyakki top [N]                        what the machine is doing: load, memory and the N busiest processes (default 15)
  iyakki web [ADDR]                     only the browser front end (no screen needed): ADDR is a port, host:port,
                                      or http:port to speak plain HTTP behind a TLS proxy (default HTTPS on 8443);
                                      also from the boot option iyakki.web=ADDR or the environment IYAKKI_WEB
  iyakki web-open URL                   open URL in the optional full web engine (WPE WebKit in the cage compositor; only in the
                                      `web` image variant) and wait; the shell must not be using the screen (docs/FULL-BROWSER.md)
  iyakki swarm-run <module> [--entry n] [--wait <secs>]
                                      run a module on a discovered LAN peer
  iyakki ledger-verify <file>           verify a replay ledger's hash chain
";

/// `println!` that never panics (see `iyakki_core::console`).
macro_rules! out {
    ($($a:tt)*) => { iyakki_core::console::out_write(&format!("{}\n", format_args!($($a)*))) };
}
/// `eprintln!` that never panics.
macro_rules! err {
    ($($a:tt)*) => { iyakki_core::console::err_write(&format!("{}\n", format_args!($($a)*))) };
}
/// `print!` that never panics.
macro_rules! out_raw {
    ($($a:tt)*) => { iyakki_core::console::out_write(&format!($($a)*)) };
}

fn main() -> ExitCode {
    iyakki_core::console::install_panic_hook();
    match iyakki_core::console::run_with_big_stack(real_main) {
        Ok(code) => code,
        Err(_) => {
            // the panic hook already printed what went wrong
            if std::process::id() == 1 {
                // as PID 1, exiting would be a kernel panic that hides the message: stay up long enough to read it
                iyakki_core::console::kmsg(2, "IYAKKI stopped because of the internal error above. Restarting in two minutes; if this repeats, photograph this screen.");
                std::thread::sleep(std::time::Duration::from_secs(120));
                iyakki_core::boot::reboot();
            }
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> ExitCode {
    // the sample MCP server (the MCP tile starts this program again with --mcp-demo)
    #[cfg(feature = "shell")]
    if std::env::args().nth(1).as_deref() == Some("--mcp-demo") {
        iyakki_agent::mcp::demo_serve_stdio();
        return ExitCode::SUCCESS;
    }
    init_logger();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // As PID 1 with no arguments, behave as the supervisor.
    let cmd = args.first().map(String::as_str).unwrap_or(if std::process::id() == 1 { "serve" } else { "help" });
    match cmd {
        "net" => match args.get(1).map(String::as_str) {
            Some("up") => match iyakki_core::net::bring_up(std::path::Path::new("/etc/iyakki/net.conf"), std::path::Path::new("/sys/class/net")) {
                Ok(errs) => {
                    errs.iter().for_each(|e| err!("iyakki net: {e}"));
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e.to_string()),
            },
            Some("status") => {
                for i in iyakki_core::net::list_ifaces(std::path::Path::new("/sys/class/net")) {
                    out!("{:<12} {:?}", i.name, i.kind);
                }
                ExitCode::SUCCESS
            }
            _ => fail("usage: iyakki net <up|status>".into()),
        },
        "storage" => storage_cmd(),
        "hwreport" => {
            out_raw!("{}", iyakki_hwinfo::collect(&iyakki_hwinfo::Roots::real()).to_text());
            ExitCode::SUCCESS
        }
        "persist" => persist_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "web-open" => match args.get(1) {
            Some(url) => web_open(url),
            None => fail("usage: iyakki web-open URL".into()),
        },
        "detect" => {
            out!("{}", detect_env());
            ExitCode::SUCCESS
        }
        #[cfg(feature = "shell")]
        "shell" => run_shell(&args[1..]),
        #[cfg(feature = "web")]
        "web" => run_web(&args[1..]),
        #[cfg(feature = "shell")]
        "user" => user_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "release" => release_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "top" => {
            let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(15).clamp(1, 200);
            out_raw!("{}", iyakki_shell::apps::system::text_report(&iyakki_agent::sysinfo::SysRoots::real(), n));
            ExitCode::SUCCESS
        }
        "ledger-verify" => match args.get(1).map(std::fs::read) {
            Some(Ok(b)) => match iyakki_ledger::Ledger::from_bytes(&b) {
                Ok(l) => {
                    out!("ok: {} entries, {} calls", l.entries.len(), l.calls().count());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(format!("invalid ledger: {e}")),
            },
            Some(Err(e)) => fail(e.to_string()),
            None => fail("missing ledger path".into()),
        },
        #[cfg(feature = "swarm")]
        "swarm-run" => swarm_run(&args[1..]),
        "serve" => {
            // Real boot = we are PID 1 *and* nothing has mounted /proc yet. A container's PID 1 (docker, k3s)
            // already has /proc, and must never try to mount filesystems or load kernel modules.
            let pid1 = std::process::id() == 1 && !std::path::Path::new("/proc/self").exists();
            // browser access: --web[=ADDRESS], IYAKKI_WEB, or (on a booted machine) the boot option iyakki.web
            let mut web_value: Option<String> = args
                .iter()
                .find_map(|a| a.strip_prefix("--web=").map(String::from))
                .or_else(|| args.iter().any(|a| a == "--web").then(|| "1".to_string()))
                .or_else(|| std::env::var("IYAKKI_WEB").ok());
            if pid1 {
                // /proc must exist before the command line can be read
                std::env::set_var("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
                iyakki_core::boot::mount_pseudo_filesystems();
                let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
                if web_value.is_none() {
                    web_value = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.web").map(String::from);
                }
                // Self-tests need the network up when boot returns; a normal start shows the sign-in screen as soon as
                // storage, input and display are ready and brings up the network and sound in the background.
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest").is_some() {
                    iyakki_core::boot::early_boot();
                } else {
                    let late = std::sync::Arc::new(std::sync::Mutex::new(Some(iyakki_core::boot::early_boot_critical())));
                    let theirs = late.clone();
                    let spawned = std::thread::Builder::new().name("iyakki-late-boot".into()).stack_size(iyakki_core::console::THREAD_STACK).spawn(move || {
                        let plan = theirs.lock().ok().and_then(|mut p| p.take());
                        if let Some(plan) = plan {
                            iyakki_core::boot::late_boot(plan);
                        }
                    });
                    if let Err(e) = spawned {
                        // no thread: do it here, slower but the network still comes up
                        log::error!("late boot: could not start its thread ({e}); running it now");
                        let plan = late.lock().ok().and_then(|mut p| p.take());
                        if let Some(plan) = plan {
                            iyakki_core::boot::late_boot(plan);
                        }
                    }
                }
                // two-boot persistence test: persist1 turns the disk named by iyakki.disk into the data volume
                // (carrying a marker file over), persist2 checks the data survived the reboot
                match iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest") {
                    Some("persist1") => {
                        let disk = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.disk").unwrap_or("vda").to_string();
                        let _ = std::fs::write(format!("{}/marker", iyakki_core::boot::DATA_DIR), "iyakki-persist-ok");
                        let before = iyakki_storage::data_location(&iyakki_storage::Roots::real(), std::path::Path::new(iyakki_core::boot::DATA_DIR));
                        out!("IYAKKI-SELFTEST persist1 location-before {before:?}");
                        let ok = persist_cmd(&[disk, "--force".into()]) == ExitCode::SUCCESS;
                        out!("IYAKKI-SELFTEST persist1 {}", if ok { "ok" } else { "failed" });
                        iyakki_core::boot::power_off();
                    }
                    // an installed disk's first boot formats the marked partition; the second must see what the first wrote
                    Some("install1") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST install1 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        let _ = std::fs::write(dir.join("marker"), "iyakki-install-ok");
                        iyakki_core::boot::power_off();
                    }
                    Some("install2") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST install2 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        out!("IYAKKI-SELFTEST install2 marker {:?}", std::fs::read_to_string(dir.join("marker")).unwrap_or_else(|e| format!("missing: {e}")));
                        iyakki_core::boot::power_off();
                    }
                    Some("persist2") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST persist2 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        out!("IYAKKI-SELFTEST persist2 marker {:?}", std::fs::read_to_string(dir.join("marker")).unwrap_or_else(|e| format!("missing: {e}")));
                        storage_cmd();
                        out!("IYAKKI-SELFTEST persist2 end");
                        iyakki_core::boot::power_off();
                    }
                    _ => {}
                }
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest") == Some("1") {
                    selftest();
                    iyakki_core::boot::power_off();
                }
                #[cfg(feature = "shell")]
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.shell") != Some("0") {
                    start_shell_thread();
                }
                // TEST ONLY (qemu-web-test.sh): open an address in the full web engine at boot, without the shell
                #[cfg(feature = "shell")]
                if let Some(url) = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.webtest").filter(|u| !u.is_empty()) {
                    let url = url.to_string();
                    let _ = std::thread::Builder::new().name("iyakki-webtest".into()).spawn(move || {
                        // wait for a default route (DHCP) so real sites can load
                        for _ in 0..240 {
                            if std::fs::read_to_string("/proc/net/route").is_ok_and(|r| r.lines().skip(1).any(|l| l.split_whitespace().nth(1) == Some("00000000"))) {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                        let code = web_open(&url);
                        out!("IYAKKI-WEBTEST ended {code:?}");
                    });
                }
            }
            #[cfg(feature = "web")]
            let _web = web_value.as_deref().and_then(start_web);
            #[cfg(not(feature = "web"))]
            let _ = web_value;
            let shutdown = match install_signal_handlers() {
                Ok(f) => f,
                Err(e) => return fail(format!("signal setup: {e}")),
            };
            #[cfg(feature = "swarm")]
            let _swarm = if args.iter().any(|a| a == "--swarm") {
                match start_swarm() {
                    Ok(x) => Some(x),
                    Err(e) => return fail(e),
                }
            } else {
                None
            };
            match Supervisor::new(detect_env(), SandboxConfig::default()) {
                Ok(s) => {
                    s.serve(&shutdown);
                    if pid1 {
                        iyakki_core::boot::power_off();
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e.to_string()),
            }
        }
        "run" => run(&args[1..]),
        _ => {
            out_raw!("{USAGE}");
            if cmd == "help" || cmd == "--help" { ExitCode::SUCCESS } else { ExitCode::from(2) }
        }
    }
}

fn run(a: &[String]) -> ExitCode {
    let Some(path) = a.first() else { return fail("missing module path".into()) };
    let (mut entry, mut cfg, mut grants) = ("_start".to_string(), SandboxConfig::default(), Grants::default());
    let mut i = 1;
    while i < a.len() {
        let Some(v) = a.get(i + 1) else { return fail(format!("{} needs a value", a[i])) };
        match a[i].as_str() {
            "--entry" => entry = v.clone(),
            "--fuel" => match v.parse() { Ok(n) => cfg.fuel = n, Err(_) => return fail("bad --fuel".into()) },
            "--mem" => match v.parse::<usize>() { Ok(n) => cfg.max_memory_bytes = n << 20, Err(_) => return fail("bad --mem".into()) },
            "--allow-read" => grants.read.push(v.clone()),
            "--allow-write" => grants.write.push(v.clone()),
            "--allow-net" => grants.net.push(v.clone()),
            other => return fail(format!("unknown option {other}")),
        }
        i += 2;
    }
    let wasm = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let sup = match Supervisor::new(detect_env(), cfg) {
        Ok(s) => s,
        Err(e) => return fail(e.to_string()),
    };
    match sup.run_task(&wasm, &entry, &grants) {
        Ok(o) => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&o.log);
            log::info!("done: result={:?} fuel_used={}", o.result, o.fuel_used);
            ExitCode::from(o.result.unwrap_or(0).clamp(0, 255) as u8)
        }
        Err(e) => fail(e.to_string()),
    }
}

/// `iyakki storage`: where data lives now, and the disks that could hold it.
fn storage_cmd() -> ExitCode {
    let roots = iyakki_storage::Roots::real();
    let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
    match iyakki_storage::data_location(&roots, dir) {
        iyakki_storage::DataLocation::Disk(d) => out!("data: persistent ({d})"),
        iyakki_storage::DataLocation::Memory => out!("data: RAM only (lost at power-off)"),
        iyakki_storage::DataLocation::Other(o) => out!("data: {o}"),
    }
    let all = iyakki_storage::list(&roots);
    for d in &all {
        let status = match iyakki_storage::eligibility(d, &all) {
            Err(why) => format!("not available: {why}"),
            Ok(None) => "free".into(),
            Ok(Some(what)) => format!("holds {what}"),
        };
        out!("{:<10} {:>9}  {:<16} {:<24} {status}", d.name, d.human_size(), d.model, d.content.describe());
    }
    ExitCode::SUCCESS
}

/// `iyakki persist <disk> [--force]`: format a disk as the IYAKKI-DATA volume (erases it).
fn persist_cmd(args: &[String]) -> ExitCode {
    let force = args.iter().any(|a| a == "--force");
    let Some(dev) = args.iter().find(|a| !a.starts_with("--")) else { return fail("usage: iyakki persist <disk, e.g. vdb or sda1> [--force]   (erases the disk)".into()) };
    let roots = iyakki_storage::Roots::real();
    let mke2fs = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists());
    let Some(mke2fs) = mke2fs else { return fail("mke2fs is not installed".into()) };
    match iyakki_storage::persist(&roots, &iyakki_storage::SysMounter, &mke2fs, std::path::Path::new("/run"), dev, force, Some(std::path::Path::new(iyakki_core::boot::DATA_DIR))) {
        Ok(r) => {
            out!("{} is now the IYAKKI data disk; {} files ({} bytes) copied. Reboot to start using it.", r.device, r.files_copied, r.bytes_copied);
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn fail(msg: String) -> ExitCode {
    err!("iyakki: {msg}");
    ExitCode::FAILURE
}

#[cfg(feature = "swarm")]
fn start_swarm() -> Result<(tokio::runtime::Runtime, iyakki_swarm::Node), String> {
    let rt = tokio::runtime::Builder::new_multi_thread().thread_stack_size(iyakki_core::console::THREAD_STACK).enable_all().build().map_err(|e| e.to_string())?;
    let node = {
        let _g = rt.enter();
        iyakki_swarm::Node::start("/ip4/0.0.0.0/tcp/0".parse().unwrap(), SandboxConfig::default()).map_err(|e| e.to_string())?
    };
    log::info!("swarm node {} up; discovering peers via mDNS", node.peer_id);
    Ok((rt, node))
}

#[cfg(feature = "swarm")]
fn swarm_run(a: &[String]) -> ExitCode {
    let Some(path) = a.first() else { return fail("missing module path".into()) };
    let mut entry = "_start".to_string();
    let mut wait = 10u64;
    let mut i = 1;
    while i < a.len() {
        match (a[i].as_str(), a.get(i + 1)) {
            ("--entry", Some(v)) => entry = v.clone(),
            ("--wait", Some(v)) => wait = v.parse().unwrap_or(10),
            _ => return fail(format!("bad option {}", a[i])),
        }
        i += 2;
    }
    let wasm = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let (rt, node) = match start_swarm() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let res = rt.block_on(async {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
        loop {
            match node.submit_any(iyakki_swarm::TaskRequest { wasm: wasm.clone(), entry: entry.clone() }).await {
                Err(iyakki_swarm::SwarmError::NoPeers) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await
                }
                other => break other,
            }
        }
    });
    match res {
        Ok((peer, r)) => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&r.log);
            if r.ok {
                log::info!("ran on {peer}: result={:?}", r.result);
                ExitCode::SUCCESS
            } else {
                fail(format!("remote {peer}: {}", r.error))
            }
        }
        Err(e) => fail(e.to_string()),
    }
}

/// Boot self-test (kernel arg `iyakki.selftest=1`): prints machine facts and runs a sandboxed
/// guest, so a boot in QEMU or on real hardware can be verified from the serial console.
fn selftest() {
    out!("IYAKKI-SELFTEST begin");
    out!("IYAKKI-SELFTEST env={}", detect_env());
    for l in iyakki_hwinfo::collect(&iyakki_hwinfo::Roots::real()).to_text().lines().filter(|l| !l.trim().is_empty()) {
        out!("IYAKKI-HWREPORT {l}");
    }
    for i in iyakki_core::net::list_ifaces(std::path::Path::new("/sys/class/net")) {
        out!("IYAKKI-SELFTEST iface {} {:?}", i.name, i.kind);
    }
    // Voice needs the ALSA command-line tools and a sound device.
    for tool in ["arecord", "aplay"] {
        let found = std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(tool).exists()) || std::path::Path::new("/usr/bin").join(tool).exists();
        out!("IYAKKI-SELFTEST tool {tool} {}", if found { "ok" } else { "missing" });
    }
    if let Ok(o) = std::process::Command::new("arecord").arg("-l").output() {
        for l in String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.starts_with("card")) {
            out!("IYAKKI-SELFTEST audio-capture {l}");
        }
    }
    // Give DHCP a moment, then report addresses.
    std::thread::sleep(std::time::Duration::from_secs(4));
    if let Ok(o) = std::process::Command::new("ip").args(["-br", "addr"]).output() {
        for l in String::from_utf8_lossy(&o.stdout).lines() {
            out!("IYAKKI-SELFTEST addr {l}");
        }
    }
    let wat = r#"(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1)
      (data (i32.const 0) "guest-ok") (func (export "_start") (call $l (i32.const 0) (i32.const 8))))"#;
    match Supervisor::new(detect_env(), SandboxConfig::default()).and_then(|s| s.run_task(wat.as_bytes(), "_start", &Grants::default())) {
        Ok(o) => out!("IYAKKI-SELFTEST wasm {}", String::from_utf8_lossy(&o.log)),
        Err(e) => out!("IYAKKI-SELFTEST wasm-error {e}"),
    }
    out!("IYAKKI-SELFTEST end");
}

/// The one set of accounts, services and settings every shell on this machine (the screen, browsers) works over.
#[cfg(feature = "shell")]
static SHARED_ENV: std::sync::Mutex<Option<iyakki_shell::shell::ShellEnv>> = std::sync::Mutex::new(None);

#[cfg(feature = "shell")]
fn shell_env() -> Result<iyakki_shell::shell::ShellEnv, String> {
    let mut shared = SHARED_ENV.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(e) = shared.as_ref() {
        return Ok(e.share());
    }
    let env = build_shell_env()?;
    let mine = env.share();
    *shared = Some(env);
    Ok(mine)
}

#[cfg(feature = "shell")]
fn build_shell_env() -> Result<iyakki_shell::shell::ShellEnv, String> {
    let host = std::sync::Arc::new(iyakki_core::host::CoreHost::new());
    let hwcheck = iyakki_core::boot::cmdline_param(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default(), "iyakki.hwcheck") == Some("1");
    let paths = iyakki_shell::shell::ShellPaths { hwcheck, ..Default::default() };
    iyakki_shell::shell::ShellEnv::from_paths(&paths, host).map_err(|e| e.to_string())
}

/// Run the optional full web engine on `url` and wait for it (no shell involved; used by `iyakki web-open` and the
/// `iyakki.webtest` boot test).
#[cfg(feature = "shell")]
fn web_open(url: &str) -> ExitCode {
    if !iyakki_agent::webengine::available() {
        return fail(format!("{}.", iyakki_agent::webengine::NOT_INSTALLED));
    }
    let stop = std::sync::atomic::AtomicBool::new(false);
    match iyakki_agent::webengine::run(std::path::Path::new(iyakki_core::boot::DATA_DIR), "root", url, &stop) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => fail(format!("the full browser stopped with status {code} (see /run/iyakki-web.log)")),
        Err(e) => fail(e),
    }
}

/// Wait for a display to appear (drivers load asynchronously), then run the shell on it.
#[cfg(feature = "shell")]
fn start_shell_thread() {
    // a big stack (the renderer and the apps are stack hungry) and a guard: if the shell panics the message is
    // printed (panic hook) and the shell starts again, instead of the whole system going down with it
    let spawn = std::thread::Builder::new().name("iyakki-shell".into()).stack_size(iyakki_core::console::SHELL_STACK).spawn(|| {
        let fb = "/dev/fb0";
        iyakki_core::boot::progress(98, "Starting the desktop");
        for _ in 0..300 {
            if std::path::Path::new(fb).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !std::path::Path::new(fb).exists() {
            log::warn!("shell: no framebuffer appeared; running headless");
            return;
        }
        for attempt in 1..=5 {
            let run = std::panic::catch_unwind(|| match shell_env() {
                Ok(env) => {
                    let mut sh = iyakki_shell::shell::Shell::new(env);
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    // the boot screen lets go of the display before the desktop draws on it
                    iyakki_core::boot::finish_boot_screen();
                    if let Err(e) = iyakki_shell::backend::run_fb(&mut sh, fb, &stop) {
                        log::error!("shell: {e}");
                    }
                }
                Err(e) => log::error!("shell: {e}"),
            });
            match run {
                Ok(()) => return,
                Err(_) => {
                    log::error!("shell: crashed (attempt {attempt} of 5), restarting");
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        }
        log::error!("shell: giving up after repeated crashes; the supervisor keeps running");
    });
    if let Err(e) = spawn {
        log::error!("shell: could not start the shell thread: {e}");
    }
}

/// `iyakki release keygen FILE | sign DIR --key FILE [--title T] [--version V] | verify DIR --pubkey HEX`: sums, signs and checks the
/// images of a release (see `iyakki_agent::release`).
#[cfg(feature = "shell")]
fn release_cmd(a: &[String]) -> ExitCode {
    use iyakki_agent::release;
    let flag = |name: &str| a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned();
    match a.first().map(String::as_str) {
        Some("keygen") => match a.get(1) {
            Some(f) => match release::keygen(std::path::Path::new(f)) {
                Ok(public) => {
                    out!("secret key written to {f} (keep it private and backed up)");
                    out!("public key: {public}");
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            },
            None => fail("usage: iyakki release keygen FILE".into()),
        },
        Some("sign") => {
            let (Some(dir), Some(key)) = (a.get(1).filter(|d| !d.starts_with("--")), flag("--key")) else { return fail("usage: iyakki release sign DIR --key FILE [--title T] [--version V]".into()) };
            match release::sign_dir(std::path::Path::new(dir), std::path::Path::new(&key), &flag("--title").unwrap_or_else(|| "IYAKKI".into()), &flag("--version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into())) {
                Ok(entries) => {
                    for e in &entries {
                        out!("{}  {}  ({} bytes)", e.sha256, e.name, e.bytes);
                    }
                    out!("wrote {}, {} and {} in {dir}; public key {}", release::SUMS, release::SIG, release::PAGE, release::public_key_of(std::path::Path::new(&key)).unwrap_or_default());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        Some("verify") => {
            let (Some(dir), Some(pubkey)) = (a.get(1).filter(|d| !d.starts_with("--")), flag("--pubkey")) else { return fail("usage: iyakki release verify DIR --pubkey HEX".into()) };
            match release::verify_dir(std::path::Path::new(dir), &pubkey) {
                Ok(list) => {
                    let mut bad = 0;
                    for v in &list {
                        if v.ok {
                            out!("ok      {}", v.name);
                        } else {
                            bad += 1;
                            out!("FAILED  {}: {}", v.name, v.why);
                        }
                    }
                    if bad == 0 { ExitCode::SUCCESS } else { fail(format!("{bad} file(s) failed")) }
                }
                Err(e) => fail(e),
            }
        }
        _ => fail("usage: iyakki release <keygen FILE | sign DIR --key FILE | verify DIR --pubkey HEX>".into()),
    }
}

/// `iyakki user add NAME [--role admin|operator|user|guest] [--password-stdin | --password-file FILE]` and `iyakki user list`:
/// accounts for a machine with no screen (a server, a container) where the first-run setup screen cannot be used.
#[cfg(feature = "shell")]
fn user_cmd(a: &[String]) -> ExitCode {
    use iyakki_sec::policy::Role;
    let users_path = std::env::var("IYAKKI_USERS").unwrap_or_else(|_| iyakki_shell::shell::ShellPaths::default().users);
    let mut store = match iyakki_shell::accounts::UserStore::open(std::path::Path::new(&users_path)) {
        Ok(s) => s,
        Err(e) => return fail(format!("{users_path}: {e}")),
    };
    match a.first().map(String::as_str) {
        Some("list") => {
            for u in store.list() {
                out!("{}\t{}", u.name, u.role.name());
            }
            ExitCode::SUCCESS
        }
        Some("add") => {
            let Some(name) = a.get(1).filter(|n| !n.starts_with("--")) else { return fail("usage: iyakki user add NAME [--role R] [--if-missing] [--password-stdin | --password-file FILE]".into()) };
            let mut role = if store.is_empty() { Role::Admin } else { Role::User };
            let mut pw: Option<String> = None;
            let mut if_missing = false;
            let mut i = 2;
            while i < a.len() {
                match a[i].as_str() {
                    "--role" => match a.get(i + 1).and_then(|r| Role::parse(r)) {
                        Some(r) => role = r,
                        None => return fail("--role is one of admin, operator, user, guest".into()),
                    },
                    "--if-missing" => if_missing = true,
                    "--password-stdin" => {
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line).is_err() {
                            return fail("could not read the password from standard input".into());
                        }
                        pw = Some(line.trim_end_matches(['\n', '\r']).to_string());
                    }
                    "--password-file" => match a.get(i + 1).map(std::fs::read_to_string) {
                        Some(Ok(t)) => pw = Some(t.trim_end_matches(['\n', '\r']).to_string()),
                        Some(Err(e)) => return fail(format!("password file: {e}")),
                        None => return fail("--password-file needs a path".into()),
                    },
                    _ => {}
                }
                i += if matches!(a[i].as_str(), "--role" | "--password-file") { 2 } else { 1 };
            }
            let Some(pw) = pw else { return fail("give the password with --password-stdin or --password-file (never on the command line)".into()) };
            match store.add_local(name, &pw, role) {
                Ok(()) => {
                    out!("added {name} ({})", role.name());
                    ExitCode::SUCCESS
                }
                Err(iyakki_shell::accounts::AccountError::Exists) if if_missing => {
                    out!("{name} already exists");
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e.to_string()),
            }
        }
        _ => fail("usage: iyakki user <add NAME [--role R] [--password-stdin|--password-file F] | list>".into()),
    }
}

/// Start the browser front end described by `value` (the part after `iyakki.web=`); the server stops when dropped.
#[cfg(feature = "web")]
fn start_web(value: &str) -> Option<iyakki_remote::server::Server> {
    let spec = iyakki_core::web::parse_spec(value)?;
    let env = match shell_env() {
        Ok(e) => e,
        Err(e) => {
            log::error!("web: {e}");
            return None;
        }
    };
    match iyakki_core::web::start(&spec, env, std::path::Path::new("/var/lib/iyakki")) {
        Ok(s) => Some(s),
        Err(e) => {
            log::error!("web: {e}");
            None
        }
    }
}

/// `iyakki web [ADDRESS]`: only the browser front end, for a server or a container with no screen.
#[cfg(feature = "web")]
fn run_web(a: &[String]) -> ExitCode {
    let value = a.first().map(String::as_str).unwrap_or("1");
    let shutdown = match install_signal_handlers() {
        Ok(f) => f,
        Err(e) => return fail(format!("signal setup: {e}")),
    };
    let Some(_server) = start_web(value) else { return fail("the web front end did not start (see the log; the address may be wrong or in use)".into()) };
    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    ExitCode::SUCCESS
}

#[cfg(feature = "shell")]
fn run_shell(a: &[String]) -> ExitCode {
    let fb = match (a.first().map(String::as_str), a.get(1)) {
        (Some("--fb"), Some(p)) => p.clone(),
        (None, _) => "/dev/fb0".to_string(),
        _ => return fail("usage: iyakki shell [--fb /dev/fb0]".into()),
    };
    let env = match shell_env() {
        Ok(e) => e,
        Err(e) => return fail(e),
    };
    let shutdown = match install_signal_handlers() {
        Ok(f) => f,
        Err(e) => return fail(format!("signal setup: {e}")),
    };
    let mut sh = iyakki_shell::shell::Shell::new(env);
    match iyakki_shell::backend::run_fb(&mut sh, &fb, &shutdown) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
