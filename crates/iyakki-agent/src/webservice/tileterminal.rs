//! The `terminal` domain of a browser tile (F-SDK-34): what the UI SDK's `TerminalPort` asks of a tile host, mapped onto the core's terminal
//! facet (`iyakki_core_api::Terminal`) for the signed-in person and the tile's own id. The core decides everything that matters: the
//! commands the person's role allows, the files and hosts they may reach, which line needs a yes, the limits and the audit lines. The tile
//! holds no session id: the host keeps them (one line session and one whole session per tile).
//!
//! Two kinds of session, as the facet has them:
//! * the command line: `open`, `run {line}`, `decide {call, approve}`, `close`. A line that changes something comes back with `question`
//!   and does not run; the person answers it.
//! * the whole session (a real program on a pseudo-terminal): `screen_open {cols, rows}` gives the question, `screen_decide`, then
//!   `screen_key {key, ch, n, ctrl, alt}`, `screen_text {text}`, `screen_resize`, `screen_scroll {lines}`, `screen_frame`, `screen_close`.
//!
//! Consent is the person's: a yes (`decide` or `screen_decide` with `approve` true) is taken from their page by the web service's approval
//! card (`TileBackend::terminal_call`), never from the tile. A no needs nobody. A refusal is an answer `{"error": sentence}`, so the tile can
//! say it in words (a failed host call would only say "the service did not answer").
//!
//! The screen travels as runs: `lines` holds one list per row, each entry `[text, fg, bg, attrs]`; a colour is null (default), a number
//! (0 to 255, indexed) or `"rrggbb"`; the right half of a double-width character is a NUL in the text.

use iyakki_core_api::terminal::{PtyCell, PtyColor, PtyFrame, PtyKey, PtyMods, Terminal};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The largest screen answer a tile is handed (the tile's reply buffer for this domain is 128 KiB).
pub const MAX_FRAME_TEXT: usize = 120 * 1024;

#[derive(Debug, PartialEq)]
pub enum Req {
    Open,
    Run(String),
    Decide(String, bool),
    Close,
    ScreenOpen(usize, usize),
    ScreenDecide(String, bool),
    ScreenKey(PtyKey, PtyMods),
    ScreenText(String),
    ScreenResize(usize, usize),
    ScreenScroll(isize),
    ScreenFrame,
    ScreenClose,
}

fn whole(v: &Value, k: &str, lo: u64, hi: u64) -> Result<usize, String> {
    match v.get(k).and_then(Value::as_u64) {
        Some(n) if (lo..=hi).contains(&n) => Ok(n as usize),
        _ => Err(format!("\"{k}\" is a whole number from {lo} to {hi}")),
    }
}

fn text<'a>(v: &'a Value, k: &str, max: usize) -> Result<&'a str, String> {
    match v.get(k).and_then(Value::as_str) {
        Some(s) if !s.is_empty() && s.len() <= max && !s.contains('\0') => Ok(s),
        _ => Err(format!("\"{k}\" is text of 1 to {max} bytes")),
    }
}

/// The key a tile names (`key` is `char` with `ch`, `f` with `n`, or the name of a named key).
pub fn key_of(v: &Value) -> Result<(PtyKey, PtyMods), String> {
    let name = text(v, "key", 16)?;
    let key = match name {
        "char" => {
            let mut it = text(v, "ch", 8)?.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => PtyKey::Char(c),
                _ => return Err("\"ch\" is one character".into()),
            }
        }
        "f" => PtyKey::F(whole(v, "n", 1, 12)? as u8),
        "enter" => PtyKey::Enter,
        "tab" => PtyKey::Tab,
        "backtab" => PtyKey::BackTab,
        "backspace" => PtyKey::Backspace,
        "esc" => PtyKey::Esc,
        "up" => PtyKey::Up,
        "down" => PtyKey::Down,
        "left" => PtyKey::Left,
        "right" => PtyKey::Right,
        "home" => PtyKey::Home,
        "end" => PtyKey::End,
        "pageup" => PtyKey::PageUp,
        "pagedown" => PtyKey::PageDown,
        "insert" => PtyKey::Insert,
        "delete" => PtyKey::Delete,
        _ => return Err("that is not a key a terminal takes".into()),
    };
    iyakki_core_api::terminal::check_key(key)?;
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    Ok((key, PtyMods { ctrl: flag("ctrl"), alt: flag("alt"), shift: false }))
}

pub fn plan(method: &str, args_json: &str) -> Result<Req, String> {
    let v: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !v.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    let approve = |v: &Value| v.get("approve").and_then(Value::as_bool).ok_or("\"approve\" is true or false".to_string());
    Ok(match method {
        "open" => Req::Open,
        "run" => {
            let line = text(&v, "line", iyakki_core_api::terminal::MAX_LINE)?;
            iyakki_core_api::terminal::check_line(line)?;
            Req::Run(line.to_string())
        }
        "decide" => Req::Decide(text(&v, "call", 64)?.to_string(), approve(&v)?),
        "close" => Req::Close,
        "screen_open" => {
            let (c, r) = (whole(&v, "cols", 1, 300)?, whole(&v, "rows", 1, 100)?);
            iyakki_core_api::terminal::check_size(c, r)?;
            Req::ScreenOpen(c, r)
        }
        "screen_decide" => Req::ScreenDecide(text(&v, "call", 64)?.to_string(), approve(&v)?),
        "screen_key" => {
            let (k, m) = key_of(&v)?;
            Req::ScreenKey(k, m)
        }
        "screen_text" => {
            let t = v.get("text").and_then(Value::as_str).ok_or("\"text\" is text")?;
            if t.is_empty() || t.len() > iyakki_core_api::terminal::PTY_MAX_PASTE {
                return Err(format!("\"text\" is 1 to {} bytes", iyakki_core_api::terminal::PTY_MAX_PASTE));
            }
            Req::ScreenText(t.to_string())
        }
        "screen_resize" => {
            let (c, r) = (whole(&v, "cols", 1, 300)?, whole(&v, "rows", 1, 100)?);
            iyakki_core_api::terminal::check_size(c, r)?;
            Req::ScreenResize(c, r)
        }
        "screen_scroll" => match v.get("lines").and_then(Value::as_i64) {
            Some(n) if (-1000..=1000).contains(&n) => Req::ScreenScroll(n as isize),
            _ => return Err("\"lines\" is a whole number from -1000 to 1000".into()),
        },
        "screen_frame" => Req::ScreenFrame,
        "screen_close" => Req::ScreenClose,
        _ => return Err("that is not a call this tile may make".into()),
    })
}

/// A whole session of the tile: the id, the question that waits for the person (until it is answered yes), and whether it runs.
struct Pty {
    session: String,
    call: String,
    summary: String,
    started: bool,
}

/// What the host remembers for one tile: its line session, its whole session, and the cards it was shown (call id to the words of the card).
#[derive(Default)]
pub struct Own {
    line: Option<String>,
    pty: Option<Pty>,
    pub cards: BTreeMap<String, String>,
}

impl Own {
    /// The tool name of the person's card for a yes to this call, and the call (the card's arguments), when it is one that needs the person.
    pub fn card_for(&self, req: &Req) -> Option<(&'static str, String)> {
        match req {
            Req::Decide(call, true) => Some(("terminal_line", call.clone())),
            Req::ScreenDecide(call, true) => Some(("terminal_session", call.clone())),
            _ => None,
        }
    }
}

/// The words of the person's card for a yes (the core's summary names a command line, which is the person's own text: cut and cleaned).
pub fn card_words(tile: &str, summary: &str) -> String {
    let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    format!("Let {} run in your terminal: {}", clean(tile, 40), clean(summary, 200))
}

fn colour(c: PtyColor) -> Value {
    match c {
        PtyColor::Default => Value::Null,
        PtyColor::Indexed(n) => json!(n),
        PtyColor::Rgb(r, g, b) => json!(format!("{r:02x}{g:02x}{b:02x}")),
    }
}

fn row_runs(row: &[PtyCell]) -> Vec<Value> {
    let mut runs: Vec<(String, PtyColor, PtyColor, u8)> = Vec::new();
    for c in row {
        let ch = if c.tail { '\0' } else if c.ch == '\0' { ' ' } else { c.ch };
        match runs.last_mut() {
            Some((t, fg, bg, a)) if *fg == c.fg && *bg == c.bg && *a == c.attrs => t.push(ch),
            _ => runs.push((ch.to_string(), c.fg, c.bg, c.attrs)),
        }
    }
    runs.into_iter().map(|(t, fg, bg, a)| json!([t, colour(fg), colour(bg), a])).collect()
}

/// The screen as the answer to `screen_frame`; refused when it would not fit the tile's reply buffer.
pub fn frame_json(f: &PtyFrame) -> Result<String, String> {
    let lines: Vec<Vec<Value>> = f.cells.iter().map(|r| row_runs(r)).collect();
    let s = json!({"seq": f.seq, "cols": f.cols, "rows": f.rows, "cursor": [f.cursor.0, f.cursor.1], "cursor_visible": f.cursor_visible, "back": f.back, "history": f.history, "exited": f.exited, "lines": lines}).to_string();
    if s.len() > MAX_FRAME_TEXT {
        return Err("that screen is too big for an app of this kind; make the window smaller".into());
    }
    Ok(s)
}

fn error(e: &str) -> String {
    json!({"error": e}).to_string()
}

fn reply_json(r: &iyakki_core_api::TermReply, own: &mut Own, tile: &str) -> String {
    if let Some(a) = &r.approval {
        if own.cards.len() < 64 {
            own.cards.insert(a.call.clone(), card_words(tile, &a.summary));
        }
    }
    json!({"text": r.text, "prompt": r.prompt, "clear": r.clear, "cut": r.cut, "question": r.approval.as_ref().map(|a| json!({"call": a.call, "summary": a.summary}))}).to_string()
}

/// Run a planned request. `Decide` and `ScreenDecide` with `approve` true must have been approved by the person before this is called; this
/// function does not ask. A refusal of the core is an `{"error": ...}` answer, not a failed call.
pub fn run(c: &dyn Terminal, user: &str, app: &str, own: &mut Own, req: &Req) -> Result<String, String> {
    Ok(match go(c, user, app, own, req) {
        Ok(s) => s,
        Err(e) => error(&e),
    })
}

fn live_pty<'a>(own: &'a Own) -> Result<&'a str, String> {
    match &own.pty {
        None => Err("that terminal is not open".into()),
        Some(p) if !p.started => Err("the terminal waits for your yes".into()),
        Some(p) => Ok(&p.session),
    }
}

fn go(c: &dyn Terminal, user: &str, app: &str, own: &mut Own, req: &Req) -> Result<String, String> {
    let ok = || json!({"ok": true}).to_string();
    Ok(match req {
        Req::Open => {
            if let Some(old) = own.line.take() {
                c.close(user, &old);
            }
            let o = c.open(user, app)?;
            own.line = Some(o.id);
            json!({"prompt": o.prompt, "banner": o.banner}).to_string()
        }
        Req::Run(line) => {
            let Some(s) = own.line.clone() else { return Err("that terminal is not open".into()) };
            let r = c.run(user, &s, line)?;
            reply_json(&r, own, app)
        }
        Req::Decide(call, approve) => {
            let Some(s) = own.line.clone() else { return Err("that terminal is not open".into()) };
            let r = c.decide(user, &s, call, *approve)?;
            own.cards.remove(call);
            reply_json(&r, own, app)
        }
        Req::Close => {
            if let Some(s) = own.line.take() {
                c.close(user, &s);
            }
            own.cards.clear();
            ok()
        }
        Req::ScreenOpen(cols, rows) => {
            // the question that waits is asked again, not made again: the person answers one card
            if let Some(p) = own.pty.as_ref().filter(|p| !p.started) {
                return Ok(json!({"call": p.call, "summary": p.summary}).to_string());
            }
            if let Some(old) = own.pty.take() {
                c.pty_close(user, &old.session);
            }
            let o = c.pty_open(user, app, *cols, *rows)?;
            let (call, summary) = (o.approval.call.clone(), o.approval.summary.clone());
            if own.cards.len() < 64 {
                own.cards.insert(call.clone(), card_words(app, &summary));
            }
            own.pty = Some(Pty { session: o.id, call: call.clone(), summary: summary.clone(), started: false });
            json!({"call": call, "summary": summary}).to_string()
        }
        Req::ScreenDecide(call, approve) => {
            let Some(p) = own.pty.as_mut().filter(|p| &p.call == call && !p.started) else { return Err("there is no such question waiting".into()) };
            let session = p.session.clone();
            c.pty_decide(user, &session, call, *approve)?;
            own.cards.remove(call);
            if *approve {
                p.started = true;
            } else {
                own.pty = None;
            }
            ok()
        }
        Req::ScreenKey(k, m) => {
            c.pty_key(user, live_pty(own)?, *k, *m)?;
            ok()
        }
        Req::ScreenText(t) => {
            c.pty_text(user, live_pty(own)?, t)?;
            ok()
        }
        Req::ScreenResize(cols, rows) => {
            c.pty_resize(user, live_pty(own)?, *cols, *rows)?;
            ok()
        }
        Req::ScreenScroll(n) => {
            c.pty_scroll(user, live_pty(own)?, *n)?;
            ok()
        }
        Req::ScreenFrame => frame_json(&c.pty_frame(user, live_pty(own)?)?)?,
        Req::ScreenClose => {
            if let Some(p) = own.pty.take() {
                own.cards.remove(&p.call);
                c.pty_close(user, &p.session);
            }
            ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::NoTerminal;
    use iyakki_core_fake::FakeTerminal;

    fn call(core: &dyn Terminal, own: &mut Own, who: &str, m: &str, a: &str) -> Value {
        serde_json::from_str(&plan(m, a).and_then(|r| run(core, who, "cli", own, &r)).unwrap_or_else(|e| error(&e))).unwrap()
    }

    #[test]
    fn calls_are_checked_before_the_core_is_asked() {
        assert_eq!(plan("run", r#"{"line":"ls"}"#).unwrap(), Req::Run("ls".into()));
        assert_eq!(plan("screen_open", r#"{"cols":80,"rows":24}"#).unwrap(), Req::ScreenOpen(80, 24));
        assert_eq!(plan("screen_key", r#"{"key":"char","ch":"x","ctrl":true}"#).unwrap(), Req::ScreenKey(PtyKey::Char('x'), PtyMods { ctrl: true, alt: false, shift: false }));
        assert_eq!(plan("screen_key", r#"{"key":"f","n":5}"#).unwrap(), Req::ScreenKey(PtyKey::F(5), PtyMods::default()));
        assert_eq!(plan("screen_key", r#"{"key":"pageup"}"#).unwrap(), Req::ScreenKey(PtyKey::PageUp, PtyMods::default()));
        for (m, a) in [
            ("run", "{}"),
            ("run", r#"{"line":""}"#),
            ("run", r#"{"line":"a\nb"}"#),
            ("decide", r#"{"call":"c"}"#),
            ("screen_open", r#"{"cols":0,"rows":24}"#),
            ("screen_open", r#"{"cols":80,"rows":101}"#),
            ("screen_key", r#"{"key":"char","ch":"ab"}"#),
            ("screen_key", r#"{"key":"char","ch":"\u0007"}"#),
            ("screen_key", r#"{"key":"f","n":13}"#),
            ("screen_key", r#"{"key":"meta"}"#),
            ("screen_text", r#"{"text":""}"#),
            ("screen_scroll", r#"{"lines":5000}"#),
            ("power", "{}"),
            ("run", "[1]"),
            ("run", "nope"),
        ] {
            assert!(plan(m, a).is_err(), "{m} {a}");
        }
    }

    #[test]
    fn a_line_that_reads_runs_and_one_that_changes_asks_first_and_runs_only_after_the_yes() {
        let core = FakeTerminal::default();
        let mut own = Own::default();
        assert_eq!(call(&core, &mut own, "amy", "run", r#"{"line":"pwd"}"#)["error"], "that terminal is not open");
        let o = call(&core, &mut own, "amy", "open", "{}");
        assert_eq!((o["prompt"].as_str(), o["banner"].as_str()), (Some("amy$"), Some("fake terminal")));
        let r = call(&core, &mut own, "amy", "run", r#"{"line":"pwd"}"#);
        assert_eq!((r["text"].as_str(), r["question"].is_null()), (Some("ran: pwd"), true));
        let r = call(&core, &mut own, "amy", "run", r#"{"line":"mkdir x"}"#);
        let q = r["question"]["call"].as_str().expect("a change asks first").to_string();
        assert_eq!(core.log(), vec!["amy: pwd".to_string()], "nothing ran yet");
        assert!(own.cards[&q].starts_with("Let cli run in your terminal: Run in your terminal: mkdir x"), "{:?}", own.cards);
        assert_eq!(own.card_for(&plan("decide", &format!(r#"{{"call":"{q}","approve":true}}"#)).unwrap()), Some(("terminal_line", q.clone())));
        assert_eq!(own.card_for(&plan("decide", &format!(r#"{{"call":"{q}","approve":false}}"#)).unwrap()), None, "a no needs nobody");
        assert_eq!(call(&core, &mut own, "amy", "run", r#"{"line":"ls"}"#)["error"], "answer the waiting question first");
        let r = call(&core, &mut own, "amy", "decide", &format!(r#"{{"call":"{q}","approve":true}}"#));
        assert_eq!(r["text"], "ran: mkdir x");
        assert!(!own.cards.contains_key(&q));
        // a no runs nothing
        let r = call(&core, &mut own, "amy", "run", r#"{"line":"rm y"}"#);
        let q = r["question"]["call"].as_str().unwrap().to_string();
        assert!(call(&core, &mut own, "amy", "decide", &format!(r#"{{"call":"{q}","approve":false}}"#))["text"].as_str().unwrap().starts_with("Not run"));
        assert_eq!(core.log(), vec!["amy: pwd".to_string(), "amy: mkdir x".to_string()]);
        // a power line is told no in the core's words
        assert!(call(&core, &mut own, "amy", "run", r#"{"line":"reboot"}"#)["text"].as_str().unwrap().contains("Power button"));
        assert_eq!(call(&core, &mut own, "amy", "close", "{}")["ok"], true);
        assert_eq!(call(&core, &mut own, "amy", "run", r#"{"line":"pwd"}"#)["error"], "that terminal is not open");
    }

    #[test]
    fn a_tile_reaches_only_its_own_session_and_a_second_open_closes_the_first() {
        let core = FakeTerminal::default();
        let (mut a, mut b) = (Own::default(), Own::default());
        call(&core, &mut a, "amy", "open", "{}");
        call(&core, &mut b, "amy", "open", "{}");
        let q = call(&core, &mut a, "amy", "run", r#"{"line":"mkdir x"}"#)["question"]["call"].as_str().unwrap().to_string();
        // another tile's decide for that call finds no such question; the call id of the other session is not its own
        assert_eq!(call(&core, &mut b, "amy", "decide", &format!(r#"{{"call":"{q}","approve":true}}"#))["error"], "there is no such question waiting");
        // a person who does not have the session gets the core's refusal
        assert_eq!(call(&core, &mut a, "bob", "run", r#"{"line":"pwd"}"#)["error"], "that terminal is not open");
        // opening again closes the first session: the limit of the core is not eaten by restarts
        for _ in 0..8 {
            assert!(call(&core, &mut a, "amy", "open", "{}")["prompt"].is_string());
        }
    }

    #[test]
    fn a_core_with_no_terminal_answers_with_its_sentence() {
        let mut own = Own::default();
        assert!(call(&NoTerminal, &mut own, "amy", "open", "{}")["error"].as_str().unwrap().contains("no terminal"));
        assert!(call(&NoTerminal, &mut own, "amy", "screen_open", r#"{"cols":80,"rows":24}"#)["error"].as_str().unwrap().contains("no terminal"));
    }

    #[test]
    fn a_whole_session_starts_only_after_the_yes_and_the_screen_comes_back_as_runs() {
        let core = FakeTerminal::default();
        let mut own = Own::default();
        assert_eq!(call(&core, &mut own, "amy", "screen_frame", "{}")["error"], "that terminal is not open");
        let q = call(&core, &mut own, "amy", "screen_open", r#"{"cols":20,"rows":4}"#);
        let c = q["call"].as_str().unwrap().to_string();
        assert!(q["summary"].as_str().unwrap().starts_with("Open a full terminal?"));
        assert!(own.cards[&c].contains("Open a full terminal?"));
        // asking again gives the same question and makes no second session
        assert_eq!(call(&core, &mut own, "amy", "screen_open", r#"{"cols":20,"rows":4}"#)["call"], c.as_str());
        assert_eq!(call(&core, &mut own, "amy", "screen_key", r#"{"key":"enter"}"#)["error"], "the terminal waits for your yes");
        assert_eq!(call(&core, &mut own, "amy", "screen_frame", "{}")["error"], "the terminal waits for your yes");
        assert_eq!(call(&core, &mut own, "amy", "screen_decide", r#"{"call":"nope","approve":true}"#)["error"], "there is no such question waiting");
        assert_eq!(own.card_for(&plan("screen_decide", &format!(r#"{{"call":"{c}","approve":true}}"#)).unwrap()), Some(("terminal_session", c.clone())));
        assert_eq!(call(&core, &mut own, "amy", "screen_decide", &format!(r#"{{"call":"{c}","approve":true}}"#))["ok"], true);
        for (m, a) in [("screen_key", r#"{"key":"char","ch":"l"}"#), ("screen_key", r#"{"key":"char","ch":"s"}"#), ("screen_key", r#"{"key":"enter"}"#), ("screen_text", r#"{"text":"hi"}"#), ("screen_resize", r#"{"cols":30,"rows":5}"#), ("screen_scroll", r#"{"lines":1}"#)] {
            assert_eq!(call(&core, &mut own, "amy", m, a)["ok"], true, "{m}");
        }
        let f = call(&core, &mut own, "amy", "screen_frame", "{}");
        assert_eq!((f["cols"].as_u64(), f["rows"].as_u64(), f["lines"].as_array().unwrap().len()), (Some(30), Some(5), 5));
        let rows: Vec<String> = f["lines"].as_array().unwrap().iter().map(|r| r.as_array().unwrap().iter().map(|run| run[0].as_str().unwrap()).collect::<String>().trim_end().to_string()).collect();
        assert!(rows.join("\n").contains("ran: ls"), "{rows:?}");
        assert_eq!(call(&core, &mut own, "amy", "screen_close", "{}")["ok"], true);
        assert_eq!(call(&core, &mut own, "amy", "screen_frame", "{}")["error"], "that terminal is not open");
    }

    #[test]
    fn a_no_to_the_session_drops_it_and_a_new_ask_makes_a_new_session() {
        let core = FakeTerminal::default();
        let mut own = Own::default();
        let c = call(&core, &mut own, "amy", "screen_open", r#"{"cols":20,"rows":4}"#)["call"].as_str().unwrap().to_string();
        assert_eq!(call(&core, &mut own, "amy", "screen_decide", &format!(r#"{{"call":"{c}","approve":false}}"#))["ok"], true);
        assert_eq!(call(&core, &mut own, "amy", "screen_frame", "{}")["error"], "that terminal is not open");
        let c2 = call(&core, &mut own, "amy", "screen_open", r#"{"cols":20,"rows":4}"#)["call"].as_str().unwrap().to_string();
        assert_ne!(c, c2);
        // a session of one tile is closed when the tile opens a bigger one after it began
        call(&core, &mut own, "amy", "screen_decide", &format!(r#"{{"call":"{c2}","approve":true}}"#));
        let c3 = call(&core, &mut own, "amy", "screen_open", r#"{"cols":20,"rows":4}"#)["call"].as_str().unwrap().to_string();
        assert_ne!(c2, c3, "a running session is closed and a new question made");
    }

    #[test]
    fn colours_attributes_and_double_width_tails_survive_the_wire_and_a_huge_screen_is_refused() {
        let mut f = PtyFrame { seq: 3, cols: 4, rows: 1, cursor: (1, 0), cursor_visible: true, exited: Some(0), ..PtyFrame::default() };
        let red = PtyCell { ch: 'a', fg: PtyColor::Indexed(1), bg: PtyColor::Rgb(1, 2, 255), attrs: 1, tail: false };
        f.cells = vec![vec![red, PtyCell { ch: 'b', ..red }, PtyCell { ch: '\u{5b57}', ..PtyCell::default() }, PtyCell { tail: true, ..PtyCell::default() }]];
        let v: Value = serde_json::from_str(&frame_json(&f).unwrap()).unwrap();
        assert_eq!((v["seq"].as_u64(), v["exited"].as_i64(), v["cursor"][0].as_u64()), (Some(3), Some(0), Some(1)));
        assert_eq!(v["lines"][0], json!([["ab", 1, "0102ff", 1], ["\u{5b57}\0", null, null, 0]]));
        let mut big = PtyFrame { cols: 300, rows: 100, ..PtyFrame::default() };
        big.cells = (0..100).map(|_| (0..300).map(|i| PtyCell { ch: 'x', fg: PtyColor::Indexed((i % 250) as u8), ..PtyCell::default() }).collect()).collect();
        assert!(frame_json(&big).unwrap_err().contains("too big"));
    }

    #[test]
    fn the_card_words_are_cleaned_and_short() {
        let w = card_words("cli\u{7}", &format!("rm {}", "x".repeat(400)));
        assert!(w.starts_with("Let cli run in your terminal: rm xxx") && w.len() < 300 && !w.contains('\u{7}'), "{w}");
    }
}
