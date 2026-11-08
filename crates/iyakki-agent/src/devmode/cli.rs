//! `iyakki dev ...` on the developer's computer: pair once, then install, remove, list, read logs, open apps, take a screenshot and
//! push a folder on every change (live reload). Talks only to paired devices, over their TLS, with signed requests ([`super::client`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::client::{self, DevClient, Home};
use super::{ops, DEFAULT_PORT};

pub const USAGE_BASE: &str = "usage: iyakki dev <command>
  pair <address[:port] | name.local> [--code CODE] [--name THIS-COMPUTER] [--as DEVICE-NAME] [--fingerprint FP]
                                      pair once with the code and fingerprint the device shows (Settings, Developer mode)
  scan                                list the IYAKKI devices that announce themselves on this network (name, address, services)
  devices [--default NAME | --forget NAME]   the devices this computer paired with
  usb [check|authorize|pair|speed|link|gadget]   connect two computers with ONE cable (Thunderbolt/USB4 or a bridge cable), see the steps
  install <pkg.iyp | folder> [--shared] [--open]   install a tile app, skill, MCP server, extension... (Developer (local))
  uninstall <id>                      remove something developer mode installed (kind:name or name)
  list [--all]                        what developer mode installed on the device (--all: everything you can see)
  logs <id> [--follow]                the app's log on the device (installs, removals, its page's console)
  run <id> | open <id>                open the app on the device's screen
  push <folder> [--watch] [--open]    pack and install the folder; with --watch again on every change (live reload)
  screenshot [-o FILE.png]            a picture of the device's screen
  whoami                              the device, the account this computer acts as, and the scopes it holds
  info | ps | dmesg [--follow] [--grep X] | logs --system | status      look at the device (scope debug)
  files ls|stat|push|pull|rm|mkdir PATH       copy files to and from the device (scope files)
  input tap X Y | key NAME | text 'abc' | swipe X1 Y1 X2 Y2 | click \"LABEL\"    drive the screen (scope input)
{DIAG}
  config list | get KEY | set KEY VALUE | export [FILE] | import FILE      settings through a whitelist (scope config)
  config provider add ID --kind K [--url U] [--key-file F|--key-env V|--key-stdin] | key ID ... | remove ID     (keys are write-only)
  config firewall list | allow VALUE [--ports P] [--proto tcp|udp|both] [--minutes N] | remove ID | keep | revert
  app info|permissions|clear-data|reset APP | app data APP [KEY] | app config APP get|set|rm KEY [VALUE]  tile apps from developer mode
  shell [--minutes N] [--root] [--reason TEXT] [-- CMD ARGS]    run commands on the device (scope shell + approval on its screen)
  forward LOCAL:REMOTE                forward 127.0.0.1:LOCAL here to the device's own port REMOTE (scope forward)
  patch build DIR -o PACK [--delta-from OLD] | show PACK | apply PACK [--live|--next-boot] [--shell] | list | remove ID     hot-patch files of the running system (scope update)
  reboot | poweroff                   restart or switch off the device (approval on its screen)
  update PAYLOAD|IMAGE.img [--force] [--reboot] | update --status | update --discard    replace the boot files with a newer IYAKKI (scope update)
  update-pack IMAGE.img -o DIR [--version V] [--variant NAME] [--sign KEYFILE]   make the payload from an install image (nothing is written to the image); --sign adds update.json.sig for Settings -> System update
  screenshot [--display N] [--record SECONDS [--fps N]] [-o FILE|DIR]
options for every command: --device NAME (default: the first paired), --home DIR (default: ~/.config/iyakki/dev or $IYAKKI_DEV_HOME)";

/// The usage text of `iyakki dev` (the base text with the diagnosis commands in place).
pub fn usage() -> String {
    USAGE_BASE.replace("{DIAG}", super::cli_diag::USAGE)
}

pub(super) struct Args {
    pub(super) pos: Vec<String>,
    pub(super) flags: Vec<(String, Option<String>)>,
    /// Everything after a bare `--` (a command to run on the device).
    pub(super) after: Vec<String>,
}

const WITH_VALUE: [&str; 33] = ["--code", "--name", "--as", "--fingerprint", "--device", "--home", "-o", "--default", "--forget", "--grep", "-n", "--source", "--display", "--record", "--fps", "--minutes", "--reason", "--lines", "--image", "--set", "--kind", "--url", "--key-file", "--key-env", "--ports", "--proto", "--comment", "--file", "--value", "--version", "--variant", "--sign", "--delta-from"];

impl Args {
    pub(super) fn parse(a: &[String]) -> Result<Args, String> {
        let mut pos = Vec::new();
        let mut flags = Vec::new();
        let mut after = Vec::new();
        let mut it = a.iter();
        while let Some(x) = it.next() {
            if x == "--" {
                after = it.by_ref().cloned().collect();
                break;
            }
            if let Some((k, v)) = x.split_once('=').filter(|(k, _)| k.starts_with("--")) {
                flags.push((k.to_string(), Some(v.to_string())));
            } else if WITH_VALUE.contains(&x.as_str()) {
                flags.push((x.clone(), Some(it.next().ok_or_else(|| format!("{x} needs a value"))?.clone())));
            } else if x.starts_with('-') && x.len() > 1 {
                flags.push((x.clone(), None));
            } else {
                pos.push(x.clone());
            }
        }
        Ok(Args { pos, flags, after })
    }

    pub(super) fn get(&self, k: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| f == k).and_then(|(_, v)| v.as_deref())
    }

    pub(super) fn has(&self, k: &str) -> bool {
        self.flags.iter().any(|(f, _)| f == k)
    }
}

fn computer_name() -> String {
    let h = std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()).filter(|s| super::valid_name(s));
    h.or_else(|| std::env::var("HOSTNAME").ok().filter(|s| super::valid_name(s))).unwrap_or_else(|| "laptop".into())
}

fn read_line(prompt: &str, out: &mut dyn FnMut(&str)) -> String {
    out(prompt);
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s.trim().to_string()
}

fn show_install(v: &Value) -> String {
    let mut s = format!(
        "{} {}:{} {} for {} ({}){}",
        if v["reinstalled"].as_bool() == Some(true) { "Reinstalled" } else { "Installed" },
        v["kind"].as_str().unwrap_or(""),
        v["name"].as_str().unwrap_or(""),
        v["version"].as_str().unwrap_or(""),
        if v["scope"] == "shared" { "everyone".to_string() } else { v["scope"].as_str().unwrap_or("").trim_start_matches("user/").to_string() },
        "Developer (local)",
        if v["kind"] == "tile" { format!("; open it with `iyakki dev run {}`", v["name"].as_str().unwrap_or("")) } else { String::new() }
    );
    for p in v["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        s.push_str(&format!("\n  may: {p}"));
    }
    for w in v["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        s.push_str(&format!("\n  warning: {w}"));
    }
    s
}

/// What `push` does besides installing once.
#[derive(Clone, Copy, Debug, Default)]
pub struct Push {
    pub shared: bool,
    /// Open the app on the device's screen after the first install.
    pub open: bool,
    /// Keep watching the folder and install again on every change.
    pub watch: bool,
}

/// Pack and install `dir`; with `watch`, again whenever a file in it changes, for as long as `go_on` says so (live reload: an open copy
/// of a tile app reloads by itself on the device). A build that fails is reported and the watch goes on.
pub fn push(c: &DevClient, dir: &Path, key: &ed25519_dalek::SigningKey, opts: Push, out: &mut dyn FnMut(&str), go_on: &dyn Fn() -> bool) -> Result<(), String> {
    let mut opened = false;
    let mut once = |out: &mut dyn FnMut(&str)| -> Result<(), String> {
        let bytes = ops::package_bytes(dir, key)?;
        let v = c.install(&bytes, opts.shared)?;
        out(&show_install(&v));
        if opts.open && !opened {
            c.open(v["name"].as_str().unwrap_or(""))?;
            opened = true;
        }
        Ok(())
    };
    once(out)?;
    if !opts.watch {
        return Ok(());
    }
    out(&format!("Watching {} for changes (Ctrl+C stops). Each change is packed and installed again; an open copy reloads.", dir.display()));
    let mut stamp = ops::folder_stamp(dir);
    while go_on() {
        std::thread::sleep(Duration::from_millis(400));
        if ops::folder_stamp(dir) == stamp {
            continue;
        }
        // let a burst of saves settle
        std::thread::sleep(Duration::from_millis(300));
        stamp = ops::folder_stamp(dir);
        if let Err(e) = once(out) {
            out(&format!("not installed: {e}"));
        }
    }
    Ok(())
}

/// Run `iyakki dev <args>`; lines go to `out` as they happen (`logs --follow` and `push --watch` run until interrupted).
pub fn run(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let args = Args::parse(a)?;
    let home = Home::at(&args.get("--home").map(PathBuf::from).unwrap_or_else(client::default_home));
    let Some(cmd) = args.pos.first().map(String::as_str) else { return Err(usage()) };
    let arg1 = args.pos.get(1).map(String::as_str);
    let connect = || -> Result<DevClient, String> { DevClient::new(&home, home.device(args.get("--device"))?) };
    if let Some(r) = super::cli_adb::run(cmd, &args, &connect, out) {
        return r;
    }
    if let Some(r) = super::cli_patch::run(cmd, &args, &connect, out) {
        return r;
    }
    if let Some(r) = super::cli_diag::run(cmd, a, &args, &connect, out) {
        return r;
    }
    match cmd {
        "help" | "--help" => out(&usage()),
        "usb" => {
            // the raw words after `usb`: it has flags of its own (`--fixed-pair on`)
            let at = a.iter().position(|x| x == "usb").unwrap_or(0);
            return crate::directlink::cli::run(&a[at + 1..], home.dir(), out);
        }
        "pair" => {
            let (host, port) = client::split_host(arg1.ok_or("usage: iyakki dev pair <address[:port] | name.local> [--code CODE]")?, DEFAULT_PORT)?;
            // a name like iyakki-0a1b2c.local is looked up on the local network with our own resolver
            let ip = if iyakki_netscan::mdns::is_local_name(&host) { client::resolve_local(&host).into_iter().next().ok_or_else(|| format!("cannot find {host} on the local network (is the device on, on the same network, and announcing its name?)"))? } else { host.clone() };
            let (fp, info) = client::hello(&ip, port)?;
            out(&format!("Device {} (IYAKKI {}) at {host}:{port}{}", info["name"].as_str().unwrap_or("?"), info["version"].as_str().unwrap_or("?"), if ip != host { format!(" ({ip})") } else { String::new() }));
            out(&format!("Its certificate fingerprint: {fp}\nCheck that the device's screen shows the same fingerprint."));
            if info["pairing"].as_bool() != Some(true) {
                return Err("no pairing code is open on the device: an administrator chooses Pair a computer in Settings, Developer mode".into());
            }
            let code = match args.get("--code") {
                Some(c) => c.to_string(),
                None => read_line("Pairing code shown on the device: ", out),
            };
            let name = args.get("--name").map(str::to_string).unwrap_or_else(computer_name);
            let d = client::pair(&home, &host, port, &name, &code, args.get("--fingerprint").or(Some(&fp)), args.get("--as"))?;
            out(&format!("Paired: this computer is “{name}” on {} and acts as the account {}. Saved as device “{}” in {}.", host, d.account, d.name, home.dir().display()));
        }
        "scan" => {
            let found = iyakki_netscan::mdns::net::scan(&iyakki_netscan::mdns::net::Ask::new(Duration::from_secs(2)));
            if found.is_empty() {
                out("No IYAKKI device answered. It must be on this network with \"Announce this device on the local network\" on (Settings, Network); some Wi-Fi access points block this kind of message between devices.");
            }
            for f in found {
                let svc: Vec<String> = f.services.iter().map(|(k, p)| format!("{k}:{p}")).collect();
                out(&format!("{:<24} {:<32} {}", f.host, f.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","), svc.join(" ")));
            }
        }
        "devices" => {
            if let Some(n) = args.get("--forget") {
                home.remove(n)?;
                out(&format!("Forgot {n} on this computer (revoke the key on the device too: Settings, Developer mode)."));
                return Ok(());
            }
            if let Some(n) = args.get("--default") {
                home.set_default(n)?;
            }
            let def = home.default_name();
            let list = home.devices();
            if list.is_empty() {
                out("No device is paired yet: run `iyakki dev pair <address>`.");
            }
            for d in list {
                out(&format!("{} {:<16} {}{}:{}  as {}  key {}", if d.name == def { "*" } else { " " }, d.name, d.host, if d.addr.is_empty() { String::new() } else { format!(" ({})", d.addr) }, d.port, d.account, d.fingerprint.chars().take(23).collect::<String>()));
            }
            if let Some(k) = home.public_key() {
                out(&format!("This computer's key: {k}"));
            }
        }
        "whoami" => {
            let c = connect()?;
            let v = c.whoami()?;
            out(&format!("{} ({}:{}): this computer is “{}”, acting as {} ({}); developer mode {}", v["device"].as_str().unwrap_or(""), c.device.host, c.device.port, v["computer"].as_str().unwrap_or(""), v["account"].as_str().unwrap_or(""), v["role"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or("")));
            let sc: Vec<String> = v["scopes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            out(&format!("Scopes this computer holds: {}", if sc.is_empty() { "none".to_string() } else { sc.join(", ") }));
            for l in super::cli_diag::scope_lines(&sc) {
                out(&l);
            }
        }
        "install" => {
            let path = Path::new(arg1.ok_or("usage: iyakki dev install <pkg.iyp | folder> [--shared] [--open]")?);
            let c = connect()?;
            let bytes = ops::package_bytes(path, &home.key()?)?;
            let v = c.install(&bytes, args.has("--shared"))?;
            out(&show_install(&v));
            if args.has("--open") {
                c.open(v["name"].as_str().unwrap_or(""))?;
                out("Opened on the device's screen.");
            }
        }
        "uninstall" | "remove" => {
            let id = arg1.ok_or("usage: iyakki dev uninstall <id>")?;
            let v = connect()?.uninstall(id)?;
            out(&format!("Removed {}:{} {} (its saved data is kept).", v["kind"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or(""), v["version"].as_str().unwrap_or("")));
        }
        "list" => {
            let v = connect()?.list(args.has("--all"))?;
            let items = v["items"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                out("Nothing installed from a developer computer.");
            }
            for i in items {
                out(&format!("{:<10} {:<24} {:<10} {:<18} {:<14} {}", i["kind"].as_str().unwrap_or(""), i["name"].as_str().unwrap_or(""), i["version"].as_str().unwrap_or(""), i["trust"].as_str().unwrap_or(""), i["scope"].as_str().unwrap_or(""), i["source"].as_str().unwrap_or("")));
            }
        }
        "logs" => {
            let id = arg1.ok_or("usage: iyakki dev logs <id> [--follow]")?;
            let name = id.rsplit(':').next().unwrap_or(id);
            let c = connect()?;
            let mut since = 0u64;
            loop {
                let v = c.logs(name, since)?;
                let t = v["text"].as_str().unwrap_or("");
                if !t.is_empty() {
                    out(t.trim_end_matches('\n'));
                }
                let next = v["next"].as_u64().unwrap_or(since);
                let more = next > since && v["more"].as_bool() == Some(true);
                since = next;
                if more {
                    continue;
                }
                if !args.has("--follow") && !args.has("-f") {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "run" | "open" => {
            let id = arg1.ok_or("usage: iyakki dev run <id>")?;
            let v = connect()?.open(id.rsplit(':').next().unwrap_or(id))?;
            out(v["message"].as_str().unwrap_or("Asked the device to open it."));
        }
        "push" => {
            let dir = PathBuf::from(arg1.ok_or("usage: iyakki dev push <folder> [--watch] [--open]")?);
            if !dir.is_dir() {
                return Err(format!("{} is not a folder", dir.display()));
            }
            let c = connect()?;
            let opts = Push { shared: args.has("--shared"), open: args.has("--open"), watch: args.has("--watch") };
            push(&c, &dir, &home.key()?, opts, out, &|| true)?;
        }
        "screenshot" => {
            let png = connect()?.screenshot()?;
            let file = args.get("-o").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("iyakki-screen-{}.png", crate::clock::now_unix())));
            std::fs::write(&file, &png).map_err(|e| format!("{}: {e}", file.display()))?;
            out(&format!("Saved {} ({} KB).", file.display(), png.len().div_ceil(1024)));
        }
        other => return Err(format!("unknown command {other:?}\n{}", usage())),
    }
    Ok(())
}
