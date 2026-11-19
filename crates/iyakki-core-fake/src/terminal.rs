//! A terminal for tests (the `Terminal` facet): lines are echoed as `ran: <line>`; a line that is not read-only waits for a yes, like the real
//! one, because it uses the same classification (`iyakki_core_api::terminal`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use iyakki_core_api::terminal::{self as t, PtyCell, PtyFrame, PtyKey, PtyMods, PtyOpen, TermApproval, TermOpen, TermReply, Terminal};

#[derive(Default)]
struct Inner {
    sessions: BTreeMap<(String, String), Option<(String, String)>>,
    next: u32,
    /// Whole sessions: waiting call (until the yes), the typed line, the lines on the screen, the size, closed.
    pty: BTreeMap<(String, String), FakePty>,
    /// What was run, in order: `user: line`.
    pub log: Vec<String>,
}

struct FakePty {
    call: Option<String>,
    typed: String,
    lines: Vec<String>,
    size: (usize, usize),
    seq: u64,
    back: usize,
}

#[derive(Default)]
pub struct FakeTerminal {
    inner: Mutex<Inner>,
    off: bool,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeTerminal {
    pub fn log(&self) -> Vec<String> {
        lk(&self.inner).log.clone()
    }
}

impl Terminal for FakeTerminal {
    fn open(&self, user: &str, _app: &str) -> Result<TermOpen, String> {
        let _ = self.off;
        let mut g = lk(&self.inner);
        if g.sessions.keys().filter(|(u, _)| u == user).count() >= t::MAX_SESSIONS {
            return Err(format!("at most {} terminals can be open at once", t::MAX_SESSIONS));
        }
        g.next += 1;
        let id = format!("t{}", g.next);
        g.sessions.insert((user.into(), id.clone()), None);
        Ok(TermOpen { id, prompt: format!("{user}$"), banner: "fake terminal".into() })
    }

    fn run(&self, user: &str, session: &str, line: &str) -> Result<TermReply, String> {
        t::check_line(line)?;
        let mut g = lk(&self.inner);
        let prompt = format!("{user}$");
        let key = (user.to_string(), session.to_string());
        match g.sessions.get(&key) {
            None => return Err("that terminal is not open".into()),
            Some(Some(_)) => return Err("answer the waiting question first".into()),
            _ => {}
        }
        if t::is_power(line) {
            return Ok(TermReply { text: t::NO_POWER.into(), prompt, ..TermReply::default() });
        }
        if !t::runs_without_asking(line) {
            g.next += 1;
            let call = format!("c{}", g.next);
            g.sessions.insert(key, Some((call.clone(), line.to_string())));
            return Ok(TermReply { prompt, approval: Some(TermApproval { call, summary: format!("Run in your terminal: {line}") }), ..TermReply::default() });
        }
        g.log.push(format!("{user}: {line}"));
        Ok(TermReply { text: format!("ran: {line}"), prompt, clear: line.trim() == "clear", ..TermReply::default() })
    }

    fn decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<TermReply, String> {
        let mut g = lk(&self.inner);
        let key = (user.to_string(), session.to_string());
        let prompt = format!("{user}$");
        let Some(Some((c, line))) = g.sessions.get(&key).cloned() else { return Err("there is no such question waiting".into()) };
        if c != call {
            return Err("there is no such question waiting".into());
        }
        g.sessions.insert(key, None);
        if !approve {
            return Ok(TermReply { text: "Not run: you said no.".into(), prompt, ..TermReply::default() });
        }
        g.log.push(format!("{user}: {line}"));
        Ok(TermReply { text: format!("ran: {line}"), prompt, ..TermReply::default() })
    }

    fn close(&self, user: &str, session: &str) {
        lk(&self.inner).sessions.remove(&(user.to_string(), session.to_string()));
    }

    fn pty_open(&self, user: &str, _app: &str, cols: usize, rows: usize) -> Result<PtyOpen, String> {
        t::check_size(cols, rows)?;
        let mut g = lk(&self.inner);
        if g.pty.keys().filter(|(u, _)| u == user).count() >= t::PTY_MAX_SESSIONS {
            return Err(format!("at most {} full terminals can be open at once", t::PTY_MAX_SESSIONS));
        }
        g.next += 1;
        let (id, call) = (format!("p{}", g.next), format!("pc{}", g.next));
        g.pty.insert((user.into(), id.clone()), FakePty { call: Some(call.clone()), typed: String::new(), lines: vec!["fake$ ".into()], size: (cols, rows), seq: 1, back: 0 });
        Ok(PtyOpen { id, approval: TermApproval { call, summary: t::PTY_QUESTION.into() } })
    }

    fn pty_decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<(), String> {
        let mut g = lk(&self.inner);
        let key = (user.to_string(), session.to_string());
        match g.pty.get_mut(&key) {
            Some(p) if p.call.as_deref() == Some(call) => {
                p.call = None;
                if !approve {
                    g.pty.remove(&key);
                }
                Ok(())
            }
            _ => Err("there is no such question waiting".into()),
        }
    }

    fn pty_key(&self, user: &str, session: &str, key: PtyKey, mods: PtyMods) -> Result<(), String> {
        t::check_key(key)?;
        let mut g = lk(&self.inner);
        let p = live(&mut g.pty, user, session)?;
        match key {
            PtyKey::Char(c) if mods.ctrl => p.typed.push_str(&format!("^{}", c.to_ascii_uppercase())),
            PtyKey::Char(c) => p.typed.push(c),
            PtyKey::Backspace => {
                p.typed.pop();
            }
            PtyKey::Enter => {
                let line = std::mem::take(&mut p.typed);
                if let Some(l) = p.lines.last_mut() {
                    l.push_str(&line);
                }
                p.lines.push(format!("ran: {line}"));
                p.lines.push("fake$ ".into());
            }
            _ => {}
        }
        p.seq += 1;
        Ok(())
    }

    fn pty_text(&self, user: &str, session: &str, text: &str) -> Result<(), String> {
        let mut g = lk(&self.inner);
        let p = live(&mut g.pty, user, session)?;
        p.typed.push_str(text);
        p.seq += 1;
        Ok(())
    }

    fn pty_resize(&self, user: &str, session: &str, cols: usize, rows: usize) -> Result<(), String> {
        t::check_size(cols, rows)?;
        let mut g = lk(&self.inner);
        let p = live(&mut g.pty, user, session)?;
        p.size = (cols, rows);
        p.seq += 1;
        Ok(())
    }

    fn pty_scroll(&self, user: &str, session: &str, lines: isize) -> Result<(), String> {
        let mut g = lk(&self.inner);
        let p = live(&mut g.pty, user, session)?;
        p.back = (p.back as isize + lines).clamp(0, p.lines.len() as isize) as usize;
        p.seq += 1;
        Ok(())
    }

    fn pty_frame(&self, user: &str, session: &str) -> Result<PtyFrame, String> {
        let mut g = lk(&self.inner);
        let p = live(&mut g.pty, user, session)?;
        let (cols, rows) = p.size;
        let end = p.lines.len().saturating_sub(p.back);
        let start = end.saturating_sub(rows);
        let mut cells: Vec<Vec<PtyCell>> = p.lines[start..end].iter().enumerate().map(|(i, l)| {
            let mut row: Vec<PtyCell> = l.chars().take(cols).map(|ch| PtyCell { ch, ..PtyCell::default() }).collect();
            if i + 1 == end - start && p.back == 0 {
                row.extend(p.typed.chars().take(cols.saturating_sub(row.len())).map(|ch| PtyCell { ch, ..PtyCell::default() }));
            }
            row.resize(cols, PtyCell::default());
            row
        }).collect();
        cells.resize(rows, vec![PtyCell::default(); cols]);
        let cursor = (p.lines.get(end.wrapping_sub(1)).map_or(0, |l| l.chars().count() + p.typed.chars().count()).min(cols - 1), (end - start).saturating_sub(1));
        Ok(PtyFrame { seq: p.seq, cols, rows, cells, cursor, cursor_visible: p.back == 0, title: String::new(), back: p.back, history: p.lines.len().saturating_sub(rows), exited: None })
    }

    fn pty_close(&self, user: &str, session: &str) {
        lk(&self.inner).pty.remove(&(user.to_string(), session.to_string()));
    }
}

fn live<'a>(m: &'a mut BTreeMap<(String, String), FakePty>, user: &str, session: &str) -> Result<&'a mut FakePty, String> {
    match m.get_mut(&(user.to_string(), session.to_string())) {
        None => Err("that terminal is not open".into()),
        Some(p) if p.call.is_some() => Err("the terminal waits for your yes".into()),
        Some(p) => Ok(p),
    }
}
