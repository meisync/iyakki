//! The chat as a tile, first slice (F-WEB-53): one conversation at a time. The person types a message (text events), presses Send or Enter,
//! and the tile starts a run with `chat.send` (the machine's current model, the profile's tools, the person's own limits and approvals) and
//! then reads the run's lines with `runs.get` every two seconds while it goes on. Whatever the assistant asks to do that needs a person
//! shows as "asks: ..." and is answered ONLY in the approvals list (the tile has no button for it and sends no verdict). Not here: a choice
//! of tools, attachments, formatted answers. Earlier conversations: the Earlier button lists the person's own runs (`runs.list`) and a press on one reads it again (`runs.get`).
use iyakki_tile_kit::abi::{line_height, Color, DisplayList, Event, Op, Rect};
use iyakki_tile_kit::{button, call_json, clip, palette, text, App, Source};
use serde_json::json;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
pub const TILE_WASM: &[u8] = include_bytes!("../tile.wasm");
pub const CAPS: [&str; 5] = ["chat.send", "chat.models", "chat.stop", "runs.get", "runs.list"];

pub const MAX_PROMPT: usize = 2000;
const POLL_MS: u32 = 2000;
const TOP: i32 = 62;
const INPUT_H: i32 = 30;

pub struct ChatTile<S: Source> {
    src: S,
    size: (i32, i32),
    dark: bool,
    input: String,
    run: Option<String>,
    /// The models the service offers (provider, model); the choice is an index, 0 meaning the machine's own.
    models: Vec<(String, String)>,
    sel: usize,
    status: String,
    lines: Vec<String>,
    error: String,
    scroll: usize,
    since: u32,
    /// The list of earlier conversations while it is shown: (run id, status, first words of the task).
    history: Option<Vec<(String, String, String)>>,
    /// What the assistant may use in the next run: 0 every tool the service gives it (asking as always), 1 only the ones that read, 2 none.
    tools: u8,
    /// Files to send with the next message (paths in the person's own folders; the service reads them) and the line being typed for one.
    attach: Vec<String>,
    attach_input: String,
    attach_focus: bool,
}

impl<S: Source> ChatTile<S> {
    pub fn new(src: S) -> Self {
        let mut t = Self::bare(src);
        t.load_models();
        t
    }

    fn bare(src: S) -> Self {
        ChatTile { src, size: (360, 480), dark: false, input: String::new(), run: None, models: vec![], sel: 0, status: String::new(), lines: vec![], error: String::new(), scroll: 0, since: 0, history: None, tools: 0, attach: vec![], attach_input: String::new(), attach_focus: false }
    }

    fn load_models(&mut self) {
        if let Ok(v) = call_json(&mut self.src, "chat", "models", "{}") {
            self.models = v["models"].as_array().map(|a| a.iter().take(10).filter_map(|m| Some((m["provider"].as_str()?.to_string(), m["model"].as_str()?.to_string()))).collect()).unwrap_or_default();
        }
    }

    /// The model the next message goes to: the chosen one, or the machine's own.
    pub fn model_label(&self) -> String {
        match self.sel.checked_sub(1).and_then(|i| self.models.get(i)) {
            Some((p, m)) => format!("{p} / {m}"),
            None => "the machine's own model".to_string(),
        }
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    fn per_page(&self) -> usize {
        (((self.size.1 - TOP - INPUT_H - 8) / 16).max(1)) as usize
    }

    fn going(&self) -> bool {
        self.run.is_some() && (self.status.is_empty() || self.status == "running")
    }

    fn send(&mut self) -> bool {
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() || self.going() {
            return false;
        }
        let mut args = json!({"prompt": prompt});
        if !self.attach.is_empty() {
            args["attach"] = json!(self.attach);
        }
        if self.tools > 0 {
            args["tools"] = json!(if self.tools == 1 { "reading" } else { "none" });
        }
        if let Some((p, m)) = self.sel.checked_sub(1).and_then(|i| self.models.get(i)) {
            args["provider"] = json!(p);
            args["model"] = json!(m);
        }
        match call_json(&mut self.src, "chat", "send", &args.to_string()) {
            Ok(v) => match v["id"].as_str().filter(|s| !s.is_empty()) {
                Some(id) => {
                    self.run = Some(id.to_string());
                    self.lines = vec![format!("you: {}", prompt.chars().take(300).collect::<String>())];
                    self.status = "running".into();
                    self.input.clear();
                    self.attach.clear();
                    self.error.clear();
                    self.scroll = 0;
                    self.poll();
                }
                None => self.error = v["error"].as_str().map(|e| e.chars().take(200).collect()).unwrap_or_else(|| "the service did not name the run".to_string()),
            },
            Err(e) => self.error = e.chars().take(200).collect(),
        }
        true
    }

    /// The person's own runs, newest first (the service sorts them), as a list to choose from.
    fn open_history(&mut self) {
        match call_json(&mut self.src, "runs", "list", "{}") {
            Ok(v) => {
                self.error.clear();
                let rows: Vec<(String, String, String)> = v["runs"].as_array().map(|a| a.iter().take(12).filter_map(|r| Some((r["id"].as_str()?.to_string(), r["status"].as_str().unwrap_or("").to_string(), r["prompt"].as_str().unwrap_or("").chars().take(60).collect()))).collect()).unwrap_or_default();
                self.history = Some(rows);
            }
            Err(e) => self.error = e.chars().take(200).collect(),
        }
    }

    /// Open an earlier conversation: its lines are read again from the service.
    fn open_run(&mut self, i: usize) {
        let Some((id, status, prompt)) = self.history.as_ref().and_then(|h| h.get(i)).cloned() else { return };
        self.history = None;
        self.run = Some(id);
        self.status = status;
        self.lines = vec![format!("you: {prompt}")];
        self.error.clear();
        self.scroll = 0;
        self.poll();
    }

    fn poll(&mut self) {
        self.since = 0;
        let Some(id) = self.run.clone() else { return };
        match call_json(&mut self.src, "runs", "get", &json!({"id": id}).to_string()) {
            Ok(v) => {
                self.error.clear();
                self.status = v["status"].as_str().unwrap_or("").to_string();
                let mut lines = vec![self.lines.first().cloned().unwrap_or_default()];
                lines.extend(v["lines"].as_array().map(|a| a.iter().take(60).map(|l| l.as_str().unwrap_or("").to_string()).collect::<Vec<_>>()).unwrap_or_default());
                self.lines = lines;
                // keep the newest lines in view
                self.scroll = self.lines.len().saturating_sub(self.per_page());
            }
            Err(e) => self.error = e.chars().take(200).collect(),
        }
    }
}

impl<S: Source> App for ChatTile<S> {
    fn event(&mut self, e: &Event) -> bool {
        match e {
            Event::Resize { w, h, .. } => {
                self.size = (*w, *h);
                true
            }
            Event::Theme { dark, .. } => {
                self.dark = *dark;
                true
            }
            Event::TextInput { text } if self.attach_focus => {
                for c in text.chars().filter(|c| !c.is_control()) {
                    if self.attach_input.len() < 300 {
                        self.attach_input.push(c);
                    }
                }
                true
            }
            Event::Key { code: 8, down: true, .. } if self.attach_focus => {
                self.attach_input.pop();
                true
            }
            Event::Key { code: 27, down: true, .. } if self.attach_focus => {
                self.attach_focus = false;
                self.attach_input.clear();
                true
            }
            Event::Key { code: 13, down: true, .. } if self.attach_focus => {
                let p = self.attach_input.trim().to_string();
                self.attach_focus = false;
                self.attach_input.clear();
                if !p.is_empty() && self.attach.len() < 3 && !self.attach.contains(&p) {
                    self.attach.push(p);
                } else if self.attach.len() >= 3 {
                    self.error = "At most three files go with a message.".into();
                }
                true
            }
            Event::Hit { id: 9 } => {
                self.attach_focus = true;
                true
            }
            Event::Hit { id: 10 } if !self.attach.is_empty() => {
                self.attach.clear();
                true
            }
            Event::TextInput { text } => {
                for c in text.chars().filter(|c| !c.is_control()) {
                    if self.input.len() < MAX_PROMPT {
                        self.input.push(c);
                    }
                }
                true
            }
            Event::Key { code: 8, down: true, .. } => {
                self.input.pop();
                true
            }
            Event::Key { code: 13, down: true, .. } | Event::Hit { id: 1 } => self.send(),
            Event::Hit { id: 7 } => {
                if self.history.is_some() {
                    self.history = None;
                } else {
                    self.open_history();
                }
                true
            }
            Event::Hit { id } if (100..112).contains(id) && self.history.is_some() => {
                self.open_run((*id - 100) as usize);
                true
            }
            Event::Hit { id: 8 } => {
                self.tools = (self.tools + 1) % 3;
                true
            }
            Event::Hit { id: 5 } => {
                self.sel = (self.sel + 1) % (self.models.len() + 1);
                true
            }
            Event::Hit { id: 6 } => {
                let Some(id) = self.run.clone().filter(|_| self.going()) else { return false };
                match call_json(&mut self.src, "chat", "stop", &json!({"id": id}).to_string()) {
                    Ok(_) => {
                        self.error.clear();
                        self.poll();
                    }
                    Err(e) => self.error = e.chars().take(200).collect(),
                }
                true
            }
            Event::Hit { id: 2 } => {
                self.history = None;
                self.run = None;
                self.status.clear();
                self.lines.clear();
                self.attach.clear();
                self.error.clear();
                self.scroll = 0;
                true
            }
            Event::Hit { id: 3 } => {
                self.scroll = self.scroll.saturating_sub(self.per_page());
                true
            }
            Event::Hit { id: 4 } => {
                if self.scroll + self.per_page() < self.lines.len() {
                    self.scroll += self.per_page();
                }
                true
            }
            Event::Tick { dt_ms } => {
                self.since = self.since.saturating_add(*dt_ms);
                if self.going() && self.since >= POLL_MS {
                    self.poll();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn frame(&self) -> DisplayList {
        let (w, h) = self.size;
        let p = palette(self.dark);
        let mut l = DisplayList::new();
        l.push(Op::Rect { r: Rect::new(0, 0, w, h), color: p.bg });
        l.push(Op::Rect { r: Rect::new(0, 0, w, 32), color: p.bar });
        l.push(text(8, 8, 14, p.fg, if self.status.is_empty() { "CHAT" } else { "CHAT: " }));
        if !self.status.is_empty() {
            l.push(text(60, 8, 14, if self.status == "failed" { p.bad } else { p.accent }, &clip(&self.status, 140, 14)));
        }
        button(&mut l, 2, Rect::new(w - 60, 4, 52, 24), "New", &p);
        let mut bx = w - 104;
        if self.going() {
            button(&mut l, 6, Rect::new(bx, 4, 40, 24), "Stop", &p);
            bx -= 44;
        }
        if self.lines.len() > self.per_page() {
            button(&mut l, 3, Rect::new(bx, 4, 38, 24), "Up", &p);
            button(&mut l, 4, Rect::new(bx - 44, 4, 38, 24), "Down", &p);
            bx -= 88;
        }
        // the list of earlier conversations: not while a run goes on (the person looks at that one)
        if !self.going() {
            button(&mut l, 7, Rect::new(bx - 62, 4, 64, 24), if self.history.is_some() { "Close" } else { "Earlier" }, &p);
        }
        // the model line: a press chooses the next model (the first is the machine's own)
        l.push(Op::Outline { r: Rect::new(6, 36, w - 12, 20), thickness: 1, color: p.mute });
        l.push(text(12, 40, 12, p.fg, &clip(&format!("model: {}", self.model_label()), w - 24, 12)));
        iyakki_tile_kit::named_hit(&mut l, 5, iyakki_tile_kit::abi::Role::Button, Rect::new(6, 36, w - 12, 20), "Model, press to choose the next", &self.model_label(), 0);
        let mut y = TOP;
        if !self.error.is_empty() {
            l.push(text(8, y, 12, p.bad, &clip(&self.error, w - 16, 12)));
            y += 16;
        }
        if let Some(rows) = &self.history {
            if rows.is_empty() {
                l.push(text(8, y, 13, p.mute, "No earlier conversations."));
            }
            for (i, (_, status, prompt)) in rows.iter().enumerate().take(self.per_page()) {
                l.push(text(8, y, 12, p.fg, &clip(&format!("{status}: {prompt}"), w - 16, 12)));
                iyakki_tile_kit::named_hit(&mut l, 100 + i as u32, iyakki_tile_kit::abi::Role::ListItem, Rect::new(4, y - 1, w - 8, line_height(12) + 2), &format!("Earlier conversation: {status}"), prompt, 0);
                y += line_height(12) + 2;
            }
        } else if self.lines.is_empty() && self.error.is_empty() {
            l.push(text(8, y, 13, p.mute, "Type a message and press Send."));
        }
        let waiting = self.lines.iter().any(|t| t.starts_with("asks: "));
        // the answer's own marks are shown, not printed: headings are bold, list marks are bullets, code is monospace and muted,
        // `**` and the fences are not drawn; the state of a code block is carried from the first line
        let mut code = false;
        let shown = if self.history.is_some() { 0 } else { self.per_page() };
        for (i, t) in self.lines.iter().enumerate() {
            let plain = t.starts_with("you: ") || t.starts_with("asks: ");
            if !plain && t.trim_start().starts_with("```") {
                code = !code;
                if i >= self.scroll && i < self.scroll + shown {
                    y += line_height(12);
                }
                continue;
            }
            if i < self.scroll || i >= self.scroll + shown {
                continue;
            }
            let (face, size, c, body) = if t.starts_with("you: ") {
                (0u8, 12u32, p.accent, t.clone())
            } else if t.starts_with("asks: ") {
                (0, 12, Color(200, 130, 20, 255), t.clone())
            } else if code {
                (1, 12, p.mute, t.clone())
            } else {
                format_line(t, p.fg)
            };
            l.push(Op::Text { x: 8, y, face, size, color: c, text: clip_face(&body, w - 16, face, size) });
            y += line_height(12);
        }
        if waiting && self.going() {
            l.push(text(8, h - INPUT_H - 22, 11, p.mute, "Answer the question in the approvals list."));
        }
        let by = h - INPUT_H - 4;
        // the files that go with the next message, and the line for one more
        // the tools the assistant may use in the next run: all, only the ones that read, or none
        let tools_label = ["all", "read", "none"][self.tools as usize % 3];
        let tools_x = w - 150;
        button(&mut l, 8, Rect::new(tools_x, by - 22, 92, 18), &format!("tools: {tools_label}"), &p);
        if self.attach_focus {
            l.push(text(8, by - 18, 11, p.accent, &clip(&format!("file: {}|", self.attach_input), tools_x - 14, 11)));
        } else if !self.attach.is_empty() {
            l.push(text(8, by - 18, 11, p.mute, &clip(&format!("with: {}", self.attach.join(", ")), tools_x - 14, 11)));
            button(&mut l, 10, Rect::new(w - 54, by - 22, 48, 18), "None", &p);
        }
        l.push(Op::Outline { r: Rect::new(6, by, w - 140, INPUT_H), thickness: 1, color: p.mute });
        button(&mut l, 9, Rect::new(w - 130, by, 48, INPUT_H), "File", &p);
        let shown = if self.input.is_empty() { "message".to_string() } else { self.input.clone() };
        let tail: String = {
            let cs: Vec<char> = shown.chars().collect();
            let fit = ((w - 160) / 6).max(1) as usize;
            cs[cs.len().saturating_sub(fit)..].iter().collect()
        };
        l.push(text(12, by + 8, 13, if self.input.is_empty() { p.mute } else { p.fg }, &tail));
        button(&mut l, 1, Rect::new(w - 78, by, 70, INPUT_H), "Send", &p);
        l
    }
}

/// One line of an answer for the screen: (face 0 sans, 1 monospace, 2 bold; size; colour; the words). Headings are bold, `- ` and
/// `* ` lists get a bullet, `**` marks are dropped. Nothing is ever longer than the line it came from.
fn format_line(t: &str, fg: Color) -> (u8, u32, Color, String) {
    let lead = t.len() - t.trim_start().len();
    let rest = t.trim_start();
    let pad = " ".repeat(lead.min(8));
    if let Some(h) = rest.strip_prefix("### ").or_else(|| rest.strip_prefix("## ")).or_else(|| rest.strip_prefix("# ")) {
        return (2, 12, fg, h.replace("**", ""));
    }
    if let Some(b) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return (0, 12, fg, format!("{pad}\u{2022} {}", b.replace("**", "")));
    }
    (0, 12, fg, t.replace("**", ""))
}

/// `clip` for a face (monospace and bold letters are wider than plain ones).
fn clip_face(t: &str, width: i32, face: u8, size: u32) -> String {
    if iyakki_tile_kit::abi::measure_text(face, size, t) <= width {
        return t.to_string();
    }
    let mut out = String::new();
    for c in t.chars() {
        let mut n = out.clone();
        n.push(c);
        n.push_str("...");
        if iyakki_tile_kit::abi::measure_text(face, size, &n) > width {
            break;
        }
        out.push(c);
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_tile_kit::abi::measure_text;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, String, String)>,
        replies: VecDeque<Result<String, String>>,
    }
    impl Source for Fake {
        fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
            self.calls.push((d.into(), m.into(), j.into()));
            self.replies.pop_front().unwrap_or_else(|| Ok("{}".into()))
        }
    }

    fn tile(r: Vec<Result<String, String>>) -> ChatTile<Fake> {
        // the first call a tile makes is the list of models: here the service has none to offer
        let mut replies: VecDeque<Result<String, String>> = r.into();
        replies.push_front(Ok(r#"{"models":[]}"#.into()));
        let mut t = ChatTile::new(Fake { replies, ..Default::default() });
        t.event(&Event::Resize { w: 360, h: 400, scale_pct: 100 });
        t
    }

    fn type_in(t: &mut ChatTile<Fake>, s: &str) {
        t.event(&Event::TextInput { text: s.into() });
    }

    fn texts(l: &DisplayList) -> Vec<String> {
        l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    fn got(lines: &[&str], status: &str) -> Result<String, String> {
        Ok(json!({"id": "r1", "status": status, "lines": lines}).to_string())
    }

    #[test]
    fn a_message_is_sent_once_and_the_runs_lines_follow() {
        let mut t = tile(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running"), got(&["tool now ok: 12:00", "answer: it is noon"], "completed")]);
        type_in(&mut t, "what time is it");
        assert!(t.event(&Event::Hit { id: 1 }));
        assert_eq!(t.source().calls[0].1, "models");
        assert_eq!(t.source().calls[1], ("chat".to_string(), "send".to_string(), r#"{"prompt":"what time is it"}"#.to_string()));
        assert_eq!(t.source().calls[2].0, "runs");
        let x = texts(&t.frame());
        assert!(x.contains(&"you: what time is it".to_string()), "{x:?}");
        // while it goes on, a second Send does nothing
        type_in(&mut t, "again");
        assert!(!t.event(&Event::Hit { id: 1 }));
        assert_eq!(t.source().calls.iter().filter(|c| c.1 == "send").count(), 1);
        // two seconds later the lines arrive and it is over
        assert!(!t.event(&Event::Tick { dt_ms: 1000 }));
        assert!(t.event(&Event::Tick { dt_ms: 1000 }));
        let x = texts(&t.frame());
        assert!(x.contains(&"answer: it is noon".to_string()) && x.contains(&"completed".to_string()), "{x:?}");
        // now it can be sent again; a finished run is not polled any more
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Tick { dt_ms: 5000 }));
        assert_eq!(t.source().calls.len(), n);
        assert!(t.event(&Event::Key { code: 13, mods: 0, down: true }), "Enter sends too");
    }

    #[test]
    fn a_question_for_the_person_is_shown_and_never_answered_by_the_tile() {
        let mut t = tile(vec![Ok(r#"{"id":"r1"}"#.into()), got(&["asks: delete the old files?"], "running")]);
        type_in(&mut t, "clean up");
        t.event(&Event::Hit { id: 1 });
        let x = texts(&t.frame());
        assert!(x.contains(&"asks: delete the old files?".to_string()) && x.iter().any(|s| s.contains("approvals list")), "{x:?}");
        for id in 0..200 {
            t.event(&Event::Hit { id });
        }
        assert!(t.source().calls.iter().all(|c| matches!((c.0.as_str(), c.1.as_str()), ("chat", "send" | "models" | "stop") | ("runs", "get" | "list"))), "only the chat and run-reading calls: {:?}", t.source().calls);
    }

    #[test]
    fn typing_has_limits_backspace_works_and_new_starts_over() {
        let mut t = tile(vec![]);
        type_in(&mut t, "ab\u{0}c");
        t.event(&Event::Key { code: 8, mods: 0, down: true });
        assert!(texts(&t.frame()).contains(&"ab".to_string()));
        type_in(&mut t, &"x".repeat(5000));
        assert!(t.frame().encode().is_ok());
        assert!(!t.event(&Event::Hit { id: 1 }) || t.source().calls.len() <= 2);
        let mut t = tile(vec![Ok(r#"{"id":"r1"}"#.into()), got(&["answer: hi"], "completed")]);
        type_in(&mut t, "hi");
        t.event(&Event::Hit { id: 1 });
        t.event(&Event::Hit { id: 2 });
        assert!(texts(&t.frame()).contains(&"Type a message and press Send.".to_string()));
        assert!(!t.event(&Event::Hit { id: 1 }), "an empty message is not sent");
    }

    #[test]
    fn answers_show_headings_bullets_and_code_as_such_and_not_as_marks() {
        let mut t = tile(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running"), got(&["# Plan", "- first **step**", "```rust", "let x = 1;", "```", "done"], "completed")]);
        type_in(&mut t, "hi");
        t.event(&Event::Hit { id: 1 });
        t.event(&Event::Tick { dt_ms: 1000 });
        t.event(&Event::Tick { dt_ms: 1000 });
        let f = t.frame();
        let find = |s: &str| f.ops.iter().find_map(|o| match o { Op::Text { face, text, .. } if text == s => Some(*face), _ => None });
        assert_eq!(find("Plan"), Some(2), "a heading is bold and has no #");
        assert_eq!(find("\u{2022} first step"), Some(0), "a list mark is a bullet and ** is gone");
        assert_eq!(find("let x = 1;"), Some(1), "code is monospace");
        assert_eq!(find("done"), Some(0), "text after the block is plain again");
        assert!(texts(&f).iter().all(|x| !x.contains("```") && !x.contains("**")), "{:?}", texts(&f));
    }

    #[test]
    fn the_earlier_button_lists_the_persons_runs_and_a_press_on_one_reads_it_again() {
        let list = json!({"runs": [{"id": "r9", "status": "completed", "prompt": "what is the weather"}, {"id": "r8", "status": "failed", "prompt": "an old question"}]}).to_string();
        let mut t = tile(vec![Ok(list), got(&["answer: sunny"], "completed")]);
        assert!(t.event(&Event::Hit { id: 7 }), "Earlier opens the list");
        assert_eq!(t.source().calls[1], ("runs".to_string(), "list".to_string(), "{}".to_string()));
        let x = texts(&t.frame());
        assert!(x.contains(&"completed: what is the weather".to_string()) && x.contains(&"failed: an old question".to_string()) && x.contains(&"Close".to_string()), "{x:?}");
        assert!(t.frame().ops.iter().any(|o| matches!(o, Op::Hit { id: 100, .. })) && t.frame().ops.iter().any(|o| matches!(o, Op::Hit { id: 101, .. })));
        // a press on the first row reads that run
        assert!(t.event(&Event::Hit { id: 100 }));
        assert_eq!(t.source().calls[2].0, "runs");
        assert!(t.source().calls[2].2.contains("r9"));
        let x = texts(&t.frame());
        assert!(x.contains(&"you: what is the weather".to_string()) && x.contains(&"answer: sunny".to_string()) && !x.contains(&"Close".to_string()), "{x:?}");
        // a row that is not there does nothing; the list closes with Close, New and a second press of the button
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Hit { id: 105 }));
        assert_eq!(t.source().calls.len(), n);
    }

    #[test]
    fn an_empty_history_a_refusal_and_closing_are_plain() {
        let mut t = tile(vec![Ok(r#"{"runs":[]}"#.into()), Err("your account may not use the chat".into())]);
        t.event(&Event::Hit { id: 7 });
        assert!(texts(&t.frame()).contains(&"No earlier conversations.".to_string()));
        t.event(&Event::Hit { id: 7 });
        assert!(!texts(&t.frame()).contains(&"No earlier conversations.".to_string()), "the second press closes it");
        t.event(&Event::Hit { id: 7 });
        assert_eq!(t.error(), "your account may not use the chat");
        let mut t = tile(vec![Ok(r#"{"runs":[{"id":"a","status":"running","prompt":"x"}]}"#.into())]);
        t.event(&Event::Hit { id: 7 });
        t.event(&Event::Hit { id: 2 });
        assert!(texts(&t.frame()).contains(&"Type a message and press Send.".to_string()), "New closes the list too");
        // nothing but the declared calls is ever made
        for id in 0..300 {
            t.event(&Event::Hit { id });
        }
        assert!(t.source().calls.iter().all(|c| matches!((c.0.as_str(), c.1.as_str()), ("chat", "send" | "models" | "stop") | ("runs", "get" | "list"))), "{:?}", t.source().calls);
    }

    #[test]
    fn a_refusal_or_a_broken_answer_is_one_red_line_and_the_message_is_kept() {
        let mut t = tile(vec![Err("this tile is not allowed to do that".into())]);
        type_in(&mut t, "hello");
        t.event(&Event::Hit { id: 1 });
        assert_eq!(t.error(), "this tile is not allowed to do that");
        assert!(texts(&t.frame()).contains(&"hello".to_string()), "the message stays in the box");
        let mut t = tile(vec![Ok("nope".into())]);
        type_in(&mut t, "hello");
        t.event(&Event::Hit { id: 1 });
        assert!(t.error().contains("not readable"));
        let mut t = tile(vec![Ok("{}".into())]);
        type_in(&mut t, "hello");
        t.event(&Event::Hit { id: 1 });
        assert!(t.error().contains("did not name"));
    }

    #[test]
    fn long_conversations_scroll_and_nothing_is_wider_than_the_tile() {
        let long: Vec<String> = (0..50).map(|i| format!("answer line {i} {}", "w".repeat(200))).collect();
        let refs: Vec<&str> = long.iter().map(String::as_str).collect();
        let mut t = tile(vec![Ok(r#"{"id":"r1"}"#.into()), got(&refs, "completed")]);
        type_in(&mut t, "go");
        t.event(&Event::Hit { id: 1 });
        let l = t.frame();
        assert!(texts(&l).iter().any(|s| s.starts_with("answer line 49")), "the newest lines are in view");
        for o in &l.ops {
            if let Op::Text { x, size, text, .. } = o {
                assert!(x + measure_text(0, *size, text) <= 360, "{text}");
            }
        }
        for _ in 0..10 {
            t.event(&Event::Hit { id: 3 });
        }
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("you: go")));
    }

    fn tile_with_models(r: Vec<Result<String, String>>) -> ChatTile<Fake> {
        let mut replies: VecDeque<Result<String, String>> = r.into();
        replies.push_front(Ok(json!({"models": [{"provider": "local", "model": "tiny"}, {"provider": "cloud", "model": "big"}]}).to_string()));
        let mut t = ChatTile::new(Fake { replies, ..Default::default() });
        t.event(&Event::Resize { w: 360, h: 400, scale_pct: 100 });
        t
    }

    #[test]
    fn a_press_on_the_model_line_chooses_the_next_model_and_the_message_goes_to_it() {
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running")]);
        assert!(texts(&t.frame()).contains(&"model: the machine's own model".to_string()));
        t.event(&Event::Hit { id: 5 });
        assert!(texts(&t.frame()).contains(&"model: local / tiny".to_string()));
        t.event(&Event::Hit { id: 5 });
        assert_eq!(t.model_label(), "cloud / big");
        t.event(&Event::Hit { id: 5 });
        assert_eq!(t.model_label(), "the machine's own model", "it wraps around");
        t.event(&Event::Hit { id: 5 });
        type_in(&mut t, "hello");
        t.event(&Event::Hit { id: 1 });
        let send = t.source().calls.iter().find(|c| c.1 == "send").unwrap().2.clone();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&send).unwrap(), json!({"prompt": "hello", "provider": "local", "model": "tiny"}));
        // the default sends no model at all
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running")]);
        type_in(&mut t, "hello");
        t.event(&Event::Hit { id: 1 });
        assert_eq!(t.source().calls.iter().find(|c| c.1 == "send").unwrap().2, r#"{"prompt":"hello"}"#);
    }

    #[test]
    fn stop_is_there_only_while_a_run_goes_on_and_asks_the_service_to_stop_that_run() {
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running"), Ok("{}".into()), got(&["stopped: you stopped it"], "stopped")]);
        assert!(!t.frame().ops.iter().any(|o| matches!(o, Op::Hit { id: 6, .. })), "no Stop before a run");
        assert!(!t.event(&Event::Hit { id: 6 }), "and pressing it does nothing");
        type_in(&mut t, "long job");
        t.event(&Event::Hit { id: 1 });
        assert!(t.frame().ops.iter().any(|o| matches!(o, Op::Hit { id: 6, .. })), "Stop while it goes on");
        assert!(t.event(&Event::Hit { id: 6 }));
        let stop = t.source().calls.iter().find(|c| c.1 == "stop").unwrap().clone();
        assert_eq!((stop.0.as_str(), stop.2.as_str()), ("chat", r#"{"id":"r1"}"#));
        let x = texts(&t.frame());
        assert!(x.contains(&"stopped".to_string()) && x.contains(&"stopped: you stopped it".to_string()), "{x:?}");
        assert!(!t.frame().ops.iter().any(|o| matches!(o, Op::Hit { id: 6, .. })), "no Stop once it ended");
    }

    #[test]
    fn the_tools_choice_goes_with_the_message_only_when_it_is_not_all_and_wraps_around() {
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running")]);
        assert!(texts(&t.frame()).contains(&"tools: all".to_string()));
        t.event(&Event::Hit { id: 8 });
        assert!(texts(&t.frame()).contains(&"tools: read".to_string()));
        type_in(&mut t, "look");
        t.event(&Event::Hit { id: 1 });
        assert_eq!(serde_json::from_str::<serde_json::Value>(&t.source().calls.iter().find(|c| c.1 == "send").unwrap().2).unwrap(), json!({"prompt": "look", "tools": "reading"}));
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running")]);
        t.event(&Event::Hit { id: 8 });
        t.event(&Event::Hit { id: 8 });
        assert!(texts(&t.frame()).contains(&"tools: none".to_string()));
        type_in(&mut t, "just talk");
        t.event(&Event::Hit { id: 1 });
        assert_eq!(serde_json::from_str::<serde_json::Value>(&t.source().calls.iter().find(|c| c.1 == "send").unwrap().2).unwrap()["tools"], "none");
        t.event(&Event::Hit { id: 8 });
        assert!(texts(&t.frame()).contains(&"tools: all".to_string()), "it wraps around");
    }

    #[test]
    fn files_are_named_to_go_with_the_message_at_most_three_and_are_sent_once() {
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running"), Ok(r#"{"id":"r2"}"#.into()), got(&[], "running")]);
        t.event(&Event::Hit { id: 9 });
        type_in(&mut t, "/a");
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("file: /a")));
        t.event(&Event::Key { code: 13, mods: 0, down: true });
        assert!(texts(&t.frame()).iter().any(|s| s == "with: /a"));
        // Esc drops a line being typed; the same name twice is one file; four are too many
        t.event(&Event::Hit { id: 9 });
        type_in(&mut t, "/a");
        t.event(&Event::Key { code: 13, mods: 0, down: true });
        for p in ["/b", "/c", "/d"] {
            t.event(&Event::Hit { id: 9 });
            type_in(&mut t, p);
            t.event(&Event::Key { code: 13, mods: 0, down: true });
        }
        assert!(texts(&t.frame()).iter().any(|s| s == "with: /a, /b, /c"), "{:?}", texts(&t.frame()));
        assert!(t.error().contains("At most three"));
        type_in(&mut t, "summarise");
        t.event(&Event::Hit { id: 1 });
        let sent: serde_json::Value = serde_json::from_str(&t.source().calls.iter().find(|c| c.1 == "send").unwrap().2).unwrap();
        assert_eq!(sent["attach"], json!(["/a", "/b", "/c"]));
        assert!(!texts(&t.frame()).iter().any(|s| s.starts_with("with: ")), "the files went with that message only");
        // the None button takes them away before sending
        let mut t = tile_with_models(vec![Ok(r#"{"id":"r1"}"#.into()), got(&[], "running")]);
        t.event(&Event::Hit { id: 9 });
        type_in(&mut t, "/a");
        t.event(&Event::Key { code: 13, mods: 0, down: true });
        t.event(&Event::Hit { id: 10 });
        type_in(&mut t, "hi");
        t.event(&Event::Hit { id: 1 });
        assert_eq!(t.source().calls.iter().find(|c| c.1 == "send").unwrap().2, r#"{"prompt":"hi"}"#);
    }
}
