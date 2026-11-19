use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

const A_NAME: u32 = 1;
const A_SAVE: u32 = 2;

/// The rule is a plain function: the test calls it without a screen.
pub fn check_name(s: &str) -> Result<(), &'static str> {
    if s.trim().is_empty() { return Err("Give a name."); }
    if s.chars().count() > 30 { return Err("That name is too long (30 letters at most)."); }
    Ok(())
}

pub struct FormApp { pub name: String, pub saved: Option<String>, focused: bool, said: String }

impl FormApp {
    pub fn new() -> FormApp { FormApp { name: String::new(), saved: None, focused: false, said: String::new() } }
}

impl UiApp for FormApp {
    fn id(&self) -> &'static str { "form-demo" }
    fn title(&self) -> String { "Form demo".into() }
    fn wants_keyboard(&self) -> bool { self.focused }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(A_NAME) => self.focused = true,
            UiEvent::Text(t) if self.focused => self.name.push_str(t),
            UiEvent::Key(UiKey::Backspace) if self.focused => { self.name.pop(); }
            UiEvent::Action(A_SAVE) | UiEvent::Key(UiKey::Enter) => match check_name(&self.name) {
                Ok(()) => { self.saved = Some(self.name.clone()); self.said = "Saved.".into(); }
                Err(why) => self.said = why.into(), // say why, in the person's words, next to the field
            },
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, _) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        let width = w - 2.0 * g;
        let fh = m.min_target();
        widgets::field(c, th, &m, Rect::new(g, g, width, fh), "Name", &self.name, self.focused, A_NAME);
        widgets::button(c, th, &m, Rect::new(g, g + fh + m.gap(Space::Md), width, fh), "Save", A_SAVE, true);
        if !self.said.is_empty() {
            let size = m.size(TypeRole::Body);
            let line = fit(c, size, &self.said, TextStyle::Regular, width);
            c.text(g, g + 2.0 * fh + 2.0 * m.gap(Space::Md), size, th.muted, &line, TextStyle::Regular);
        }
    }
}
