//! Base-layer home of the Linux apps sandbox program side, moved out of `iyakki-shared` (F-ARCH-07, KI-729) so that `iyakki-shared` and
//! `iyakki-core-local` can both depend on one small crate. `iyakki-shared` keeps `pub use` at the old paths.

pub mod quietio;
pub mod linuxapps_devices;
pub mod linuxapps_net;
pub mod linuxapps_init;

/// The kernel's IPv4 forwarding switch: the one place it is read and written (internet sharing and the outbound-only Linux apps network).
pub fn ip_forward_get() -> Result<String, String> {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").map(|s| s.trim().to_string()).map_err(|e| format!("ip_forward: {e}"))
}

pub fn ip_forward_set(value: &str) -> Result<(), String> {
    if !matches!(value, "0" | "1") {
        return Err("not a forwarding value".into());
    }
    std::fs::write("/proc/sys/net/ipv4/ip_forward", value).map_err(|e| format!("ip_forward: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_zero_and_one_are_forwarding_values() {
        assert!(super::ip_forward_set("2").is_err());
        assert!(super::ip_forward_set("").is_err());
        assert!(super::ip_forward_set("yes").is_err());
    }
}
