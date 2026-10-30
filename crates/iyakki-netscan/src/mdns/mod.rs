//! mDNS for the stable device name: the device announces `iyakki-<random id>.local` (and `_iyakki-dev._tcp` for developer mode),
//! and `iyakki dev` finds a device by that name when its address has changed. A small in-house implementation of the part of RFC
//! 6762 this needs ([`wire`]: packets, [`responder`]: the device's state machine, [`net`]: sockets, the announcer thread and the
//! resolver). No cache, no forwarding, answers only for its own names. The certificate fingerprint pinned at pairing stays the
//! security anchor: a forged mDNS answer can send a computer to the wrong address, but not past the TLS check.

pub mod net;
pub mod responder;
pub mod wire;

#[cfg(test)]
mod tests;

/// The service types of IYAKKI devices.
pub const DEV_SERVICE: &str = "_iyakki-dev._tcp";
pub const WEB_SERVICE: &str = "_iyakki-web._tcp";

/// Is this a name `iyakki dev` should look up with mDNS?
pub fn is_local_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h.len() > 6 && h.ends_with(".local") && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// A device label from random bytes: `iyakki-` and six hex digits. Never made from a person's name or the host name.
pub fn label_from(bytes: [u8; 3]) -> String {
    format!("iyakki-{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

/// Is this a label [`label_from`] could have made?
pub fn is_device_label(s: &str) -> bool {
    s.len() == 13 && s.starts_with("iyakki-") && s[7..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
