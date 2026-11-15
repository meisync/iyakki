//! Settings -> Documents (RAG): how found passages are ordered (off, the in-house scorer, or a reranking model of the local Lemonade server)
//! and Graph RAG (passages found through links between names). The same choices as the chat tools `rag_rerank_set` and `rag_graph_set`
//! (F-RAG-14, F-RAG-15; KI-744, KI-745). Administrators only, as the rest of this page: both are choices for the whole computer.
//! `o` cycles off and lexical, `O` names a reranking model (empty: off), `g` switches Graph RAG.

use super::*;
use iyakki_agent::ragraph;
use iyakki_agent::ragrerank::Mode;

impl SettingsApp {
    /// The two rows under the sliders; returns their height (with the gaps) so the rows below move down.
    pub(super) fn render_rag_order(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx, can: bool, two: bool) -> f32 {
        let dir = cx.services.dir();
        let (mode, graph) = (Mode::load(dir), ragraph::is_on(dir));
        let gap = 10.0 * s;
        let rh = 76.0 * s;
        let (cw, x2) = if two { ((a.w - gap) / 2.0, a.x + (a.w - gap) / 2.0 + gap) } else { (a.w, a.x) };
        let y2 = if two { a.y } else { a.y + rh + gap };
        controls::switch_row(g, R::new(a.x, a.y, cw, rh), s, "Graph RAG", "A question that names a person, place or product also finds passages about names linked to it, marked as found through a link.", graph, can, Event::Key(Key::Char('g')), ACCENT);
        let bw = g.text_width(15.0 * s, "Model (O)", Face::Bold) + 15.0 * s * 1.7;
        let cw2 = g.text_width(15.0 * s, "Change (o)", Face::Bold) + 15.0 * s * 1.7;
        let help = match &mode {
            Mode::Off => "The search order stands.".to_string(),
            Mode::Lexical => "An in-house scorer looks at the question's words, their order and how close they sit.".to_string(),
            Mode::Model { .. } => "A reranking model of the local Lemonade server scores them; when it does not answer, the search order stands.".to_string(),
        };
        // a card with the title and the help under it (the plain info row puts the value beside the title, which overlaps when it is long)
        let card = R::new(x2, y2, cw, rh);
        crate::fx::glass(g, card.x, card.y, card.w, card.h, 14.0 * s, crate::fx::CYAN);
        let tw = (cw - bw - cw2 - 40.0 * s).max(60.0 * s);
        let title = g.fit(16.0 * s, &format!("Order of found passages: {}", mode.text()), Face::Bold, tw);
        g.text(x2 + 18.0 * s, y2 + 10.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        for (i, l) in g.wrap(13.0 * s, &help, Face::Sans, cw - 36.0 * s).iter().take(2).enumerate() {
            g.text(x2 + 18.0 * s, y2 + 42.0 * s + i as f32 * 15.0 * s, 13.0 * s, DIM, l, Face::Sans);
        }
        if can {
            g.button(x2 + cw - bw, y2 + 8.0 * s, 15.0 * s, "Model (O)", ACCENT, Event::Key(Key::Char('O')));
            g.button(x2 + cw - bw - cw2 - 8.0 * s, y2 + 8.0 * s, 15.0 * s, "Change (o)", ACCENT, Event::Key(Key::Char('o')));
        }
        if two { rh + gap } else { 2.0 * (rh + gap) }
    }

    pub(super) fn handle_rag_order(&mut self, k: &Key, cx: &AppCx) {
        let dir = cx.services.dir();
        let user = &cx.session.user;
        match k {
            Key::Char('g') => {
                let on = !ragraph::is_on(dir);
                match ragraph::set_on(dir, on) {
                    Ok(()) => {
                        cx.audit.record(user, "settings.rag", &format!("graph {}", if on { "on" } else { "off" }));
                        self.ok(format!("Graph RAG {}.", if on { "on: searches add passages found through links" } else { "off" }));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('o') => {
                let next = if Mode::load(dir) == Mode::Lexical { Mode::Off } else { Mode::Lexical };
                self.save_rerank(next, cx);
            }
            Key::Char('O') => {
                let pre = match Mode::load(dir) {
                    Mode::Model { model } => model,
                    _ => String::new(),
                };
                self.open_form(FormKind::RagRerankModel, Form::new("Reranking model").text("Model name", &pre, "a reranking model of the local Lemonade server (empty: no model)"));
            }
            _ => {}
        }
    }

    fn save_rerank(&mut self, m: Mode, cx: &AppCx) {
        match m.save(cx.services.dir()) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.rag", &format!("rerank {}", m.text()));
                self.ok(format!("Found passages are ordered by: {}.", m.text()));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn rag_rerank_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        let name = form.value(0).trim().to_string();
        let m = if name.is_empty() { Mode::Off } else { Mode::parse("model", &name)? };
        m.save(cx.services.dir())?;
        cx.audit.record(&cx.session.user, "settings.rag", &format!("rerank {}", m.text()));
        Ok(format!("Found passages are ordered by: {}.", m.text()))
    }
}
