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
  iyakki vpn inspect FILE               offline: what is in a local .ovpn or WireGuard .conf (kind, whether it needs a user name and password,
                                      every setting name with ACCEPTED / IGNORED / REFUSED, block counts); never prints a value or a key
  iyakki backup <areas|preview|create|inspect|restore>   back up one person's own data into one file (optionally encrypted) and restore it; see docs/BACKUP.md
  iyakki checkup-result FILE            check the signature of a CHECK-UP result file and print the hardware-matrix rows it holds
  iyakki doctor [--detail]              what is wrong with this computer, in plain words (the DOCTOR tile's checks as text; exit 1 when
                                      something needs attention; the running system's own state is not checked from here)
  iyakki top [N]                        what the machine is doing: load, memory and the N busiest processes (default 15)
  iyakki dlmanifest keygen FILE | sign MANIFEST --key FILE | verify MANIFEST [--pubkey HEX|FILE] | check | gen
                                      the signed downloads manifest (versions and checksums of the download items; docs/development/DOWNLOADS-MANIFEST.md)
  iyakki quick-install k3s|duckdb|elasticsearch [--data DIR]
                                      one Quick install step (Store -> Setup runs it as a detached job)
  iyakki linux-apps --data DIR --user NAME create|install|remove|upgrade|update|run|import ...
                                      the Linux apps (Debian/Ubuntu sandbox) steps the IYAKKI menu runs as detached jobs (docs/LINUX-APPS.md)
  iyakki store-manage reset|remove --kubeconfig FILE --namespace NS [--delete-data] [--wait SECS] [--force]
                                      the cluster side of resetting or removing the private store (run as a detached job)
  iyakki serve --profile NAME [--config FILE] [--listen ADDR] [--check]
                                      only the headless web service of a profile (agent-api, cloud): HTTPS, sign-in required,
                                      loopback unless the config opts in (docs/development/MINIMAL-WEBSERVICE.md)
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
  iyakki firmware <list|fetch>          the vendor firmware this computer lacks: list it, or fetch it on another computer and push it over a cable
                                      (docs/FIRMWARE-LICENCES.md, dist/scripts/firmware-notes/)
  iyakki usb share-internet [--apply|--undo]
                                      on a computer joined by a USB-C / Thunderbolt cable: share its internet with the device (docs/DEVELOPER-MODE.md)
  iyakki share <start|stop|status|list> [--uplink IFACE] [--via IFACE] [--hours 1-72] [--allow-lan] [--no-isolation]
                                      give other computers this computer's internet over an Ethernet, USB or Thunderbolt cable: private
                                      network, address and name service on that cable only, off by itself when idle (docs/development/HOTSPOT-PLAN.md)
  iyakki dev <pair|devices|install|uninstall|list|logs|run|push|screenshot|whoami>
                                      from a developer's computer: pair once with a device in developer mode, then install, upgrade,
                                      remove, open and watch apps on it, push a folder on every change (docs/DEVELOPER-MODE.md)
  iyakki cloud <example|manifests|image|bundle|common|deploy>
                                      a cloud tenant: configuration, Kubernetes manifests, a container image made from this program, and the
                                      deploy to a cluster (docs/development/CLOUD-MULTITENANT-PLAN.md)
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
    // the throw-away process of the guest self-test `iyakki.selftest=sandboxnet` (dist/qemu/README.md): it makes a network namespace
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some(iyakki_agent::linuxapps_net_selftest::SUBCOMMAND) {
        let args: Vec<String> = std::env::args().skip(2).collect();
        return ExitCode::from(iyakki_agent::linuxapps_net_selftest::probe_main(&args).clamp(0, 255) as u8);
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
            Some("up") => match iyakki_core::net::bring_up(std::path::Path::new("/etc/iyakki/net.conf"), std::path::Path::new(iyakki_core::net::SYS_CLASS_NET)) {
                Ok(errs) => {
                    errs.iter().for_each(|e| err!("iyakki net: {e}"));
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e.to_string()),
            },
            Some("status") => {
                for i in iyakki_core::net::list_ifaces(std::path::Path::new(iyakki_core::net::SYS_CLASS_NET)) {
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
        // system components of a private store: publish, list, plan (docs/development/COMPONENT-UPDATES-PLAN.md)
        #[cfg(feature = "shell")]
        "store" => match iyakki_agent::store::compcli::run(&args[1..]) {
            Ok(text) => {
                out!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        // developer mode, the computer's side: talks to a paired device over its TLS (docs/DEVELOPER-MODE.md)
        #[cfg(feature = "shell")]
        "dev" => match iyakki_agent::devmode::cli::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // a cloud tenant: its image and manifests, put on a cluster (docs/development/CLOUD-MULTITENANT-PLAN.md)
        #[cfg(feature = "shell")]
        "cloud" => match iyakki_agent::cloudcli::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // the cloud vault service and its keys (docs/development/CLOUD-MULTITENANT-PLAN.md, 3.2)
        #[cfg(feature = "shell")]
        "vault" => match iyakki_agent::cloudvaultd::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // the SDK, the computer's side: start a project, check this computer, read the guides (dist/sdk/README.md; works offline)
        #[cfg(feature = "shell")]
        "sdk" => match iyakki_agent::sdkcli::run(&args[1..], &mut |s| out!("{s}")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        },
        // a container's HEALTHCHECK without curl: GET /healthz on the local listener (docs/CLOUD-IMAGES.md)
        #[cfg(feature = "headless")]
        "healthcheck" => ExitCode::from(iyakki_agent::healthcheck::cli_main(&args[1..], std::env::var("IYAKKI_HEALTH_URL").ok(), &mut |s| err!("{s}")).clamp(0, 255) as u8),
        #[cfg(feature = "shell")]
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
        #[cfg(feature = "headless")]
        "user" => user_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "release" => release_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "backup" => ExitCode::from(iyakki_agent::backup_cli::cli_main(&args[1..]).clamp(0, 255) as u8),
        "checkup-result" => match args.get(1).map(std::fs::read_to_string) {
            Some(Ok(t)) => match iyakki_agent::checkup_result::verify(&t) {
                Ok(b) => {
                    for l in iyakki_agent::checkup_result::lines(&b) {
                        out!("{l}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            },
            Some(Err(e)) => fail(e.to_string()),
            None => fail("usage: iyakki checkup-result FILE (checks a CHECK-UP result file and prints its matrix rows)".into()),
        },
        #[cfg(feature = "shell")]
        "doctor" => doctor_cmd(&args[1..]),
        #[cfg(feature = "shell")]
        "top" => {
            let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(15).clamp(1, 200);
            out_raw!("{}", iyakki_shell::apps::system::text_report(&iyakki_agent::corehandle::LocalHost, &iyakki_agent::sysinfo::SysRoots::real(), n));
            ExitCode::SUCCESS
        }
        #[cfg(feature = "shell")]
        "quick-install" => ExitCode::from(iyakki_agent::quickinstall::cli_main(&args[1..]).clamp(0, 255) as u8),
        "dlmanifest" => ExitCode::from(iyakki_agent::dlmanifest::cli_main(&args[1..]).clamp(0, 255) as u8),
        "firmware" => ExitCode::from(iyakki_agent::fwextra_cli::firmware_main(&args[1..]).clamp(0, 255) as u8),
        "new-app" => ExitCode::from(iyakki_agent::newapp::cli(&args[1..]).clamp(0, 255) as u8),
        "startup-disk" => ExitCode::from(iyakki_agent::startupdisk::cli(&args[1..]).clamp(0, 255) as u8),
        "share" => ExitCode::from(iyakki_agent::sharing::cli::main(&args[1..]).clamp(0, 255) as u8),
        "usb" if args.get(1).map(String::as_str) == Some("share-internet") => ExitCode::from(iyakki_agent::fwextra_cli::share_main(&args[2..]).clamp(0, 255) as u8),
        "linux-apps" => ExitCode::from(iyakki_agent::linuxapps::cli_main(&args[1..]).clamp(0, 255) as u8),
        "store-manage" => ExitCode::from(iyakki_agent::store::privatemanage::cli_main(&args[1..]).clamp(0, 255) as u8),
        "live-restart" => live_restart_cmd(&args[1..]),
        "hostname" => hostname_cmd(&args[1..]),
        "firewall" => firewall_cmd(&args[1..]),
        "vpn" => ExitCode::from(iyakki_vpn::inspect::cli_main(&args[1..]).clamp(0, 255) as u8),
        "vpn-helper" => match install_signal_handlers() {
            Ok(stop) => ExitCode::from(iyakki_agent::vpn_helper::cli_main_until(&args[1..], &stop).clamp(0, 255) as u8),
            Err(e) => fail(format!("signal setup: {e}")),
        },
        "fan-service" => fan_service_cmd(&args[1..]),
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
            // `serve --profile NAME` (or IYAKKI_PROFILE): only the headless web service of that profile, no shell, no screen
            #[cfg(feature = "headless")]
            match iyakki_core::svc::parse_args(serve_args(&args), &|k| std::env::var(k).ok()) {
                Ok(Some(ask)) => return serve_profile(&ask),
                Ok(None) => {}
                Err(e) => return fail(e),
            }
            // a build without the shell has nothing else to serve
            if !cfg!(feature = "shell") {
                return fail("this build is the headless web service only: use `iyakki serve --profile NAME` (or set IYAKKI_PROFILE)".into());
            }
            // Real boot = we are PID 1 *and* nothing has mounted /proc yet. A container's PID 1 (docker, k3s)
            // already has /proc, and must never try to mount filesystems or load kernel modules.
            let pid1 = std::process::id() == 1 && !std::path::Path::new("/proc/self").exists();
            // The new program of a live restart is still process 1 but /proc is mounted: `liverestart::begin` finds the state file the old
            // program left for this very process (off unless /etc/iyakki/live-restart.enabled exists). It skips the boot steps.
            #[cfg(feature = "shell")]
            let live_start = if !pid1 && std::process::id() == 1 { iyakki_core::liverestart::begin(&iyakki_core::liverestart::Paths::real()) } else { None };
            #[cfg(not(feature = "shell"))]
            let live_start: Option<iyakki_core::liverestart::Started> = None;
            let restarted = live_start.is_some();
            // the new program of a live restart owes a health report; the shell thread sends it when it draws its first frame
            if live_start.as_ref().is_some_and(|s| !s.rolled_back) {
                iyakki_core::liverestart::note_restarted();
            }
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
                let cmdline = iyakki_storage::cmdline_text();
                if web_value.is_none() {
                    web_value = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.web").map(String::from);
                }
                // Self-tests need the network up when boot returns; a normal start shows the sign-in screen as soon as
                // storage, input and display are ready and brings up the network and sound in the background.
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest").is_some() {
                    iyakki_core::boot::early_boot();
                } else if iyakki_core::boot::install_mode(&cmdline) {
                    // the installer needs storage, input and display only (the critical part); no firewall, driver packs, network, sound,
                    // Bluetooth or camera: it copies from the boot stick to a disk and restarts (docs/INSTALLER.md)
                    let _ = iyakki_core::boot::early_boot_critical();
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
                // cloud node: install to the internal disk only when the kernel line carries BOTH iyakki.cloud.install=DISK and
                // iyakki.cloud.confirm=ERASE-DISK (docs/CLOUD-IMAGES.md); the existing installer does the work, once per stick
                #[cfg(feature = "shell")]
                match iyakki_agent::cloudseed::install_request(&cmdline) {
                    Some(Ok(a)) => {
                        let done = std::path::Path::new(iyakki_core::boot::DATA_DIR).join("cloud-install-done");
                        if done.exists() {
                            out!("IYAKKI-CLOUD self-install skipped: this stick already installed once (remove {} to allow it again)", done.display());
                        } else {
                            let _ = std::fs::write(&done, "installed\n");
                            out!("IYAKKI-CLOUD self-install of {} starts", a[1]);
                            let rc = iyakki_usb::install_cli::run(&a);
                            out!("IYAKKI-CLOUD self-install exit {rc}: remove the stick and restart");
                            if rc == 0 {
                                iyakki_core::boot::power_off();
                            }
                        }
                    }
                    Some(Err(e)) => out!("IYAKKI-CLOUD {e}"),
                    None => {}
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
                    // the outbound-only sandbox network on a real kernel (dist/qemu/sandbox-net-test.sh); prints check lines and a RESULT line
                    #[cfg(all(feature = "shell", target_os = "linux"))]
                    Some("sandboxnet") => {
                        let ports = iyakki_core::boot::cmdline_param(&cmdline, "iyakki.testnet").unwrap_or("0,0");
                        let (a, d) = ports.split_once(',').unwrap_or(("0", "0"));
                        let checks: Vec<_> = iyakki_agent::linuxapps_net_selftest::run(a.parse().unwrap_or(0), d.parse().unwrap_or(0)).into_iter().map(|(n, ok, d)| iyakki_core::selfcheck::check(&n, ok, d)).collect();
                        for l in iyakki_core::selfcheck::lines("sandboxnet", &checks) {
                            out!("{l}");
                        }
                        iyakki_core::boot::power_off();
                    }
                    // boot-order recording and prefetch on a real kernel (dist/qemu/boot-prefetch-test.sh); needs iyakki.bootrecord=1
                    Some("bootlist") => {
                        for l in iyakki_core::selfcheck::lines("bootlist", &bootlist_checks()) {
                            out!("{l}");
                        }
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
                    // upgrade test (dist/buildroot/qemu-upgrade-test.sh): the data volume holds an older release's files (the alpha.1
                    // fixtures); open the stores the way the system does and say what they hold. Test only: reachable only through the
                    // self-test boot option; it writes what a normal start would write (the migrations and their `.pre-v*` copies)
                    #[cfg(feature = "shell")]
                    Some("upgrade") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        match iyakki_agent::settings::SettingsStore::open(&dir.join("settings.json")) {
                            Ok(st) => out!("IYAKKI-SELFTEST upgrade settings providers={} chat={} tool_log_days={} read_only={}", st.get().providers.len(), st.get().chat.as_ref().map(|c| c.model.as_str()).unwrap_or("none"), st.get().tool_log_days, st.read_only().is_some()),
                            Err(e) => out!("IYAKKI-SELFTEST upgrade settings error {e}"),
                        }
                        out!("IYAKKI-SELFTEST upgrade sources {}", iyakki_dash::store::Sources::open(dir, "amy").own().len());
                        let inbox = iyakki_agent::inbox::Inbox::new(&dir.join("inbox"));
                        let _ = inbox.maintain("amy", 1_700_000_100);
                        out!("IYAKKI-SELFTEST upgrade inbox {}", inbox.list("amy").iter().map(|n| n.count).sum::<u32>());
                        out!("IYAKKI-SELFTEST upgrade end");
                        iyakki_core::boot::power_off();
                    }
                    // per-computer settings keyed by the real DMI strings (dist/qemu/computer-profile-test.sh): the settings store of a scratch folder on the data volume is opened for this computer;
                    // `iyakki.hwset=NN` on the command line sets the sound volume to NN percent (a per-computer value) before the report
                    #[cfg(target_os = "linux")]
                    Some("computer") => {
                        use iyakki_agent::hwprofile::{identify, Computers, RealFacts};
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR).join("m80-computers");
                        let _ = std::fs::create_dir_all(&dir);
                        let id = identify(&RealFacts);
                        out!("IYAKKI-SELFTEST computer id={} label={:?}", id.id, id.label);
                        match iyakki_agent::settings::SettingsStore::open(&dir.join("settings.json")) {
                            Ok(mut st) => {
                                let c = Computers::open(&dir, &RealFacts, iyakki_agent::clock::now_unix(), st.get());
                                let fresh = c.new_computer;
                                st.attach_computers(c);
                                out!("IYAKKI-SELFTEST computer new={} volume={} profiles={}", fresh, st.get().sound.volume_pct, st.computers().map_or(0, |c| c.list().len()));
                                let cmd = iyakki_storage::cmdline_text();
                                if let Some(v) = cmd.split_whitespace().find_map(|w| w.strip_prefix("iyakki.hwset=")).and_then(|v| v.parse::<u8>().ok()) {
                                    let _ = st.update(|s| s.sound.volume_pct = v.into());
                                    out!("IYAKKI-SELFTEST computer set volume={}", st.get().sound.volume_pct);
                                }
                            }
                            Err(e) => out!("IYAKKI-SELFTEST computer error {e:?}"),
                        }
                        out!("IYAKKI-SELFTEST computer end");
                        iyakki_core::boot::power_off();
                    }
                    // the USB controllers kept awake on the real sysfs (dist/qemu/usb-awake-test.sh): which PCI USB controllers sleep, then the same step the system takes
                    #[cfg(target_os = "linux")]
                    Some("usbawake") => {
                        for l in iyakki_agent::usbawake::selftest_lines(std::path::Path::new("/")) {
                            out!("IYAKKI-SELFTEST usbawake {l}");
                        }
                        out!("IYAKKI-SELFTEST usbawake end");
                        iyakki_core::boot::power_off();
                    }
                    // the sound output list of the real /proc/asound (dist/qemu/sound-output-test.sh): waits for the cards the kernel finds, then prints what the selector makes of them
                    #[cfg(target_os = "linux")]
                    Some("sound") => {
                        let cards = || iyakki_agent::audioout::cards_text(std::path::Path::new("/proc"));
                        let t0 = std::time::Instant::now();
                        while t0.elapsed().as_secs() < 20 && !cards().contains("]:") {
                            std::thread::sleep(std::time::Duration::from_millis(250));
                        }
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        for l in cards().lines() {
                            out!("IYAKKI-SELFTEST sound card {}", l.trim());
                        }
                        // the card's own controls, as `amixer -c N contents` says them (the image ships amixer)
                        let mixer = |card: u32| std::process::Command::new("amixer").args(["-c", &card.to_string(), "contents"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string());
                        let outs = iyakki_agent::audioout::scan(std::path::Path::new("/proc"), &mixer);
                        for o in &outs {
                            out!("IYAKKI-SELFTEST sound output key={} kind={} alsa={} label={:?} state={:?}", o.key(), o.kind.label(), o.alsa(), o.label(), o.state());
                        }
                        let c = iyakki_agent::audioout::choose(&outs, "");
                        out!("IYAKKI-SELFTEST sound chosen {:?} because {}", c.output.as_ref().map(|o| o.key()), c.reason);
                        out!("IYAKKI-SELFTEST sound end");
                        iyakki_core::boot::power_off();
                    }
                    // the startup disk commands on the firmware's real variable store (dist/qemu/startup-disk-test.sh, a guest started through OVMF so that efivarfs exists)
                    #[cfg(target_os = "linux")]
                    Some("startupdisk") => {
                        for step in ["status", "register", "first", "status", "revert", "status"] {
                            out!("IYAKKI-SELFTEST startupdisk step {step}");
                            let rc = iyakki_agent::startupdisk::cli(&[step.to_string()]);
                            out!("IYAKKI-SELFTEST startupdisk rc {step} {rc}");
                        }
                        out!("IYAKKI-SELFTEST startupdisk end");
                        iyakki_core::boot::power_off();
                    }
                    // /home on the data volume (dist/qemu/homebind-test.sh): reports what is mounted at /home, writes a note there the first time and reads it back the next time
                    Some("homebind") => {
                        let mounts = iyakki_storage::mounts_text();
                        let home = mounts.lines().find(|l| l.split_whitespace().nth(1) == Some("/home")).unwrap_or("none");
                        out!("IYAKKI-SELFTEST homebind mount {home}");
                        out!("IYAKKI-SELFTEST homebind data {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), std::path::Path::new(iyakki_core::boot::DATA_DIR)));
                        let note = std::path::Path::new("/home/m80-note.txt");
                        match std::fs::read_to_string(note) {
                            Ok(t) => out!("IYAKKI-SELFTEST homebind note {}", t.trim()),
                            Err(_) => {
                                let _ = std::fs::write(note, "home-survives-the-reboot\n");
                                out!("IYAKKI-SELFTEST homebind note written");
                            }
                        }
                        out!("IYAKKI-SELFTEST homebind end");
                        iyakki_core::boot::power_off();
                    }
                    // a USB data disk pulled and plugged in again (dist/qemu/drive-loss-test.sh): the host script removes and re-adds the disk when it sees READY
                    #[cfg(target_os = "linux")]
                    Some("driveloss") => {
                        use iyakki_core::selfcheck::check;
                        use iyakki_core::storageloss::status_value;
                        let state = || std::fs::read_to_string(iyakki_storage::loss::STATUS_FILE).unwrap_or_default();
                        let val = |k: &str| status_value(&state(), k).unwrap_or("").to_string();
                        let wait_for = |want: &dyn Fn(&str) -> bool, secs: u64| -> bool {
                            let t0 = std::time::Instant::now();
                            while t0.elapsed().as_secs() < secs {
                                if want(&val("state")) {
                                    return true;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(250));
                            }
                            false
                        };
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        let mut checks = Vec::new();
                        let healthy = wait_for(&|s| s == "Healthy", 30);
                        let dev = val("dev");
                        checks.push(check("watching-the-data-disk", healthy, format!("state {} dev {dev} uuid {}", val("state"), val("uuid"))));
                        // written AND synced before the loss: unsynced data of the last seconds may be lost (docs/STORAGE-LOSS.md), what was flushed must still be there afterwards
                        let _ = std::fs::write(dir.join("m80-before"), "written-before-the-loss");
                        let _ = std::fs::File::open(dir.join("m80-before")).and_then(|f| f.sync_all());
                        let _ = std::fs::File::open(dir).and_then(|f| f.sync_all());
                        out!("IYAKKI-SELFTEST driveloss READY");
                        let lost = wait_for(&|s| s != "Healthy", 90);
                        checks.push(check("the-loss-is-seen", lost, format!("state {} at uptime {} s", val("state"), val("since"))));
                        let back = wait_for(&|s| s == "Reattachable" || s == "Healthy", 90);
                        checks.push(check("the-same-disk-is-recognised-on-its-new-node", back, format!("state {}", val("state"))));
                        // the automatic reattach may already have run; otherwise ask like the Reconnect button does
                        let answer = if val("state") == "Healthy" { Ok("done by itself".to_string()) } else { iyakki_core::storageloss::cli_reattach(std::time::Duration::from_secs(60)) };
                        checks.push(check("reattach", answer.is_ok(), format!("{answer:?}")));
                        let healthy2 = wait_for(&|s| s == "Healthy", 30);
                        let (dev2, uuid2) = (val("dev"), val("uuid"));
                        checks.push(check("healthy-again-on-another-node", healthy2 && dev2 != dev, format!("dev {dev} -> {dev2}, uuid {uuid2}, reattached={}", val("reattached"))));
                        checks.push(check("what-was-written-before-is-there", std::fs::read_to_string(dir.join("m80-before")).is_ok_and(|t| t == "written-before-the-loss"), format!("{:?}", std::fs::read_to_string(dir.join("m80-before")).ok())));
                        let wrote = std::fs::write(dir.join("m80-after"), "written-after-the-return").and_then(|_| std::fs::File::open(dir.join("m80-after")).and_then(|f| f.sync_all())).is_ok();
                        checks.push(check("writing-works-again", wrote, "m80-after written and synced"));
                        let mounts = iyakki_storage::mounts_text();
                        let line = mounts.lines().find(|l| l.split_whitespace().nth(1) == Some(iyakki_core::boot::DATA_DIR)).unwrap_or("none");
                        checks.push(check("the-data-folder-is-mounted-from-the-new-node", line.contains(&dev2) && !line.contains(&format!("{dev} ")), line));
                        for l in iyakki_core::selfcheck::lines("driveloss", &checks) {
                            out!("{l}");
                        }
                        iyakki_core::boot::power_off();
                    }
                    Some("persist2") => {
                        let dir = std::path::Path::new(iyakki_core::boot::DATA_DIR);
                        out!("IYAKKI-SELFTEST persist2 location {:?}", iyakki_storage::data_location(&iyakki_storage::Roots::real(), dir));
                        out!("IYAKKI-SELFTEST persist2 marker {:?}", std::fs::read_to_string(dir.join("marker")).unwrap_or_else(|e| format!("missing: {e}")));
                        let _ = storage_cmd(&[]);
                        out!("IYAKKI-SELFTEST persist2 end");
                        iyakki_core::boot::power_off();
                    }
                    _ => {}
                }
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.selftest") == Some("1") {
                    selftest();
                    iyakki_core::boot::power_off();
                }
            }
            // What a program does for ITS OWN life, not for the boot (F-SPLIT-06): the good-start mark, the shell thread, the test-only web
            // page. A normal PID 1 does it after the boot steps above; the program of a live restart (`iyakki dev patch ... --live`, same
            // process id, /proc already mounted) does ONLY this, so the desktop comes back without mounting, driver loading or the
            // network being done a second time. The boot steps above stay inside `if pid1` and must never move down here
            // (tests/live_restart_start.rs reads this file to make sure).
            if pid1 || restarted {
                let cmdline = iyakki_storage::cmdline_text();
                // the new program of a live restart: if it does not report healthy in time the watchdog starts the old program again
                if let Some(st) = live_start.as_ref().filter(|s| !s.rolled_back) {
                    let _ = st;
                    let _ = std::thread::Builder::new().name("iyakki-live-watchdog".into()).spawn(|| {
                        let p = iyakki_core::liverestart::Paths::real();
                        let ok = iyakki_core::liverestart::watchdog(&p, std::time::Duration::from_secs(iyakki_core::liverestart::HEALTH_SECONDS), std::time::Duration::from_millis(500), &iyakki_core::liverestart::RealExec, &[], &std::thread::sleep);
                        log::info!("live restart: the new program {}", if ok { "reported healthy" } else { "did not report healthy" });
                    });
                }
                // live restart (F-SPLIT-06): when the owner switched it on (flag file), look every two seconds for a request that
                // `iyakki live-restart` left and carry it out; the watchdog and the health report are the new program's
                if iyakki_core::liverestart::enabled(&iyakki_core::liverestart::Paths::real()) {
                    // before the exec: the shell loop leaves (the screen is let go), the disks are synced
                    iyakki_core::liverestart::before_exec(|| {
                        // the vault keys of the signed-in people go into a memory file the new program reads (never a disk)
                        if let Some(svc) = iyakki_agent::livekeys::current() {
                            let bytes = iyakki_agent::livekeys::pack(&svc);
                            if bytes.len() > 2 {
                                match iyakki_core::liverestart::hand_over("session-keys", &bytes).and_then(|fd| iyakki_core::liverestart::keep_across_exec(fd).map(|_| fd)) {
                                    Ok(fd) => std::env::set_var("IYAKKI_LIVE_KEYS_FD", fd.to_string()),
                                    Err(e) => log::warn!("live restart: the vault keys could not be handed over ({e}); the vaults will be locked"),
                                }
                            }
                        }
                        SHELL_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(600));
                        // SAFETY: sync(2) takes no arguments and cannot fail in a way that matters here
                        unsafe { libc::sync() };
                    });
                    let _ = std::thread::Builder::new().name("iyakki-live-restart".into()).spawn(|| loop {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let p = iyakki_core::liverestart::Paths::real();
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                        let me = std::env::current_exe().unwrap_or_else(|_| "/usr/bin/iyakki".into());
                        if let Some(why) = iyakki_core::liverestart::serve_once(&p, &me, env!("CARGO_PKG_VERSION"), now, &[], &iyakki_core::liverestart::RealExec) {
                            log::error!("live restart: {why}");
                        }
                    });
                }
                // hot patches (iyakki dev patch): once the system has been up for a while this start counts as good, so the first stage's
                // start counter is cleared (after repeated starts that never get here it stops using patches)
                let _ = std::thread::Builder::new().name("iyakki-patch-ok".into()).spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    iyakki_agent::devmode::patch::mark_boot_good(std::path::Path::new(iyakki_core::boot::DATA_DIR));
                    // the record of installed system components follows what really runs (kernel release, release file, program version)
                    match iyakki_agent::store::compinstalled::correct_at_boot(std::path::Path::new(iyakki_core::boot::DATA_DIR), &iyakki_agent::store::compinstalled::Facts::real()) {
                        Ok(notes) => notes.iter().for_each(|n| eprintln!("components: {n}")),
                        Err(e) => eprintln!("components: the installed record could not be saved: {e}"),
                    }
                });
                #[cfg(feature = "shell")]
                if iyakki_core::boot::cmdline_param(&cmdline, "iyakki.shell") != Some("0") {
                    start_ui(&cmdline);
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
            // a cloud node with no administrator: the pairing page holds the web port until the first account exists (docs/CLOUD-IMAGES.md)
            #[cfg(feature = "web")]
            let (web_value, adopting) = {
                let cmd = iyakki_storage::cmdline_text();
                let users = std::path::PathBuf::from(&iyakki_shell::shell::ShellPaths::default().users);
                let wanted = iyakki_core::cloudnode::adopt_wanted(&cmd, std::env::var("IYAKKI_ADOPT").ok().as_deref(), &users, &|| iyakki_agent::cloudmode::detect(iyakki_agent::cloudmode::Setting::Auto).on);
                let value = web_value.clone().unwrap_or_else(|| "1".to_string());
                let started = wanted
                    && iyakki_remote::adopt_server::start(
                        std::path::PathBuf::from(iyakki_core::boot::DATA_DIR),
                        iyakki_remote::adopt_server::PORT,
                        Box::new(move || {
                            if let Some(s) = start_web(&value) {
                                std::mem::forget(s);
                                start_devmode_watch();
                            }
                        }),
                    )
                    .map_err(|e| log::error!("adopt: {e}"))
                    .is_ok();
                // a cloud node serves its browser front end by default (nothing else can reach it); a desktop keeps the old rule
                let cloud_default = || iyakki_agent::cloudmode::detect(iyakki_agent::cloudmode::Setting::Auto).on.then(|| "1".to_string());
                (if started { None } else { web_value.or_else(cloud_default) }, started)
            };
            #[cfg(feature = "web")]
            let _web = web_value.as_deref().and_then(start_web);
            #[cfg(feature = "web")]
            let _ = adopting;
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

/// `iyakki live-restart PROGRAM`: ask the running program to replace itself with PROGRAM without a restart of the device (F-SPLIT-06). Off
/// unless `/etc/iyakki/live-restart.enabled` exists. Run through developer mode's shell, which asks the person on the device first.
fn live_restart_cmd(a: &[String]) -> ExitCode {
    let Some(exe) = a.first() else { return fail("usage: iyakki live-restart PROGRAM".into()) };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match iyakki_core::liverestart::write_request(&iyakki_core::liverestart::Paths::real(), std::path::Path::new(exe), now) {
        Ok(()) => {
            out!("live restart requested: the running program takes it within a few seconds, and goes back to the old program by itself if the new one does not report healthy in {} s", iyakki_core::liverestart::HEALTH_SECONDS);
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
            match node.submit_scheduled(iyakki_swarm::TaskRequest { wasm: wasm.clone(), entry: entry.clone() }).await {
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
/// The checks of `iyakki.selftest=bootlist`: the recorder wrote a list of files opened while booting, the prefetch reads it to the end,
/// and with too little free memory it stops at once.
fn bootlist_checks() -> Vec<iyakki_core::selfcheck::Check> {
    use iyakki_core::bootprefetch as bp;
    use iyakki_core::selfcheck::check;
    let mut v = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let list = std::path::Path::new(bp::LIST_PATH);
        for _ in 0..140 {
            if list.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let text = std::fs::read_to_string(list).unwrap_or_default();
        let entries = bp::parse_list(&text, bp::UNKNOWN_SIZE);
        v.push(check("list-written", list.is_file() && !text.is_empty(), format!("{} ({} bytes)", bp::LIST_PATH, text.len())));
        v.push(check("list-has-boot-files", entries.len() >= 20, format!("{} files", entries.len())));
        let bad: Vec<_> = entries.iter().filter(|e| bp::SKIP_PREFIXES.iter().any(|p| e.path.to_string_lossy().starts_with(p))).take(3).collect();
        v.push(check("list-skips-kernel-views", bad.is_empty(), format!("{bad:?}")));
        let all = bp::prefetch_from(list, &iyakki_core::boot::available_ram);
        v.push(check("prefetch-ran-to-the-end", all.contains("ended Done") && !all.contains("0 batch(es)"), all));
        // the plan is made with the real memory; then only 100 MiB are reported free while it runs (memory that dropped after the plan was made)
        let short = bp::prefetch_with(list, iyakki_core::boot::available_ram(), &|| 100 << 20);
        v.push(check("prefetch-stops-when-memory-is-short", short.contains("MemoryShort") && short.contains("0 batch(es)"), short));
    }
    #[cfg(not(target_os = "linux"))]
    v.push(check("linux-only", false, "the prefetch is for Linux"));
    v
}

fn mem_report(stage: &str) {
    let rd = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| format!("missing: {e}"));
    // a swap file is made in the background after login: give it time (up to 40 s) when one is configured
    let wants_file = |c: &str| c.lines().any(|l| l.trim().strip_prefix("swapfile=").is_some_and(|v| v != "off" && v != "0"));
    let experiment = iyakki_zram::compressed::resolve_experiment(&iyakki_storage::cmdline_text(), &rd(iyakki_zram::CONF_PATH)).0 == iyakki_zram::compressed::Experiment::On;
    if experiment || wants_file(&rd(iyakki_zram::CONF_PATH)) || iyakki_core::boot::cmdline_param(&iyakki_storage::cmdline_text(), "iyakki.swapfile").is_some_and(|v| v != "off" && v != "0") {
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
    // the experimental compressed memory (dist/qemu/compressed-memory-test.sh): zswap on or off, and the swap file
    out!("IYAKKI-SELFTEST {stage} zswap enabled {}", match iyakki_zram::zswap_enabled(&iyakki_zram::Env::real()) { Some(true) => "Y", Some(false) => "N", None => "absent" });
    out!("IYAKKI-SELFTEST {stage} swapfile {}", std::fs::metadata(iyakki_zram::SWAPFILE_PATH).map_or("none".to_string(), |m| format!("{} bytes", m.len())));
}

/// `iyakki hostname [NAME]`: show the host name, or set it (needs root): applied now, written to /etc/hostname and kept in
/// `system.conf` for the next boots. The boot option `iyakki.hostname=` locks it.
/// `iyakki fan-service [--sys DIR] [--data DIR] [--interval-ms N]`: the fan controller loop (F-DRIVERS-14), run by init as a supervised
/// child. It acts only when `fan.json` in the data folder says it is on; it hands the fans back to the firmware when it is stopped.
/// Every change of who controls the fans goes to standard output as an `AUDIT` line for the core to write to the audit log.
fn fan_service_cmd(args: &[String]) -> ExitCode {
    let (mut sys, mut data, mut ms) = ("/sys".to_string(), "/var/lib/iyakki".to_string(), 3000u64);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--sys", Some(v)) => sys = v.clone(),
            ("--data", Some(v)) => data = v.clone(),
            ("--interval-ms", Some(v)) => ms = v.parse().unwrap_or(3000).clamp(10, 60_000),
            _ => return fail("usage: iyakki fan-service [--sys DIR] [--data DIR] [--interval-ms N]".into()),
        }
    }
    let stop = match install_signal_handlers() {
        Ok(f) => f,
        Err(e) => return fail(format!("signal setup: {e}")),
    };
    iyakki_drivers::fanservice::run(std::path::Path::new(&sys), std::path::Path::new(&data), &stop, std::time::Duration::from_millis(ms), &mut |event, detail| {
        out!("AUDIT\t{event}\t{}", detail.replace(['\n', '\t'], " "));
    });
    ExitCode::SUCCESS
}

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
    let cmdline = iyakki_storage::cmdline_text();
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
    for i in iyakki_core::net::list_ifaces(std::path::Path::new(iyakki_core::net::SYS_CLASS_NET)) {
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
    // N1: the smallest real call of every runtime path that used to start an external program, against a loopback listener started here
    // (never the internet); `qemu-test.sh` requires each `IYAKKI-SELFTEST path <name> ok`
    for l in iyakki_agent::pathcheck::lines() {
        out!("{l}");
    }
    out!("IYAKKI-SELFTEST end");
}

/// The one set of accounts, services and settings every shell on this machine (the screen, browsers) works over.
#[cfg(feature = "shell")]
static SHARED_ENV: std::sync::Mutex<Option<iyakki_shell::shell::ShellEnv>> = std::sync::Mutex::new(None);

/// The root objects the UI socket serves when its `rpc` switch is on (F-SPLIT-03): the composition root knows which real object answers each trait.
#[cfg(feature = "shell")]
fn rpc_factory(env: &iyakki_shell::shell::ShellEnv) -> Option<iyakki_agent::contract_local::uiserve::RpcFactory> {
    Some(iyakki_compose::rpc::RpcRoots::new().with_audit(env.audit.clone()).factory())
}

#[cfg(feature = "shell")]
fn shell_env() -> Result<iyakki_shell::shell::ShellEnv, String> {
    shell_env_with(false)
}

/// The environment of a headless web service (`serve --profile NAME`): the same set, but no sound is set up or probed.
#[cfg(feature = "shell")]
fn service_shell_env() -> Result<iyakki_shell::shell::ShellEnv, String> {
    shell_env_with(true)
}

#[cfg(feature = "shell")]
fn shell_env_with(headless: bool) -> Result<iyakki_shell::shell::ShellEnv, String> {
    let mut shared = SHARED_ENV.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(e) = shared.as_ref() {
        return Ok(e.share());
    }
    let env = build_shell_env(headless)?;
    let mine = env.share();
    *shared = Some(env);
    Ok(mine)
}

#[cfg(feature = "shell")]
fn build_shell_env(headless: bool) -> Result<iyakki_shell::shell::ShellEnv, String> {
    // the composition root first: it installs the real agent handles before anything reads them
    iyakki_compose::wire();
    let host = std::sync::Arc::new(iyakki_core::host::CoreHost::new());
    let hwcheck = iyakki_core::boot::cmdline_param(&iyakki_storage::cmdline_text(), "iyakki.hwcheck") == Some("1");
    let paths = iyakki_shell::shell::ShellPaths { hwcheck, headless, ..Default::default() };
    let env = iyakki_shell::shell::ShellEnv::from_paths(&paths, host).map_err(|e| e.to_string())?;
    // live restart (F-SPLIT-06): the previous program handed over the vault keys of the people who were signed in, in a memory file
    iyakki_agent::livekeys::set_current(&iyakki_agent::services::Services::from_handle(&env.services));
    if let Some(fd) = std::env::var("IYAKKI_LIVE_KEYS_FD").ok().and_then(|v| v.parse::<i32>().ok()) {
        std::env::remove_var("IYAKKI_LIVE_KEYS_FD");
        match iyakki_core::liverestart::take_over(fd) {
            Ok(bytes) => log::info!("live restart: {} vault(s) opened with the keys handed over", iyakki_agent::services::Services::from_handle(&env.services).import_session_keys(&iyakki_agent::livekeys::unpack(&bytes))),
            Err(e) => log::warn!("live restart: the vault keys could not be read: {e}"),
        }
        iyakki_core::liverestart::release(fd);
    }
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
        // internet sharing over a cable (`iyakki share`, Settings -> Network -> Share internet): off after every boot, leftovers of a crash removed
        iyakki_agent::sharing::real::start_system(std::path::Path::new(iyakki_core::boot::DATA_DIR));
        // helper services (fan control, the privileged VPN helper): started only when their setting is on, restarted when they die
        // (docs/development/VPN-CONNECTOR-PLAN.md, F-DRIVERS-14); their audit lines are written here, by the one writer of the log
        if let Ok(exe) = std::env::current_exe() {
            let a = env.audit.clone();
            // SAFETY: getuid has no arguments and cannot fail.
            let uid = unsafe { libc::getuid() };
            let _ = iyakki_core::helpers::keep_running(exe, "/sys".into(), std::path::PathBuf::from(iyakki_core::boot::DATA_DIR), uid, std::sync::Arc::new(move |event, detail| a.record("system", event, detail)), std::time::Duration::from_secs(5), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        }
        iyakki_agent::directlink::manager::start_system(std::path::Path::new(iyakki_core::boot::DATA_DIR), std::sync::Arc::new(|ifaces, more| iyakki_fw::service::set_direct_links(ifaces, more)));
    }
    if std::process::id() == 1 {
        let svc = env.services.clone();
        iyakki_agent::ntp::spawn_sync_thread(move || svc.settings().lock().map(|s| (!s.get().time.no_ntp, s.get().time.ntp_server.clone())).unwrap_or((true, String::new())));
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
/// Set to leave the shell loop (see `liverestart::before_exec`).
static SHELL_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The UI: the program `iyakki-ui` under the supervisor when it exists and was asked for (F-SPLIT-03, `uiprocess`), else the shell thread.
/// A program the supervisor gave up on is replaced by the thread, so the person is never left without a screen.
#[cfg(feature = "shell")]
fn start_ui(cmdline: &str) {
    let exe = std::path::PathBuf::from(iyakki_core::uiprocess::EXE);
    let flag = std::path::Path::new(iyakki_core::uiprocess::FLAG).exists();
    if iyakki_core::uiprocess::mode(cmdline, flag, exe.exists()) != iyakki_core::uiprocess::Mode::Process {
        start_shell_thread();
        return;
    }
    log::info!("boot: the UI runs as its own program (iyakki-ui), supervised");
    let audit: iyakki_core::helpers::Audit = std::sync::Arc::new(|event, detail| log::info!("ui program: {event} {detail}"));
    let handle = iyakki_core::uiprocess::start(exe, "/dev/fb0", audit);
    let _ = std::thread::Builder::new().name("iyakki-ui-watch".into()).spawn(move || {
        while !iyakki_core::uiprocess::give_up_falls_back(&handle.state()) {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        log::error!("ui program: given up on after repeated crashes; the shell runs as a thread of the core instead");
        start_shell_thread();
        drop(handle);
    });
}

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
                // the program of a live restart stops this loop (and so lets go of the screen) before it replaces itself
                let stop = &SHELL_STOP;
                // the wake phrase counters move to their new layout through the two-phase library (F-CORE-31): staged next to the old file BEFORE
                // anything reads it, committed only for a start that was confirmed earlier and has run through its grace period
                {
                    let data = std::env::var("IYAKKI_DATA").unwrap_or_else(|_| iyakki_shell::shell::ShellPaths::default().data);
                    iyakki_agent::wakestats::boot_step(std::path::Path::new(&data), iyakki_agent::relinfo::VERSION, "previous", iyakki_agent::clock::now_unix());
                }
                // the boot screen lets go of the display before the desktop draws on it
                iyakki_core::boot::finish_boot_screen();
                // the sign-in screen is the first thing the shell draws: tell the boot loader this start worked (docs/BOOT-LOADER.md)
                iyakki_core::bootconfirm::confirm_soon(std::time::Duration::from_secs(2));
                // a live-restarted program reports healthy here too (the sign-in screen is about to be drawn)
                iyakki_core::liverestart::report_healthy_soon(iyakki_core::liverestart::Paths::real(), std::time::Duration::from_secs(2));
                // data migrations staged by an update (F-CORE-20): this version started, so the staged ones for it are confirmed and the
                // ones staged for another version are rolled back; the old data is never dropped here
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let data = std::env::var("IYAKKI_DATA").unwrap_or_else(|_| iyakki_shell::shell::ShellPaths::default().data);
                    for (id, phase) in iyakki_agent::twophase::reconcile(std::path::Path::new(&data), iyakki_agent::relinfo::VERSION, iyakki_agent::clock::now_unix()) {
                        log::info!("boot: data migration {id} is now {phase:?}");
                    }
                });
                // a monitor that woke up during boot but was not seen: one more look (in the background)
                iyakki_core::display::probe_once_in_background("the desktop started", iyakki_core::boot::screen_found);
                if let Err(e) = iyakki_shell::backend::run_fb(sh, fb, stop) {
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
/// `iyakki doctor [--detail]`: DOCTOR's checks printed as text, for a computer without a screen or a script (exit 0: nothing wrong,
/// 1: a problem or warning, 2: bad use). The facts come from the same reader as the tile (`iyakki_agent::doctor_gather`); this process
/// does not hold the running system's state, so the dev cluster, vault, VPN and failed services are not checked and the report says so.
#[cfg(feature = "shell")]
fn doctor_cmd(a: &[String]) -> ExitCode {
    use iyakki_agent::doctor_gather as dg;
    let detail = match dg::parse_cli(a) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let paths = iyakki_shell::shell::ShellPaths::default();
    let data = std::env::var("IYAKKI_DATA").unwrap_or(paths.data);
    let audit_path = std::env::var("IYAKKI_AUDIT").unwrap_or(paths.audit);
    let dir = std::path::Path::new(&data);
    let settings = match iyakki_agent::settings::SettingsStore::open(&dir.join("settings.json")) {
        Ok(s) => std::sync::Mutex::new(s),
        Err(e) => return fail(format!("{data}/settings.json: {e}")),
    };
    let audit = std::sync::Arc::new(iyakki_shell::accounts::AuditLog::new(std::path::Path::new(&audit_path)));
    let g = dg::Gather { dir, user: "", settings: &settings, live: None };
    let slow: dg::SlowProbe = std::sync::Arc::new(dg::slow_probe);
    let (text, code) = dg::cli_report(&g, std::sync::Arc::new(move |n| audit.recent(n)), detail, &slow);
    out_raw!("{}", text);
    ExitCode::from(code as u8)
}


/// `iyakki user add NAME [--role admin|operator|user|guest] [--password-stdin | --password-file FILE]` and `iyakki user list`:
/// accounts for a machine with no screen (a server, a container) where the first-run setup screen cannot be used.
#[cfg(feature = "headless")]
fn user_cmd(a: &[String]) -> ExitCode {
    use iyakki_sec::policy::Role;
    let users_path = std::env::var("IYAKKI_USERS").unwrap_or_else(|_| iyakki_core::svc::USERS_PATH.to_string());
    let mut store = match iyakki_accounts::UserStore::open(std::path::Path::new(&users_path)) {
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
                Err(iyakki_accounts::AccountError::Exists) if if_missing => {
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
    // the stable device name (mDNS) follows its Settings switch, in its own thread
    if let Ok(env) = shell_env() {
        if let Err(e) = iyakki_remote::devname::watch(env) {
            log::error!("device name: could not start its watcher: {e}");
        }
    }
    // The UI socket (F-SPLIT-07): the typed contract for a UI program on this machine over a unix socket. Off by default; follows its
    // switch (chat tool ui_socket_set), in its own thread. No network port.
    if let Ok(env) = shell_env() {
        let core = iyakki_agent::contract_local::core_for(&iyakki_agent::services::Services::from_handle(&env.services));
        // the assistant and the camera views of the same services, for the person the socket is set to (their agent switch and budget, the
        // machine's model, the audit log)
        let accounts = env.store.clone();
        let policy_of: std::sync::Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync> = std::sync::Arc::new(move |user: &str| accounts.lock().ok().and_then(|s| s.get(user).map(|r| r.policy())));
        let agent = iyakki_agent::corehandle::AgentCore::new(iyakki_agent::services::Services::from_handle(&env.services))
            .with_embedded_assistant(env.chat.clone(), policy_of.clone(), std::sync::Arc::new(iyakki_core::svc::ShellAudit(env.audit.clone())))
            .with_embedded_cameras(policy_of);
        let facets = iyakki_agent::contract_local::uiserve::Facets { assistant: Some(agent.shared_assistant()), cameras: Some(agent.shared_cameras()), rpc: rpc_factory(&env) };
        let _ = iyakki_agent::contract_local::uiserve::watch_with(core, env.hostname.clone(), std::path::PathBuf::from("/run/iyakki/ui/ui.sock"), facets);
    }
    // "Serve this desktop to browsers" (Settings -> Remote access) follows its switch, in its own thread
    if let Ok(env) = shell_env() {
        if let Err(e) = iyakki_remote::webswitch::watch(env) {
            log::error!("remote access: could not start its watcher: {e}");
        }
    }
}

/// `iyakki serve --profile NAME`: the headless web service (docs/development/MINIMAL-WEBSERVICE.md). With `--check` it only
/// validates the configuration and says what would start.
#[cfg(feature = "headless")]
fn serve_profile(ask: &iyakki_core::svc::Ask) -> ExitCode {
    let chk = match iyakki_core::svc::load(ask) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if ask.check_only {
        out!("{}", iyakki_core::svc::summary(&chk));
        return ExitCode::SUCCESS;
    }
    let env = match service_env() {
        Ok(e) => e,
        Err(e) => return fail(e),
    };
    let shutdown = match install_signal_handlers() {
        Ok(f) => f,
        Err(e) => return fail(format!("signal setup: {e}")),
    };
    match iyakki_core::svc::run(&chk, env, &shutdown) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

/// The accounts, audit log, services and host the service works over: the shell's own set on a build with the shell (one set per
/// machine), else a set made without the desktop.
#[cfg(feature = "shell")]
fn service_env() -> Result<iyakki_core::svc::ServiceEnv, String> {
    service_shell_env().map(|e| iyakki_core::svc::ServiceEnv::from_shell(&e))
}

#[cfg(all(feature = "headless", not(feature = "shell")))]
fn service_env() -> Result<iyakki_core::svc::ServiceEnv, String> {
    iyakki_core::svc::ServiceEnv::headless(std::sync::Arc::new(iyakki_core::host::CoreHost::new()))
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

/// The arguments after the command word. As PID 1 the program is started with no arguments at all and the command
/// defaults to `serve`, so `args` can be empty: `&args[1..]` would panic and the boot would stop.
fn serve_args(args: &[String]) -> &[String] {
    args.get(1..).unwrap_or(&[])
}

#[cfg(test)]
mod serve_args_tests {
    use super::serve_args;

    #[test]
    fn no_arguments_at_boot_give_an_empty_list_instead_of_a_panic() {
        assert!(serve_args(&[]).is_empty());
        assert!(serve_args(&["serve".to_string()]).is_empty());
        let a = ["serve".to_string(), "--profile".to_string(), "agent-api".to_string()];
        assert_eq!(serve_args(&a), &a[1..]);
    }
}
