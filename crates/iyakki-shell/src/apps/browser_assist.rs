//! The assistant inside the browser: ask about the page (or the words selected on it), summarise it, translate it into
//! English, or keep it in the person's own knowledge base. Nothing leaves the machine before the person has seen where
//! it would go and agreed; the answer is shown over the page and nothing is stored unless they ask for that.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use iyakki_agent::chat::Message;

use super::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Key, TextField, CARD, ERR, MUTED, OK, R, TEXT};

/// Most characters of a page handed to a model.
const MAX_SENT: usize = 12_000;
const ACCENT: [u8; 4] = rgb(150, 190, 255);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Task {
    Ask(String),
    Summarise,
    Translate,
}

impl Task {
    fn label(&self) -> &'static str {
        match self {
            Task::Ask(_) => "Ask the assistant",
            Task::Summarise => "Summary",
            Task::Translate => "English translation",
        }
    }
}

enum Stage {
    Question(TextField),
    Confirm(Task),
    Working { task: Task, rx: Receiver<Result<String, String>> },
    Answer { task: Task, text: String, scroll: usize },
    Failed(String),
}

pub struct Assist {
    stage: Stage,
    /// The words that would be sent, and what they are ("this page" or "the selected text").
    source: String,
    what: &'static str,
    url: String,
}

/// What a key did.
pub enum Outcome {
    Open,
    Closed,
}

/// Where the model for chat lives, in words for the question "send it there?".
pub(super) fn destination(cx: &AppCx) -> Result<String, String> {
    let st = cx.services.settings.lock().map_err(|_| "settings are busy".to_string())?;
    let s = st.get();
    let Some(choice) = &s.chat else { return Err("No assistant is set up yet (Settings → AI).".into()) };
    let host = s.providers.iter().find(|p| p.id == choice.provider).and_then(|p| p.base_url.clone()).unwrap_or_default();
    let host = host.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or("").to_string();
    Ok(if host.is_empty() { format!("{} ({})", choice.provider, choice.model) } else { format!("{} ({}) at {host}", choice.provider, choice.model) })
}

impl Assist {
    pub fn ask(source: String, what: &'static str, url: String) -> Assist {
        let mut field = TextField::new("Your question about it", false);
        field.max = 400;
        Assist { stage: Stage::Question(field), source, what, url }
    }

    pub fn task(task: Task, source: String, what: &'static str, url: String) -> Assist {
        Assist { stage: Stage::Confirm(task), source, what, url }
    }

    fn messages(&self, task: &Task) -> Vec<Message> {
        let text: String = self.source.chars().take(MAX_SENT).collect();
        let (system, user) = match task {
            Task::Ask(q) => ("You answer questions about a web page. Use only the page text given. If the page does not say, say so. Be brief.", format!("Page: {}\n\n{text}\n\nQuestion: {q}", self.url)),
            Task::Summarise => ("You summarise web pages in plain English: a one-sentence gist, then up to six short bullet points.", format!("Page: {}\n\n{text}", self.url)),
            Task::Translate => ("You translate text into English. Keep the structure. Do not add comments.", text),
        };
        vec![Message::system(system), Message::user(user)]
    }

    fn start(&mut self, task: Task, cx: &AppCx) {
        let history = self.messages(&task);
        let chat = cx.chat.clone();
        let (tx, rx) = mpsc::channel();
        cx.audit.record(&cx.session.user, "browser.assist", &format!("{} {}", task.label(), self.url.split('/').nth(2).unwrap_or("")));
        std::thread::spawn(move || {
            let _ = tx.send(chat.complete(&history));
        });
        self.stage = Stage::Working { task, rx };
    }

    /// Check for the model's answer.
    pub fn poll(&mut self) {
        let Stage::Working { task, rx } = &self.stage else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the assistant stopped without an answer".into()),
        };
        let task = task.clone();
        self.stage = match r {
            Ok(text) if !text.trim().is_empty() => Stage::Answer { task, text: text.trim().to_string(), scroll: 0 },
            Ok(_) => Stage::Failed("The assistant sent an empty answer.".into()),
            Err(e) => Stage::Failed(e),
        };
    }

    pub fn paste(&mut self, t: &str) {
        if let Stage::Question(f) = &mut self.stage {
            f.paste(t);
        }
    }

    pub fn key(&mut self, k: &Key, cx: &AppCx) -> Outcome {
        match &mut self.stage {
            Stage::Question(f) => match k {
                Key::Esc => return Outcome::Closed,
                Key::Enter => {
                    let q = f.value.trim().to_string();
                    if !q.is_empty() {
                        self.confirm_or_fail(Task::Ask(q), cx);
                    }
                }
                other => {
                    f.handle(other);
                }
            },
            Stage::Confirm(task) => match k {
                Key::Enter | Key::Char('y') => {
                    let task = task.clone();
                    self.start(task, cx);
                }
                Key::Esc | Key::Char('n') => return Outcome::Closed,
                _ => {}
            },
            Stage::Working { .. } => {
                if matches!(k, Key::Esc) {
                    return Outcome::Closed;
                }
            }
            Stage::Answer { text, scroll, .. } => match k {
                Key::Esc | Key::Enter => return Outcome::Closed,
                Key::Up => *scroll = scroll.saturating_sub(1),
                Key::Down => *scroll += 1,
                Key::PageUp => *scroll = scroll.saturating_sub(10),
                Key::PageDown => *scroll += 10,
                Key::Home => *scroll = 0,
                Key::Char('c') => {
                    crate::ui::clipboard::copy(text);
                    crate::ui::clipboard::toast("Copied");
                }
                _ => {}
            },
            Stage::Failed(_) => return Outcome::Closed,
        }
        Outcome::Open
    }

    /// Before the question goes anywhere: make sure there is an assistant, then ask whether to send.
    pub fn confirm_or_fail(&mut self, task: Task, cx: &AppCx) {
        self.stage = match destination(cx) {
            Ok(_) => Stage::Confirm(task),
            Err(e) => Stage::Failed(e),
        };
    }

    /// The first thing to show when opened for a task that needs no question: fail early with no assistant.
    pub fn check(mut self, cx: &AppCx) -> Assist {
        if let Stage::Confirm(t) = &self.stage {
            let t = t.clone();
            self.confirm_or_fail(t, cx);
        }
        self
    }

    pub fn render(&mut self, g: &mut Gfx, view: R, s: f32, cx: &AppCx, tick: u64) {
        let w = (view.w - 40.0 * s).min(760.0 * s);
        let h = (view.h - 40.0 * s).min(460.0 * s);
        let r = R::new(view.x + (view.w - w) / 2.0, view.y + 16.0 * s, w, h);
        g.rect(view.x, view.y, view.w, view.h, [6, 8, 18, 150]);
        g.rrect_glow(r.x, r.y, r.w, r.h, 14.0 * s, 8.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 60]);
        g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, rgb(16, 21, 40));
        g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 1.5 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 150]);
        let pad = 18.0 * s;
        let (x, mut y, iw) = (r.x + pad, r.y + 14.0 * s, r.w - 2.0 * pad);
        match &mut self.stage {
            Stage::Question(f) => {
                g.text(x, y, 17.0 * s, ACCENT, &format!("Ask the assistant about {}", self.what), Face::Bold);
                y += 34.0 * s;
                f.render(g, R::new(x, y, iw, 38.0 * s), true, s, (tick / 15) % 2 == 0, Face::Sans, 521);
                y += 52.0 * s;
                g.text(x, y, 13.0 * s, MUTED, "Enter to continue · Esc to cancel. You are asked before anything is sent.", Face::Sans);
            }
            Stage::Confirm(task) => {
                let dest = destination(cx).unwrap_or_default();
                g.text(x, y, 17.0 * s, ACCENT, task.label(), Face::Bold);
                y += 34.0 * s;
                let chars = self.source.chars().count().min(MAX_SENT);
                let msg = format!("Send {} ({} characters) to {dest}?", self.what, chars);
                for line in g.wrap(15.0 * s, &msg, Face::Sans, iw) {
                    g.text(x, y, 15.0 * s, TEXT, &line, Face::Sans);
                    y += 22.0 * s;
                }
                y += 10.0 * s;
                g.text(x, y, 13.0 * s, MUTED, "Enter or Y sends it · Esc or N cancels.", Face::Sans);
            }
            Stage::Working { task, .. } => {
                g.text(x, y, 17.0 * s, ACCENT, task.label(), Face::Bold);
                y += 34.0 * s;
                let dots = ".".repeat((tick / 10 % 4) as usize);
                g.text(x, y, 15.0 * s, MUTED, &format!("Waiting for the assistant{dots}   (Esc stops waiting)"), Face::Sans);
            }
            Stage::Answer { task, text, scroll } => {
                g.text(x, y, 17.0 * s, ACCENT, task.label(), Face::Bold);
                y += 32.0 * s;
                let lines = g.wrap(15.0 * s, text, Face::Sans, iw);
                let room = (((r.y + r.h - y - 44.0 * s) / (21.0 * s)) as usize).max(1);
                *scroll = (*scroll).min(lines.len().saturating_sub(room));
                for l in lines.iter().skip(*scroll).take(room) {
                    g.text(x, y, 15.0 * s, TEXT, l, Face::Sans);
                    y += 21.0 * s;
                }
                if lines.len() > room {
                    g.text(r.x + r.w - 150.0 * s, r.y + 16.0 * s, 12.0 * s, MUTED, &format!("line {} of {}", *scroll + 1, lines.len()), Face::Sans);
                }
                g.text(x, r.y + r.h - 30.0 * s, 13.0 * s, MUTED, "↑↓ scroll · C copy the answer · Esc close", Face::Sans);
                let _ = (CARD, OK);
            }
            Stage::Failed(e) => {
                g.text(x, y, 17.0 * s, ERR, "The assistant could not help", Face::Bold);
                y += 34.0 * s;
                for line in g.wrap(15.0 * s, e, Face::Sans, iw) {
                    g.text(x, y, 15.0 * s, TEXT, &line, Face::Sans);
                    y += 22.0 * s;
                }
                g.text(x, r.y + r.h - 30.0 * s, 13.0 * s, MUTED, "Any key closes this.", Face::Sans);
            }
        }
    }

    /// The answer text, once there is one.
    pub fn answer(&self) -> Option<&str> {
        match &self.stage {
            Stage::Answer { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.stage {
            Stage::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn is_confirming(&self) -> bool {
        matches!(self.stage, Stage::Confirm(_))
    }
}

/// Keep a page (or the selected words) in the person's own knowledge base, where the assistant can find it later.
pub fn remember(cx: &AppCx, title: &str, url: &str, text: &str) -> Result<usize, String> {
    if text.trim().is_empty() {
        return Err("There is no text to keep.".into());
    }
    let ns = format!("user/{}", cx.session.user);
    let mut meta = BTreeMap::new();
    meta.insert("from".to_string(), "browser".to_string());
    let title = if title.trim().is_empty() { url.to_string() } else { title.trim().to_string() };
    let mut kb = cx.services.kb.lock().map_err(|_| "the knowledge base is busy".to_string())?;
    kb.add_document(&ns, &title, url, text, meta).map_err(|e| e.to_string())?;
    cx.audit.record(&cx.session.user, "browser.remember", url.split('/').nth(2).unwrap_or(""));
    Ok(text.chars().count())
}
