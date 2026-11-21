//! Runs one access point on a (virtual) radio for the virtual-radio test: see crates/iyakki-hotspot/tools/hwsim-test.sh.
//! Usage: hotspot_hwsim <interface> <ssid> <passphrase> <channel> <seconds> [wpa2|transition|wpa3]
//! Needs root and a mac80211 radio; it is started inside the test VM, never on a development machine.
use iyakki_agent::hotspot_radio::{run, RadioConfig};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 6 && a.len() != 7 {
        eprintln!("usage: hotspot_hwsim <interface> <ssid> <passphrase> <channel> <seconds> [wpa2|transition|wpa3]");
        std::process::exit(2);
    }
    let cfg = RadioConfig { ifname: a[1].clone(), ssid: a[2].clone(), passphrase: a[3].clone(), channel: a[4].parse().unwrap_or(6), gateway: std::net::Ipv4Addr::new(10, 42, 0, 1), max_clients: 8, run_secs: a[5].parse().unwrap_or(30), security: match a.get(6).map(String::as_str) { Some("transition") => iyakki_hotspot::rsn::SecurityMode::Transition, Some("wpa3") => iyakki_hotspot::rsn::SecurityMode::Wpa3, _ => iyakki_hotspot::rsn::SecurityMode::Wpa2 } };
    let mut log = |s: String| {
        println!("{s}");
    };
    match run(&cfg, &mut log, &|| false) {
        Ok(s) => println!("AP-RESULT ok associated={} authorized={} keys={} failed={} dhcp_acks={}", s.associated.len(), s.authorized.len(), s.keys_installed.len(), s.failed.len(), s.dhcp_acks.len()),
        Err(e) => {
            println!("AP-RESULT error {e}");
            std::process::exit(1);
        }
    }
}
