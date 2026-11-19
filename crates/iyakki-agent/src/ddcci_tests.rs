//! DDC/CI (F-KBD-08): message bytes and checksums, replies, the EDID, and the rules, against a fake monitor on a fake bus.

use super::*;

/// A monitor: EDID at 0x50, brightness at 0x37, with the quirks real ones have.
pub(crate) struct Monitor {
    pub(crate) edid: Vec<u8>,
    pub(crate) value: u16,
    pub(crate) max: u16,
    /// Answers the first N reads with garbage.
    pub(crate) flaky: u32,
    pub(crate) ignores_set: bool,
    pub(crate) unsupported: bool,
    pub(crate) log: Vec<(u16, Vec<u8>)>,
    pub(crate) pending: Option<Vec<u8>>,
}

fn edid_block(maker: [char; 3], product: u16, serial: u32, name: &str) -> Vec<u8> {
    let mut b = vec![0u8; 128];
    b[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
    let l = |c: char| c as u16 - 'A' as u16 + 1;
    let m = (l(maker[0]) << 10) | (l(maker[1]) << 5) | l(maker[2]);
    b[8..10].copy_from_slice(&m.to_be_bytes());
    b[10..12].copy_from_slice(&product.to_le_bytes());
    b[12..16].copy_from_slice(&serial.to_le_bytes());
    let o = 54 + 18;
    b[o + 3] = 0xFC;
    let mut n = name.as_bytes().to_vec();
    n.push(b'\n');
    n.resize(13, b' ');
    b[o + 5..o + 18].copy_from_slice(&n);
    let sum = b[..127].iter().fold(0u8, |a, x| a.wrapping_add(*x));
    b[127] = 0u8.wrapping_sub(sum);
    b
}

pub(crate) fn monitor(serial: u32) -> Monitor {
    Monitor { edid: edid_block(['D', 'E', 'L'], 0xA0C5, serial, "DELL U2720Q"), value: 40, max: 100, flaky: 0, ignores_set: false, unsupported: false, log: vec![], pending: None }
}

fn reply(code: u8, result: u8, value: u16, max: u16) -> Vec<u8> {
    let mut r = vec![0x6E, 0x88, 0x02, result, code, 0x00, (max >> 8) as u8, max as u8, (value >> 8) as u8, value as u8];
    r.push(checksum(0x50, &r));
    r
}

impl I2cBus for Monitor {
    fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), String> {
        self.log.push((addr, data.to_vec()));
        if addr == EDID_ADDR {
            return Ok(());
        }
        if addr != DDC_ADDR {
            return Err("no device".into());
        }
        // the monitor checks the checksum like a real one: a wrong message is not answered
        let (body, sum) = data.split_at(data.len() - 1);
        if checksum(0x6E, body) != sum[0] {
            return Ok(());
        }
        match body[2] {
            0x01 => self.pending = Some(if self.unsupported { reply(body[3], 1, 0, 0) } else { reply(body[3], 0, self.value, self.max) }),
            0x03 if !self.ignores_set => self.value = u16::from_be_bytes([body[4], body[5]]).min(self.max),
            _ => {}
        }
        Ok(())
    }
    fn read(&mut self, addr: u16, len: usize) -> Result<Vec<u8>, String> {
        if addr == EDID_ADDR {
            return Ok(self.edid.clone());
        }
        if self.flaky > 0 {
            self.flaky -= 1;
            return Ok(vec![0xFF; len]);
        }
        self.pending.take().ok_or_else(|| "nothing to read".to_string())
    }
}

#[test]
fn the_requests_have_the_bytes_and_checksums_ddc_ci_defines() {
    assert_eq!(get_vcp_request(0x10), vec![0x51, 0x82, 0x01, 0x10, 0xAC]);
    assert_eq!(set_vcp_request(0x10, 0x0032), vec![0x51, 0x84, 0x03, 0x10, 0x00, 0x32, 0x9A ^ 0x00 ^ 0x00 ^ 0x00]
        .into_iter().take(6).chain([checksum(0x6E, &[0x51, 0x84, 0x03, 0x10, 0x00, 0x32])]).collect::<Vec<u8>>());
    assert_eq!(checksum(0x6E, &[0x51, 0x82, 0x01, 0x10]), 0xAC);
}

#[test]
fn a_reply_is_read_and_a_damaged_wrong_or_refusing_one_is_named() {
    assert_eq!(parse_get_reply(&reply(0x10, 0, 40, 100), 0x10), Ok(Vcp { code: 0x10, value: 40, max: 100 }));
    let mut bad = reply(0x10, 0, 40, 100);
    bad[8] ^= 1;
    assert_eq!(parse_get_reply(&bad, 0x10), Err(DdcError::BadChecksum));
    assert_eq!(parse_get_reply(&reply(0x10, 1, 0, 0), 0x10), Err(DdcError::Unsupported));
    assert_eq!(parse_get_reply(&reply(0x12, 0, 1, 2), 0x10), Err(DdcError::WrongCode));
    assert_eq!(parse_get_reply(&[0x6E, 0x88], 0x10), Err(DdcError::TooShort));
    assert_eq!(parse_get_reply(&[0xFF; 11], 0x10), Err(DdcError::NotAReply));
    assert_eq!(parse_get_reply(&reply(0x10, 0, 40, 100)[..8], 0x10), Err(DdcError::TooShort));
}

#[test]
fn the_edid_gives_maker_product_serial_and_name_and_a_damaged_block_is_refused() {
    let e = parse_edid(&edid_block(['D', 'E', 'L'], 0xA0C5, 0x1234_5678, "DELL U2720Q")).unwrap();
    assert_eq!((e.maker.as_str(), e.product, e.serial, e.name.as_str()), ("DEL", 0xA0C5, 0x1234_5678, "DELL U2720Q"));
    assert_eq!(e.id(), "DEL-a0c5-12345678");
    assert_eq!(parse_edid(&edid_block(['A', 'O', 'C'], 1, 2, "")).unwrap().label(), "AOC monitor 0001");
    let mut b = edid_block(['D', 'E', 'L'], 1, 2, "x");
    b[20] ^= 0xFF;
    assert_eq!(parse_edid(&b), Err(DdcError::BadEdid("checksum")));
    assert_eq!(parse_edid(&[0; 128]), Err(DdcError::BadEdid("no header")));
    assert_eq!(parse_edid(&[0; 10]), Err(DdcError::BadEdid("too short")));
}

#[test]
fn probing_only_reads_and_a_flaky_monitor_is_asked_again() {
    let mut m = monitor(7);
    let (e, b) = probe(&mut m).unwrap();
    assert_eq!((e.name.as_str(), b.percent(), b.value, b.max), ("DELL U2720Q", 40, 40, 100));
    assert!(m.log.iter().all(|(a, d)| *a == EDID_ADDR || d[2] == 0x01), "no write that changes anything: {:?}", m.log);
    let mut f = monitor(7);
    f.flaky = 2;
    assert_eq!(probe(&mut f).unwrap().1.value, 40, "two bad answers, the third is good");
    let mut dead = monitor(7);
    dead.flaky = 99;
    assert!(probe(&mut dead).is_err());
    let mut un = monitor(7);
    un.unsupported = true;
    assert_eq!(probe(&mut un).unwrap_err(), DdcError::Unsupported);
    assert_eq!(Brightness { value: 0, max: 0 }.percent(), 0);
    assert_eq!(Brightness { value: 255, max: 255 }.percent(), 100);
}

#[test]
fn a_monitor_is_written_to_only_when_chosen_by_its_id_and_the_value_is_read_back() {
    let mut m = monitor(7);
    let id = parse_edid(&m.edid.clone()).unwrap().id();
    // nobody chosen: refused, and nothing was written
    assert_eq!(Chosen::default().set_brightness(&mut m, 80), Err(DdcError::NotSelected));
    assert_eq!(Chosen(Some("DEL-a0c5-00000099".into())).set_brightness(&mut m, 80), Err(DdcError::NotSelected), "another monitor's id");
    assert_eq!(m.value, 40);
    assert!(m.log.iter().all(|(_, d)| d.get(2) != Some(&0x03)), "no set request was sent");
    // chosen: set, read back
    let b = Chosen(Some(id.clone())).set_brightness(&mut m, 80).unwrap();
    assert_eq!((b.value, b.percent()), (80, 80));
    assert_eq!(m.value, 80);
    // never fully dark
    assert_eq!(Chosen(Some(id.clone())).set_brightness(&mut m, 0).unwrap().value, 1);
    assert_eq!(Chosen(Some(id.clone())).set_brightness(&mut m, 250).unwrap().value, 100);
    // a monitor that ignores the write is reported, not believed
    let mut deaf = monitor(7);
    deaf.ignores_set = true;
    assert_eq!(Chosen(Some(id)).set_brightness(&mut deaf, 80), Err(DdcError::NotApplied { wanted: 80, got: 40 }));
}
