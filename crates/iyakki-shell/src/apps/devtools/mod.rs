//! DEV TOOLS: thirteen small tools for developers on one tile. A list of all of them stays on the left (it shrinks on a narrow window,
//! it never hides); the chosen tool's page is on the right: an input box, sometimes a second box, buttons, and an output that
//! scrolls (wheel, trackpad, touch drag, scroll bar, PageUp / PageDown). Everything is also reachable by keyboard.
//!
//! Keys: Up / Down choose a tool while the list has the focus; Tab and Shift+Tab move between the list, the boxes and the buttons;
//! Ctrl+Enter runs the first button from a box; in the buttons Left / Right choose, Enter or Space presses, Up / Down / PageUp /
//! PageDown scroll the output; Esc steps back to the list, then out.
//!
//! A very large document does not fit the box (the clipboard keeps 64 KB): type `@/path/to/file` in the input box and the tool reads
//! that file (if the account may read it), up to 64 MB. The tools themselves are plain functions in the sibling modules.

pub mod b64;
pub mod convert;
pub mod diff;
pub mod hashes;
pub mod json;
pub mod jwt;
pub mod net;
mod page;
pub mod qrcode;
pub mod query;
pub mod regex;
pub mod tools;
pub mod url;
pub mod uuid_time;

use std::sync::Arc;

use super::multiline::MultiLine;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Icon};
use crate::scrolllist::ScrollList;
use crate::ui::{clipboard, Event, Key};
use tools::{Ctx, Kind, TOOLS};

pub const ACCENT: [u8; 4] = rgb(90, 150, 200);

const T_TOOL: usize = 1000;
const T_ACT: usize = 100;
const T_PASTE: usize = 10;
const T_COPY: usize = 11;
const T_CLEAR: usize = 12;
const T_MORE: usize = 13;
const T_STOP: usize = 14;
const T_FLAG: usize = 15;
/// The QR code page: a kind of code (eight ids from here), the error-correction level, the Wi-Fi security and hidden choices, the two saves.
const T_QR_KIND: usize = 200;
const T_QR_LEVEL: usize = 210;
const T_QR_SEC: usize = 211;
const T_QR_HIDDEN: usize = 212;
const T_SAVE_PNG: usize = 213;
const T_SAVE_SVG: usize = 214;
/// On a short window the eight kinds are one button that goes to the next kind.
const T_QR_NEXT: usize = 215;
const A_INPUT: usize = 61_500;
const A_AUX: usize = 61_501;
const A_BAR: usize = 61_502;
const A_OUT: usize = 61_503;

/// The most characters typed or pasted into a box.
const BOX_MAX: usize = 200_000;
/// The biggest file `@/path` reads.
const FILE_MAX: u64 = 64 * 1024 * 1024;
/// Lines of output drawn at first, and how many Show more adds.
const SHOW_LINES: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    List,
    Input,
    Aux,
    Buttons,
}

/// What the last button wrote.
struct Out {
    pub text: String,
    pub err: bool,
    pub total: usize,
    pub shown: usize,
    pub ver: u64,
}

pub struct DevToolsApp {
    tool: usize,
    focus: Focus,
    input: MultiLine,
    aux: MultiLine,
    /// The second box's text per tool (switching tools keeps what was typed).
    auxes: Vec<String>,
    flags: Vec<bool>,
    out: Out,
    job: Option<Arc<net::Job>>,
    scroll: ScrollList,
    /// The button the keyboard is on.
    btn: usize,
    limit: usize,
    last_act: Option<usize>,
    redraw: bool,
    vis: page::Visual,
    qr: qrcode::QrState,
    /// The window was short at the last drawing (the QR page then keeps its controls small).
    short: bool,
    /// The home folder the QR pictures are saved under (`Pictures`); the person's own unless a test says otherwise.
    qr_home: Option<std::path::PathBuf>,
}

impl DevToolsApp {
    pub fn new(_cx: &AppCx) -> DevToolsApp {
        let mut input = MultiLine::new(BOX_MAX);
        input.min_rows = 2;
        let mut aux = MultiLine::new(BOX_MAX);
        aux.enter_newline = true;
        DevToolsApp {
            tool: 0,
            focus: Focus::Input,
            input,
            aux,
            auxes: vec![String::new(); TOOLS.len()],
            flags: vec![false; TOOLS.len()],
            out: Out { text: String::new(), err: false, total: 0, shown: SHOW_LINES, ver: 0 },
            job: None,
            scroll: ScrollList::default(),
            btn: 0,
            limit: 200,
            last_act: None,
            redraw: true,
            vis: page::Visual::default(),
            qr: qrcode::QrState::default(),
            short: false,
            qr_home: None,
        }
    }

    fn select(&mut self, i: usize) {
        let i = i.min(TOOLS.len() - 1);
        if i == self.tool {
            return;
        }
        self.auxes[self.tool] = self.aux.text.clone();
        self.stop_job();
        self.tool = i;
        let a = std::mem::take(&mut self.auxes[i]);
        self.aux.set(&a);
        self.btn = 0;
        self.limit = 200;
        self.last_act = None;
        self.set_out(String::new(), false);
    }

    fn stop_job(&mut self) {
        if let Some(j) = self.job.take() {
            j.stop();
        }
    }

    fn set_out(&mut self, text: String, err: bool) {
        self.out.total = text.lines().count();
        self.out.text = text;
        self.out.err = err;
        self.out.shown = SHOW_LINES;
        self.out.ver += 1;
        self.scroll.to_top();
    }

    /// The input box's text, or the file it names (`@/path`) when the account may read it.
    fn input_text(&self, cx: &AppCx) -> Result<String, String> {
        let t = &self.input.text;
        let Some(path) = t.trim().strip_prefix('@').filter(|p| p.starts_with('/') && !p.contains('\n')) else { return Ok(t.clone()) };
        if !cx.session.policy.allows_read(&cx.session.user, path) {
            return Err(format!("This account may not read {path}."));
        }
        let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
        if !meta.is_file() || meta.len() > FILE_MAX {
            return Err(format!("{path} is not a file of at most {} MB.", FILE_MAX / 1024 / 1024));
        }
        let b = std::fs::read(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
        String::from_utf8(b).map_err(|_| format!("{path} is not UTF-8 text."))
    }

    fn run(&mut self, act: usize, cx: &AppCx) {
        let tool = &TOOLS[self.tool];
        let Some((_, f)) = tool.actions.get(act) else { return };
        self.last_act = Some(act);
        self.stop_job();
        let input = match self.input_text(cx) {
            Ok(t) => t,
            Err(e) => return self.set_out(e, true),
        };
        match tool.kind {
            Kind::Ping | Kind::Ports => {
                let started = if tool.kind == Kind::Ping { net::start_ping(&input, [5, 10][act.min(1)]) } else { net::start_ports(&input, &self.aux.text) };
                match started {
                    Ok(j) => {
                        self.set_out(j.text(), false);
                        self.job = Some(j);
                    }
                    Err(e) => self.set_out(e, true),
                }
            }
            Kind::Qr => self.press(T_COPY, cx),
            Kind::Text => {
                let ctx = Ctx { input: &input, aux: &self.aux.text, flag: self.flags[self.tool], limit: self.limit };
                match f(&ctx) {
                    Ok(t) => self.set_out(t, false),
                    Err(e) => self.set_out(e, true),
                }
            }
        }
    }

    /// The buttons of the page in order: (label, event id). The first ones run the tool.
    fn buttons(&self) -> Vec<(String, usize)> {
        let t = &TOOLS[self.tool];
        let mut v = Vec::new();
        if t.kind == Kind::Qr {
            if self.short {
                v.push((format!("Kind: {}", qrcode::PRESETS[self.qr.preset].name), T_QR_NEXT));
            } else {
                v.extend(qrcode::PRESETS.iter().enumerate().map(|(i, p)| (p.name.to_string(), T_QR_KIND + i)));
            }
            v.push((format!("Error correction: {}", self.qr.level.letter()), T_QR_LEVEL));
            if self.qr.preset == qrcode::WIFI {
                v.push((format!("Security: {}", qrcode::security_name(self.qr.security)), T_QR_SEC));
                v.push((format!("Hidden network: {}", if self.qr.hidden { "yes" } else { "no" }), T_QR_HIDDEN));
            }
            v.extend([("Copy text".to_string(), T_COPY), ("Save as PNG".into(), T_SAVE_PNG), ("Save as SVG".into(), T_SAVE_SVG), ("Paste into input".into(), T_PASTE), ("Clear".into(), T_CLEAR)]);
            return v;
        }
        if self.job.is_some() {
            v.push(("Stop".to_string(), T_STOP));
        } else {
            v.extend(t.actions.iter().enumerate().map(|(i, (l, _))| (l.to_string(), T_ACT + i)));
        }
        if let Some(l) = t.toggle {
            v.push((format!("{l}: {}", if self.flags[self.tool] { "on" } else { "off" }), T_FLAG));
        }
        v.push(("Paste into input".into(), T_PASTE));
        v.push(("Copy output".into(), T_COPY));
        v.push(("Clear".into(), T_CLEAR));
        if self.more_available() {
            v.push(("Show more".into(), T_MORE));
        }
        v
    }

    fn more_available(&self) -> bool {
        self.out.shown < self.out.total || (self.tool == tools::QUERY && self.out.text.contains("more not shown"))
    }

    fn press(&mut self, id: usize, cx: &AppCx) {
        match id {
            T_PASTE => match clipboard::paste() {
                Some(t) => {
                    self.input.set(&t);
                    self.focus = Focus::Input;
                }
                None => self.set_out("The clipboard has no text to paste.".into(), true),
            },
            T_COPY if TOOLS[self.tool].kind == Kind::Qr => {
                let typed = self.input.text.clone();
                let (payload, status) = {
                    let m = self.qr.made(&typed);
                    (m.payload.clone(), m.status.clone())
                };
                if payload.is_empty() {
                    self.qr.note = Some((status, true));
                } else {
                    clipboard::copy(&payload);
                    self.qr.note = Some(("Copied the text of the code.".into(), false));
                }
            }
            T_SAVE_PNG => self.save_qr("png", cx),
            T_SAVE_SVG => self.save_qr("svg", cx),
            T_QR_LEVEL => self.qr.level = qrcode::next_level(self.qr.level),
            T_QR_SEC => self.qr.security = qrcode::next_security(self.qr.security),
            T_QR_HIDDEN => self.qr.hidden = !self.qr.hidden,
            T_QR_NEXT => self.press(T_QR_KIND + (self.qr.preset + 1) % qrcode::PRESETS.len(), cx),
            n if (T_QR_KIND..T_QR_KIND + qrcode::PRESETS.len()).contains(&n) => {
                self.qr.texts[self.qr.preset] = self.input.text.clone();
                self.qr.preset = n - T_QR_KIND;
                let t = std::mem::take(&mut self.qr.texts[self.qr.preset]);
                self.input.set(&t);
                self.qr.note = None;
            }
            T_COPY => {
                if self.out.text.is_empty() {
                    return;
                }
                clipboard::copy(&self.out.text);
            }
            T_CLEAR => {
                self.stop_job();
                self.input.set("");
                self.aux.set("");
                self.set_out(String::new(), false);
                self.qr.note = None;
                self.qr.texts.iter_mut().for_each(String::clear);
                self.focus = Focus::Input;
            }
            T_MORE => {
                if self.tool == tools::QUERY && self.out.shown >= self.out.total {
                    self.limit *= 4;
                    if let Some(a) = self.last_act {
                        let top = self.scroll.offset;
                        self.run(a, cx);
                        self.scroll.offset = top;
                    }
                } else {
                    self.out.shown += SHOW_LINES;
                    self.out.ver += 1;
                }
            }
            T_STOP => {
                if let Some(j) = &self.job {
                    j.stop();
                }
            }
            T_FLAG => self.flags[self.tool] = !self.flags[self.tool],
            n if (T_ACT..T_ACT + 20).contains(&n) => self.run(n - T_ACT, cx),
            _ => {}
        }
    }

    /// Save the QR code as a picture (`png` or `svg`) in the person's Pictures folder and say where it went.
    fn save_qr(&mut self, ext: &str, cx: &AppCx) {
        let typed = self.input.text.clone();
        let (code, status, err) = {
            let m = self.qr.made(&typed);
            (m.qr.clone(), m.status.clone(), m.err)
        };
        let Some(code) = code else {
            self.qr.note = Some((if err { status } else { "There is no code to save yet: type the text first.".into() }, true));
            return;
        };
        let bytes = if ext == "png" {
            match qrcode::png(&code) {
                Ok(b) => b,
                Err(e) => {
                    self.qr.note = Some((format!("Could not make the PNG: {e}"), true));
                    return;
                }
            }
        } else {
            qrcode::svg(&code).into_bytes()
        };
        let home = self.qr_home.clone().unwrap_or_else(|| std::path::PathBuf::from(format!("/home/{}", cx.session.user)));
        let dir = home.join("Pictures");
        let name = qrcode::file_name(ext);
        self.qr.note = Some(match super::chatpic::save_file(&cx.session.policy, &cx.session.user, &dir, &name, &bytes) {
            Ok(path) => {
                crate::camera::own::give_saved(&path);
                // the file name only: what the code holds (a Wi-Fi password, say) is not written to the log
                cx.audit.record(&cx.session.user, "devtools.qr.save", &path.display().to_string());
                (format!("Saved {} in {}.", path.file_name().map_or(name.clone(), |n| n.to_string_lossy().into_owned()), path.parent().unwrap_or(&dir).display()), false)
            }
            Err(e) => (format!("Could not save the picture: {e}"), true),
        });
    }

    /// Take what a running check has written so far.
    fn poll_job(&mut self) -> bool {
        let Some(j) = &self.job else { return false };
        let t = j.text();
        let done = j.finished();
        let changed = t != self.out.text;
        if changed {
            let keep = self.scroll.offset;
            self.set_out(t, false);
            self.scroll.offset = keep;
        }
        if done {
            self.job = None;
        }
        changed || done
    }

    fn cycle(&mut self, back: bool) {
        let has_aux = TOOLS[self.tool].aux.is_some();
        let order: Vec<Focus> = [Focus::List, Focus::Input].into_iter().chain(has_aux.then_some(Focus::Aux)).chain([Focus::Buttons]).collect();
        let at = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = order[if back { (at + order.len() - 1) % order.len() } else { (at + 1) % order.len() }];
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        match k {
            Key::Tab => self.cycle(false),
            Key::BackTab => self.cycle(true),
            Key::Esc => {
                if self.focus == Focus::List {
                    return Flow::Back;
                }
                self.focus = Focus::List;
            }
            Key::Ctrl('\n') | Key::Ctrl('\r') | Key::F(5) => self.run(0, cx),
            Key::PageUp | Key::PageDown | Key::End | Key::Home if matches!(self.focus, Focus::List | Focus::Buttons) => {
                self.scroll.key(k);
            }
            _ => match self.focus {
                Focus::List => match k {
                    Key::Up => self.select(self.tool.saturating_sub(1)),
                    Key::Down => self.select(self.tool + 1),
                    Key::Enter | Key::Right => self.focus = Focus::Input,
                    _ => {}
                },
                Focus::Input => {
                    self.input.key(k);
                }
                Focus::Aux => {
                    self.aux.key(k);
                }
                Focus::Buttons => {
                    let b = self.buttons();
                    match k {
                        Key::Left => self.btn = self.btn.saturating_sub(1),
                        Key::Right => self.btn = (self.btn + 1).min(b.len() - 1),
                        Key::Up => self.scroll.by(-self.scroll.row * 2.0),
                        Key::Down => self.scroll.by(self.scroll.row * 2.0),
                        Key::Enter | Key::Char(' ') => {
                            if let Some((_, id)) = b.get(self.btn) {
                                self.press(*id, cx);
                            }
                        }
                        _ => {}
                    }
                }
            },
        }
        Flow::Stay
    }
}

impl App for DevToolsApp {
    fn title(&self) -> &'static str {
        "Developer tools"
    }
    fn page(&self) -> String {
        TOOLS[self.tool].name.to_lowercase().replace(' ', "-")
    }
    fn icon(&self) -> Icon {
        Icon::Edit
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "↑↓ tool · Tab next box · Ctrl+Enter run · wheel or drag scrolls · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "DEV TOOLS has thirteen tools in the list on the left: JSON (format, minify, validate, sort keys), JSON query, Base64, JWT, URL, Hash, UUID and time, Regex, Diff, Converters, Ping, Port checker and QR code. Click, tap or press Up / Down to choose one.",
            "Type or paste into the input box, press a button (or Ctrl+Enter for the first one) and read the output below. Paste into input takes the clipboard, Copy output puts the output on it (the clipboard keeps 64 KB), Clear empties the boxes.",
            "The output scrolls with the wheel, the trackpad, a touch drag, its scroll bar, or PageUp / PageDown. Long results show the first part; Show more adds the next.",
            "Tab and Shift+Tab move between the list, the boxes and the buttons; in the buttons Left / Right choose and Enter or Space presses. Esc goes back to the list, then leaves.",
            "For a document too big for the box, type @/path/to/file in the input box: the tool reads that file (up to 64 MB) if your account may read it.",
            "QR code: choose the kind of code (text, web address, Wi-Fi, phone, e-mail, SMS, location, contact), type into the box (more than one line where the label says so) and the code is drawn at once, black on white so a phone can scan it. Error correction L to M to Q to H lets more of the code be damaged and still read, and makes it bigger. Copy text puts the text of the code on the clipboard; Save as PNG and Save as SVG write a picture into your Pictures folder and say where. A Wi-Fi code holds the password in plain form: anyone who scans it can join the network.",
            "JWT: the signature is not checked unless you type a secret and press Verify HS256. Ping and Port checker only contact the host you type; ports are limited to 256 per check and the check runs in the background (Stop ends it).",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.redraw = true;
        match ev {
            Event::Key(k) => return self.key(k, cx),
            Event::Tab(n) if *n >= T_TOOL => {
                self.select(n - T_TOOL);
                self.focus = Focus::Input;
            }
            Event::Tab(n) => {
                self.press(*n, cx);
                if let Some(i) = self.buttons().iter().position(|(_, id)| id == n) {
                    self.btn = i;
                }
            }
            Event::AreaAt(A_INPUT, x, y) => {
                self.focus = Focus::Input;
                self.input.press(*x, *y);
            }
            Event::AreaAt(A_AUX, x, y) => {
                self.focus = Focus::Aux;
                self.aux.press(*x, *y);
            }
            Event::AreaAt(A_OUT, ..) => self.focus = Focus::Buttons,
            Event::Paste(t) => {
                match self.focus {
                    Focus::Aux => self.aux.insert(t),
                    _ => self.input.insert(t),
                };
            }
            Event::Scroll(d) if self.input.wheel(*d) || self.aux.wheel(*d) => {}
            other => {
                self.scroll.event(other, A_BAR);
            }
        }
        Flow::Stay
    }

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.scroll.wheel_px(px)
    }

    fn wants_redraw(&mut self) -> bool {
        let moved = self.poll_job();
        moved || std::mem::take(&mut self.redraw)
    }

    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, _cx: &AppCx, tick: u64) {
        self.draw(g, area, s, tick);
    }
}

#[cfg(test)]
mod tests;
