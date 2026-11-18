//! Which network drivers belong to a cable that joins two computers directly (no DHCP server on such a cable). Shared by the
//! boot-time network setup (which must not start a DHCP client on them) and `iyakki_agent::directlink` (which brings them up).

/// Host-to-host USB bridge cables: Prolific PL-2301/2302/25A1/27A1 (`plusb`), NetChip (`net1080`), the CDC "subset" links.
pub const BRIDGE_DRIVERS: [&str; 3] = ["plusb", "net1080", "cdc_subset"];

/// The interface is the end of a direct link whose address the link manager chooses (Thunderbolt/USB4 `thunderbolt-net`, or a
/// bridge cable). USB network class drivers (cdc_ether, ...) are not listed here: they may be an ordinary adapter, so DHCP gets
/// its chance first.
pub fn is_direct_link(name: &str, driver: &str) -> bool {
    let d = driver.replace('-', "_");
    name.starts_with("thunderbolt") || d == "thunderbolt_net" || BRIDGE_DRIVERS.contains(&d.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thunderbolt_and_bridge_cables_are_direct_ordinary_cards_are_not() {
        assert!(is_direct_link("thunderbolt0", ""));
        assert!(is_direct_link("eth5", "thunderbolt-net"));
        assert!(is_direct_link("enx0011", "plusb") && is_direct_link("usb1", "net1080") && is_direct_link("enx", "cdc_subset"));
        assert!(!is_direct_link("eth0", "e1000e") && !is_direct_link("enx0011", "cdc_ncm") && !is_direct_link("wlan0", "iwlwifi"));
    }
}
