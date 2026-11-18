//! Tests for the WireGuard generic-netlink messages. The byte vectors are written by hand from the attribute numbers of the
//! kernel's `wireguard.h` (our reading of the UAPI, no copied code). Integers are in the machine's byte order, so the
//! vectors are checked on little-endian hosts only. All keys are FAKE (a repeated byte value).

use std::net::SocketAddr;

use crate::b64;
use crate::net::Cidr;
use crate::profile::PublicKey;
use crate::secret::Secret;
use crate::wgnl::*;
use crate::VpnError;

fn pk(n: u8) -> PublicKey {
    PublicKey([n; 32])
}

fn cidrs(v: &[&str]) -> Vec<Cidr> {
    v.iter().map(|s| Cidr::parse(s).unwrap()).collect()
}

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
}

#[cfg(target_endian = "little")]
#[test]
fn get_family_request_bytes() {
    let m = get_family_request(7);
    // len 36, type 0x10 (nlctrl), flags REQUEST|ACK, seq 7, port 0 | cmd GETFAMILY(3), version 1 | name attr "wireguard\0"
    let want = hex("20 00 00 00 10 00 05 00 07 00 00 00 00 00 00 00 03 01 00 00 0e 00 02 00 77 69 72 65 67 75 61 72 64 00 00 00");
    assert_eq!(m.len(), 0x24);
    let mut want = want;
    want[0] = 0x24;
    assert_eq!(m, want);
}

#[cfg(target_endian = "little")]
#[test]
fn set_device_with_a_port_only_matches_the_layout() {
    let d = DeviceSet { ifname: "wg0", private_key: None, listen_port: Some(51820), fwmark: None, replace_peers: false, peers: vec![] };
    let m = set_device(0x1c, 1, &d).unwrap();
    let want = hex(
        "24 00 00 00 1c 00 05 00 01 00 00 00 00 00 00 00 01 01 00 00 \
         08 00 02 00 77 67 30 00 \
         06 00 06 00 6c ca 00 00",
    );
    assert_eq!(m.as_bytes(), &want[..]);
}

#[cfg(target_endian = "little")]
#[test]
fn set_device_with_key_flags_and_one_peer_matches_the_layout() {
    let key = Secret::new(vec![0x55; 32]);
    let psk = Secret::new(vec![0x66; 32]);
    let p = pk(0x01);
    let allowed = cidrs(&["10.0.0.0/24"]);
    let mut peer = PeerSet::new(&p, &allowed);
    peer.endpoint = Some("192.0.2.1:51820".parse().unwrap());
    peer.keepalive = Some(25);
    peer.preshared = Some(&psk);
    peer.replace_allowed_ips = true;
    let d = DeviceSet { ifname: "wg0", private_key: Some(&key), listen_port: None, fwmark: None, replace_peers: true, peers: vec![peer] };
    let m = set_device(0x1c, 2, &d).unwrap();
    let b = m.as_bytes();
    let rep = |n: u8, c: usize| vec![n; c];
    let mut want = hex("00 00 00 00 1c 00 05 00 02 00 00 00 00 00 00 00 01 01 00 00");
    want.extend(hex("08 00 02 00 77 67 30 00")); // IFNAME
    want.extend(hex("08 00 05 00 01 00 00 00")); // FLAGS = REPLACE_PEERS
    want.extend(hex("24 00 03 00"));
    want.extend(rep(0x55, 32)); // PRIVATE_KEY
    // PEERS (nested 8) > element 0 (nested) > peer attributes
    let mut peer_attrs = Vec::new();
    peer_attrs.extend(hex("24 00 01 00"));
    peer_attrs.extend(rep(0x01, 32)); // PUBLIC_KEY
    peer_attrs.extend(hex("08 00 03 00 02 00 00 00")); // FLAGS = REPLACE_ALLOWEDIPS
    peer_attrs.extend(hex("24 00 02 00"));
    peer_attrs.extend(rep(0x66, 32)); // PRESHARED_KEY
    peer_attrs.extend(hex("14 00 04 00 02 00 ca 6c c0 00 02 01 00 00 00 00 00 00 00 00")); // ENDPOINT sockaddr_in, port big endian
    peer_attrs.extend(hex("06 00 05 00 19 00 00 00")); // KEEPALIVE 25
    peer_attrs.extend(hex(
        "20 00 09 80  1c 00 00 80  06 00 01 00 02 00 00 00  08 00 02 00 0a 00 00 00  05 00 03 00 18 00 00 00", // ALLOWEDIPS > 0 > family, ip, mask
    ));
    let el_len = 4 + peer_attrs.len();
    let mut el = vec![el_len as u8, (el_len >> 8) as u8, 0x00, 0x80];
    el.extend(peer_attrs);
    let peers_len = 4 + el.len();
    want.extend([peers_len as u8, (peers_len >> 8) as u8, 0x08, 0x80]);
    want.extend(el);
    let total = want.len() as u32;
    want[0..4].copy_from_slice(&total.to_ne_bytes());
    assert_eq!(b, &want[..]);
}

#[test]
fn ipv6_endpoint_and_allowed_ips_are_encoded() {
    let p = pk(2);
    let allowed = cidrs(&["fd00::/64", "::/0"]);
    let mut peer = PeerSet::new(&p, &allowed);
    peer.endpoint = Some("[2001:db8::1]:443".parse().unwrap());
    let m = set_device(0x1c, 1, &DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: false, peers: vec![peer] }).unwrap();
    // the kernel reads a 28 byte sockaddr_in6 here
    let b = m.as_bytes();
    let pos = b.windows(4).position(|w| w == [0x20, 0x00, 0x04, 0x00]).expect("endpoint attribute");
    assert_eq!(&b[pos + 4..pos + 6], &10u16.to_ne_bytes());
    assert_eq!(&b[pos + 6..pos + 8], &443u16.to_be_bytes());
    assert_eq!(&b[pos + 12..pos + 28], &"2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap().octets());
}

#[test]
fn bad_input_is_refused_before_a_message_exists() {
    let key = Secret::new(vec![1; 31]);
    let ok = DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: false, peers: vec![] };
    for n in ["", "a/b", "wg 0", "0123456789abcdef", "wg0\0", "wg0\n", "../x"] {
        assert!(set_device(1, 1, &DeviceSet { ifname: n, ..ok.clone() }).is_err(), "{n:?}");
        assert!(get_device(1, 1, n).is_err(), "{n:?}");
    }
    assert!(set_device(1, 1, &DeviceSet { private_key: Some(&key), ..ok.clone() }).is_err());
    let p = pk(1);
    let mut peer = PeerSet::new(&p, &[]);
    peer.preshared = Some(&key);
    assert!(set_device(1, 1, &DeviceSet { peers: vec![peer], ..ok }).is_err());
}

fn configured_client() -> WgClient<FakeGenl> {
    let mut f = FakeGenl::new();
    f.add_device("wg0");
    let mut c = WgClient::new(f);
    let key = Secret::new(vec![0x55; 32]);
    let psk = Secret::new(vec![0x66; 32]);
    let (a, b) = (pk(0xA1), pk(0xA2));
    let ips_a = cidrs(&["0.0.0.0/1", "128.0.0.0/1", "::/1"]);
    let ips_b = cidrs(&["10.8.0.0/24"]);
    let mut pa = PeerSet::new(&a, &ips_a);
    pa.endpoint = Some("203.0.113.9:51820".parse().unwrap());
    pa.keepalive = Some(25);
    pa.preshared = Some(&psk);
    let pb = PeerSet::new(&b, &ips_b);
    c.configure(&DeviceSet { ifname: "wg0", private_key: Some(&key), listen_port: Some(40000), fwmark: None, replace_peers: true, peers: vec![pa, pb] }).unwrap();
    c
}

#[test]
fn configure_then_read_back_through_the_fake_kernel() {
    let mut c = configured_client();
    {
        let d = c.transport().device("wg0").unwrap();
        assert!(d.has_private_key);
        assert_eq!(d.listen_port, 40000);
        assert_eq!(d.peers.len(), 2);
        assert_eq!(d.preshared_for, vec![pk(0xA1)]);
    }
    let st = c.status("wg0").unwrap();
    assert_eq!(st.ifname, "wg0");
    assert_eq!(st.listen_port, 40000);
    assert_eq!(st.public_key, Some(pk(0xAB)));
    assert_eq!(st.peers.len(), 2);
    let a = &st.peers[0];
    assert_eq!(a.public_key, pk(0xA1));
    assert_eq!(a.endpoint, Some("203.0.113.9:51820".parse::<SocketAddr>().unwrap()));
    assert_eq!(a.keepalive, 25);
    assert_eq!(a.last_handshake, None);
    assert_eq!(a.allowed_ips, cidrs(&["0.0.0.0/1", "128.0.0.0/1", "::/1"]));
    // the family is resolved once: GETFAMILY, SET, GET
    assert_eq!(c.transport().requests.len(), 3);
}

#[test]
fn status_carries_public_data_only() {
    let mut c = configured_client();
    let st = c.status("wg0").unwrap();
    let text = format!("{st:?}");
    // the fake kernel returns the private key (0x55) and preshared key (0x66) like the real one; the decoder drops them
    assert!(!text.contains(&b64::encode(&[0x55; 32])) && !text.contains(&b64::encode(&[0x66; 32])));
    assert!(text.contains(&b64::encode(&[0xAB; 32])));
}

#[test]
fn handshake_and_counters_are_decoded() {
    let mut c = configured_client();
    {
        let p = &mut c.transport_mut().devices[0].peers[0];
        p.last_handshake = Some(1_700_000_000);
        p.rx_bytes = 1234;
        p.tx_bytes = 5678;
    }
    let st = c.status("wg0").unwrap();
    assert_eq!((st.peers[0].last_handshake, st.peers[0].rx_bytes, st.peers[0].tx_bytes), (Some(1_700_000_000), 1234, 5678));
}

#[test]
fn dumps_split_over_messages_are_merged() {
    let mut c = configured_client();
    c.transport_mut().split_peers = true;
    let st = c.status("wg0").unwrap();
    assert_eq!(st.peers.len(), 2);
    assert_eq!(st.listen_port, 40000);
}

#[test]
fn replace_remove_and_update_behave_like_the_kernel_flags() {
    let mut c = configured_client();
    let (a, b) = (pk(0xA1), pk(0xA2));
    // remove one peer
    let mut rm = PeerSet::new(&b, &[]);
    rm.remove = true;
    c.configure(&DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: false, peers: vec![rm] }).unwrap();
    assert_eq!(c.transport().device("wg0").unwrap().peers.len(), 1);
    // replace the allowed IPs of the other
    let new_ips = cidrs(&["192.0.2.0/24"]);
    let mut up = PeerSet::new(&a, &new_ips);
    up.replace_allowed_ips = true;
    c.configure(&DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: false, peers: vec![up] }).unwrap();
    assert_eq!(c.transport().device("wg0").unwrap().peers[0].allowed_ips, new_ips);
    // replace_peers with an empty list clears everything
    c.configure(&DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: true, peers: vec![] }).unwrap();
    assert!(c.transport().device("wg0").unwrap().peers.is_empty());
}

#[test]
fn kernel_errors_come_back_with_a_readable_reason() {
    let mut c = configured_client();
    let d = |n| DeviceSet { ifname: n, private_key: None, listen_port: Some(1), fwmark: None, replace_peers: false, peers: vec![] };
    let e = c.configure(&d("wg9")).unwrap_err();
    assert!(matches!(&e, VpnError::Netlink(m) if m.contains("no such device")), "{e}");
    c.transport_mut().fail_next = Some(1);
    let e = c.configure(&d("wg0")).unwrap_err();
    assert!(matches!(&e, VpnError::Netlink(m) if m.contains("CAP_NET_ADMIN")), "{e}");
    c.transport_mut().fail_next = Some(22);
    assert!(c.configure(&d("wg0")).is_err());
    // the next call works again
    c.configure(&d("wg0")).unwrap();
    assert!(c.status("wg9").is_err());

    let mut f = FakeGenl::new();
    f.no_family = true;
    let e = WgClient::new(f).status("wg0").unwrap_err();
    assert!(matches!(&e, VpnError::Netlink(_)), "{e}");
}

#[test]
fn errors_never_contain_key_bytes() {
    let mut c = configured_client();
    c.transport_mut().fail_next = Some(22);
    let key = Secret::new(vec![0x55; 32]);
    let e = c.configure(&DeviceSet { ifname: "wg0", private_key: Some(&key), listen_port: None, fwmark: None, replace_peers: false, peers: vec![] }).unwrap_err();
    assert!(!e.to_string().contains(&b64::encode(&[0x55; 32])));
}

#[test]
fn truncated_or_corrupt_replies_are_errors_not_panics() {
    let mut c = configured_client();
    let req = get_device(0x1c, 9, "wg0").unwrap();
    let reply = c.transport_mut().exchange(&req).unwrap();
    let whole: Vec<u8> = reply[0].clone();
    for cut in 0..whole.len() {
        let _ = decode_device(&[whole[..cut].to_vec()]);
        let _ = split_messages(&whole[..cut]);
    }
    // flip every byte in turn
    for i in 0..whole.len() {
        let mut m = whole.clone();
        m[i] ^= 0xff;
        let _ = decode_device(&[m]);
    }
    assert!(split_messages(&[1, 2, 3]).is_err());
    assert!(parse_family_id(&[]).is_err());
    assert!(decode_device(&[]).is_err());
}

#[test]
fn get_device_is_a_dump_request_for_the_name() {
    let m = get_device(0x1c, 3, "wg0").unwrap();
    assert_eq!(u16::from_ne_bytes([m[6], m[7]]), 0x301); // REQUEST | ROOT | MATCH
    assert_eq!(m[16], WG_CMD_GET_DEVICE);
    assert_eq!(m[17], 1);
    assert!(m.windows(4).any(|w| w == b"wg0\0"));
}
