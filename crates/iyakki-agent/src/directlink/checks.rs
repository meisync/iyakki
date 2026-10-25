//! The seven-step check list of Settings -> Developer -> Connect over USB-C and `iyakki dev usb`. [`run`] reads the machine through
//! [`Env`] and returns the steps with a state and, when a step fails, plain words about what to do. A step is only tried when every step
//! before it passed (the later ones say which step they wait for). Nothing here changes the machine; the buttons (authorize, pair,
//! speed check) are separate functions.

use std::net::Ipv4Addr;

use super::tb::{self, Kind};
use super::{classify, Env, NetIf, Peer, SpeedSample, Transport, DEV_PORT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pass,
    Fail,
    /// Not tried: an earlier step has not passed (or, for the speed check, it has not been asked for yet).
    Wait,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Approve this Thunderbolt device (administrators).
    Authorize(String),
    /// Type the pairing code shown on the other computer.
    Pair,
    /// Run the speed check.
    Speed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub state: State,
    pub text: String,
    pub action: Option<Action>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub checks: Vec<Check>,
    pub link: Option<NetIf>,
    pub transport: Option<Transport>,
    pub peer: Option<Peer>,
    pub at: i64,
    /// Seconds in words: "Last result".
    pub last: String,
}

/// The last speed check, kept by whoever runs the checks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    pub sample: SpeedSample,
    pub rtt_ms: Option<f64>,
}

pub const TITLES: [(&str, &str); 7] = [
    ("port", "1  Cable and port can carry a network"),
    ("peer", "2  The other computer is connected"),
    ("iface", "3  Network interface is up with an address"),
    ("beacon", "4  The other IYAKKI is found"),
    ("devmode", "5  Developer mode answers"),
    ("paired", "6  This computer is paired with it"),
    ("speed", "7  Speed and delay"),
];

const NO_PLAIN_USBC: &str = "A plain USB-C cable between two PCs cannot carry a network: both ports are USB hosts, and a PC's USB controller has no device mode. Use a Thunderbolt 3, Thunderbolt 4 or USB4 cable between two Thunderbolt/USB4 ports (not a charge-only cable), or a host-to-host USB bridge cable (Prolific PL25A1/PL27A1 type) in a USB-A or USB-C port on each side.";

fn find_direct(ifs: &[NetIf], udc: bool) -> Vec<(&NetIf, Transport)> {
    let mut v: Vec<(&NetIf, Transport)> = ifs.iter().filter_map(|i| classify(i, udc).map(|t| (i, t))).collect();
    // an ambiguous USB network interface counts only without a real address
    v.retain(|(i, t)| *t != Transport::UsbNet || !i.ipv4.iter().any(|a| !super::linklocal::is_link_local(*a)));
    v.sort_by_key(|(i, t)| (!(i.carrier && i.up), *t as u8, i.name.clone()));
    v
}

fn rate_words(mbps: Option<u32>) -> String {
    match mbps {
        Some(m) if m >= 1000 => format!("{:.0} Gbit/s", f64::from(m) / 1000.0),
        Some(m) => format!("{m} Mbit/s"),
        None => "speed not reported".into(),
    }
}

/// Run the checks now. `speed` is the last speed result, if any. Peers come from the manager's state, and when it has none a short
/// scan (about a second) is made.
pub fn run(env: &dyn Env, speed: Option<&Speed>) -> Report {
    let at = env.now();
    let ifs = env.net_ifaces();
    let udc = env.udc_present();
    let view = env.tb();
    let direct = find_direct(&ifs, udc);
    let mut checks: Vec<Check> = TITLES.iter().map(|(id, title)| Check { id, title, state: State::Wait, text: String::new(), action: None }).collect();
    let mut stop = false;
    let mut link: Option<NetIf> = None;
    let mut transport: Option<Transport> = None;
    let mut peer: Option<Peer> = None;

    macro_rules! step {
        ($n:expr, $ok:expr, $text:expr) => {{
            let n: usize = $n;
            checks[n].state = if $ok { State::Pass } else { State::Fail };
            checks[n].text = $text;
            if !$ok {
                stop = true;
            }
        }};
    }
    let waits = |n: usize| format!("Waiting for step {n}.");

    // 1 ---- the port
    if view.controller_present {
        let sec = if view.security.is_empty() { String::new() } else { format!(" Security level: {} ({}).", view.security, tb::security_words(&view.security)) };
        if view.driver_loaded {
            step!(0, true, format!("A Thunderbolt/USB4 port is present.{sec} The cable must be a Thunderbolt 3/4 or USB4 cable, not a charge-only one."));
        } else {
            step!(0, false, "A Thunderbolt/USB4 controller is present but its driver is not loaded. In a terminal: modprobe thunderbolt (the kernel needs CONFIG_USB4, and CONFIG_USB4_NET for the network). If that command is not found or fails, this image was built without it.".into());
        }
    } else if let Some((i, t)) = direct.first() {
        step!(0, true, format!("{} found: {} ({}).", t.label(), i.name, if i.driver.is_empty() { "no driver name" } else { &i.driver }));
    } else if udc {
        step!(0, true, "This computer has a USB device controller, so it can act as a USB network card for the other one (iyakki dev usb gadget on). Plug it into a USB port of the other computer with a data cable.".into());
    } else {
        step!(0, false, format!("No Thunderbolt/USB4 port, no host-to-host bridge cable and no USB device controller found. {NO_PLAIN_USBC}"));
    }

    // 2 ---- the other computer
    if stop {
        checks[1].text = waits(1);
    } else if view.controller_present {
        if let Some(d) = view.peer_computer() {
            let speed = match (d.rx_gbps, d.tx_gbps, d.lanes) {
                (Some(r), Some(t), Some(l)) => format!(", link {r:.0}/{t:.0} Gbit/s over {l} lane(s)"),
                (Some(r), _, _) => format!(", link {r:.0} Gbit/s"),
                _ => String::new(),
            };
            let who = if d.model.is_empty() { d.name.clone() } else { format!("{} {}", d.vendor, d.model).trim().to_string() };
            step!(1, true, format!("Connected host to host: {who}{speed}."));
        } else if let Some(d) = view.pending().into_iter().next() {
            let name = d.name.clone();
            step!(1, false, format!("A device is connected but not approved ({} {}): the security level is {} ({}). Press Authorize.", d.vendor, d.model, view.security, tb::security_words(&view.security)));
            checks[1].action = Some(Action::Authorize(name));
        } else if view.devices.iter().any(|d| d.kind == Kind::Router) {
            step!(1, true, "A Thunderbolt device is connected and approved, but it is not a computer joined host to host. Check the cable goes to the other computer, not to a dock or drive.".to_string());
            // approved devices are not a computer: keep going only when a network interface shows up (step 3 decides)
        } else if !direct.is_empty() {
            step!(1, true, "A cable with a link is in the other port.".into());
        } else {
            step!(1, false, "Nothing is connected to the Thunderbolt/USB4 ports. Plug the cable into a Thunderbolt/USB4 port on BOTH computers (on a Mac: the ports with the lightning symbol; some PCs need Thunderbolt switched on in the firmware setup). A USB-C cable that only charges has no high-speed wires and shows nothing here. If you just plugged it in, wait a few seconds and check again.".into());
        }
    } else if let Some((i, _)) = direct.first() {
        if i.carrier {
            step!(1, true, format!("{} has a link: the cable is connected.", i.name));
        } else {
            step!(1, false, format!("{} is there but has no link. Plug the cable into the other computer and make sure that one is switched on and running IYAKKI (a gadget side must have its USB device mode on).", i.name));
        }
    } else {
        step!(1, false, "The other computer is not seen. Plug it in with the cable and check again.".into());
    }

    // 3 ---- interface and address
    if stop {
        checks[2].text = waits(2);
    } else if let Some((i, t)) = direct.iter().find(|(i, _)| i.carrier && i.up && (!i.ipv4.is_empty())) {
        let v6 = i.ipv6_ll.first().map(|a| format!(", {a}")).unwrap_or_default();
        let addrs = i.ipv4.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ");
        step!(2, true, format!("{} is up: {addrs}{v6}; {}.", i.name, rate_words(i.speed_mbps)));
        link = Some((*i).clone());
        transport = Some(*t);
    } else if let Some((i, t)) = direct.first() {
        let why = if !i.up {
            "it is down: the link manager has not switched it on yet (wait a few seconds), or networking is switched off".to_string()
        } else if !i.carrier {
            "the cable has no link yet".to_string()
        } else {
            "it has no address yet: a link-local address is being chosen (this takes about four seconds)".to_string()
        };
        step!(2, false, format!("{} exists but {why}. Check again in a moment.", i.name));
        transport = Some(*t);
    } else if view.peer_computer().is_some() {
        step!(2, false, "The other computer is connected, but no network interface appeared (thunderbolt0 is missing): the kernel module thunderbolt_net is not loaded or not built (CONFIG_USB4_NET). In a terminal: modprobe thunderbolt_net. Then check again.".into());
    } else {
        step!(2, false, "No network interface for the cable. For Thunderbolt: thunderbolt0 appears once the other computer is connected and the thunderbolt_net module is loaded; for a bridge cable: usb0 or enx... appears when its driver (plusb) is loaded.".into());
    }

    // 4 ---- beacon
    if stop {
        checks[3].text = waits(3);
    } else {
        let mut peers = env.snapshot().map(|s| s.peers).unwrap_or_default();
        let iface = link.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        peers.retain(|p| p.iface == iface);
        if peers.is_empty() {
            peers = env.discover(1500).into_iter().filter(|p| p.iface == iface).collect();
        }
        match peers.into_iter().max_by_key(|p| p.last_seen) {
            Some(p) => {
                let b = &p.beacon;
                step!(3, true, format!("Found {} (IYAKKI {}) at {}: developer mode {}{}.", b.host, b.version, p.addr, if b.dev_on { "on" } else { "off" }, if b.pairing_open { ", a pairing code is open" } else { "" }));
                peer = Some(p);
            }
            None => step!(3, false, "No IYAKKI answered on the cable. The other computer must run IYAKKI with its link up; both ends send a small beacon every three seconds on the cable only. If the other computer runs another system, use its address with `iyakki dev pair <address>` instead.".into()),
        }
    }

    // 5 ---- developer mode reachable
    let mut hello = None;
    if stop {
        checks[4].text = waits(4);
    } else if let Some(p) = &peer {
        match env.dev_hello(p.addr) {
            Ok(h) => {
                step!(4, true, format!("Developer mode on {} answers at {}:{DEV_PORT} (certificate {}).", h.name, p.addr, short_fp(&h.fingerprint)));
                hello = Some(h);
            }
            Err(e) if e.contains("developer mode is off") => step!(4, false, "Developer mode is switched off on the other computer. There: Settings, Developer mode, press e.".into()),
            Err(e) => step!(4, false, format!("{}:{DEV_PORT} did not answer ({e}). Developer mode may be off there (Settings, Developer mode, press e), or its firewall blocks the cable: the firewall's direct-link zone lets port {DEV_PORT} through, so both computers need this version.", p.addr)),
        }
    }

    // 6 ---- paired
    if stop {
        checks[5].text = waits(5);
    } else if let (Some(p), Some(h)) = (&peer, &hello) {
        match env.paired_as(p.addr) {
            Some(name) => step!(5, true, format!("Paired: this computer knows {} as \"{name}\".", h.name)),
            None => {
                let todo = if h.pairing_open { "A pairing code is open there: press Pair and type it. Compare the fingerprint with the one on that screen first." } else { "On the other computer press p (Pair a computer) in Settings, Developer mode: it shows a code and the certificate fingerprint. Then press Pair here and type the code." };
                step!(5, false, format!("Not paired yet. {todo} Pairing needs the code, so nobody on the cable can pair without someone at that screen."));
                checks[5].action = Some(Action::Pair);
            }
        }
    }

    // 7 ---- speed
    if stop {
        checks[6].text = waits(6);
    } else {
        match speed {
            Some(s) => {
                let g = s.sample.gbit_per_s();
                let rtt = s.rtt_ms.map(|r| format!(", delay {r:.2} ms")).unwrap_or_default();
                checks[6].state = State::Pass;
                checks[6].text = format!("{:.2} Gbit/s ({} MiB in {:.2} s through the developer-mode link, TLS and signing included){rtt}.", g, s.sample.bytes >> 20, s.sample.secs);
                checks[6].action = Some(Action::Speed);
            }
            None => {
                checks[6].text = "Not measured yet. Press Speed to send 64 MiB to the other computer (it is thrown away there).".into();
                checks[6].action = Some(Action::Speed);
            }
        }
    }

    let passed = checks.iter().filter(|c| c.state == State::Pass).count();
    let last = match checks.iter().find(|c| c.state == State::Fail) {
        Some(c) => format!("{passed} of 7 steps passed. Stuck at: {}", c.title.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == ' ')),
        None if speed.is_some() => "All 7 steps passed.".to_string(),
        None => format!("{passed} of 7 steps passed; the speed check has not been run."),
    };
    Report { checks, link, transport, peer, at, last }
}

fn short_fp(fp: &str) -> String {
    let t: String = fp.chars().take(23).collect();
    if fp.len() > 23 { format!("{t}...") } else { t }
}

impl Report {
    /// Plain text for the "copy diagnostics" button: no secrets, nothing personal beyond host names of the two computers.
    pub fn diagnostic(&self, env: &dyn Env) -> String {
        let me = env.me();
        let mut o = format!("IYAKKI direct link report\nthis computer: {} (IYAKKI {})\nlast result: {}\n\n", me.host, me.version, self.last);
        for c in &self.checks {
            o.push_str(&format!("[{}] {}\n    {}\n", match c.state { State::Pass => "ok", State::Fail => "FAIL", State::Wait => "..." }, c.title, if c.text.is_empty() { "-" } else { &c.text }));
        }
        o.push_str("\ninterfaces:\n");
        for i in env.net_ifaces() {
            o.push_str(&format!("  {} driver={} up={} carrier={} speed={} ipv4={:?} ipv6ll={:?} mac={}\n", i.name, i.driver, i.up, i.carrier, rate_words(i.speed_mbps), i.ipv4, i.ipv6_ll, i.mac));
        }
        let v = env.tb();
        o.push_str(&format!("\nthunderbolt: controller={} driver_loaded={} security={}\n", v.controller_present, v.driver_loaded, v.security));
        for d in &v.devices {
            o.push_str(&format!("  {} {:?} authorized={:?} {} {} rx={:?} tx={:?} lanes={:?}\n", d.name, d.kind, d.authorized, d.vendor, d.model, d.rx_gbps, d.tx_gbps, d.lanes));
        }
        o.push_str(&format!("usb device controller: {}\n", env.udc_present()));
        o
    }
}

/// The address to use for the dev link: the peer found in step 4.
pub fn peer_addr(r: &Report) -> Option<Ipv4Addr> {
    r.peer.as_ref().map(|p| p.addr)
}

/// The "Authorize" button: approve the device the report offers.
pub fn authorize_pending(env: &dyn Env, r: &Report) -> Result<String, String> {
    match r.checks.get(1).and_then(|c| c.action.clone()) {
        Some(Action::Authorize(d)) => tb::authorize(env, &d),
        _ => Err("nothing waits for approval".into()),
    }
}

/// The "Pair" button: runs the same pairing as `iyakki dev pair` (the code, the certificate checked, the proof both ways). Nothing is
/// weakened: without the code shown on the other computer this fails.
pub fn pair(env: &dyn Env, r: &Report, code: &str) -> Result<String, String> {
    let addr = peer_addr(r).ok_or("no other computer found yet (step 4)")?;
    let h = env.dev_hello(addr)?;
    if !h.pairing_open {
        return Err("no pairing code is open on the other computer: press p there first".into());
    }
    let code = code.trim();
    if code.is_empty() {
        return Err("type the code shown on the other computer".into());
    }
    env.pair(addr, code, &h.fingerprint)
}

/// The "Speed" button: one bounded run (64 MiB) and the delay of a TCP connect to the developer-mode port.
pub fn speed_test(env: &dyn Env, r: &Report) -> Result<Speed, String> {
    let addr = peer_addr(r).ok_or("no other computer found yet (step 4)")?;
    if env.paired_as(addr).is_none() {
        return Err("pair first (step 6): the speed check goes through the developer-mode link".into());
    }
    let sample = env.speed(addr)?;
    if sample.bytes == 0 || sample.secs <= 0.0 {
        return Err("the speed check moved no data".into());
    }
    Ok(Speed { sample, rtt_ms: env.tcp_rtt_ms(addr, DEV_PORT).ok() })
}
