//! Joining a WiFi network and the known-network store as a typed local module (F-ARCH-07): the chat tools of Settings -> Network
//! (`wifi_join`, `wifi_forget`, `wifi_status`) take the data types and the one join path from here and never name the device-control
//! crate. The join path is the one the Network page uses: the entry is written (newest first), the private address is applied when one
//! is chosen, the supplicant's file is written and the supplicant asked to read it.

pub use iyakki_devctl::mac::MacEnv;
pub use iyakki_devctl::wifi::{Ap, Known, KnownStore, Security, Wifi};
pub use iyakki_devctl::wifi_ext::{EapMethod, Enterprise};
/// The name of the interface the Bluetooth network access point makes.
pub const BT_NAP_IFACE: &str = iyakki_devctl::BT_NAP_IFACE;

/// The plain warning said before a WEP network (broken encryption) is joined.
pub fn wep_warning() -> &'static str {
    iyakki_devctl::wifi_ext::wep_warning()
}

/// Join `k` on `iface` now and remember it. Returns the sentence to show.
pub fn join(w: &Wifi, store: &KnownStore, mac: &MacEnv, iface: &str, k: Known) -> Result<String, String> {
    iyakki_devctl::wifi::join_now(w, store, mac, iface, k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn made_up(name: &str) -> (std::path::PathBuf, Wifi, KnownStore) {
        let d = std::env::temp_dir().join(format!("iyakki-wifijoin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let w = Wifi { run_dir: d.join("run"), path: Some(d.join("bin")) };
        let store = KnownStore::new(&d.join("wifi.json"));
        (d, w, store)
    }

    #[test]
    fn the_entry_is_remembered_first_and_a_bad_interface_name_stops_before_any_program_runs() {
        let (d, w, store) = made_up("bad-iface");
        let mac = MacEnv::nowhere(Arc::new(iyakki_devctl::System));
        let k = Known::new("Cafe", "a long enough passphrase", Security::Wpa2, false, 10).unwrap();
        let err = join(&w, &store, &mac, "bad name", k).unwrap_err();
        assert!(err.contains("not an interface name"), "{err}");
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ssid, "Cafe");
        assert!(!w.run_dir.exists(), "nothing was written for the supplicant");
        // a second network goes first
        let k2 = Known::new("Home", "another passphrase here", Security::Wpa2, false, 20).unwrap();
        let _ = join(&w, &store, &mac, "bad name", k2);
        assert_eq!(store.list().first().map(|k| k.ssid.as_str()), Some("Home"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_wep_warning_and_the_bluetooth_interface_name_are_the_device_crates_words() {
        assert!(wep_warning().to_lowercase().contains("wep"));
        assert!(!BT_NAP_IFACE.is_empty());
    }
}
