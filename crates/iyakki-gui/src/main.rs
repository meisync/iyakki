//! `iyakki-editor`: egui front-end for the agentic editor (build with `--features gui`).
//! eframe picks Wayland natively when available, otherwise X11, and renders on the GPU.

use iyakki_ui::{highlight_rust, Editor, TokenKind};
use eframe::egui;

struct App {
    ed: Editor,
    prompt: String,
}

fn color(k: TokenKind) -> egui::Color32 {
    match k {
        TokenKind::Keyword => egui::Color32::from_rgb(197, 134, 192),
        TokenKind::String => egui::Color32::from_rgb(206, 145, 120),
        TokenKind::Comment => egui::Color32::from_rgb(106, 153, 85),
        TokenKind::Number => egui::Color32::from_rgb(181, 206, 168),
        TokenKind::Type => egui::Color32::from_rgb(78, 201, 176),
        TokenKind::Function => egui::Color32::from_rgb(220, 220, 170),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("console").min_size(140.0).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for l in &self.ed.console.lines {
                    ui.monospace(l);
                }
            });
            let r = ui.add(egui::TextEdit::singleline(&mut self.prompt).hint_text("prompt…").desired_width(f32::INFINITY));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.prompt.is_empty() {
                let p = std::mem::take(&mut self.prompt);
                self.ed.console.submit_prompt(&p);
                r.request_focus();
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let is_rs = self.ed.path.ends_with(".rs");
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _w: f32| {
                let src = text.as_str();
                let mut job = egui::text::LayoutJob::default();
                let font = egui::FontId::monospace(14.0);
                let default = ui.visuals().text_color();
                let mut pos = 0;
                let spans = if is_rs { highlight_rust(src) } else { Vec::new() };
                for s in spans {
                    if s.start > pos {
                        job.append(&src[pos..s.start], 0.0, egui::TextFormat::simple(font.clone(), default));
                    }
                    job.append(&src[s.start..s.end], 0.0, egui::TextFormat::simple(font.clone(), color(s.kind)));
                    pos = s.end;
                }
                job.append(&src[pos..], 0.0, egui::TextFormat::simple(font, default));
                ui.fonts_mut(|f| f.layout_job(job))
            };
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.ed.text).code_editor().desired_width(f32::INFINITY).layouter(&mut layouter));
            });
        });
    }
}

fn main() -> eframe::Result {
    let mut ed = Editor::new("/scratch.rs");
    ed.text = "fn main() {\n    println!(\"hello, IYAKKI\");\n}\n".into();
    ed.console.log("iyakki-editor ready");
    eframe::run_native("IYAKKI Editor", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(App { ed, prompt: String::new() }))))
}
