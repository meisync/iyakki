//! Settings -> Keyboard and function keys: how the top row of an Apple keyboard behaves (media keys first or F-keys first),
//! ISO layout and Option/Command swap, key repeat, the controls the media keys drive (screen and keyboard backlights, sound),
//! a test button for each, the last unusual keys pressed (to find a key's code) and the table of keys IYAKKI reacts to.
//!
//! The choices are the machine's (administrators), kept in the settings file (`keyboard`) and written to the kernel at once and
//! again at every start (`iyakki_agent::keyboard`, `Shell::fn_sync_keyboard`). Nothing here runs a program.

use crate::safeclamp::SafeClamp;
use super::*;
use crate::fnkeys;
use iyakki_agent::keyboard::{self, KeyboardSettings};

/// Where the kernel's files are read and written (tests use a fake directory).
pub(super) struct KbdPanel {
    pub(super) sys: std::path::PathBuf,
    /// The Scroll test: a tall list to feel the scrolling choices on.
    pub(super) test: crate::scrolllist::ScrollList,
}

impl Default for KbdPanel {
    fn default() -> KbdPanel {
        KbdPanel { sys: "/sys".into(), test: Default::default() }
    }
}

/// The Scroll test's scroll bar, and how many rows it holds.
pub(super) const KBD_TEST_BAR: usize = 9720;
const TEST_ROWS: usize = 120;
/// The most the Scroll speed button goes to, and the steps it cycles through (percent).
const SPEEDS: [u32; 7] = [50, 75, 100, 150, 200, 250, 300];
const WHEEL_LINES: [u32; 6] = [1, 2, 3, 5, 8, 10];

/// The delays and rates the buttons cycle through (0 = the kernel's own).
const DELAYS: [u32; 7] = [0, 200, 300, 400, 500, 700, 1000];
const RATES: [u32; 6] = [0, 10, 15, 20, 30, 40];

fn next_in(list: &[u32], cur: u32) -> u32 {
    list.iter().position(|v| *v == cur).map(|i| list[(i + 1) % list.len()]).unwrap_or(list[0])
}

fn mode_label(m: &str) -> &'static str {
    match m {
        "media" => "media keys first (Fn + F-key gives F1 to F12)",
        "fkeys" => "F-keys first (Fn + F-key gives the media key)",
        "auto" => "automatic (the keyboard decides)",
        _ => "not changed (the kernel's setting)",
    }
}

fn touchbar_label(m: &str) -> &'static str {
    match m {
        "off" => "off (dark)",
        "keys" => "the firmware's keys only (Esc, F-keys or media keys; Fn switches)",
        "strip" => "IYAKKI strip (the keys come back if it cannot be used)",
        _ => "default: IYAKKI strip where the display works, the keys otherwise",
    }
}

/// The sets of strips the button cycles through ("" list = all four).
const STRIP_SETS: [&[&str]; 8] = [&[], &["system", "recent"], &["system", "ai"], &["system", "recent", "ai"], &["system"], &["recent"], &["ai"], &["app"]];

fn tri(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "on",
        Some(false) => "off",
        None => "not changed",
    }
}

impl SettingsApp {
    /// Use another directory for the kernel's files (tests).
    pub fn with_keyboard_sys(mut self, sys: std::path::PathBuf) -> SettingsApp {
        self.kbd = KbdPanel { sys, ..KbdPanel::default() };
        self
    }

    fn kbd_settings(cx: &AppCx) -> KeyboardSettings {
        Self::svc(cx).settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default()
    }

    /// Change the saved choices, then apply them to the kernel now.
    fn kbd_update(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut KeyboardSettings)) -> Option<KeyboardSettings> {
        if !self.admin(cx) {
            return None;
        }
        let r = Self::svc(cx).settings.lock().unwrap().update(|s| f(&mut s.keyboard));
        if let Err(e) = r {
            self.fail(e.to_string());
            return None;
        }
        cx.audit.record(&cx.session.user, "settings.keyboard", what);
        let k = Self::kbd_settings(cx);
        if let Some((d, p)) = k.repeat() {
            crate::input::request_repeat(d, p);
        }
        let (tap, _) = k.trackpad();
        crate::input::request_touchpad(tap);
        crate::scrolleng::request_config(k.scroll().into());
        match keyboard::apply_apple(&self.kbd.sys, &k) {
            Ok(_) => Some(k),
            Err(e) => {
                self.fail(format!("Saved, but not applied now: {e}"));
                None
            }
        }
    }

    pub(super) fn handle_keyboard(&mut self, k: &Key, cx: &AppCx) {
        let cur = Self::kbd_settings(cx);
        let st = keyboard::apple_state(&self.kbd.sys);
        let driver = st.driver || st.spi;
        match k {
            Key::Up => self.sel[self.tab] = self.sel[self.tab].saturating_sub(1),
            Key::Down => self.sel[self.tab] = (self.sel[self.tab] + 1).min(fnkeys::KEYS.len().saturating_sub(1)),
            Key::Char('f') => {
                let next = match cur.fn_mode.as_str() {
                    "" => "media",
                    "media" => "fkeys",
                    "fkeys" => "auto",
                    _ => "",
                };
                if self.kbd_update(cx, &format!("fn_mode={next}"), |s| s.fn_mode = next.to_string()).is_some() {
                    let tail = if driver { "" } else { " (no Apple keyboard driver is loaded here; it is applied when one appears)" };
                    self.ok(format!("Function keys: {}{tail}.", mode_label(next)));
                }
            }
            Key::Char('u') => {
                let next = match cur.touchbar.as_str() {
                    "" => "strip",
                    "strip" => "keys",
                    "keys" => "off",
                    _ => "",
                };
                if self.kbd_update(cx, &format!("touchbar={next}"), |s| s.touchbar = next.to_string()).is_some() {
                    self.ok(format!("Touch Bar: {}.", touchbar_label(next)));
                }
            }
            Key::Char('e') => {
                let at = STRIP_SETS.iter().position(|set| set.iter().map(|x| x.to_string()).collect::<Vec<_>>() == cur.touchbar_strips).map_or(0, |i| (i + 1) % STRIP_SETS.len());
                let next: Vec<String> = STRIP_SETS[at].iter().map(|x| x.to_string()).collect();
                let name = if next.is_empty() { "all four".to_string() } else { next.join(", ") };
                if self.kbd_update(cx, &format!("touchbar_strips={name}"), |s| s.touchbar_strips = next.clone()).is_some() {
                    self.ok(format!("Touch Bar strips: {name}."));
                }
            }
            Key::Char('l') => {
                let next = (cur.touchbar_flip + 1) % 4;
                if self.kbd_update(cx, &format!("touchbar_flip={next}"), |s| s.touchbar_flip = next).is_some() {
                    self.ok(format!("Touch Bar picture flip {next} (0 as is, 1 mirrored along the bar, 2 mirrored across, 3 both)."));
                }
            }
            Key::Char('i') | Key::Char('o') => {
                let iso = *k == Key::Char('i');
                let now = if iso { cur.iso_layout } else { cur.swap_opt_cmd };
                let next = match now {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
                let name = if iso { "ISO layout" } else { "Option and Command swapped" };
                if self.kbd_update(cx, &format!("{name}={}", tri(next)), |s| if iso { s.iso_layout = next } else { s.swap_opt_cmd = next }).is_some() {
                    self.ok(format!("{name}: {}. A keyboard plugged in later may need to be plugged in again.", tri(next)));
                }
            }
            Key::Char('p') | Key::Char('n') => {
                let tap = *k == Key::Char('p');
                let (t, n) = cur.trackpad();
                let next = !(if tap { t } else { n });
                let name = if tap { "Trackpad tap to click" } else { "Trackpad natural scrolling" };
                if self.kbd_update(cx, &format!("{name}={}", if next { "on" } else { "off" }), |s| if tap { s.trackpad_tap = Some(next) } else { s.trackpad_natural = Some(next) }).is_some() {
                    self.ok(format!("{name}: {}.", if next { "on" } else { "off" }));
                }
            }
            Key::Char('w') | Key::Char('g') => {
                let wheel = *k == Key::Char('w');
                let now = cur.wheel_natural.unwrap_or(false);
                let next = !now;
                if wheel {
                    if self.kbd_update(cx, &format!("wheel_natural={}", if next { "on" } else { "off" }), |s| s.wheel_natural = Some(next)).is_some() {
                        self.ok(format!("Mouse wheel direction: {}.", if next { "natural (rolling towards you moves the page up)" } else { "traditional (rolling away from you moves the page up)" }));
                    }
                } else {
                    let m = cur.scroll().smooth;
                    if self.kbd_update(cx, &format!("scroll_smooth={}", if m { "off" } else { "on" }), |s| s.scroll_smooth = Some(!m)).is_some() {
                        self.ok(format!("Smooth scrolling: {}.", if m { "off" } else { "on" }));
                    }
                }
            }
            Key::Char('s') | Key::Char('y') => {
                let speed = *k == Key::Char('s');
                if speed {
                    let now = cur.scroll().speed_pct;
                    let next = SPEEDS.iter().copied().find(|v| *v > now).unwrap_or(SPEEDS[0]);
                    if self.kbd_update(cx, &format!("scroll_speed_pct={next}"), |s| s.scroll_speed_pct = next).is_some() {
                        self.ok(format!("Scroll speed: {next}%."));
                    }
                } else {
                    let now = cur.scroll().wheel_lines;
                    let next = WHEEL_LINES.iter().copied().find(|v| *v > now).unwrap_or(WHEEL_LINES[0]);
                    if self.kbd_update(cx, &format!("wheel_lines={next}"), |s| s.wheel_lines = next).is_some() {
                        self.ok(format!("A mouse wheel click moves {next} line{}.", if next == 1 { "" } else { "s" }));
                    }
                }
            }
            Key::Char('m') => {
                let m = cur.scroll().momentum;
                if self.kbd_update(cx, &format!("scroll_momentum={}", if m { "off" } else { "on" }), |s| s.scroll_momentum = Some(!m)).is_some() {
                    self.ok(format!("Trackpad momentum: {}.", if m { "off" } else { "on" }));
                }
            }
            Key::Char('d') => {
                let next = next_in(&DELAYS, cur.repeat_delay_ms);
                if self.kbd_update(cx, &format!("repeat_delay={next}"), |s| s.repeat_delay_ms = next).is_some() {
                    self.ok(if next == 0 { "Key repeat delay: the kernel's own.".to_string() } else { format!("Key repeat starts after {next} ms.") });
                }
            }
            Key::Char('r') => {
                let next = next_in(&RATES, cur.repeat_rate);
                if self.kbd_update(cx, &format!("repeat_rate={next}"), |s| s.repeat_rate = next).is_some() {
                    self.ok(if next == 0 { "Key repeat rate: the kernel's own.".to_string() } else { format!("Key repeat: {next} per second.") });
                }
            }
            Key::Char('b') => {
                if !self.admin(cx) {
                    return;
                }
                let sys = self.kbd.sys.clone();
                self.start("testing the screen brightness", move || JobOut::Msg(fnkeys::test_panel(&sys)));
            }
            Key::Char('h') => {
                if !self.admin(cx) {
                    return;
                }
                let sys = self.kbd.sys.clone();
                let level = iyakki_agent::touchprobe::next_level();
                cx.audit.record(&cx.session.user, "settings.touch.probe", &format!("step {level}"));
                self.start("probing the touch screen", move || JobOut::Msg(Ok(iyakki_agent::touchprobe::probe(&sys, level))));
            }
            Key::Char('k') => {
                if !self.admin(cx) {
                    return;
                }
                let mut last = 0;
                let r = fnkeys::adjust_kbd(&self.kbd.sys, fnkeys::KbdOp::Toggle, &mut last).and_then(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                    fnkeys::adjust_kbd(&self.kbd.sys, fnkeys::KbdOp::Toggle, &mut last)
                });
                match r {
                    Ok(_) => self.ok("The keyboard light went off and on again."),
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('t') => {
                if !self.admin(cx) {
                    return;
                }
                let svc = Self::svc(cx).clone();
                cx.audit.record(&cx.session.user, "settings.sound.test", "keyboard page");
                self.start("playing a test sound", move || JobOut::Msg(svc.test_sound(None)));
            }
            _ => {}
        }
    }

    /// (label, value, warn) rows above the table.
    pub(super) fn kbd_rows(&self, cx: &AppCx) -> Vec<(String, String, bool)> {
        let k = Self::kbd_settings(cx);
        let a = keyboard::apple_state(&self.kbd.sys);
        let mut v: Vec<(String, String, bool)> = vec![("Function keys".into(), mode_label(&k.fn_mode).into(), false)];
        v.push((
            "Apple keyboard driver".into(),
            match (a.driver, a.spi) {
                (true, false) => format!("hid_apple loaded, fnmode {}", a.fnmode.map_or("?".to_string(), |m| m.to_string())),
                (false, true) => format!("applespi (built-in MacBook keyboard) loaded, fnmode {}", a.spi_fnmode.map_or("?".to_string(), |m| m.to_string())),
                (true, true) => format!("hid_apple fnmode {}, applespi fnmode {}", a.fnmode.map_or("?".to_string(), |m| m.to_string()), a.spi_fnmode.map_or("?".to_string(), |m| m.to_string())),
                (false, false) => "not loaded (no Apple keyboard here, or the driver is missing)".into(),
            },
            false,
        ));
        let (tap, natural) = k.trackpad();
        v.push(("Trackpad".into(), format!("tap to click {}, natural scrolling {} (a touchpad is used when the kernel shows one)", if tap { "on" } else { "off" }, if natural { "on" } else { "off" }), false));
        let sc = k.scroll();
        v.push(("Scrolling".into(), format!("speed {}%, mouse wheel {} lines a click and {}, trackpad {}, smooth {}, momentum {}", sc.speed_pct, sc.wheel_lines, if sc.wheel_natural { "natural" } else { "traditional" }, if sc.pad_natural { "natural" } else { "traditional" }, if sc.smooth { "on" } else { "off" }, if sc.momentum { "on" } else { "off" }), false));
        v.push(("Touch Bar keys".into(), if a.touchbar.is_empty() { "not found (its keys need the Mac image's Touch Bar driver)".into() } else { format!("{} device(s), mode {} (0 Esc only, 1 F-keys, 2 media keys, 3 off; the Fn key switches 1 and 2 while held)", a.touchbar.len(), a.touchbar[0].1) }, false));
        v.push(("Touch Bar".into(), touchbar_label(&k.touchbar).into(), false));
        let st = crate::touchbar::status_text();
        v.push(("Touch Bar strip".into(), if st.is_empty() { "not started (no T2 MacBook, or turned off)".into() } else { format!("{st}; display only: touching the bar works only when the kernel exposes its touch panel") }, false));
        v.push(("Touch Bar strips".into(), k.strips().join(", ") + &(if k.touchbar_flip != 0 { format!("; picture flip {}", k.touchbar_flip) } else { String::new() }), false));
        v.push(("ISO layout / Option-Command swap".into(), format!("{} / {}", tri(k.iso_layout), tri(k.swap_opt_cmd)), false));
        v.push(("Key repeat".into(), match k.repeat() {
            Some((d, p)) => format!("starts after {d} ms, every {p} ms"),
            None => "the kernel's own (250 ms, 30 per second)".into(),
        }, false));
        v.push(("Layout".into(), "US keyboard (other layouts are planned)".into(), false));
        let panels = iyakki_agent::touchprobe::panels(&self.kbd.sys);
        v.push(("Touch screen".into(), match panels.len() {
            0 => "no USB touch screen listed".to_string(),
            n => format!("{n} found ({}). Touch not working right now? Click Probe touch screen: it switches the screen's USB device off and on", panels.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")),
        }, false));
        let panel = fnkeys::panel_backlights(&self.kbd.sys);
        v.push(("Screen brightness".into(), match panel.first() {
            Some(l) => format!("{} {}% (steps: {})", l.name, l.percent(), fnkeys::brightness_levels(l.max).len()),
            None => format!("none: {}", fnkeys::NO_PANEL.to_lowercase()),
        }, panel.is_empty()));
        let all = fnkeys::all_backlights(&self.kbd.sys);
        if all.len() > panel.len() || panel.len() > 1 {
            v.push(("Other backlights".into(), all.iter().filter(|l| panel.first().is_none_or(|p| p.name != l.name)).map(|l| format!("{} {}%", l.name, l.percent())).collect::<Vec<_>>().join(", "), false));
        }
        let kb = fnkeys::kbd_backlights(&self.kbd.sys);
        v.push(("Keyboard light".into(), match kb.first() {
            Some(l) => format!("{} {}% (max {})", l.name, l.percent(), l.max),
            None => "none found".into(),
        }, false));
        let snd = Self::svc(cx).sound_settings();
        v.push(("Sound".into(), format!("IYAKKI volume {}%{}{}", snd.volume_pct, if snd.muted { ", muted" } else { "" }, if fnkeys::sound_present() { "" } else { " (no sound output found)" }), false));
        let seen = fnkeys::seen();
        v.push(("Last special keys".into(), if seen.is_empty() { "press a media or Fn key to see its code here".into() } else { seen.iter().rev().map(|c| fnkeys::describe_code(*c)).collect::<Vec<_>>().join(" | ") }, false));
        v
    }

    pub(super) fn render_keyboard(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mut y = a.y;
        // on a wide page the Scroll test sits in a column on the right; on a narrow one it goes under the rows when there is room
        let wide = a.w >= 900.0 * s;
        let rows_w = if wide { a.w * 0.62 } else { a.w };
        let mut test_bottom = a.y;
        if wide {
            let col = R::new(a.x + a.w * 0.64, a.y, a.w * 0.36, a.h);
            test_bottom = self.render_scroll_test(g, col, col.y, s, (a.h * 0.6).min(380.0 * s));
        } else {
            self.kbd.test.layout(R::new(0.0, 0.0, 0.0, 0.0), 0.0, 22.0 * s, s);
        }
        let label_w = (210.0 * s).min(rows_w * 0.38);
        for (label, value, warn) in self.kbd_rows(cx) {
            g.text(a.x, y, 14.0 * s, MUTED, &label, Face::Sans);
            let lines = g.wrap(14.0 * s, &value, Face::Bold, rows_w - label_w);
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(a.x + label_w, y + i as f32 * 18.0 * s, 14.0 * s, if warn { rgb(255, 196, 94) } else { TEXT }, l, Face::Bold);
            }
            y += 24.0 * s + if lines.len() > 1 { 18.0 * s * (lines.len().min(3) - 1) as f32 } else { 0.0 };
        }
        y = y.max(test_bottom) + 6.0 * s; // the table starts below the column
        if !wide {
            let room = a.y + a.h - y - 90.0 * s;
            y = self.render_scroll_test(g, a, y, s, (a.h * 0.26).clamp_safe(80.0 * s, 190.0 * s).min(room));
        }
        // the rows above can fill a short page (the touch screen row): the heading is drawn only where it fits
        if y + 20.0 * s <= a.y + a.h {
            g.text(a.x, y, 14.0 * s, ACCENT, "Keys IYAKKI reacts to (Up/Down scroll)", Face::Bold);
        }
        y += 24.0 * s;
        let code_w = 250.0 * s;
        let first = self.sel[self.tab].min(fnkeys::KEYS.len().saturating_sub(1));
        for i in fnkeys::KEYS.iter().skip(first) {
            if y + 18.0 * s > a.y + a.h {
                break;
            }
            let c = g.fit(13.0 * s, &format!("{} {}", i.code, i.name), Face::Sans, code_w - 8.0 * s);
            g.text(a.x, y, 13.0 * s, DIM, &c, Face::Sans);
            let t = g.fit(13.0 * s, i.action, Face::Sans, a.w - code_w);
            g.text(a.x + code_w, y, 13.0 * s, TEXT, &t, Face::Sans);
            y += 19.0 * s;
        }
    }

    /// The Scroll test: a tall list under the scrolling rows. Returns the y below it (nothing is drawn when the page is too short).
    fn render_scroll_test(&mut self, g: &mut Gfx, a: R, y: f32, s: f32, th: f32) -> f32 {
        if th < 60.0 * s {
            self.kbd.test.layout(R::new(0.0, 0.0, 0.0, 0.0), 0.0, 22.0 * s, s);
            return y;
        }
        let title = g.fit(14.0 * s, "Scroll test: roll the wheel or use two fingers here", Face::Bold, a.w);
        g.text(a.x, y, 14.0 * s, ACCENT, &title, Face::Bold);
        let view = R::new(a.x, y + 24.0 * s, a.w, th);
        let row = 22.0 * s;
        self.kbd.test.layout(view, TEST_ROWS as f32 * row, row, s);
        g.rrect(view.x, view.y, view.w, view.h, 6.0 * s, rgb(14, 18, 34));
        let off = self.kbd.test.offset;
        g.clipped(view, |g| {
            let first = (off / row).floor() as usize;
            for i in first..TEST_ROWS.min(first + (view.h / row).ceil() as usize + 2) {
                let ry = view.y + i as f32 * row - off;
                let (c, bold) = if i % 10 == 0 { (ACCENT, Face::Bold) } else { (TEXT, Face::Sans) };
                g.text(view.x + 10.0 * s, ry + 3.0 * s, 13.0 * s, c, &format!("Row {} of {}", i + 1, TEST_ROWS), bold);
            }
        });
        self.kbd.test.draw_bar(g, s, KBD_TEST_BAR, ACCENT);
        view.y + th + 14.0 * s
    }

    /// The Scroll test's pointer events (wheel notches, the scroll bar). True when it took the event.
    pub(super) fn kbd_test_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Scroll(_) => {
                self.kbd.test.pointer = Some(self.mouse);
                self.kbd.test.event(ev, KBD_TEST_BAR)
            }
            Event::MouseMove(..) | Event::MouseUp(..) => self.kbd.test.event(ev, KBD_TEST_BAR),
            Event::AreaAt(id, ..) if *id == KBD_TEST_BAR => self.kbd.test.event(ev, KBD_TEST_BAR),
            _ => false,
        }
    }

    /// A smooth scroll step over the Scroll test.
    pub(super) fn kbd_test_px(&mut self, px: i32) -> bool {
        self.kbd.test.pointer = Some(self.mouse);
        self.kbd.test.wheel_px(px)
    }

    pub(super) fn keyboard_buttons(&self) -> Vec<Btn> {
        let (acc, mut_) = (ACCENT, TEXT);
        vec![("Function keys", acc, 'f'), ("Touch Bar", acc, 'u'), ("Touch Bar strips", mut_, 'e'), ("Touch Bar flip", mut_, 'l'), ("ISO layout", mut_, 'i'), ("Swap Option/Cmd", mut_, 'o'), ("Repeat delay", mut_, 'd'), ("Repeat rate", mut_, 'r'), ("Trackpad tap", mut_, 'p'), ("Trackpad scroll", mut_, 'n'), ("Scroll speed", mut_, 's'), ("Wheel lines", mut_, 'y'), ("Wheel direction", mut_, 'w'), ("Smooth scrolling", mut_, 'g'), ("Momentum", mut_, 'm'), ("Probe touch screen", acc, 'h'), ("Test screen", acc, 'b'), ("Test keyboard light", mut_, 'k'), ("Test sound", mut_, 't')]
    }
}
