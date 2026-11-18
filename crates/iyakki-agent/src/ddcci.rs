//! External monitor brightness over DDC/CI (F-KBD-08): the messages, the monitor's EDID and the rules, with the I2C bus behind a trait so
//! everything here is tested against a fake monitor.
//!
//! * A Get VCP Feature request is `51 82 01 <code> <checksum>` written to the monitor's I2C address 0x37; the reply (read after the monitor
//!   had about 40 ms) is `6E <0x80 | length> 02 <result> <code> <type> <max hi> <max lo> <now hi> <now lo> <checksum>`. A Set VCP Feature
//!   request is `51 84 03 <code> <value hi> <value lo> <checksum>`. The checksum is the XOR of every byte of the message including the
//!   destination address as the bus sends it (0x6E for a write to 0x37, 0x50 for a reply).
//! * Brightness is VCP code 0x10, a value from 0 to the monitor's own maximum (often 100).
//! * Reading is always allowed (probing only reads: the EDID at 0x50 and the brightness). Writing needs the person to have SELECTED that
//!   monitor, by the id built from its EDID; a monitor that was not selected is never written to, and every write is read back.
//!
//! What is NOT here: opening `/dev/i2c-N` and finding which bus belongs to which screen. That needs the `i2c-dev` driver in the kernel
//! (not in the image yet) and a monitor that answers; many do not.

pub const DDC_ADDR: u16 = 0x37;
pub const EDID_ADDR: u16 = 0x50;
pub const VCP_BRIGHTNESS: u8 = 0x10;
/// Bytes of a Get VCP Feature reply.
pub const REPLY_LEN: usize = 11;
const HOST: u8 = 0x51;
const WRITE_ADDR: u8 = 0x6E;
const REPLY_ADDR: u8 = 0x50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdcError {
    /// The bus said no (no such device, a timeout).
    Bus(String),
    TooShort,
    BadChecksum,
    NotAReply,
    /// The monitor says it does not know this VCP code.
    Unsupported,
    WrongCode,
    NotSelected,
    /// What was written did not read back.
    NotApplied { wanted: u16, got: u16 },
    BadEdid(&'static str),
}

impl std::fmt::Display for DdcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdcError::Bus(e) => write!(f, "the monitor's bus answered: {e}"),
            DdcError::TooShort => write!(f, "the monitor's answer was cut short"),
            DdcError::BadChecksum => write!(f, "the monitor's answer was damaged (checksum)"),
            DdcError::NotAReply => write!(f, "the monitor did not answer the way DDC/CI does"),
            DdcError::Unsupported => write!(f, "this monitor does not support that setting over DDC/CI"),
            DdcError::WrongCode => write!(f, "the monitor answered about another setting"),
            DdcError::NotSelected => write!(f, "this monitor was not chosen; choose it first"),
            DdcError::NotApplied { wanted, got } => write!(f, "the monitor did not take the value {wanted} (it reads back {got})"),
            DdcError::BadEdid(w) => write!(f, "the monitor's identity block is not valid: {w}"),
        }
    }
}

/// The XOR checksum: `start` (the address byte the bus adds) with every byte of `bytes`.
pub fn checksum(start: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(start, |a, b| a ^ b)
}

/// What to write to address 0x37 to ask for a setting.
pub fn get_vcp_request(code: u8) -> Vec<u8> {
    let mut m = vec![HOST, 0x82, 0x01, code];
    m.push(checksum(WRITE_ADDR, &m));
    m
}

/// What to write to address 0x37 to set a setting.
pub fn set_vcp_request(code: u8, value: u16) -> Vec<u8> {
    let mut m = vec![HOST, 0x84, 0x03, code, (value >> 8) as u8, value as u8];
    m.push(checksum(WRITE_ADDR, &m));
    m
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vcp {
    pub code: u8,
    pub value: u16,
    pub max: u16,
}

/// Read a Get VCP Feature reply for `code`.
pub fn parse_get_reply(b: &[u8], code: u8) -> Result<Vcp, DdcError> {
    if b.len() < 3 {
        return Err(DdcError::TooShort);
    }
    if b[0] != WRITE_ADDR || b[1] & 0x80 == 0 || b[2] != 0x02 {
        return Err(DdcError::NotAReply);
    }
    let len = (b[1] & 0x7F) as usize;
    if len != 8 {
        return Err(if len < 8 { DdcError::TooShort } else { DdcError::NotAReply });
    }
    if b.len() < 3 + len {
        return Err(DdcError::TooShort);
    }
    let end = 2 + len;
    if checksum(REPLY_ADDR, &b[..end]) != b[end] {
        return Err(DdcError::BadChecksum);
    }
    if b[3] == 0x01 {
        return Err(DdcError::Unsupported);
    }
    if b[3] != 0x00 {
        return Err(DdcError::NotAReply);
    }
    if b[4] != code {
        return Err(DdcError::WrongCode);
    }
    Ok(Vcp { code, max: u16::from_be_bytes([b[6], b[7]]), value: u16::from_be_bytes([b[8], b[9]]) })
}

/// An I2C bus a monitor sits on. The real one is a file `/dev/i2c-N` (not written here); tests use a fake monitor.
pub trait I2cBus {
    fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), String>;
    fn read(&mut self, addr: u16, len: usize) -> Result<Vec<u8>, String>;
    /// The monitor needs about 40 ms between a request and its reply (a fake may skip it).
    fn pause(&mut self, _ms: u64) {}
}

/// Ask for a setting (up to three tries: monitors drop a message now and then).
pub fn get_vcp(bus: &mut dyn I2cBus, code: u8) -> Result<Vcp, DdcError> {
    let mut last = DdcError::TooShort;
    for _ in 0..3 {
        bus.write(DDC_ADDR, &get_vcp_request(code)).map_err(DdcError::Bus)?;
        bus.pause(40);
        match bus.read(DDC_ADDR, REPLY_LEN).map_err(DdcError::Bus).and_then(|r| parse_get_reply(&r, code)) {
            Ok(v) => return Ok(v),
            Err(DdcError::Unsupported) => return Err(DdcError::Unsupported),
            Err(e) => last = e,
        }
        bus.pause(40);
    }
    Err(last)
}

/// A monitor's identity from its 128-byte EDID block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edid {
    /// Three letters, e.g. `DEL`.
    pub maker: String,
    pub product: u16,
    pub serial: u32,
    /// The name in the block (`DELL U2720Q`), empty when it has none.
    pub name: String,
}

impl Edid {
    /// A short stable text the person's choice is kept under: maker, product and serial.
    pub fn id(&self) -> String {
        format!("{}-{:04x}-{:08x}", self.maker, self.product, self.serial)
    }

    pub fn label(&self) -> String {
        if self.name.is_empty() { format!("{} monitor {:04x}", self.maker, self.product) } else { self.name.clone() }
    }
}

pub fn parse_edid(b: &[u8]) -> Result<Edid, DdcError> {
    if b.len() < 128 {
        return Err(DdcError::BadEdid("too short"));
    }
    if b[..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
        return Err(DdcError::BadEdid("no header"));
    }
    if b[..128].iter().fold(0u8, |a, x| a.wrapping_add(*x)) != 0 {
        return Err(DdcError::BadEdid("checksum"));
    }
    let m = u16::from_be_bytes([b[8], b[9]]);
    let letter = |v: u16| if (1..=26).contains(&v) { Some((b'A' + v as u8 - 1) as char) } else { None };
    let maker: String = [letter((m >> 10) & 31), letter((m >> 5) & 31), letter(m & 31)].into_iter().collect::<Option<String>>().ok_or(DdcError::BadEdid("maker"))?;
    let product = u16::from_le_bytes([b[10], b[11]]);
    let serial = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
    let mut name = String::new();
    for d in 0..4 {
        let o = 54 + d * 18;
        if b[o] == 0 && b[o + 1] == 0 && b[o + 3] == 0xFC {
            name = b[o + 5..o + 18].iter().take_while(|c| **c != b'\n' && **c != 0).filter(|c| (0x20..0x7F).contains(*c)).map(|c| *c as char).collect::<String>().trim().to_string();
        }
    }
    Ok(Edid { maker, product, serial, name })
}

/// The brightness of a monitor as a percent of its own range, with the raw numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brightness {
    pub value: u16,
    pub max: u16,
}

impl Brightness {
    pub fn percent(self) -> u8 {
        if self.max == 0 { 0 } else { ((u32::from(self.value) * 100 + u32::from(self.max) / 2) / u32::from(self.max)).min(100) as u8 }
    }
}

/// Reading only: the monitor's identity and its brightness.
pub fn probe(bus: &mut dyn I2cBus) -> Result<(Edid, Brightness), DdcError> {
    bus.write(EDID_ADDR, &[0]).map_err(DdcError::Bus)?;
    let edid = parse_edid(&bus.read(EDID_ADDR, 128).map_err(DdcError::Bus)?)?;
    let v = get_vcp(bus, VCP_BRIGHTNESS)?;
    Ok((edid, Brightness { value: v.value, max: v.max }))
}

/// The one monitor the person chose (by [`Edid::id`]); nothing else is ever written to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chosen(pub Option<String>);

impl Chosen {
    /// Set the brightness of the monitor on `bus` to `percent` (1 to 100: never fully dark, so a screen cannot be lost), read back.
    pub fn set_brightness(&self, bus: &mut dyn I2cBus, percent: u8) -> Result<Brightness, DdcError> {
        let (edid, now) = probe(bus)?;
        if self.0.as_deref() != Some(edid.id().as_str()) {
            return Err(DdcError::NotSelected);
        }
        let p = percent.clamp(1, 100) as u32;
        let want = ((p * u32::from(now.max) + 50) / 100).max(1).min(u32::from(now.max)) as u16;
        bus.write(DDC_ADDR, &set_vcp_request(VCP_BRIGHTNESS, want)).map_err(DdcError::Bus)?;
        bus.pause(50);
        let back = get_vcp(bus, VCP_BRIGHTNESS)?;
        if back.value != want {
            return Err(DdcError::NotApplied { wanted: want, got: back.value });
        }
        Ok(Brightness { value: back.value, max: back.max })
    }
}

#[cfg(test)]
#[path = "ddcci_tests.rs"]
pub(crate) mod tests;
