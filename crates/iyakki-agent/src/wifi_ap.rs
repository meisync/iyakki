//! The Wi-Fi hotspot's radio for a card that does authentication, association and beacons itself (FullMAC, `ApMode::PskOffload` or card-managed): a second interface of type
//! access point is made next to the Wi-Fi client and the access point is started on it. The 4-way handshake is run by this computer (`HandshakeMode::Host`, see
//! `wifi_ap_host`, the default) or handed to the firmware with the key derived from the passphrase (`HandshakeMode::PskOffload`). Nothing here knows a chip or a driver: the capability probe decides
//! (`iyakki_hotspot::caps::can_host_ap`), and a card that needs our own handshake (`ApMode::Software`) is not started by this module.
//!
//! The access point itself stays up without this process, but in host mode only this process runs the handshake (a thread, ended by `stop`). Addresses, DHCP, DNS and the firewall are the
//! sharing manager's job (`crate::sharing`), on the interface this returns.

use crate::nl80211_os::{probe, Nl80211Socket};
use iyakki_hotspot::nl80211::{self, ApControl, ApParams};
use iyakki_hotspot::{caps, kdf, mgmt, settings};
use std::time::Duration;

/// The name of the access point's interface.
pub const AP_IFACE: &str = iyakki_devctl::HOTSPOT_IFACE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Up {
    pub ifname: String,
    pub ifindex: u32,
    pub channel: u32,
    pub bssid: String,
}

fn sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn ifindex_of(name: &str) -> Option<u32> {
    sysfs(&format!("/sys/class/net/{name}/ifindex")).and_then(|s| s.parse().ok())
}

/// Who runs the WPA2 4-way handshake on a card that does authentication and association itself (FullMAC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeMode {
    /// The firmware gets the PMK (`NL80211_ATTR_PMK`) and does the handshake. The Mac mini's BCM4364 accepts the key and never
    /// sends message 1, so this is only used when asked for.
    PskOffload,
    /// The access point starts without a PMK; `wifi_ap_host` runs the handshake and installs the keys, like hostapd does on brcmfmac.
    Host,
}

/// The mode for any FullMAC card. The hotspot is generic (no code for a chip or a driver), so the host handshake is the default:
/// it needs only what hostapd needs (key install, authorize, EAPOL on the interface), while the firmware's PMK hand-over is
/// proven not to work on the Mac mini's Broadcom card although it advertises it. `IYAKKI_AP_HANDSHAKE=offload` selects the
/// firmware's handshake (and `host` the default) for a card that is known to do it right.
pub fn handshake_mode() -> HandshakeMode {
    match std::env::var("IYAKKI_AP_HANDSHAKE").ok().as_deref() {
        Some("offload") => HandshakeMode::PskOffload,
        _ => HandshakeMode::Host,
    }
}

/// The beacon pieces for a 2.4 GHz network: head with the name and channel, tail with the rates and the WPA2 element.
pub fn params(ifindex: u32, bssid: [u8; 6], ssid: &str, channel: u32, freq: u32, pmk: [u8; 32]) -> ApParams {
    let ch = channel as u8;
    ApParams {
        ifindex,
        ssid: ssid.as_bytes().to_vec(),
        freq_mhz: freq,
        beacon_interval_tu: 100,
        dtim_period: 2,
        beacon_head: mgmt::beacon_head(bssid, ssid.as_bytes(), 100, mgmt::Band::G24, ch),
        beacon_tail: mgmt::beacon_tail(mgmt::Band::G24),
        probe_resp: mgmt::probe_response(bssid, ssid.as_bytes(), 100, mgmt::Band::G24, ch),
        inactivity_secs: 0,
        pmk: Some(pmk),
    }
}

/// Which channel the access point takes: on the client's channel in whichever band that is ("auto"), or in the band the person chose ("2.4" or "5", the card then moves the client).
/// A band that cannot host an access point (5 GHz channels need a country first) falls back to 2.4 GHz and the client follows. The second value says the person chose a band.
fn choose_plan(c: &caps::PhyCaps, band_pref: &str, sta_freq: Option<u32>, country: &str, log: &mut dyn FnMut(String)) -> Result<(caps::ApPlan, Option<u32>), String> {
    let chosen = match band_pref {
        "2.4" => Some(caps::BAND_2GHZ),
        "5" => Some(caps::BAND_5GHZ),
        _ => None,
    };
    let band = chosen.unwrap_or(if sta_freq.is_some_and(|f| f >= 4900) { caps::BAND_5GHZ } else { caps::BAND_2GHZ });
    let sta_freq = if chosen.is_some() { None } else { sta_freq };
    let want = caps::Want { concurrent_sta: true, sta_channel_freq: sta_freq, ..caps::Want::new(band, country) };
    let plan = match caps::can_host_ap(c, &want) {
        Ok(p) => p,
        Err(first) if band != caps::BAND_2GHZ => {
            log(format!("hotspot: not on the client's channel ({}); trying 2.4 GHz", first.reason));
            let w24 = caps::Want { concurrent_sta: true, sta_channel_freq: None, ..caps::Want::new(caps::BAND_2GHZ, country) };
            caps::can_host_ap(c, &w24).map_err(|r| r.reason)?
        }
        Err(r) => return Err(r.reason),
    };
    Ok((plan, chosen))
}

/// Start the hotspot's radio next to the Wi-Fi client `client` (for example `wlan0`). `Err` is one plain line (with the kernel's own
/// words when it gave some).
/// How long the access point waits for the Wi-Fi client to join its network (one channel for both).
const STA_WAIT_SECS: u32 = 15;

pub fn start(client: &str, ssid: &str, passphrase: &str, band_pref: &str, client_paused: bool, log: &mut dyn FnMut(String)) -> Result<Up, String> {
    settings::validate_ssid(ssid).map_err(|e| format!("the network name: {e:?}"))?;
    settings::validate_passphrase(passphrase).map_err(|e| format!("the password: {e:?}"))?;
    let phy_name = sysfs(&format!("/sys/class/net/{client}/phy80211/name")).ok_or_else(|| format!("{client} is not a Wi-Fi interface"))?;
    let wiphy: u32 = sysfs(&format!("/sys/class/net/{client}/phy80211/index")).and_then(|s| s.parse().ok()).ok_or("the radio has no index")?;

    let sock = Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let nl = crate::nl80211_os::SharedNl::new(sock);
    let err = |what: &str, e: nl80211::NlError, nl: &crate::nl80211_os::SharedNl| -> String {
        let ext = nl.0.borrow().last_ext_ack().map(|t| format!(" ({t})")).unwrap_or_default();
        format!("{what}: {e}{ext}")
    };
    let pr = probe(&mut nl.0.borrow_mut()).map_err(|e| e.to_string())?;
    let c = pr.phys.iter().find(|p| p.name == phy_name).ok_or("the kernel lists no radio for this interface")?;
    // at start-up the access point may come before the client has joined the router: give it a few seconds, so both end on one channel
    let mut sta_freq = None;
    for waited in 0..if client_paused { 0 } else { STA_WAIT_SECS } {
        sta_freq = ifindex_of(client).and_then(|i| ApControl::new(nl.clone()).interface_freq(i).ok().flatten());
        if sta_freq.is_some() {
            break;
        }
        if waited == 0 {
            log(format!("hotspot: {client} is not on a channel yet; waiting up to {STA_WAIT_SECS} s for it to join its network"));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    log(format!("hotspot: {client} is on {}", sta_freq.map(|f| format!("{f} MHz")).unwrap_or_else(|| "no channel".into())));
    let (plan, chosen) = choose_plan(c, band_pref, sta_freq, &pr.country, log)?;
    log(format!("hotspot: {} ({:?}) can host: channel {} ({} MHz), mode {:?}", c.name, c.driver, plan.channel, plan.freq, plan.mode));
    let hs = handshake_mode();
    log(format!("hotspot: handshake by {}", if hs == HandshakeMode::Host { "this computer (host mode)" } else { "the card's firmware (PMK offload)" }));
    match (plan.mode, hs) {
        (caps::ApMode::Software, _) => return Err("this card needs the computer to run the handshake; that path is only used for a card of its own".into()),
        (caps::ApMode::PskOffload, _) | (caps::ApMode::CardManaged, HandshakeMode::Host) => {}
        _ => return Err("this card manages the access point by itself and gives no way to set the password; not supported".to_string()),
    }

    let mut ap = ApControl::new(nl.clone());
    if let Some(old) = ifindex_of(AP_IFACE) {
        // left over from a crash
        let _ = ap.stop_ap(old);
        let _ = ap.remove_interface(old);
    }
    ap.add_ap_interface(wiphy, AP_IFACE).map_err(|e| err("making the access point interface", e, &nl))?;
    let ifindex = ifindex_of(AP_IFACE).ok_or("the access point interface did not appear")?;
    let mac_text = sysfs(&format!("/sys/class/net/{AP_IFACE}/address")).ok_or("the access point interface has no address")?;
    let bssid: [u8; 6] = mac_text.split(':').filter_map(|p| u8::from_str_radix(p, 16).ok()).collect::<Vec<_>>().try_into().map_err(|_| "bad interface address".to_string())?;
    let undo = |ap: &mut ApControl<crate::nl80211_os::SharedNl>| {
        let _ = ap.remove_interface(ifindex);
    };
    if let Err(e) = crate::sharing::real::sys::link_up(AP_IFACE) {
        undo(&mut ap);
        return Err(e);
    }
    let pmk = kdf::pmk_from_passphrase(passphrase, ssid.as_bytes());
    // host mode: no PMK in the start, so the driver expects us to install keys; the runner follows every (re)start of the access point
    let mk = |ch: u32, f: u32| {
        let mut p = params(ifindex, bssid, ssid, ch, f, pmk);
        if hs == HandshakeMode::Host {
            p.pmk = None;
        }
        p
    };
    let host_begin = || -> Result<(), String> {
        if hs == HandshakeMode::Host {
            crate::wifi_ap_host::begin(ifindex, bssid, ssid, passphrase)?;
        }
        Ok(())
    };
    if let Err(e) = ap.start_ap(&mk(plan.channel, plan.freq)) {
        let msg = err("starting the access point", e, &nl);
        undo(&mut ap);
        return Err(msg);
    }
    if let Err(e) = host_begin() {
        let _ = ap.stop_ap(ifindex);
        undo(&mut ap);
        return Err(format!("starting the handshake: {e}"));
    }
    log(format!("hotspot: access point {ssid:?} up on {AP_IFACE} bssid {mac_text}"));
    // starting the access point can move the client (a card with one radio gives up its 5 GHz link for 2.4 GHz): the access point
    // follows it, so both sit on one channel. Without that the access point hears nobody.
    let (mut freq, mut channel) = (plan.freq, plan.channel);
    let mut steady = 0;
    // with a band the person chose, the access point stays there
    for _ in 0..if chosen.is_some() { 0 } else { 10 } {
        std::thread::sleep(Duration::from_secs(4));
        let Some(now) = ifindex_of(client).and_then(|i| ApControl::new(nl.clone()).interface_freq(i).ok().flatten()) else { break };
        if now == freq {
            // the client can move again a few seconds after the first change: stay a while and watch
            steady += 1;
            if steady >= 3 {
                break;
            }
            continue;
        }
        steady = 0;
        let ch = if now < 4000 { (now - 2407) / 5 } else { (now - 5000) / 5 };
        log(format!("hotspot: {client} moved to {now} MHz; moving the access point from {freq} MHz"));
        // stopping the access point drops its keys: the runner ends first and begins again after the start
        crate::wifi_ap_host::end();
        let _ = ap.stop_ap(ifindex);
        match ap.start_ap(&mk(ch, now)) {
            Ok(()) => {
                (freq, channel) = (now, ch);
                if let Err(e) = host_begin() {
                    log(format!("hotspot: handshake did not restart: {e}"));
                }
            }
            Err(e) => {
                log(format!("hotspot: could not move the access point: {}", err("starting it again", e, &nl)));
                // go back to where it was
                let _ = ap.start_ap(&mk(channel, freq));
                let _ = host_begin();
                break;
            }
        }
    }
    Ok(Up { ifname: AP_IFACE.into(), ifindex, channel, bssid: mac_text })
}

/// Ask `wpa_supplicant` (its control socket) to do one thing for the client interface; the answer is its one-line reply. The run directory is given
/// (`<dir>/wpa_supplicant/<client>` is the supplicant's socket); tests use a scratch directory.
fn wpa_command_at(dir: &str, client: &str, cmd: &str) -> Result<String, String> {
    use std::os::unix::net::UnixDatagram;
    let me = format!("{dir}/iyakki-wpa-{}-{}", std::process::id(), cmd.len());
    let _ = std::fs::remove_file(&me);
    let sock = UnixDatagram::bind(&me).map_err(|e| format!("{me}: {e}"))?;
    let r = (|| {
        sock.set_read_timeout(Some(Duration::from_secs(3))).map_err(|e| e.to_string())?;
        sock.connect(format!("{dir}/wpa_supplicant/{client}")).map_err(|e| format!("wpa_supplicant: {e}"))?;
        sock.send(cmd.as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 256];
        let n = sock.recv(&mut buf).map_err(|e| format!("wpa_supplicant did not answer: {e}"))?;
        Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
    })();
    let _ = std::fs::remove_file(&me);
    r
}

/// Let go of the router on the client interface, so the card's one radio belongs to the hotspot. Used when the internet comes from a cable.
pub fn pause_client(client: &str) -> Result<(), String> {
    pause_client_at("/run", client)
}

fn pause_client_at(dir: &str, client: &str) -> Result<(), String> {
    wpa_command_at(dir, client, "DISCONNECT").and_then(|r| if r == "OK" { Ok(()) } else { Err(format!("wpa_supplicant said {r}")) })
}

/// Join the router again after [`pause_client`].
pub fn resume_client(client: &str) -> Result<(), String> {
    resume_client_at("/run", client)
}

fn resume_client_at(dir: &str, client: &str) -> Result<(), String> {
    wpa_command_at(dir, client, "RECONNECT").map(|_| ())
}

/// End the access point and remove its interface. Not an error when it is not there.
pub fn stop() -> Result<(), String> {
    crate::wifi_ap_host::end();
    let Some(ifindex) = ifindex_of(AP_IFACE) else { return Ok(()) };
    let sock = Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let mut ap = ApControl::new(crate::nl80211_os::SharedNl::new(sock));
    let _ = ap.stop_ap(ifindex);
    ap.remove_interface(ifindex).map_err(|e| format!("removing the access point interface: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_carries_the_key_the_name_and_the_wpa2_element() {
        let pmk = kdf::pmk_from_passphrase("correct horse", b"IYAKKI-1A2B");
        let p = params(9, [2, 0, 0, 0, 0, 1], "IYAKKI-1A2B", 6, 2437, pmk);
        assert_eq!((p.pmk, p.freq_mhz, p.ssid.as_slice()), (Some(pmk), 2437, &b"IYAKKI-1A2B"[..]));
        let rsn = iyakki_hotspot::eapol::rsn_ie_ccmp_psk();
        assert!(p.beacon_tail.windows(rsn.len()).any(|w| w == rsn.as_slice()), "the tail has the RSN element the card reads");
        assert!(p.beacon_head.windows(11).any(|w| w == b"IYAKKI-1A2B"));
    }

    #[test]
    fn the_host_handshake_is_the_default() {
        assert_eq!(handshake_mode(), HandshakeMode::Host);
    }

    #[test]
    fn a_bad_name_or_password_is_refused_before_anything_is_touched() {
        let mut l = |_: String| {};
        assert!(start("wlan0", "", "longenough1", "auto", false, &mut l).is_err());
        assert!(start("wlan0", "ok", "short", "auto", false, &mut l).is_err());
        assert!(start("not-a-wifi-interface-xyz", "ok", "longenough1", "auto", false, &mut l).unwrap_err().contains("not a Wi-Fi interface"));
    }

    use caps::{Band, Channel, IfComb, IfLimit, PhyCaps};

    fn chan(freq: u32, flags: &str) -> Channel {
        Channel { freq, disabled: flags.contains('d'), no_ir: flags.contains('n'), radar: flags.contains('r'), ..Default::default() }
    }

    /// A one-radio FullMAC card (like the Mac mini's): a client and an access point on one channel; the 5 GHz channels are free to use only with a country.
    fn card(with_country_5ghz_free: bool) -> PhyCaps {
        let w5 = if with_country_5ghz_free { "" } else { "n" };
        PhyCaps {
            name: "phy0".into(),
            iftypes: vec![caps::IFTYPE_STATION, caps::IFTYPE_AP],
            combos: vec![IfComb { limits: vec![IfLimit { max: 2, types: vec![caps::IFTYPE_STATION] }, IfLimit { max: 1, types: vec![caps::IFTYPE_AP] }], max_total: 3, num_channels: 1, sta_ap_bi_match: false }],
            bands: vec![
                Band { id: caps::BAND_2GHZ, channels: (1..=11).map(|c| chan(2407 + 5 * c, "")).collect() },
                Band { id: caps::BAND_5GHZ, channels: [36, 40, 44, 48, 149].iter().map(|c| chan(5000 + 5 * c, w5)).collect() },
            ],
            ciphers: vec![caps::CIPHER_CCMP],
            ap_sme: true,
            ..Default::default()
        }
    }

    #[test]
    fn auto_follows_the_clients_channel_in_its_band() {
        let mut said = Vec::new();
        let (p, chosen) = choose_plan(&card(true), "auto", Some(5180), "DE", &mut |l| said.push(l)).unwrap();
        assert_eq!((p.freq, chosen), (5180, None));
        let (p, _) = choose_plan(&card(true), "auto", Some(2437), "DE", &mut |_| {}).unwrap();
        assert_eq!(p.freq, 2437);
        assert!(said.is_empty());
    }

    #[test]
    fn a_5ghz_client_without_a_country_falls_back_to_2_4_ghz_and_says_why() {
        let mut said = Vec::new();
        let (p, _) = choose_plan(&card(false), "auto", Some(5180), "00", &mut |l| said.push(l)).unwrap();
        assert!(p.freq < 3000, "2.4 GHz channel, got {}", p.freq);
        assert!(said.iter().any(|l| l.contains("trying 2.4 GHz")), "{said:?}");
    }

    #[test]
    fn a_chosen_band_is_kept_and_a_chosen_5ghz_without_a_country_falls_back() {
        let (p, chosen) = choose_plan(&card(true), "5", Some(2437), "DE", &mut |_| {}).unwrap();
        assert!(p.freq >= 5000 && chosen == Some(caps::BAND_5GHZ));
        let (p, chosen) = choose_plan(&card(true), "2.4", Some(5180), "DE", &mut |_| {}).unwrap();
        assert!(p.freq < 3000 && chosen == Some(caps::BAND_2GHZ));
        let (p, _) = choose_plan(&card(false), "5", None, "00", &mut |_| {}).unwrap();
        assert!(p.freq < 3000);
    }

    #[test]
    fn a_card_with_no_2_4_ghz_band_gives_the_reason_when_the_fallback_fails() {
        let mut c = card(false);
        c.bands.retain(|b| b.id == caps::BAND_5GHZ);
        let e = choose_plan(&c, "auto", Some(5180), "00", &mut |_| {}).unwrap_err();
        assert!(e.contains("2.4 GHz"), "{e}");
    }

    /// A fake wpa_supplicant: answers each datagram with the given reply and reports what it was asked.
    fn fake_supplicant(reply: &'static str) -> (String, std::sync::mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        use std::os::unix::net::UnixDatagram;
        let dir = crate::testdir::short("iyakki-wpa", reply); // short: a Unix socket path must stay under SUN_LEN, whatever TMPDIR is
        std::fs::create_dir_all(dir.join("wpa_supplicant")).unwrap();
        let sock = UnixDatagram::bind(dir.join("wpa_supplicant/wlan0")).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((n, from)) = sock.recv_from(&mut buf) {
                tx.send(String::from_utf8_lossy(&buf[..n]).to_string()).unwrap();
                if let Some(p) = from.as_pathname() {
                    let _ = sock.send_to(reply.as_bytes(), p);
                }
            }
        });
        (dir.to_string_lossy().to_string(), rx, h)
    }

    #[test]
    fn pausing_sends_disconnect_and_resuming_sends_reconnect() {
        let (dir, rx, h) = fake_supplicant("OK\n");
        pause_client_at(&dir, "wlan0").unwrap();
        assert_eq!(rx.recv().unwrap(), "DISCONNECT");
        h.join().unwrap();
        let (dir2, rx2, h2) = fake_supplicant("OK\n");
        resume_client_at(&dir2, "wlan0").unwrap();
        assert_eq!(rx2.recv().unwrap(), "RECONNECT");
        h2.join().unwrap();
        let _ = (std::fs::remove_dir_all(dir), std::fs::remove_dir_all(dir2));
    }

    #[test]
    fn a_refusal_or_a_missing_supplicant_makes_the_pause_fail_cleanly() {
        let (dir, _rx, h) = fake_supplicant("FAIL\n");
        let e = pause_client_at(&dir, "wlan0").unwrap_err();
        assert!(e.contains("FAIL"), "{e}");
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let none = crate::testdir::short("iyakki-wpa", "none"); // short for the same reason: the error must be "no supplicant", not "path too long"
        std::fs::create_dir_all(&none).unwrap();
        assert!(pause_client_at(none.to_str().unwrap(), "wlan0").unwrap_err().contains("wpa_supplicant"));
        let _ = std::fs::remove_dir_all(&none);
    }
}
