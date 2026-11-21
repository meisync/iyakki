//! `iyakki-writer`: the USB writer window (F-PKG-08): drag the image onto it (or type its path), pick the stick, press one button.
//! Every rule is in `iyakki_usb::writerwin` (the state machine, tested without a screen); this file only draws its `View`, turns
//! clicks and drops into `Event`s, refreshes the drive list, and runs the write on its own thread (`iyakki_usb::writerwin_run`).

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use iyakki_usb::writerwin::{drop_event, Action, Event, Page, Window};
use iyakki_usb::filepick::Picker;
use iyakki_usb::writerwin_run::{platform_steps, spawn};

struct App {
    win: Window,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    last_scan: Option<Instant>,
    typed: String,
    note: String,
    show_all: bool,
    /// The in-window file picker (open while the person chooses a file).
    picker: Option<Picker>,
}

impl App {
    fn new() -> App {
        let (tx, rx) = channel();
        App { win: Window::new(), tx, rx, last_scan: None, typed: String::new(), note: String::new(), show_all: false, picker: None }
    }

    fn send(&mut self, e: Event) {
        match self.win.handle(e) {
            Action::None => {}
            Action::Close => std::process::exit(0),
            a @ Action::Write { .. } => {
                // the signature of the new disk label: any four bytes that differ per stick
                let sig = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos()).to_le_bytes();
                match platform_steps(sig) {
                    Ok(steps) => {
                        if spawn(&a, true, self.tx.clone(), steps).is_none() {
                            self.note = "the write could not be started".into();
                        }
                    }
                    Err(e) => self.note = e,
                }
            }
        }
    }

    fn refresh(&mut self) {
        if self.last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) && !matches!(self.win.page(), Page::Writing) {
            self.last_scan = Some(Instant::now());
            match iyakki_usb::drives::list_drives() {
                Ok(d) => self.send(Event::Drives(d)),
                Err(e) => self.note = e,
            }
        }
        while let Ok(e) = self.rx.try_recv() {
            self.send(e);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.refresh();
        ui.ctx().request_repaint_after(Duration::from_millis(250));
        let dropped: Vec<Option<std::path::PathBuf>> = ui.input(|i| i.raw.dropped_files.iter().map(|f| Some(f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty())).collect());
        if !dropped.is_empty() {
            match drop_event(&dropped) {
                Ok(e) => {
                    self.note.clear();
                    self.send(e);
                }
                Err(m) => self.note = m,
            }
        }
        let v = self.win.view();
        let mut events: Vec<Event> = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(&v.title);
            for l in &v.lines {
                ui.label(l);
            }
            if v.page == Page::Image {
                ui.label("Drop the image file on this window, or type its path:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.typed);
                    if ui.button("Use this file").clicked() && !self.typed.trim().is_empty() {
                        events.push(Event::ImageChosen(self.typed.trim().into()));
                    }
                    if ui.button("Browse…").clicked() {
                        let start = iyakki_usb::filepick::start_places().into_iter().next().unwrap_or_else(|| std::path::PathBuf::from("/"));
                        self.picker = Some(Picker::new(&start));
                    }
                });
                if let Some(p) = &mut self.picker {
                    let mut chosen = None;
                    let mut close = false;
                    ui.group(|ui| {
                        ui.label(format!("Folder: {}", p.dir().display()));
                        ui.horizontal(|ui| {
                            if ui.button("Up").clicked() {
                                p.up();
                            }
                            for r in iyakki_usb::filepick::start_places() {
                                if ui.button(r.display().to_string()).clicked() {
                                    p.go(&r);
                                }
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        });
                        if let Some(e) = &p.error {
                            ui.colored_label(egui::Color32::from_rgb(220, 160, 60), e);
                        }
                        let rows: Vec<(usize, String)> = p.entries().iter().enumerate().map(|(i, e)| (i, if e.is_dir { format!("[folder] {}", e.name) } else { format!("{}   {} bytes", e.name, e.size) })).collect();
                        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            for (i, label) in rows {
                                if ui.button(label).clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                    });
                    if let Some(i) = chosen {
                        if let Some(path) = p.open(i) {
                            events.push(Event::ImageChosen(path));
                            close = true;
                        }
                    }
                    if close {
                        self.picker = None;
                    }
                }
            }
            for (i, r) in v.rows.iter().enumerate() {
                ui.horizontal(|ui| {
                    let text = format!("{}   {}", r.label, r.detail);
                    if ui.add_enabled(r.selectable, egui::Button::selectable(r.selected, text)).clicked() {
                        events.push(Event::PickRow(i));
                    }
                    if let Some(why) = &r.reason {
                        ui.weak(why);
                    }
                });
            }
            if v.page == Page::Stick {
                if ui.checkbox(&mut self.show_all, "Show all disks (including this computer's own)").changed() {
                    events.push(Event::ShowAllDisks(self.show_all));
                }
            }
            if let Some((label, on)) = &v.tick {
                let mut t = *on;
                if ui.checkbox(&mut t, label).changed() {
                    events.push(Event::Acknowledge(t));
                }
            }
            if let Some((label, frac)) = &v.progress {
                ui.add(egui::ProgressBar::new(*frac as f32).text(label.clone()));
            }
            ui.horizontal(|ui| {
                if v.can_go_back && ui.button("Back").clicked() {
                    events.push(Event::Back);
                }
                if let Some(b) = &v.button {
                    if ui.add_enabled(b.enabled, egui::Button::new(b.label)).clicked() {
                        events.push(Event::Button);
                    }
                }
            });
            if !self.note.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), &self.note);
            }
        });
        for e in events {
            self.send(e);
        }
    }
}

fn main() -> eframe::Result {
    // Windows: the administrator prompt first (the program starts again elevated and this one ends)
    match iyakki_usb::writerwin_run::ensure_admin() {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => eprintln!("iyakki-writer: {e}"),
    }
    eframe::run_native("IYAKKI USB writer", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(App::new()))))
}
