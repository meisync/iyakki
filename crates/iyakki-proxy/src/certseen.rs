//! The certificate chains the browser's own HTTPS connections were given (F-BROW-98): every TLS handshake the in-house HTTP/2 client
//! completes records the chain the site sent, so the lock panel of an open tab can show the certificate that tab's connection used
//! and not the answer of a second handshake. Kept in memory only, at most 128 sites, the newest replacing the oldest; nothing is written.
//! A site reached only over HTTP/1.1 (the system client) has no record here: the panel then asks for a fresh handshake as before.

use std::collections::VecDeque;
use std::sync::Mutex;

const MAX: usize = 128;

static SEEN: Mutex<VecDeque<(String, u16, Vec<Vec<u8>>)>> = Mutex::new(VecDeque::new());

/// Remember the chain (leaf first) the site `host:port` sent in its last handshake.
pub fn record(host: &str, port: u16, chain: &[Vec<u8>]) {
    if chain.is_empty() || chain.len() > 16 {
        return;
    }
    let host = host.to_ascii_lowercase();
    let mut g = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    g.retain(|(h, p, _)| !(*h == host && *p == port));
    if g.len() >= MAX {
        g.pop_front();
    }
    g.push_back((host, port, chain.to_vec()));
}

/// The chain the browser's last handshake with `host:port` was given.
pub fn latest(host: &str, port: u16) -> Option<Vec<Vec<u8>>> {
    let host = host.to_ascii_lowercase();
    SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(h, p, _)| *h == host && *p == port).map(|(_, _, c)| c.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_chain_of_a_site_is_kept_the_table_is_bounded_and_empty_chains_are_ignored() {
        record("seen-a.test", 443, &[vec![1], vec![2]]);
        record("SEEN-A.test", 443, &[vec![3]]);
        assert_eq!(latest("seen-a.test", 443), Some(vec![vec![3]]), "the newer chain replaced the older, names are not case sensitive");
        assert_eq!(latest("seen-a.test", 8443), None);
        record("seen-b.test", 443, &[]);
        assert_eq!(latest("seen-b.test", 443), None);
        record("seen-c.test", 443, &vec![vec![0]; 17]);
        assert_eq!(latest("seen-c.test", 443), None);
        for i in 0..(MAX + 5) {
            record(&format!("seen-fill-{i}.test"), 443, &[vec![9]]);
        }
        assert!(SEEN.lock().unwrap().len() <= MAX);
        assert_eq!(latest(&format!("seen-fill-{}.test", MAX + 4), 443), Some(vec![vec![9]]));
    }
}
