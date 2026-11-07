// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menu on screen. `boot-core` holds the state machine; this file turns firmware events into its inputs and draws its view,
//! in text mode (character cells) or on the graphics output (big rows, built-in font, mouse through the simple pointer).

use crate::gfx::{Rgb, Screen};
use crate::sys::{bs, st};
use crate::say;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use boot_core::item::Item;
use boot_core::layout::{self, Levels, Plan, BACK_ROW, MORE_ROW};
use boot_core::menu::{Event as MenuEvent, Input, Layout, Menu, View};
use core::ptr::null_mut;
use uefi_sys::*;

/// What the person (or the countdown) picked.
pub struct Pick {
    /// Index of the item.
    pub item: usize,
    /// The countdown ran out (nobody chose).
    pub timed_out: bool,
}

/// How often the menu gets a tick, in milliseconds.
const TICK_MS: u32 = 100;

fn key_input(k: &InputKey) -> Option<Input> {
    match k.scan_code {
        SCAN_UP => Some(Input::Up),
        SCAN_DOWN => Some(Input::Down),
        SCAN_PAGE_UP => Some(Input::PageUp),
        SCAN_PAGE_DOWN => Some(Input::PageDown),
        SCAN_HOME => Some(Input::Home),
        SCAN_END => Some(Input::End),
        SCAN_ESC => Some(Input::Esc),
        _ => match k.unicode_char {
            13 | 10 => Some(Input::Enter),
            0x1b => Some(Input::Esc),
            _ => None,
        },
    }
}

/// Read the keys that are waiting (`None` when there is none).
pub fn read_key() -> Option<InputKey> {
    let cin = st().con_in;
    if cin.is_null() {
        return None;
    }
    let mut k = InputKey { scan_code: 0, unicode_char: 0 };
    let s = unsafe { ((*cin).read_key_stroke)(cin, &mut k) };
    if s.is_error() {
        None
    } else {
        Some(k)
    }
}

/// Mouse state for the graphics menu: every pointer device the firmware offers (a PS/2 and a USB mouse can both exist).
struct Mouse {
    devices: Vec<(*mut SimplePointer, u64, u64)>,
    x: i64,
    y: i64,
    down: bool,
}

impl Mouse {
    fn open(w: u32, h: u32) -> Option<Mouse> {
        let mut devices = Vec::new();
        for handle in crate::sys::handles_with(&SIMPLE_POINTER_PROTOCOL_GUID) {
            if let Some(p) = crate::sys::protocol::<SimplePointer>(handle, &SIMPLE_POINTER_PROTOCOL_GUID) {
                let (rx, ry) = unsafe { ((*(*p).mode).resolution_x, (*(*p).mode).resolution_y) };
                unsafe { ((*p).reset)(p, 0) };
                say!("menu: pointer present, resolution {rx} x {ry}\n");
                devices.push((p, rx, ry));
            }
        }
        if devices.is_empty() {
            return None;
        }
        Some(Mouse { devices, x: w as i64 / 2, y: h as i64 / 2, down: false })
    }

    /// New position and, on a fresh press, the tap.
    fn poll(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let mut tap = None;
        for &(p, res_x, res_y) in &self.devices {
            let mut s = SimplePointerState::default();
            if unsafe { ((*p).get_state)(p, &mut s) }.is_error() {
                continue;
            }
            // Counts per millimetre is the spec's unit, but firmware reports odd values (65536 seen); outside 1..=256 assume 8.
            // One count moves about one pixel.
            let scale = |v: i32, res: u64| v as i64 * 8 / if (1..=256).contains(&res) { res as i64 } else { 8 };
            self.x = (self.x + scale(s.relative_movement_x, res_x)).clamp(0, w as i64 - 1);
            self.y = (self.y + scale(s.relative_movement_y, res_y)).clamp(0, h as i64 - 1);
            let pressed = s.left_button != 0 && !self.down;
            self.down = s.left_button != 0;
            if pressed {
                tap = Some((self.x as u32, self.y as u32));
            }
        }
        tap
    }
}

const BG: Rgb = Rgb(0x10, 0x12, 0x1c);
const FG: Rgb = Rgb(0xe6, 0xe8, 0xf0);
const DIM: Rgb = Rgb(0x6c, 0x70, 0x80);
const SEL: Rgb = Rgb(0x2a, 0x6f, 0xd6);
const ROW: Rgb = Rgb(0x1e, 0x22, 0x32);
const WARN: Rgb = Rgb(0xff, 0xc8, 0x57);

enum Mode {
    Text { cols: usize, rows: usize },
    Gfx { screen: Screen, scale: u32, mouse: Option<Mouse> },
}

fn text_size() -> (usize, usize) {
    let out = st().con_out;
    let (mut c, mut r) = (80usize, 25usize);
    unsafe {
        let m = (*out).mode;
        if !m.is_null() {
            ((*out).query_mode)(out, (*m).mode as usize, &mut c, &mut r);
        }
    }
    (c.clamp(40, 400), r.clamp(10, 200))
}

fn pad(s: &str, width: usize) -> String {
    let mut o: String = s.chars().take(width).collect();
    while o.chars().count() < width {
        o.push(' ');
    }
    o
}

fn put_text(col: usize, row: usize, attr: usize, s: &str) {
    let out = st().con_out;
    let w = crate::sys::ucs2(s);
    unsafe {
        ((*out).set_cursor_position)(out, col, row);
        ((*out).set_attribute)(out, attr);
        ((*out).output_string)(out, w.as_ptr());
    }
}

fn draw_text(v: &View, cols: usize, rows: usize, waiting: bool) {
    let w = cols - 1;
    put_text(0, 0, 0x0f, &pad(&format!("  {}", v.header), w));
    put_text(0, 1, 0x07, &pad("", w));
    for l in &v.lines {
        let (attr, mark) = if l.selected { (0x70, "> ") } else if l.disabled { (0x08, "  ") } else { (0x07, "  ") };
        put_text(0, l.rect.y as usize, attr, &pad(&format!("  {mark}{}", l.text), w));
    }
    let first = v.lines.first().map_or(3, |l| l.rect.y as usize);
    let end = first + v.lines.len();
    for r in end..rows.saturating_sub(3) {
        put_text(0, r, 0x07, &pad("", w));
    }
    put_text(0, rows - 3, 0x07, &pad(if v.more_above || v.more_below { "  (more: use the arrow keys)" } else { "" }, w));
    put_text(0, rows - 2, if waiting { 0x0e } else { 0x07 }, &pad(&format!("  {}", v.footer.as_deref().unwrap_or("")), w));
    put_text(0, rows - 1, 0x08, &pad("  Up/Down choose, Enter start", w));
}

fn draw_gfx(s: &mut Screen, scale: u32, v: &View, waiting: bool, cursor: Option<(u32, u32)>, bg: Rgb) {
    let (w, h) = (s.w, s.h);
    s.rect(0, 0, w, h, bg);
    let cw = 8 * scale;
    let head = v.lines.first().map_or(h / 6, |l| l.rect.y);
    s.text(cw * 2, (head.saturating_sub(2 * cw)) / 2, &v.header, scale, FG);
    for l in &v.lines {
        let r = l.rect;
        let gap = scale * 2;
        s.rect(r.x, r.y, r.w, r.h.saturating_sub(gap), if l.selected { SEL } else { ROW });
        let max = (r.w / cw).saturating_sub(4) as usize;
        let t: String = l.text.chars().take(max).collect();
        s.text(r.x + cw, r.y + (r.h.saturating_sub(gap) - cw) / 2, &t, scale, if l.disabled { DIM } else { FG });
    }
    if v.more_above || v.more_below {
        s.text(cw * 2, h - 5 * cw, "more rows: use the arrow keys or the mouse wheel", scale.max(2) / 2 + 1, DIM);
    }
    if let Some(f) = &v.footer {
        s.text(cw * 2, h - 3 * cw, f, scale, if waiting { WARN } else { FG });
    }
    if let Some((x, y)) = cursor {
        s.rect(x, y, 3 * scale + 6, 3 * scale + 6, Rgb(255, 255, 255));
        s.rect(x + 1, y + 1, 3 * scale + 4, 3 * scale + 4, Rgb(0, 0, 0));
    }
    s.present();
}

/// The menu drawn under the logo: the splash picture near the top, the list below it on the same background, then the countdown (or
/// notice) line and a key hint. `plan` says where everything goes.
fn draw_under_logo(s: &mut Screen, plan: &Plan, v: &View, waiting: bool, cursor: Option<(u32, u32)>, bg: Rgb, hint: &str) {
    let (w, h) = (s.w, s.h);
    s.rect(0, 0, w, h, bg);
    crate::splash::draw_logo(s, plan.logo_x, plan.logo_y, plan.logo_w, plan.logo_h);
    let sc = plan.text_scale;
    let cw = 8 * sc;
    for l in &v.lines {
        let r = l.rect;
        let gap = (r.h / 12).max(2);
        let rh = r.h.saturating_sub(gap);
        if l.selected {
            s.rect(r.x, r.y, r.w, rh, SEL);
        }
        let max = (r.w / cw).saturating_sub(2) as usize;
        let t: String = l.text.chars().take(max).collect();
        s.text(r.x + cw, r.y + rh.saturating_sub(cw) / 2, &t, sc, if l.disabled { DIM } else { FG });
    }
    if let Some(first) = v.lines.first() {
        let right = first.rect.x + first.rect.w;
        if v.more_above {
            s.text(right.saturating_sub(2 * cw), first.rect.y, "^", sc, DIM);
        }
        if v.more_below {
            let last = v.lines.last().map_or(first.rect.y, |l| l.rect.y);
            s.text(right.saturating_sub(2 * cw), last, "v", sc, DIM);
        }
    }
    let centred = |s: &mut Screen, y: u32, text: &str, scale: u32, c: Rgb| {
        let cwx = 8 * scale;
        let t: String = text.chars().take((w / cwx).saturating_sub(2) as usize).collect();
        s.text(w.saturating_sub(t.chars().count() as u32 * cwx) / 2, y, &t, scale, c);
    };
    if let Some(f) = &v.footer {
        centred(s, plan.countdown_y(v.lines.len() as u32), f, sc, if waiting { WARN } else { FG });
    }
    centred(s, plan.hint_y(), hint, sc, DIM);
    if let Some((x, y)) = cursor {
        s.rect(x, y, 3 * sc + 6, 3 * sc + 6, Rgb(255, 255, 255));
        s.rect(x + 1, y + 1, 3 * sc + 4, 3 * sc + 4, Rgb(0, 0, 0));
    }
    s.present();
}

/// Show the menu and wait for a choice. `waiting`: no countdown, `notice` stays on the notice line (boot assessment said stop).
/// The first level lists the items that are not `more` and a "More options" row; that row opens the second level (Esc or the
/// "Back" row returns). The result is always an index into `items`.
pub fn choose(items: &[Item], selected: usize, timeout: Option<u32>, header: &str, notice: Option<&str>, waiting: bool) -> Pick {
    let mut mode = match Screen::open().filter(|_| !crate::force_text()) {
        Some(screen) => {
            let scale = if screen.w >= 1920 { 4 } else if screen.w >= 1280 { 3 } else { 2 };
            let mouse = Mouse::open(screen.w, screen.h);
            Mode::Gfx { screen, scale, mouse }
        }
        None => {
            let (cols, rows) = text_size();
            Mode::Text { cols, rows }
        }
    };
    // With a splash on the screen the menu is drawn over it in the same background: no clear (it would flash the screen black), and
    // on a countdown the menu stays hidden until the person presses a key or touches the screen.
    let on_splash = crate::splash::active() && matches!(mode, Mode::Gfx { .. });
    let bg = crate::splash::background().filter(|_| on_splash).unwrap_or(BG);
    let plan: Option<Plan> = match &mode {
        Mode::Gfx { screen, .. } if on_splash => Some(layout::under_logo(screen.w, screen.h, crate::splash::logo_size())),
        _ => None,
    };
    let layout = match (&mode, &plan) {
        (_, Some(p)) => p.list,
        (Mode::Text { cols, rows }, _) => Layout { width: *cols as u32, height: *rows as u32, header_h: 3, footer_h: 3, row_h: 1, margin: 2 },
        (Mode::Gfx { screen, scale, .. }, _) => {
            let cw = 8 * *scale;
            Layout { width: screen.w, height: screen.h, header_h: cw * 5, footer_h: cw * 7, row_h: cw * 4, margin: cw }
        }
    };
    let levels = Levels::split(items, "More options", "Back");
    let make = |more: bool, sel: usize, countdown: Option<u32>| {
        let rows = if more { levels.more.clone() } else { levels.top.clone() };
        if waiting {
            Menu::new_waiting(rows, layout, sel, header, notice.unwrap_or(""))
        } else {
            Menu::new(rows, layout, sel, countdown, header, notice)
        }
    };
    let mut more = levels.on_more(selected);
    let mut menu = make(more, selected, if more { None } else { timeout });
    let out = st().con_out;
    // Quiet by default: on a countdown over the splash the rows stay hidden until a key or a touch. Not on install media: they are recognised by their
    // "install" entry (boot.cfg lists it only when EFI/iyakki-boot/install-media exists), and there the menu is the point, so it is shown at once.
    let install_media = items.iter().any(|i| i.id == "install" && !i.hidden);
    let mut hidden = on_splash && !waiting && timeout.is_some() && !install_media;
    unsafe {
        ((*out).enable_cursor)(out, 0);
        ((*out).set_attribute)(out, 0x07);
        if !on_splash {
            ((*out).clear_screen)(out);
        }
    }
    say!("menu: open{}{}\n", if waiting { " (waiting, no timeout)" } else { "" }, if plan.is_some() { " (under the logo)" } else { "" });

    let mut timer: Event = null_mut();
    unsafe {
        (bs().create_event)(EVT_TIMER, TPL_CALLBACK, None, null_mut(), &mut timer);
        (bs().set_timer)(timer, TimerDelay::Periodic, TICK_MS as u64 * 10_000);
    }
    let key_ev = unsafe { (*st().con_in).wait_for_key };
    let mut events: Vec<Event> = alloc::vec![key_ev, timer];
    if let Mode::Gfx { mouse: Some(m), .. } = &mode {
        events.extend(m.devices.iter().map(|d| unsafe { (*d.0).wait_for_input }));
    }

    let mut last: Option<(View, Option<(i64, i64)>, bool)> = None;
    let pick = 'outer: loop {
        let view = menu.view();
        let cur = match &mode {
            Mode::Gfx { mouse: Some(m), .. } => Some((m.x, m.y)),
            _ => None,
        };
        if !hidden && last.as_ref().is_none_or(|(v, c, m)| *v != view || *c != cur || *m != more) {
            let hint = if more { "Up/Down choose   Enter start   Esc back" } else { "Up/Down choose   Enter start" };
            match &mut mode {
                Mode::Text { cols, rows } => draw_text(&view, *cols, *rows, waiting),
                Mode::Gfx { screen, scale, .. } => match &plan {
                    Some(p) => draw_under_logo(screen, p, &view, waiting, cur.map(|(x, y)| (x as u32, y as u32)), bg, hint),
                    None => draw_gfx(screen, *scale, &view, waiting, cur.map(|(x, y)| (x as u32, y as u32)), bg),
                },
            }
            last = Some((view, cur, more));
        }
        let mut idx = 0usize;
        unsafe { (bs().wait_for_event)(events.len(), events.as_mut_ptr(), &mut idx) };
        let mut inputs: Vec<Input> = Vec::new();
        if idx == 1 {
            inputs.push(Input::Tick { ms: TICK_MS });
        }
        while let Some(k) = read_key() {
            if hidden {
                // the first key only shows the menu (it is not also an action)
                hidden = false;
                crate::splash::spent(u32::MAX);
                inputs.push(Input::Wake); // the countdown stops: a person who pressed a key is choosing
                say!("menu: shown by a key\n");
                continue;
            }
            if let Some(i) = key_input(&k) {
                inputs.push(i);
            }
        }
        if let Mode::Gfx { screen, mouse: Some(m), .. } = &mut mode {
            if let Some((x, y)) = m.poll(screen.w, screen.h) {
                say!("menu: tap {x} {y}\n");
                if hidden {
                    hidden = false;
                    crate::splash::spent(u32::MAX);
                    inputs.push(Input::Wake);
                } else {
                    inputs.push(Input::Tap { x, y });
                }
            }
        }
        for i in inputs {
            let tick = matches!(i, Input::Tick { .. });
            match menu.handle(i) {
                MenuEvent::Boot(MORE_ROW) => {
                    more = true;
                    menu = make(true, levels.more.get(1).map_or(BACK_ROW, |r| r.item), None);
                    say!("menu: more options\n");
                }
                MenuEvent::Boot(BACK_ROW) => {
                    more = false;
                    menu = make(false, MORE_ROW, None);
                    say!("menu: back\n");
                }
                MenuEvent::Boot(item) => break 'outer Pick { item, timed_out: tick },
                MenuEvent::Cancel if more => {
                    more = false;
                    menu = make(false, MORE_ROW, None);
                    say!("menu: back\n");
                }
                MenuEvent::Cancel | MenuEvent::None => {}
            }
        }
    };
    unsafe {
        (bs().set_timer)(timer, TimerDelay::Cancel, 0);
        (bs().close_event)(timer);
        ((*out).set_attribute)(out, 0x07);
        if !on_splash {
            ((*out).clear_screen)(out);
        }
    }
    if on_splash && last.is_some() {
        crate::splash::repaint(); // the menu covered the picture: put it back before the kernel starts
    }
    pick
}

/// Wait for a key for at most `ms` milliseconds; true if one arrived.
pub fn wait_key(ms: usize) -> bool {
    let mut left = ms;
    while left > 0 {
        if read_key().is_some() {
            return true;
        }
        crate::sys::stall_us(10_000);
        left = left.saturating_sub(10);
    }
    false
}

