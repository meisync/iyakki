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
  devices [--default NAME | --forget NAME | --rename OLD NEW | --prune [--yes]]   the devices this computer paired with (last seen, duplicates and stale entries marked)
  usb [check|authorize|pair|speed|link|gadget]   connect two computers with ONE cable (Thunderbolt/USB4 or a bridge cable), see the steps
  install <pkg.iyp | folder> [--shared] [--open]   install a tile app, skill, MCP server, extension... (Developer (local))
  uninstall <id>                      remove something developer mode installed (kind:name or name)
  list [--all]                        what developer mode installed on the device (--all: everything you can see)
  logs <id> [--follow]                the app's log on the device (installs, removals, its page's console)
  events [settings | flows [RUN]] [--after N] [--seconds N] [--json]   the events the device PUSHES while you listen (core contract; needs the contract on and its read scope)
  run <id> | open <id>                open the app on the device's screen
  push <folder> [--watch] [--open]    pack and install the folder; with --watch again on every change (live reload)
  screenshot [-o FILE.png]            a picture of the device's screen
  whoami                              the device, the account this computer acts as, and the scopes it holds
  info | ps | dmesg [--follow] [--grep X] | logs --system | status      look at the device (scope debug)
  doctor [--detail]                                                   the DOCTOR report of the device, with its dev cluster, vault and VPN (scope debug, administrator)
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
  push-apps --bin FILE [--target PATH] [--against OLDFILE] [--add] [--shell] [--layer live-ui|live-core|reboot]   ship the apps program as a delta, no image build (scope update; the device answers the approval; no reboot is done for you)
{LOOP}
  reboot | poweroff                   restart or switch off the device (approval on its screen)
  update PAYLOAD|IMAGE.img [--force] [--reboot] | update --status | update --discard    replace the boot files with a newer IYAKKI (scope update)
  update-pack IMAGE.img -o DIR [--version V] [--variant NAME] [--against PREV] [--sign KEYFILE]   make the payload from an install image (nothing is written to the image); --against PREV (a layers folder or an earlier v2 pack) makes a layered pack that carries only the changed layers; --sign adds update.json.sig for Settings -> System update
  screenshot [--display N] [--record SECONDS [--fps N]] [-o FILE|DIR]
options for every command: --device NAME (default: the first paired), --home DIR (default: ~/.config/iyakki/dev or $IYAKKI_DEV_HOME)";

/// The usage text of `iyakki dev` (the base text with the diagnosis commands in place).
pub fn usage() -> String {
    USAGE_BASE.replace("{LOOP}", super::cli_loop::USAGE).replace("{DIAG}", &format!("{}\n{}\n{}", super::cli_diag::USAGE, super::cli_follow::USAGE, super::cli_webruns::USAGE))
}

pub(super) struct Args {
    pub(super) pos: Vec<String>,
    pub(super) flags: Vec<(String, Option<String>)>,
    /// Everything after a bare `--` (a command to run on the device).
    pub(super) after: Vec<String>,
}

const WITH_VALUE: [&str; 45] = ["--code", "--name", "--as", "--fingerprint", "--device", "--home", "-o", "--default", "--forget", "--rename", "--layer", "--grep", "-n", "--source", "--display", "--record", "--fps", "--minutes", "--reason", "--lines", "--image", "--set", "--kind", "--url", "--key-file", "--key-env", "--ports", "--proto", "--comment", "--file", "--value", "--version", "--variant", "--sign", "--delta-from", "--against", "--bin", "--target", "--seconds", "--max", "--repo", "--base", "--reopen", "--fetch", "--retries"];

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
    if let Some(r) = super::cli_push_apps::run(cmd, &args, &home, &connect, out) {
        return r;
    }
    if let Some(r) = super::cli_loop::run(cmd, &args, &connect, out) {
        return r;
    }
    if let Some(r) = super::cli_follow::run(cmd, &args, &home, out) {
        return r;
    }
    if let Some(r) = super::cli_webruns::run(cmd, &args, &home, out) {
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
            let gone = home.replace_duplicates_of(&d)?;
            if !gone.is_empty() {
                out(&format!("The same device was saved before as {}; that entry is replaced by this one.", gone.join(", ")));
            }
        }
        "scan" => {
            for l in scan_report(&iyakki_netscan::mdns::net::scan(&iyakki_netscan::mdns::net::Ask::new(SCAN_WAIT))) {
                out(&l);
            }
        }
        "devices" => {
            let now = crate::clock::now_unix();
            if let Some(n) = args.get("--forget") {
                home.remove(n)?;
                out(&format!("Forgot {n} on this computer (remove this computer on the device too: Settings, Developer mode, Remove this computer)."));
                return Ok(());
            }
            if let Some(old) = args.get("--rename") {
                let new = arg1.ok_or("usage: iyakki dev devices --rename OLD NEW")?;
                home.rename(old, new)?;
                out(&format!("Renamed {old} to {new}."));
                return Ok(());
            }
            if args.has("--prune") {
                let mut confirm = |q: &str| {
                    crate::qeprintln!("{q}");
                    read_line("", &mut |_| {}).to_ascii_lowercase().starts_with('y')
                };
                return super::cli_devices::prune(&home, args.has("--yes"), now, &|d| super::cli_devices::answers(&home, d), &mut confirm, out);
            }
            if let Some(n) = args.get("--default") {
                home.set_default(n)?;
            }
            let list = super::cli_devices::list_lines(&home, now);
            if list.is_empty() {
                out("No device is paired yet: run `iyakki dev pair <address>`.");
            }
            for l in list {
                out(&l);
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
        "events" => {
            let (domain, call) = match arg1 {
                None | Some("settings") => ("settings", "watch"),
                Some("flows") => ("flows", "events"),
                Some(_) => return Err("usage: iyakki dev events [settings | flows [RUN]] [--after N] [--seconds N] [--json]".into()),
            };
            let secs: u64 = args.get("--seconds").map(|n| n.parse().map_err(|_| "--seconds is a whole number".to_string())).transpose()?.unwrap_or(60);
            let resume = match args.get("--after") {
                Some(n) => serde_json::json!({ "after": n.parse::<u64>().map_err(|_| "--after is a whole number".to_string())? }),
                None => serde_json::json!("latest"),
            };
            let mut a = serde_json::json!({ "resume": resume });
            if let (Some(run), "flows") = (args.pos.get(2), domain) {
                a["run"] = serde_json::json!(run);
            }
            let raw = args.has("--json");
            let c = connect()?;
            let mut failed: Option<String> = None;
            c.push_events(&serde_json::json!({ "domain": domain, "call": call, "args": a, "max_ms": secs.clamp(1, 600) * 1000 }), &mut |v| {
                if v["ok"] != true {
                    failed = Some(v["error"]["message"].as_str().unwrap_or("the device refused the stream").to_string());
                    return false;
                }
                for e in v["result"]["events"].as_array().into_iter().flatten() {
                    if raw {
                        out(&e.to_string());
                    } else {
                        out(&format!("#{} {} {}", e["seq"], e["topic"].as_str().unwrap_or(""), e["body"]));
                    }
                }
                if v["result"]["gap"] == true {
                    out("(events were missed: read the state again before trusting the stream)");
                }
                true
            })?;
            if let Some(f) = failed {
                return Err(f);
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

/// How long `iyakki dev scan` waits for answers (the name lookup of a paired device waits `client::RESOLVE_WAIT`, the same time).
pub const SCAN_WAIT: Duration = Duration::from_secs(2);

/// The lines `iyakki dev scan` prints: one per device with its name, its addresses and its services, or the help line when none answered.
pub fn scan_report(found: &[iyakki_netscan::mdns::net::Found]) -> Vec<String> {
    if found.is_empty() {
        return vec!["No IYAKKI device answered. It must be on this network with \"Announce this device on the local network\" on (Settings, Network); some Wi-Fi access points block this kind of message between devices.".to_string()];
    }
    found
        .iter()
        .map(|f| {
            let svc: Vec<String> = f.services.iter().map(|(k, p)| format!("{k}:{p}")).collect();
            format!("{:<24} {:<32} {}", f.host, f.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","), svc.join(" "))
        })
        .collect()
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    use iyakki_netscan::mdns::net::Found;

    #[test]
    fn the_scan_lists_name_addresses_and_services_and_says_what_to_check_when_nobody_answers() {
        let found = vec![
            Found { host: "iyakki-0a1b2c.local".into(), addrs: vec!["192.0.2.66".parse().unwrap(), "fd00::1".parse().unwrap()], services: vec![("_iyakki-dev._tcp".into(), 7979), ("_iyakki-web._tcp".into(), 8443)], txt: vec![] },
            Found { host: "iyakki-ffeedd.local".into(), addrs: vec![], services: vec![("_iyakki-dev._tcp".into(), 7979)], txt: vec![] },
        ];
        let lines = scan_report(&found);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("iyakki-0a1b2c.local") && lines[0].contains("192.0.2.66,fd00::1") && lines[0].ends_with("_iyakki-dev._tcp:7979 _iyakki-web._tcp:8443"), "{lines:?}");
        assert!(lines[1].contains("iyakki-ffeedd.local") && lines[1].ends_with("_iyakki-dev._tcp:7979"));
        let none = scan_report(&[]);
        assert_eq!(none.len(), 1);
        assert!(none[0].contains("Announce this device on the local network") && none[0].contains("block"));
    }

    #[test]
    fn the_scan_and_the_name_lookup_both_give_up_after_two_seconds() {
        assert_eq!(SCAN_WAIT, Duration::from_secs(2));
        assert_eq!(super::super::client::RESOLVE_WAIT, Duration::from_secs(2));
        // and a scan against a responder that is not there returns with nothing instead of waiting for ever
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let ask = iyakki_netscan::mdns::net::Ask { timeout: Duration::from_millis(300), dests: vec![dead], allow_loopback: true };
        assert!(iyakki_netscan::mdns::net::scan(&ask).is_empty());
    }
}
