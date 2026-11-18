//! Is the connection the machine uses to reach the internet one that is paid by the amount (a phone shared by USB, a mobile modem)? The periodic update check
//! (`store::updatecheck::due`) waits on such a link unless the administrator allowed it. Nothing here asks a router: the answer is from the kind of
//! device that holds the default route, and `None` when that does not say (Wi-Fi may be a phone's hotspot, which the machine cannot tell from a home router).

/// What the rule reads about the interface that holds the default route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    /// The kernel driver's name (`rndis_host`, `ipheth`, `e1000e`, ...); empty when unknown.
    pub driver: String,
    pub wireless: bool,
}

/// Drivers of phones shared over a cable and of mobile modems.
const METERED_DRIVERS: [&str; 9] = ["rndis_host", "ipheth", "qmi_wwan", "cdc_mbim", "huawei_cdc_ncm", "sierra_net", "cdc_ncm_wwan", "option", "hso"];

/// Pure: `Some(true)` for a phone tether or a mobile modem, `Some(false)` for a wired Ethernet card, `None` when the link does not say (Wi-Fi, no default route).
pub fn metered(link: Option<&Link>) -> Option<bool> {
    let l = link?;
    let driver = l.driver.replace('-', "_");
    if METERED_DRIVERS.contains(&driver.as_str()) || l.name.starts_with("wwan") || l.name.starts_with("ppp") {
        return Some(true);
    }
    if l.wireless {
        return None;
    }
    (l.name.starts_with("eth") || l.name.starts_with("en")).then_some(false)
}

/// The interface holding the IPv4 default route, from `/proc/net/route` text and a function that gives an interface's driver and whether it is wireless.
pub fn default_link(route_text: &str, info: &dyn Fn(&str) -> (String, bool)) -> Option<Link> {
    let mut best: Option<(u32, String)> = None;
    for l in route_text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 8 || f[1] != "00000000" || f[7] != "00000000" || u32::from_str_radix(f[3], 16).unwrap_or(0) & 1 == 0 {
            continue;
        }
        let metric = f[6].parse::<u32>().unwrap_or(u32::MAX);
        if best.as_ref().is_none_or(|(m, _)| metric < *m) {
            best = Some((metric, f[0].to_string()));
        }
    }
    let (_, name) = best?;
    let (driver, wireless) = info(&name);
    Some(Link { name, driver, wireless })
}

/// The machine now (asks the one module that reads the routing table and the interfaces: `iyakki_devctl`): for `updatecheck::due`.
pub fn metered_now() -> Option<bool> {
    let roots = iyakki_devctl::Roots::real();
    let link = default_link(&iyakki_devctl::route_table(&roots), &|n| iyakki_devctl::iface_info(&roots, n));
    metered(link.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str, driver: &str, wireless: bool) -> Option<Link> {
        Some(Link { name: name.into(), driver: driver.into(), wireless })
    }

    #[test]
    fn a_phone_on_a_cable_and_a_mobile_modem_are_metered_a_wired_card_is_not_and_wifi_is_unknown() {
        assert_eq!(metered(link("enx0a1b2c", "rndis_host", false).as_ref()), Some(true));
        assert_eq!(metered(link("eth1", "ipheth", false).as_ref()), Some(true));
        assert_eq!(metered(link("wwan0", "qmi_wwan", false).as_ref()), Some(true));
        assert_eq!(metered(link("ppp0", "", false).as_ref()), Some(true));
        assert_eq!(metered(link("eno1", "e1000e", false).as_ref()), Some(false));
        assert_eq!(metered(link("wlan0", "brcmfmac", true).as_ref()), None, "a phone's hotspot looks like any router");
        assert_eq!(metered(None), None, "no default route: cannot tell");
    }

    #[test]
    fn the_link_with_the_lowest_metric_default_route_is_the_one_read() {
        let text = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                    wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                    usb0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                    lo\t0000007F\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n";
        let info = |n: &str| if n == "usb0" { ("rndis_host".to_string(), false) } else { ("brcmfmac".to_string(), true) };
        let l = default_link(text, &info).unwrap();
        assert_eq!(l.name, "usb0");
        assert_eq!(metered(Some(&l)), Some(true));
        assert!(default_link("Iface\tDestination\n", &info).is_none());
    }

    #[test]
    fn a_metered_link_holds_the_daily_check_back_unless_the_administrator_allowed_it() {
        use crate::store::updatecheck::{due, CheckSettings, Due, Mode};
        let s = CheckSettings { mode: Mode::Daily, metered_ok: false, ..CheckSettings::default() };
        assert_eq!(due(&s, 0, 1_000_000, metered(link("usb0", "rndis_host", false).as_ref()), "dev"), Due::Metered);
        assert_eq!(due(&s, 0, 1_000_000, metered(link("eno1", "e1000e", false).as_ref()), "dev"), Due::Now);
        let ok = CheckSettings { metered_ok: true, ..s };
        assert_eq!(due(&ok, 0, 1_000_000, Some(true), "dev"), Due::Now);
    }
}
