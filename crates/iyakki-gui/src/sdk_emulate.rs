//! `iyakki-sdk-emulate-window`: the SDK emulator window (F-SDK-02, docs/SDK-EMULATOR.md). The tile runs in the real WASM sandbox against
//! the fake bridge services of `iyakki-sdk-emulator`; this file only draws the picture, turns the mouse and keyboard into inputs and
//! lists the bridge calls, prompts and grants in a side panel. All behaviour lives in the library, which is tested without a display.

use eframe::egui;
use iyakki_sdk_emulator::emulator::{Emulator, Input, Mode, SIZES};
use iyakki_sdk_emulator::project::Project;
use iyakki_sdk_emulator::watch::Watcher;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct EmuApp {
    emu: Emulator,
    watcher: Watcher,
    tex: Option<egui::TextureHandle>,
    last_tick: Instant,
    status: String,
    error: Option<String>,
    drawn: Option<u64>,
}

fn key_code(k: egui::Key) -> Option<u32> {
    Some(match k {
        egui::Key::Enter => 13,
        egui::Key::Backspace => 8,
        egui::Key::Tab => 9,
        egui::Key::Escape => 27,
        egui::Key::ArrowLeft => 0x100,
        egui::Key::ArrowRight => 0x101,
        egui::Key::ArrowUp => 0x102,
        egui::Key::ArrowDown => 0x103,
        _ => return None,
    })
}

impl EmuApp {
    fn send(&mut self, i: Input) {
        match self.emu.apply(i) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e),
        }
    }

    fn side(&mut self, ui: &mut egui::Ui) {
        let mut restart: Option<(i32, i32)> = None;
        ui.horizontal_wrapped(|ui| {
            for (w, h) in SIZES {
                if ui.selectable_label(self.emu.size == (w, h), format!("{w}x{h}")).clicked() {
                    restart = Some((w, h));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.emu.mode, Mode::Touch, "Touch");
            ui.selectable_value(&mut self.emu.mode, Mode::Mouse, "Mouse");
            let mut dark = self.emu.dark;
            if ui.checkbox(&mut dark, "Dark").changed() {
                self.send(Input::Theme(dark));
            }
        });
        if ui.button("Save picture").clicked() {
            let img = self.emu.image();
            let path = self.emu.project.dir.join("emulator-shot.png");
            self.status = match std::fs::write(&path, iyakki_sdk_emulator::png::encode(img.w as u32, img.h as u32, &img.rgba)) {
                Ok(()) => format!("saved {}", path.display()),
                Err(e) => format!("could not save: {e}"),
            };
        }
        if let Some((w, h)) = restart {
            if let Err(e) = self.emu.restart_at((w, h)) {
                self.error = Some(e);
            }
        }
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        ui.separator();
        ui.strong("Install card would say");
        ui.monospace(self.emu.project.review_text());
        ui.strong("Grants held (manifest)");
        if self.emu.project.caps.is_empty() {
            ui.label("none");
        }
        for c in &self.emu.project.caps {
            ui.monospace(c);
        }
        ui.strong("Prompts the tile triggered");
        for p in self.emu.services.prompts() {
            ui.monospace(format!("{}.{}: {} -> {}", p.domain, p.method, p.words, if p.granted { "allowed (emulated)" } else { "refused (emulated)" }));
        }
        ui.strong("Bridge calls (newest first, secrets redacted)");
        egui::ScrollArea::vertical().id_salt("calls").max_height(f32::INFINITY).show(ui, |ui| {
            for c in self.emu.services.calls().iter().rev().take(200) {
                let color = if c.refused { egui::Color32::LIGHT_RED } else if !c.ok { egui::Color32::YELLOW } else { ui.visuals().text_color() };
                ui.colored_label(color, egui::RichText::new(format!("#{} {}.{} {}\n    -> {}", c.seq, c.domain, c.method, c.args, c.reply)).monospace());
            }
        });
        ui.separator();
        ui.weak("Emulated: no real sound, camera, GPU or network; timing differs from a device; text uses the reference 5x7 font.");
    }
}

impl eframe::App for EmuApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.watcher.changed() {
            self.status = match self.emu.reload() {
                Ok(m) => m,
                Err(e) => format!("reload failed, the old tile keeps running: {e}"),
            };
            self.drawn = None;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_millis() as u32;
        if dt >= 16 && self.emu.refused().is_none() {
            self.last_tick = now;
            self.send(Input::Tick(dt.min(100)));
        }
        for ev in ctx.input(|i| i.events.clone()) {
            match ev {
                egui::Event::Text(t) => self.send(Input::Text(t)),
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    if let Some(code) = key_code(key) {
                        self.send(Input::Key { code, mods: u8::from(modifiers.shift) | (u8::from(modifiers.ctrl) << 1) | (u8::from(modifiers.alt) << 2) });
                    }
                }
                _ => {}
            }
        }
        egui::Panel::right("side").min_size(380.0).show(ui, |ui| self.side(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                let img = self.emu.image();
                let key = img.rgba.iter().step_by(97).fold(img.w as u64 * 31 + img.h as u64, |a, b| a.wrapping_mul(131).wrapping_add(*b as u64));
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.rgba);
                match &mut self.tex {
                    Some(t) if self.drawn == Some(key) => {
                        let _ = t;
                    }
                    Some(t) => t.set(ci, egui::TextureOptions::NEAREST),
                    None => self.tex = Some(ctx.load_texture("tile", ci, egui::TextureOptions::NEAREST)),
                }
                self.drawn = Some(key);
                let size = egui::vec2(img.w as f32, img.h as f32);
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                if let Some(t) = &self.tex {
                    ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                let at = |p: egui::Pos2| ((p.x - rect.min.x) as i32, (p.y - rect.min.y) as i32);
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let (x, y) = at(p);
                        self.send(Input::Tap(x, y));
                    }
                } else if self.emu.mode == Mode::Mouse && resp.hovered() && ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                    if let Some(p) = resp.hover_pos() {
                        let (x, y) = at(p);
                        self.send(Input::Move(x, y));
                    }
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(tile) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: iyakki-sdk-emulate-window TILE_FOLDER [--size WxH] [--mouse] [--dark]");
        std::process::exit(2);
    };
    let size = args.iter().position(|a| a == "--size").and_then(|i| args.get(i + 1)).map(|s| iyakki_sdk_emulator::cli::parse_size(s)).transpose().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let project = Project::load(std::path::Path::new(tile)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let scratch: PathBuf = project.dir.join(".emulator-scratch");
    let mode = if args.iter().any(|a| a == "--mouse") { Mode::Mouse } else { Mode::Touch };
    println!("{}", project.review_text());
    let watcher = Watcher::new(project.watched());
    let emu = Emulator::new(project, scratch, size.unwrap_or(SIZES[0]), mode, args.iter().any(|a| a == "--dark")).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 760.0]).with_title("IYAKKI tile emulator"), ..Default::default() };
    eframe::run_native("IYAKKI tile emulator", opts, Box::new(move |_| Ok(Box::new(EmuApp { emu, watcher, tex: None, last_tick: Instant::now(), status: String::new(), error: None, drawn: None }))))
}
