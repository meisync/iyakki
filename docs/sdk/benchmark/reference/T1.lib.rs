//! Reference solution of T1 (counter, notes with persistence, a switch and its chat tool).
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::assistant::{AssistantPort, NoAssistant, SessionConfig, ToolSpec};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::chatview::ChatView;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::desktop::DockStatus;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::toolargs::Args;
use iyakki_ui_sdk::widgets;

pub mod ports;
use ports::{MemStorage, StoragePort};

pub const ID: &str = "bench-t1";
const A_TAB_COUNT: u32 = 1;
const A_TAB_NOTES: u32 = 2;
const A_TAB_CHAT: u32 = 3;
const A_MORE: u32 = 4;
const A_FIELD: u32 = 5;
const A_ADD: u32 = 6;
const A_COMPACT: u32 = 7;
const A_ROW0: u32 = 20;
const CHAT: u32 = 200;
const TABS: f32 = 3.0;
const MAX_NOTES: usize = 100;
const COUNT_KEY: &str = "count";
const NOTES_KEY: &str = "notes";
const COMPACT_KEY: &str = "compact";

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Counter,
    Notes,
    Chat,
}

pub struct BenchT1App {
    pane: Pane,
    count: u32,
    notes: Vec<String>,
    compact: bool,
    draft: String,
    editing: bool,
    said: String,
    storage: Box<dyn StoragePort>,
    chat: ChatView,
}

impl Default for BenchT1App {
    fn default() -> Self {
        BenchT1App::new()
    }
}

impl BenchT1App {
    pub fn new() -> BenchT1App {
        BenchT1App::with_ports(Box::new(MemStorage::default()), Box::new(NoAssistant))
    }

    pub fn with_storage(storage: Box<dyn StoragePort>) -> BenchT1App {
        BenchT1App::with_ports(storage, Box::new(NoAssistant))
    }

    pub fn with_ports(mut storage: Box<dyn StoragePort>, assistant: Box<dyn AssistantPort>) -> BenchT1App {
        let count = storage.get(COUNT_KEY).and_then(|s| s.parse().ok()).unwrap_or(0);
        let notes = storage.get(NOTES_KEY).map(|s| s.split('\n').filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default();
        let compact = storage.get(COMPACT_KEY).is_some_and(|s| s == "true");
        let tools = vec![
            ToolSpec::new("compact_status", "Say whether the notes list uses compact rows.", r#"{"type":"object"}"#, false),
            ToolSpec::new("compact_set", "Turn compact rows in the notes list on or off.", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#, true),
        ];
        let cfg = SessionConfig { conversation: "bench-t1".into(), title: "Notes".into(), tools, ..SessionConfig::default() };
        BenchT1App { pane: Pane::Counter, count, notes, compact, draft: String::new(), editing: false, said: String::new(), storage, chat: ChatView::new(assistant, cfg, CHAT) }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }

    pub fn compact(&self) -> bool {
        self.compact
    }

    fn save(&mut self) {
        let r = self
            .storage
            .set(COUNT_KEY, &self.count.to_string())
            .and_then(|_| self.storage.set(NOTES_KEY, &self.notes.join("\n")))
            .and_then(|_| self.storage.set(COMPACT_KEY, if self.compact { "true" } else { "false" }));
        self.said = if r.is_err() { "The core did not allow saving.".into() } else { String::new() };
    }

    /// The one function behind the screen's switch and the chat tool.
    fn set_compact(&mut self, on: bool) -> String {
        self.compact = on;
        self.save();
        format!("Compact rows are {}.", if on { "on" } else { "off" })
    }

    fn add_note(&mut self) {
        let t = self.draft.trim().to_string();
        if t.is_empty() {
            self.said = "Write something first.".into();
        } else if self.notes.len() >= MAX_NOTES {
            self.said = "That is enough notes: delete one first.".into();
        } else {
            self.notes.push(t);
            self.draft.clear();
            self.save();
        }
    }
}

impl UiApp for BenchT1App {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "BENCH T1".into()
    }
    fn dock_status(&self) -> Option<DockStatus> {
        Some(DockStatus::new(None, &format!("{} notes", self.notes.len())))
    }
    fn wants_keyboard(&self) -> bool {
        self.editing || (self.pane == Pane::Chat && self.chat.wants_keyboard())
    }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => {
                for inv in self.chat.tick() {
                    let r = match inv.tool.as_str() {
                        "compact_status" => Ok(format!("Compact rows are {}.", if self.compact { "on" } else { "off" })),
                        "compact_set" => match Args::parse(&inv.args).ok().and_then(|a| a.bool("on")) {
                            Some(on) => Ok(self.set_compact(on)),
                            None => Err("give on: true or false".to_string()),
                        },
                        _ => Err("unknown tool".to_string()),
                    };
                    self.chat.answer(&inv, r);
                }
            }
            UiEvent::Action(A_TAB_COUNT) => self.pane = Pane::Counter,
            UiEvent::Action(A_TAB_NOTES) => self.pane = Pane::Notes,
            UiEvent::Action(A_TAB_CHAT) => self.pane = Pane::Chat,
            UiEvent::Action(A_MORE) => {
                self.count = self.count.saturating_add(1);
                self.save();
            }
            UiEvent::Action(A_COMPACT) => {
                let on = !self.compact;
                self.set_compact(on);
            }
            UiEvent::Action(A_FIELD) => self.editing = true,
            UiEvent::Action(A_ADD) => self.add_note(),
            UiEvent::Action(a) if self.chat.owns(*a) => {
                self.chat.action(*a);
            }
            UiEvent::Wheel(n) if self.pane == Pane::Chat => self.chat.wheel(*n),
            UiEvent::Text(t) if self.pane == Pane::Chat => {
                self.chat.text(t);
            }
            UiEvent::Text(t) if self.editing && self.pane == Pane::Notes => self.draft.push_str(t),
            UiEvent::Key(k) => {
                if self.pane == Pane::Chat && self.chat.key(*k) {
                    return Flow::Stay;
                }
                match k {
                    UiKey::Backspace if self.editing && self.pane == Pane::Notes => {
                        self.draft.pop();
                    }
                    UiKey::Enter if self.editing && self.pane == Pane::Notes => self.add_note(),
                    UiKey::Esc if self.editing => self.editing = false,
                    UiKey::Esc => return Flow::Back,
                    _ => {}
                }
            }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let t = m.min_target();
        let tab_w = (w - g - g - m.gap(Space::Md) - m.gap(Space::Md)) / TABS;
        for (i, (label, a, p)) in [("Counter", A_TAB_COUNT, Pane::Counter), ("Notes", A_TAB_NOTES, Pane::Notes), ("Assistant", A_TAB_CHAT, Pane::Chat)].into_iter().enumerate() {
            widgets::button(c, th, &m, Rect::new(g + i as f32 * (tab_w + m.gap(Space::Md)), g, tab_w, t), label, a, self.pane == p);
        }
        let top = g + t + m.gap(Space::Lg);
        let area = Rect::new(g, top, w - 2.0 * g, (h - top - g).max(0.0));
        let body = m.size(TypeRole::Body);
        match self.pane {
            Pane::Counter => {
                let big = m.size(TypeRole::Large);
                let line = fit(c, big, &format!("Pressed {} times", self.count), TextStyle::Bold, area.w);
                c.text(area.x, area.y, big, th.text, &line, TextStyle::Bold);
                widgets::button(c, th, &m, Rect::new(area.x, area.y + 2.0 * big, area.w, t), "More", A_MORE, true);
            }
            Pane::Notes => {
                widgets::field(c, th, &m, Rect::new(area.x, area.y, area.w - 2.0 * t, t), "Note", &self.draft, self.editing, A_FIELD);
                widgets::button(c, th, &m, Rect::new(area.x + area.w - 2.0 * t, area.y, 2.0 * t, t), "Add", A_ADD, true);
                widgets::button(c, th, &m, Rect::new(area.x, area.y + t + m.gap(Space::Md), area.w, t), "Compact rows", A_COMPACT, self.compact);
                let mut y = area.y + 2.0 * t + 2.0 * m.gap(Space::Md);
                let row_h = if self.compact { t } else { t + m.gap(Space::Md) };
                for (i, n) in self.notes.iter().enumerate() {
                    if y + row_h > area.y + area.h - body * 2.0 {
                        break;
                    }
                    widgets::row(c, th, &m, Rect::new(area.x, y, area.w, row_h), n, false, A_ROW0 + i as u32);
                    y += row_h + m.gap(Space::Xs);
                }
                if !self.said.is_empty() {
                    let s = fit(c, body, &self.said, TextStyle::Regular, area.w);
                    c.text(area.x, area.y + area.h - body * 1.5, body, th.muted, &s, TextStyle::Regular);
                }
            }
            Pane::Chat => self.chat.draw(c, th, &m, area),
        }
    }
}
