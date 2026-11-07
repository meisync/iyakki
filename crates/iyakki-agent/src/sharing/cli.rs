//! `iyakki share <start|stop|status|list|policy> [--uplink X] [--via Y] [--hours N] [--allow-lan] [--no-isolation]`.
//!
//! `start` checks the choice against the machine's links first (the same pure plan the service uses), so a mistake is explained at once,
//! then writes the request file and waits a few seconds for the service to answer. The service runs inside the system's first process;
//! this command never opens a socket or changes a network setting itself.

use super::control::{self, Backend};
use super::plan::{apply_policy, make_plan};
use super::view::choices;
use super::policy::Who;
use super::{human_duration, Request, State, DEFAULT_IDLE_HOURS};

pub const USAGE: &str = "usage: iyakki share <start|stop|status|list> [--uplink IFACE] [--via IFACE] [--hours 1-72] [--allow-lan] [--no-isolation]   (Wi-Fi hotspot: --via iyakap0; iyakki share wifi-info | wifi-set NAME PASSWORD | wifi-caps)";

#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    Start { uplink: Option<String>, via: Option<String>, hours: u8, isolate: bool, allow_lan: bool },
    Stop,
    Status,
    List,
    /// Show (no options) or change the sharing policy.
    Policy(Vec<(String, String)>),
}

fn value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i).filter(|v| !v.starts_with("--")).cloned().ok_or_else(|| format!("{flag} needs a value"))
}

pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let sub = args.first().map(String::as_str).ok_or(USAGE)?;
    if sub == "policy" {
        let mut edits = Vec::new();
        let mut i = 1;
        while i < args.len() {
            let f = args[i].as_str();
            if !matches!(f, "--wired" | "--wifi" | "--bluetooth" | "--max-clients" | "--max-hours" | "--downstreams" | "--uplinks") {
                return Err(format!("unknown option {f}. {USAGE}"));
            }
            let v = value(args, &mut i, f)?;
            match f {
                "--wired" | "--wifi" | "--bluetooth" if Who::parse(&v).is_none() => return Err(format!("{f} must be admins, users or nobody")),
                "--max-clients" | "--max-hours" if v.parse::<u8>().is_err() => return Err(format!("{f} must be a whole number from 0 to 255 (0: no limit)")),
                _ => {}
            }
            edits.push((f.trim_start_matches("--").to_string(), v));
            i += 1;
        }
        return Ok(Cmd::Policy(edits));
    }
    let (mut uplink, mut via, mut hours, mut isolate, mut allow_lan) = (None, None, DEFAULT_IDLE_HOURS, true, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--uplink" => uplink = Some(value(args, &mut i, "--uplink")?),
            "--via" => via = Some(value(args, &mut i, "--via")?),
            "--hours" => {
                let v = value(args, &mut i, "--hours")?;
                hours = v.parse::<u8>().ok().filter(|h| (1..=super::MAX_IDLE_HOURS).contains(h)).ok_or_else(|| format!("--hours must be a whole number from 1 to {}", super::MAX_IDLE_HOURS))?;
            }
            "--allow-lan" => allow_lan = true,
            "--no-isolation" => isolate = false,
            other => return Err(format!("unknown option {other}. {USAGE}")),
        }
        i += 1;
    }
    match sub {
        "start" => Ok(Cmd::Start { uplink, via, hours, isolate, allow_lan }),
        "stop" | "status" | "list" if uplink.is_some() || via.is_some() || allow_lan || !isolate || hours != DEFAULT_IDLE_HOURS => Err(format!("`{sub}` takes no options. {USAGE}")),
        "stop" => Ok(Cmd::Stop),
        "status" => Ok(Cmd::Status),
        "list" => Ok(Cmd::List),
        other => Err(format!("unknown command {other}. {USAGE}")),
    }
}

/// The text of `status`.
pub fn status_text(s: &super::Status) -> Vec<String> {
    let mut v = vec![format!("state: {}", s.state.word()), s.line()];
    if s.state == State::On {
        v.push(format!("uplink: {}   downstream: {}   network: {}   this computer: {}", s.uplink, s.downstream, s.subnet, s.gateway));
        v.push(format!("isolation: {}   switch-off after {} h idle (in {})", if s.isolate { "on" } else { "off" }, s.idle_hours, s.off_in_secs.map(human_duration).unwrap_or_else(|| "-".into())));
        v.push(format!("bytes from devices: {}   bytes to devices: {}", s.rx_bytes, s.tx_bytes));
        for c in &s.clients {
            v.push(format!("  {}  {}  {}  lease ends in {}", if c.name.is_empty() { "(no name)" } else { &c.name }, c.ip, c.mac, human_duration(c.expires_in)));
        }
    }
    v
}

/// Run against a backend (tests pass a fake). `wait_ms` is how long `start`/`stop` wait for the service to answer.
pub fn run(b: &dyn Backend, cmd: Cmd, out: &mut dyn FnMut(&str), wait_ms: u64) -> i32 {
    match cmd {
        Cmd::List => {
            let c = choices(&b.interfaces(), b.wifi_ap());
            out("internet comes from (a link that has a route to the internet):");
            if c.uplinks.is_empty() {
                out("  none: no connection on this computer has a route to the internet");
            }
            for u in &c.uplinks {
                out(&format!("  {}", u.label));
            }
            out("share it through:");
            for d in &c.downs {
                out(&format!("  {}{}", d.label, if d.usable { String::new() } else { format!("  (cannot be used: {})", d.why) }));
            }
            if let Some(w) = c.wifi_line {
                out(&format!("  {w}"));
            }
            0
        }
        Cmd::Policy(edits) => {
            let mut p = b.policy();
            if edits.is_empty() {
                out(&p.summary());
                out(&p.changed_line());
                return 0;
            }
            for (k, v) in edits {
                let list = || v.split(',').filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>();
                match k.as_str() {
                    "wired" => p.wired = Who::parse(&v).unwrap_or(p.wired),
                    "wifi" => p.wifi = Who::parse(&v).unwrap_or(p.wifi),
                    "bluetooth" => p.bluetooth = Who::parse(&v).unwrap_or(p.bluetooth),
                    "max-clients" => p.max_clients = v.parse().unwrap_or(0),
                    "max-hours" => p.max_hours = v.parse().unwrap_or(0),
                    "downstreams" => p.allowed_downstreams = list(),
                    _ => p.allowed_uplinks = list(),
                }
            }
            // the command line runs as root: an administrator
            match b.set_policy(&p, "cli", true) {
                Ok(line) => {
                    out(&line);
                    0
                }
                Err(e) => {
                    out(&format!("error: {e}"));
                    1
                }
            }
        }
        Cmd::Status => {
            for l in status_text(&b.status()) {
                out(&l);
            }
            0
        }
        Cmd::Start { uplink, via, hours, isolate, allow_lan } => {
            let prev = b.request();
            let req = control::start_request(&prev, uplink, via, hours, isolate, allow_lan, "cli", true);
            let checked = make_plan(&b.interfaces(), &req, b.wifi_ap(), &[]).and_then(|mut p| apply_policy(&mut p, &b.policy(), &req));
            if let Err(e) = checked {
                out(&format!("error: {}", e.line()));
                return 2;
            }
            out("This computer will give the devices on that cable access to your internet connection. Client isolation is on unless you said otherwise; it switches off by itself when nobody uses it.");
            finish(b, &req, out, wait_ms)
        }
        Cmd::Stop => finish(b, &control::stop_request(&b.request(), "cli"), out, wait_ms),
    }
}

fn finish(b: &dyn Backend, req: &Request, out: &mut dyn FnMut(&str), wait_ms: u64) -> i32 {
    if let Err(e) = b.submit(req) {
        out(&format!("error: could not write the request: {e} (run this as root)"));
        return 1;
    }
    let step = 100;
    let mut waited = 0;
    loop {
        let s = b.status();
        if s.request_id == req.id {
            for l in status_text(&s) {
                out(&l);
            }
            return if s.state == State::Failed { 2 } else { 0 };
        }
        if waited >= wait_ms {
            out("The request is written, but the sharing service did not answer yet. It runs inside the IYAKKI system; check `iyakki share status` in a moment.");
            return 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(step));
        waited += step;
    }
}

/// `iyakki share wifi-caps`: what every Wi-Fi radio of this computer can do as an access point, in plain lines (read only; the kernel answers, nothing is
/// started). The words come from the same check the Settings page uses.
pub fn wifi_caps_lines() -> Vec<String> {
    use crate::nl80211_os::{probe, wifi_hardware_seen, Nl80211Socket};
    use iyakki_hotspot::caps::{can_host_ap, ApMode, Want, BAND_2GHZ, BAND_5GHZ, EXT_4WAY_HANDSHAKE_AP_PSK};
    let mut out = Vec::new();
    let mut sock = match Nl80211Socket::open(std::time::Duration::from_secs(2)) {
        Ok(s) => s,
        Err(e) => return vec![format!("Wi-Fi: the kernel's wireless interface could not be opened ({e:?}).")],
    };
    let p = match probe(&mut sock) {
        Ok(p) => p,
        Err(e) => return vec![format!("Wi-Fi: the kernel did not describe its radios ({e:?}).")],
    };
    if p.phys.is_empty() {
        out.push(format!("Wi-Fi: {}", iyakki_hotspot::caps::no_card_reason(wifi_hardware_seen())));
        return out;
    }
    out.push(format!("Wi-Fi: regulatory country in force: {}", p.country));
    for c in &p.phys {
        out.push(format!(
            "radio {} (driver {}): modes {:?}, access point stations at most {}, card decides auth itself (FullMAC style): {}, firmware decides the country: {}, 4-way handshake offload for an access point (PSK): {}",
            c.name,
            c.driver.as_deref().unwrap_or("unknown"),
            c.iftypes,
            c.max_ap_stations.map_or("not said".to_string(), |n| n.to_string()),
            c.ap_sme,
            c.self_managed_reg,
            c.ext_feature(EXT_4WAY_HANDSHAKE_AP_PSK)
        ));
        out.push(format!("  interface combinations: {}", if c.combos.is_empty() { "none reported".to_string() } else { c.combos.iter().map(|k| format!("[total {}, channels {}, {}]", k.max_total, k.num_channels, k.limits.iter().map(|l| format!("{}x{:?}", l.max, l.types)).collect::<Vec<_>>().join(" + "))).collect::<Vec<_>>().join(" ") }));
        for (label, band) in [("2.4 GHz", BAND_2GHZ), ("5 GHz", BAND_5GHZ)] {
            for (how, sta) in [("alone", false), ("while also a Wi-Fi client", true)] {
                let mut w = Want::new(band, &p.country);
                w.concurrent_sta = sta;
                out.push(format!(
                    "  {label} access point {how}: {}",
                    match can_host_ap(c, &w) {
                        Ok(plan) => format!("possible on channel {} ({})", plan.channel, match plan.mode { ApMode::Software => "this computer runs the handshake", ApMode::PskOffload => "the card runs the handshake from our passphrase", ApMode::CardManaged => "the card manages it all; not supported yet" }),
                        Err(r) => format!("not possible: {}", r.reason),
                    }
                ));
            }
        }
    }
    out
}

/// `iyakki share ...` on the real machine.
pub fn main(args: &[String]) -> i32 {
    if matches!(args.first().map(String::as_str), Some("wifi-info" | "wifi-set")) {
        let data = std::path::PathBuf::from(std::env::var("IYAKKI_DATA").unwrap_or_else(|_| "/var/lib/iyakki".into()));
        let b = control::FileBackend { data };
        if args[0] == "wifi-set" {
            let [_, ssid, pass] = args else {
                eprintln!("usage: iyakki share wifi-set NAME PASSWORD (the password is 8 to 63 characters)");
                return 2;
            };
            return match b.set_hotspot(ssid, pass) {
                Ok(()) => {
                    println!("The hotspot is called {ssid:?} now. It takes effect the next time the hotspot starts.");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            };
        }
        return match b.hotspot() {
            Some((s, p)) => {
                println!("name: {s}\npassword: {p}");
                0
            }
            None => {
                eprintln!("error: cannot read the hotspot settings (run this as root)");
                1
            }
        };
    }
    if args.first().map(String::as_str) == Some("wifi-caps") {
        for l in wifi_caps_lines() {
            println!("{l}");
        }
        return 0;
    }
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            println!("{e}");
            return 2;
        }
    };
    let b = control::FileBackend { data: std::path::PathBuf::from("/var/lib/iyakki") };
    run(&b, cmd, &mut |l| println!("{l}"), 8000)
}
