//! The terminal port: what an SDK app is handed so that it can embed a command line (see [`crate::terminalview::TerminalView`]). The port is
//! the core's `Terminal` facet seen from the app: one session of the IYAKKI command line, checked by the person's policy; a line that
//! changes something comes back with a question the person must answer (the core asks, the app only shows the card). [`MemTerminal`] is the
//! in-memory port for tests and the browser preview, following the same rules.

use std::collections::VecDeque;

pub const MAX_LINE: usize = 4096;
pub const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub prompt: String,
    pub banner: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub call: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub prompt: String,
    pub clear: bool,
    pub cut: bool,
    pub question: Option<Question>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermError {
    Refused(String),
    Unavailable,
}

impl TermError {
    pub fn sentence(&self) -> String {
        match self {
            TermError::Refused(s) => s.clone(),
            TermError::Unavailable => "The terminal is not available here.".into(),
        }
    }
}

/// The colour of a cell of a whole-session screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenColor {
    #[default]
    Default,
    /// 0 to 15 the usual sixteen, 16 to 255 the xterm cube and grey ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One character cell. `attrs` bits: 1 bold, 2 dim, 4 italic, 8 underline, 16 blink, 32 reverse, 64 hidden, 128 strike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenCell {
    pub ch: char,
    pub fg: ScreenColor,
    pub bg: ScreenColor,
    pub attrs: u8,
    /// The right half of a double-width character: nothing is drawn here.
    pub tail: bool,
}

impl Default for ScreenCell {
    fn default() -> ScreenCell {
        ScreenCell { ch: ' ', fg: ScreenColor::Default, bg: ScreenColor::Default, attrs: 0, tail: false }
    }
}

/// The screen of a whole session as the core keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<ScreenCell>>,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub back: usize,
    pub history: usize,
    pub exited: Option<i32>,
}

impl Frame {
    /// The screen as text, one line per row, trailing blanks removed.
    pub fn text(&self) -> String {
        self.cells.iter().map(|r| r.iter().filter(|c| !c.tail).map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }
}

/// A key sent to a whole session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKey {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

pub trait TerminalPort {
    fn open(&mut self) -> Result<Opened, TermError>;
    fn run(&mut self, line: &str) -> Result<Reply, TermError>;
    fn decide(&mut self, call: &str, approve: bool) -> Result<Reply, TermError>;
    fn close(&mut self);

    // The whole-session terminal (a real program on a pseudo-terminal: `vi`, `top`, `ssh`). Nothing starts before the person's yes.
    /// Ask for a session of `cols` x `rows`; the answer is the question the person must answer.
    fn screen_open(&mut self, _cols: usize, _rows: usize) -> Result<Question, TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_decide(&mut self, _call: &str, _approve: bool) -> Result<(), TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_key(&mut self, _key: ScreenKey, _ctrl: bool, _alt: bool) -> Result<(), TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_text(&mut self, _text: &str) -> Result<(), TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_resize(&mut self, _cols: usize, _rows: usize) -> Result<(), TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_scroll(&mut self, _lines: isize) -> Result<(), TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_frame(&mut self) -> Result<Frame, TermError> {
        Err(TermError::Unavailable)
    }
    fn screen_close(&mut self) {}
}

/// An in-memory terminal: a read-only command (`echo`, `pwd`, `ls`, `clear`, `whoami`) answers; anything else asks first and, when
/// allowed, answers `done: <line>`. Lines run are kept in `ran`.
#[derive(Default)]
pub struct MemTerminal {
    scr: Option<MemScreen>,
    /// What the whole session received, for tests: keys as `key:...` and text as `text:...`, a resize as `size:CxR`.
    pub screen_log: Vec<String>,
    off: bool,
    open: bool,
    waiting: Option<(String, String)>,
    n: u32,
    pub ran: VecDeque<String>,
}

struct MemScreen {
    call: Option<String>,
    size: (usize, usize),
    typed: String,
    lines: Vec<String>,
    seq: u64,
    back: usize,
}

const READ_ONLY: &[&str] = &["echo", "pwd", "ls", "clear", "whoami", "cat", "help"];

impl MemTerminal {
    pub fn new() -> MemTerminal {
        MemTerminal::default()
    }
    pub fn unavailable() -> MemTerminal {
        MemTerminal { off: true, ..MemTerminal::default() }
    }
}

impl MemTerminal {
    fn screen(&mut self) -> Result<&mut MemScreen, TermError> {
        match self.scr.as_mut() {
            None => Err(TermError::Refused("that terminal is not open".into())),
            Some(m) if m.call.is_some() => Err(TermError::Refused("the terminal waits for your yes".into())),
            Some(m) => Ok(m),
        }
    }
}

impl TerminalPort for MemTerminal {
    fn screen_open(&mut self, cols: usize, rows: usize) -> Result<Question, TermError> {
        if self.off {
            return Err(TermError::Unavailable);
        }
        if !(1..=300).contains(&cols) || !(1..=100).contains(&rows) {
            return Err(TermError::Refused("a terminal is 1 to 300 columns by 1 to 100 rows".into()));
        }
        self.n += 1;
        let call = format!("pc{}", self.n);
        self.scr = Some(MemScreen { call: Some(call.clone()), size: (cols, rows), typed: String::new(), lines: vec!["mem$ ".into()], seq: 1, back: 0 });
        Ok(Question { call, summary: "Open a full terminal? Programs in it run as you and are not checked line by line; what you type is not recorded.".into() })
    }

    fn screen_decide(&mut self, call: &str, approve: bool) -> Result<(), TermError> {
        match self.scr.as_mut() {
            Some(m) if m.call.as_deref() == Some(call) => {
                m.call = None;
                if !approve {
                    self.scr = None;
                }
                Ok(())
            }
            _ => Err(TermError::Refused("there is no such question waiting".into())),
        }
    }

    fn screen_key(&mut self, key: ScreenKey, ctrl: bool, _alt: bool) -> Result<(), TermError> {
        self.screen()?;
        self.screen_log.push(format!("key:{key:?}{}", if ctrl { "+ctrl" } else { "" }));
        let m = self.screen()?;
        match key {
            ScreenKey::Char(_) if ctrl => {}
            ScreenKey::Char(c) => m.typed.push(c),
            ScreenKey::Backspace => {
                m.typed.pop();
            }
            ScreenKey::Enter => {
                let line = std::mem::take(&mut m.typed);
                if let Some(l) = m.lines.last_mut() {
                    l.push_str(&line);
                }
                m.lines.push(format!("ran: {line}"));
                m.lines.push("mem$ ".into());
            }
            _ => {}
        }
        m.seq += 1;
        Ok(())
    }

    fn screen_text(&mut self, text: &str) -> Result<(), TermError> {
        self.screen()?;
        self.screen_log.push(format!("text:{text}"));
        let m = self.screen()?;
        m.typed.push_str(text);
        m.seq += 1;
        Ok(())
    }

    fn screen_resize(&mut self, cols: usize, rows: usize) -> Result<(), TermError> {
        self.screen()?;
        self.screen_log.push(format!("size:{cols}x{rows}"));
        let m = self.screen()?;
        m.size = (cols.clamp(1, 300), rows.clamp(1, 100));
        m.seq += 1;
        Ok(())
    }

    fn screen_scroll(&mut self, lines: isize) -> Result<(), TermError> {
        let m = self.screen()?;
        m.back = (m.back as isize + lines).clamp(0, m.lines.len() as isize) as usize;
        m.seq += 1;
        Ok(())
    }

    fn screen_frame(&mut self) -> Result<Frame, TermError> {
        let m = self.screen()?;
        let (cols, rows) = m.size;
        let end = m.lines.len().saturating_sub(m.back);
        let start = end.saturating_sub(rows);
        let mut cells: Vec<Vec<ScreenCell>> = m.lines[start..end].iter().enumerate().map(|(i, l)| {
            let mut row: Vec<ScreenCell> = l.chars().take(cols).map(|ch| ScreenCell { ch, ..ScreenCell::default() }).collect();
            if i + 1 == end - start && m.back == 0 {
                let room = cols.saturating_sub(row.len());
                row.extend(m.typed.chars().take(room).map(|ch| ScreenCell { ch, ..ScreenCell::default() }));
            }
            row.resize(cols, ScreenCell::default());
            row
        }).collect();
        cells.resize(rows, vec![ScreenCell::default(); cols]);
        let x = m.lines.get(end.wrapping_sub(1)).map_or(0, |l| l.chars().count() + m.typed.chars().count()).min(cols - 1);
        Ok(Frame { seq: m.seq, cols, rows, cells, cursor: (x, (end - start).saturating_sub(1)), cursor_visible: m.back == 0, back: m.back, history: m.lines.len().saturating_sub(rows), exited: None })
    }

    fn screen_close(&mut self) {
        self.scr = None;
    }

    fn open(&mut self) -> Result<Opened, TermError> {
        if self.off {
            return Err(TermError::Unavailable);
        }
        self.open = true;
        Ok(Opened { prompt: "$".into(), banner: "Memory terminal.".into() })
    }

    fn run(&mut self, line: &str) -> Result<Reply, TermError> {
        if self.off {
            return Err(TermError::Unavailable);
        }
        if !self.open {
            return Err(TermError::Refused("that terminal is not open".into()));
        }
        if line.trim().is_empty() || line.len() > MAX_LINE || line.chars().any(char::is_control) {
            return Err(TermError::Refused("a command line is one line without control characters".into()));
        }
        if self.waiting.is_some() {
            return Err(TermError::Refused("answer the waiting question first".into()));
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if READ_ONLY.contains(&first) && !line.contains('>') && !line.contains('|') {
            self.ran.push_back(line.to_string());
            let text = match first {
                "echo" => line.trim_start_matches("echo").trim().to_string(),
                "clear" => String::new(),
                _ => format!("ran: {line}"),
            };
            return Ok(Reply { text, prompt: "$".into(), clear: first == "clear", ..Reply::default() });
        }
        self.n += 1;
        let call = format!("c{}", self.n);
        self.waiting = Some((call.clone(), line.to_string()));
        Ok(Reply { prompt: "$".into(), question: Some(Question { call, summary: format!("Run in your terminal: {line}") }), ..Reply::default() })
    }

    fn decide(&mut self, call: &str, approve: bool) -> Result<Reply, TermError> {
        let Some((c, line)) = self.waiting.clone().filter(|(c, _)| c == call) else { return Err(TermError::Refused("there is no such question waiting".into())) };
        let _ = c;
        self.waiting = None;
        if !approve {
            return Ok(Reply { text: "Not run: you said no.".into(), prompt: "$".into(), ..Reply::default() });
        }
        self.ran.push_back(line.clone());
        Ok(Reply { text: format!("done: {line}"), prompt: "$".into(), ..Reply::default() })
    }

    fn close(&mut self) {
        self.open = false;
        self.waiting = None;
    }
}
