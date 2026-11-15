//! Happy Eyeballs (RFC 8305) for the browser's connections: when a name has several addresses (IPv6 and IPv4), one dead address must
//! not stall the connect for the whole timeout. [`interleave`] orders the addresses (IPv6 first, then alternating families, each
//! family in the order the resolver gave), [`race`] starts the attempts one after another (the next one at once when the one before
//! failed, else after the stagger) and the first that connects wins, and [`fastest_first`] puts the winner first in the list that the
//! HTTP client then walks. Nothing here sends a byte: the winning connection is closed again, the client opens its own.

use std::net::SocketAddr;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long one attempt runs before the next address is tried alongside it (RFC 8305 suggests 250 ms).
pub const STAGGER: Duration = Duration::from_millis(250);
/// The whole race is cut off after this long.
pub const TOTAL: Duration = Duration::from_secs(5);

/// IPv6 first, then alternating IPv6 and IPv4, each family keeping the resolver's order.
pub fn interleave(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let (mut v6, mut v4): (Vec<SocketAddr>, Vec<SocketAddr>) = (Vec::new(), Vec::new());
    for a in addrs {
        if a.is_ipv6() { v6.push(*a) } else { v4.push(*a) }
    }
    let (mut out, mut i) = (Vec::with_capacity(addrs.len()), 0);
    while i < v6.len() || i < v4.len() {
        if let Some(a) = v6.get(i) {
            out.push(*a);
        }
        if let Some(a) = v4.get(i) {
            out.push(*a);
        }
        i += 1;
    }
    out
}

/// Try the addresses in order, each started `stagger` after the one before (at once when that one failed), and return the first
/// that `connect` accepts. `connect` is given the address and what is left of `total`.
pub fn race<F>(addrs: &[SocketAddr], stagger: Duration, total: Duration, connect: F) -> Result<SocketAddr, String>
where
    F: Fn(SocketAddr, Duration) -> std::io::Result<()> + Send + Sync + 'static,
{
    if addrs.is_empty() {
        return Err("no address to connect to".into());
    }
    let connect = Arc::new(connect);
    let began = Instant::now();
    let (tx, rx) = channel::<(SocketAddr, Result<(), String>)>();
    let (mut started, mut failed, mut last) = (0usize, 0usize, String::new());
    loop {
        if started < addrs.len() {
            let (a, c, t) = (addrs[started], connect.clone(), tx.clone());
            let left = total.saturating_sub(began.elapsed());
            started += 1;
            std::thread::spawn(move || {
                let _ = t.send((a, c(a, left).map_err(|e| e.to_string())));
            });
        }
        let left = total.saturating_sub(began.elapsed());
        if left.is_zero() {
            return Err(format!("no address answered within {} s", total.as_secs().max(1)));
        }
        // wait for an answer; a quiet stagger starts the next attempt, every attempt started and failed ends the race
        let wait = if started < addrs.len() { stagger.min(left) } else { left };
        match rx.recv_timeout(wait) {
            Ok((a, Ok(()))) => return Ok(a),
            Ok((_, Err(e))) => {
                last = e;
                failed += 1;
                if failed == addrs.len() {
                    return Err(last);
                }
                // the attempt that failed makes room: start the next one at once (the loop does that when any are left)
                if started >= addrs.len() {
                    continue;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if started >= addrs.len() {
                    return Err(format!("no address answered within {} s", total.as_secs().max(1)));
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Err(last),
        }
    }
}

fn tcp(a: SocketAddr, left: Duration) -> std::io::Result<()> {
    std::net::TcpStream::connect_timeout(&a, left.min(Duration::from_secs(5))).map(|_| ())
}

/// The addresses in the order the client should try them: interleaved, with the address that connected first moved to the front. With
/// one address, or when none connects, the interleaved order is returned unchanged (the client then reports its own error).
pub fn fastest_first(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    fastest_first_with(addrs, STAGGER, TOTAL, tcp)
}

/// [`fastest_first`] with the connect function and the times given (for tests).
pub fn fastest_first_with<F>(addrs: &[SocketAddr], stagger: Duration, total: Duration, connect: F) -> Vec<SocketAddr>
where
    F: Fn(SocketAddr, Duration) -> std::io::Result<()> + Send + Sync + 'static,
{
    let mut order = interleave(addrs);
    if order.len() > 1 {
        if let Ok(win) = race(&order, stagger, total, connect) {
            if let Some(i) = order.iter().position(|a| *a == win) {
                order[..=i].rotate_right(1);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn addresses_are_interleaved_ipv6_first_and_each_family_keeps_its_order() {
        let got = interleave(&[a("192.0.2.1:443"), a("192.0.2.2:443"), a("[2001:db8::1]:443"), a("192.0.2.3:443"), a("[2001:db8::2]:443")]);
        assert_eq!(got, vec![a("[2001:db8::1]:443"), a("192.0.2.1:443"), a("[2001:db8::2]:443"), a("192.0.2.2:443"), a("192.0.2.3:443")]);
        assert_eq!(interleave(&[a("192.0.2.1:80")]), vec![a("192.0.2.1:80")]);
        assert!(interleave(&[]).is_empty());
    }

    #[test]
    fn a_dead_first_address_does_not_stall_the_connect_the_next_one_wins_after_the_stagger() {
        // the IPv6 address hangs for 3 s (a dead route); the IPv4 one answers at once: the race ends about one stagger in
        let (dead, live) = (a("[2001:db8::1]:443"), a("192.0.2.1:443"));
        let began = Instant::now();
        let win = race(&[dead, live], Duration::from_millis(100), Duration::from_secs(10), move |x, _| {
            if x == dead {
                std::thread::sleep(Duration::from_secs(3));
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(win, live);
        assert!(began.elapsed() < Duration::from_millis(2500), "the dead address was not waited for: {:?}", began.elapsed());
    }

    #[test]
    fn a_refused_address_starts_the_next_attempt_at_once_without_waiting_for_the_stagger() {
        let (bad, good) = (a("[2001:db8::1]:443"), a("192.0.2.1:443"));
        let began = Instant::now();
        let win = race(&[bad, good], Duration::from_secs(5), Duration::from_secs(10), move |x, _| if x == bad { Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused")) } else { Ok(()) }).unwrap();
        assert_eq!(win, good);
        assert!(began.elapsed() < Duration::from_secs(2), "no 5 s stagger was waited: {:?}", began.elapsed());
    }

    #[test]
    fn when_every_address_fails_the_last_reason_comes_back_and_when_none_answers_in_time_the_time_is_said() {
        let list = [a("192.0.2.1:443"), a("192.0.2.2:443")];
        let e = race(&list, Duration::from_millis(10), Duration::from_secs(5), |_, _| Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"))).unwrap_err();
        assert!(e.contains("refused"), "{e}");
        let e = race(&list, Duration::from_millis(20), Duration::from_millis(300), |_, _| {
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        })
        .unwrap_err();
        assert!(e.contains("no address answered"), "{e}");
        assert!(race(&[], STAGGER, TOTAL, |_, _| Ok(())).is_err());
    }

    #[test]
    fn the_winner_is_moved_to_the_front_and_one_address_is_left_alone() {
        let (v6, v4a, v4b) = (a("[2001:db8::1]:443"), a("192.0.2.1:443"), a("192.0.2.2:443"));
        // v6 is dead, v4a refuses, v4b answers: the order handed to the client starts with v4b, then the rest as they were
        let got = fastest_first_with(&[v6, v4a, v4b], Duration::from_millis(50), Duration::from_secs(5), move |x, _| {
            if x == v4b { Ok(()) } else if x == v6 { std::thread::sleep(Duration::from_secs(2)); Err(std::io::ErrorKind::TimedOut.into()) } else { Err(std::io::ErrorKind::ConnectionRefused.into()) }
        });
        assert_eq!(got, vec![v4b, v6, v4a]);
        assert_eq!(fastest_first_with(&[v4a], STAGGER, TOTAL, |_, _| Err(std::io::ErrorKind::ConnectionRefused.into())), vec![v4a], "one address: nothing to race");
        // nothing connects: the interleaved order is kept for the client's own error
        let none = fastest_first_with(&[v4a, v4b], Duration::from_millis(10), Duration::from_secs(5), |_, _| Err(std::io::ErrorKind::ConnectionRefused.into()));
        assert_eq!(none, vec![v4a, v4b]);
    }

    #[test]
    fn the_real_connect_finds_the_one_listening_address_on_the_loopback() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live = l.local_addr().unwrap();
        // a port nobody listens on (bind and drop)
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let got = fastest_first(&[dead, live]);
        assert_eq!(got[0], live, "{got:?}");
    }
}
