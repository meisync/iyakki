//! The terminal facet an app may embed (docs/development/UI-SDK.md, "Embedding the terminal"): the IYAKKI command line, the same one as
//! the CLI app, for the signed-in person. The core owns the session: it checks every line against the person's policy (files they may read
//! or write, the commands their role allows, the hosts they may reach), keeps the working folder, and asks the person before anything that
//! writes, runs a program, reaches the network or starts another session. The app sends lines and shows what comes back; it never sees a
//! process, a path outside the home folder's own names, or a key.
//!
//! A line that needs the person's yes does not run: the reply carries a [`TermApproval`] and the session waits ([`Terminal::decide`]); a new line
//! meanwhile is refused with a sentence. Switching the machine off or restarting it is never done from an embedded terminal. Every
//! implementation uses the checks below so a request that breaks a limit is refused with the same words everywhere.

use serde::{Deserialize, Serialize};

pub const TERMINAL_FACET_VERSION: u32 = 1;

pub const MAX_LINE: usize = 4096;
/// The most output one line gives back (the rest is cut, and the reply says so).
pub const MAX_OUTPUT: usize = 64 * 1024;
/// Sessions one person may have open at once.
pub const MAX_SESSIONS: usize = 4;

pub const NO_TERMINAL: &str = "this core has no terminal";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermOpen {
    pub id: String,
    pub prompt: String,
    /// A line to show first (what this terminal is, what it will not do).
    pub banner: String,
}

/// A line that waits for the person: `summary` says in plain words what will happen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermApproval {
    pub call: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermReply {
    pub text: String,
    pub prompt: String,
    /// The line asked to clear the screen.
    pub clear: bool,
    /// The output was longer than [`MAX_OUTPUT`] and was cut.
    pub cut: bool,
    /// Set: nothing ran; the person must decide.
    pub approval: Option<TermApproval>,
}

/// A line that fits: not empty, at most [`MAX_LINE`] characters, one line, no control characters.
pub fn check_line(line: &str) -> Result<(), String> {
    if line.trim().is_empty() {
        return Err("the line is empty".into());
    }
    if line.len() > MAX_LINE || line.chars().any(char::is_control) {
        return Err(format!("a command line is one line of at most {MAX_LINE} characters without control characters"));
    }
    Ok(())
}

/// The names of the commands an embedded terminal runs without asking (they only read and print). Anything else asks first.
pub const READ_ONLY: &[&str] = &[
    "help", "whoami", "policy", "echo", "clear", "pwd", "cd", "ls", "cat", "grep", "sed", "tr", "cut", "sort", "uniq", "wc", "head", "tail", "tac", "rev", "nl", "seq", "date", "basename", "dirname",
];

/// What the person is told for a command that switches the machine off or restarts it.
pub const NO_POWER: &str = "this terminal does not switch the machine off or restart it; use the Power button";

/// Cut `text` to [`MAX_OUTPUT`] bytes at a character boundary; whether it was cut.
pub fn cap_output(text: &str) -> (String, bool) {
    if text.len() <= MAX_OUTPUT {
        return (text.to_string(), false);
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// The words a line's stages start with (`a | b` has two stages), for classifying it.
pub fn commands_of(line: &str) -> Vec<&str> {
    line.split('|').map(|s| s.split_whitespace().next().unwrap_or("")).filter(|c| !c.is_empty()).collect()
}

/// Whether a line may run without asking: every stage is a read-only command and there is no redirection into a file.
pub fn runs_without_asking(line: &str) -> bool {
    !line.contains('>') && commands_of(line).iter().all(|c| READ_ONLY.contains(c))
}

/// Whether a line asks to switch the machine off or restart it.
pub fn is_power(line: &str) -> bool {
    commands_of(line).iter().any(|c| matches!(*c, "poweroff" | "reboot" | "shutdown" | "halt"))
}

/// Where a person's policy comes from (the accounts), for a core that has to check a person's lines against it. The accounts' store
/// implements it; the terminal never takes a policy from the app.
#[cfg(feature = "native")]
pub trait PolicySource: Send + Sync {
    fn policy_of(&self, user: &str) -> Option<iyakki_sec::policy::Policy>;
}


// ---- the whole-session terminal (F-SDK-103) ----
//
// A real program on a pseudo-terminal (a shell, then `vi`, `top`, `ssh`), as the person's own system user. Unlike the lines above it is not
// checked line by line, so the person says yes once for the whole session ([`Terminal::pty_open`] answers with the question, nothing runs
// before [`Terminal::pty_decide`]) and what the person types in it is never recorded: the audit holds the open, the answer and the close.
// The core keeps the screen (the parser and the cell grid); the app asks for a [`PtyFrame`] and draws it.

/// Widest and tallest screen of a session, and sessions of this kind one person may have at once.
pub const PTY_MAX_COLS: usize = 300;
pub const PTY_MAX_ROWS: usize = 100;
pub const PTY_MAX_SESSIONS: usize = 2;
/// The most text one paste into a session may carry.
pub const PTY_MAX_PASTE: usize = 64 * 1024;

/// What the person is asked before a whole session starts.
pub const PTY_QUESTION: &str = "Open a full terminal? Programs in it run as you and are not checked line by line; what you type is not recorded.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PtyColor {
    #[default]
    Default,
    /// 0 to 15 the usual sixteen, 16 to 255 the xterm cube and grey ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One character cell. `attrs` bits: 1 bold, 2 dim, 4 italic, 8 underline, 16 blink, 32 reverse, 64 hidden, 128 strike. The right half of a
/// double-width character has `tail` set and is not drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyCell {
    pub ch: char,
    pub fg: PtyColor,
    pub bg: PtyColor,
    pub attrs: u8,
    pub tail: bool,
}

impl Default for PtyCell {
    fn default() -> PtyCell {
        PtyCell { ch: ' ', fg: PtyColor::Default, bg: PtyColor::Default, attrs: 0, tail: false }
    }
}

/// The screen of a session as the core sees it now. `seq` grows with every change, so a drawing layer can skip an unchanged frame.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyFrame {
    pub seq: u64,
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<PtyCell>>,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub title: String,
    /// How many lines the view is scrolled back (0 is the live screen).
    pub back: usize,
    /// Lines of history above the screen.
    pub history: usize,
    /// The program ended with this exit code (the screen stays readable until the session is closed).
    pub exited: Option<i32>,
}

impl PtyFrame {
    /// The screen as text, one line per row, trailing blanks removed (for tests, a screen reader and the chat).
    pub fn text(&self) -> String {
        self.cells.iter().map(|r| r.iter().filter(|c| !c.tail).map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }
}

/// A key a person presses in a session; the core turns it into the bytes the program expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PtyKey {
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
    /// F1 to F12.
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// A session waiting for the person's yes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOpen {
    pub id: String,
    pub approval: TermApproval,
}

/// A size that fits: 1 to [`PTY_MAX_COLS`] by 1 to [`PTY_MAX_ROWS`].
pub fn check_size(cols: usize, rows: usize) -> Result<(), String> {
    if (1..=PTY_MAX_COLS).contains(&cols) && (1..=PTY_MAX_ROWS).contains(&rows) {
        Ok(())
    } else {
        Err(format!("a terminal is 1 to {PTY_MAX_COLS} columns by 1 to {PTY_MAX_ROWS} rows"))
    }
}

/// A key that fits: a printable character (with or without ctrl or alt), a function key 1 to 12, anything else as named.
pub fn check_key(k: PtyKey) -> Result<(), String> {
    match k {
        PtyKey::Char(c) if c.is_control() => Err("a key is a character you can see, or a named key".into()),
        PtyKey::F(n) if !(1..=12).contains(&n) => Err("function keys are F1 to F12".into()),
        _ => Ok(()),
    }
}


pub trait Terminal: Send + Sync {
    /// Open a session for `user` on behalf of `app` (at most [`MAX_SESSIONS`] at once).
    fn open(&self, _user: &str, _app: &str) -> Result<TermOpen, String> {
        Err(NO_TERMINAL.into())
    }
    /// Run one line. A line that needs a yes comes back with `approval` set and does not run.
    fn run(&self, _user: &str, _session: &str, _line: &str) -> Result<TermReply, String> {
        Err(NO_TERMINAL.into())
    }
    /// The person's answer to the waiting line: yes runs it, no drops it. Answers the call that waits, no other.
    fn decide(&self, _user: &str, _session: &str, _call: &str, _approve: bool) -> Result<TermReply, String> {
        Err(NO_TERMINAL.into())
    }
    fn close(&self, _user: &str, _session: &str) {}

    /// Ask for a whole session of `cols` x `rows` (at most [`PTY_MAX_SESSIONS`] at once). Nothing runs: the answer carries the question
    /// and the session waits for [`Terminal::pty_decide`].
    fn pty_open(&self, _user: &str, _app: &str, _cols: usize, _rows: usize) -> Result<PtyOpen, String> {
        Err(NO_TERMINAL.into())
    }
    /// The person's answer: yes starts the shell, no drops the session.
    fn pty_decide(&self, _user: &str, _session: &str, _call: &str, _approve: bool) -> Result<(), String> {
        Err(NO_TERMINAL.into())
    }
    fn pty_key(&self, _user: &str, _session: &str, _key: PtyKey, _mods: PtyMods) -> Result<(), String> {
        Err(NO_TERMINAL.into())
    }
    /// Typed or pasted text (bracketed when the program asked for it), at most [`PTY_MAX_PASTE`] bytes.
    fn pty_text(&self, _user: &str, _session: &str, _text: &str) -> Result<(), String> {
        Err(NO_TERMINAL.into())
    }
    fn pty_resize(&self, _user: &str, _session: &str, _cols: usize, _rows: usize) -> Result<(), String> {
        Err(NO_TERMINAL.into())
    }
    /// Scroll the view back (positive) or forward by `lines`.
    fn pty_scroll(&self, _user: &str, _session: &str, _lines: isize) -> Result<(), String> {
        Err(NO_TERMINAL.into())
    }
    /// Read what the program wrote and give the screen as it is now.
    fn pty_frame(&self, _user: &str, _session: &str) -> Result<PtyFrame, String> {
        Err(NO_TERMINAL.into())
    }
    fn pty_close(&self, _user: &str, _session: &str) {}
}

pub struct NoTerminal;

impl Terminal for NoTerminal {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_checked_classified_and_output_is_cut_at_a_character() {
        assert!(check_line("ls").is_ok());
        for bad in ["", "   ", "a\nb", "a\u{7}", &"x".repeat(MAX_LINE + 1)] {
            assert!(check_line(bad).is_err(), "{bad:?}");
        }
        assert!(runs_without_asking("ls -l | grep a | wc"));
        assert!(!runs_without_asking("ls > out.txt") && !runs_without_asking("mkdir x") && !runs_without_asking("ls | curl x") && !runs_without_asking("run skill"));
        assert!(is_power("echo hi | reboot") && is_power("poweroff") && !is_power("echo reboot"));
        let long = "é".repeat(MAX_OUTPUT);
        let (t, cut) = cap_output(&long);
        assert!(cut && t.len() <= MAX_OUTPUT && t.is_char_boundary(t.len()));
        assert_eq!(cap_output("short"), ("short".to_string(), false));
        assert!(NoTerminal.open("u", "a").unwrap_err().contains("no terminal"));
    }
}
