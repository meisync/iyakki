//! Sockets that follow the tunnel's policy routing (F-VPN-08). The marking table of policy routing sends packets that carry the plan's
//! mark through the VPN interface; this module makes a TCP connection whose socket carries that mark when the HYBRID filters say the
//! address goes through the tunnel (`Route::Tunnel`), and a plain one otherwise. The reader browser and the HTTP client of the core
//! call [`connect_for`] instead of connecting themselves. `SO_MARK` needs administrator rights, so an unprivileged caller gets a plain
//! sentence, never a silent direct connection: a fetch the filters sent into the tunnel does not leave outside it.

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use iyakki_vpn::filter::{FilterSet, Route};

/// The mark to put on the socket of a fetch of `url`: the plan's mark when the filters send it into the tunnel, none otherwise.
/// A URL the filters cannot judge is an error, so the caller does not connect at all.
pub fn mark_for_url(filters: &FilterSet, url: &str, plan_mark: u32) -> Result<Option<u32>, String> {
    let d = filters.decide_url(url).map_err(|e| e.to_string())?;
    Ok(if d.route == Route::Tunnel { Some(plan_mark) } else { None })
}

/// Connect to `addr`; with `mark` the socket carries it from before the first packet.
pub fn connect_marked(addr: SocketAddr, mark: Option<u32>, timeout: Duration) -> io::Result<TcpStream> {
    match mark {
        None => TcpStream::connect_timeout(&addr, timeout),
        Some(m) => marked(addr, m, timeout),
    }
}

/// Decide by the filters and connect accordingly.
pub fn connect_for(filters: &FilterSet, url: &str, plan_mark: u32, addr: SocketAddr, timeout: Duration) -> Result<TcpStream, String> {
    let mark = mark_for_url(filters, url, plan_mark)?;
    connect_marked(addr, mark, timeout).map_err(|e| match mark {
        Some(_) => format!("the connection to {} was meant to go through the VPN and could not be made ({e}); it was not sent outside the tunnel", addr.ip()),
        None => format!("could not connect to {} ({e})", addr.ip()),
    })
}

/// The hook for `iyakki_proxy::dial`: every connection of the reader browser and HTTP client asks the HYBRID filters about the host name
/// and then the address (a name rule that points one way is not overruled by silence of the address rules; no rule at all means a plain
/// connection). A destination that goes into the tunnel gets the marked socket, or a sentence when it cannot be marked: never a plain one.
pub fn dial_hook(filters: FilterSet, plan_mark: u32) -> iyakki_proxy::dial::Hook {
    std::sync::Arc::new(move |host: &str, addr: SocketAddr, timeout: Duration| {
        use iyakki_vpn::filter::Reason;
        let by_name = filters.decide_domain(host);
        let d = if matches!(by_name.reason, Reason::Rule { .. }) { by_name } else { filters.decide_ip(addr.ip()) };
        if d.route != Route::Tunnel {
            return None;
        }
        Some(connect_marked(addr, Some(plan_mark), timeout).map_err(|e| io::Error::new(e.kind(), format!("the connection to {host} was meant to go through the VPN and could not be made ({e}); it was not sent outside the tunnel"))))
    })
}

#[cfg(not(target_os = "linux"))]
fn marked(_addr: SocketAddr, _mark: u32, _timeout: Duration) -> io::Result<TcpStream> {
    Err(io::Error::new(io::ErrorKind::Unsupported, format!("marking a socket is {}", crate::oscompat::UNAVAILABLE)))
}

#[cfg(target_os = "linux")]
fn marked(addr: SocketAddr, mark: u32, timeout: Duration) -> io::Result<TcpStream> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let fam = if addr.is_ipv4() { libc::AF_INET } else { libc::AF_INET6 };
    // SAFETY: plain socket(2); the descriptor is owned right below and closed on every path.
    let fd = unsafe { libc::socket(fam, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a fresh descriptor nobody else owns.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let set = |level: i32, name: i32, ptr: *const libc::c_void, len: usize| -> io::Result<()> {
        // SAFETY: ptr points at a live value of `len` bytes for the duration of the call.
        if unsafe { libc::setsockopt(fd, level, name, ptr, len as u32) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    set(libc::SOL_SOCKET, libc::SO_MARK, (&mark as *const u32).cast(), 4).map_err(|e| io::Error::new(e.kind(), format!("could not mark the socket ({e}); this needs administrator rights")))?;
    let tv = libc::timeval { tv_sec: timeout.as_secs() as _, tv_usec: 0 };
    set(libc::SOL_SOCKET, libc::SO_SNDTIMEO, (&tv as *const libc::timeval).cast(), std::mem::size_of::<libc::timeval>())?;
    let sa = socket2_addr(&addr);
    // SAFETY: sa holds a valid sockaddr of the stated length for the call.
    if unsafe { libc::connect(fd, sa.0.as_ptr().cast(), sa.1) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(TcpStream::from(owned))
}

/// The raw `sockaddr` bytes of an address (no extra crate).
#[cfg(target_os = "linux")]
fn socket2_addr(a: &SocketAddr) -> (Vec<u8>, u32) {
    match a {
        SocketAddr::V4(v) => {
            let mut b = vec![0u8; std::mem::size_of::<libc::sockaddr_in>()];
            b[0..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
            b[2..4].copy_from_slice(&v.port().to_be_bytes());
            b[4..8].copy_from_slice(&v.ip().octets());
            let n = b.len() as u32;
            (b, n)
        }
        SocketAddr::V6(v) => {
            let mut b = vec![0u8; std::mem::size_of::<libc::sockaddr_in6>()];
            b[0..2].copy_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes());
            b[2..4].copy_from_slice(&v.port().to_be_bytes());
            b[4..8].copy_from_slice(&v.flowinfo().to_be_bytes());
            b[8..24].copy_from_slice(&v.ip().octets());
            b[24..28].copy_from_slice(&v.scope_id().to_ne_bytes());
            let n = b.len() as u32;
            (b, n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn08_only_urls_the_filters_send_into_the_tunnel_get_the_mark_and_unjudgeable_urls_are_errors() {
        let f = FilterSet::from_entries(&["*.corp.example".to_string()]).unwrap();
        assert_eq!(mark_for_url(&f, "https://wiki.corp.example/a", 7).unwrap(), Some(7));
        assert_eq!(mark_for_url(&f, "https://other.example/", 7).unwrap(), None);
        assert!(mark_for_url(&f, "not a url", 7).is_err());
    }

    #[test]
    fn vpn08_a_plain_connect_works_and_a_marked_one_either_connects_or_says_why_it_cannot() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        assert!(connect_marked(addr, None, Duration::from_secs(5)).is_ok());
        match connect_marked(addr, Some(0x4956_0002), Duration::from_secs(5)) {
            Ok(_) => {}
            Err(e) => assert!(e.to_string().contains("administrator") || e.kind() == io::ErrorKind::Unsupported, "{e}"),
        }
    }

    #[test]
    fn vpn08_the_dial_hook_leaves_unchosen_destinations_alone_and_never_lets_a_chosen_one_connect_plainly() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let f = FilterSet::from_entries(&["*.corp.example".to_string(), "198.51.100.0/24".to_string()]).unwrap();
        let h = dial_hook(f, 0x4956_0002);
        assert!(h("other.example", addr, Duration::from_secs(5)).is_none(), "no rule: the plain way");
        // a chosen name: the marked connect either works (rights) or says why not; it is never None, so never plain
        match h("wiki.corp.example", addr, Duration::from_secs(5)).expect("chosen names are decided here") {
            Ok(_) => {}
            Err(e) => assert!(e.to_string().contains("not sent outside the tunnel"), "{e}"),
        }
    }
}
