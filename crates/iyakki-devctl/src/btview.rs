//! What the Bluetooth controllers report through the kernel's management interface, without a Bluetooth stack: the state the
//! Settings -> Bluetooth page and the Devices popover show, plus "Power on" and "Scan for devices" as the proof that a controller works.
//!
//! Why this exists: current kernels have no `address` file under `/sys/class/bluetooth/hci0`, so the old "no answer" chip (which looked for
//! that file) could not tell a controller that is silent from one that runs from its ROM. The management channel can: it lists the
//! controller, its address, HCI version, manufacturer, supported and current settings, and name (Read Controller Information), and a
//! controller whose address is not valid shows up as "unconfigured" (Read Extended Index List, Read Controller Configuration Information).
//!
//! Read-only plus power and scan. Nothing here pairs, connects or disconnects.

use std::time::Duration;

use crate::btmgmt::{self, ControlSocket, ControllerInfo, Error, Found, Mgmt, Transport, INDEX_NONE, OP_READ_CONFIG_INFO, OP_READ_EXT_INDEX_LIST, OP_READ_INDEX_LIST, SET_POWERED};

/// Whether a Bluetooth address is one a controller can really have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrState {
    Valid,
    /// `00:00:00:00:00:00`: the chip was never given an address.
    Zero,
    /// `AA:AA:AA:AA:AA:AA`: the placeholder some Broadcom chips report before a firmware file gives them a real one.
    Placeholder,
    /// The controller did not report an address at all (it is unconfigured).
    NotSet,
}

pub fn addr_state(addr: &str) -> AddrState {
    let a = addr.trim();
    if a.is_empty() {
        AddrState::NotSet
    } else if a.eq_ignore_ascii_case("00:00:00:00:00:00") {
        AddrState::Zero
    } else if a.eq_ignore_ascii_case("AA:AA:AA:AA:AA:AA") {
        AddrState::Placeholder
    } else {
        AddrState::Valid
    }
}

impl AddrState {
    pub fn valid(self) -> bool {
        self == AddrState::Valid
    }
}

/// What the page says about an address that is not valid, and what it means.
pub const NO_VALID_ADDRESS: &str = "no valid address: the chip has no Bluetooth address of its own until its firmware file gives it one, so the kernel treats the controller as not set up. Power on and Scan show whether it works at all; pairing needs a valid address and a Bluetooth stack";

/// What every Bluetooth place says (Settings, the Devices popover, the top bar's Devices button) for a controller that exists but runs in its ROM
/// state because its patch file is missing. The words are the way to the fix: the Setup card for Mac firmware is under Store, Setup, Hardware.
pub const NEEDS_FIRMWARE: &str = "Bluetooth needs firmware: Store \u{2192} Setup \u{2192} Hardware";

/// The company id Bluetooth gives Broadcom, whose chips in Macs load their patch RAM from a `.hcd` file at every start.
pub const BROADCOM: u16 = 15;

/// A Broadcom controller that has no valid address and whose patch file (`brcm/BCM.hcd`) is missing: the kernel lists it (`hci0`), but it
/// is in its ROM state and cannot be used for pairing or audio. A controller with a valid address works, whatever files exist.
pub fn needs_firmware(c: &Controller, firmware_file: bool) -> bool {
    c.manufacturer == BROADCOM && !c.addr_state().valid() && !firmware_file
}

/// The pairing/audio sentence the page always shows.
pub const NOT_YET: &str = "Pairing and audio are not available yet: they need a Bluetooth stack (decision pending in docs/BETA-PLAN.md). This page only reads the controller, powers it and scans";

/// One controller as the kernel's management interface describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controller {
    pub index: u16,
    /// Set up with a valid address (Read Info works). `false`: listed as unconfigured.
    pub configured: bool,
    pub address: String,
    pub version: Option<u8>,
    pub manufacturer: u16,
    pub supported: u32,
    pub current: u32,
    pub name: String,
}

impl Controller {
    pub fn hci(&self) -> String {
        format!("hci{}", self.index)
    }

    pub fn powered(&self) -> bool {
        self.current & SET_POWERED != 0
    }

    pub fn addr_state(&self) -> AddrState {
        if self.configured {
            addr_state(&self.address)
        } else {
            addr_state(&self.address).min_unset()
        }
    }

    pub fn from_info(i: ControllerInfo) -> Controller {
        Controller { index: i.index, configured: true, address: i.address, version: Some(i.version), manufacturer: i.manufacturer, supported: i.supported, current: i.current, name: i.name }
    }
}

impl AddrState {
    /// An unconfigured controller reports no address: whatever the text was, it is "not set" unless it is a known placeholder.
    fn min_unset(self) -> AddrState {
        match self {
            AddrState::Valid => AddrState::NotSet,
            other => other,
        }
    }
}

/// The company name for a Bluetooth company id (only the ones this project meets).
pub fn manufacturer_name(id: u16) -> String {
    match id {
        15 => "Broadcom".into(),
        2 => "Intel".into(),
        10 => "Qualcomm (CSR)".into(),
        29 => "Qualcomm".into(),
        93 => "Realtek".into(),
        70 => "MediaTek".into(),
        n => format!("company {n}"),
    }
}

/// `(type, bus)` entries of a Read Extended Index List reply: count (u16), then index (u16), type (u8: 0 configured, 1 unconfigured,
/// 2 AMP), bus (u8) for each. A count larger than the data is cut to the data; trailing bytes are ignored.
pub fn parse_ext_index_list(r: &[u8]) -> Result<Vec<(u16, u8, u8)>, Error> {
    if r.len() < 2 {
        return Err(Error::Protocol("extended index list"));
    }
    let n = u16::from_le_bytes([r[0], r[1]]) as usize;
    Ok(r[2..].chunks_exact(4).take(n).map(|c| (u16::from_le_bytes([c[0], c[1]]), c[2], c[3])).collect())
}

/// Read Controller Configuration Information: manufacturer (u16), supported options (u32), missing options (u32). Bit 0 of the options is
/// "external configuration", bit 1 is "set the public address".
pub fn parse_config_info(r: &[u8]) -> Result<(u16, u32, u32), Error> {
    if r.len() < 10 {
        return Err(Error::Protocol("configuration info"));
    }
    Ok((u16::from_le_bytes([r[0], r[1]]), u32::from_le_bytes([r[2], r[3], r[4], r[5]]), u32::from_le_bytes([r[6], r[7], r[8], r[9]])))
}

/// Every controller the kernel knows, configured or not. Falls back to the plain index list on a kernel that has no extended one. A
/// controller that cannot be read (it vanished, or is being set up) is left out rather than failing the whole list.
pub fn read_controllers<T: Transport>(m: &mut Mgmt<T>) -> Result<Vec<Controller>, Error> {
    let list: Vec<(u16, u8)> = match m.command(OP_READ_EXT_INDEX_LIST, INDEX_NONE, &[]) {
        Ok(r) => parse_ext_index_list(&r)?.into_iter().filter(|e| e.1 <= 1).map(|e| (e.0, e.1)).collect(),
        Err(Error::Status(1)) => {
            let r = m.command(OP_READ_INDEX_LIST, INDEX_NONE, &[])?;
            if r.len() < 2 {
                return Err(Error::Protocol("index list"));
            }
            let n = u16::from_le_bytes([r[0], r[1]]) as usize;
            r[2..].chunks_exact(2).take(n).map(|c| (u16::from_le_bytes([c[0], c[1]]), 0)).collect()
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for (index, ty) in list {
        if ty == 0 {
            if let Ok(i) = m.info(index) {
                out.push(Controller::from_info(i));
            }
        } else if let Ok(r) = m.command(OP_READ_CONFIG_INFO, index, &[]) {
            let (manufacturer, _supported, _missing) = parse_config_info(&r)?;
            out.push(Controller { index, configured: false, address: String::new(), version: None, manufacturer, supported: 0, current: 0, name: String::new() });
        }
    }
    Ok(out)
}

/// The first line of the page: `Controller hci0: BCM4364 (ROM state, no firmware file)`. `chip` is the chip name the machine model tells
/// (the kernel does not report it); `firmware_file` says whether the patch file the kernel asks for exists.
pub fn headline(c: &Controller, chip: Option<&str>, firmware_file: bool) -> String {
    let who = match chip {
        Some(n) => n.to_string(),
        None => manufacturer_name(c.manufacturer),
    };
    if firmware_file {
        format!("Controller {}: {who}", c.hci())
    } else if chip.is_some() {
        format!("Controller {}: {who} (ROM state, no firmware file)", c.hci())
    } else if c.manufacturer == BROADCOM {
        format!("Controller {}: {who} (no firmware file found)", c.hci())
    } else {
        // Intel, Realtek, MediaTek and the like load their own files from other folders: this check says nothing about them
        format!("Controller {}: {who}", c.hci())
    }
}

/// The second line: address, power, and (when the address is not valid) the reason.
pub fn detail(c: &Controller) -> String {
    let st = c.addr_state();
    let addr = match st {
        AddrState::NotSet => "address not set".to_string(),
        AddrState::Zero => format!("address {} (not set)", c.address),
        AddrState::Placeholder => format!("address {} (placeholder)", c.address),
        AddrState::Valid => format!("address {}", c.address),
    };
    let power = if !c.configured { "cannot be powered until it has an address" } else if c.powered() { "powered on" } else { "powered off" };
    if st.valid() {
        format!("{addr} · {power}")
    } else {
        format!("{addr} · {power} · {NO_VALID_ADDRESS}")
    }
}

/// Short chip text for the Devices popover.
pub fn short(c: &Controller, chip: Option<&str>, firmware_file: bool) -> String {
    let h = headline(c, chip, firmware_file);
    let st = c.addr_state();
    format!("{h} · {} · {}", if st.valid() { c.address.clone() } else { "no valid address".to_string() }, if !c.configured { "not set up" } else if c.powered() { "powered on" } else { "powered off" })
}

/// One device the scan saw: name (or the address when it has none), address, signal.
pub fn found_line(f: &Found) -> (String, String) {
    let title = if f.name.is_empty() { f.address.clone() } else { f.name.clone() };
    (title, format!("{} · {} dBm", f.address, f.rssi))
}

// ------------------------------------------------------------------------------------------------ the real machine

/// The controllers now (needs root). `Err` with a plain sentence when the management channel cannot be opened.
pub fn controllers() -> Result<Vec<Controller>, String> {
    let sock = ControlSocket::open().map_err(|e| format!("cannot open the Bluetooth management channel: {e} (needs root, and the Bluetooth drivers loaded)"))?;
    let mut m = Mgmt::new(sock);
    read_controllers(&mut m).map_err(|e| e.to_string())
}

/// Switch a controller on or off.
pub fn set_powered(index: u16, on: bool) -> Result<(), String> {
    let sock = ControlSocket::open().map_err(|e| format!("cannot open the Bluetooth management channel: {e}"))?;
    let mut m = Mgmt::new(sock);
    m.set_powered(index, on).map(|_| ()).map_err(|e| power_error(&e, on))
}

/// Scan for `secs` seconds (classic and low energy where the controller has them). The controller must be powered on.
pub fn scan(index: u16, secs: u64) -> Result<Vec<Found>, String> {
    let sock = ControlSocket::open().map_err(|e| format!("cannot open the Bluetooth management channel: {e}"))?;
    let mut m = Mgmt::new(sock);
    scan_with(&mut m, index, Duration::from_secs(secs))
}

/// [`scan`] on any transport (the tests use a scripted one).
pub fn scan_with<T: Transport>(m: &mut Mgmt<T>, index: u16, dur: Duration) -> Result<Vec<Found>, String> {
    let types = m.discovery_types(index).map_err(|e| scan_error(&e))?;
    if types == 0 {
        return Err("this controller reports neither classic nor low energy Bluetooth, so there is nothing to scan with".into());
    }
    m.scan_types(index, dur, types).map_err(|e| scan_error(&e))
}

fn power_error(e: &Error, on: bool) -> String {
    match e {
        Error::Status(17) => "the kernel will not power this controller: it has no valid address, so it is listed as not set up. A firmware file normally gives it one".into(),
        other => format!("could not switch the controller {}: {other}", if on { "on" } else { "off" }),
    }
}

fn scan_error(e: &Error) -> String {
    match e {
        Error::Status(15) => "the controller is off: use Power on first".into(),
        Error::Status(17) => "the kernel does not offer this controller for scanning: it has no valid address (not set up)".into(),
        other => format!("the scan did not work: {other}"),
    }
}

pub fn power_error_text(e: &btmgmt::Error, on: bool) -> String {
    power_error(e, on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use btmgmt::{encode, EV_CMD_COMPLETE, EV_DEVICE_FOUND, EV_DISCOVERING, OP_READ_INFO, OP_SET_BREDR, OP_SET_LE, OP_SET_POWERED, OP_START_DISCOVERY, OP_STOP_DISCOVERY, SET_BREDR, SET_LE};
    use std::collections::VecDeque;
    use std::io;

    struct Fake {
        sent: Vec<Vec<u8>>,
        inbox: VecDeque<Vec<u8>>,
        script: Vec<(u16, Vec<Vec<u8>>)>,
    }

    impl Transport for Fake {
        fn send(&mut self, p: &[u8]) -> io::Result<()> {
            self.sent.push(p.to_vec());
            let op = u16::from_le_bytes([p[0], p[1]]);
            if let Some(i) = self.script.iter().position(|(o, _)| *o == op) {
                let (_, replies) = self.script.remove(i);
                self.inbox.extend(replies);
            }
            Ok(())
        }
        fn recv(&mut self, _wait: Duration) -> io::Result<Option<Vec<u8>>> {
            Ok(self.inbox.pop_front())
        }
    }

    fn fake(script: Vec<(u16, Vec<Vec<u8>>)>) -> Mgmt<Fake> {
        Mgmt::new(Fake { sent: vec![], inbox: VecDeque::new(), script })
    }

    fn complete(op: u16, index: u16, status: u8, data: &[u8]) -> Vec<u8> {
        let mut p = op.to_le_bytes().to_vec();
        p.push(status);
        p.extend_from_slice(data);
        encode(EV_CMD_COMPLETE, index, &p)
    }

    /// What the Mac mini 2018 reports in its ROM state: Broadcom (15), HCI version 6, address all zero.
    fn rom_info(addr: [u8; 6], current: u32) -> Vec<u8> {
        let mut r = vec![0u8; 280];
        r[..6].copy_from_slice(&addr);
        r[6] = 6;
        r[7..9].copy_from_slice(&15u16.to_le_bytes());
        r[9..13].copy_from_slice(&(SET_POWERED | SET_BREDR | SET_LE).to_le_bytes());
        r[13..17].copy_from_slice(&current.to_le_bytes());
        r[20..26].copy_from_slice(b"iyakki");
        r[269..272].copy_from_slice(b"iya");
        r
    }

    fn ext_list(entries: &[(u16, u8, u8)]) -> Vec<u8> {
        let mut v = (entries.len() as u16).to_le_bytes().to_vec();
        for e in entries {
            v.extend_from_slice(&e.0.to_le_bytes());
            v.push(e.1);
            v.push(e.2);
        }
        v
    }

    #[test]
    fn the_fixtures_of_the_messages_encode_and_decode() {
        assert_eq!(encode(OP_READ_EXT_INDEX_LIST, INDEX_NONE, &[]), vec![0x3c, 0, 0xff, 0xff, 0, 0]);
        assert_eq!(encode(OP_READ_INDEX_LIST, INDEX_NONE, &[]), vec![3, 0, 0xff, 0xff, 0, 0]);
        assert_eq!(encode(OP_READ_INFO, 0, &[]), vec![4, 0, 0, 0, 0, 0]);
        assert_eq!(encode(OP_SET_POWERED, 0, &[1]), vec![5, 0, 0, 0, 1, 0, 1]);
        assert_eq!(encode(OP_START_DISCOVERY, 0, &[7]), vec![0x23, 0, 0, 0, 1, 0, 7]);
        assert_eq!(encode(OP_STOP_DISCOVERY, 0, &[7]), vec![0x24, 0, 0, 0, 1, 0, 7]);
        let p = btmgmt::decode(&complete(OP_READ_INFO, 0, 0, &[1, 2])).unwrap();
        assert_eq!((p.code, p.index, p.params), (EV_CMD_COMPLETE, 0, vec![4, 0, 0, 1, 2]));
    }

    #[test]
    fn the_info_reply_gives_address_version_manufacturer_settings_and_names() {
        let i = btmgmt::parse_info(0, &rom_info([0; 6], 0)).unwrap();
        assert_eq!((i.address.as_str(), i.version, i.manufacturer, i.name.as_str(), i.short_name.as_str()), ("00:00:00:00:00:00", 6, 15, "iyakki", "iya"));
        assert!(!i.powered());
        assert_eq!(i.supported, SET_POWERED | SET_BREDR | SET_LE);
        let i = btmgmt::parse_info(3, &rom_info([0x66, 0x55, 0x44, 0x33, 0x22, 0x11], SET_POWERED)).unwrap();
        assert_eq!((i.index, i.address.as_str(), i.powered()), (3, "11:22:33:44:55:66", true));
    }

    #[test]
    fn truncated_and_hostile_data_is_an_error_or_ignored_never_a_panic() {
        for n in [0, 1, 5, 6, 100, 279] {
            assert!(btmgmt::parse_info(0, &vec![0xff; n]).is_err(), "{n}");
        }
        // a name without a terminator and invalid UTF-8 is still text
        let mut r = vec![0xffu8; 280];
        r[6] = 9;
        assert!(btmgmt::parse_info(0, &r).is_ok());
        assert!(parse_ext_index_list(&[]).is_err() && parse_ext_index_list(&[1]).is_err());
        assert_eq!(parse_ext_index_list(&[200, 0, 1, 0, 0, 0]).unwrap(), vec![(1, 0, 0)], "a count larger than the data is cut to the data");
        assert_eq!(parse_ext_index_list(&[0, 0, 9, 9, 9]).unwrap(), vec![], "a count of zero ignores trailing bytes");
        assert!(parse_config_info(&[0; 9]).is_err());
        assert_eq!(parse_config_info(&[15, 0, 2, 0, 0, 0, 2, 0, 0, 0]).unwrap(), (15, 2, 2));
        // packets: shorter than the header, shorter than the declared length, a huge declared length
        assert!(btmgmt::decode(&[1, 2, 3]).is_err());
        assert!(btmgmt::decode(&[1, 0, 0, 0, 0xff, 0xff, 1]).is_err());
        // a found-device event with a lying EIR length, and one cut short
        let mut p = vec![0u8; 12];
        p.extend_from_slice(&500u16.to_le_bytes());
        p.extend_from_slice(&[1, 2, 3]);
        assert!(btmgmt::parse_found(&p).is_none());
        assert!(btmgmt::parse_found(&p[..13]).is_none());
        assert!(btmgmt::parse_found(&[]).is_none());
        // an EIR field that claims more than it has stops the reading, with no panic
        let (n, _, _, _) = btmgmt::parse_eir(&[200, 9, b'x']);
        assert_eq!(n, "");
        let (n, _, _, _) = btmgmt::parse_eir(&[0, 9, 9]);
        assert_eq!(n, "");
    }

    #[test]
    fn a_rom_state_controller_with_a_zero_address_is_listed_and_described() {
        let mut m = fake(vec![
            (OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 0, &ext_list(&[(0, 0, 1)]))]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], 0))]),
        ]);
        let c = read_controllers(&mut m).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].configured && !c[0].powered() && c[0].addr_state() == AddrState::Zero);
        let h = headline(&c[0], Some("BCM4364"), false);
        assert_eq!(h, "Controller hci0: BCM4364 (ROM state, no firmware file)");
        let d = detail(&c[0]);
        assert!(d.starts_with("address 00:00:00:00:00:00 (not set) · powered off · no valid address"), "{d}");
        assert!(d.contains("pairing needs a valid address"));
        assert_eq!(headline(&c[0], Some("BCM4364"), true), "Controller hci0: BCM4364");
        assert_eq!(headline(&c[0], None, false), "Controller hci0: Broadcom (no firmware file found)");
        assert!(short(&c[0], Some("BCM4364"), false).ends_with("no valid address · powered off"));
    }

    #[test]
    fn the_placeholder_address_and_a_real_one_and_an_unconfigured_controller() {
        assert_eq!(addr_state("00:00:00:00:00:00"), AddrState::Zero);
        assert_eq!(addr_state("aa:aa:aa:aa:aa:aa"), AddrState::Placeholder);
        assert_eq!(addr_state("AA:AA:AA:AA:AA:AA"), AddrState::Placeholder);
        assert_eq!(addr_state(""), AddrState::NotSet);
        assert_eq!(addr_state("AC:BC:32:11:22:33"), AddrState::Valid);
        let mut m = fake(vec![
            (OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 0, &ext_list(&[(0, 1, 1), (1, 0, 2), (2, 2, 1)]))]),
            (OP_READ_CONFIG_INFO, vec![complete(OP_READ_CONFIG_INFO, 0, 0, &[15, 0, 2, 0, 0, 0, 2, 0, 0, 0])]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 1, 0, &rom_info([0x33, 0x22, 0x11, 0x32, 0xbc, 0xac], SET_POWERED))]),
        ]);
        let c = read_controllers(&mut m).unwrap();
        assert_eq!(c.len(), 2, "the AMP entry is not a controller we show");
        assert!(!c[0].configured && c[0].manufacturer == 15 && c[0].addr_state() == AddrState::NotSet);
        let d = detail(&c[0]);
        assert!(d.starts_with("address not set · cannot be powered until it has an address · no valid address"), "{d}");
        assert!(c[1].addr_state().valid() && c[1].powered());
        assert_eq!(detail(&c[1]), "address AC:BC:32:11:22:33 · powered on");
    }

    #[test]
    fn a_kernel_without_the_extended_list_falls_back_and_other_failures_are_errors() {
        let mut m = fake(vec![
            (OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 1, &[])]),
            (OP_READ_INDEX_LIST, vec![complete(OP_READ_INDEX_LIST, INDEX_NONE, 0, &[1, 0, 0, 0])]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], 0))]),
        ]);
        assert_eq!(read_controllers(&mut m).unwrap().len(), 1);
        let mut m = fake(vec![(OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 20, &[])])]);
        assert!(read_controllers(&mut m).unwrap_err().to_string().contains("permission denied"));
        let mut m = fake(vec![]);
        assert!(matches!(read_controllers(&mut m), Err(Error::Timeout)), "a silent kernel times out");
        // a controller that cannot be read is left out, not an error for the others
        let mut m = fake(vec![
            (OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 0, &ext_list(&[(0, 0, 1)]))]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 17, &[])]),
        ]);
        assert!(read_controllers(&mut m).unwrap().is_empty());
        // a truncated info reply is left out too
        let mut m = fake(vec![
            (OP_READ_EXT_INDEX_LIST, vec![complete(OP_READ_EXT_INDEX_LIST, INDEX_NONE, 0, &ext_list(&[(0, 0, 1)]))]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &[0; 40])]),
        ]);
        assert!(read_controllers(&mut m).unwrap().is_empty());
    }

    #[test]
    fn command_status_and_complete_errors_become_plain_sentences() {
        let mut m = fake(vec![(OP_SET_POWERED, vec![complete(OP_SET_POWERED, 0, 17, &[])])]);
        let e = m.set_powered(0, true).unwrap_err();
        assert!(power_error(&e, true).contains("no valid address"), "{}", power_error(&e, true));
        let e = Error::Status(18);
        assert!(power_error(&e, true).contains("rfkill"));
        assert!(scan_error(&Error::Status(15)).contains("Power on first"));
        assert!(scan_error(&Error::Status(17)).contains("not set up"));
        assert!(scan_error(&Error::Timeout).contains("did not answer"));
    }

    fn found_event(index: u16, addr: [u8; 6], rssi: i8, name: &str) -> Vec<u8> {
        let mut eir = vec![name.len() as u8 + 1, 0x09];
        eir.extend_from_slice(name.as_bytes());
        let mut p = addr.to_vec();
        p.push(0);
        p.push(rssi as u8);
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&(eir.len() as u16).to_le_bytes());
        p.extend_from_slice(&eir);
        encode(EV_DEVICE_FOUND, index, &p)
    }

    #[test]
    fn a_scan_lists_name_address_and_signal_and_never_pairs() {
        let mut m = fake(vec![
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], SET_POWERED | SET_BREDR | SET_LE))]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], SET_POWERED | SET_BREDR | SET_LE))]),
            (
                OP_START_DISCOVERY,
                vec![
                    complete(OP_START_DISCOVERY, 0, 0, &[7]),
                    found_event(0, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], -60, "Keyboard K380"),
                    found_event(0, [1, 2, 3, 4, 5, 6], -80, ""),
                    found_event(0, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], -55, "Keyboard K380"),
                    encode(EV_DISCOVERING, 0, &[7, 0]),
                ],
            ),
            (OP_STOP_DISCOVERY, vec![complete(OP_STOP_DISCOVERY, 0, 0, &[7])]),
        ]);
        let f = scan_with(&mut m, 0, Duration::from_secs(10)).unwrap();
        assert_eq!(f.len(), 2, "the same device seen twice is one");
        assert_eq!(found_line(&f[0]), ("Keyboard K380".to_string(), "11:22:33:44:55:66 · -55 dBm".to_string()));
        assert_eq!(found_line(&f[1]).0, "06:05:04:03:02:01", "no name: the address is the title");
        let sent_ops: Vec<u16> = m_sent(&m);
        assert!(sent_ops.iter().all(|o| [OP_READ_INFO, OP_START_DISCOVERY, OP_STOP_DISCOVERY].contains(o)), "only reading and scanning was sent: {sent_ops:?}");
    }

    fn m_sent(m: &Mgmt<Fake>) -> Vec<u16> {
        m.transport().sent.iter().map(|p| u16::from_le_bytes([p[0], p[1]])).collect()
    }

    #[test]
    fn scanning_while_off_or_with_no_radio_says_what_to_do_and_turns_on_only_supported_types() {
        // powered off: the kernel refuses with "Bluetooth is off"
        let mut m = fake(vec![
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], 0))]),
            (OP_SET_BREDR, vec![complete(OP_SET_BREDR, 0, 0, &SET_BREDR.to_le_bytes())]),
            (OP_SET_LE, vec![complete(OP_SET_LE, 0, 0, &SET_LE.to_le_bytes())]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &rom_info([0; 6], SET_BREDR | SET_LE))]),
            (OP_START_DISCOVERY, vec![complete(OP_START_DISCOVERY, 0, 15, &[7])]),
        ]);
        let e = scan_with(&mut m, 0, Duration::from_secs(1)).unwrap_err();
        assert!(e.contains("Power on first"), "{e}");
        assert!(m_sent(&m).contains(&OP_SET_LE), "the radio types the controller supports are turned on for the scan");
        // a controller with neither classic nor LE
        let mut none = rom_info([0; 6], SET_POWERED);
        none[9..13].copy_from_slice(&SET_POWERED.to_le_bytes());
        let mut m = fake(vec![(OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &none)]), (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &none)])]);
        assert!(scan_with(&mut m, 0, Duration::from_secs(1)).unwrap_err().contains("neither classic nor low energy"));
    }

    #[test]
    fn the_wording_is_plain() {
        for w in ["Pairing and audio are not available yet", "Bluetooth stack", "docs/BETA-PLAN.md"] {
            assert!(NOT_YET.contains(w), "{w}");
        }
        assert_eq!(manufacturer_name(15), "Broadcom");
        assert_eq!(manufacturer_name(9999), "company 9999");
    }

    fn ctl(manufacturer: u16, address: &str, configured: bool) -> Controller {
        Controller { index: 0, configured, address: address.into(), version: Some(6), manufacturer, supported: 0, current: 0, name: String::new() }
    }

    #[test]
    fn needs_firmware_is_a_broadcom_controller_without_address_and_without_the_patch_file() {
        for addr in ["00:00:00:00:00:00", "AA:AA:AA:AA:AA:AA", ""] {
            assert!(needs_firmware(&ctl(BROADCOM, addr, true), false), "{addr:?}");
        }
        assert!(needs_firmware(&ctl(BROADCOM, "", false), false), "an unconfigured controller too");
        assert!(!needs_firmware(&ctl(BROADCOM, "A4:83:E7:11:22:33", true), false), "a valid address means it works");
        assert!(!needs_firmware(&ctl(BROADCOM, "00:00:00:00:00:00", true), true), "the file is there: another problem");
        assert!(!needs_firmware(&ctl(2, "00:00:00:00:00:00", true), false), "Intel loads its files from elsewhere");
        assert!(NEEDS_FIRMWARE.contains("Store") && NEEDS_FIRMWARE.contains("Setup") && NEEDS_FIRMWARE.contains("Hardware"));
        // only a Broadcom controller is told "no firmware file found"
        assert_eq!(headline(&ctl(2, "A4:83:E7:11:22:33", true), None, false), "Controller hci0: Intel");
        assert_eq!(headline(&ctl(BROADCOM, "", true), None, false), "Controller hci0: Broadcom (no firmware file found)");
    }
}
