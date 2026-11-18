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

pub trait TerminalPort {
    fn open(&mut self) -> Result<Opened, TermError>;
    fn run(&mut self, line: &str) -> Result<Reply, TermError>;
    fn decide(&mut self, call: &str, approve: bool) -> Result<Reply, TermError>;
    fn close(&mut self);
}

/// An in-memory terminal: a read-only command (`echo`, `pwd`, `ls`, `clear`, `whoami`) answers; anything else asks first and, when
/// allowed, answers `done: <line>`. Lines run are kept in `ran`.
#[derive(Default)]
pub struct MemTerminal {
    off: bool,
    open: bool,
    waiting: Option<(String, String)>,
    n: u32,
    pub ran: VecDeque<String>,
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

impl TerminalPort for MemTerminal {
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
