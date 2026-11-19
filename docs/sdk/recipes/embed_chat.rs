use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::assistant::{AssistantPort, ContextItem, ContextKind, SessionConfig, ToolSpec};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::chatview::ChatView;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::toolargs::Args;

const CHAT: u32 = 200; // the view owns CHAT .. CHAT + chatview::ACTIONS

pub struct ChatApp {
    chat: ChatView,
    pub notes: Vec<String>,
}

impl ChatApp {
    pub fn new(port: Box<dyn AssistantPort>) -> ChatApp {
        let tools = vec![
            // true: the person must allow each call on a card (anything that changes data asks)
            ToolSpec::new("add_note", "Add one note.", r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#, true),
            ToolSpec::new("list_notes", "List the notes.", r#"{"type":"object"}"#, false),
        ];
        let cfg = SessionConfig { conversation: "notes".into(), title: "Notes".into(), tools, ..SessionConfig::default() };
        ChatApp { chat: ChatView::new(port, cfg, CHAT), notes: Vec::new() }
    }
}

impl UiApp for ChatApp {
    fn id(&self) -> &'static str { "chat-demo" }
    fn title(&self) -> String { "Chat demo".into() }
    fn wants_keyboard(&self) -> bool { self.chat.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => {
                for inv in self.chat.tick() {
                    let result = match inv.tool.as_str() {
                        "add_note" => Args::parse(&inv.args).ok().and_then(|a| a.str("text").map(String::from)).map(|t| { self.notes.push(t); "added".to_string() }).ok_or_else(|| "give a text".to_string()),
                        _ => Ok(self.notes.join("; ")),
                    };
                    self.chat.answer(&inv, result);
                }
                self.chat.set_context(vec![ContextItem::new(ContextKind::Document, "Notes", &self.notes.join("\n"))]);
            }
            UiEvent::Action(a) if self.chat.owns(*a) => { self.chat.action(*a); }
            UiEvent::Text(t) => { self.chat.text(t); }
            UiEvent::Key(k) => { if !self.chat.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            UiEvent::Wheel(n) => self.chat.wheel(*n),
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.chat.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
