//! The machine reads of the Mac firmware job (F-ARCH-07): the wireless interfaces and Bluetooth controller under a `/sys` folder, the
//! controller addresses the kernel's management interface lists, the board's DMI text, and the loaded-module list. The job itself (what to
//! download, unpack and install) stays in `iyakki-agent::macfw`; it calls these and keeps its own result types.

use std::path::Path;

/// The program registers what "Load the Bluetooth driver anyway" does and how to tell a driver is held back (the driver packs live in a crate above
/// the device controls; set once, the first call wins).
pub fn register_bt_loader(held: fn() -> bool, load: fn() -> Result<String, String>) {
    crate::devctl::set_bt_loader(held, load);
}

/// What the machine shows under a `/sys` folder: Wi-Fi interface names, whether a Bluetooth controller exists and its address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wireless {
    pub wifi: Vec<String>,
    pub bt_controller: bool,
    pub bt_address: Option<String>,
}

/// The Wi-Fi interfaces and Bluetooth controller under a `/sys` folder. An all-zero address is no address.
pub fn wireless_in(sys: &Path) -> Wireless {
    let names = |d: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(sys.join(d)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    let wifi = names("class/net").into_iter().filter(|n| sys.join("class/net").join(n).join("wireless").exists() || sys.join("class/net").join(n).join("phy80211").exists()).collect();
    let hci = names("class/bluetooth").into_iter().find(|n| n.starts_with("hci") && !n.contains(':'));
    let bt_address = hci.as_ref().and_then(|h| std::fs::read_to_string(sys.join("class/bluetooth").join(h).join("address")).ok()).map(|a| a.trim().to_string()).filter(|a| !a.is_empty() && a != "00:00:00:00:00:00");
    Wireless { wifi, bt_controller: hci.is_some(), bt_address }
}

/// The addresses of the Bluetooth controllers the kernel's management interface lists (needs root). Current kernels have no `address`
/// file under `/sys/class/bluetooth/hci0`, and a controller that did not finish its setup is not in this list.
pub fn mgmt_addresses() -> Vec<String> {
    use iyakki_devctl::btmgmt::{ControlSocket, Mgmt};
    let Ok(sock) = ControlSocket::open() else { return Vec::new() };
    let mut m = Mgmt::new(sock);
    let idx = m.index_list().unwrap_or_default();
    idx.into_iter().filter_map(|i| m.info(i).ok()).map(|i| i.address).filter(|a| !a.is_empty() && a != "00:00:00:00:00:00").collect()
}

/// One DMI text of the board under a `/sys` folder (`product_name`, `board_name`, `sys_vendor`), trimmed; empty when missing.
pub fn dmi_text(sys: &Path, name: &str) -> String {
    std::fs::read_to_string(sys.join("class/dmi/id").join(name)).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Is the kernel module `name` loaded, by the module list under a `/proc` folder?
pub fn module_loaded(proc: &Path, name: &str) -> bool {
    std::fs::read_to_string(proc.join("modules")).unwrap_or_default().lines().any(|l| l.split_whitespace().next() == Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn reads_a_made_up_machine() {
        let d = std::env::temp_dir().join(format!("iyakki-macfwsys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        put(&d, "class/net/wlan0/wireless/x", b"");
        put(&d, "class/net/eth0/device/x", b"");
        put(&d, "class/bluetooth/hci0/address", b"00:00:00:00:00:00\n");
        put(&d, "class/dmi/id/product_name", b"MacBookPro14,1\n");
        put(&d, "modules", b"brcmfmac 1 0 - Live 0x0\nhci_uart 1 0 - Live 0x0\n");
        let w = wireless_in(&d);
        assert_eq!((w.wifi.clone(), w.bt_controller, w.bt_address.clone()), (vec!["wlan0".to_string()], true, None));
        put(&d, "class/bluetooth/hci0/address", b"AC:DE:48:00:11:22\n");
        assert_eq!(wireless_in(&d).bt_address.as_deref(), Some("AC:DE:48:00:11:22"));
        assert_eq!(dmi_text(&d, "product_name"), "MacBookPro14,1");
        assert_eq!(dmi_text(&d, "board_name"), "");
        assert!(module_loaded(&d, "brcmfmac") && !module_loaded(&d, "brcmfmac_wcc"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
