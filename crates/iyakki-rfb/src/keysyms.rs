//! X11 keysyms the RFB key event carries (the ones IYAKKI's key mapping uses).

pub const BACKSPACE: u32 = 0xff08;
pub const TAB: u32 = 0xff09;
pub const RETURN: u32 = 0xff0d;
pub const ESCAPE: u32 = 0xff1b;
pub const HOME: u32 = 0xff50;
pub const LEFT: u32 = 0xff51;
pub const UP: u32 = 0xff52;
pub const RIGHT: u32 = 0xff53;
pub const DOWN: u32 = 0xff54;
pub const PAGE_UP: u32 = 0xff55;
pub const PAGE_DOWN: u32 = 0xff56;
pub const END: u32 = 0xff57;
pub const INSERT: u32 = 0xff63;
pub const DELETE: u32 = 0xffff;
pub const SHIFT_L: u32 = 0xffe1;
pub const CONTROL_L: u32 = 0xffe3;
pub const ALT_L: u32 = 0xffe9;
pub const ISO_LEFT_TAB: u32 = 0xfe20;
pub const F1: u32 = 0xffbe;

/// F1 to F12 (anything else gives F1 or F12).
pub fn function_key(n: u8) -> u32 {
    F1 + u32::from(n.clamp(1, 12)) - 1
}

/// The keysym of a character: Latin-1 is its own code, other Unicode is `0x0100_0000 + code`.
pub fn from_char(c: char) -> u32 {
    let u = c as u32;
    if (0x20..=0x7e).contains(&u) || (0xa0..=0xff).contains(&u) {
        u
    } else {
        0x0100_0000 + u
    }
}
