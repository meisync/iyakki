

/// A loopback, private-range or cluster-local host.
pub fn is_private_host(h: &str) -> bool {
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h.ends_with(".svc") || h.ends_with(".cluster.local") || h.ends_with(".internal") {
        return true;
    }
    let p: Vec<u8> = h.split('.').filter_map(|x| x.parse().ok()).collect();
    p.len() == 4 && h.split('.').count() == 4 && (p[0] == 127 || p[0] == 10 || (p[0] == 192 && p[1] == 168) || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 169 && p[1] == 254))
}
