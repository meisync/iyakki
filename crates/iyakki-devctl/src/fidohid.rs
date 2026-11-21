//! Security keys on USB (F-BROX-03): the FIDO HID devices among the hidraw nodes, found by their own report descriptor (usage page 0xF1D0,
//! usage 1), and a node that moves 64-byte reports in and out. The CTAP2 conversation on top of it is `iyakki_webcrypto::ctap`; this file
//! only finds the device and carries the packets. No table of makes or models: any key that says "FIDO" in its descriptor is one.

use crate::Roots;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

const FIDO_PAGE: u32 = 0xF1D0;
const FIDO_USAGE: u32 = 0x01;
/// The most a read waits for one report, in milliseconds (a key blinking for a touch sends a keep-alive every 100 ms).
pub const READ_WAIT_MS: i32 = 3000;

/// Does this HID report descriptor describe a FIDO authenticator? Walks the short items; a long item or a damaged descriptor says no.
pub fn is_fido(desc: &[u8]) -> bool {
    let (mut i, mut page, mut found) = (0usize, 0u32, false);
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            return false; // a long item: not used by keys, not walked
        }
        let size = match b & 3 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = desc.get(i + 1..i + 1 + size) else { return false };
        let value = data.iter().rev().fold(0u32, |v, &x| (v << 8) | x as u32);
        match (b >> 2) & 3 {
            // global: usage page
            1 if b >> 4 == 0 => page = value,
            // local: usage
            2 if b >> 4 == 0 => found |= page == FIDO_PAGE && value == FIDO_USAGE,
            _ => {}
        }
        i += 1 + size;
    }
    found
}

/// The hidraw nodes of the FIDO devices (`<sys>/class/hidraw/hidrawN/device/report_descriptor`), their nodes under `dev`.
pub fn find(roots: &Roots, dev: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in crate::dirs(&roots.sys.join("class/hidraw")) {
        let Some(name) = d.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        if std::fs::read(d.join("device/report_descriptor")).is_ok_and(|desc| is_fido(&desc)) {
            out.push(dev.join(name));
        }
    }
    out.sort();
    out
}

/// One key's hidraw node.
pub struct Node {
    file: std::fs::File,
    wait_ms: i32,
}

impl Node {
    pub fn open(path: &std::path::Path) -> Result<Node, String> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Node { file, wait_ms: READ_WAIT_MS })
    }

    /// A node on any file (a test gives one end of a socket pair) and how long a read waits.
    pub fn from_file(file: std::fs::File, wait_ms: i32) -> Node {
        Node { file, wait_ms }
    }

    /// One 64-byte report out: hidraw wants the report number first, and keys use none (0).
    pub fn send(&mut self, packet: &[u8; 64]) -> Result<(), String> {
        let mut out = [0u8; 65];
        out[1..].copy_from_slice(packet);
        self.file.write_all(&out).map_err(|e| format!("the security key did not take the packet: {e}"))
    }

    /// One 64-byte report in; an error after the wait when the key says nothing.
    pub fn recv(&mut self) -> Result<[u8; 64], String> {
        let mut p = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for the length given.
        let n = unsafe { libc::poll(&mut p, 1, self.wait_ms) };
        if n <= 0 {
            return Err("the security key did not answer in time".into());
        }
        let mut buf = [0u8; 64];
        let got = self.file.read(&mut buf).map_err(|e| format!("the security key was unplugged or failed: {e}"))?;
        if got != 64 {
            return Err("the security key sent a short packet".into());
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;

    /// The report descriptor of a CTAPHID device as the specification lists it.
    const KEY: [u8; 34] = [0x06, 0xD0, 0xF1, 0x09, 0x01, 0xA1, 0x01, 0x09, 0x20, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x81, 0x02, 0x09, 0x21, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x91, 0x02, 0xC0];

    #[test]
    fn a_descriptor_that_says_fido_is_a_key_and_a_keyboard_or_a_cut_descriptor_is_not() {
        assert!(is_fido(&KEY));
        // a keyboard: usage page 1, usage 6
        assert!(!is_fido(&[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xC0]));
        // the right page with another usage, the right usage on another page
        assert!(!is_fido(&[0x06, 0xD0, 0xF1, 0x09, 0x02, 0xC0]));
        assert!(!is_fido(&[0x05, 0x01, 0x09, 0x01, 0xC0]));
        // cut in the middle of an item, a long item, nothing
        assert!(!is_fido(&KEY[..2]));
        assert!(!is_fido(&[0xFE, 0x02, 0x01]));
        assert!(!is_fido(&[]));
    }

    #[test]
    fn keys_are_found_among_the_hidraw_nodes_by_their_descriptor_and_others_are_left() {
        let d = std::env::temp_dir().join(format!("iyakki-fido-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, desc) in [("hidraw2", &KEY[..]), ("hidraw0", &[0x05, 0x01, 0x09, 0x06][..]), ("hidraw5", &KEY[..])] {
            let dir = d.join("sys/class/hidraw").join(n).join("device");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("report_descriptor"), desc).unwrap();
        }
        let roots = Roots { sys: d.join("sys"), proc_: d.join("proc") };
        assert_eq!(find(&roots, std::path::Path::new("/dev")), vec![PathBuf::from("/dev/hidraw2"), PathBuf::from("/dev/hidraw5")]);
        assert!(find(&Roots { sys: d.join("nothing"), proc_: d.join("proc") }, std::path::Path::new("/dev")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_report_goes_out_with_the_report_number_and_comes_in_whole_and_a_silent_key_times_out() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut node = Node::from_file(unsafe { std::fs::File::from_raw_fd(a.into_raw_fd()) }, 50);
        let mut key = b;
        let mut p = [0u8; 64];
        p[0] = 0xAB;
        p[63] = 0xCD;
        node.send(&p).unwrap();
        let mut got = [0u8; 65];
        key.read_exact(&mut got).unwrap();
        assert_eq!((got[0], got[1], got[64]), (0, 0xAB, 0xCD));
        key.write_all(&p).unwrap();
        assert_eq!(node.recv().unwrap(), p);
        assert!(node.recv().unwrap_err().contains("in time"));
        key.write_all(&[1, 2, 3]).unwrap();
        assert!(node.recv().unwrap_err().contains("short"));
        drop(key);
        assert!(node.recv().is_err(), "an unplugged key is an error, not a hang");
    }
}
