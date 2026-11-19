//! How the hardware-firmware item (`fwextra.rs`) reaches its files, and what the machine can do about a missing network.
//!
//! A Wi-Fi chip without its firmware cannot bring the network that would download that firmware. The item therefore looks for ways,
//! in this order, and shows every one of them on its card with a plain line (nothing here changes anything by itself):
//!
//! 1. **Ethernet** with an address and a route to the internet: the normal download.
//! 2. **A cable to another computer that shares its internet** (USB-C to USB-C, Thunderbolt / USB4, a USB network link): after the
//!    person's click on this device, a route to the allowed hosts of the job only (not a default route, unless the person chooses
//!    "use for everything") and, where the system needs it, a hosts-file entry; removed on unplug, on restart and on Stop.
//! 3. **A USB stick** with a prepared folder (`fwextra::run` with a folder source).
//! 4. **The other computer pushes the files** over the developer-mode link (`iyakki firmware fetch --for DEVICE --usb-link` on that
//!    computer; `iyakki dev files push` into the staging folder); this device verifies, lists the licences and installs only
//!    after the click on its own screen.
//!
//! The other computer's side of 2 is `iyakki usb share-internet` ([`share_commands`]): it prints the exact commands and runs them only
//! with `--apply` (root, never `sudo` by itself) and undoes them with `--undo`. Everything here that decides or builds a command is a
//! pure function; the tests use fake interface tables and a fake runner, and nothing touches the real system.

use std::net::Ipv4Addr;
use std::path::PathBuf;

// ------------------------------------------------------------------------------------------------ interfaces

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A wired network card (eth0, enp3s0, eno1).
    Ethernet,
    Wifi,
    /// A cable to another computer: thunderbolt0, usb0, or a USB network class adapter (enx...) with a USB network driver.
    Link,
    Other,
}

/// One network interface as the offers need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub kind: Kind,
    /// The cable is plugged in and the other end answers at layer 2 (`carrier` 1).
    pub carrier: bool,
    pub ipv4: Vec<Ipv4Addr>,
    /// The default gateway of the interface from the routing table, when it has one.
    pub gateway: Option<Ipv4Addr>,
}

/// USB network drivers that a cable to a computer, or a phone's USB tethering, shows up with.
pub const USBNET_DRIVERS: [&str; 7] = ["cdc_ether", "cdc_ncm", "cdc_eem", "rndis_host", "asix", "ax88179_178a", "r8152"];

/// What an interface is from its name, its driver, and whether it has a `wireless` folder in sysfs.
pub fn classify(name: &str, driver: &str, wireless: bool) -> Kind {
    let d = driver.replace('-', "_");
    if name == "lo" || name.starts_with("docker") || name.starts_with("veth") || name.starts_with("br-") || name.starts_with("cni") || name.starts_with("flannel") || name.starts_with("tun") || name.starts_with("tap") || name.starts_with("virbr") {
        Kind::Other
    } else if wireless || name.starts_with("wl") {
        Kind::Wifi
    } else if name.starts_with("thunderbolt") || name.starts_with("usb") || (name.starts_with("enx") && USBNET_DRIVERS.contains(&d.as_str())) || (d == "thunderbolt_net") {
        Kind::Link
    } else if name.starts_with("eth") || name.starts_with("en") {
        Kind::Ethernet
    } else {
        Kind::Other
    }
}

/// The default gateways per interface from `/proc/net/route` text (hex little-endian addresses).
pub fn gateways_from_route(text: &str) -> Vec<(String, Ipv4Addr)> {
    let mut v = Vec::new();
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 8 || f[1] != "00000000" || f[7] != "00000000" {
            continue;
        }
        let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
        if flags & 2 == 0 {
            continue; // not a gateway route
        }
        if let Ok(g) = u32::from_str_radix(f[2], 16) {
            v.push((f[0].to_string(), Ipv4Addr::from(g.swap_bytes())));
        }
    }
    v
}

/// The interface the kernel would use to reach `ip`, from `/proc/net/route` text: the longest matching prefix, then the lowest metric.
pub fn route_iface_for(text: &str, ip: Ipv4Addr) -> Option<String> {
    let target = u32::from(ip);
    let mut best: Option<(u32, u32, String)> = None;
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let (Ok(dest), Ok(mask), Ok(flags), Ok(metric)) = (u32::from_str_radix(f[1], 16), u32::from_str_radix(f[7], 16), u32::from_str_radix(f[3], 16), f[6].parse::<u32>()) else { continue };
        if flags & 1 == 0 {
            continue; // route is down
        }
        let (dest, mask) = (dest.swap_bytes(), mask.swap_bytes());
        if target & mask != dest {
            continue;
        }
        let better = match &best {
            None => true,
            Some((m, me, _)) => mask.count_ones() > m.count_ones() || (mask == *m && metric < *me),
        };
        if better {
            best = Some((mask, metric, f[0].to_string()));
        }
    }
    best.map(|b| b.2)
}

// ------------------------------------------------------------------------------------------------ the offers

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Ethernet,
    /// The cable to another computer that shares its internet.
    LinkInternet,
    Stick,
    /// The other computer pushes the files over the developer-mode link.
    Push,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub way: Way,
    /// Usable right now (Ethernet reaches the host; the stick is there; the developer-mode link is up). A link that shares
    /// internet is `ready` only after the person clicked and the route test passed; before that it is `needs_click`.
    pub ready: bool,
    pub needs_click: bool,
    /// One plain line for the card.
    pub line: String,
    pub iface: Option<String>,
}

/// What the machine looks like to the offers.
#[derive(Clone, Debug, Default)]
pub struct Situation {
    pub ifaces: Vec<Iface>,
    /// Interfaces whose route to the pinned host was tested and works (`NetEnv::reaches_host`).
    pub internet_on: Vec<String>,
    /// Folders on removable media that look like a firmware stick (`find_sticks`).
    pub sticks: Vec<PathBuf>,
    /// The developer-mode link to another computer is up (a paired computer was seen on a direct link).
    pub devlink_up: bool,
    /// A Wi-Fi interface is missing or has no firmware, so Wi-Fi cannot be the way.
    pub wifi_lacks_firmware: bool,
}

/// The four ways in the order of detection. Every way is listed (so the card can say why it is not available); `recommended` picks
/// the first one that is ready, or that only needs the person's click.
pub fn offers(s: &Situation) -> Vec<Offer> {
    let on = |n: &str| s.internet_on.iter().any(|x| x == n);
    let eth = s.ifaces.iter().find(|i| i.kind == Kind::Ethernet && i.carrier && !i.ipv4.is_empty() && on(&i.name));
    let eth_cable = s.ifaces.iter().find(|i| i.kind == Kind::Ethernet && i.carrier);
    let mut v = Vec::new();
    v.push(match (eth, eth_cable) {
        (Some(i), _) => Offer { way: Way::Ethernet, ready: true, needs_click: false, line: format!("Ethernet ({}) reaches the internet: download the files now.", i.name), iface: Some(i.name.clone()) },
        (None, Some(i)) => Offer { way: Way::Ethernet, ready: false, needs_click: false, line: format!("Ethernet ({}) is plugged in but has no internet yet: check the router, then Check again.", i.name), iface: Some(i.name.clone()) },
        (None, None) => Offer { way: Way::Ethernet, ready: false, needs_click: false, line: if s.wifi_lacks_firmware { "Plug in an Ethernet cable to your router: Wi-Fi is the only network here and it cannot work without this firmware.".into() } else { "Plug in an Ethernet cable to your router to download the files.".into() }, iface: None },
    });
    let link = s.ifaces.iter().find(|i| i.kind == Kind::Link && i.carrier);
    v.push(match link {
        Some(i) if on(&i.name) => Offer { way: Way::LinkInternet, ready: true, needs_click: false, line: format!("The cable on {} reaches the internet through the other computer: download the files now.", i.name), iface: Some(i.name.clone()) },
        Some(i) => Offer { way: Way::LinkInternet, ready: false, needs_click: true, line: format!("Use the other computer's internet through the cable ({}), this session only. On that computer run: iyakki usb share-internet --apply", i.name), iface: Some(i.name.clone()) },
        None => Offer { way: Way::LinkInternet, ready: false, needs_click: false, line: "Connect a USB-C to USB-C (or Thunderbolt) cable to another computer that shares its internet to use it here.".into(), iface: None },
    });
    v.push(match s.sticks.first() {
        Some(p) => Offer { way: Way::Stick, ready: true, needs_click: false, line: format!("A USB stick with firmware files is inserted ({}): copy from it.", p.display()), iface: None },
        None => Offer { way: Way::Stick, ready: false, needs_click: false, line: "Insert a USB stick with a firmware-extra folder (made on another computer by dist/scripts/firmware-notes/fetch.sh) or pick a folder yourself.".into(), iface: None },
    });
    v.push(if s.devlink_up {
        Offer { way: Way::Push, ready: true, needs_click: false, line: "The developer-mode link to another computer is up: it can fetch the files and send them here (iyakki firmware fetch --usb-link). You still approve the install on this screen.".into(), iface: None }
    } else {
        Offer { way: Way::Push, ready: false, needs_click: false, line: "Pair another computer over the cable (iyakki dev usb) so it can fetch the files and send them here.".into(), iface: None }
    });
    v
}

/// The way to start with: the first that is ready, else the first that only needs a click.
pub fn recommended(o: &[Offer]) -> Option<&Offer> {
    o.iter().find(|x| x.ready).or_else(|| o.iter().find(|x| x.needs_click))
}

/// Folders on removable media that hold firmware files: a `firmware-extra` folder, or an unpacked `linux-firmware*` folder, at the top
/// of a mounted volume. `mounts` are the mount points to look under (the caller reads `/media`, `/run/media`, `/mnt`).
pub fn find_sticks(mounts: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for m in mounts {
        for e in std::fs::read_dir(m).into_iter().flatten().flatten() {
            let p = e.path();
            for cand in [p.clone(), p.join("firmware-extra")] {
                if cand.join("firmware-extra.json").is_file() || cand.file_name().is_some_and(|n| n == "firmware-extra") && cand.is_dir() {
                    v.push(cand);
                }
            }
            for e2 in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                let n = e2.file_name().to_string_lossy().into_owned();
                if e2.path().is_dir() && n.starts_with("linux-firmware") {
                    v.push(e2.path());
                }
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

// ------------------------------------------------------------------------------------------------ this device: use the cable's internet

/// The fixed pair of addresses of a shared-internet cable: the other computer is `.1`, this device `.2` (the same private pair the
/// direct link uses, `directlink::linklocal::private_pair`).
pub const SHARE_HOST: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 1);
pub const SHARE_DEVICE: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 2);
pub const SHARE_PREFIX: u8 = 30;
/// Resolvers asked through the cable when the other computer does not forward DNS (routed as single hosts, only for the session).
pub const FALLBACK_DNS: [Ipv4Addr; 2] = [Ipv4Addr::new(9, 9, 9, 9), Ipv4Addr::new(1, 1, 1, 1)];
/// Marker on the hosts-file lines this session adds (so removal touches nothing else).
pub const HOSTS_MARK: &str = "# iyakki-firmware-session";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub iface: String,
    pub gateway: Ipv4Addr,
    /// Resolved addresses of the allowed hosts: name, addresses.
    pub hosts: Vec<(String, Vec<Ipv4Addr>)>,
    /// The person chose "use for everything": also a default route, at a low priority.
    pub everything: bool,
}

/// `ip` commands that start the session (address alias, a route per allowed host address). No default route unless `everything`.
pub fn session_commands(s: &Session) -> Vec<Vec<String>> {
    let c = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut v = vec![c(&["ip", "addr", "add", &format!("{SHARE_DEVICE}/{SHARE_PREFIX}"), "dev", &s.iface, "label", &format!("{}:fw", s.iface)])];
    let mut seen: Vec<Ipv4Addr> = Vec::new();
    for ip in FALLBACK_DNS.iter().chain(s.hosts.iter().flat_map(|(_, ips)| ips.iter())) {
        if !seen.contains(ip) {
            seen.push(*ip);
            v.push(c(&["ip", "route", "add", &format!("{ip}/32"), "via", &s.gateway.to_string(), "dev", &s.iface, "metric", "50"]));
        }
    }
    if s.everything {
        v.push(c(&["ip", "route", "add", "default", "via", &s.gateway.to_string(), "dev", &s.iface, "metric", "700"]));
    }
    v
}

/// The commands that take the session away again (unplug, restart, Stop). Deleting what is not there is harmless, so this is the same
/// list in reverse whatever happened.
pub fn session_undo(s: &Session) -> Vec<Vec<String>> {
    let mut v: Vec<Vec<String>> = session_commands(s)
        .into_iter()
        .rev()
        .map(|mut c| {
            if c.len() > 2 {
                c[2] = "del".into();
            }
            // `ip addr del` takes no label, `ip route del` does not need the metric
            if c[1] == "addr" {
                c.truncate(6);
            }
            c
        })
        .collect();
    v.retain(|c| !c.is_empty());
    v
}

/// Hosts-file lines for the allowed hosts (the first address of each).
pub fn hosts_lines(hosts: &[(String, Vec<Ipv4Addr>)]) -> String {
    hosts.iter().filter_map(|(n, ips)| ips.first().map(|ip| format!("{ip} {n} {HOSTS_MARK}\n"))).collect()
}

/// A hosts file without the session's lines.
pub fn hosts_without_session(text: &str) -> String {
    text.lines().filter(|l| !l.contains(HOSTS_MARK)).map(|l| format!("{l}\n")).collect()
}

// ---- a minimal DNS A query (so no resolver has to exist on this device while the session starts)

pub fn dns_query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut q = vec![(id >> 8) as u8, id as u8, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]);
    Some(q)
}

/// The IPv4 addresses in a DNS answer for the query with this id. A packet that is short, has another id, or is not a response gives none.
pub fn dns_parse_a(id: u16, p: &[u8]) -> Vec<Ipv4Addr> {
    if p.len() < 12 || p[0] != (id >> 8) as u8 || p[1] != id as u8 || p[2] & 0x80 == 0 || p[3] & 0x0f != 0 {
        return Vec::new();
    }
    let (qd, an) = (u16::from_be_bytes([p[4], p[5]]) as usize, u16::from_be_bytes([p[6], p[7]]) as usize);
    let mut i = 12;
    let skip_name = |mut i: usize| -> Option<usize> {
        loop {
            let l = *p.get(i)? as usize;
            if l == 0 {
                return Some(i + 1);
            }
            if l & 0xc0 == 0xc0 {
                return Some(i + 2);
            }
            i += 1 + l;
        }
    };
    for _ in 0..qd {
        let Some(j) = skip_name(i) else { return Vec::new() };
        i = j + 4;
    }
    let mut out = Vec::new();
    for _ in 0..an {
        let Some(j) = skip_name(i) else { break };
        if j + 10 > p.len() {
            break;
        }
        let (ty, len) = (u16::from_be_bytes([p[j], p[j + 1]]), u16::from_be_bytes([p[j + 8], p[j + 9]]) as usize);
        let d = j + 10;
        if d + len > p.len() {
            break;
        }
        if ty == 1 && len == 4 {
            out.push(Ipv4Addr::new(p[d], p[d + 1], p[d + 2], p[d + 3]));
        }
        i = d + len;
    }
    out
}

/// What the session does to the world. The real one runs `ip`, reads and writes the hosts file and asks a DNS server over UDP.
pub trait NetEnv {
    fn run(&mut self, argv: &[String]) -> Result<(), String>;
    fn resolve(&mut self, server: Ipv4Addr, name: &str) -> Vec<Ipv4Addr>;
    fn read_hosts(&self) -> String;
    fn write_hosts(&mut self, text: &str) -> Result<(), String>;
    /// Can this host be reached on port 443 (the route test; with the session's routes in place)?
    fn reaches(&mut self, host: &str) -> bool;
    fn audit(&mut self, line: &str);
}

/// Start the session: resolve the allowed hosts through the cable, add the routes, note the hosts. Returns the lines for the log.
/// Stops at the first failure, undoes what it did, and says what to do.
pub fn start_session(env: &mut dyn NetEnv, iface: &str, hosts: &[&str], everything: bool) -> Result<(Session, Vec<String>), String> {
    let gateway = SHARE_HOST;
    let mut log = vec![format!("using the other computer's internet through {iface} (this session only): address {SHARE_DEVICE}/{SHARE_PREFIX}, gateway {gateway}, hosts {}{}", hosts.join(", "), if everything { ", and a default route for everything" } else { ", no default route" })];
    let mut s = Session { iface: iface.to_string(), gateway, hosts: Vec::new(), everything: false };
    // first the address and the routes to the resolvers, then the names
    let first = session_commands(&s);
    for c in &first {
        if let Err(e) = env.run(c) {
            let _ = stop_session(env, &s);
            return Err(format!("{}: {e}. On the other computer: iyakki usb share-internet --apply", c.join(" ")));
        }
    }
    for h in hosts {
        let mut ips = Vec::new();
        for server in std::iter::once(gateway).chain(FALLBACK_DNS) {
            ips = env.resolve(server, h);
            if !ips.is_empty() {
                break;
            }
        }
        if ips.is_empty() {
            let _ = stop_session(env, &s);
            return Err(format!("could not look up {h} through the cable: the other computer has no internet, or it does not share it (run iyakki usb share-internet --apply there)"));
        }
        s.hosts.push((h.to_string(), ips));
    }
    s.everything = everything;
    for c in session_commands(&s).iter().skip(first.len()) {
        if let Err(e) = env.run(c) {
            let _ = stop_session(env, &s);
            return Err(format!("{}: {e}", c.join(" ")));
        }
    }
    let hosts_text = env.read_hosts();
    if let Err(e) = env.write_hosts(&format!("{}{}", hosts_without_session(&hosts_text), hosts_lines(&s.hosts))) {
        log.push(format!("note: the hosts file could not be written ({e}); the system resolver is used"));
    }
    for h in hosts {
        if !env.reaches(h) {
            let _ = stop_session(env, &s);
            return Err(format!("{h} is not reachable through the cable (the other computer does not forward, or its firewall blocks it); the session was removed"));
        }
    }
    for (n, ips) in &s.hosts {
        log.push(format!("{n} -> {}", ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")));
    }
    env.audit(&format!("firmware.link-internet start {iface} hosts={}{}", hosts.join(","), if everything { " everything" } else { "" }));
    Ok((s, log))
}

/// End the session (the same on unplug, restart and Stop).
pub fn stop_session(env: &mut dyn NetEnv, s: &Session) -> Result<(), String> {
    let mut first = None;
    for c in session_undo(s) {
        if let Err(e) = env.run(&c) {
            first.get_or_insert(e);
        }
    }
    let t = env.read_hosts();
    if t.contains(HOSTS_MARK) {
        let _ = env.write_hosts(&hosts_without_session(&t));
    }
    env.audit(&format!("firmware.link-internet stop {}", s.iface));
    match first {
        Some(_) => Ok(()), // deleting something already gone is not a failure
        None => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ the other computer: share its internet

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    /// The interface of the cable.
    pub link: String,
    /// The interface that reaches the internet.
    pub uplink: String,
}

/// The cable interface among the interfaces of the other computer: the only `Link`, or the one named.
pub fn pick_link(ifaces: &[Iface], wanted: Option<&str>) -> Result<String, String> {
    if let Some(w) = wanted {
        return ifaces.iter().find(|i| i.name == w).map(|i| i.name.clone()).ok_or_else(|| format!("there is no interface {w}; the interfaces are: {}", names(ifaces)));
    }
    let links: Vec<&Iface> = ifaces.iter().filter(|i| i.kind == Kind::Link && i.carrier).collect();
    match links.as_slice() {
        [one] => Ok(one.name.clone()),
        [] => Err(format!("no cable link was found (thunderbolt0, usb0, enx...); the interfaces are: {}. Name it with --device NAME", names(ifaces))),
        many => Err(format!("more than one cable link ({}): name one with --device NAME", many.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

fn names(i: &[Iface]) -> String {
    i.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The uplink: the one named, or the interface with the default route that is not the cable.
pub fn pick_uplink(gateways: &[(String, Ipv4Addr)], link: &str, wanted: Option<&str>) -> Result<String, String> {
    if let Some(w) = wanted {
        if w == link {
            return Err("the uplink cannot be the cable itself".into());
        }
        return Ok(w.to_string());
    }
    gateways.iter().find(|(n, _)| n != link).map(|(n, _)| n.clone()).ok_or_else(|| "this computer has no default route to the internet besides the cable; name the uplink with --uplink IFACE".into())
}

/// A name that is safe to put in a command and a nftables rule.
pub fn valid_iface(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// The commands that share the uplink with the cable, for a Linux computer (nftables). The firewall rules name the cable only; the
/// previous value of `ip_forward` is passed in so that [`unshare_commands`] can put it back.
pub fn share_commands(s: &Share) -> Result<Vec<String>, String> {
    if !valid_iface(&s.link) || !valid_iface(&s.uplink) {
        return Err("an interface name is not valid".into());
    }
    let (l, u) = (&s.link, &s.uplink);
    Ok(vec![
        format!("ip addr add {SHARE_HOST}/{SHARE_PREFIX} dev {l}"),
        format!("ip link set {l} up"),
        "sysctl -w net.ipv4.ip_forward=1".into(),
        "nft add table ip iyakki_share".into(),
        "nft add chain ip iyakki_share forward '{ type filter hook forward priority -10 ; policy accept ; }'".into(),
        format!("nft add rule ip iyakki_share forward iifname \"{l}\" oifname \"{u}\" accept"),
        format!("nft add rule ip iyakki_share forward iifname \"{u}\" oifname \"{l}\" ct state established,related accept"),
        "nft add chain ip iyakki_share post '{ type nat hook postrouting priority 100 ; }'".into(),
        format!("nft add rule ip iyakki_share post iifname \"{l}\" oifname \"{u}\" masquerade"),
    ])
}

/// The commands that undo [`share_commands`]; `previous_forward` is what `net.ipv4.ip_forward` was before (`0` or `1`).
pub fn unshare_commands(s: &Share, previous_forward: &str) -> Result<Vec<String>, String> {
    if !valid_iface(&s.link) || !valid_iface(&s.uplink) || !matches!(previous_forward, "0" | "1") {
        return Err("an interface name or the previous forwarding value is not valid".into());
    }
    Ok(vec!["nft delete table ip iyakki_share".into(), format!("sysctl -w net.ipv4.ip_forward={previous_forward}"), format!("ip addr del {SHARE_HOST}/{SHARE_PREFIX} dev {}", s.link)])
}

/// Optional: answers for DNS on the cable's address (printed, never run). Without it the device asks 9.9.9.9 and 1.1.1.1 through the NAT.
pub fn dns_hints(s: &Share) -> Vec<String> {
    vec![
        format!("optional DNS for the device (any one): dnsmasq --no-daemon --interface={} --bind-interfaces --listen-address={SHARE_HOST} --no-resolv --server=9.9.9.9", s.link),
        "(systemd-resolved: DNSStubListenerExtra=10.77.0.1 in resolved.conf; without DNS here the device asks 9.9.9.9 and 1.1.1.1 through the cable)".into(),
    ]
}

/// What a computer that is not Linux has to do by hand (documentation only; this program does not change those systems).
pub const OTHER_HOSTS: &str = "macOS: System Settings > General > Sharing > Internet Sharing: share your connection from Wi-Fi/Ethernet to the Thunderbolt/USB interface of the cable (macOS gives the device an address by DHCP; this device then uses it like an Ethernet link). Windows: Settings > Network & internet > Mobile hotspot is for Wi-Fi only; for a cable, open the properties of the adapter that has the internet, Sharing tab, tick Allow other network users to connect, and choose the cable's adapter (Internet Connection Sharing; it uses 192.168.137.1).";

/// Run a command list through a runner (a fake in the tests). Stops at the first failure.
pub fn apply(run: &mut dyn FnMut(&str) -> Result<(), String>, cmds: &[String]) -> Result<(), String> {
    for c in cmds {
        run(c).map_err(|e| format!("`{c}` failed: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "fwextra_net_tests.rs"]
mod tests;
