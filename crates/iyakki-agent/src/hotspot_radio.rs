//! The radio side of the Wi-Fi hotspot, phase P1 of docs/development/HOTSPOT-PLAN.md: one access point on one interface,
//! driven through the real nl80211 socket and the EAPOL packet socket. It wires the pure parts of `iyakki-hotspot`
//! (management frames, 4-way handshake, DHCP) to the operating system and runs the event loop.
//!
//! What the kernel does and what we do (found out on virtual radios, see the plan):
//! * mac80211 builds and sends the beacons (our head and tail, it adds the TIM) and answers probe requests from our template.
//! * Authentication and association are NOT done by the kernel for an access point: they arrive as registered management
//!   frames and we answer them (`mgmt`), and we add the station (`NEW_STATION`) ourselves, like hostapd does.
//! * EAPOL travels over an `AF_PACKET` socket on the interface; we install the keys and open the port (`SET_STATION`).
//!
//! This is the loop used by the virtual-radio test (`examples/hotspot_hwsim.rs`). The product's manager will own the same
//! pieces with its own timers, settings and firewall; link and address set-up here use the `ip` program and are for the test
//! bed only (the product will use rtnetlink).

use crate::nl80211_os::{poll_readable, udp_on_interface, EapolChannel, Nl80211Socket, PacketEapol, SharedNl};
use iyakki_hotspot::dhcp::{self, DhcpConfig, DhcpServer, Dest};
use iyakki_hotspot::handshake::{Action, AuthConfig, Authenticator, FailReason};
use iyakki_hotspot::nl80211::{self, ApControl, ApParams, Event};
use iyakki_hotspot::{caps, eapol, kdf, mgmt, Mac};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RadioConfig {
    pub ifname: String,
    pub ssid: String,
    pub passphrase: String,
    /// 2.4 GHz channel 1 to 13.
    pub channel: u8,
    pub gateway: Ipv4Addr,
    pub max_clients: usize,
    /// How long the test access point runs.
    pub run_secs: u64,
}

/// What happened, for the test script and the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub associated: Vec<Mac>,
    pub authorized: Vec<Mac>,
    pub keys_installed: Vec<Mac>,
    pub failed: Vec<(Mac, String)>,
    pub dhcp_acks: Vec<(Mac, Ipv4Addr)>,
}

pub fn channel_freq(ch: u8) -> Option<u32> {
    match ch {
        1..=13 => Some(2407 + 5 * ch as u32),
        14 => Some(2484),
        _ => None,
    }
}

pub fn mac_text(m: &Mac) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

fn sysfs(ifname: &str, file: &str) -> Result<String, String> {
    if ifname.is_empty() || !ifname.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.') {
        return Err(format!("bad interface name {ifname:?}"));
    }
    std::fs::read_to_string(format!("/sys/class/net/{ifname}/{file}")).map(|s| s.trim().to_string()).map_err(|e| format!("{ifname}: {e}"))
}

fn parse_mac(s: &str) -> Option<Mac> {
    let v: Vec<u8> = s.split(':').filter_map(|p| u8::from_str_radix(p, 16).ok()).collect();
    v.try_into().ok()
}

fn ip(args: &[&str]) -> Result<(), String> {
    let o = std::process::Command::new("ip").args(args).output().map_err(|e| format!("ip: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("ip {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

fn random(buf: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).map_err(|e| format!("random bytes: {e}"))
}

struct Sta {
    auth: Authenticator,
    aid: u16,
}

/// Run one access point for `cfg.run_secs` seconds (or until `stop` says so), logging every step through `log`.
pub fn run(cfg: &RadioConfig, log: &mut dyn FnMut(String), stop: &dyn Fn() -> bool) -> Result<Summary, String> {
    let freq = channel_freq(cfg.channel).ok_or("unknown channel")?;
    iyakki_hotspot::settings::validate_ssid(&cfg.ssid).map_err(|e| format!("{e:?}"))?;
    iyakki_hotspot::settings::validate_passphrase(&cfg.passphrase).map_err(|e| format!("{e:?}"))?;
    let ifindex: u32 = sysfs(&cfg.ifname, "ifindex")?.parse().map_err(|_| "ifindex".to_string())?;
    let bssid = parse_mac(&sysfs(&cfg.ifname, "address")?).ok_or("interface address")?;
    let ssid = cfg.ssid.as_bytes().to_vec();
    let pmk = kdf::pmk_from_passphrase(&cfg.passphrase, &ssid);

    let sock = Nl80211Socket::open(Duration::from_secs(3)).map_err(|e| e.to_string())?;
    let nl = SharedNl::new(sock);
    let fam = nl.0.borrow_mut().family().map_err(|e| e.to_string())?;
    log(format!("AP: nl80211 family {} groups {:?}", fam.id, fam.groups.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()));
    nl.0.borrow_mut().join_ap_groups().map_err(|e| e.to_string())?;
    let mut ap = ApControl::new(nl.clone());
    let nlerr = |what: &str, e: nl80211::NlError, nl: &SharedNl| -> String {
        let ext = nl.0.borrow().last_ext_ack().map(|t| format!(" ({t})")).unwrap_or_default();
        format!("{what}: {e}{ext}")
    };

    let wiphys = ap.wiphys().map_err(|e| nlerr("listing the radios", e, &nl))?;
    log(format!("AP: radios {:?}", wiphys.iter().map(|w| (w.name.clone(), w.supports_ap())).collect::<Vec<_>>()));

    // the generic capability probe decides whether and where this interface can host the access point
    let pr = crate::nl80211_os::probe(&mut nl.0.borrow_mut()).map_err(|e| e.to_string())?;
    let phy_name = sysfs(&cfg.ifname, "phy80211/name")?;
    let caps = pr.phys.iter().find(|p| p.name == phy_name).ok_or("the kernel lists no radio for this interface")?;
    log(format!("AP: probe {} driver={:?} country={} iftypes={:?} combos={} ciphers_ccmp={} max_ap_sta={:?} ap_sme={} self_managed_reg={}", caps.name, caps.driver, pr.country, caps.iftypes, caps.combos.len(), caps.ciphers.contains(&caps::CIPHER_CCMP), caps.max_ap_stations, caps.ap_sme, caps.self_managed_reg));
    for c in &caps.combos {
        log(format!("AP: probe combination {:?} total<={} channels<={}", c.limits.iter().map(|l| (l.max, l.types.clone())).collect::<Vec<_>>(), c.max_total, c.num_channels));
    }
    match caps::can_host_ap(caps, &caps::Want::new(caps::BAND_2GHZ, &pr.country)) {
        Ok(plan) => {
            log(format!("AP: probe verdict yes: channel {} ({} MHz) width {} mode {:?} concurrent-ap+sta {}", plan.channel, plan.freq, plan.width_mhz, plan.mode, caps::can_host_ap(caps, &caps::Want { concurrent_sta: true, ..caps::Want::new(caps::BAND_2GHZ, &pr.country) }).is_ok()));
            if plan.mode != caps::ApMode::Software {
                return Err("this card runs the access point itself; not supported yet".into());
            }
        }
        Err(r) => return Err(format!("probe verdict no: {}", r.reason)),
    }

    // interface to AP mode (it must be down), then up
    let _ = ip(&["link", "set", &cfg.ifname, "down"]);
    ap.set_ap_mode(ifindex).map_err(|e| nlerr("switching to access point mode", e, &nl))?;
    ip(&["link", "set", &cfg.ifname, "up"])?;
    ip(&["addr", "add", &format!("{}/24", cfg.gateway), "dev", &cfg.ifname])?;

    // management frames we answer: authentication and association requests
    for t in [0x00b0u16, 0x0000] {
        nl.0.borrow_mut().run(|f, q| nl80211::register_frame(f, q, ifindex, t)).map_err(|e| nlerr("registering for frames", e, &nl))?;
    }
    let params = ApParams {
        ifindex,
        ssid: ssid.clone(),
        freq_mhz: freq,
        beacon_interval_tu: 100,
        dtim_period: 2,
        beacon_head: mgmt::beacon_head(bssid, &ssid, 100, mgmt::Band::G24, cfg.channel),
        beacon_tail: mgmt::beacon_tail(mgmt::Band::G24),
        probe_resp: mgmt::probe_response(bssid, &ssid, 100, mgmt::Band::G24, cfg.channel),
        // only with the capability: NL80211_FEATURE_INACTIVITY_TIMER (bit 2) in the feature flags
        inactivity_secs: if caps.feature_flags & (1 << 2) != 0 { 300 } else { 0 },
        pmk: None,
    };
    ap.start_ap(&params).map_err(|e| nlerr("starting the access point", e, &nl))?;
    let mut gtk = [0u8; 16];
    random(&mut gtk)?;
    ap.install_gtk(ifindex, 1, &gtk).map_err(|e| nlerr("installing the group key", e, &nl))?;
    log(format!("AP: started ssid={} bssid={} channel={} freq={}", cfg.ssid, mac_text(&bssid), cfg.channel, freq));

    let mut eap = PacketEapol::open(ifindex)?;
    let udp = udp_on_interface(&cfg.ifname, 67)?;
    let o = cfg.gateway.octets();
    let mut dhcp_srv = DhcpServer::new(DhcpConfig {
        server_ip: cfg.gateway,
        mask: Ipv4Addr::new(255, 255, 255, 0),
        broadcast: Ipv4Addr::new(o[0], o[1], o[2], 255),
        pool_start: Ipv4Addr::new(o[0], o[1], o[2], 10),
        pool_end: Ipv4Addr::new(o[0], o[1], o[2], 40),
        lease_secs: 3600,
        dns: cfg.gateway,
    });

    let t0 = Instant::now();
    let now_ms = || t0.elapsed().as_millis() as u64;
    let mut stations: HashMap<Mac, Sta> = HashMap::new();
    let mut sum = Summary::default();

    while !stop() && t0.elapsed() < Duration::from_secs(cfg.run_secs) {
        let ready = poll_readable(&[nl.0.borrow().as_raw_fd(), eap.raw_fd(), std::os::fd::AsRawFd::as_raw_fd(&udp)], 100);
        let mut actions: Vec<(Mac, Vec<Action>)> = Vec::new();

        // kernel events: management frames, stations coming and going
        if ready[0] {
            loop {
                let ev = nl.0.borrow_mut().next_event(0).map_err(|e| e.to_string())?;
                let Some(raw) = ev else { break };
                match nl80211::parse_event(&raw) {
                    Some(Event::Frame { ifindex: i, frame, .. }) if i == ifindex => {
                        let Some(h) = mgmt::parse_header(&frame) else { continue };
                        if h.fc0 == mgmt::SUBTYPE_AUTH {
                            if let Some(resp) = mgmt::auth_response(&frame, bssid) {
                                log(format!("AP: auth request from {} -> answer", mac_text(&h.sa)));
                                if let Err(e) = nl.0.borrow_mut().run(|f, q| nl80211::send_frame(f, q, ifindex, freq, &resp)) {
                                    log(format!("AP: sending the authentication answer failed: {e}"));
                                }
                            }
                        } else if h.fc0 == mgmt::SUBTYPE_ASSOC_REQ {
                            let Some(r) = mgmt::parse_assoc_request(&frame, bssid) else { continue };
                            let status = mgmt::assoc_status(&r, &ssid, stations.len(), cfg.max_clients);
                            log(format!("AP: association request from {} status {status}", mac_text(&r.sta)));
                            if status != mgmt::STATUS_OK {
                                let resp = mgmt::assoc_response(r.sta, bssid, mgmt::Band::G24, status, 0);
                                let _ = nl.0.borrow_mut().run(|f, q| nl80211::send_frame(f, q, ifindex, freq, &resp));
                                continue;
                            }
                            if stations.remove(&r.sta).is_some() {
                                let _ = ap.remove_station(ifindex, r.sta, mgmt::REASON_UNSPECIFIED);
                            }
                            let used: Vec<u16> = stations.values().map(|s| s.aid).collect();
                            let Some(aid) = mgmt::free_aid(&used) else { continue };
                            if let Err(e) = ap.add_station(ifindex, r.sta, aid, r.listen_interval, &r.rates, r.capability) {
                                log(format!("AP: adding the station failed: {}", nlerr("NEW_STATION", e, &nl)));
                                continue;
                            }
                            let resp = mgmt::assoc_response(r.sta, bssid, mgmt::Band::G24, mgmt::STATUS_OK, aid);
                            if let Err(e) = nl.0.borrow_mut().run(|f, q| nl80211::send_frame(f, q, ifindex, freq, &resp)) {
                                log(format!("AP: sending the association answer failed: {e}"));
                            }
                            let mut anonce = [0u8; 32];
                            random(&mut anonce)?;
                            let mut auth = Authenticator::new(AuthConfig { pmk, aa: bssid, spa: r.sta, anonce, gtk, gtk_id: 1, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1 });
                            let first = auth.start(now_ms());
                            stations.insert(r.sta, Sta { auth, aid });
                            sum.associated.push(r.sta);
                            log(format!("AP: associated {} aid {aid}, handshake started", mac_text(&r.sta)));
                            actions.push((r.sta, first));
                        }
                    }
                    Some(Event::Station { ifindex: i, sta, added }) if i == ifindex => {
                        log(format!("AP: kernel says station {} {}", mac_text(&sta), if added { "added" } else { "removed" }));
                        if !added && stations.remove(&sta).is_some() {
                            dhcp_srv.station_left(&sta);
                        }
                    }
                    _ => {}
                }
            }
        }

        // EAPOL from clients
        if ready[1] {
            while let Some((sta, bytes)) = eap.recv(0)? {
                if let Some(s) = stations.get_mut(&sta) {
                    let a = s.auth.on_frame(&bytes, now_ms());
                    actions.push((sta, a));
                }
            }
        }

        // handshake time-outs and resends
        for (mac, s) in stations.iter_mut() {
            let a = s.auth.tick(now_ms());
            if !a.is_empty() {
                actions.push((*mac, a));
            }
        }

        for (sta, list) in actions {
            for a in list {
                match a {
                    Action::Send(b) => {
                        if let Err(e) = eap.send(sta, &b) {
                            log(format!("AP: EAPOL send to {} failed: {e}", mac_text(&sta)));
                        }
                    }
                    Action::InstallPtk { peer, tk } => match ap.install_ptk(ifindex, peer, &tk) {
                        Ok(()) => {
                            sum.keys_installed.push(peer);
                            log(format!("AP: NEW_KEY pairwise installed for {}", mac_text(&peer)));
                        }
                        Err(e) => log(format!("AP: installing the pairwise key failed: {}", nlerr("NEW_KEY", e, &nl))),
                    },
                    Action::Authorized => match ap.authorize_station(ifindex, sta) {
                        Ok(()) => {
                            dhcp_srv.station_joined(sta);
                            sum.authorized.push(sta);
                            log(format!("AP: authorized {}", mac_text(&sta)));
                        }
                        Err(e) => log(format!("AP: authorizing failed: {}", nlerr("SET_STATION", e, &nl))),
                    },
                    Action::Failed(why) => {
                        let text = match why {
                            FailReason::WrongPassphrase => "wrong passphrase",
                            FailReason::Timeout => "timeout",
                            FailReason::BadRsnIe => "bad RSN element",
                        };
                        log(format!("AP: handshake failed for {}: {text}", mac_text(&sta)));
                        sum.failed.push((sta, text.to_string()));
                        stations.remove(&sta);
                        let _ = ap.remove_station(ifindex, sta, mgmt::REASON_4WAY_TIMEOUT);
                    }
                }
            }
        }

        // DHCP
        if ready[2] {
            let mut buf = [0u8; 1500];
            while let Ok((n, _from)) = udp.recv_from(&mut buf) {
                if let Some(rep) = dhcp_srv.handle(&buf[..n], t0.elapsed().as_secs()) {
                    let to = match rep.dest {
                        Dest::Broadcast => SocketAddrV4::new(Ipv4Addr::BROADCAST, 68),
                        Dest::Unicast(ip) => SocketAddrV4::new(ip, 68),
                    };
                    if let Err(e) = udp.send_to(&rep.bytes, to) {
                        log(format!("AP: DHCP send failed: {e}"));
                    }
                    if let Some((t, addr)) = dhcp::reply_summary(&rep.bytes) {
                        log(format!("AP: dhcp reply type {t} address {addr} for {}", mac_text(&rep.client_mac)));
                        if t == 5 {
                            sum.dhcp_acks.push((rep.client_mac, addr));
                        }
                    }
                }
            }
        }
    }

    let _ = ap.stop_ap(ifindex);
    let _ = ap.set_station_mode(ifindex);
    log(format!("AP: stopped; associated {} authorized {} failed {}", sum.associated.len(), sum.authorized.len(), sum.failed.len()));
    Ok(sum)
}
