use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::editorview::EditorView;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const EDIT: u32 = 400; // EditorView uses EDIT .. EDIT + editorview::ACTIONS

pub struct EditorApp {
    editor: EditorView,
    /// What the person saved (the app decides where it goes: through a port, never a file).
    pub saved: Option<(String, String)>,
}

impl EditorApp {
    pub fn new(path: &str, text: &str) -> EditorApp {
        let mut editor = EditorView::new(EDIT);
        let _ = editor.load(path, text); // Err: too big or binary, the view shows the sentence
        EditorApp { editor, saved: None }
    }
}

impl UiApp for EditorApp {
    fn id(&self) -> &'static str { "editor-demo" }
    fn title(&self) -> String { "Editor demo".into() }
    fn wants_keyboard(&self) -> bool { self.editor.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(a) if self.editor.owns(*a) => { self.editor.action(*a); }
            UiEvent::Text(t) => { self.editor.text_in(t); }
            UiEvent::Wheel(n) => self.editor.wheel(*n),
            UiEvent::Key(k) => { if !self.editor.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        if let Some(s) = self.editor.take_save() {
            self.saved = Some(s);
            self.editor.mark_saved();
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.editor.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
