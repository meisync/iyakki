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
//! pieces with its own timers, settings and firewall; link and address set-up here go through the routing netlink (`crate::rtnl`).

use crate::softap::{KernelIo, SoftAp, SoftApConfig, SoftEvent};
use crate::nl80211_os::{poll_readable, udp_on_interface, EapolChannel, Nl80211Socket, PacketEapol, SharedNl};
use iyakki_hotspot::dhcp::{self, DhcpConfig, DhcpServer, Dest};
use iyakki_hotspot::nl80211::{self, ApControl, ApParams, Event};
use iyakki_hotspot::rsn::{rsn_ie, SecurityMode};
use iyakki_hotspot::{caps, eapol, mgmt, Mac};
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
    /// What the beacon announces: WPA2, WPA2 and WPA3 together, or WPA3 only. WPA3 runs the SAE exchange in `hotspot_sae` on the
    /// authentication frames the kernel hands over (plain SAE, no hash-to-element: D-HOTSPOT-22).
    pub security: SecurityMode,
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
    iyakki_core_local::net::iface_attr(ifname, file).ok_or_else(|| format!("{ifname}: {file} cannot be read"))
}

fn parse_mac(s: &str) -> Option<Mac> {
    let v: Vec<u8> = s.split(':').filter_map(|p| u8::from_str_radix(p, 16).ok()).collect();
    v.try_into().ok()
}

fn random(buf: &mut [u8]) -> Result<(), String> {
    iyakki_sec::random_fill(buf).map_err(|e| format!("random bytes: {e}"))
}

/// Run one access point for `cfg.run_secs` seconds (or until `stop` says so), logging every step through `log`.
pub fn run(cfg: &RadioConfig, log: &mut dyn FnMut(String), stop: &dyn Fn() -> bool) -> Result<Summary, String> {
    let freq = channel_freq(cfg.channel).ok_or("unknown channel")?;
    iyakki_hotspot::settings::validate_ssid(&cfg.ssid).map_err(|e| format!("{e:?}"))?;
    iyakki_hotspot::settings::validate_passphrase(&cfg.passphrase).map_err(|e| format!("{e:?}"))?;
    let ifindex: u32 = sysfs(&cfg.ifname, "ifindex")?.parse().map_err(|_| "ifindex".to_string())?;
    let bssid = parse_mac(&sysfs(&cfg.ifname, "address")?).ok_or("interface address")?;
    let ssid = cfg.ssid.as_bytes().to_vec();
    // the element the beacon, the probe answer and message 3 of every handshake carry
    let rsn = if cfg.security == SecurityMode::Wpa2 { eapol::rsn_ie_ccmp_psk() } else { rsn_ie(cfg.security) };

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
    let _ = crate::rtnl::set_link(&cfg.ifname, false);
    ap.set_ap_mode(ifindex).map_err(|e| nlerr("switching to access point mode", e, &nl))?;
    crate::rtnl::set_link(&cfg.ifname, true)?;
    crate::rtnl::add_address(&cfg.ifname, cfg.gateway, 24)?;

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
        beacon_tail: mgmt::beacon_tail_with(mgmt::Band::G24, &rsn),
        probe_resp: mgmt::probe_response_with(bssid, &ssid, 100, mgmt::Band::G24, cfg.channel, &rsn),
        // only with the capability: NL80211_FEATURE_INACTIVITY_TIMER (bit 2) in the feature flags
        inactivity_secs: if caps.feature_flags & (1 << 2) != 0 { 300 } else { 0 },
        pmk: None,
        security: cfg.security,
    };
    ap.start_ap(&params).map_err(|e| nlerr("starting the access point", e, &nl))?;
    let mut gtk = [0u8; 16];
    random(&mut gtk)?;
    ap.install_gtk(ifindex, 1, &gtk).map_err(|e| nlerr("installing the group key", e, &nl))?;
    // the integrity group key the handshake announces (WPA3 and the transition mode) is the kernel's too: one key for both
    let mut igtk = [0u8; 16];
    random(&mut igtk)?;
    if cfg.security != SecurityMode::Wpa2 {
        ap.install_igtk(ifindex, crate::wifi_ap_sw::IGTK_IDX, &igtk).map_err(|e| nlerr("installing the integrity group key", e, &nl))?;
    }
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
    let mut sum = Summary::default();
    // the frame handling (open authentication, WPA3 SAE, association, four-way handshake) is `softap`, shared with the product's runner
    let mut sae_secret = [0u8; 32];
    random(&mut sae_secret)?;
    let mut soft = SoftAp::new(
        SoftApConfig { bssid, ssid: ssid.clone(), passphrase: cfg.passphrase.clone(), band: mgmt::Band::G24, security: cfg.security, max_clients: cfg.max_clients, gtk, igtk },
        sae_secret,
    );
    debug_assert_eq!(soft.rsn_element(), &rsn[..]);

    while !stop() && t0.elapsed() < Duration::from_secs(cfg.run_secs) {
        let ready = poll_readable(&[nl.0.borrow().as_raw_fd(), eap.raw_fd(), std::os::fd::AsRawFd::as_raw_fd(&udp)], 100);
        let mut events: Vec<SoftEvent> = Vec::new();

        // kernel events: management frames, stations coming and going
        if ready[0] {
            let mut io = KernelIo { ap: &mut ap, eap: &mut eap, ifindex, freq_mhz: freq };
            loop {
                let ev = nl.0.borrow_mut().next_event(0).map_err(|e| e.to_string())?;
                let Some(raw) = ev else { break };
                match nl80211::parse_event(&raw) {
                    Some(Event::Frame { ifindex: i, frame, .. }) if i == ifindex => events.extend(soft.on_mgmt(&mut io, &frame, now_ms())),
                    Some(Event::Station { ifindex: i, sta, added }) if i == ifindex => {
                        log(format!("AP: kernel says station {} {}", mac_text(&sta), if added { "added" } else { "removed" }));
                        if !added {
                            events.extend(soft.on_station_removed(sta));
                        }
                    }
                    _ => {}
                }
            }
        }

        {
            let mut io = KernelIo { ap: &mut ap, eap: &mut eap, ifindex, freq_mhz: freq };
            // EAPOL from clients is read through the same channel the handshake answers on: collect first
            let mut packets = Vec::new();
            if ready[1] {
                while let Some(p) = io.eap.recv(0)? {
                    packets.push(p);
                }
            }
            for (sta, bytes) in packets {
                events.extend(soft.on_eapol(&mut io, sta, &bytes, now_ms()));
            }
            // handshake time-outs and resends
            events.extend(soft.tick(&mut io, now_ms()));
        }

        for e in events {
            match e {
                SoftEvent::Log(l) => log(format!("AP: {l}")),
                SoftEvent::Associated(m) => sum.associated.push(m),
                SoftEvent::KeysInstalled(m) => sum.keys_installed.push(m),
                SoftEvent::Authorized(m) => {
                    dhcp_srv.station_joined(m);
                    sum.authorized.push(m);
                }
                SoftEvent::Failed(m, why) => sum.failed.push((m, why)),
                SoftEvent::Left(m) => dhcp_srv.station_left(&m),
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
