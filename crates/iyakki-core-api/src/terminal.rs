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
