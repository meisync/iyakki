// SPDX-License-Identifier: Apache-2.0
use super::*;

fn le(v: u32) -> [u8; 4] {
    v.to_ne_bytes()
}

#[test]
fn the_link_message_is_byte_for_byte_what_ip_link_set_up_sends() {
    let m = link_set(7, 5, true);
    let mut want = Vec::new();
    want.extend_from_slice(&le(32)); // length
    want.extend_from_slice(&16u16.to_ne_bytes()); // RTM_NEWLINK
    want.extend_from_slice(&5u16.to_ne_bytes()); // request + ack
    want.extend_from_slice(&le(7));
    want.extend_from_slice(&le(0));
    want.extend_from_slice(&[0, 0, 0, 0]); // family, pad, type
    want.extend_from_slice(&le(5)); // index
    want.extend_from_slice(&le(1)); // flags: up
    want.extend_from_slice(&le(1)); // change: up only
    assert_eq!(m, want);
    // down clears the flag and still touches only the up bit
    let d = link_set(8, 5, false);
    assert_eq!(&d[24..32], &[0u8, 0, 0, 0, 1, 0, 0, 0]);
}

#[test]
fn the_address_messages_carry_the_local_address_the_peer_address_and_the_broadcast_of_the_network() {
    let m = addr_add(1, 9, Ipv4Addr::new(10, 42, 0, 1), 24).unwrap();
    let ty = u16::from_ne_bytes([m[4], m[5]]);
    let flags = u16::from_ne_bytes([m[6], m[7]]);
    assert_eq!((ty, flags), (20, 1 | 4 | 0x400 | 0x100));
    assert_eq!(u32::from_ne_bytes(m[0..4].try_into().unwrap()) as usize, m.len());
    assert_eq!(&m[16..20], &[2, 24, 0, 0], "family, prefix, flags, scope");
    assert_eq!(&m[20..24], &le(9));
    // three attributes of 8 bytes each: local, address, broadcast
    assert_eq!(&m[24..], &[8, 0, 2, 0, 10, 42, 0, 1, 8, 0, 1, 0, 10, 42, 0, 1, 8, 0, 4, 0, 10, 42, 0, 255]);
    let d = addr_del(2, 9, Ipv4Addr::new(10, 42, 0, 1), 24).unwrap();
    assert_eq!(u16::from_ne_bytes([d[4], d[5]]), 21);
    assert_eq!(&d[24..], &[8, 0, 2, 0, 10, 42, 0, 1, 8, 0, 1, 0, 10, 42, 0, 1]);
    // a /32 and a /30 have the right broadcast; nonsense prefixes are refused before anything is sent
    assert_eq!(&addr_add(1, 1, Ipv4Addr::new(192, 168, 1, 1), 32).unwrap()[44..], &[192, 168, 1, 1]);
    assert_eq!(&addr_add(1, 1, Ipv4Addr::new(192, 168, 1, 5), 30).unwrap()[44..], &[192, 168, 1, 7]);
    assert!(addr_add(1, 1, Ipv4Addr::new(1, 1, 1, 1), 0).is_err() && addr_add(1, 1, Ipv4Addr::new(1, 1, 1, 1), 33).is_err() && addr_del(1, 1, Ipv4Addr::new(1, 1, 1, 1), 33).is_err());
}

fn ack(errno: i32) -> Vec<u8> {
    let mut m = header(36, NLMSG_ERROR, 0, 1);
    m.extend_from_slice(&(-errno).to_ne_bytes());
    m.extend_from_slice(&[0; 16]);
    m
}

#[test]
fn an_answer_is_an_ack_or_an_error_in_words_and_garbage_is_never_trusted() {
    assert!(check_ack(&ack(0)).is_ok());
    assert!(check_ack(&ack(1)).unwrap_err().contains("administrator"));
    assert!(check_ack(&ack(19)).unwrap_err().contains("no such device"));
    assert!(check_ack(&ack(99)).unwrap_err().contains("cannot be assigned"));
    assert!(check_ack(&ack(5)).unwrap_err().contains("error 5"));
    assert!(check_ack(&[]).is_err());
    let good = ack(0);
    for n in 0..good.len() {
        // a cut answer is never taken for an ack
        if n < 20 {
            assert!(check_ack(&good[..n]).is_err(), "cut at {n}");
        }
    }
    let mut lying = ack(0);
    lying[0..4].copy_from_slice(&le(4000));
    assert!(check_ack(&lying).is_err());
}

struct Fake {
    sent: Vec<Vec<u8>>,
    errno: i32,
}

impl RtnlTransport for Fake {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String> {
        self.sent.push(request.to_vec());
        Ok(ack(self.errno))
    }
}

#[test]
fn the_calls_use_a_fresh_sequence_number_each_and_report_the_kernels_refusal() {
    let mut r = Rtnl::new(Fake { sent: vec![], errno: 0 });
    r.link_set(4, true).unwrap();
    r.addr_add(4, Ipv4Addr::new(10, 0, 0, 1), 24).unwrap();
    r.addr_del(4, Ipv4Addr::new(10, 0, 0, 1), 24).unwrap();
    let seqs: Vec<u32> = r.transport().sent.iter().map(|m| u32::from_ne_bytes(m[8..12].try_into().unwrap())).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    let types: Vec<u16> = r.transport().sent.iter().map(|m| u16::from_ne_bytes([m[4], m[5]])).collect();
    assert_eq!(types, vec![16, 20, 21]);
    let mut bad = Rtnl::new(Fake { sent: vec![], errno: 1 });
    assert!(bad.link_set(4, true).unwrap_err().contains("administrator"));
    // a wrong prefix is refused before the transport sees anything
    assert!(bad.addr_add(4, Ipv4Addr::new(10, 0, 0, 1), 40).is_err());
    assert_eq!(bad.transport().sent.len(), 1);
    // names that are not interface names are refused
    assert!(ifindex("../etc").is_err() && ifindex("").is_err());
    assert!(ifindex("no-such-iface0").is_err());
}
