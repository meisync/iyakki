//! The kernel side of the direct link between two computers (F-ARCH-07): the interface address ioctls, the ARP probes on a raw
//! packet socket, the beacon sockets bound to one interface, and the plain-file reads of `/sys/class/net`, `/sys/class/udc`,
//! `/proc/net/if_inet6` and `/proc/interrupts`. The agent's `directlink::real::RealEnv` calls these functions; the manager and the
//! checks stay in the agent and talk to the `Env` trait (a fake in the tests). The file reads take the roots as parameters, so the
//! tests run on made-up trees; the socket and ioctl parts need privileges and are covered by the hardware run.

use std::path::Path;

use iyakki_shared::directlink::NetIf;

/// An interface name that is safe to put into a path or a request.
pub fn valid_ifname(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The fe80 addresses of `name` in the text of `/proc/net/if_inet6`, as `fe80:0:...%name`.
pub fn ipv6_ll_from(text: &str, name: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() >= 6 && f[5] == name && f[0].len() == 32 && f[0].starts_with("fe80") {
                let g: Vec<String> = (0..8).map(|i| f[0][i * 4..i * 4 + 4].trim_start_matches('0').to_string()).map(|s| if s.is_empty() { "0".into() } else { s }).collect();
                Some(format!("{}%{name}", g.join(":")))
            } else {
                None
            }
        })
        .collect()
}

/// Every interface under `<sys>/class/net` except `lo`: driver, hardware address, up and carrier, speed, the IPv4 addresses given
/// (`v4`: interface name, optionally `name:alias`, and the address text) and the IPv6 link-local ones from `<proc_>/net/if_inet6`.
pub fn net_ifaces(sys: &Path, proc_: &Path, v4: &[(String, String)]) -> Vec<NetIf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys.join("class/net")) else { return out };
    let inet6 = read(&proc_.join("net/if_inet6"));
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !valid_ifname(&name) || name == "lo" {
            continue;
        }
        let p = e.path();
        let driver = std::fs::read_link(p.join("device/driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let flags = u32::from_str_radix(read(&p.join("flags")).trim_start_matches("0x"), 16).unwrap_or(0);
        let speed = read(&p.join("speed")).parse::<i64>().ok().filter(|s| *s > 0).map(|s| s as u32);
        let ipv4 = v4.iter().filter(|(n, _)| n.split(':').next() == Some(name.as_str())).filter_map(|(_, a)| a.parse().ok()).collect();
        let ipv6_ll = ipv6_ll_from(&inet6, &name);
        out.push(NetIf { driver, mac: read(&p.join("address")), up: flags & 1 != 0, carrier: read(&p.join("carrier")) == "1", speed_mbps: speed, ipv4, ipv6_ll, name });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The machine has a USB device controller (`<sys>/class/udc` is not empty).
pub fn udc_present(sys: &Path) -> bool {
    std::fs::read_dir(sys.join("class/udc")).is_ok_and(|mut r| r.next().is_some())
}

/// The text of `<proc_>/interrupts` (empty when unreadable); the caller sums the Thunderbolt lines.
pub fn interrupts_text(proc_: &Path) -> Option<String> {
    std::fs::read_to_string(proc_.join("interrupts")).ok()
}

/// Write the `key` (64 hex digits, optional) and then the `authorized` level of one Thunderbolt device under `<sys>/bus/thunderbolt/devices`.
pub fn tb_write_authorized(sys: &Path, device: &str, level: u8, key: Option<&str>) -> Result<(), String> {
    if !device.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')) || device.is_empty() {
        return Err("not a Thunderbolt device name".into());
    }
    let dir = sys.join("bus/thunderbolt/devices").join(device);
    if let Some(k) = key {
        if k.len() != 64 || !k.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("the key must be 64 hex characters".into());
        }
        std::fs::write(dir.join("key"), k).map_err(|e| format!("{device}: writing the key: {e}"))?;
    }
    std::fs::write(dir.join("authorized"), level.to_string()).map_err(|e| format!("{device}: approving: {e}"))
}

#[cfg(target_os = "linux")]
pub mod sys {
    use iyakki_shared::directlink::linklocal::{self, Probe};
    use std::net::Ipv4Addr;

    const SIOCSIFADDR: libc::c_ulong = 0x8916;
    const SIOCSIFNETMASK: libc::c_ulong = 0x891c;
    const SIOCSIFBRDADDR: libc::c_ulong = 0x891a;

    #[repr(C)]
    pub(crate) struct IfReqAddr {
        name: [u8; 16],
        family: libc::sa_family_t,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
        pad: [u8; 8],
    }

    pub(crate) fn req(name: &str, ip: Ipv4Addr) -> Result<IfReqAddr, String> {
        if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
            return Err(format!("not an interface name: {name:?}"));
        }
        let mut r = IfReqAddr { name: [0; 16], family: libc::AF_INET as libc::sa_family_t, port: 0, addr: ip.octets(), zero: [0; 8], pad: [0; 8] };
        r.name[..name.len()].copy_from_slice(name.as_bytes());
        Ok(r)
    }

    /// Set (or, with 0.0.0.0, remove) the IPv4 address of `name` (`eth0` or an alias `eth0:1`) with its netmask and broadcast address.
    pub fn set_addr(name: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        if prefix > 32 {
            return Err("bad prefix".into());
        }
        let mask = if prefix == 0 { 0u32 } else { u32::MAX << (32 - u32::from(prefix)) };
        let bcast = u32::from(ip) | !mask;
        // SAFETY: a datagram socket used for three interface requests; every `IfReqAddr` matches the kernel's `struct ifreq`
        // (16-byte name, then a `sockaddr_in` in the union, padded to 40 bytes) and outlives its call.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(format!("socket: {}", std::io::Error::last_os_error()));
            }
            let res = (|| {
                let mut a = req(name, ip)?;
                if libc::ioctl(fd, SIOCSIFADDR as _, &mut a as *mut IfReqAddr) < 0 {
                    return Err(format!("{name}: {}", std::io::Error::last_os_error()));
                }
                if ip.is_unspecified() {
                    return Ok(());
                }
                let mut m = req(name, Ipv4Addr::from(mask))?;
                if libc::ioctl(fd, SIOCSIFNETMASK as _, &mut m as *mut IfReqAddr) < 0 {
                    return Err(format!("{name}: netmask: {}", std::io::Error::last_os_error()));
                }
                let mut b = req(name, Ipv4Addr::from(bcast))?;
                let _ = libc::ioctl(fd, SIOCSIFBRDADDR as _, &mut b as *mut IfReqAddr);
                Ok(())
            })();
            libc::close(fd);
            res
        }
    }

    fn ifindex(name: &str) -> Option<i32> {
        let c = std::ffi::CString::new(name).ok()?;
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
        let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
        (i != 0).then_some(i as i32)
    }

    fn packet_socket(name: &str) -> Result<i32, String> {
        let idx = ifindex(name).ok_or_else(|| format!("{name}: no such interface"))?;
        // SAFETY: a raw packet socket for ARP only (protocol 0x0806 in network order), bound to one interface; `sll` is a fully
        // initialised `sockaddr_ll` and the length passed is its size. The descriptor is closed by the callers.
        unsafe {
            let fd = libc::socket(libc::AF_PACKET, libc::SOCK_RAW | libc::SOCK_CLOEXEC, i32::from((libc::ETH_P_ARP as u16).to_be()));
            if fd < 0 {
                return Err(format!("packet socket: {}", std::io::Error::last_os_error()));
            }
            let mut sll: libc::sockaddr_ll = std::mem::zeroed();
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_protocol = (libc::ETH_P_ARP as u16).to_be();
            sll.sll_ifindex = idx;
            if libc::bind(fd, &sll as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_ll>() as u32) < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(format!("bind: {e}"));
            }
            Ok(fd)
        }
    }

    fn send(fd: i32, name: &str, frame: &[u8]) -> bool {
        let Some(idx) = ifindex(name) else { return false };
        // SAFETY: `frame` is valid for its length; `sll` is a fully initialised `sockaddr_ll` of the right size.
        unsafe {
            let mut sll: libc::sockaddr_ll = std::mem::zeroed();
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_ifindex = idx;
            sll.sll_halen = 6;
            sll.sll_addr[..6].copy_from_slice(&[0xff; 6]);
            libc::sendto(fd, frame.as_ptr() as *const libc::c_void, frame.len(), 0, &sll as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_ll>() as u32) == frame.len() as isize
        }
    }

    /// Wait up to `ms` for a frame; the frame's bytes.
    fn recv(fd: i32, ms: i32, buf: &mut [u8]) -> Option<usize> {
        // SAFETY: `pfd` is one valid pollfd; `buf` is valid for its length.
        unsafe {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if libc::poll(&mut pfd, 1, ms) <= 0 {
                return None;
            }
            let n = libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            (n > 0).then_some(n as usize)
        }
    }

    /// RFC 3927: PROBE_NUM probes about a second apart (a little different per MAC), then ANNOUNCE_WAIT; any packet that shows the
    /// address in use ends it at once.
    pub fn arp_probe(name: &str, mac: [u8; 6], ip: Ipv4Addr) -> Probe {
        let Ok(fd) = packet_socket(name) else { return Probe::Failed };
        let frame = linklocal::probe_frame(&mac, ip);
        let jitter = 200 + i32::from(mac[5]) * 2; // 200..710 ms of spread, so two computers do not probe in step
        let mut buf = [0u8; 128];
        let mut result = Probe::Free;
        'outer: for round in 0..=linklocal::PROBE_NUM {
            if round < linklocal::PROBE_NUM && !send(fd, name, &frame) {
                result = Probe::Failed;
                break;
            }
            let wait_ms = if round < linklocal::PROBE_NUM { 600 + jitter } else { 1500 };
            let end = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms as u64);
            while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                if let Some(n) = recv(fd, left.as_millis().min(i32::MAX as u128) as i32, &mut buf) {
                    if let Some(p) = linklocal::parse_arp(&buf[..n]) {
                        if let Some(other) = linklocal::conflicts(&p, &mac, ip) {
                            result = Probe::InUse(other);
                            break 'outer;
                        }
                    }
                }
            }
        }
        // SAFETY: closing the descriptor this function opened.
        unsafe { libc::close(fd) };
        result
    }

    pub fn arp_announce(name: &str, mac: [u8; 6], ip: Ipv4Addr) {
        if let Ok(fd) = packet_socket(name) {
            let f = linklocal::arp_frame(&mac, ip, ip);
            send(fd, name, &f);
            // SAFETY: closing the descriptor this function opened.
            unsafe { libc::close(fd) };
        }
    }

    /// The interface's index (for the IPv6 group and the scope of an address).
    pub fn ifindex6(name: &str) -> Option<u32> {
        ifindex(name).map(|i| i as u32)
    }

    /// The IPv6 beacon socket of one interface: bound to the beacon port on that interface only, joined to the all-nodes group
    /// `ff02::1`, our own packets not looped back, IPv6 only.
    pub fn beacon_socket6(name: &str) -> Result<std::net::UdpSocket, String> {
        use std::os::fd::FromRawFd;
        if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
            return Err("not an interface name".into());
        }
        let idx = ifindex(name).ok_or_else(|| format!("{name}: no such interface"))? as u32;
        // SAFETY: a new UDP6 socket; every setsockopt gets a pointer to a live value of the stated size; the descriptor is closed on
        // each failure path and otherwise handed to the returned `UdpSocket`.
        unsafe {
            let fd = libc::socket(libc::AF_INET6, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0);
            if fd < 0 {
                return Err(format!("IPv6 beacon socket: {}", std::io::Error::last_os_error()));
            }
            let fail = |fd: i32, what: &str| {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                Err(format!("{name}: {what}: {e}"))
            };
            let one: libc::c_int = 1;
            let zero: libc::c_int = 0;
            let int = std::mem::size_of::<libc::c_int>() as u32;
            for (lvl, opt, v) in [(libc::SOL_SOCKET, libc::SO_REUSEADDR, &one), (libc::SOL_SOCKET, libc::SO_REUSEPORT, &one), (libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, &one), (libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_LOOP, &zero)] {
                if libc::setsockopt(fd, lvl, opt, v as *const _ as *const libc::c_void, int) < 0 {
                    return fail(fd, "socket option");
                }
            }
            if libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_BINDTODEVICE, name.as_ptr() as *const libc::c_void, name.len() as u32) < 0 {
                return fail(fd, "bind to the interface");
            }
            if libc::setsockopt(fd, libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_IF, &idx as *const _ as *const libc::c_void, 4) < 0 {
                return fail(fd, "multicast interface");
            }
            let mut sin6: libc::sockaddr_in6 = std::mem::zeroed();
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = iyakki_shared::directlink::BEACON_PORT.to_be();
            if libc::bind(fd, &sin6 as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in6>() as u32) < 0 {
                return fail(fd, "bind");
            }
            let mreq = libc::ipv6_mreq { ipv6mr_multiaddr: libc::in6_addr { s6_addr: iyakki_shared::directlink::linklocal::BEACON_GROUP6.octets() }, ipv6mr_interface: idx };
            if libc::setsockopt(fd, libc::IPPROTO_IPV6, libc::IPV6_ADD_MEMBERSHIP, &mreq as *const _ as *const libc::c_void, std::mem::size_of::<libc::ipv6_mreq>() as u32) < 0 {
                return fail(fd, "join the all-nodes group");
            }
            Ok(std::net::UdpSocket::from_raw_fd(fd))
        }
    }

    pub fn beacon_socket(name: &str) -> Result<std::net::UdpSocket, String> {
        use std::os::fd::AsRawFd;
        if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
            return Err("not an interface name".into());
        }
        // every socket of ours shares the port (SO_REUSEADDR), so the manager and a one-off scan can both listen
        let s = reuse_bind().map_err(|e| format!("beacon port {}: {e}", iyakki_shared::directlink::BEACON_PORT))?;
        // SAFETY: setsockopt on a valid descriptor with a pointer and length of a byte string that lives through the call.
        let r = unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, name.as_ptr() as *const libc::c_void, name.len() as u32) };
        if r < 0 {
            return Err(format!("{name}: bind to the interface: {}", std::io::Error::last_os_error()));
        }
        s.set_broadcast(true).map_err(|e| e.to_string())?;
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(s)
    }

    fn reuse_bind() -> Result<std::net::UdpSocket, std::io::Error> {
        use std::os::fd::FromRawFd;
        // SAFETY: a new UDP socket with SO_REUSEADDR and SO_REUSEPORT set, bound to 0.0.0.0:BEACON_PORT with a fully initialised
        // `sockaddr_in`; on failure the descriptor is closed, on success ownership passes to the returned `UdpSocket`.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let one: libc::c_int = 1;
            for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
                libc::setsockopt(fd, libc::SOL_SOCKET, opt, &one as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32);
            }
            let mut sin: libc::sockaddr_in = std::mem::zeroed();
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = iyakki_shared::directlink::BEACON_PORT.to_be();
            if libc::bind(fd, &sin as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            Ok(std::net::UdpSocket::from_raw_fd(fd))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub mod sys {
    use iyakki_shared::directlink::linklocal::Probe;
    use std::net::Ipv4Addr;
    pub fn set_addr(_: &str, _: Ipv4Addr, _: u8) -> Result<(), String> {
        Err("not available on this OS".into())
    }
    pub fn arp_probe(_: &str, _: [u8; 6], _: Ipv4Addr) -> Probe {
        Probe::Failed
    }
    pub fn arp_announce(_: &str, _: [u8; 6], _: Ipv4Addr) {}
    pub fn beacon_socket(_: &str) -> Result<std::net::UdpSocket, String> {
        Err("not available on this OS".into())
    }
    pub fn beacon_socket6(_: &str) -> Result<std::net::UdpSocket, String> {
        Err("not available on this OS".into())
    }
    pub fn ifindex6(_: &str) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyk-linksys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn interfaces_are_read_with_flags_speed_addresses_and_link_local_v6_and_loopback_is_left_out() {
        let d = tree("ifaces");
        let (sys, proc_) = (d.join("sys"), d.join("proc"));
        put(&sys, "class/net/lo/flags", "0x9\n");
        put(&sys, "class/net/thunderbolt0/flags", "0x1003\n");
        put(&sys, "class/net/thunderbolt0/address", "02:11:22:33:44:55\n");
        put(&sys, "class/net/thunderbolt0/carrier", "1\n");
        put(&sys, "class/net/thunderbolt0/speed", "10000\n");
        put(&sys, "class/net/eth0/flags", "0x1002\n");
        put(&sys, "class/net/eth0/carrier", "0\n");
        put(&sys, "class/net/eth0/speed", "-1\n");
        put(&sys, "class/net/bad name/flags", "0x1\n");
        put(&proc_, "net/if_inet6", "fe80000000000000021122fffe334455 05 40 20 80 thunderbolt0\n20010db8000000000000000000000001 05 40 00 80 thunderbolt0\n");
        let v4 = vec![("thunderbolt0".to_string(), "169.254.7.9".to_string()), ("thunderbolt0:1".to_string(), "169.254.7.10".to_string()), ("eth0".to_string(), "10.0.0.2".to_string())];
        let list = net_ifaces(&sys, &proc_, &v4);
        let names: Vec<&str> = list.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["eth0", "thunderbolt0"]);
        let tb = &list[1];
        assert!(tb.up && tb.carrier);
        assert_eq!(tb.speed_mbps, Some(10000));
        assert_eq!(tb.mac, "02:11:22:33:44:55");
        assert_eq!(tb.ipv4.len(), 2);
        assert_eq!(tb.ipv6_ll, ["fe80:0:0:0:211:22ff:fe33:4455%thunderbolt0"]);
        let eth = &list[0];
        assert!(!eth.up && !eth.carrier);
        assert_eq!(eth.speed_mbps, None);
        assert_eq!(eth.ipv4.len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_net_folder_is_no_interfaces_and_the_udc_folder_must_have_an_entry() {
        let d = tree("udc");
        assert!(net_ifaces(&d, &d, &[]).is_empty());
        assert!(!udc_present(&d));
        std::fs::create_dir_all(d.join("class/udc")).unwrap();
        assert!(!udc_present(&d));
        put(&d, "class/udc/fe800000.usb", "");
        assert!(udc_present(&d));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn interface_names_are_checked_and_only_link_local_v6_lines_of_that_name_are_taken() {
        assert!(valid_ifname("enx0a-1_b.2"));
        assert!(!valid_ifname("") && !valid_ifname("0123456789abcdef") && !valid_ifname("a/b") && !valid_ifname("a b"));
        let t = "fe800000000000000000000000000001 02 40 20 80 eth0\nfe800000000000000000000000000002 03 40 20 80 eth1\nshort line\n";
        assert_eq!(ipv6_ll_from(t, "eth0"), ["fe80:0:0:0:0:0:0:1%eth0"]);
        assert!(ipv6_ll_from(t, "eth9").is_empty());
    }

    #[test]
    fn interrupts_are_read_from_the_given_root_or_none() {
        let d = tree("irq");
        assert!(interrupts_text(&d).is_none());
        put(&d, "interrupts", " 30: 4 0 PCI thunderbolt\n");
        assert!(interrupts_text(&d).unwrap().contains("thunderbolt"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_thunderbolt_approval_writes_the_key_first_and_refuses_bad_names_and_keys() {
        let d = tree("tb");
        let sys = d.join("sys");
        put(&sys, "bus/thunderbolt/devices/0-1/authorized", "0");
        let key = "ab".repeat(32);
        tb_write_authorized(&sys, "0-1", 2, Some(&key)).unwrap();
        assert_eq!(std::fs::read_to_string(sys.join("bus/thunderbolt/devices/0-1/key")).unwrap(), key);
        assert_eq!(std::fs::read_to_string(sys.join("bus/thunderbolt/devices/0-1/authorized")).unwrap(), "2");
        assert!(tb_write_authorized(&sys, "../x", 1, None).is_err());
        assert!(tb_write_authorized(&sys, "", 1, None).is_err());
        assert!(tb_write_authorized(&sys, "0-1", 2, Some("short")).is_err());
        assert!(tb_write_authorized(&sys, "0-1", 2, Some(&"zz".repeat(32))).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_address_and_socket_calls_refuse_bad_interface_names_and_prefixes_before_any_system_call() {
        use std::net::Ipv4Addr;
        assert!(sys::set_addr("a/b", Ipv4Addr::new(169, 254, 1, 1), 16).is_err());
        assert!(sys::set_addr("eth0", Ipv4Addr::new(169, 254, 1, 1), 33).is_err());
        assert!(sys::set_addr("", Ipv4Addr::new(169, 254, 1, 1), 16).is_err());
        assert!(sys::beacon_socket("a/b").is_err());
        assert!(sys::beacon_socket6("").is_err());
        assert_eq!(sys::ifindex6("no-such-if0"), None);
        assert_eq!(sys::arp_probe("no-such-if0", [2, 0, 0, 0, 0, 1], Ipv4Addr::new(169, 254, 1, 1)), iyakki_shared::directlink::linklocal::Probe::Failed);
    }
}
