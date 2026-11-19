//! The input lines a browser or client sends back, and the session that applies them. The renderer (`ui/displaylist.js`) writes exactly
//! these lines; a [`Session`] reads them, drives the app through the same [`Host`] every target uses, and answers with the next frame
//! as display list text. Plain text, one event per line, additive like the display list: an unknown line is an error that names it.
//!
//! ```text
//! ptr ID PHASE X Y T       pointer (PHASE is down, move, up or cancel; T in milliseconds)
//! key NAME                 enter backspace delete esc tab up down left right
//! key char C               one typed character (C escaped like the text of a display list)
//! text S                   typed, pasted or composed text for the focused field
//! action N                 a hit region was used (a screen reader's click, Enter on a focused control)
//! wheel N                  positive = towards older content
//! resize W H               the pane changed size (logical units)
//! tick T                   the heartbeat (T in milliseconds)
//! ```

use crate::app::{Flow, UiApp};
use crate::event::{Phase, Pointer, UiEvent, UiKey};
use crate::host::Host;
use crate::list::{esc, unesc};

/// Limits of one batch of input (a reader refuses a bigger one).
pub const MAX_LINES: usize = 256;
pub const MAX_LINE: usize = 4096;
/// A pane the browser may ask for: at least 100 and at most 4096 logical units each way.
pub const MIN_PANE: f32 = 100.0;
pub const MAX_PANE: f32 = 4096.0;

/// What one input line means to the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Event(UiEvent),
    Pointer(Pointer),
    Resize(f32, f32),
    Tick(u64),
}

fn key_name(k: UiKey) -> String {
    match k {
        UiKey::Char(c) => format!("char {}", esc(&c.to_string())),
        UiKey::Enter => "enter".into(),
        UiKey::Backspace => "backspace".into(),
        UiKey::Delete => "delete".into(),
        UiKey::Esc => "esc".into(),
        UiKey::Tab => "tab".into(),
        UiKey::Up => "up".into(),
        UiKey::Down => "down".into(),
        UiKey::Left => "left".into(),
        UiKey::Right => "right".into(),
    }
}

fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::Down => "down",
        Phase::Move => "move",
        Phase::Up => "up",
        Phase::Cancel => "cancel",
    }
}

/// The line for an input (what a client in Rust sends; the JavaScript renderer writes the same text).
pub fn to_line(i: &Input) -> Option<String> {
    Some(match i {
        Input::Pointer(p) => format!("ptr {} {} {} {} {}", p.id, phase_name(p.phase), p.x, p.y, p.t_ms),
        Input::Event(UiEvent::Key(k)) => format!("key {}", key_name(*k)),
        Input::Event(UiEvent::Text(s)) => format!("text {}", esc(s)),
        Input::Event(UiEvent::Action(n)) => format!("action {n}"),
        Input::Event(UiEvent::Wheel(n)) => format!("wheel {n}"),
        Input::Resize(w, h) => format!("resize {w} {h}"),
        Input::Tick(t) => format!("tick {t}"),
        // raw gestures and Resized are made by the host itself; they never travel
        Input::Event(_) => return None,
    })
}

fn num(s: Option<&&str>, line: usize) -> Result<f32, String> {
    match s.and_then(|v| v.parse::<f32>().ok()) {
        Some(v) if v.is_finite() => Ok(v),
        _ => Err(format!("line {line}: bad number")),
    }
}

fn whole(s: Option<&&str>, line: usize) -> Result<u64, String> {
    s.and_then(|v| v.parse::<u64>().ok()).ok_or(format!("line {line}: bad number"))
}

/// Read one line; `n` is its number in the batch (for the error).
pub fn parse_line(line: &str, n: usize) -> Result<Input, String> {
    if line.len() > MAX_LINE {
        return Err(format!("line {n}: too long"));
    }
    let f: Vec<&str> = line.split(' ').collect();
    match (f[0], f.len()) {
        ("ptr", 6) => {
            let phase = match f[2] {
                "down" => Phase::Down,
                "move" => Phase::Move,
                "up" => Phase::Up,
                "cancel" => Phase::Cancel,
                _ => return Err(format!("line {n}: unknown pointer phase")),
            };
            let id = whole(f.get(1), n)?;
            Ok(Input::Pointer(Pointer { id: u32::try_from(id).map_err(|_| format!("line {n}: bad number"))?, x: num(f.get(3), n)?, y: num(f.get(4), n)?, phase, t_ms: whole(f.get(5), n)? }))
        }
        ("key", 2) => {
            let k = match f[1] {
                "enter" => UiKey::Enter,
                "backspace" => UiKey::Backspace,
                "delete" => UiKey::Delete,
                "esc" => UiKey::Esc,
                "tab" => UiKey::Tab,
                "up" => UiKey::Up,
                "down" => UiKey::Down,
                "left" => UiKey::Left,
                "right" => UiKey::Right,
                other => return Err(format!("line {n}: unknown key '{other}'")),
            };
            Ok(Input::Event(UiEvent::Key(k)))
        }
        ("key", 3) if f[1] == "char" => {
            let s = unesc(f[2], n)?;
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if !c.is_control() => Ok(Input::Event(UiEvent::Key(UiKey::Char(c)))),
                _ => Err(format!("line {n}: a char key is one printable character")),
            }
        }
        ("text", 2) => Ok(Input::Event(UiEvent::Text(unesc(f[1], n)?))),
        ("action", 2) => Ok(Input::Event(UiEvent::Action(u32::try_from(whole(f.get(1), n)?).map_err(|_| format!("line {n}: bad number"))?))),
        ("wheel", 2) => {
            let v = f[1].parse::<i32>().map_err(|_| format!("line {n}: bad number"))?;
            Ok(Input::Event(UiEvent::Wheel(v.clamp(-1000, 1000))))
        }
        ("resize", 3) => {
            let (w, h) = (num(f.get(1), n)?, num(f.get(2), n)?);
            if !(MIN_PANE..=MAX_PANE).contains(&w) || !(MIN_PANE..=MAX_PANE).contains(&h) {
                return Err(format!("line {n}: the pane must be {MIN_PANE} to {MAX_PANE} units each way"));
            }
            Ok(Input::Resize(w, h))
        }
        ("tick", 2) => Ok(Input::Tick(whole(f.get(1), n)?)),
        (other, _) => Err(format!("line {n}: unknown or malformed input '{other}'")),
    }
}

/// Read a batch; the first bad line refuses the whole batch (nothing is applied).
pub fn parse_batch(text: &str) -> Result<Vec<Input>, String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    if lines.len() > MAX_LINES {
        return Err(format!("too many input lines (at most {MAX_LINES})"));
    }
    lines.iter().enumerate().map(|(i, l)| parse_line(l, i + 1)).collect()
}

/// An app, the host that drives it, and the text in and out: one browser or client connected.
pub struct Session<A: UiApp> {
    pub host: Host<A>,
    left: bool,
}

impl<A: UiApp> Session<A> {
    pub fn new(host: Host<A>) -> Session<A> {
        Session { host, left: false }
    }

    /// The app answered `Back`: the person left it.
    pub fn left(&self) -> bool {
        self.left
    }

    /// The current frame as display list text.
    pub fn frame(&mut self) -> String {
        self.host.record().to_text()
    }

    /// Apply a batch of input lines and return the new frame. A bad batch applies nothing and says why.
    pub fn apply(&mut self, batch: &str) -> Result<String, String> {
        let inputs = parse_batch(batch)?;
        // the frame the person saw is the one taps resolve against
        if !inputs.is_empty() {
            let _ = self.host.record();
        }
        for i in inputs {
            let flow = match i {
                Input::Pointer(p) => self.host.pointer(p),
                Input::Event(e) => self.host.event(&e),
                Input::Resize(w, h) => {
                    self.host.resize(w, h);
                    Flow::Stay
                }
                Input::Tick(t) => self.host.tick(t),
            };
            if flow == Flow::Back {
                self.left = true;
            }
        }
        Ok(self.frame())
    }
}
