//! The machine's network links, radio switches, screen brightness, sound cards, private-address facts and Bluetooth hardware, as the
//! core serves them to the hardware pages of Settings (UI/OS split plan, F-ARCH-06). The data types and pure helpers are those of
//! `iyakki-devctl` and are re-exported here; reading the kernel and switching things go through [`Hardware`]: the device has
//! `iyakki_core_local::LocalHardware`, a test has `iyakki_core_fake::FakeHardware`.

pub use iyakki_devctl::{hostname_ok, btstate, mac, radio_on, Backlight, Card, Control, Link, LinkKind, Rfkill, Roots, Screen, Volume, BROADCOM_NO_DRIVER_TEXT};

/// What the Network, Display, Sound and Overview pages read and change on the machine.
pub trait Hardware: Send + Sync {
    /// Ethernet and Wi-Fi interfaces.
    fn links(&self) -> Vec<Link>;
    /// Addresses of the interfaces (interface name, IPv4).
    fn ipv4(&self) -> Vec<(String, String)>;
    /// The radio switches (rfkill).
    fn rfkill(&self) -> Vec<Rfkill>;
    /// Block (`blocked`) or free one radio switch.
    fn set_rfkill(&self, id: &str, blocked: bool) -> Result<(), String>;
    /// A Broadcom Wi-Fi card that has no driver is in the machine.
    fn broadcom_wifi_without_driver(&self) -> bool;
    /// The framebuffer, the connected outputs and the backlights.
    fn screen(&self) -> Screen;
    /// Set one backlight to a percentage.
    fn set_brightness(&self, name: &str, percent: u32) -> Result<(), String>;
    /// Sound cards and the playback volume of each (when it has one).
    fn cards(&self) -> Vec<(Card, Option<Volume>)>;
    /// The private-address facts of each interface under this configuration.
    fn mac_infos(&self, cfg: &mac::MacConfig) -> Vec<mac::MacInfo>;
    /// The Bluetooth hardware as the kernel shows it now.
    fn bluetooth(&self) -> btstate::BtState;
    /// A Mac whose CS8409 sound codec has no driver: the line to show.
    fn cs8409_line(&self) -> Option<String>;
    /// Change the host name of the running system and write it to `etc_hostname` (a change: it asks every time).
    fn set_hostname(&self, name: &str, etc_hostname: &std::path::Path) -> Result<(), String>;
    /// One line about the private address of the link in use (`MAC: ...`), when there is a link.
    fn mac_summary(&self) -> Option<String>;
    /// The Hardware page of System as text lines: the machine report, the PCI devices with their drivers and the loaded drivers.
    fn hardware_report(&self) -> Vec<String>;
    /// First use of a hardware class (`audio`, `gpu`, `bluetooth`) by `who` (an app id or `core`): the core's lazy device broker
    /// (F-BOOT-04) loads the class's drivers once when the asker is allowed, and answers with one plain sentence. With the broker
    /// off (the default) every driver is loaded already and the answer says so.
    fn request_class(&self, class: &str, who: &str) -> String;
    /// The playback volume of one sound card (a read; `None` when the card has no volume control).
    fn volume(&self, _card: u32) -> Option<Volume> {
        None
    }
    /// Set the playback level of a card (0 to 100) and give back what the mixer says afterwards (`system.volume.set`).
    fn set_volume(&self, _card: u32, _percent: u32) -> Result<Volume, String> {
        Err(NOT_HERE.into())
    }
    /// Move the level of a card by `delta` percent points, or toggle its mute when `delta` is `None` (`system.volume.set`).
    fn change_volume(&self, _card: u32, _delta: Option<i32>) -> Result<Volume, String> {
        Err(NOT_HERE.into())
    }
    /// Bring a network interface up or down (`system.link.set`; the person's tap or the chat card is the approval).
    fn set_link(&self, _name: &str, _up: bool) -> Result<(), String> {
        Err(NOT_HERE.into())
    }
    /// Ask for an address on an interface that was just brought up (`system.link.set`).
    fn dhcp(&self, _name: &str) -> Result<(), String> {
        Err(NOT_HERE.into())
    }
}

/// What a machine without a way to do it answers.
pub const NOT_HERE: &str = "not available here";
