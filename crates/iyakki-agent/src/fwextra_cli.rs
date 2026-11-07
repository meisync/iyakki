//! The command lines of the hardware-firmware item and the real-machine side of `fwextra_net.rs`.
//!
//! ```text
//! iyakki quick-install fw-hardware [--data D] [--list] [--accept-licence] [--from FOLDER] [--with nvidia] [--link IFACE [--link-everything]] [--remove] [--delete-download]
//!                                   the job the Setup card starts (and the same by hand on the device)
//! iyakki firmware list [--log FILE]  what this computer asks for, grouped by vendor with the licences
//! iyakki firmware fetch --for DEVICE [--usb-link] | --names A,B | --log FILE   [--out DIR] [--with nvidia]
//!                                   on ANOTHER computer: download, verify, and (with --usb-link) push to the device over the cable
//! iyakki usb share-internet [--device NAME] [--uplink IFACE] [--apply | --undo]
//!                                   on the other computer: print (or with --apply run) the commands that share its internet over the cable
//! ```

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::fwextra::{self, manifest, Env, Manifest, Opts, Plan, Request, Source};
use crate::fwextra_net::{self as net, Iface, Kind, NetEnv, Situation};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

// ------------------------------------------------------------------------------------------------ the real network side

pub struct RealNet;

impl NetEnv for RealNet {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        let (cmd, rest) = argv.split_first().ok_or("an empty command")?;
        let o = std::process::Command::new(cmd).args(rest).output().map_err(|e| format!("{cmd}: {e}"))?;
        if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
    }
    fn resolve(&mut self, server: Ipv4Addr, name: &str) -> Vec<Ipv4Addr> {
        let Some(q) = net::dns_query(0x4957, name) else { return Vec::new() };
        let Ok(s) = UdpSocket::bind("0.0.0.0:0") else { return Vec::new() };
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
        if s.send_to(&q, SocketAddr::from((server, 53))).is_err() {
            return Vec::new();
        }
        let mut buf = [0u8; 1500];
        match s.recv_from(&mut buf) {
            Ok((n, _)) => net::dns_parse_a(0x4957, &buf[..n]),
            Err(_) => Vec::new(),
        }
    }
    fn read_hosts(&self) -> String {
        std::fs::read_to_string("/etc/hosts").unwrap_or_default()
    }
    fn write_hosts(&mut self, text: &str) -> Result<(), String> {
        std::fs::write("/etc/hosts", text).map_err(|e| e.to_string())
    }
    fn reaches(&mut self, host: &str) -> bool {
        (host, 443).to_socket_addrs().map(|mut a| a.any(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(6)).is_ok())).unwrap_or(false)
    }
    fn audit(&mut self, line: &str) {
        crate::qprintln!("audit: {line}");
    }
}

/// The interfaces of this computer from `/sys/class/net`, `/proc/net/route` and the addresses.
pub fn read_ifaces_real() -> Vec<Iface> {
    let route = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let gws = net::gateways_from_route(&route);
    let v4 = iyakki_devctl::ipv4_addresses();
    let mut out = Vec::new();
    for e in std::fs::read_dir("/sys/class/net").into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        let driver = std::fs::read_link(p.join("device/driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let kind = net::classify(&name, &driver, p.join("wireless").exists() || p.join("phy80211").exists());
        if kind == Kind::Other {
            continue;
        }
        let carrier = std::fs::read_to_string(p.join("carrier")).map(|c| c.trim() == "1").unwrap_or(false);
        let ipv4 = v4.iter().filter(|(n, _)| n.split(':').next() == Some(name.as_str())).filter_map(|(_, a)| a.parse().ok()).collect();
        out.push(Iface { gateway: gws.iter().find(|(n, _)| *n == name).map(|g| g.1), name, kind, carrier, ipv4 });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// What the machine looks like to the card (blocks for a few seconds: one connection test). Call it from a worker, not from drawing.
pub fn situation_now(sys: &Path) -> Situation {
    let ifaces = read_ifaces_real();
    let route = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let mut internet_on = Vec::new();
    if let Ok(addrs) = ("git.kernel.org", 443).to_socket_addrs() {
        for a in addrs.filter(|a| a.is_ipv4()) {
            if std::net::TcpStream::connect_timeout(&a, Duration::from_secs(4)).is_ok() {
                if let std::net::IpAddr::V4(ip) = a.ip() {
                    internet_on.extend(net::route_iface_for(&route, ip));
                }
                break;
            }
        }
    }
    let sticks = net::find_sticks(&[PathBuf::from("/media"), PathBuf::from("/run/media"), PathBuf::from("/mnt")]);
    let devlink_up = std::fs::read_to_string(crate::directlink::STATE_FILE).ok().and_then(|t| serde_json::from_str::<crate::directlink::Snapshot>(&t).ok()).is_some_and(|s| !s.peers.is_empty());
    let devs = fwextra::read_devs(sys);
    let wifi_card = devs.iter().any(|d| d.class >> 8 == 0x0280);
    let wifi_lacks_firmware = wifi_card && !ifaces.iter().any(|i| i.kind == Kind::Wifi);
    Situation { ifaces, internet_on, sticks, devlink_up, wifi_lacks_firmware }
}

// ------------------------------------------------------------------------------------------------ what the card shows

/// Everything the Setup card shows before the click.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HwView {
    /// One line per vendor group and licence, then one per file reason (the card's Details).
    pub lines: Vec<String>,
    /// "3 files for Intel Wi-Fi, MediaTek ..., about 4.2 MB"
    pub summary: String,
    /// (vendor group, licence class label) for the form.
    pub vendors: Vec<(String, String)>,
    pub files: usize,
    pub bytes: u64,
    pub has_nvidia: bool,
    pub offers: Vec<net::Offer>,
    /// The staging folder of the developer-mode link when it holds firmware files.
    pub staging: Option<PathBuf>,
    /// A stick or folder that looks like firmware.
    pub stick: Option<PathBuf>,
    /// The cable interface that could share the other computer's internet.
    pub link_iface: Option<String>,
}

/// The plan for this computer (kernel log, requests kept earlier, devices).
pub fn plan_here(data: &Path) -> Plan {
    fwextra::plan_now(&fwextra::RealEnv::new(data, streamer()), data)
}

/// Read the kernel log, the devices and the network (blocks a few seconds: call it from a worker).
pub fn view_now(data: &Path, home: &Path) -> HwView {
    let env = fwextra::RealEnv::new(data, streamer());
    let m = manifest();
    fwextra::remember(data, &fwextra::parse_log(&env.kernel_log(), m));
    let plan = fwextra::plan_now(&env, data);
    let sel = plan.select(&[]);
    let bytes = sel.bytes(m);
    let vendors: Vec<(String, String)> = plan.groups.iter().map(|g| (g.title.clone(), g.class().label().to_string())).collect();
    let summary = if plan.is_empty() {
        "No missing firmware file was found in the kernel log: nothing to install right now.".to_string()
    } else {
        format!("{} file(s) for {}, about {} KB with the licence texts.", plan.groups.iter().map(|g| g.files.len()).sum::<usize>(), plan.groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>().join(", "), bytes.max(plan.groups.iter().map(|g| g.bytes()).sum::<u64>()) >> 10)
    };
    let situation = situation_now(Path::new("/sys"));
    let staging = fwextra::staging_dir(home);
    let has_staging = std::fs::read_dir(&staging).map(|d| d.flatten().next().is_some()).unwrap_or(false);
    let link_iface = situation.ifaces.iter().find(|i| i.kind == Kind::Link && i.carrier).map(|i| i.name.clone());
    let stick = situation.sticks.first().cloned();
    HwView { lines: plan.lines(), summary, vendors, files: plan.groups.iter().map(|g| g.files.len()).sum(), bytes, has_nvidia: plan.groups.iter().any(|g| g.separate), offers: net::offers(&situation), staging: has_staging.then_some(staging), stick, link_iface }
}

// ------------------------------------------------------------------------------------------------ fetch to a folder (the other computer)

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FetchSummary {
    pub files: usize,
    pub bytes: u64,
    pub folder: PathBuf,
}

/// Download the files of `plan` (the non-separate groups, plus `with`), licence texts and WHENCE included, into `<out>/firmware-extra/`
/// in the layout the device reads, each checked against the manifest. `fetch` gets the address, the size and the destination.
pub fn fetch_to_folder(fetch: &mut dyn FnMut(&str, u64, &Path) -> Result<(), String>, m: &Manifest, plan: &Plan, with: &[String], out: &Path) -> Result<FetchSummary, String> {
    let sel = plan.select(with);
    if sel.files.is_empty() {
        return Err("nothing to fetch: no missing firmware file was named (or only a separate group, which needs --with)".into());
    }
    let root = out.join("firmware-extra");
    let mut s = FetchSummary { folder: root.clone(), ..Default::default() };
    let all: Vec<(String, (u64, String))> = sel.docs(m).into_iter().chain(sel.unique()).collect();
    for (p, (size, sha)) in &all {
        let dest = root.join(p);
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        fetch(&m.url(p), *size, &dest)?;
        let bytes = std::fs::read(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        if bytes.len() as u64 != *size || iyakki_drivers::pack::sha256_hex(&bytes) != *sha {
            let _ = std::fs::remove_file(&dest);
            return Err(format!("{p} does not match the pinned size and sha256; it was deleted"));
        }
        s.files += 1;
        s.bytes += size;
    }
    let index = serde_json::json!({ "tag": m.tag, "commit": m.commit, "files": all.iter().map(|(p, _)| p).collect::<Vec<_>>(), "note": "made by iyakki firmware fetch; the device checks every file again" });
    std::fs::write(root.join("firmware-extra.json"), serde_json::to_string_pretty(&index).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(s)
}

/// A plan for names given by hand.
pub fn plan_for_names(names: &[String], m: &Manifest) -> Plan {
    let reqs: Vec<Request> = names.iter().map(|n| Request { name: n.clone(), driver: String::new(), device: String::new() }).collect();
    fwextra::build_plan(&reqs, &[], m)
}

// ------------------------------------------------------------------------------------------------ iyakki quick-install fw-hardware

fn streamer() -> std::sync::Arc<dyn iyakki_proxy::Streamer> {
    std::sync::Arc::new(iyakki_proxy::https::UreqStreamer::default())
}

pub fn hardware_main(args: &[String], data: &Path, user: &str) -> i32 {
    let mut env = fwextra::RealEnv::new(data, streamer());
    if has(args, "--remove") {
        return match fwextra::remove(data) {
            Ok(r) => {
                for l in r.lines {
                    crate::qprintln!("{l}");
                }
                crate::qprintln!("Hardware firmware removed (the downloaded pieces and the driver pack). Restart for drivers that already loaded it.");
                crate::qprintln!("audit: firmware.remove");
                0
            }
            Err(e) => {
                crate::qprintln!("error: {e}");
                1
            }
        };
    }
    if has(args, "--delete-download") {
        crate::qprintln!("deleted {} bytes of kept download pieces", fwextra::delete_leftovers(data));
        return 0;
    }
    let with = flags(args, "--with");
    let m = manifest();
    let plan = fwextra::plan_now(&env, data);
    let log_reqs = fwextra::parse_log(&env.kernel_log(), m);
    fwextra::remember(data, &log_reqs);
    if has(args, "--list") || has(args, "--accept-licence") {
        crate::qprintln!("Hardware firmware for this computer, linux-firmware {} (commit {}):", m.tag, &m.commit[..10]);
        for l in plan.lines() {
            crate::qprintln!("{l}");
        }
        let sel = plan.select(&with);
        if sel.files.is_empty() {
            crate::qprintln!("No missing firmware file was found in the kernel log.");
        } else {
            crate::qprintln!("Total {} KB with the licence texts.", sel.bytes(m) >> 10);
        }
        if has(args, "--list") {
            let situation = situation_now(Path::new("/sys"));
            crate::qprintln!("Ways to get the files:");
            for o in net::offers(&situation) {
                crate::qprintln!("  [{}] {}", if o.ready { "ready" } else if o.needs_click { "on your click" } else { "not now" }, o.line);
            }
            return 0;
        }
        if let Err(e) = fwextra::accept(data, &sel, &format!("cli:{user}"), crate::clock::now_unix()) {
            crate::qprintln!("error: {e}");
            return 1;
        }
        crate::qprintln!("licences of this list accepted by cli:{user}");
        crate::qprintln!("audit: firmware.accept list={}", &sel.digest(m)[..16]);
    }
    let source = match flag(args, "--from") {
        Some(f) => Source::Folder(PathBuf::from(f)),
        None => Source::Download,
    };
    let opts = Opts { with };
    let mut net_env = RealNet;
    let session = match flag(args, "--link") {
        Some(iface) if matches!(source, Source::Download) => {
            if !net::valid_iface(&iface) {
                crate::qprintln!("error: {iface} is not an interface name");
                return 2;
            }
            match net::start_session(&mut net_env, &iface, &["git.kernel.org"], has(args, "--link-everything")) {
                Ok((s, log)) => {
                    for l in log {
                        crate::qprintln!("{l}");
                    }
                    Some(s)
                }
                Err(e) => {
                    crate::qprintln!("error: {e}");
                    return 1;
                }
            }
        }
        _ => None,
    };
    let r = fwextra::run(&mut env, data, &source, &opts);
    if let Some(s) = session {
        let _ = net::stop_session(&mut net_env, &s);
        crate::qprintln!("the other computer's internet is no longer used by this device");
    }
    match r {
        Ok(_) => 0,
        Err(e) => {
            crate::qprintln!("error: {e}");
            1
        }
    }
}

// ------------------------------------------------------------------------------------------------ iyakki firmware ...

pub const FIRMWARE_USAGE: &str = "usage: iyakki firmware list [--log FILE] | fetch (--for DEVICE [--usb-link] | --names A,B | --log FILE) [--out DIR] [--with nvidia]";

pub fn firmware_main(args: &[String]) -> i32 {
    let m = manifest();
    let Some(sub) = args.first().map(String::as_str) else {
        crate::qprintln!("{FIRMWARE_USAGE}");
        return 2;
    };
    match sub {
        "list" => {
            let env = fwextra::RealEnv::new(Path::new("/var/lib/iyakki"), streamer());
            let text = match flag(args, "--log") {
                Some(f) => std::fs::read_to_string(&f).unwrap_or_default(),
                None => env.kernel_log(),
            };
            let plan = fwextra::build_plan(&fwextra::parse_log(&text, m), &fwextra::read_devs(Path::new("/sys")), m);
            for l in plan.lines() {
                crate::qprintln!("{l}");
            }
            if plan.is_empty() {
                crate::qprintln!("No missing firmware file was found.");
            }
            0
        }
        "fetch" => fetch_main(args, m),
        _ => {
            crate::qprintln!("{FIRMWARE_USAGE}");
            2
        }
    }
}

fn fetch_main(args: &[String], m: &Manifest) -> i32 {
    let usb = has(args, "--usb-link");
    let device = flag(args, "--for");
    let with = flags(args, "--with");
    let plan = if let Some(n) = flag(args, "--names") {
        plan_for_names(&n.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>(), m)
    } else if let Some(f) = flag(args, "--log") {
        fwextra::build_plan(&fwextra::parse_log(&std::fs::read_to_string(&f).unwrap_or_default(), m), &[], m)
    } else if let Some(d) = &device {
        // ask the device for its kernel log over the developer-mode link
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("iyakki"));
        match std::process::Command::new(exe).args(["dev", "dmesg", "--device", d]).output() {
            Ok(o) if o.status.success() => fwextra::build_plan(&fwextra::parse_log(&String::from_utf8_lossy(&o.stdout), m), &[], m),
            Ok(o) => {
                crate::qprintln!("error: could not read the kernel log of {d}: {}", String::from_utf8_lossy(&o.stderr).trim());
                return 1;
            }
            Err(e) => {
                crate::qprintln!("error: {e}");
                return 1;
            }
        }
    } else {
        crate::qprintln!("{FIRMWARE_USAGE}");
        return 2;
    };
    for l in plan.lines() {
        crate::qprintln!("{l}");
    }
    let out = PathBuf::from(flag(args, "--out").unwrap_or_else(|| "firmware-fetch".into()));
    let st = streamer();
    let mut fetch = |url: &str, want: u64, dest: &Path| crate::fwpack::fetch_resume(&*st, url, want, dest, 5, Duration::from_secs(3), &mut |l| crate::qprintln!("{l}"), &|| false);
    let s = match fetch_to_folder(&mut fetch, m, &plan, &with, &out) {
        Ok(s) => s,
        Err(e) => {
            crate::qprintln!("error: {e}");
            return 1;
        }
    };
    crate::qprintln!("Fetched and checked {} file(s), {} KB, into {}", s.files, s.bytes >> 10, s.folder.display());
    if usb {
        let Some(d) = device else {
            crate::qprintln!("error: --usb-link needs --for DEVICE (a device paired with `iyakki dev pair`)");
            return 2;
        };
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("iyakki"));
        let st = std::process::Command::new(exe).args(["dev", "files", "push"]).arg(&s.folder).arg("~/firmware-extra").args(["--device", &d]).status();
        match st {
            Ok(x) if x.success() => crate::qprintln!("Sent. On the device open Store, Setup, Hardware firmware: it checks every file again, shows the licences, and installs only after you click there."),
            _ => {
                crate::qprintln!("error: the push did not finish (iyakki dev usb check shows the link); the folder {} is still here", s.folder.display());
                return 1;
            }
        }
    } else {
        crate::qprintln!("Copy the folder firmware-extra to a USB stick and choose it on the device (Store, Setup, Hardware firmware, From a USB stick).");
    }
    0
}

// ------------------------------------------------------------------------------------------------ iyakki usb share-internet

pub const SHARE_USAGE: &str = "usage: iyakki usb share-internet [--device NAME] [--uplink IFACE] [--apply | --undo]";

fn prev_forward_file(link: &str) -> PathBuf {
    PathBuf::from("/run/iyakki-share").join(format!("{link}.prev"))
}

fn root() -> bool {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

pub fn share_main(args: &[String]) -> i32 {
    let ifaces = read_ifaces_real();
    let route = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let link = match net::pick_link(&ifaces, flag(args, "--device").as_deref()) {
        Ok(l) => l,
        Err(e) => {
            crate::qprintln!("error: {e}");
            return 2;
        }
    };
    let undo = has(args, "--undo");
    let uplink = match net::pick_uplink(&net::gateways_from_route(&route), &link, flag(args, "--uplink").as_deref()) {
        Ok(u) => u,
        Err(e) if undo => {
            let _ = e;
            "unknown".to_string()
        }
        Err(e) => {
            crate::qprintln!("error: {e}");
            return 2;
        }
    };
    let share = net::Share { link: link.clone(), uplink };
    let prev = std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").map(|s| s.trim().to_string()).unwrap_or_else(|_| "0".into());
    let cmds = if undo {
        let p = std::fs::read_to_string(prev_forward_file(&link)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "0".into());
        net::unshare_commands(&share, &p)
    } else {
        net::share_commands(&share)
    };
    let cmds = match cmds {
        Ok(c) => c,
        Err(e) => {
            crate::qprintln!("error: {e}");
            return 2;
        }
    };
    crate::qprintln!("{} the internet of this computer ({}) with the cable {} (the device will be {}, this computer {}):", if undo { "Stop sharing" } else { "Share" }, share.uplink, link, net::SHARE_DEVICE, net::SHARE_HOST);
    for c in &cmds {
        crate::qprintln!("  {c}");
    }
    if !undo {
        for l in net::dns_hints(&share) {
            crate::qprintln!("  {l}");
        }
        crate::qprintln!("This is for Linux. macOS and Windows: {}", net::OTHER_HOSTS);
    }
    if !has(args, "--apply") && !undo {
        crate::qprintln!("Nothing was changed. Run the lines above as root yourself, or run this command again with --apply as root (it never calls sudo).");
        return 0;
    }
    if !root() {
        crate::qprintln!("error: this needs root. Run: sudo iyakki usb share-internet{} (nothing was changed)", if undo { " --undo" } else { " --apply" });
        return 1;
    }
    if !undo {
        let f = prev_forward_file(&link);
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if !f.exists() {
            let _ = std::fs::write(&f, &prev);
        }
    }
    // the typed firewall writer, the interface ioctls and the forwarding switch: no shell, no `ip`, no `nft` (the lines printed above are for a person doing it by hand)
    let r = if undo { crate::sharing::real::legacy_undo(&link, &std::fs::read_to_string(prev_forward_file(&link)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "0".into())) } else { crate::sharing::real::legacy_apply(&link, &share.uplink) };
    match r {
        Ok(()) => {
            if undo {
                let _ = std::fs::remove_file(prev_forward_file(&link));
            }
            crate::qprintln!("{}", if undo { "Sharing stopped." } else { "Sharing is on. On the device, open Store, Setup, Hardware firmware and choose: use the other computer's internet. Stop with: iyakki usb share-internet --undo" });
            0
        }
        Err(e) => {
            crate::qprintln!("error: {e}. Run iyakki usb share-internet --undo to remove what was added.");
            1
        }
    }
}

/// Used by the tests: the lines the other computer would print, without touching anything.
pub fn share_preview(link: &str, uplink: &str) -> Result<Vec<String>, String> {
    net::share_commands(&net::Share { link: link.into(), uplink: uplink.into() })
}

#[cfg(test)]
#[path = "fwextra_cli_tests.rs"]
mod tests;
