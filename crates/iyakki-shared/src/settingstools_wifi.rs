//! Types moved out of `iyakki_agent::settingstools_wifi` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::sync::Arc;
use iyakki_devctl::mac::MacEnv;
use iyakki_devctl::wifi::KnownStore;
use iyakki_devctl::wifi::Wifi;

pub const PROMPT_RULE: &str = "\nWIFI (Settings -> Network, administrators): wifi_status lists the saved networks and the adapter, wifi_scan lists the networks in range with signal, security and band (both free). wifi_join joins and remembers a network (asks; a saved or open network needs only the name; for WPA, WPA3, WEP and enterprise networks first call vault_request for the passphrase, password or key and pass its entry name as secret_entry; never ask for or type it in the chat. An enterprise network also needs identity, ca_cert (a full path of the authority file that checks the network) and optionally method (PEAP, TTLS or TLS), server_name, client_cert and private_key. WEP is broken: only with accept_wep_risk true after the person has read the warning). wifi_forget removes a saved network (asks). HOTSPOT: hotspot_status says the hotspot's name, band, country and security (never the password; free); hotspot_set changes the country (two capital letters, empty for the card's own rules), the security (wpa2, transition or wpa3; a card that cannot do WPA3 itself keeps WPA2) and the band (auto, 2.4 or 5); it asks, and it applies the next time sharing starts.";

/// What the WiFi tools act on: the programs that scan and join, the saved networks, the machine's links and the private-address settings.
#[derive(Clone)]
pub struct WifiCtl {
    pub wifi: Arc<Wifi>,
    pub known: Arc<KnownStore>,
    pub roots: iyakki_devctl::Roots,
    pub mac: MacEnv,
}

impl WifiCtl {
    pub fn real() -> WifiCtl {
        WifiCtl { wifi: Arc::new(Wifi::real()), known: Arc::new(KnownStore::new(std::path::Path::new("/var/lib/iyakki/wifi.json"))), roots: iyakki_devctl::Roots::real(), mac: MacEnv::real() }
    }

    /// Points at nothing (unit tests of other crates never scan the real air or touch the real files).
    pub fn offline() -> WifiCtl {
        WifiCtl {
            wifi: Arc::new(Wifi { run_dir: "/nonexistent/run".into(), path: Some("/nonexistent/bin".into()) }),
            known: Arc::new(KnownStore::new(std::path::Path::new("/nonexistent/wifi.json"))),
            roots: iyakki_devctl::Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() },
            mac: MacEnv::real(),
        }
    }

    pub fn iface(&self) -> Option<String> {
        iyakki_devctl::links(&self.roots).into_iter().find(|l| l.kind == iyakki_devctl::LinkKind::Wifi).map(|l| l.name)
    }

    pub fn no_adapter(&self) -> String {
        if iyakki_devctl::broadcom_wifi_without_driver(&self.roots) { iyakki_devctl::BROADCOM_NO_DRIVER_TEXT.to_string() } else { "This computer has no WiFi adapter.".to_string() }
    }
}

/// Reads the text of a vault entry of the signed-in person (the shell gives the real one; it fails when the vault is locked).
pub type SecretReader = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;
