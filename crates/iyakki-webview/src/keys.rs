//! Key codes are X11 keysyms (`X11/keysymdef.h`), the numbers WPE (`wpe_input_keyboard_event.key_code`) and GTK (`GDK_KEY_*`)
//! both use. Printable characters use their Unicode value when it is in Latin-1 (0x20..=0x7e, 0xa0..=0xff) and
//! `0x01000000 + code point` otherwise.

pub const BACKSPACE: u32 = 0xff08;
pub const TAB: u32 = 0xff09;
pub const LEFT_TAB: u32 = 0xfe20; // ISO_Left_Tab (Shift+Tab)
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
pub const SPACE: u32 = 0x20;
pub const F1: u32 = 0xffbe;
pub const SHIFT_L: u32 = 0xffe1;
pub const CONTROL_L: u32 = 0xffe3;
pub const ALT_L: u32 = 0xffe9;

/// `F(n)` for n in 1..=12.
pub fn function_key(n: u8) -> u32 {
    F1 + (n.clamp(1, 12) as u32 - 1)
}

/// The keysym of a character.
pub fn from_char(c: char) -> u32 {
    let u = c as u32;
    match u {
        0x20..=0x7e | 0xa0..=0xff => u,
        0x0a | 0x0d => RETURN,
        0x08 => BACKSPACE,
        0x09 => TAB,
        0x1b => ESCAPE,
        0x7f => DELETE,
        _ => 0x0100_0000 + u,
    }
}

/// The character a keysym stands for, when it is a printable one.
pub fn to_char(sym: u32) -> Option<char> {
    match sym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(sym),
        0x0100_0100..=0x0110_ffff => char::from_u32(sym - 0x0100_0000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_map_to_x11_keysyms() {
        assert_eq!(from_char('a'), 0x61);
        assert_eq!(from_char('A'), 0x41);
        assert_eq!(from_char(' '), SPACE);
        assert_eq!(from_char('é'), 0xe9);
        assert_eq!(from_char('€'), 0x0100_20ac);
        assert_eq!(from_char('\u{1f600}'), 0x0101_f600);
        assert_eq!(to_char(from_char('€')), Some('€'));
        assert_eq!(to_char(0x61), Some('a'));
        assert_eq!(to_char(RETURN), None);
        assert_eq!(function_key(1), 0xffbe);
        assert_eq!(function_key(12), 0xffc9);
    }
}
