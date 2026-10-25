use std::process::ExitCode;

use iyakki_core::{detect_env, init_logger, install_signal_handlers, Grants, Supervisor};
use iyakki_wasm::SandboxConfig;

const USAGE: &str = "\
iyakki — An OS for Agents

USAGE:
  iyakki --version                      print the version (and, on an installed image, its /etc/iyakki-release)
  iyakki shell [--fb /dev/fb0]          show the login screen and app tiles on a framebuffer
  iyakki net <up|status>                bring up / list network interfaces (/etc/iyakki/net.conf)
  iyakki net mac [show|off|per-network|random|apply] [--iface X] [--wifi|--ethernet] [--yes]
                                      private MAC address: show it, choose a mode (needs root), or change it now
  iyakki detect                         print detected host environment
  iyakki hwreport                       hardware report: firmware, display, keyboard/mouse/touch, network, …
  iyakki storage                        where data lives now and which disks could hold it
  iyakki storage status|reattach        the drive's health; reconnect it after a USB loss
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
  iyakki firewall off|status            the emergency way out of a firewall lockout, from a console or SSH: off removes the rules and keeps
                                      the firewall off until it is switched on in Settings (docs/FIREWALL.md); status shows the settings and the kernel
  iyakki top [N]                        what the machine is doing: load, memory and the N busiest processes (default 15)
  iyakki quick-install k3s|duckdb|elasticsearch [--data DIR]
                                      one Quick install step (Store -> Setup runs it as a detached job)
  iyakki linux-apps --data DIR --user NAME create|install|remove|upgrade|update|run|import ...
                                      the Linux apps (Debian/Ubuntu sandbox) steps the IYAKKI menu runs as detached jobs (docs/LINUX-APPS.md)
  iyakki store-manage reset|remove --kubeconfig FILE --namespace NS [--delete-data] [--wait SECS] [--force]
                                      the cluster side of resetting or removing the private store (run as a detached job)
  iyakki web [ADDR]                     only the browser front end (no screen needed): ADDR is a port, host:port,
                                      or http:port to speak plain HTTP behind a TLS proxy (default HTTPS on 8443);
                                      also from the boot option iyakki.web=ADDR or the environment IYAKKI_WEB
  iyakki web-open URL                   open URL in the optional full web engine (WPE WebKit in the cage compositor; only in the
                                      `web` image variant) and wait; the shell must not be using the screen (docs/FULL-BROWSER.md)
  iyakki browser-test SPEC.json [--out DIR] [--json] [--profile DIR] [--size WxH]
                                      run a browser test spec (no model): the steps drive a real page in the full web engine and a
                                      pass/fail report is printed; the spec's allow_domains are approved by running it (docs/BROWSER-AGENT.md)
  iyakki swarm-run <module> [--entry n] [--wait <secs>]
                                      run a module on a discovered LAN peer
  iyakki ledger-verify <file>           verify a replay ledger's hash chain
  iyakki pkg <keygen|build|sign|verify|catalog-add|cert-issue|revoke-sign|license-issue|validate|pack|publish>
                                      make and check signed app-store packages, catalogs and licences, offline; publish to a private store
                                      (docs/STORE.md, docs/APP-AUTHORING.md)
  iyakki store-server --data DIR [--listen 0.0.0.0:8080] [--publish-token-file F] [--catalog-key-file F | --catalog-seed-file F]
                      [--read-only] [--require-trusted] [--id ID] [--name N] [--max-body-mib N] | publisher-add --pub-id ID [--pub-name N]
                                      a private app store: signed catalog, packages and publishing over HTTP (docs/STORE.md, section 13)
  iyakki dev <pair|devices|install|uninstall|list|logs|run|push|screenshot|whoami>
                                      from a developer's computer: pair once with a device in developer mode, then install, upgrade,
                                      remove, open and watch apps on it, push a folder on every change (docs/DEVELOPER-MODE.md)
  iyakki sdk <init|doctor|docs>         start a tile/skill/MCP/service project with agent instructions and typings, check this computer for
                                      the device loop, read the guides offline (dist/sdk/README.md); the emulator is specified, not built
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
    // the inside of a Linux apps sandbox (docs/LINUX-APPS.md): it forks, so it runs before anything starts a thread
    if std::env::args().nth(1).as_deref() == Some(iyakki_agent::linuxapps_init::SUBCOMMAND) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        return ExitCode::from(iyakki_agent::linuxapps_init::main(&args).clamp(0, 255) as u8);
    }
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
    // speech engines already on this computer are found in the background and used for reading aloud (docs/VOICE.md)
    #[cfg(feature = "shell")]
    iyakki_agent::hostengines::enable_by_default();
    // the sign-in relay page for Claude Code / Copilot logins: a listener of its own is started only while a login waits for a code
    // (the web front end's port is used when it runs); the certificate is the web front end's
    #[cfg(feature = "web")]
    iyakki_agent::extagent::relay::set_listener(std::sync::Arc::new(iyakki_remote::relay::LazyListener::new(std::path::Path::new("/var/lib/iyakki/tls"))));
    // the sample MCP server (the MCP tile starts this program again with --mcp-demo)
    #[cfg(feature = "shell")]
    if std::env::args().nth(1).as_deref() == Some("--mcp-demo") {
        iyakki_agent::mcp::demo_serve_stdio();
        return ExitCode::SUCCESS;
    }
    // an external coding agent's view of IYAKKI's tools (docs/EXTERNAL-AGENTS.md): a relay to the run's local socket
    #[cfg(feature = "shell")]
    if std::env::args().nth(1).as_deref() == Some("--mcp-tools") {
        let code = iyakki_agent::extagent::bridge::relay_stdio(&std::env::args().nth(2).unwrap_or_default());
        return if code == 0 { ExitCode::SUCCESS } else { ExitCode::from(code as u8) };
    }
    // git's GIT_ASKPASS: this program started under the name iyakki-git-askpass answers git's prompt from the IDE (docs/GIT-AND-SSH.md)
    if let Some(code) = iyakki_agent::gitagent::maybe_askpass(&std::env::args().next().unwrap_or_default(), &std::env::args().skip(1).collect::<Vec<_>>()) {
        return if code == 0 { ExitCode::SUCCESS } else { ExitCode::from(code as u8) };
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
            Some("mac") => match iyakki_devctl::mac::parse_cmd(&args[2..]) {
                Ok(c) => {
                    // SAFETY: geteuid has no preconditions.
                    let admin = unsafe { libc::geteuid() } == 0;
                    match iyakki_devctl::mac::run_cmd(&c, &iyakki_devctl::mac::MacEnv::real(), admin) {
                        Ok(t) => {
                            out!("{t}");
                            ExitCode::SUCCESS
                        }
                        Err(e) => fail(e),
                    }
                }
                Err(e) => fail(e),
            },
            _ => fail("usage: iyakki net <up|status|mac>".into()),
        },
        "storage" => storage_cmd(&args[1..]),
        "hwreport" => {
            out_raw!("{}", iyakki_hwinfo::collect(&iyakki_hwinfo::Roots::real()).to_text());
            ExitCode::SUCCESS
        }
        "persist" => persist_cmd(&args[1..]),
        // install to this computer's disk from the booted USB stick (docs/ZRAM-AND-INSTALL.md)
        "install" => ExitCode::from(iyakki_usb::install_cli::run(&args[1..]).clamp(0, 255) as u8),
        "mem" => mem_cmd(&args[1..]),
        // the kernel log kept on the data volume across crashes, and the list of sessions (docs/TROUBLESHOOTING.md)
        "log" => {
            let boot = iyakki_core::crashlog::current_session().map(|s| s.boot_id).unwrap_or_else(|| std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default().trim().replace('-', "").chars().take(8).collect());
            let (text, ok) = iyakki_core::crashlog::cli(&args[1..], std::path::Path::new(iyakki_core::boot::DATA_DIR), &boot);
            print!("{text}");
            if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        // keys, packages, certificates, catalogs, revocation lists and licences for the app store (works offline)
        #[cfg(feature = "shell")]
        "pkg" => match iyakki_agent::store::publish::run(&args[1..]) {
            Ok(text) => {
                out!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        #[cfg(feature = "shell")]
        // a private app store server (runs as the one binary of a container image)
        #[cfg(feature = "shell")]
        "store-server" => match iyakki_agent::store::server_http::run_cli(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // developer mode, the computer's side: talks to a paired device over its TLS (docs/DEVELOPER-MODE.md)
        #[cfg(feature = "shell")]
        "dev" => match iyakki_agent::devmode::cli::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // the SDK, the computer's side: start a project, check this computer, read the guides (dist/sdk/README.md; works offline)
        #[cfg(feature = "shell")]
        "sdk" => match iyakki_agent::sdkcli::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        "web-open" => match args.get(1) {
            Some(url) => web_open(url),
            None => fail("usage: iyakki web-open URL".into()),
        },
        #[cfg(feature = "shell")]
        "browser-test" => iyakki_core::browsertest::run(&args[1..]),
        "--version" | "-V" | "version" => {
            out!("iyakki {}", env!("CARGO_PKG_VERSION"));
            #[cfg(feature = "shell")]
            if let Some(i) = iyakki_agent::relinfo::ReleaseInfo::load(std::path::Path::new(iyakki_agent::relinfo::RELEASE_FILE)) {
                out!("{}", iyakki_agent::relinfo::about_line(Some(&i)));
            }
            ExitCode::SUCCESS
        }
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
        #[cfg(feature = "shell")]
        "quick-install" => ExitCode::from(iyakki_agent::quickinstall::cli_main(&args[1..]).clamp(0, 255) as u8),
        "linux-apps" => ExitCode::from(iyakki_agent::linuxapps::cli_main(&args[1..]).clamp(0, 255) as u8),
        "store-manage" => ExitCode::from(iyakki_agent::store::privatemanage::cli_main(&args[1..]).clamp(0, 255) as u8),
        "hostname" => hostname_cmd(&args[1..]),
        "firewall" => firewall_cmd(&args[1..]),
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
            log::info!("IYAKKI {} starting", env!("CARGO_PKG_VERSION"));
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
                // install mode (GRUB entry "IYAKKI - Install to a disk", docs/INSTALLER.md): the installer's pages on the console
                // instead of the desktop; when it ends the machine restarts. Nothing else (shell, web, supervisor) is started.
                if iyakki_core::boot::install_mode(&cmdline) {
                    iyakki_core::boot::run_install_console();
                    iyakki_core::boot::reboot();
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
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
                    // boot a USB-made install image with a second blank disk; install onto it with the real installer, no prompts
                    // (test only: reachable only through the self-test boot option)
                    Some("installhere") => {
                        let spec = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.testinstall").unwrap_or("vdb").to_string();
                        let mut it = spec.split(',');
                        let disk = it.next().unwrap_or("vdb").to_string();
                        let mut a: Vec<String> = vec!["--disk".into(), disk.clone(), "--confirm".into(), disk, "--yes".into()];
                        for kv in it {
                            if let Some((k, v)) = kv.split_once('=') {
                                a.extend([format!("--{k}"), v.to_string()]);
                            }
                        }
                        if let Some(t) = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.testtemplate") {
                            a.extend(["--template".into(), t.to_string()]);
                        }
                        let rc = iyakki_usb::install_cli::run(&a);
                        out!("IYAKKI-SELFTEST installhere exit {rc}");
                        iyakki_core::boot::power_off();
                    }
                    // memory facts only (zram, swap, settings): for boots that test the kernel options and system.conf
                    Some("memory") => {
                        mem_report("memory");
                        iyakki_core::boot::power_off();
                    }
                    Some("install1") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        mem_report("install1");
                        out!("IYAKKI-SELFTEST install1 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        let _ = std::fs::write(dir.join("marker"), "iyakki-install-ok");
                        iyakki_core::boot::power_off();
                    }
                    Some("install2") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST install2 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        mem_report("install2");
                        out!("IYAKKI-SELFTEST install2 marker {:?}", std::fs::read_to_string(dir.join("marker")).unwrap_or_else(|e| format!("missing: {e}")));
                        iyakki_core::boot::power_off();
                    }
                    Some("persist2") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST persist2 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        out!("IYAKKI-SELFTEST persist2 marker {:?}", std::fs::read_to_string(dir.join("marker")).unwrap_or_else(|e| format!("missing: {e}")));
                        storage_cmd(&[]);
                        out!("IYAKKI-SELFTEST persist2 end");
                        iyakki_core::boot::power_off();
                    }
                    _ => {}
                }
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest") == Some("1") {
                    selftest();
                    iyakki_core::boot::power_off();
                }
                // hot patches (iyakki dev patch): once the system has been up for a while this start counts as good, so the first stage's
                // start counter is cleared (after repeated starts that never get here it stops using patches)
                let _ = std::thread::Builder::new().name("iyakki-patch-ok".into()).spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    iyakki_agent::devmode::patch::mark_boot_good(std::path::Path::new(iyakki_core::boot::DATA_DIR));
                });
                #[cfg(feature = "shell")]
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.shell") != Some("0") {
                    start_shell_thread();
                    #[cfg(feature = "web")]
                    start_devmode_watch();
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
            // developer mode's listener follows Settings -> Developer mode (off by default; nothing listens until an admin turns it on)
            #[cfg(feature = "web")]
            if _web.is_some() {
                start_devmode_watch();
            }
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

/// `iyakki storage`: where data lives now, and the disks that could hold it. `iyakki storage status` and `iyakki storage reattach`
/// are about the drive being lost and coming back (docs/STORAGE-LOSS.md).
fn storage_cmd(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("status") => {
            out_raw!("{}", iyakki_core::storageloss::cli_status());
            return ExitCode::SUCCESS;
        }
        Some("reattach") => {
            return match iyakki_core::storageloss::cli_reattach(std::time::Duration::from_secs(60)) {
                Ok(t) => {
                    out!("{t}");
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            };
        }
        Some(other) => return fail(format!("usage: iyakki storage [status|reattach] (not {other})")),
        None => {}
    }
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

/// Memory facts for the boot self-tests: swap areas, zram settings, the saved system settings.
fn mem_report(stage: &str) {
    let rd = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| format!("missing: {e}"));
    // a swap file is made in the background after login: give it time (up to 40 s) when one is configured
    let wants_file = |c: &str| c.lines().any(|l| l.trim().strip_prefix("swapfile=").is_some_and(|v| v != "off" && v != "0"));
    if wants_file(&rd(iyakki_zram::CONF_PATH)) || iyakki_core::boot::cmdline_param(&rd("/proc/cmdline"), "iyakki.swapfile").is_some_and(|v| v != "off" && v != "0") {
        for _ in 0..80 {
            if rd("/proc/swaps").contains(iyakki_zram::SWAPFILE_PATH) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
    for l in rd("/proc/swaps").lines() {
        out!("IYAKKI-SELFTEST {stage} swaps {l}");
    }
    out!("IYAKKI-SELFTEST {stage} zram algo {}", rd("/sys/block/zram0/comp_algorithm").trim());
    out!("IYAKKI-SELFTEST {stage} zram disksize {}", rd("/sys/block/zram0/disksize").trim());
    out!("IYAKKI-SELFTEST {stage} swappiness {}", rd("/proc/sys/vm/swappiness").trim());
    out!("IYAKKI-SELFTEST {stage} page-cluster {}", rd("/proc/sys/vm/page-cluster").trim());
    for l in rd(iyakki_zram::CONF_PATH).lines() {
        out!("IYAKKI-SELFTEST {stage} system.conf {l}");
    }
    out!("IYAKKI-SELFTEST {stage} hostname {}", rd("/etc/hostname").trim());
}

/// `iyakki hostname [NAME]`: show the host name, or set it (needs root): applied now, written to /etc/hostname and kept in
/// `system.conf` for the next boots. The boot option `iyakki.hostname=` locks it.
fn firewall_cmd(args: &[String]) -> ExitCode {
    let conf = std::path::Path::new(iyakki_fw::service::CONF_PATH);
    match args.first().map(String::as_str) {
        Some("off") => match iyakki_fw::service::emergency_off(conf, std::path::Path::new(iyakki_fw::service::OFF_NOTE)) {
            Ok(m) => {
                out!("{m}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                err!("{e}");
                ExitCode::from(1)
            }
        },
        Some("status") => {
            out_raw!("{}", iyakki_fw::service::status_text(conf));
            ExitCode::SUCCESS
        }
        _ => {
            err!("usage: iyakki firewall off|status");
            ExitCode::from(2)
        }
    }
}

fn hostname_cmd(a: &[String]) -> ExitCode {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let conf = std::path::Path::new(iyakki_zram::CONF_PATH);
    match a.first().map(String::as_str) {
        None | Some("show") => {
            #[cfg(feature = "shell")]
            {
                out!("{}", iyakki_shell::hostname::current("iyakki"));
                if let Some(s) = iyakki_shell::hostname::saved(conf) {
                    out!("kept for the next boots: {s}");
                }
                if let Some(n) = iyakki_shell::hostname::locked_by(&cmdline) {
                    out!("locked by the boot option iyakki.hostname={n}");
                }
            }
            #[cfg(not(feature = "shell"))]
            out!("{}", iyakki_devctl::current_hostname("iyakki"));
            ExitCode::SUCCESS
        }
        Some(name) if a.len() == 1 => {
            #[cfg(feature = "shell")]
            {
                if let Some(n) = iyakki_shell::hostname::locked_by(&cmdline) {
                    return fail(format!("the host name is fixed by the boot option iyakki.hostname={n}; edit the boot entry to change it"));
                }
                let name = match iyakki_shell::hostname::check(name) {
                    Ok(n) => n,
                    Err(e) => return fail(e),
                };
                if let Err(e) = iyakki_shell::hostname::save(conf, &name) {
                    return fail(format!("could not save: {e}"));
                }
                if let Err(e) = iyakki_shell::hostname::apply(&name, std::path::Path::new("/etc/hostname")) {
                    return fail(format!("saved for the next boots, but not applied now: {e}"));
                }
                out!("host name is now {name} (kept for the next boots)");
                ExitCode::SUCCESS
            }
            #[cfg(not(feature = "shell"))]
            {
                let _ = (name, conf, &cmdline);
                fail("this build has no shell; set hostname= in /var/lib/iyakki/system.conf".into())
            }
        }
        _ => fail("usage: iyakki hostname [NAME]".into()),
    }
}

/// `iyakki mem [status|apply|preview]`: the memory settings of this machine (what Settings, System, Memory shows).
fn mem_cmd(a: &[String]) -> ExitCode {
    let env = iyakki_zram::Env::real();
    match a.first().map(String::as_str) {
        None | Some("status") => {
            let s = iyakki_zram::MemStats::read(&env);
            out!("{}", s.summary());
            for w in &s.swaps {
                out!("swap {} used {} of {} bytes, priority {}", w.0, w.1, w.2, w.3);
            }
            if let Some(z) = &s.zram {
                out!("{} {} of {} bytes stored as {} (ratio {}), algorithm {}", z.device, z.orig, z.disksize, z.compressed, z.ratio().map_or("-".into(), |r| format!("{r:.2}")), z.algorithm);
            }
            let r = iyakki_core::boot::memory_settings();
            out!("configured: zram={} swappiness={} swapfile={:?} zswap={}{}", r.config.zram_value(), r.config.swappiness, r.config.swapfile, r.config.zswap, if r.zram_locked { " (zram locked by the boot option)" } else { "" });
            ExitCode::SUCCESS
        }
        Some("preview") => {
            let r = iyakki_core::boot::memory_settings();
            for l in iyakki_zram::preview(&iyakki_zram::MemStats::read(&env), &r.config, iyakki_core::boot::total_ram()) {
                out!("{l}");
            }
            ExitCode::SUCCESS
        }
        Some("apply") => {
            let r = iyakki_core::boot::memory_settings();
            match iyakki_zram::reconfigure(&env, &iyakki_zram::KernelSwap, &r.config, iyakki_core::boot::total_ram()) {
                Ok(steps) => {
                    steps.iter().for_each(|s| out!("{}{}", if s.ok { "" } else { "warning: " }, s.msg));
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        _ => fail("usage: iyakki mem [status|preview|apply]".into()),
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
    for tool in ["arecord", "aplay", "ssh", "scp", "dropbearkey"] {
        let found = std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(tool).exists()) || std::path::Path::new("/usr/bin").join(tool).exists();
        out!("IYAKKI-SELFTEST tool {tool} {}", if found { "ok" } else { "missing" });
    }
    // The SSH client of the CLI tile: `ssh -V` prints its version (Dropbear's client on the images).
    match std::process::Command::new("ssh").arg("-V").output() {
        Ok(o) => out!("IYAKKI-SELFTEST ssh {}", String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).lines().find(|l| !l.trim().is_empty()).unwrap_or("(no version)").trim()),
        Err(e) => out!("IYAKKI-SELFTEST ssh missing ({e})"),
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
    let env = iyakki_shell::shell::ShellEnv::from_paths(&paths, host).map_err(|e| e.to_string())?;
    // firewall changes (and what happened at boot) go into the audit log
    {
        let audit = env.audit.clone();
        // a session that ended without a clean shutdown is written to the audit log once (crashlog.rs)
        {
            let a = env.audit.clone();
            iyakki_core::crashlog::audit_pending(&std::path::Path::new(iyakki_core::boot::DATA_DIR).join(iyakki_core::crashlog::LOG_SUBDIR), &move |event, detail| a.record("system", event, detail));
        }
        iyakki_fw::service::set_audit(std::sync::Arc::new(move |event, detail| audit.record("system", event, detail)));
    }
    // As the machine's first process, keep its clock right from the network (a machine with a flat clock battery starts
    // at 1970). The settings are read every round, so Settings -> Time takes effect without a restart.
    if std::process::id() == 1 {
        // one-cable links to another computer (Thunderbolt/USB4, a USB bridge cable, a gadget port): link-local addresses without
        // DHCP, the peer beacon, and the firewall's "direct link" zone (`iyakki dev usb`, docs/DEVELOPER-MODE.md)
        iyakki_agent::directlink::manager::start_system(std::path::Path::new(iyakki_core::boot::DATA_DIR), std::sync::Arc::new(|ifaces, more| iyakki_fw::service::set_direct_links(ifaces, more)));
    }
    if std::process::id() == 1 {
        let svc = env.services.clone();
        iyakki_agent::ntp::spawn_sync_thread(move || svc.settings.lock().map(|s| (!s.get().time.no_ntp, s.get().time.ntp_server.clone())).unwrap_or((true, String::new())));
    }
    Ok(env)
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
        iyakki_core::boot::progress(98, iyakki_core::boot::DESKTOP_STAGE);
        log::info!("boot: shell thread started");
        let mut found = None;
        for _ in 0..300 {
            found = iyakki_shell::splash::find_framebuffer(std::path::Path::new("/dev"));
            if found.is_some() {
                break;
            }
            iyakki_shell::splash::make_missing_nodes(std::path::Path::new("/sys/class/graphics"), std::path::Path::new("/dev"));
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let Some(fb_path) = found else {
            log::warn!("shell: no framebuffer appeared; running headless");
            return;
        };
        log::info!("boot: display found");
        let fb = fb_path.to_string_lossy().to_string();
        let fb = fb.as_str();
        // the same shell is run again after a panic, so the signed-in person and the open app survive; one that crashes again at once
        // is rebuilt (see `iyakki_core::keepalive`)
        let outcome = iyakki_core::keepalive::run_kept(
            || match shell_env() {
                Ok(env) => {
                    let t_new = std::time::Instant::now();
                    let sh = iyakki_shell::shell::Shell::new(env);
                    log::info!("boot: shell constructed in {} ms", t_new.elapsed().as_millis());
                    Some(sh)
                }
                Err(e) => {
                    log::error!("shell: {e}");
                    None
                }
            },
            |sh| {
                let stop = std::sync::atomic::AtomicBool::new(false);
                // the boot screen lets go of the display before the desktop draws on it
                iyakki_core::boot::finish_boot_screen();
                // a monitor that woke up during boot but was not seen: one more look (in the background)
                iyakki_core::display::probe_once_in_background("the desktop started", iyakki_core::boot::screen_found);
                if let Err(e) = iyakki_shell::backend::run_fb(sh, fb, &stop) {
                    log::error!("shell: {e}");
                }
            },
            5,
            std::time::Duration::from_secs(1),
            |attempt, kept| log::error!("shell: crashed (attempt {attempt} of 5), restarting {}", if kept { "on the same session" } else { "with a fresh shell" }),
        );
        if outcome == iyakki_core::keepalive::Outcome::Finished {
            return;
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
            let meta = release::Meta { date: flag("--date").unwrap_or_default(), git_commit: flag("--git-commit").unwrap_or_default(), git_describe: flag("--git-describe").unwrap_or_default(), kernel: flag("--kernel").unwrap_or_default() };
            match release::sign_dir_with(std::path::Path::new(dir), std::path::Path::new(&key), &flag("--title").unwrap_or_else(|| "IYAKKI".into()), &flag("--version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()), &meta) {
                Ok(entries) => {
                    for e in &entries {
                        out!("{}  {}  ({} bytes)", e.sha256, e.name, e.bytes);
                    }
                    out!("wrote {}, {}, {}, {} and {} in {dir}; public key {}", release::SUMS, release::SIG, release::MANIFEST, release::MANIFEST_SIG, release::PAGE, release::public_key_of(std::path::Path::new(&key)).unwrap_or_default());
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
                    // the signed manifest, when the folder has one, must agree too
                    if std::path::Path::new(dir).join(release::MANIFEST).exists() {
                        match release::verify_manifest(std::path::Path::new(dir), &pubkey) {
                            Ok((version, files)) => {
                                out!("manifest {}: version {version}", release::MANIFEST);
                                for v in files.iter().filter(|v| !v.ok) {
                                    bad += 1;
                                    out!("FAILED  {} (manifest): {}", v.name, v.why);
                                }
                            }
                            Err(e) => {
                                bad += 1;
                                out!("FAILED  {}: {e}", release::MANIFEST);
                            }
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

/// Start (once per process) the thread that opens the developer-mode listener while developer mode is on and closes it when it goes
/// off (`iyakki_remote::devserver::watch`). The certificate is the web front end's.
#[cfg(feature = "web")]
fn start_devmode_watch() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    match shell_env() {
        Ok(env) => {
            let tls = iyakki_agent::devmode::tls_dir(env.services.dir());
            if let Err(e) = iyakki_remote::devserver::watch(env, tls) {
                log::error!("developer mode: could not start its watcher: {e}");
            }
        }
        Err(e) => log::error!("developer mode: {e}"),
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
    start_devmode_watch();
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
    #[cfg(feature = "web")]
    start_devmode_watch();
    let mut sh = iyakki_shell::shell::Shell::new(env);
    match iyakki_shell::backend::run_fb(&mut sh, &fb, &shutdown) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
