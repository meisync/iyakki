//! `iyakki-editor`: egui front-end for the agentic editor (build with `--features gui`).
//! eframe picks Wayland natively when available, otherwise X11, and renders on the GPU.

use eframe::egui;
use iyakki_edit::{highlight_text, lang_for_path, Lang, TokenKind};

/// What the window keeps: the text being edited (its path picks the language), the console lines and the prompt box.
struct App {
    path: String,
    text: String,
    console: Vec<String>,
    prompt: String,
}

impl App {
    fn log(&mut self, s: impl Into<String>) {
        self.console.push(s.into());
        if self.console.len() > 5000 {
            self.console.drain(..1000);
        }
    }
}

fn color(k: TokenKind) -> Option<egui::Color32> {
    Some(match k {
        TokenKind::Keyword => egui::Color32::from_rgb(197, 134, 192),
        TokenKind::String => egui::Color32::from_rgb(206, 145, 120),
        TokenKind::Comment => egui::Color32::from_rgb(106, 153, 85),
        TokenKind::Number => egui::Color32::from_rgb(181, 206, 168),
        TokenKind::Type => egui::Color32::from_rgb(78, 201, 176),
        TokenKind::Function | TokenKind::Macro => egui::Color32::from_rgb(220, 220, 170),
        TokenKind::Constant => egui::Color32::from_rgb(181, 206, 168),
        TokenKind::Attribute | TokenKind::Key => egui::Color32::from_rgb(156, 220, 254),
        TokenKind::Heading => egui::Color32::from_rgb(86, 156, 214),
        TokenKind::Variable => return None,
    })
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("console").min_size(140.0).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for l in &self.console {
                    ui.monospace(l);
                }
            });
            let r = ui.add(egui::TextEdit::singleline(&mut self.prompt).hint_text("prompt…").desired_width(f32::INFINITY));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.prompt.is_empty() {
                let p = std::mem::take(&mut self.prompt);
                self.log(format!("> {p}"));
                r.request_focus();
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let lang = lang_for_path(&self.path);
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _w: f32| {
                let src = text.as_str();
                let mut job = egui::text::LayoutJob::default();
                let font = egui::FontId::monospace(14.0);
                let default = ui.visuals().text_color();
                let mut pos = 0;
                let spans = if lang == Lang::Plain { Vec::new() } else { highlight_text(lang, src) };
                for s in spans {
                    if s.start > pos {
                        job.append(&src[pos..s.start], 0.0, egui::TextFormat::simple(font.clone(), default));
                    }
                    job.append(&src[s.start..s.end], 0.0, egui::TextFormat::simple(font.clone(), color(s.kind).unwrap_or(default)));
                    pos = s.end;
                }
                job.append(&src[pos..], 0.0, egui::TextFormat::simple(font, default));
                ui.fonts_mut(|f| f.layout_job(job))
            };
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_width(f32::INFINITY).layouter(&mut layouter));
            });
        });
    }
}

fn main() -> eframe::Result {
    let mut app = App { path: "/scratch.rs".into(), text: "fn main() {\n    println!(\"hello, IYAKKI\");\n}\n".into(), console: Vec::new(), prompt: String::new() };
    app.log("iyakki-editor ready");
    eframe::run_native("IYAKKI Editor", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(app))))
}
