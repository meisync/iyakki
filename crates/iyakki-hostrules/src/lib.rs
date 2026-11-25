

/// A loopback, private-range or cluster-local host.
pub fn is_private_host(h: &str) -> bool {
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h.ends_with(".svc") || h.ends_with(".cluster.local") || h.ends_with(".internal") {
        return true;
    }
    let p: Vec<u8> = h.split('.').filter_map(|x| x.parse().ok()).collect();
    p.len() == 4 && h.split('.').count() == 4 && (p[0] == 127 || p[0] == 10 || (p[0] == 192 && p[1] == 168) || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 169 && p[1] == 254))
}

#[cfg(test)]
mod tests {
    use super::is_private_host;

    #[test]
    fn local_and_private_names_are_private() {
        for h in ["localhost", "a.localhost", "nas.local", "x.svc", "x.cluster.local", "db.internal", "127.0.0.1", "10.1.2.3", "192.168.1.9", "172.16.0.1", "172.31.255.1", "169.254.1.1"] {
            assert!(is_private_host(h), "{h}");
        }
    }

    #[test]
    fn public_names_and_near_misses_are_not() {
        for h in ["example.com", "8.8.8.8", "172.32.0.1", "172.15.0.1", "192.169.0.1", "10.1.2", "10.1.2.3.4", "localhost.example.com"] {
            assert!(!is_private_host(h), "{h}");
        }
    }
}
