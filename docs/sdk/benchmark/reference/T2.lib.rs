//! Reference solution of T2 (Files pane and Assistant pane with an approval card).
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::assistant::{AssistantPort, ContextItem, ContextKind, NoAssistant, SessionConfig, ToolSpec};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::chatview::ChatView;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::files::{FilesPort, MemFiles};
use iyakki_ui_sdk::filesview::FilesView;
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

pub mod ports;

pub const ID: &str = "bench-t2";
const A_TAB_FILES: u32 = 1;
const A_TAB_CHAT: u32 = 2;
const FILES: u32 = 100;
const CHAT: u32 = 300;
const TABS: f32 = 2.0;

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Files,
    Chat,
}

pub struct BenchT2App {
    pane: Pane,
    files: FilesView,
    chat: ChatView,
    open: Option<(String, String)>,
    context_sent: bool,
}

impl Default for BenchT2App {
    fn default() -> Self {
        BenchT2App::new()
    }
}

impl BenchT2App {
    pub fn new() -> BenchT2App {
        BenchT2App::with_ports(Box::new(MemFiles::new()), Box::new(NoAssistant))
    }

    pub fn with_ports(files: Box<dyn FilesPort>, assistant: Box<dyn AssistantPort>) -> BenchT2App {
        let tools = vec![
            ToolSpec::new("read_open_file", "Read the text of the file that is open.", r#"{"type":"object"}"#, false),
            ToolSpec::new("close_file", "Close the file that is open in the app.", r#"{"type":"object"}"#, true),
        ];
        let cfg = SessionConfig { conversation: "bench-t2".into(), title: "Files".into(), tools, ..SessionConfig::default() };
        BenchT2App { pane: Pane::Files, files: FilesView::new(files, FILES), chat: ChatView::new(assistant, cfg, CHAT), open: None, context_sent: false }
    }

    /// The path of the open file.
    pub fn opened(&self) -> Option<String> {
        self.open.as_ref().map(|(p, _)| p.clone())
    }

    fn sync_context(&mut self) {
        let items = match &self.open {
            Some((p, t)) => vec![ContextItem::new(ContextKind::Document, p, t)],
            None => Vec::new(),
        };
        self.chat.set_context(items);
    }
}

impl UiApp for BenchT2App {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "BENCH T2".into()
    }
    fn wants_keyboard(&self) -> bool {
        self.pane == Pane::Chat && self.chat.wants_keyboard()
    }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => {
                self.files.tick();
                if let Some(o) = self.files.take_opened() {
                    self.open = Some(o);
                    self.context_sent = false;
                }
                if !self.context_sent {
                    self.sync_context();
                    self.context_sent = true;
                }
                for inv in self.chat.tick() {
                    let r = match inv.tool.as_str() {
                        "read_open_file" => Ok(self.open.as_ref().map(|(_, t)| t.clone()).unwrap_or_else(|| "No file open.".into())),
                        "close_file" => {
                            self.open = None;
                            self.context_sent = false;
                            Ok("Closed.".to_string())
                        }
                        _ => Err("unknown tool".to_string()),
                    };
                    self.chat.answer(&inv, r);
                }
            }
            UiEvent::Action(A_TAB_FILES) => self.pane = Pane::Files,
            UiEvent::Action(A_TAB_CHAT) => self.pane = Pane::Chat,
            UiEvent::Action(a) if self.files.owns(*a) => {
                self.files.action(*a);
            }
            UiEvent::Action(a) if self.chat.owns(*a) => {
                self.chat.action(*a);
            }
            UiEvent::Wheel(n) if self.pane == Pane::Files => self.files.wheel(*n),
            UiEvent::Wheel(n) => self.chat.wheel(*n),
            UiEvent::Text(t) if self.pane == Pane::Chat => {
                self.chat.text(t);
            }
            UiEvent::Key(k) => {
                let used = if self.pane == Pane::Files { self.files.key(*k) } else { self.chat.key(*k) };
                if !used && *k == UiKey::Esc {
                    return Flow::Back;
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
        let tab_w = (w - g - g - m.gap(Space::Md)) / TABS;
        for (i, (label, a, p)) in [("Files", A_TAB_FILES, Pane::Files), ("Assistant", A_TAB_CHAT, Pane::Chat)].into_iter().enumerate() {
            widgets::button(c, th, &m, Rect::new(g + i as f32 * (tab_w + m.gap(Space::Md)), g, tab_w, t), label, a, self.pane == p);
        }
        let top = g + t + m.gap(Space::Lg);
        let body = m.size(TypeRole::Body);
        let status_h = body * 2.0;
        let area = Rect::new(g, top, w - 2.0 * g, (h - top - g - status_h).max(0.0));
        match self.pane {
            Pane::Files => self.files.draw(c, th, &m, area),
            Pane::Chat => self.chat.draw(c, th, &m, area),
        }
        let status = match &self.open {
            Some((p, _)) => format!("Opened: {p}"),
            None => "No file open".to_string(),
        };
        let s = fit(c, body, &status, TextStyle::Regular, area.w);
        c.text(g, h - g - body * 1.5, body, th.muted, &s, TextStyle::Regular);
    }
}
