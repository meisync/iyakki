//! Read-only: asks the kernel what every Wi-Fi radio can do and whether it could host a hotspot (no radio is changed).
//! Needs no special rights. Run it on any machine: it only sends GET_WIPHY and GET_REG.
use iyakki_agent::nl80211_os::{probe, Nl80211Socket};
use iyakki_hotspot::caps::{can_host_ap, Want, BAND_2GHZ, BAND_5GHZ};

fn main() {
    let mut s = match Nl80211Socket::open(std::time::Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => {
            println!("no netlink: {e}");
            return;
        }
    };
    let p = match probe(&mut s) {
        Ok(p) => p,
        Err(e) => {
            println!("probe failed: {e}");
            return;
        }
    };
    println!("country {} ; {} radio(s)", p.country, p.phys.len());
    if p.phys.is_empty() {
        println!("{}", iyakki_hotspot::caps::no_card_reason(iyakki_agent::nl80211_os::wifi_hardware_seen()));
    }
    for c in &p.phys {
        println!("{} driver={:?} iftypes={:?} ciphers={:x?} max_ap_sta={:?} ap_sme={} self_managed_reg={}", c.name, c.driver, c.iftypes, c.ciphers, c.max_ap_stations, c.ap_sme, c.self_managed_reg);
        for b in &c.bands {
            let open = b.channels.iter().filter(|x| x.usable_for_ap()).count();
            println!("  band {}: {} channels, {} usable for an AP now", b.id, b.channels.len(), open);
        }
        for k in &c.combos {
            println!("  combination {:?} total<={} channels<={}", k.limits.iter().map(|l| (l.max, l.types.clone())).collect::<Vec<_>>(), k.max_total, k.num_channels);
        }
        for (name, band, conc) in [("2.4 GHz alone", BAND_2GHZ, false), ("2.4 GHz with client", BAND_2GHZ, true), ("5 GHz alone", BAND_5GHZ, false)] {
            let w = Want { concurrent_sta: conc, ..Want::new(band, &p.country) };
            match can_host_ap(c, &w) {
                Ok(plan) => println!("  {name}: yes {plan:?}"),
                Err(r) => println!("  {name}: no - {} ({:?})", r.reason, r.fix),
            }
        }
    }
}
