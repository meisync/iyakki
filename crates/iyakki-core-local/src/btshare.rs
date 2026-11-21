//! The Bluetooth sharing side of the device crate as typed local calls (F-ARCH-07): the interface names, whether a powered controller
//! exists, the visibility to phones through the kernel's management channel, and the access-point service handle. Internet sharing
//! (`sharing::real`) calls these and never names the device-control crate or opens the management socket itself.

use std::path::Path;
use std::sync::Arc;

use iyakki_devctl::btmgmt::{ControlSocket, Mgmt, Transport};

/// The interface a Bluetooth network connection to a phone makes (this computer as the client).
pub const PAN_IFACE: &str = iyakki_devctl::BT_PAN_IFACE;
/// The interface the Bluetooth access point makes (phones connect to this computer).
pub const NAP_IFACE: &str = iyakki_devctl::BT_NAP_IFACE;

/// Is there a powered controller, under the given `/sys` and `/proc` roots? `Err` is the reason in plain words.
pub fn controller_ready_in(sys: &Path, proc_: &Path) -> Result<(), String> {
    iyakki_devctl::bt_pan::controller_state(&iyakki_devctl::Roots { sys: sys.to_path_buf(), proc_: proc_.to_path_buf() }).map(|_| ())
}

/// Is there a powered controller on this computer?
pub fn controller_ready() -> Result<(), String> {
    controller_ready_in(Path::new("/sys"), Path::new("/proc"))
}

/// Let phones find and pair with the first controller (`on`, for `secs` seconds) or take that away again, over any transport.
pub fn set_visible_with<T: Transport>(m: &mut Mgmt<T>, on: bool, secs: u16) -> Result<(), String> {
    let index = m.index_list().map_err(|e| format!("{e:?}"))?.first().copied().ok_or("no controller")?;
    if on {
        m.open_to_phones(index, secs).map(|_| ()).map_err(|e| format!("{e:?}"))
    } else {
        m.set_discoverable(index, false, 0).map(|_| ()).map_err(|e| format!("{e:?}"))
    }
}

/// The same on this computer's management socket (needs root; the error says why it could not).
pub fn set_visible(on: bool, secs: u16) -> Result<(), String> {
    let s = ControlSocket::open().map_err(|e| e.to_string())?;
    set_visible_with(&mut Mgmt::new(s), on, secs)
}

/// The Bluetooth access-point service of this process.
#[derive(Clone)]
pub struct Nap {
    svc: Arc<iyakki_devctl::nap_server::NapService>,
}

impl Nap {
    /// The one service of this process.
    pub fn shared() -> Nap {
        Nap { svc: iyakki_devctl::nap_server::service() }
    }
    /// Whether phones may reach each other.
    pub fn phones_see_each_other(&self, on: bool) {
        self.svc.set_phones_see_each_other(on);
    }
    pub fn set_on(&self, on: bool) {
        self.svc.set_on(on);
    }
    /// Why it could not run, in words (empty: no complaint).
    pub fn why_not(&self) -> String {
        self.svc.view().why_not
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_devctl::btmgmt::{decode, encode, EV_CMD_COMPLETE, OP_READ_INDEX_LIST, OP_READ_INFO, OP_SET_DISCOVERABLE};
    use std::collections::VecDeque;
    use std::io;
    use std::time::Duration;

    /// Answers every command with a successful Command Complete; the index list names `controllers` controllers.
    struct Auto {
        sent: Vec<u16>,
        inbox: VecDeque<Vec<u8>>,
        controllers: u16,
    }

    impl Transport for Auto {
        fn send(&mut self, p: &[u8]) -> io::Result<()> {
            let pkt = decode(p).unwrap();
            self.sent.push(pkt.code);
            let data: Vec<u8> = match pkt.code {
                OP_READ_INDEX_LIST => {
                    let mut d = self.controllers.to_le_bytes().to_vec();
                    for i in 0..self.controllers {
                        d.extend_from_slice(&i.to_le_bytes());
                    }
                    d
                }
                OP_READ_INFO => vec![0u8; 280],
                _ => vec![0u8; 4],
            };
            let mut body = pkt.code.to_le_bytes().to_vec();
            body.push(0);
            body.extend_from_slice(&data);
            self.inbox.push_back(encode(EV_CMD_COMPLETE, pkt.index, &body));
            Ok(())
        }
        fn recv(&mut self, _w: Duration) -> io::Result<Option<Vec<u8>>> {
            Ok(self.inbox.pop_front())
        }
    }

    fn auto(controllers: u16) -> Mgmt<Auto> {
        Mgmt::new(Auto { sent: vec![], inbox: VecDeque::new(), controllers })
    }

    #[test]
    fn taking_visibility_away_asks_the_first_controller_to_stop_being_discoverable_only() {
        let mut m = auto(1);
        set_visible_with(&mut m, false, 180).unwrap();
        assert_eq!(m.transport().sent, vec![OP_READ_INDEX_LIST, OP_SET_DISCOVERABLE]);
    }

    #[test]
    fn opening_to_phones_prepares_the_controller_and_ends_with_discoverable() {
        let mut m = auto(2);
        set_visible_with(&mut m, true, 180).unwrap();
        let sent = &m.transport().sent;
        assert_eq!(sent.first(), Some(&OP_READ_INDEX_LIST));
        assert_eq!(sent.last(), Some(&OP_SET_DISCOVERABLE));
        assert!(sent.len() > 3, "{sent:?}");
    }

    #[test]
    fn no_controller_is_said_in_words_and_nothing_else_is_sent() {
        let mut m = auto(0);
        assert_eq!(set_visible_with(&mut m, true, 180), Err("no controller".to_string()));
        assert_eq!(m.transport().sent, vec![OP_READ_INDEX_LIST]);
    }

    #[test]
    fn a_made_up_machine_without_bluetooth_says_it_has_no_controller_and_the_names_are_fixed() {
        let d = std::env::temp_dir().join(format!("iyakki-btshare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sys")).unwrap();
        std::fs::create_dir_all(d.join("proc")).unwrap();
        assert!(controller_ready_in(&d.join("sys"), &d.join("proc")).unwrap_err().contains("no Bluetooth controller"));
        assert!(!PAN_IFACE.is_empty() && !NAP_IFACE.is_empty() && PAN_IFACE != NAP_IFACE);
        let _ = std::fs::remove_dir_all(&d);
    }
}
