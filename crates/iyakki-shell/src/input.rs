//! Linux evdev input: keyboards and mice read straight from `/dev/input/event*`.
//! The pure parts (parsing, key translation, pointer state) are unit tested without devices.

use std::os::fd::{AsRawFd, RawFd};

use crate::ui::{Event, Key, Nav};

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_WHEEL: u16 = 8;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const SYN_REPORT: u16 = 0;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_FINGER: u16 = 0x145;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    pub typ: u16,
    pub code: u16,
    pub value: i32,
}

/// Parse a buffer of `struct input_event` records (layout of the running platform).
pub fn parse_events(buf: &[u8]) -> Vec<Raw> {
    let sz = std::mem::size_of::<libc::input_event>();
    buf.chunks_exact(sz)
        .map(|c| {
            // SAFETY: `c` is exactly size_of::<input_event>() bytes; input_event is plain old data.
            let ev: libc::input_event = unsafe { std::ptr::read_unaligned(c.as_ptr().cast()) };
            Raw { typ: ev.type_, code: ev.code, value: ev.value }
        })
        .collect()
}

/// US-layout translation of an evdev key code.
pub fn translate(code: u16, shift: bool, caps: bool) -> Option<Key> {
    const ROW_NUM: &str = "1234567890-=";
    const ROW_NUM_S: &str = "!@#$%^&*()_+";
    const ROW_Q: &str = "qwertyuiop[]";
    const ROW_Q_S: &str = "QWERTYUIOP{}";
    const ROW_A: &str = "asdfghjkl;'";
    const ROW_A_S: &str = "ASDFGHJKL:\"";
    const ROW_Z: &str = "zxcvbnm,./";
    const ROW_Z_S: &str = "ZXCVBNM<>?";
    let pick = |row: &str, row_s: &str, i: usize, letters: bool| -> Option<Key> {
        let (a, b) = (row.chars().nth(i)?, row_s.chars().nth(i)?);
        let upper = if letters { shift ^ caps } else { shift };
        Some(Key::Char(if upper { b } else { a }))
    };
    Some(match code {
        2..=13 => return pick(ROW_NUM, ROW_NUM_S, (code - 2) as usize, false),
        16..=27 => return pick(ROW_Q, ROW_Q_S, (code - 16) as usize, true),
        30..=40 => return pick(ROW_A, ROW_A_S, (code - 30) as usize, true),
        44..=53 => return pick(ROW_Z, ROW_Z_S, (code - 44) as usize, true),
        41 => Key::Char(if shift { '~' } else { '`' }),
        43 => Key::Char(if shift { '|' } else { '\\' }),
        57 => Key::Char(' '),
        28 | 96 => Key::Enter,
        14 => Key::Backspace,
        15 => if shift { Key::BackTab } else { Key::Tab },
        1 => Key::Esc,
        111 => Key::Delete,
        103 => Key::Up,
        108 => Key::Down,
        105 => Key::Left,
        106 => Key::Right,
        102 => Key::Home,
        107 => Key::End,
        104 => Key::PageUp,
        109 => Key::PageDown,
        59..=68 => Key::F((code - 58) as u8),
        87 => Key::F(11),
        88 | 99 => Key::F(12),
        _ => return None,
    })
}

/// Turns raw events from all devices into UI events, tracking modifiers and the pointer.
pub struct InputState {
    shift: bool,
    ctrl: bool,
    caps: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Per-device absolute ranges are applied by the caller via `abs_x`/`abs_y` scaling.
    abs: (i32, i32),
    /// Multi-touch slot the kernel is reporting for; only the first finger (slot 0) drives the pointer.
    slot: i32,
    /// A finger touched down in this frame: the tap is delivered at the end of the frame (`SYN_REPORT`),
    /// when the position of the touch is known (the button event can arrive before the coordinates).
    pending_touch: bool,
}

impl InputState {
    pub fn new(w: u32, h: u32) -> InputState {
        InputState { shift: false, ctrl: false, caps: false, x: w as i32 / 2, y: h as i32 / 2, w: w as i32, h: h as i32, abs: (0, 0), slot: 0, pending_touch: false }
    }

    fn clamp(&mut self) {
        self.x = self.x.clamp(0, self.w - 1);
        self.y = self.y.clamp(0, self.h - 1);
    }

    /// `abs_max` is the device's (max_x, max_y) when it is an absolute pointer (tablet, QEMU usb-tablet).
    pub fn feed(&mut self, r: Raw, abs_max: Option<(i32, i32)>) -> Vec<Event> {
        let mut out = Vec::new();
        match r.typ {
            EV_KEY => match r.code {
                42 | 54 => {
                    self.shift = r.value != 0;
                    // a Shift+click extends a text selection: the shell reads this while it handles the press
                    crate::ui::set_shift_held(self.shift);
                }
                29 | 97 => self.ctrl = r.value != 0,
                BTN_RIGHT if r.value == 1 => out.push(Event::MouseRight(self.x, self.y)),
                58 if r.value == 1 => self.caps = !self.caps,
                BTN_LEFT if r.value == 1 => out.push(Event::MouseDown(self.x, self.y)),
                BTN_LEFT if r.value == 0 => out.push(Event::MouseUp(self.x, self.y)),
                BTN_TOUCH if r.value == 1 => self.pending_touch = true,
                BTN_TOUCH if r.value == 0 => out.push(Event::MouseUp(self.x, self.y)),
                c if r.value == 1 || r.value == 2 => {
                    if let Some(k) = translate(c, self.shift, self.caps) {
                        // arrows/Home/End/Page with Shift or Ctrl carry their modifiers (selection, word jumps)
                        let nav = match k {
                            Key::Left => Some(Nav::Left),
                            Key::Right => Some(Nav::Right),
                            Key::Up => Some(Nav::Up),
                            Key::Down => Some(Nav::Down),
                            Key::Home => Some(Nav::Home),
                            Key::End => Some(Nav::End),
                            Key::PageUp => Some(Nav::PageUp),
                            Key::PageDown => Some(Nav::PageDown),
                            _ => None,
                        };
                        if let (Some(nav), true) = (nav, self.shift || self.ctrl) {
                            out.push(Event::Key(Key::Nav { nav, shift: self.shift, ctrl: self.ctrl }));
                            return out;
                        }
                        // Ctrl+letter becomes a shortcut; Shift makes the letter uppercase (Ctrl+Shift+V)
                        out.push(Event::Key(match k {
                            Key::Char(ch) if self.ctrl && ch.is_ascii_alphabetic() => Key::Ctrl(if self.shift { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() }),
                            other => other,
                        }));
                    }
                }
                _ => {}
            },
            EV_REL => match r.code {
                REL_X => {
                    self.x += r.value;
                    self.clamp();
                    out.push(Event::MouseMove(self.x, self.y));
                }
                REL_Y => {
                    self.y += r.value;
                    self.clamp();
                    out.push(Event::MouseMove(self.x, self.y));
                }
                REL_WHEEL => out.push(Event::Scroll(r.value)),
                _ => {}
            },
            EV_SYN if r.code == SYN_REPORT => {
                if std::mem::take(&mut self.pending_touch) {
                    out.push(Event::MouseDown(self.x, self.y));
                }
            }
            EV_ABS => {
                if r.code == ABS_MT_SLOT {
                    self.slot = r.value;
                    return out;
                }
                if r.code == ABS_MT_TRACKING_ID && self.slot == 0 && r.value >= 0 {
                    self.pending_touch = true; // a new contact (some panels send no BTN_TOUCH)
                    return out;
                }
                if r.code == ABS_MT_TRACKING_ID && self.slot == 0 && r.value < 0 {
                    out.push(Event::MouseUp(self.x, self.y)); // the finger lifted
                    return out;
                }
                if let Some((mx, my)) = abs_max {
                    match r.code {
                        ABS_X => self.abs.0 = (r.value as i64 * (self.w - 1) as i64 / mx.max(1) as i64) as i32,
                        ABS_Y => self.abs.1 = (r.value as i64 * (self.h - 1) as i64 / my.max(1) as i64) as i32,
                        ABS_MT_POSITION_X if self.slot == 0 => self.abs.0 = (r.value as i64 * (self.w - 1) as i64 / mx.max(1) as i64) as i32,
                        ABS_MT_POSITION_Y if self.slot == 0 => self.abs.1 = (r.value as i64 * (self.h - 1) as i64 / my.max(1) as i64) as i32,
                        _ => return out,
                    }
                    (self.x, self.y) = self.abs;
                    self.clamp();
                    out.push(Event::MouseMove(self.x, self.y));
                }
            }
            _ => {}
        }
        out
    }
}

// ------------------------------------------------------------------ device access

fn ioc_read(nr: u32, size: u32) -> libc::c_ulong {
    ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as libc::c_ulong
}

fn has_bit(bits: &[u8], n: u16) -> bool {
    bits.get(n as usize / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

pub struct Device {
    file: std::fs::File,
    pub path: String,
    /// `Some((max_x, max_y))` for absolute pointers.
    pub abs_max: Option<(i32, i32)>,
}

impl Device {
    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    min: i32,
    max: i32,
    fuzz: i32,
    flat: i32,
    res: i32,
}

fn open_device(path: &str) -> Option<Device> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    let fd = file.as_raw_fd();
    let mut key_bits = [0u8; 96];
    // SAFETY: EVIOCGBIT(EV_KEY) writes at most `len` bytes into our buffer.
    let r = unsafe { libc::ioctl(fd, ioc_read(0x20 + EV_KEY as u32, key_bits.len() as u32) as _, key_bits.as_mut_ptr()) };
    if r < 0 {
        return None;
    }
    let is_keyboard = has_bit(&key_bits, 30) && has_bit(&key_bits, 28); // KEY_A and KEY_ENTER
    let is_touchpad = has_bit(&key_bits, BTN_TOOL_FINGER); // needs gesture handling we don't ship
    let mut abs = AbsInfo::default();
    let mut abs_y = AbsInfo::default();
    // SAFETY: EVIOCGABS fills one input_absinfo.
    let (rx, ry) = unsafe {
        (
            libc::ioctl(fd, ioc_read(0x40 + ABS_X as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut abs as *mut AbsInfo),
            libc::ioctl(fd, ioc_read(0x40 + ABS_Y as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut abs_y as *mut AbsInfo),
        )
    };
    // many touch monitors report only the multi-touch axes
    let (mut mt_x, mut mt_y) = (AbsInfo::default(), AbsInfo::default());
    // SAFETY: as above, for the multi-touch position axes.
    let (mrx, mry) = unsafe {
        (
            libc::ioctl(fd, ioc_read(0x40 + ABS_MT_POSITION_X as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut mt_x as *mut AbsInfo),
            libc::ioctl(fd, ioc_read(0x40 + ABS_MT_POSITION_Y as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut mt_y as *mut AbsInfo),
        )
    };
    let abs_max = if rx >= 0 && ry >= 0 && abs.max > 0 && !is_touchpad {
        Some((abs.max, abs_y.max))
    } else if mrx >= 0 && mry >= 0 && mt_x.max > 0 && mt_y.max > 0 && !is_touchpad {
        Some((mt_x.max, mt_y.max))
    } else {
        None
    };
    let has_btn = has_bit(&key_bits, BTN_LEFT) || has_bit(&key_bits, BTN_TOUCH);
    let mut rel_bits = [0u8; 4];
    // SAFETY: as above, for EV_REL.
    unsafe { libc::ioctl(fd, ioc_read(0x20 + EV_REL as u32, rel_bits.len() as u32) as _, rel_bits.as_mut_ptr()) };
    let is_mouse = has_bit(&rel_bits, REL_X) && has_btn;
    if is_touchpad && !is_keyboard {
        return None;
    }
    if is_keyboard || is_mouse || abs_max.is_some() {
        Some(Device { file, path: path.into(), abs_max })
    } else {
        None
    }
}

/// Open every usable keyboard/mouse not already open.
pub fn scan_devices(open: &mut Vec<Device>) {
    let Ok(rd) = std::fs::read_dir("/dev/input") else { return };
    let mut paths: Vec<String> = rd.flatten().map(|e| e.path().display().to_string()).filter(|p| p.contains("/event")).collect();
    paths.sort();
    for p in paths {
        if open.iter().any(|d| d.path == p) {
            continue;
        }
        if let Some(d) = open_device(&p) {
            open.push(d);
        }
    }
}

/// Wait up to `timeout_ms` for input, then read everything available. Dead devices are dropped.
pub fn poll_devices(devs: &mut Vec<Device>, timeout_ms: i32) -> Vec<(Raw, Option<(i32, i32)>)> {
    let mut out = Vec::new();
    if devs.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms.max(0) as u64));
        return out;
    }
    let mut fds: Vec<libc::pollfd> = devs.iter().map(|d| libc::pollfd { fd: d.fd(), events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: `fds` is a valid array of pollfd for the duration of the call.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    let mut dead = Vec::new();
    for (i, pf) in fds.iter().enumerate() {
        if pf.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            dead.push(i);
            continue;
        }
        if pf.revents & libc::POLLIN != 0 {
            use std::io::Read;
            let mut buf = [0u8; 24 * 64];
            while let Ok(n) = (&devs[i].file).read(&mut buf) {
                if n == 0 {
                    break;
                }
                out.extend(parse_events(&buf[..n]).into_iter().map(|r| (r, devs[i].abs_max)));
            }
        }
    }
    for i in dead.into_iter().rev() {
        devs.remove(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_translation_with_shift_and_caps() {
        assert_eq!(translate(30, false, false), Some(Key::Char('a')));
        assert_eq!(translate(30, true, false), Some(Key::Char('A')));
        assert_eq!(translate(30, false, true), Some(Key::Char('A')));
        assert_eq!(translate(30, true, true), Some(Key::Char('a')));
        assert_eq!(translate(2, true, true), Some(Key::Char('!')), "caps lock does not shift digits");
        assert_eq!(translate(2, false, false), Some(Key::Char('1')));
        assert_eq!(translate(11, false, false), Some(Key::Char('0')));
        assert_eq!(translate(53, false, false), Some(Key::Char('/')));
        assert_eq!(translate(53, true, false), Some(Key::Char('?')));
        assert_eq!(translate(57, false, false), Some(Key::Char(' ')));
        assert_eq!(translate(28, false, false), Some(Key::Enter));
        assert_eq!(translate(15, true, false), Some(Key::BackTab));
        assert_eq!(translate(1, false, false), Some(Key::Esc));
        assert_eq!(translate(999, false, false), None);
        // every printable row code maps to a char
        for c in (2..=13).chain(16..=27).chain(30..=40).chain(44..=53).chain([41, 43, 57]) {
            assert!(matches!(translate(c, false, false), Some(Key::Char(_))), "code {c}");
        }
    }

    fn press(st: &mut InputState, code: u16) -> Vec<Event> {
        let mut v = st.feed(Raw { typ: EV_KEY, code, value: 1 }, None);
        st.feed(Raw { typ: EV_KEY, code, value: 0 }, None);
        v.drain(..).collect()
    }

    #[test]
    fn modifiers_are_tracked_across_events() {
        let mut st = InputState::new(800, 600);
        st.feed(Raw { typ: EV_KEY, code: 42, value: 1 }, None); // shift down
        assert_eq!(press(&mut st, 35), vec![Event::Key(Key::Char('H'))]);
        st.feed(Raw { typ: EV_KEY, code: 42, value: 0 }, None); // shift up
        assert_eq!(press(&mut st, 35), vec![Event::Key(Key::Char('h'))]);
        press(&mut st, 58); // caps on
        assert_eq!(press(&mut st, 35), vec![Event::Key(Key::Char('H'))]);
        // key release produces nothing; autorepeat (value 2) does
        assert!(st.feed(Raw { typ: EV_KEY, code: 35, value: 0 }, None).is_empty());
        assert_eq!(st.feed(Raw { typ: EV_KEY, code: 35, value: 2 }, None), vec![Event::Key(Key::Char('H'))]);
    }

    #[test]
    fn ctrl_shortcuts_and_right_click() {
        let mut st = InputState::new(800, 600);
        let down = |st: &mut InputState, code: u16| st.feed(Raw { typ: EV_KEY, code, value: 1 }, None);
        let up = |st: &mut InputState, code: u16| st.feed(Raw { typ: EV_KEY, code, value: 0 }, None);
        down(&mut st, 29); // left ctrl
        assert_eq!(down(&mut st, 47), vec![Event::Key(Key::Ctrl('v'))]); // KEY_V
        up(&mut st, 47);
        down(&mut st, 42); // shift too
        assert_eq!(down(&mut st, 47), vec![Event::Key(Key::Ctrl('V'))], "Ctrl+Shift+V");
        up(&mut st, 47);
        up(&mut st, 42);
        assert_eq!(down(&mut st, 30), vec![Event::Key(Key::Ctrl('a'))]);
        assert_eq!(down(&mut st, 28), vec![Event::Key(Key::Enter)], "non-letters are unaffected by Ctrl");
        up(&mut st, 29);
        assert_eq!(down(&mut st, 47), vec![Event::Key(Key::Char('v'))], "plain v after Ctrl is released");
        assert_eq!(st.feed(Raw { typ: EV_KEY, code: BTN_RIGHT, value: 1 }, None), vec![Event::MouseRight(400, 300)]);
        // right Ctrl works as well
        down(&mut st, 97);
        assert_eq!(down(&mut st, 46), vec![Event::Key(Key::Ctrl('c'))]);
    }

    #[test]
    fn relative_pointer_clamps_and_clicks() {
        let mut st = InputState::new(100, 50);
        assert_eq!((st.x, st.y), (50, 25));
        st.feed(Raw { typ: EV_REL, code: 0, value: 500 }, None);
        st.feed(Raw { typ: EV_REL, code: 1, value: -500 }, None);
        assert_eq!((st.x, st.y), (99, 0));
        let ev = st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 1 }, None);
        assert_eq!(ev, vec![Event::MouseDown(99, 0)]);
        assert_eq!(st.feed(Raw { typ: EV_REL, code: 8, value: 1 }, None), vec![Event::Scroll(1)]);
    }

    #[test]
    fn absolute_pointer_scales_to_screen() {
        let mut st = InputState::new(1280, 720);
        let max = Some((32767, 32767));
        st.feed(Raw { typ: EV_ABS, code: 0, value: 32767 }, max);
        st.feed(Raw { typ: EV_ABS, code: 1, value: 16383 }, max);
        assert_eq!((st.x, st.y), (1279, 359));
        // devices that are not absolute pointers are ignored for ABS events
        assert!(st.feed(Raw { typ: EV_ABS, code: 0, value: 5 }, None).is_empty());
    }

    fn frame(st: &mut InputState, raws: &[(u16, u16, i32)], max: Option<(i32, i32)>) -> Vec<Event> {
        raws.iter().flat_map(|(t, c, v)| st.feed(Raw { typ: *t, code: *c, value: *v }, max)).collect()
    }

    #[test]
    fn a_tap_lands_where_the_finger_is_even_if_the_button_event_comes_first() {
        let mut st = InputState::new(1920, 1080);
        let max = Some((4095, 4095));
        // button first, coordinates after, then the end-of-frame marker
        let ev = frame(&mut st, &[(EV_KEY, BTN_TOUCH, 1), (EV_ABS, ABS_X, 4095), (EV_ABS, ABS_Y, 0), (EV_SYN, SYN_REPORT, 0)], max);
        assert_eq!(ev.last(), Some(&Event::MouseDown(1919, 0)), "{ev:?}");
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::MouseDown(..))).count(), 1);
        // lifting and tapping elsewhere
        frame(&mut st, &[(EV_KEY, BTN_TOUCH, 0), (EV_SYN, SYN_REPORT, 0)], max);
        let ev = frame(&mut st, &[(EV_ABS, ABS_X, 0), (EV_ABS, ABS_Y, 4095), (EV_KEY, BTN_TOUCH, 1), (EV_SYN, SYN_REPORT, 0)], max);
        assert_eq!(ev.last(), Some(&Event::MouseDown(0, 1079)));
        // a mouse button still acts immediately
        assert_eq!(st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 1 }, None), vec![Event::MouseDown(0, 1079)]);
    }

    #[test]
    fn multi_touch_only_panels_work_and_only_the_first_finger_drives_the_pointer() {
        let mut st = InputState::new(3840, 2160);
        let max = Some((32767, 32767));
        let ev = frame(&mut st, &[(EV_ABS, ABS_MT_SLOT, 0), (EV_ABS, ABS_MT_TRACKING_ID, 7), (EV_ABS, ABS_MT_POSITION_X, 16383), (EV_ABS, ABS_MT_POSITION_Y, 8191), (EV_SYN, SYN_REPORT, 0)], max);
        assert_eq!(ev.last(), Some(&Event::MouseDown(1919, 539)), "a new contact is a tap even without BTN_TOUCH: {ev:?}");
        // a second finger in slot 1 does not move the pointer or tap
        let ev = frame(&mut st, &[(EV_ABS, ABS_MT_SLOT, 1), (EV_ABS, ABS_MT_TRACKING_ID, 8), (EV_ABS, ABS_MT_POSITION_X, 0), (EV_ABS, ABS_MT_POSITION_Y, 0), (EV_SYN, SYN_REPORT, 0)], max);
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!((st.x, st.y), (1919, 539));
        // back to slot 0: dragging moves the pointer; releasing (tracking id -1) is not a tap but ends the press
        let ev = frame(&mut st, &[(EV_ABS, ABS_MT_SLOT, 0), (EV_ABS, ABS_MT_POSITION_X, 32767), (EV_ABS, ABS_MT_TRACKING_ID, -1), (EV_SYN, SYN_REPORT, 0)], max);
        assert_eq!(ev, vec![Event::MouseMove(3839, 539), Event::MouseUp(3839, 539)]);
        assert_eq!(st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 0 }, None), vec![Event::MouseUp(3839, 539)], "releasing the button ends the press");
        // a panel that reports both axis sets taps once
        let ev = frame(&mut st, &[(EV_KEY, BTN_TOUCH, 1), (EV_ABS, ABS_MT_TRACKING_ID, 9), (EV_ABS, ABS_X, 0), (EV_ABS, ABS_MT_POSITION_X, 0), (EV_SYN, SYN_REPORT, 0)], max);
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::MouseDown(..))).count(), 1, "{ev:?}");
    }

    #[test]
    fn parses_raw_input_event_records() {
        let sz = std::mem::size_of::<libc::input_event>();
        let mut buf = vec![0u8; sz * 2];
        for (i, (t, c, v)) in [(EV_KEY, 30u16, 1i32), (EV_REL, 0, -7)].iter().enumerate() {
            let ev = libc::input_event { time: libc::timeval { tv_sec: 1, tv_usec: 2 }, type_: *t, code: *c, value: *v };
            // SAFETY: writing a POD struct into a byte buffer of sufficient size.
            unsafe { std::ptr::write_unaligned(buf[i * sz..].as_mut_ptr().cast(), ev) };
        }
        assert_eq!(parse_events(&buf), vec![Raw { typ: EV_KEY, code: 30, value: 1 }, Raw { typ: EV_REL, code: 0, value: -7 }]);
        assert!(parse_events(&buf[..sz - 1]).is_empty(), "partial records are ignored");
    }

    #[test]
    fn missing_device_directory_is_not_fatal() {
        let mut v = Vec::new();
        scan_devices(&mut v); // works (possibly empty) whether or not /dev/input exists
        let _ = poll_devices(&mut v, 1);
    }
}
