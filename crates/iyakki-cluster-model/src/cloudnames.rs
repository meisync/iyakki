//! Name and address checks the routes share (moved out of the agent's cloudtenant, cloudstorage and cloudlisten_tunnel for F-SPLIT-03).

use std::net::IpAddr;

pub fn dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_lowercase()
        && *b.last().unwrap_or(&b'-') != b'-'
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn dns_subdomain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(|p| !p.is_empty() && p.len() <= 63 && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') && !p.starts_with('-') && !p.ends_with('-'))
}

pub fn host_name(s: &str) -> bool {
    s.contains('.') && dns_subdomain(s)
}


/// A name that is one plain path component: letters, digits, `-` and `_`, not empty, not starting with a dot.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !s.starts_with('.')
}

/// Private, not loopback, not link-local, not unspecified.
pub fn is_private(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(v) => {
            let o = v.octets();
            !v.is_loopback() && !v.is_link_local() && !v.is_unspecified() && (o[0] == 10 || (o[0] == 172 && (16..32).contains(&o[1])) || (o[0] == 192 && o[1] == 168) || (o[0] == 100 && (64..128).contains(&o[1])))
        }
        IpAddr::V6(v) => !v.is_loopback() && !v.is_unspecified() && (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Check a `peer` target's address when the route table is read: an address, private, and a port.
pub fn check_peer(host: &str, addr: &str, port: u16) -> Result<(), String> {
    let ip: IpAddr = addr.parse().map_err(|_| format!("route {host}: \"{addr}\" is not an IP address"))?;
    if !is_private(ip) {
        return Err(format!("route {host}: {addr} is not a private address (a peer is reached over a VPN)"));
    }
    if port == 0 {
        return Err(format!("route {host}: port 0 is not a port"));
    }
    Ok(())
}

/// A tunnel id: `t` and one to four digits (the shape `sshtunnel` hands out).
pub fn valid_tunnel_id(id: &str) -> bool {
    id.len() >= 2 && id.len() <= 5 && id.starts_with('t') && id[1..].chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn a_dns_label_may_not_start_with_a_digit() {
        assert!(!dns_label("1abc"));
    }

    #[test]
    fn a_dns_label_may_not_end_with_a_hyphen() {
        assert!(!dns_label("abc-"));
    }

    #[test]
    fn a_dns_label_may_not_contain_uppercase_letters() {
        assert!(!dns_label("Abc"));
    }

    #[test]
    fn a_dns_label_may_not_contain_underscores() {
        assert!(!dns_label("a_b"));
    }

    #[test]
    fn a_dns_label_may_not_contain_dots() {
        assert!(!dns_label("a.b"));
    }

    #[test]
    fn a_dns_label_may_not_be_empty() {
        assert!(!dns_label(""));
    }

    #[test]
    fn a_dns_label_may_not_exceed_63_characters() {
        assert!(!dns_label(&"a".repeat(64)));
    }

    #[test]
    fn a_dns_label_may_be_63_characters_long() {
        assert!(dns_label(&"a".repeat(63)));
    }

    #[test]
    fn a_dns_label_may_contain_lowercase_letters_digits_and_hyphens() {
        assert!(dns_label("a"));
        assert!(dns_label("node-1"));
        assert!(dns_label("a1b2"));
    }

    #[test]
    fn a_dns_subdomain_may_not_be_empty() {
        assert!(!dns_subdomain(""));
    }

    #[test]
    fn a_dns_subdomain_may_not_start_with_a_dot() {
        assert!(!dns_subdomain(".a"));
    }

    #[test]
    fn a_dns_subdomain_may_not_end_with_a_dot() {
        assert!(!dns_subdomain("a."));
    }

    #[test]
    fn a_dns_subdomain_may_not_contain_consecutive_dots() {
        assert!(!dns_subdomain("a..b"));
    }

    #[test]
    fn a_dns_subdomain_may_not_start_with_a_hyphen() {
        assert!(!dns_subdomain("-a.b"));
    }

    #[test]
    fn a_dns_subdomain_may_not_end_with_a_hyphen() {
        assert!(!dns_subdomain("a-.b"));
    }

    #[test]
    fn a_dns_subdomain_may_not_contain_uppercase_letters() {
        assert!(!dns_subdomain("A.b"));
    }

    #[test]
    fn a_dns_subdomain_may_not_contain_underscores() {
        assert!(!dns_subdomain("a_b.c"));
    }

    #[test]
    fn a_dns_subdomain_may_not_exceed_253_characters_total() {
        let name = [63, 63, 63, 62].map(|n| "a".repeat(n)).join(".");
        assert_eq!(name.len(), 254);
        assert!(!dns_subdomain(&name));
    }

    #[test]
    fn a_dns_subdomain_may_not_contain_a_label_exceeding_63_characters() {
        assert!(!dns_subdomain(&format!("{}.com", "a".repeat(64))));
    }

    #[test]
    fn a_dns_subdomain_may_be_253_characters_long() {
        let name = [63, 63, 63, 61].map(|n| "a".repeat(n)).join(".");
        assert_eq!(name.len(), 253);
        assert!(dns_subdomain(&name));
    }

    #[test]
    fn a_dns_subdomain_may_contain_valid_labels() {
        assert!(dns_subdomain("a"));
        assert!(dns_subdomain("a.b.c"));
        assert!(dns_subdomain("node-1.example.com"));
        assert!(dns_subdomain("a1.b2"));
    }

    #[test]
    fn a_host_name_may_not_be_empty() {
        assert!(!host_name(""));
    }

    #[test]
    fn a_host_name_may_not_contain_no_dot() {
        assert!(!host_name("localhost"));
    }

    #[test]
    fn a_host_name_may_not_contain_consecutive_dots() {
        assert!(!host_name("a..b"));
    }

    #[test]
    fn a_host_name_may_contain_valid_subdomain() {
        assert!(host_name("example.com"));
        assert!(host_name("a.b"));
    }

    #[test]
    fn a_plain_name_may_not_be_empty() {
        assert!(!plain_name(""));
    }

    #[test]
    fn a_plain_name_may_not_start_with_a_dot() {
        assert!(!plain_name(".hidden"));
    }

    #[test]
    fn a_plain_name_may_not_contain_spaces() {
        assert!(!plain_name("a b"));
    }

    #[test]
    fn a_plain_name_may_not_contain_slashes() {
        assert!(!plain_name("a/b"));
    }

    #[test]
    fn a_plain_name_may_not_contain_dots() {
        assert!(!plain_name("a.b"));
    }

    #[test]
    fn a_plain_name_may_not_contain_non_ascii_characters() {
        assert!(!plain_name("é"));
    }

    #[test]
    fn a_plain_name_may_not_exceed_64_characters() {
        assert!(!plain_name(&"a".repeat(65)));
    }

    #[test]
    fn a_plain_name_may_be_64_characters_long() {
        assert!(plain_name(&"a".repeat(64)));
    }

    #[test]
    fn a_plain_name_may_contain_letters_digits_underscores_and_hyphens() {
        assert!(plain_name("a"));
        assert!(plain_name("My_name-1"));
    }

    #[test]
    fn an_ipv4_address_is_private_if_it_is_in_the_10_network() {
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
    }

    #[test]
    fn an_ipv4_address_is_private_if_it_is_in_the_172_16_31_network() {
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
    }

    #[test]
    fn an_ipv4_address_is_private_if_it_is_in_the_192_168_network() {
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn an_ipv4_address_is_private_if_it_is_in_the_100_64_127_network() {
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
    }

    #[test]
    fn an_ipv4_address_is_not_private_if_it_is_not_in_a_private_network() {
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(172, 15, 0, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(192, 169, 0, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(100, 63, 0, 1))));
        assert!(!is_private(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
    }

    #[test]
    fn an_ipv6_address_is_private_if_it_is_in_the_fc00_network() {
        assert!(is_private(IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1))));
        assert!(is_private(IpAddr::V6(Ipv6Addr::new(0xfd12, 0x3456, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn a_tunnel_id_is_t_and_one_to_four_digits() {
        for ok in ["t1", "t42", "t9999"] {
            assert!(valid_tunnel_id(ok), "{ok}");
        }
        for bad in ["", "t", "1", "t12345", "T1", "t1a", "x1", "t-1"] {
            assert!(!valid_tunnel_id(bad), "{bad}");
        }
    }
}
