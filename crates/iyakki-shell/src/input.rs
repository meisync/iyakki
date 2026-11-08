//! Linux evdev input: keyboards and mice read straight from `/dev/input/event*`.
//! The pure parts (parsing, key translation, pointer state) are unit tested without devices.

use crate::safeclamp::SafeClamp;
use std::os::fd::{AsRawFd, RawFd};

use crate::ui::{Event, Key, Nav};

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_WHEEL: u16 = 8;
/// A wheel's fractions of a click (120 to the click); a wheel that has it sends both, and only this one is used.
const REL_WHEEL_HI_RES: u16 = 11;
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

#[cfg(target_os = "linux")]
use libc::input_event as InputEvent;

/// macOS has no evdev and `libc` has no `input_event` there; the record is the kernel's 64-bit layout so the parser (and its
/// tests) build everywhere.
#[cfg(not(target_os = "linux"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

/// Parse a buffer of `struct input_event` records (layout of the running platform).
pub fn parse_events(buf: &[u8]) -> Vec<Raw> {
    let sz = std::mem::size_of::<InputEvent>();
    buf.chunks_exact(sz)
        .map(|c| {
            // SAFETY: `c` is exactly size_of::<input_event>() bytes; input_event is plain old data.
            let ev: InputEvent = unsafe { std::ptr::read_unaligned(c.as_ptr().cast()) };
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
    alt: bool,
    /// The Windows / Command key (Meta), tracked for the Mac input profile (`inputprofile.rs`).
    meta: bool,
    /// A Control+click made a right click (Mac profile): the release that follows is not a click.
    ctrl_click: bool,
    /// The last scroll came from a touchpad (true) or a wheel, for the live row of Settings.
    scroll_from_pad: bool,
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
    /// Touchpad gesture state (one pad at a time; a second pad shares it).
    pad: crate::touchpad::Touchpad,
    /// Touch screens, each with its own slots and the display its touch started on (`touchscreen.rs`).
    touches: crate::touchscreen::Touches,
    started: std::time::Instant,
    /// Wheel and trackpad movement becomes smooth pixel steps here (`scrolleng.rs`); [`InputState::scroll_tick`] hands them out.
    scroll: crate::scrolleng::ScrollEngine,
    /// The wheel movement of the frame being read (legacy clicks, and the hi-res units a newer wheel sends with them).
    wheel_legacy: i32,
    wheel_hires: i32,
    /// A clock for tests (milliseconds); the real one when `None`.
    fake_ms: Option<u64>,
    /// The earliest time of the next scroll step.
    scroll_due: Option<u64>,
}

impl InputState {
    pub fn new(w: u32, h: u32) -> InputState {
        InputState { shift: false, ctrl: false, alt: false, meta: false, ctrl_click: false, scroll_from_pad: false, caps: false, x: w as i32 / 2, y: h as i32 / 2, w: w as i32, h: h as i32, abs: (0, 0), slot: 0, pending_touch: false, pad: crate::touchpad::Touchpad::new(), touches: Default::default(), started: std::time::Instant::now(), scroll: crate::scrolleng::ScrollEngine::new(), wheel_legacy: 0, wheel_hires: 0, fake_ms: None, scroll_due: None }
    }

    fn now_ms(&self) -> u64 {
        self.fake_ms.unwrap_or_else(|| self.started.elapsed().as_millis() as u64)
    }

    /// The scroll steps due now, as [`Event::ScrollPx`] (none while nothing scrolls). Call it from the input loop at least every
    /// few milliseconds; it makes a step at most every [`crate::scrolleng::FRAME_MS`], so a scroll in progress asks for about 60
    /// pictures a second and the shell is back at its low rate as soon as it settles.
    pub fn scroll_tick(&mut self) -> Vec<Event> {
        if crate::scrolleng::take_bound() {
            self.scroll.bound();
        }
        if !self.scroll.wants_frames() {
            return Vec::new();
        }
        let now = self.now_ms();
        if self.scroll_due.is_some_and(|t| now < t) {
            return Vec::new();
        }
        self.scroll_due = Some(now + crate::scrolleng::FRAME_MS);
        match self.scroll.tick(now, crate::scrolleng::raw()) {
            0 => Vec::new(),
            px => {
                crate::inputprofile::note_scroll(px, self.scroll_from_pad);
                vec![Event::ScrollPx(px)]
            }
        }
    }

    /// Use a clock set by hand (tests of other modules; milliseconds).
    #[cfg(test)]
    pub(crate) fn fake_clock(&mut self, ms: u64) {
        self.fake_ms = Some(ms);
    }

    /// True while a scroll is going on (the caller keeps its loop quick).
    pub fn scrolling(&self) -> bool {
        self.scroll.wants_frames()
    }

    /// An event from a touchpad (see `touchpad.rs`): pointer movement, taps, scrolling and the physical click. The clock is the
    /// real one; tests use [`InputState::feed_pad_at`].
    pub fn feed_pad(&mut self, r: Raw, range: &crate::touchpad::PadRange) -> Vec<Event> {
        let now = self.started.elapsed().as_millis() as u64;
        self.feed_pad_at(r, range, now)
    }

    pub fn feed_pad_at(&mut self, r: Raw, range: &crate::touchpad::PadRange, now_ms: u64) -> Vec<Event> {
        use crate::touchpad::PadOut;
        let cfg = pad_config();
        let scfg = crate::scrolleng::config();
        // a finger landing stops whatever momentum is going
        if (r.typ == EV_ABS && r.code == ABS_MT_TRACKING_ID && r.value >= 0) || (r.typ == EV_KEY && r.code == BTN_TOUCH && r.value == 1) {
            self.scroll.touch();
        }
        let mut out = Vec::new();
        for o in self.pad.feed(r, now_ms, range, &cfg, self.w) {
            match o {
                PadOut::Move(dx, dy) => {
                    self.x += dx;
                    self.y += dy;
                    self.clamp();
                    out.push(Event::MouseMove(self.x, self.y));
                }
                // Control+click is a right click in the Mac profile
                PadOut::LeftDown if self.ctrl && crate::inputprofile::resolved() == iyakki_agent::inputprofile::Resolved::Mac => {
                    self.ctrl_click = true;
                    out.push(Event::MouseRight(self.x, self.y));
                }
                PadOut::LeftUp if std::mem::take(&mut self.ctrl_click) => {}
                PadOut::LeftDown => out.push(Event::MouseDown(self.x, self.y)),
                PadOut::LeftUp => out.push(Event::MouseUp(self.x, self.y)),
                PadOut::RightClick => out.push(Event::MouseRight(self.x, self.y)),
                PadOut::Scroll(d) => {
                    self.scroll_from_pad = true;
                    self.scroll.pad(d, now_ms, &scfg)
                }
                PadOut::ScrollEnd => self.scroll.pad_end(now_ms, &scfg),
            }
        }
        out
    }

    fn clamp(&mut self) {
        self.x = self.x.clamp_safe(0, self.w - 1);
        self.y = self.y.clamp_safe(0, self.h - 1);
    }

    /// An event of a touch screen, mapped onto the display rectangle `target` of the desktop (`None`: no display owns it).
    /// Positions come out as desktop positions; the pointer follows the finger.
    pub fn feed_touch(&mut self, r: Raw, dev: &crate::touchmap::TouchDev, target: Option<&crate::touchmap::OutRect>) -> crate::touchscreen::TouchOut {
        let out = self.touches.feed(r, dev, target);
        for e in &out.events {
            if let Event::MouseMove(x, y) | Event::MouseDown(x, y) | Event::MouseUp(x, y) = e {
                (self.x, self.y) = (*x, *y);
            }
        }
        out
    }

    /// [`InputState::feed_touch`] for a single picture: the device covers the whole screen.
    pub fn feed_touch_full(&mut self, r: Raw, dev: &crate::touchmap::TouchDev) -> Vec<Event> {
        let whole = crate::touchmap::OutRect { name: String::new(), x: 0, y: 0, w: self.w, h: self.h, internal: false, rot: crate::touchmap::Rot::R0 };
        self.feed_touch(r, dev, Some(&whole)).events
    }

    /// A touch screen was unplugged: a finger still on it is lifted.
    pub fn touch_gone(&mut self, key: &str) -> Vec<Event> {
        self.touches.gone(key)
    }

    /// The touch screens held now are `now`, the ones held a moment ago `before`: whatever state a screen that left, or came back under
    /// the same key, left behind is dropped (a contact that never lifted would else keep every later touch from being taken; a USB-C
    /// monitor resets its USB hub when its display link renegotiates, and its touch panel comes back as a new device). `added` are the
    /// keys of the devices opened since.
    pub fn touch_devices_changed(&mut self, before: &[String], now: &[String], added: &[String]) -> Vec<Event> {
        let mut out = Vec::new();
        for k in before.iter().filter(|k| !now.contains(k)).chain(added.iter()) {
            out.extend(self.touch_gone(k));
        }
        out
    }

    /// `abs_max` is the device's (max_x, max_y) when it is an absolute pointer (tablet, QEMU usb-tablet).
    /// The input profile's view of a key press. True when it took the key (the events, if any, are in `out`).
    fn profile_key(&mut self, code: u16, first: bool, out: &mut Vec<Event>) -> bool {
        use iyakki_agent::inputprofile as ip;
        let mac = crate::inputprofile::resolved() == ip::Resolved::Mac;
        let swap = crate::inputprofile::swap_in_effect();
        // in the Mac profile the keys are named by their print: the driver's swap is undone here, so it never swaps twice
        let (command, option) = if mac {
            let (a, m) = (self.alt, self.meta);
            if swap { (a, m) } else { (m, a) }
        } else {
            (false, false)
        };
        let held = ip::Held { command, option, control: self.ctrl, shift: self.shift };
        match crate::inputprofile::on_press(code, held, first) {
            Some(evs) => {
                out.extend(evs);
                true
            }
            None => false,
        }
    }

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
                125 | 126 => self.meta = r.value != 0,
                56 | 100 => {
                    self.alt = r.value != 0;
                    if r.value == 1 {
                        crate::fnkeys::note_seen(r.code); // Settings -> Keyboard still finds the code of this key
                    }
                }
                BTN_RIGHT if r.value == 1 => out.push(Event::MouseRight(self.x, self.y)),
                58 if r.value == 1 => self.caps = !self.caps,
                // an absolute pointer (a touch monitor's mouse-like interface, a tablet) sends the button before the new position in
                // the same frame: the click is made at the end of the frame, where the finger is, not where the last one was
                BTN_LEFT if r.value == 1 && abs_max.is_some() => self.pending_touch = true,
                // Mac profile: Control+click is a right click (the release after it is not a click)
                BTN_LEFT if r.value == 1 && self.ctrl && crate::inputprofile::resolved() == iyakki_agent::inputprofile::Resolved::Mac => {
                    self.ctrl_click = true;
                    out.push(Event::MouseRight(self.x, self.y));
                }
                BTN_LEFT if r.value == 0 && std::mem::take(&mut self.ctrl_click) => {}
                BTN_LEFT if r.value == 1 => out.push(Event::MouseDown(self.x, self.y)),
                BTN_LEFT if r.value == 0 => out.push(Event::MouseUp(self.x, self.y)),
                BTN_TOUCH if r.value == 1 => self.pending_touch = true,
                BTN_TOUCH if r.value == 0 => out.push(Event::MouseUp(self.x, self.y)),
                // media, volume, brightness and other Fn keys: the shell acts on them before any app sees a key
                c if r.value != 2 && crate::fnkeys::fn_key(c).is_some() => {
                    if let Some(fk) = crate::fnkeys::fn_key(c) {
                        if r.value == 1 {
                            crate::fnkeys::note_seen(c);
                        }
                        out.push(Event::Fn(fk, r.value != 0));
                    }
                }
                // Ctrl+Alt chords for panes and displays (arrows, 0-9, Tab, M): the shell takes them, no app sees a key
                c if self.ctrl && self.alt && (r.value == 1 || r.value == 2) && crate::panes::chord_from_evdev(c, self.shift).is_some() => {
                    if let Some(k) = crate::panes::chord_from_evdev(c, self.shift) {
                        out.push(Event::Layout(k));
                    }
                }
                // the input profile: Command shortcuts and Option word jumps of the Mac profile, and the live "last key" row
                c if (r.value == 1 || r.value == 2) && self.profile_key(c, r.value == 1, &mut out) => {}
                c if r.value == 1 || r.value == 2 => {
                    if translate(c, self.shift, self.caps).is_none() && r.value == 1 && c < 0x100 {
                        crate::fnkeys::note_seen(c); // for Settings -> Keyboard: finding the code of a key
                    }
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
                // read at the end of the frame: a wheel with fractions sends the click and its 120 units together
                REL_WHEEL => self.wheel_legacy += r.value,
                REL_WHEEL_HI_RES => self.wheel_hires += r.value,
                _ => {}
            },
            EV_SYN if r.code == SYN_REPORT => {
                let (legacy, hires) = (std::mem::take(&mut self.wheel_legacy), std::mem::take(&mut self.wheel_hires));
                match (hires, legacy) {
                    (0, 0) => {}
                    (0, clicks) => {
                        self.scroll_from_pad = false;
                        self.scroll.wheel(clicks * 120, &crate::scrolleng::config())
                    }
                    (units, _) => {
                        self.scroll_from_pad = false;
                        self.scroll.wheel(units, &crate::scrolleng::config())
                    }
                }
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

// ------------------------------------------------------------------ touchpad settings (Settings -> Keyboard)

static PAD_TAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Tap to click for every touchpad (takes effect with the next event). Scrolling choices: [`crate::scrolleng::request_config`].
pub fn request_touchpad(tap_to_click: bool) {
    use std::sync::atomic::Ordering::SeqCst;
    PAD_TAP.store(tap_to_click, SeqCst);
}

pub fn pad_config() -> crate::touchpad::PadConfig {
    use std::sync::atomic::Ordering::SeqCst;
    crate::touchpad::PadConfig { tap_to_click: PAD_TAP.load(SeqCst) }
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
    /// A typing keyboard (key repeat settings apply to these).
    pub is_keyboard: bool,
    /// `Some` for a touchpad (multi-touch, finger tools, not a touch screen): its events go to `InputState::feed_pad`.
    pub pad: Option<crate::touchpad::PadRange>,
    /// `Some` for a touch screen or digitizer: its positions are mapped onto the display it belongs to (`touchmap.rs`).
    pub touch: Option<std::sync::Arc<crate::touchmap::TouchDev>>,
    /// Fingers use it: a touch screen, or the mouse-like interface many touch monitors add (named "...TouchScreen", absolute, BTN_LEFT):
    /// a press on it opens the on-screen keyboard (`osk.rs`) for a text field, however the positions are mapped.
    pub finger: bool,
    /// The device has delivered at least one event since it was opened. A touch screen that never speaks leaves the word to the
    /// monitor's mouse-like interface (`mouse_like_muted`).
    pub spoke: bool,
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

/// Has multi-touch position axes.
fn is_multitouch(fd: RawFd) -> bool {
    let mut x = AbsInfo::default();
    // SAFETY: EVIOCGABS fills one input_absinfo.
    unsafe { libc::ioctl(fd, ioc_read(0x40 + ABS_MT_POSITION_X as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut x as *mut AbsInfo) >= 0 }
}

/// EVIOCGPROP: INPUT_PROP_DIRECT (bit 1) marks a touch screen, whose finger position is the pointer.
fn is_direct(fd: RawFd) -> bool {
    let mut props = [0u8; 4];
    // SAFETY: EVIOCGPROP writes at most `len` bytes into our buffer.
    let r = unsafe { libc::ioctl(fd, ioc_read(0x09, props.len() as u32) as _, props.as_mut_ptr()) };
    r >= 0 && has_bit(&props, 1)
}

/// EVIOCGNAME: the device's name as the kernel shows it.
fn device_name(fd: RawFd) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: EVIOCGNAME writes at most `len` bytes (a NUL-terminated name) into our buffer.
    let r = unsafe { libc::ioctl(fd, ioc_read(0x06, buf.len() as u32) as _, buf.as_mut_ptr()) };
    if r <= 0 {
        return String::new();
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// EVIOCGID: (bus type, vendor, product); bus 3 is USB.
fn device_id(fd: RawFd) -> (u16, u16, u16) {
    let mut id = [0u16; 4];
    // SAFETY: EVIOCGID fills one input_id (four u16).
    let r = unsafe { libc::ioctl(fd, ioc_read(0x02, 8) as _, id.as_mut_ptr()) };
    if r < 0 {
        (0, 0, 0)
    } else {
        (id[0], id[1], id[2])
    }
}

/// EVIOCGPHYS: the port path the device hangs on ("usb-0000:00:14.0-3.3/input0"), empty when it has none.
fn device_phys(fd: RawFd) -> String {
    let mut buf = [0u8; 128];
    // SAFETY: EVIOCGPHYS writes at most `len` bytes (a NUL-terminated string) into our buffer.
    let r = unsafe { libc::ioctl(fd, ioc_read(0x07, buf.len() as u32) as _, buf.as_mut_ptr()) };
    if r <= 0 {
        return String::new();
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// A touch panel of the Touch Bar: its finger positions belong to the bar (`touchbar_host.rs`), not to the pointer of the main screen.
pub struct BarNode {
    pub path: String,
    pub file: std::fs::File,
    /// The position axis along the long side (ABS_MT_POSITION_X or _Y) and its range.
    pub axis: u16,
    pub min: i32,
    pub max: i32,
}

impl BarNode {
    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

/// Open one event node when it is a multi-touch panel named like the Touch Bar. The long axis is the one with the bigger range (the
/// panel is built like the display, 2170 along the bar and 60 across).
pub fn open_touchbar_node(path: &str) -> Option<BarNode> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    let fd = file.as_raw_fd();
    if !crate::touchbar::is_touchbar_name(&device_name(fd)) {
        return None;
    }
    let (mut x, mut y) = (AbsInfo::default(), AbsInfo::default());
    // SAFETY: EVIOCGABS fills one input_absinfo.
    let (rx, ry) = unsafe {
        (
            libc::ioctl(fd, ioc_read(0x40 + ABS_MT_POSITION_X as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut x as *mut AbsInfo),
            libc::ioctl(fd, ioc_read(0x40 + ABS_MT_POSITION_Y as u32, std::mem::size_of::<AbsInfo>() as u32) as _, &mut y as *mut AbsInfo),
        )
    };
    if rx < 0 || ry < 0 || x.max <= x.min || y.max <= y.min {
        return None;
    }
    let (axis, a) = if (x.max - x.min) >= (y.max - y.min) { (ABS_MT_POSITION_X, x) } else { (ABS_MT_POSITION_Y, y) };
    Some(BarNode { path: path.into(), file, axis, min: a.min, max: a.max })
}

/// The event nodes of `/dev/input` (sorted).
pub fn event_nodes() -> Vec<String> {
    let Ok(rd) = std::fs::read_dir("/dev/input") else { return Vec::new() };
    let mut v: Vec<String> = rd.flatten().map(|e| e.path().display().to_string()).filter(|p| p.contains("/event")).collect();
    v.sort();
    v
}

/// Read what is waiting on a Touch Bar node (never blocks). `Err` when the node is gone.
pub fn read_touchbar_node(n: &BarNode) -> Result<Vec<Raw>, ()> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut buf = [0u8; 24 * 64];
    loop {
        match (&n.file).read(&mut buf) {
            Ok(0) => return Err(()),
            Ok(k) => out.extend(parse_events(&buf[..k])),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(out),
            Err(_) => return Err(()),
        }
    }
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
    // the Touch Bar's touch panel is the bar's own: never a pointer on the main screen (its keys node has no positions and stays)
    if !is_keyboard && is_multitouch(fd) && crate::touchbar::is_touchbar_name(&device_name(fd)) {
        return None;
    }
    let is_touchpad = has_bit(&key_bits, BTN_TOOL_FINGER) && !is_direct(fd); // a touch screen (INPUT_PROP_DIRECT) is not a pad
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
    // a multi-touch touchpad (Apple SPI, I2C and USB pads): gestures in touchpad.rs
    if is_touchpad && !is_keyboard && mrx >= 0 && mry >= 0 && mt_x.max > mt_x.min && mt_y.max > mt_y.min {
        let mut major = AbsInfo::default();
        // SAFETY: as above, for ABS_MT_TOUCH_MAJOR (0x30).
        let rm = unsafe { libc::ioctl(fd, ioc_read(0x40 + 0x30, std::mem::size_of::<AbsInfo>() as u32) as _, &mut major as *mut AbsInfo) };
        let pad = crate::touchpad::PadRange { x_min: mt_x.min, x_max: mt_x.max, y_min: mt_y.min, y_max: mt_y.max, major_max: if rm >= 0 { major.max } else { 0 } };
        return Some(Device { file, path: path.into(), abs_max: None, is_keyboard: false, pad: Some(pad), touch: None, finger: false, spoke: false });
    }
    let has_btn = has_bit(&key_bits, BTN_LEFT) || has_bit(&key_bits, BTN_TOUCH);
    let mut rel_bits = [0u8; 4];
    // SAFETY: as above, for EV_REL.
    unsafe { libc::ioctl(fd, ioc_read(0x20 + EV_REL as u32, rel_bits.len() as u32) as _, rel_bits.as_mut_ptr()) };
    let is_mouse = has_bit(&rel_bits, REL_X) && has_btn;
    if is_touchpad && !is_keyboard {
        return None;
    }
    // volume and media keys often sit on a node of their own ("Consumer Control", "Video Bus", "Power Button")
    let has_fn_keys = crate::fnkeys::KEYS.iter().any(|i| has_bit(&key_bits, i.code));
    if is_keyboard || is_mouse || abs_max.is_some() || has_fn_keys {
        // a touch screen or digitizer (INPUT_PROP_DIRECT or BTN_TOUCH) belongs to one display; a tablet pointer spans the desktop
        let touch = if abs_max.is_some() && !is_keyboard && (is_direct(fd) || has_bit(&key_bits, BTN_TOUCH)) {
            let mt = mrx >= 0 && mry >= 0 && mt_x.max > mt_x.min && mt_y.max > mt_y.min;
            let (x, y) = if mt { ((mt_x.min, mt_x.max), (mt_y.min, mt_y.max)) } else { ((abs.min, abs.max), (abs_y.min, abs_y.max)) };
            let (bus, vendor, product) = device_id(fd);
            let phys = device_phys(fd);
            let id = format!("{vendor:04x}:{product:04x}");
            let key = format!("{id}@{}", if phys.is_empty() { path.to_string() } else { phys });
            Some(std::sync::Arc::new(crate::touchmap::TouchDev { key, id, name: device_name(fd), usb: bus == 3, mt, x, y }))
        } else {
            None
        };
        let name = device_name(fd);
        let lower = name.to_ascii_lowercase();
        let finger = !is_keyboard && (touch.is_some() || (abs_max.is_some() && (lower.contains("touch") || lower.contains("digitizer"))));
        if finger {
            log::info!("input: {name} ({path}) takes fingers: its presses open the on-screen keyboard for a text field{}", if touch.is_some() { "" } else { " (a mouse-like touch interface)" });
        }
        Some(Device { file, path: path.into(), abs_max, is_keyboard, pad: None, touch, finger, spoke: false })
    } else {
        None
    }
}

// ------------------------------------------------------------------ key repeat (Settings -> Keyboard)

static REPEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REPEAT_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Ask for a key repeat of `delay_ms` before the first repeat and `period_ms` between repeats on every keyboard (the input
/// thread applies it, also to keyboards plugged in later). 0, 0 = leave the kernel's own.
pub fn request_repeat(delay_ms: u32, period_ms: u32) {
    use std::sync::atomic::Ordering::SeqCst;
    let v = ((delay_ms as u64) << 32) | period_ms as u64;
    if REPEAT.swap(v, SeqCst) != v {
        REPEAT_GEN.fetch_add(1, SeqCst);
    }
}

/// The requested (delay, period) in milliseconds, if any.
pub fn current_repeat() -> Option<(u32, u32)> {
    let v = REPEAT.load(std::sync::atomic::Ordering::SeqCst);
    (v != 0).then_some(((v >> 32) as u32, v as u32))
}

/// EVIOCSREP: `_IOW('E', 0x03, unsigned int[2])`.
const EVIOCSREP: libc::c_ulong = ((1u32 << 30) | (8 << 16) | ((b'E' as u32) << 8) | 3) as libc::c_ulong;

fn set_repeat(path: &str, delay_ms: u32, period_ms: u32) -> bool {
    // the ioctl wants the device open for writing
    let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) else { return false };
    let vals = [delay_ms, period_ms];
    // SAFETY: EVIOCSREP reads two unsigned ints from our array.
    unsafe { libc::ioctl(f.as_raw_fd(), EVIOCSREP as _, vals.as_ptr()) >= 0 }
}

/// Applies the requested key repeat to the keyboards when it changes or a keyboard appears.
pub struct RepeatApplier {
    gen: u32,
    devices: usize,
}

impl Default for RepeatApplier {
    fn default() -> Self {
        RepeatApplier { gen: u32::MAX, devices: 0 }
    }
}

impl RepeatApplier {
    pub fn tick(&mut self, devs: &[Device]) {
        let gen = REPEAT_GEN.load(std::sync::atomic::Ordering::SeqCst);
        if gen == self.gen && devs.len() == self.devices {
            return;
        }
        (self.gen, self.devices) = (gen, devs.len());
        if let Some((d, p)) = current_repeat() {
            for dev in devs.iter().filter(|d| d.is_keyboard) {
                if !set_repeat(&dev.path, d, p) {
                    log::warn!("input: could not set the key repeat of {}", dev.path);
                }
            }
        }
    }
}

/// Opens the node of every touch screen again when the touch recovery asked for it (`iyakki_agent::touchrecover`, first step of its
/// ladder). `seen` is the generation already handled by the caller (start it at the current one). Returns how many were reopened.
pub fn apply_reopen(devs: &mut [Device], seen: &mut u64) -> usize {
    use std::os::unix::fs::OpenOptionsExt;
    let gen = iyakki_agent::touchrecover::reopen_generation();
    if gen == *seen {
        return 0;
    }
    *seen = gen;
    let mut n = 0;
    for d in devs.iter_mut().filter(|d| d.touch.is_some()) {
        match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&d.path) {
            Ok(f) => {
                d.file = f;
                n += 1;
            }
            Err(e) => log::info!("input: touch node {} could not be opened again: {e}", d.path),
        }
    }
    log::info!("input: touch recovery: {n} touch node(s) opened again");
    n
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

/// Scans for input devices on a thread of its own (twice a second) and hands over the ones it finds. Opening an event device the kernel is still
/// setting up can block for seconds at boot (5.3 s seen on a Mac mini 2018): a screen that animates must not wait for it.
pub struct BackgroundScan {
    found: std::sync::Arc<std::sync::Mutex<Vec<Device>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl BackgroundScan {
    pub fn start() -> BackgroundScan {
        use std::sync::atomic::Ordering::Relaxed;
        let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (f, s) = (found.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("iyakki-input-scan".into()).spawn(move || {
            // path -> the node's inode: a node the kernel made again under the same name (a driver that binds a second time, as
            // hid-multitouch does for a touch monitor) is another device and is opened again
            let mut seen: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
            let ino = |p: &str| {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(p).map(|m| m.ino()).unwrap_or(0)
            };
            while !s.load(Relaxed) {
                // a node that is gone is forgotten: the kernel reuses `event15` for the next device plugged in
                seen.retain(|p, i| std::path::Path::new(p).exists() && ino(p) == *i);
                // only nodes not already handed over are opened: a device in use is never opened a second time
                let mut fresh: Vec<Device> = Vec::new();
                for p in event_nodes() {
                    if s.load(Relaxed) {
                        break;
                    }
                    if seen.contains_key(&p) {
                        continue;
                    }
                    if let Some(d) = open_device(&p) {
                        let i = ino(&p);
                        seen.insert(p, i);
                        fresh.push(d);
                    }
                }
                if !fresh.is_empty() {
                    f.lock().unwrap_or_else(|p| p.into_inner()).extend(fresh);
                }
                // every half second: a touch monitor that resets its USB hub when the display link renegotiates (a USB-C monitor) is back
                // within that, and its replacement is opened without the 2 s wait this used to have
                for _ in 0..5 {
                    if s.load(Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
        BackgroundScan { found, stop }
    }

    /// Move the devices found since the last call into `into` (never waits: when the scan thread holds the list, next time).
    pub fn take_into(&self, into: &mut Vec<Device>) {
        if let Ok(mut g) = self.found.try_lock() {
            into.append(&mut g);
        }
    }
}

impl Drop for BackgroundScan {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// One event with its device's absolute range (tablets), pad range (touchpads) and touch screen description.
pub type Polled = (Raw, Option<(i32, i32)>, Option<crate::touchpad::PadRange>, Option<std::sync::Arc<crate::touchmap::TouchDev>>);

/// Wait up to `timeout_ms` for input, then read everything available. Dead devices are dropped.
/// Each event comes with its device's absolute range (tablets) and pad range (touchpads).
/// A finger came down: a new multi-touch contact, or the touch or left button pressed.
fn is_finger_down(r: &Raw) -> bool {
    (r.typ == EV_ABS && r.code == ABS_MT_TRACKING_ID && r.value >= 0) || (r.typ == EV_KEY && (r.code == BTN_TOUCH || r.code == BTN_LEFT) && r.value == 1)
}

/// Whether a finger device's events are dropped as an echo: the mouse-like interface of a touch monitor (`finger` without a touch
/// mapping) is muted only while a multi-touch device is present AND has spoken. A multi-touch device that never spoke leaves the
/// word to the mouse-like interface; touch devices themselves are never muted.
pub fn mouse_like_muted(finger: bool, is_touch: bool, touch_spoke: bool) -> bool {
    finger && !is_touch && touch_spoke
}

/// The first fingers of a boot are written to the log (which device, what it said, whether the shell used it), so a touch screen that
/// does nothing can be told apart from one whose events never arrive. Capped so a long session does not fill the log.
fn log_finger_events(path: &str, evs: &[Raw], echo: bool) {
    static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    const MAX: u32 = 40;
    for r in evs {
        let down = is_finger_down(r);
        let up = (r.typ == EV_ABS && r.code == ABS_MT_TRACKING_ID && r.value < 0) || (r.typ == EV_KEY && (r.code == BTN_TOUCH || r.code == BTN_LEFT) && r.value == 0);
        if !(down || up) {
            continue;
        }
        if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= MAX {
            return;
        }
        log::info!("input: finger {} on {path} (type {} code {} value {}){}", if down { "down" } else { "up" }, r.typ, r.code, r.value, if echo { " - ignored, the multi-touch interface speaks" } else { "" });
    }
}

pub fn poll_devices(devs: &mut Vec<Device>, timeout_ms: i32) -> Vec<Polled> {
    let mut out = Vec::new();
    if devs.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms.max(0) as u64));
        return out;
    }
    let mut fds: Vec<libc::pollfd> = devs.iter().map(|d| libc::pollfd { fd: d.fd(), events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: `fds` is a valid array of pollfd for the duration of the call.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    let mut dead = Vec::new();
    // the mouse-like interface of a touch monitor repeats what its multi-touch interface says: while the touch device speaks, only it is
    // heard. A touch device that has never spoken is a controller left in mouse mode (the Mac mini's wch.cn panel after a cold boot:
    // its multi-touch interface is silent and only the mouse-like one reports), so then the mouse-like interface is the touch screen.
    let has_touch = devs.iter().any(|d| d.touch.is_some());
    let touch_spoke = devs.iter().any(|d| d.touch.is_some() && d.spoke);
    let echo: Vec<bool> = devs.iter().map(|d| mouse_like_muted(d.finger, d.touch.is_some(), touch_spoke)).collect();
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
                let evs = parse_events(&buf[..n]);
                if !devs[i].spoke {
                    devs[i].spoke = true;
                    if devs[i].finger && devs[i].touch.is_none() && has_touch && !touch_spoke {
                        log::info!("input: {}: the multi-touch interface has not spoken since it was opened; this mouse-like interface speaks for the touch screen (controller in mouse mode)", devs[i].path);
                    }
                }
                if echo[i] {
                    continue; // read (so it does not wake us again), not used
                }
                if devs[i].finger && evs.iter().any(is_finger_down) {
                    crate::osk::note_touch();
                }
                if devs[i].finger {
                    log_finger_events(&devs[i].path, &evs, echo[i]);
                    if devs[i].touch.is_none() {
                        // the recovery ladder counts these too: a panel heard through its mouse-like interface is not a silent one
                        for _ in &evs {
                            iyakki_agent::touchrecover::note_event();
                        }
                    }
                }
                out.extend(evs.into_iter().map(|r| (r, devs[i].abs_max, devs[i].pad, devs[i].touch.clone())));
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

    fn touch_dev() -> crate::touchmap::TouchDev {
        crate::touchmap::TouchDev { key: "27c0:0859@usb-1/input0".into(), id: "27c0:0859".into(), name: "TouchScreen".into(), usb: true, mt: true, x: (0, 4095), y: (0, 4095) }
    }

    fn touch(st: &mut InputState, evs: &[(u16, u16, i32)]) -> Vec<Event> {
        let d = touch_dev();
        evs.iter().flat_map(|(t, c, v)| st.feed_touch_full(Raw { typ: *t, code: *c, value: *v }, &d)).collect()
    }

    fn downs(evs: &[Event]) -> usize {
        evs.iter().filter(|e| matches!(e, Event::MouseDown(..))).count()
    }

    #[test]
    fn a_contact_that_never_lifted_keeps_later_touches_out_until_the_device_change_is_noticed() {
        let mut st = InputState::new(1920, 1080);
        let contact = [(EV_ABS, ABS_MT_TRACKING_ID, 7), (EV_ABS, 0x35, 100), (EV_ABS, 0x36, 100), (EV_SYN, SYN_REPORT, 0)];
        assert_eq!(downs(&touch(&mut st, &contact)), 1, "the first contact is a press");
        // the monitor's hub resets; the contact was never lifted. A later finger (a new device, the same key) is not taken
        assert_eq!(downs(&touch(&mut st, &[(EV_ABS, ABS_MT_TRACKING_ID, 8), (EV_ABS, 0x35, 2000), (EV_ABS, 0x36, 2000), (EV_SYN, SYN_REPORT, 0)])), 0, "the stuck contact owns the pointer");
        // the shell notices the device went and came back: the state is dropped and the stuck finger lifted
        let k = vec![touch_dev().key];
        let evs = st.touch_devices_changed(&k, &k, &k);
        assert!(evs.iter().any(|e| matches!(e, Event::MouseUp(..))), "the stuck finger is lifted");
        assert_eq!(downs(&touch(&mut st, &[(EV_ABS, ABS_MT_TRACKING_ID, 9), (EV_ABS, 0x35, 2000), (EV_ABS, 0x36, 2000), (EV_SYN, SYN_REPORT, 0)])), 1, "the next touch is a press again");
    }

    #[test]
    fn a_device_that_left_is_forgotten_and_one_that_stayed_is_left_alone() {
        let mut st = InputState::new(1920, 1080);
        touch(&mut st, &[(EV_ABS, ABS_MT_TRACKING_ID, 1), (EV_ABS, 0x35, 100), (EV_ABS, 0x36, 100), (EV_SYN, SYN_REPORT, 0)]);
        let k = vec![touch_dev().key];
        assert!(st.touch_devices_changed(&k, &k, &[]).is_empty(), "still there, nothing new: nothing changes");
        let evs = st.touch_devices_changed(&k, &[], &[]);
        assert!(evs.iter().any(|e| matches!(e, Event::MouseUp(..))), "gone: lifted");
    }

    fn abs_frame(st: &mut InputState, abs: Option<(i32, i32)>, evs: &[(u16, u16, i32)]) -> Vec<Event> {
        let mut out = Vec::new();
        for (t, c, v) in evs {
            out.extend(st.feed(Raw { typ: *t, code: *c, value: *v }, abs));
        }
        out
    }

    #[test]
    fn a_touch_monitors_button_before_its_position_clicks_where_the_finger_is() {
        let mut st = InputState::new(1920, 1080);
        let abs = Some((4095, 4095));
        // the first touch, near the top left
        let a = abs_frame(&mut st, abs, &[(EV_KEY, BTN_LEFT, 1), (EV_ABS, ABS_X, 400), (EV_ABS, ABS_Y, 400), (EV_SYN, SYN_REPORT, 0)]);
        let down = a.iter().find_map(|e| if let Event::MouseDown(x, y) = e { Some((*x, *y)) } else { None }).expect("a press");
        assert!(down.0 < 300 && down.1 < 200, "at the first finger: {down:?}");
        abs_frame(&mut st, abs, &[(EV_KEY, BTN_LEFT, 0), (EV_SYN, SYN_REPORT, 0)]);
        // the second touch, far away: the press is there, not at the first place
        let b = abs_frame(&mut st, abs, &[(EV_KEY, BTN_LEFT, 1), (EV_ABS, ABS_X, 3600), (EV_ABS, ABS_Y, 3500), (EV_SYN, SYN_REPORT, 0)]);
        let down = b.iter().find_map(|e| if let Event::MouseDown(x, y) = e { Some((*x, *y)) } else { None }).expect("a press");
        assert!(down.0 > 1500 && down.1 > 800, "at the second finger, not the first: {down:?}");
        // a mouse (no absolute range) still clicks at once, as before
        let mut st = InputState::new(1920, 1080);
        let c = abs_frame(&mut st, None, &[(EV_KEY, BTN_LEFT, 1)]);
        assert!(matches!(c.as_slice(), [Event::MouseDown(..)]), "{c:?}");
    }

    #[test]
    fn a_finger_coming_down_is_told_from_the_rest_of_a_touch_frame() {
        let raw = |typ, code, value| Raw { typ, code, value };
        assert!(is_finger_down(&raw(EV_ABS, ABS_MT_TRACKING_ID, 5)), "a new multi-touch contact");
        assert!(!is_finger_down(&raw(EV_ABS, ABS_MT_TRACKING_ID, -1)), "lifting is not pressing");
        assert!(is_finger_down(&raw(EV_KEY, BTN_TOUCH, 1)) && is_finger_down(&raw(EV_KEY, BTN_LEFT, 1)), "the touch or left button of a touch monitor's mouse-like interface");
        assert!(!is_finger_down(&raw(EV_KEY, BTN_LEFT, 0)) && !is_finger_down(&raw(EV_REL, 0, 3)));
    }

    /// KI-077: the Mac mini's wch.cn panel comes out of a cold boot in mouse mode: its multi-touch interface is listed but silent and
    /// only the mouse-like interface reports. The mouse-like interface is muted only once a multi-touch device has actually spoken.
    #[test]
    fn mouse_like_interface_speaks_until_the_touch_device_does() {
        // both present, touch device silent so far: the mouse-like interface is heard
        assert!(!mouse_like_muted(true, false, false), "controller in mouse mode: the mouse-like interface is the touch screen");
        // the touch device has spoken: the mouse-like interface is an echo
        assert!(mouse_like_muted(true, false, true), "with a speaking multi-touch interface the mouse-like one is an echo");
        // touch devices and ordinary pointers are never muted
        assert!(!mouse_like_muted(true, true, true) && !mouse_like_muted(true, true, false));
        assert!(!mouse_like_muted(false, false, true), "a mouse is not an echo of anything");
    }

    #[test]
    fn the_background_scan_never_makes_the_caller_wait_and_stops_when_dropped() {
        let t = std::time::Instant::now();
        let scan = BackgroundScan::start();
        let mut devs = Vec::new();
        scan.take_into(&mut devs); // returns at once, whatever the scan thread is doing
        assert!(t.elapsed() < std::time::Duration::from_millis(200), "starting and taking must not wait for the scan");
        std::thread::sleep(std::time::Duration::from_millis(250));
        scan.take_into(&mut devs); // what was found (maybe nothing on a build machine) arrives without waiting
        let stop = scan.stop.clone();
        drop(scan);
        assert!(stop.load(std::sync::atomic::Ordering::Relaxed), "dropping the handle tells the scan thread to stop");
    }

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
    fn ctrl_alt_chords_become_layout_events_and_nothing_else_changes() {
        use crate::panes::{LayoutKey, Side};
        let mut st = InputState::new(800, 600);
        let down = |st: &mut InputState, code: u16| st.feed(Raw { typ: EV_KEY, code, value: 1 }, None);
        let up = |st: &mut InputState, code: u16| st.feed(Raw { typ: EV_KEY, code, value: 0 }, None);
        // Alt alone or Ctrl alone: the keys behave as before
        down(&mut st, 56);
        assert_eq!(down(&mut st, 105), vec![Event::Key(Key::Left)], "Alt+Left is a plain Left for the apps");
        up(&mut st, 105);
        down(&mut st, 29);
        assert_eq!(down(&mut st, 105), vec![Event::Layout(LayoutKey::Snap(Side::Left))]);
        up(&mut st, 105);
        assert_eq!(down(&mut st, 106), vec![Event::Layout(LayoutKey::Snap(Side::Right))]);
        up(&mut st, 106);
        assert_eq!(down(&mut st, 103), vec![Event::Layout(LayoutKey::Snap(Side::Up))]);
        assert_eq!(down(&mut st, 108), vec![Event::Layout(LayoutKey::Snap(Side::Down))]);
        assert_eq!(down(&mut st, 3), vec![Event::Layout(LayoutKey::Apply(2))], "Ctrl+Alt+2");
        assert_eq!(down(&mut st, 10), vec![Event::Layout(LayoutKey::Apply(9))]);
        assert_eq!(down(&mut st, 11), vec![Event::Layout(LayoutKey::Picker)], "Ctrl+Alt+0");
        assert_eq!(down(&mut st, 15), vec![Event::Layout(LayoutKey::Cycle { back: false })]);
        assert_eq!(down(&mut st, 50), vec![Event::Layout(LayoutKey::NextDisplay)]);
        down(&mut st, 42);
        assert_eq!(down(&mut st, 105), vec![Event::Layout(LayoutKey::Nudge(Side::Left))]);
        assert_eq!(down(&mut st, 15), vec![Event::Layout(LayoutKey::Cycle { back: true })]);
        up(&mut st, 42);
        // a Ctrl+Alt key that is no chord still reaches the apps as a Ctrl shortcut
        assert_eq!(down(&mut st, 47), vec![Event::Key(Key::Ctrl('v'))]);
        up(&mut st, 56);
        up(&mut st, 29);
        // right Alt works too, and releasing it ends the chords
        down(&mut st, 29);
        down(&mut st, 100);
        assert_eq!(down(&mut st, 4), vec![Event::Layout(LayoutKey::Apply(3))]);
        up(&mut st, 100);
        assert_eq!(down(&mut st, 4), vec![Event::Key(Key::Char('3'))], "Ctrl+3 without Alt is not a chord");
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
        // the wheel is read at the end of the frame and handed out as pixel steps (see `scroll_tick`)
        assert!(st.feed(Raw { typ: EV_REL, code: 8, value: 1 }, None).is_empty());
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
    fn a_touchpad_swipe_moves_the_pointer_a_tap_clicks_and_the_old_paths_are_untouched() {
        use crate::touchpad::PadRange;
        const PAD: PadRange = PadRange { x_min: -5087, x_max: 8270, y_min: -182, y_max: 6749, major_max: 5000 };
        let mut st = InputState::new(1920, 1080);
        let (x0, y0) = (st.x, st.y);
        let t = std::cell::Cell::new(1000u64);
        let mut ev = Vec::new();
        let frame = |st: &mut InputState, ms: u64, x: i32, y: i32, ev: &mut Vec<Event>| {
            t.set(t.get() + ms);
            for (c, v) in [(ABS_MT_SLOT, 0), (ABS_MT_TRACKING_ID, 4), (ABS_MT_POSITION_X, x), (ABS_MT_POSITION_Y, y), (0x30, 700)] {
                ev.extend(st.feed_pad_at(Raw { typ: EV_ABS, code: c, value: v }, &PAD, t.get()));
            }
            ev.extend(st.feed_pad_at(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, &PAD, t.get()));
        };
        frame(&mut st, 0, 0, 3000, &mut ev);
        for i in 1..=8 {
            frame(&mut st, 16, i * 400, 3000, &mut ev);
        }
        assert!(ev.iter().any(|e| matches!(e, Event::MouseMove(..))));
        assert!(st.x > x0 + 100 && st.y == y0, "moved right only: ({}, {})", st.x, st.y);
        ev.clear();
        // lift, then a tap
        for (c, v) in [(ABS_MT_SLOT, 0), (ABS_MT_TRACKING_ID, -1)] {
            st.feed_pad_at(Raw { typ: EV_ABS, code: c, value: v }, &PAD, t.get() + 16);
        }
        st.feed_pad_at(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, &PAD, t.get() + 16);
        t.set(t.get() + 500);
        frame(&mut st, 0, 100, 100, &mut ev);
        frame(&mut st, 60, 100, 100, &mut ev);
        for (c, v) in [(ABS_MT_SLOT, 0), (ABS_MT_TRACKING_ID, -1)] {
            ev.extend(st.feed_pad_at(Raw { typ: EV_ABS, code: c, value: v }, &PAD, t.get() + 40));
        }
        ev.extend(st.feed_pad_at(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, &PAD, t.get() + 40));
        let (x, y) = (st.x, st.y);
        assert_eq!(ev, vec![Event::MouseDown(x, y), Event::MouseUp(x, y)]);
        // the physical button and a mouse wheel keep working through the old path
        assert_eq!(st.feed_pad_at(Raw { typ: EV_KEY, code: BTN_LEFT, value: 1 }, &PAD, t.get() + 50), vec![Event::MouseDown(x, y)]);
        crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
        st.fake_ms = Some(0);
        st.feed(Raw { typ: EV_REL, code: REL_WHEEL, value: 1 }, None);
        st.feed(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, None);
        assert!(st.scroll_tick().iter().any(|e| matches!(e, Event::ScrollPx(p) if *p > 0)), "a wheel click scrolls up");
        // the settings the Keyboard page writes
        request_touchpad(false);
        assert!(!pad_config().tap_to_click);
        request_touchpad(true);
    }

    // ---- scrolling: raw wheel and trackpad events become the engine's pixel steps

    fn px_total(ev: &[Event]) -> i32 {
        ev.iter().map(|e| if let Event::ScrollPx(p) = e { *p } else { 0 }).sum()
    }

    /// Run `scroll_tick` every 4 ms (the input loop's pace) from `t0` for `ms`, returning the steps and how many there were.
    fn tick_for(st: &mut InputState, t0: u64, ms: u64) -> (Vec<Event>, usize) {
        let mut all = Vec::new();
        let mut t = t0;
        while t < t0 + ms {
            st.fake_ms = Some(t);
            all.extend(st.scroll_tick());
            t += 4;
        }
        let n = all.len();
        (all, n)
    }

    fn rel(st: &mut InputState, code: u16, value: i32) {
        st.feed(Raw { typ: EV_REL, code, value }, None);
    }
    fn syn(st: &mut InputState) {
        st.feed(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, None);
    }

    #[test]
    fn a_hi_res_wheel_is_counted_once_not_twice_and_a_plain_wheel_still_works() {
        crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
        crate::scrolleng::set_raw(false);
        // a hi-res wheel sends the click (1) and its 120 units in one frame, in either order
        for hires_first in [false, true] {
            let mut st = InputState::new(800, 600);
            st.fake_ms = Some(0);
            if hires_first {
                rel(&mut st, REL_WHEEL_HI_RES, 120);
                rel(&mut st, REL_WHEEL, 1);
            } else {
                rel(&mut st, REL_WHEEL, 1);
                rel(&mut st, REL_WHEEL_HI_RES, 120);
            }
            syn(&mut st);
            let (ev, _) = tick_for(&mut st, 0, 600);
            assert_eq!(px_total(&ev), 54, "one click = three lines, not six (hires first: {hires_first})");
        }
        // fractions of a click add up across frames
        let mut st = InputState::new(800, 600);
        for i in 0..15 {
            st.fake_ms = Some(i * 8);
            rel(&mut st, REL_WHEEL_HI_RES, 8);
            syn(&mut st);
        }
        let (ev, _) = tick_for(&mut st, 120, 600);
        assert_eq!(px_total(&ev), 54);
        // a wheel without hi-res: whole clicks, away from the person = up (positive)
        let mut st = InputState::new(800, 600);
        st.fake_ms = Some(0);
        rel(&mut st, REL_WHEEL, -2);
        syn(&mut st);
        assert_eq!(px_total(&tick_for(&mut st, 0, 600).0), -108);
    }

    #[test]
    fn a_scroll_asks_for_about_sixty_steps_a_second_and_stops_when_it_settles() {
        crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
        crate::scrolleng::set_raw(false);
        let mut st = InputState::new(800, 600);
        st.fake_ms = Some(0);
        assert!(!st.scrolling() && st.scroll_tick().is_empty(), "idle: nothing is asked of the shell");
        rel(&mut st, REL_WHEEL, 5);
        syn(&mut st);
        assert!(st.scrolling());
        let (ev, steps) = tick_for(&mut st, 0, 200);
        assert!(steps >= 8 && steps <= 13, "about one step per 16 ms over 200 ms: {steps}");
        let (rest, _) = tick_for(&mut st, 200, 800);
        assert!(!st.scrolling(), "settled: the loop and the shell go back to their low rate");
        assert!(tick_for(&mut st, 1000, 200).1 == 0, "and nothing more is sent");
        assert_eq!(px_total(&ev) + px_total(&rest), 270);
    }

    #[test]
    fn two_finger_travel_is_sent_with_the_chosen_direction_and_momentum_follows_the_lift() {
        use crate::touchpad::PadRange;
        const PAD: PadRange = PadRange { x_min: -5087, x_max: 8270, y_min: -182, y_max: 6749, major_max: 5000 };
        let keep = crate::scrolleng::config();
        let run = |natural: bool, momentum: bool| {
            crate::scrolleng::request_config(crate::scrolleng::ScrollConfig { pad_natural: natural, momentum, ..Default::default() });
            crate::scrolleng::set_raw(false);
            let mut st = InputState::new(1920, 1080);
            let mut t = 1000u64;
            let mut steps = Vec::new();
            let frame = |st: &mut InputState, t: &mut u64, dy: i32, lift: bool, steps: &mut Vec<Event>| {
                *t += 16;
                st.fake_ms = Some(*t);
                for slot in 0..2 {
                    for (c, v) in [(ABS_MT_SLOT, slot), (ABS_MT_TRACKING_ID, if lift { -1 } else { 10 + slot }), (ABS_MT_POSITION_X, 1000 + slot * 2000), (ABS_MT_POSITION_Y, 3000 + dy), (0x30, 600)] {
                        if lift && c != ABS_MT_SLOT && c != ABS_MT_TRACKING_ID {
                            continue;
                        }
                        st.feed_pad_at(Raw { typ: EV_ABS, code: c, value: v }, &PAD, *t);
                    }
                }
                steps.extend(st.feed_pad_at(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, &PAD, *t));
                steps.extend(st.scroll_tick());
            };
            frame(&mut st, &mut t, 0, false, &mut steps);
            frame(&mut st, &mut t, 0, false, &mut steps);
            frame(&mut st, &mut t, 0, false, &mut steps);
            for i in 1..=12 {
                frame(&mut st, &mut t, i * 120, false, &mut steps);
            }
            let during = px_total(&steps);
            assert!(steps.iter().all(|e| !matches!(e, Event::MouseMove(..))), "two fingers never move the pointer");
            frame(&mut st, &mut t, 0, true, &mut steps);
            let at_lift = px_total(&steps);
            let mut after = 0;
            for _ in 0..200 {
                t += 16;
                st.fake_ms = Some(t);
                after += px_total(&st.scroll_tick());
            }
            (during, at_lift - during, after, st.scrolling())
        };
        let (down, _, coast, still) = run(true, true);
        assert!(down > 0, "natural: fingers down = content follows = scrolls up (positive): {down}");
        assert!(coast.abs() > 50 && coast > 0 && !still, "momentum carries on the same way and ends: {coast}");
        let (down, _, coast, _) = run(false, true);
        assert!(down < 0 && coast < 0, "traditional trackpad: the other way");
        let (_, _, coast, _) = run(true, false);
        assert_eq!(coast, 0, "no momentum when it is off");
        crate::scrolleng::request_config(keep);
    }

    #[test]
    fn a_new_touch_stops_the_coasting() {
        use crate::touchpad::PadRange;
        const PAD: PadRange = PadRange { x_min: -5087, x_max: 8270, y_min: -182, y_max: 6749, major_max: 5000 };
        crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
        crate::scrolleng::set_raw(false);
        let mut st = InputState::new(1920, 1080);
        // a fling already in the engine
        st.scroll.pad(0.01, 1000, &crate::scrolleng::ScrollConfig::default());
        for i in 0..8 {
            st.scroll.pad(0.01, 1016 + i * 16, &crate::scrolleng::ScrollConfig::default());
        }
        st.scroll.pad_end(1150, &crate::scrolleng::ScrollConfig::default());
        st.fake_ms = Some(1200);
        st.scroll_tick(); // the last trackpad movement goes out; what is left is the coasting
        assert!(st.scrolling(), "coasting");
        for (c, v) in [(ABS_MT_SLOT, 0), (ABS_MT_TRACKING_ID, 3), (ABS_MT_POSITION_X, 100), (ABS_MT_POSITION_Y, 100), (0x30, 600)] {
            st.feed_pad_at(Raw { typ: EV_ABS, code: c, value: v }, &PAD, 1200);
        }
        st.feed_pad_at(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, &PAD, 1200);
        assert!(!st.scrolling(), "a finger landing stops the momentum at once");
    }

    #[test]
    fn the_end_of_a_list_stops_momentum_through_the_shared_flag() {
        crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
        let mut st = InputState::new(800, 600);
        st.fake_ms = Some(0);
        rel(&mut st, REL_WHEEL, -10);
        syn(&mut st);
        assert!(st.scrolling());
        crate::scrolleng::note_bound(); // a tile that reached its end says so
        st.fake_ms = Some(16);
        assert!(st.scroll_tick().is_empty() && !st.scrolling());
    }

    #[test]
    fn parses_raw_input_event_records() {
        let sz = std::mem::size_of::<InputEvent>();
        let mut buf = vec![0u8; sz * 2];
        for (i, (t, c, v)) in [(EV_KEY, 30u16, 1i32), (EV_REL, 0, -7)].iter().enumerate() {
            let ev = InputEvent { time: libc::timeval { tv_sec: 1, tv_usec: 2 }, type_: *t, code: *c, value: *v };
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

#[cfg(test)]
mod profile_tests {
    use super::*;
    use iyakki_agent::keyboard::KeyboardSettings;

    fn press(st: &mut InputState, code: u16) -> Vec<Event> {
        let a = st.feed(Raw { typ: EV_KEY, code, value: 1 }, None);
        st.feed(Raw { typ: EV_KEY, code, value: 0 }, None);
        a
    }
    fn hold(st: &mut InputState, code: u16, on: bool) {
        st.feed(Raw { typ: EV_KEY, code, value: on as i32 }, None);
    }
    fn profile(p: &str, swap: Option<bool>, apple: bool) {
        let devs = if apple { "I: Bus=0003 Vendor=05ac Product=027b Version=0111\nN: Name=\"Apple Inc. Apple Internal Keyboard / Trackpad\"\nH: Handlers=kbd event2\n\n" } else { "" };
        crate::inputprofile::refresh_from(&KeyboardSettings { profile: p.into(), swap_opt_cmd: swap, ..Default::default() }, devs);
    }

    #[test]
    fn mac_profile_command_is_the_shortcut_key_and_pc_is_unchanged() {
        let mut st = InputState::new(100, 100);
        profile("mac", None, true);
        hold(&mut st, 125, true); // left Command (Meta)
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Ctrl('c'))]);
        assert_eq!(press(&mut st, 47), vec![Event::Key(Key::Ctrl('v'))]);
        assert_eq!(press(&mut st, 16), vec![], "Command+Q does nothing");
        assert_eq!(press(&mut st, 105), vec![Event::Key(Key::Home)]);
        hold(&mut st, 125, false);
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Char('c'))], "plain c after Command is released");
        hold(&mut st, 29, true);
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Ctrl('c'))], "Control+C stays Control+C");
        hold(&mut st, 29, false);
        hold(&mut st, 56, true); // Option
        assert_eq!(press(&mut st, 105), vec![Event::Key(Key::Nav { nav: Nav::Left, shift: false, ctrl: true })]);
        hold(&mut st, 56, false);
        profile("pc", None, true);
        hold(&mut st, 125, true);
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Char('c'))], "the Windows key does nothing in the PC profile");
    }

    #[test]
    fn with_the_driver_swap_on_the_key_printed_command_is_still_command() {
        let mut st = InputState::new(100, 100);
        profile("mac", Some(true), true);
        hold(&mut st, 56, true); // the kernel sends Alt for the swapped Command
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Ctrl('c'))]);
        hold(&mut st, 56, false);
        hold(&mut st, 125, true); // and Meta for the Option key: a word jump, not a shortcut
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Char('c'))]);
        // a PC keyboard: the saved swap is not in effect, Meta is Command
        profile("mac", Some(true), false);
        assert_eq!(press(&mut st, 46), vec![Event::Key(Key::Ctrl('c'))]);
    }

    #[test]
    fn control_click_is_a_right_click_only_in_the_mac_profile() {
        let mut st = InputState::new(100, 100);
        profile("mac", None, true);
        hold(&mut st, 29, true);
        let d = st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 1 }, None);
        let u = st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 0 }, None);
        assert!(matches!(d.as_slice(), [Event::MouseRight(..)]) && u.is_empty(), "{d:?} {u:?}");
        profile("pc", None, true);
        let d = st.feed(Raw { typ: EV_KEY, code: BTN_LEFT, value: 1 }, None);
        assert!(matches!(d.as_slice(), [Event::MouseDown(..)]), "{d:?}");
    }
}
