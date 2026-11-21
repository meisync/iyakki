//! `iyakki-installer`: the window installer for Linux desktops. Pages: Image, Drive, Mode, Plan, Confirm, Progress, Done.
//!
//! All decisions are the library's (`iyakki_usb::wizard` drives the pages, `iyakki_usb::planner` makes and checks the plan). This
//! file only draws them. The window never runs as root and never sees a password: reading the drives and writing the plan are done
//! by `iyakki-usb`, started through `pkexec` (polkit asks the person in the desktop's own dialog). See docs/INSTALLER.md.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use eframe::egui;
use iyakki_usb::elevate::{self, Elevation, Event};
use iyakki_usb::fetch;
use iyakki_usb::json::Json;
use iyakki_usb::planner::{DiskModel, Mode, SourceSpec};
use iyakki_usb::ui::human_size;
use iyakki_usb::wizard::{Next, Page, WKey, Wizard};

#[derive(Default)]
struct Shared {
    /// Result of the drive scan.
    scan: Option<Result<(SourceSpec, bool, Vec<DiskModel>), String>>,
    scanning: bool,
    /// Image download: (done, total), finished path or error.
    dl: Option<(u64, u64)>,
    dl_done: Option<Result<(PathBuf, String), String>>,
    /// Image checksum check of a local file.
    check_note: Option<String>,
    /// The writer: phase, done, total; finished result.
    wr: Option<(String, u64, u64)>,
    wr_done: Option<Result<String, String>>,
    /// The shrink command's answer: the plan text or the result of the shrink.
    shrink_out: Option<(bool, Result<String, String>)>,
}

#[derive(Default)]
struct ShrinkUi {
    open: bool,
    part: Option<u32>,
    mib: String,
    plan: Option<String>,
    typed: String,
    ack: bool,
    busy: bool,
    done: Option<Result<String, String>>,
}

struct App {
    shared: Arc<Mutex<Shared>>,
    image_path: String,
    image_url: String,
    status: String,
    elevation: Elevation,
    writer: Option<PathBuf>,
    wiz: Option<Wizard>,
    /// The shrink step on the Mode page (F-INST-30): picked partition, size, the plan text and the second step.
    shrink: ShrinkUi,
    work: PathBuf,
    plan_file: PathBuf,
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_else(std::env::temp_dir).join("iyakki")
}

impl App {
    fn new() -> App {
        let path = std::env::var("PATH").unwrap_or_default();
        let graphical = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let own = std::env::current_exe().unwrap_or_default();
        // a private working folder for the plan file (owner only)
        let work = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(format!("iyakki-installer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&work);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700));
        }
        let plan_file = work.join("plan.json");
        let found = std::fs::read_dir(".").ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("iyakki") && n.to_string_lossy().contains("install") && n.to_string_lossy().ends_with(".img")));
        App { shared: Arc::new(Mutex::new(Shared::default())), image_path: found.map(|p| p.display().to_string()).unwrap_or_default(), image_url: String::new(), status: String::new(), elevation: elevate::find_elevation(&path, graphical), writer: elevate::writer_program(&own, &path), wiz: None, shrink: ShrinkUi::default(), work, plan_file }
    }

    fn scan(&mut self) {
        let Some(writer) = self.writer.clone() else {
            self.status = "iyakki-usb was not found next to this program or on the PATH.".into();
            return;
        };
        let image = PathBuf::from(self.image_path.trim());
        if !image.is_file() {
            self.status = format!("{} is not a file.", image.display());
            return;
        }
        let Some(cmd) = elevate::list_command(&self.elevation, &writer, Some(&image)) else {
            self.status = format!("No graphical administrator prompt (polkit) was found. Start this program from a terminal as administrator instead: sudo {}", std::env::current_exe().unwrap_or_default().display());
            return;
        };
        self.status = "Reading the drives (the administrator prompt asks for your password; this program never sees it)...".into();
        let sh = self.shared.clone();
        sh.lock().unwrap().scanning = true;
        std::thread::spawn(move || {
            let r = run_capture(&cmd).and_then(|out| {
                let j = Json::parse(&out).map_err(|e| format!("the drive list is not readable: {e}"))?;
                let src = SourceSpec::from_json(j.get("source").ok_or("no source in the drive list")?)?;
                let uefi = j.get("uefi").and_then(Json::as_bool).unwrap_or(false);
                let disks = j.get("disks").and_then(Json::as_arr).ok_or("no drives in the list")?.iter().map(DiskModel::from_json).collect::<Result<Vec<_>, _>>()?;
                Ok((src, uefi, disks))
            });
            let mut s = sh.lock().unwrap();
            s.scan = Some(r);
            s.scanning = false;
        });
    }

    fn download(&mut self) {
        let url = self.image_url.trim().to_string();
        if let Err(e) = fetch::url_allowed(&url) {
            self.status = e;
            return;
        }
        let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("iyakki-install.img").to_string();
        let dest = cache_dir().join(name);
        let sh = self.shared.clone();
        {
            let mut s = sh.lock().unwrap();
            s.dl = Some((0, 0));
            s.dl_done = None;
        }
        self.status = format!("Downloading to {} ...", dest.display());
        std::thread::spawn(move || {
            let st = iyakki_proxy::https::UreqStreamer::default();
            let open = |u: &str| -> Result<fetch::Head, String> {
                use iyakki_proxy::Streamer as _;
                let h = st.open(u, &[])?;
                Ok(fetch::Head { status: h.status, headers: h.headers, body: h.body })
            };
            let sh2 = sh.clone();
            let r = fetch::download(&open, &url, &dest, 4 << 30, &mut |d, t| sh2.lock().unwrap().dl = Some((d, t)));
            let mut s = sh.lock().unwrap();
            s.dl = None;
            s.dl_done = Some(r.map(|d| (dest, match d.verified { Some(true) => format!("{} downloaded; the checksum matches the published .sha256", human_size(d.bytes)), _ => format!("{} downloaded; no .sha256 was published, so it is NOT verified", human_size(d.bytes)) })));
        });
    }

    /// A local file with a `.sha256` next to it is checked before it is used.
    fn check_local(&mut self) {
        let image = PathBuf::from(self.image_path.trim());
        let sh = self.shared.clone();
        std::thread::spawn(move || {
            let note = match iyakki_usb::writer::read_sidecar(&image) {
                None => "no .sha256 next to the file: not verified".to_string(),
                Some(want) => match iyakki_usb::writer::file_sha256(&image, &mut |_| {}) {
                    Ok(got) if got == want => "the checksum matches the .sha256 next to the file".to_string(),
                    Ok(_) => "the file does NOT match its .sha256: do not use it".to_string(),
                    Err(e) => e,
                },
            };
            sh.lock().unwrap().check_note = Some(note);
        });
    }

    fn start_write(&mut self) {
        let Some(wiz) = self.wiz.as_mut() else { return };
        let Some(plan) = wiz.plan.clone() else { return };
        let (Some(writer), Some(cmd)) = (self.writer.clone(), self.writer.as_ref().and_then(|w| elevate::apply_command(&self.elevation, w, &self.plan_file))) else {
            wiz.finished(Err("iyakki-usb was not found.".into()));
            return;
        };
        if let Err(e) = iyakki_usb::plan_cli::write_plan_file(&self.plan_file, &plan, wiz.confirmation().as_ref()) {
            wiz.finished(Err(e));
            return;
        }
        let _ = writer;
        let sh = self.shared.clone();
        sh.lock().unwrap().wr = Some(("starting".into(), 0, 1));
        sh.lock().unwrap().wr_done = None;
        std::thread::spawn(move || {
            let child = Command::new(&cmd[0]).args(&cmd[1..]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    sh.lock().unwrap().wr_done = Some(Err(format!("could not start the writer: {e}")));
                    return;
                }
            };
            let mut notes = Vec::new();
            let mut written = 0;
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    match elevate::parse_line(&line) {
                        Some(Event::Progress { phase, done, total }) => sh.lock().unwrap().wr = Some((phase, done, total)),
                        Some(Event::Done(n)) => written = n,
                        Some(Event::Note(n)) => notes.push(n),
                        None => {}
                    }
                }
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let status = child.wait();
            let ok = status.as_ref().is_ok_and(|s| s.success());
            let r = if ok {
                Ok(format!("{} written and verified.\n{}\n\nThe first start of the new system formats its data partition (a few seconds) and asks for the first account. Restart and open the firmware's boot menu (F12, F10, Esc or Option at power-on, depending on the make), then choose IYAKKI. Your other system still starts as before.", human_size(written), notes.join("\n")))
            } else if err.contains("dismissed") || err.contains("Not authorized") || status.as_ref().is_ok_and(|s| s.code() == Some(126) || s.code() == Some(127)) {
                Err("The administrator prompt was cancelled or refused. Nothing was written.".to_string())
            } else {
                Err(if err.trim().is_empty() { "the writer stopped without a message".to_string() } else { err.trim().to_string() })
            };
            sh.lock().unwrap().wr_done = Some(r);
        });
    }

    fn poll(&mut self) {
        let out = self.shared.lock().unwrap().shrink_out.take();
        if let Some((applying, r)) = out {
            self.shrink.busy = false;
            if applying {
                self.shrink.plan = None;
                self.shrink.ack = false;
                self.shrink.typed.clear();
                self.shrink.done = Some(r.map(|t| format!("{}\nGo back to the Drive page and read the drives again to see the new free space.", t.trim())));
            } else {
                match r {
                    Ok(t) => self.shrink.plan = Some(t),
                    Err(e) => self.shrink.done = Some(Err(e)),
                }
            }
        }
        let mut s = self.shared.lock().unwrap();
        if let Some(r) = s.scan.take() {
            match r {
                Ok((src, uefi, disks)) => {
                    drop(s);
                    self.status = format!("{} drive(s) found.", disks.len());
                    self.wiz = Some(Wizard::new(disks, src, uefi));
                    return;
                }
                Err(e) => self.status = e,
            }
        }
        if let Some(r) = s.dl_done.take() {
            match r {
                Ok((p, note)) => {
                    self.image_path = p.display().to_string();
                    self.status = note;
                }
                Err(e) => self.status = e,
            }
        }
        if let Some(n) = s.check_note.take() {
            self.status = n;
        }
        let progress = s.wr.clone();
        let done = s.wr_done.take();
        drop(s);
        if let Some(wiz) = self.wiz.as_mut() {
            if let Some(p) = progress {
                if wiz.page == Page::Progress {
                    wiz.progress = (p.0, p.1, p.2);
                }
            }
            if let Some(r) = done {
                wiz.finished(r);
                let _ = std::fs::remove_file(&self.plan_file);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

fn run_capture(cmd: &[String]) -> Result<String, String> {
    let out = Command::new(&cmd[0]).args(&cmd[1..]).output().map_err(|e| format!("could not start {}: {e}", cmd[0]))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(if err.trim().is_empty() { "the administrator prompt was cancelled or refused".into() } else { err.trim().to_string() });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run the shrink command (plan when `confirm` is None) through the administrator prompt, off the interface thread.
fn run_shrink(ui_state: &mut ShrinkUi, writer: &Option<PathBuf>, elevation: &Elevation, shared: &Arc<Mutex<Shared>>, disk: &str, part: u32, mib: u64, confirm: Option<String>) {
    let Some(writer) = writer.clone() else {
        ui_state.done = Some(Err("iyakki-usb was not found.".into()));
        return;
    };
    let Some(cmd) = elevate::shrink_command(elevation, &writer, disk, part, mib, confirm.as_deref()) else {
        ui_state.done = Some(Err("No graphical administrator prompt (polkit) was found. Run `iyakki-usb --shrink` in a terminal as administrator (docs/INSTALLER.md).".into()));
        return;
    };
    ui_state.busy = true;
    let sh = shared.clone();
    let applying = confirm.is_some();
    std::thread::spawn(move || {
        let r = run_capture(&cmd);
        sh.lock().unwrap().shrink_out = Some((applying, r));
    });
}

const STEPS: [&str; 7] = ["Image", "Drive", "Mode", "Plan", "Confirm", "Progress", "Done"];

fn step_of(w: &Option<Wizard>) -> usize {
    match w.as_ref().map(|w| w.page) {
        None => 0,
        Some(Page::Drive) => 1,
        Some(Page::Mode | Page::Choose | Page::Sizes | Page::Shrink) => 2,
        Some(Page::Plan) => 3,
        Some(Page::Confirm) => 4,
        Some(Page::Progress) => 5,
        Some(Page::Done | Page::Failed) => 6,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        egui::Panel::top("steps").show(ui, |ui| {
            ui.horizontal(|ui| {
                let now = step_of(&self.wiz);
                for (i, s) in STEPS.iter().enumerate() {
                    let t = format!("{}. {s}", i + 1);
                    if i == now { ui.strong(t) } else { ui.weak(t) };
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.page(ui));
        });
    }
}

impl App {
    fn page(&mut self, ui: &mut egui::Ui) {
        let red = egui::Color32::from_rgb(220, 80, 80);
        let green = egui::Color32::from_rgb(90, 190, 120);
        if self.wiz.is_none() {
            ui.heading("Install IYAKKI");
            ui.label("Choose the IYAKKI install image (iyakki-install-x86_64.img): a file on this computer, or a link to download it. Nothing is changed on any drive until the last page.");
            ui.add_space(8.0);
            ui.label("File:");
            ui.add(egui::TextEdit::singleline(&mut self.image_path).desired_width(f32::INFINITY).hint_text("/path/to/iyakki-install-x86_64.img"));
            let (scanning, dl) = {
                let s = self.shared.lock().unwrap();
                (s.scanning, s.dl)
            };
            ui.horizontal(|ui| {
                if ui.add_enabled(!scanning && !self.image_path.trim().is_empty(), egui::Button::new("Check the file's checksum")).clicked() {
                    self.check_local();
                }
                if ui.add_enabled(!scanning && !self.image_path.trim().is_empty(), egui::Button::new("Next: choose the drive")).clicked() {
                    self.scan();
                }
            });
            ui.add_space(8.0);
            ui.label("or a link (https://, with a .sha256 beside it for a checksum check):");
            ui.add(egui::TextEdit::singleline(&mut self.image_url).desired_width(f32::INFINITY).hint_text("https://.../iyakki-install-x86_64.img"));
            if ui.add_enabled(dl.is_none() && !self.image_url.trim().is_empty(), egui::Button::new("Download")).clicked() {
                self.download();
            }
            if let Some((d, t)) = dl {
                let f = if t == 0 { 0.0 } else { d as f32 / t as f32 };
                ui.add(egui::ProgressBar::new(f).text(format!("{} of {}", human_size(d), if t == 0 { "?".into() } else { human_size(t) })));
            }
            if scanning {
                ui.add(egui::Spinner::new());
            }
            if !self.status.is_empty() {
                ui.add_space(8.0);
                ui.label(&self.status);
            }
            if matches!(self.elevation, Elevation::Manual) {
                ui.colored_label(red, "No graphical administrator prompt (polkit/pkexec) was found. Start this program with sudo from a terminal, or use the terminal installer: sudo iyakki-usb --plan");
            }
            return;
        }
        let mut next = Next::Nothing;
        let wiz = self.wiz.as_mut().unwrap();
        match wiz.page {
            Page::Drive => {
                ui.heading("Choose the drive");
                ui.label(format!("Image: {} (boot volume {}).", wiz.source.path.display(), human_size(wiz.source.boot_bytes)));
                ui.add_space(6.0);
                for i in 0..wiz.disks.len() {
                    let d = wiz.disks[i].clone();
                    let reason = Wizard::disk_reason_of(&d);
                    let kind = if d.usb { "USB" } else if d.removable { "removable" } else { "internal" };
                    let free: u64 = d.free_gaps(d.table.unwrap_or(iyakki_usb::gpt_install::TableKind::Gpt)).iter().map(|g| g.bytes(d.sector)).sum();
                    let text = format!("{}   {}   {}   {}   {} partition(s), {} free{}{}", d.path, human_size(d.size_bytes), d.model, kind, d.parts.len(), human_size(free), if d.running_system { "   (holds the running system)" } else { "" }, reason.as_ref().map(|r| format!("   NOT USABLE: {r}")).unwrap_or_default());
                    if ui.selectable_label(wiz.disk_sel == i, text).clicked() {
                        wiz.disk_sel = i;
                    }
                    if wiz.disk_sel == i {
                        for p in &d.parts {
                            ui.monospace(format!("      {:<12} {:>9}  {:<20} {} {}", p.name, human_size(p.bytes(d.sector)), p.kind, p.fs, p.in_use().map(|m| format!("[{m}]")).unwrap_or_default()));
                        }
                    }
                }
                if let Some(e) = &wiz.error {
                    ui.colored_label(red, e);
                }
                if ui.button("Next").clicked() {
                    next = wiz.handle(WKey::Enter);
                }
            }
            Page::Mode | Page::Choose | Page::Sizes | Page::Shrink => {
                ui.heading("What to do with the drive");
                ui.label(format!("{} ({})", wiz.disk().path, human_size(wiz.disk().size_bytes)));
                ui.add_space(6.0);
                for (i, m) in Mode::ALL.iter().enumerate() {
                    let avail = wiz.mode_available(*m);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(avail.is_ok(), egui::RadioButton::new(wiz.mode_sel == i, m.title())).clicked() {
                            wiz.mode_sel = i;
                            wiz.page = Page::Mode;
                            wiz.start_mode();
                        }
                        if let Err(e) = &avail {
                            ui.colored_label(red, format!("unavailable: {e}"));
                        }
                    });
                }
                ui.checkbox(&mut wiz.req.keep_esp, "Dual boot: keep the existing EFI System Partition (only the folder EFI/IYAKKI and a boot entry are added)");
                let mode = wiz.mode();
                // legacy BIOS boot of a shared disk (F-INST-13): the old boot sectors are saved first, `iyakki-usb --bios-restore` puts them back
                if mode != Mode::WholeDisk {
                    ui.checkbox(&mut wiz.req.bios, "Also start this disk on older PCs through legacy BIOS (the old boot sectors are saved to a file first)");
                    if wiz.req.bios {
                        ui.checkbox(&mut wiz.req.bios_replace, "Allow replacing another system's boot code (it must then be chained from IYAKKI's menu; the save file undoes it)");
                    } else {
                        wiz.req.bios_replace = false;
                    }
                }
                ui.add_space(6.0);
                if mode == Mode::WholeDisk {
                    ui.colored_label(red, "Everything on the drive will be erased.");
                } else if mode == Mode::Reuse {
                    ui.label("Choose the BOOT partition (formatted FAT32) and the DATA partition (formatted):");
                    let d = wiz.disk().clone();
                    for p in &d.parts {
                        ui.horizontal(|ui| {
                            let usable = !p.extended && p.in_use().is_none();
                            ui.monospace(format!("{:<12} {:>9}  {:<20} {} {}", p.name, human_size(p.bytes(d.sector)), p.kind, p.fs, p.in_use().map(|m| format!("[{m}]")).unwrap_or_default()));
                            if ui.add_enabled_ui(usable && !wiz.req.keep_esp, |ui| ui.selectable_label(wiz.req.boot_part == Some(p.index), "BOOT")).inner.clicked() {
                                wiz.req.boot_part = Some(p.index);
                            }
                            if ui.add_enabled_ui(usable, |ui| ui.selectable_label(wiz.req.data_part == Some(p.index), "DATA")).inner.clicked() {
                                wiz.req.data_part = Some(p.index);
                            }
                        });
                    }
                } else {
                    ui.label("Free space to use:");
                    let d = wiz.disk().clone();
                    let gaps = d.free_gaps(d.table.unwrap_or(iyakki_usb::gpt_install::TableKind::Gpt));
                    if gaps.is_empty() {
                        ui.colored_label(red, "This drive has no free space. Shrink a partition below (ext4 and NTFS), or with the other system's disk tool.");
                    }
                    shrink_panel(ui, &mut self.shrink, (&self.writer, &self.elevation, &self.shared), &d);
                    for g in &gaps {
                        if ui.selectable_label(wiz.req.gap_start == Some(g.first_lba) || (wiz.req.gap_start.is_none() && g.sectors == gaps.iter().map(|x| x.sectors).max().unwrap_or(0)), format!("free space at {}  ({})", human_size(g.first_lba * d.sector as u64), human_size(g.bytes(d.sector)))).clicked() {
                            wiz.req.gap_start = Some(g.first_lba);
                        }
                    }
                    if mode == Mode::Custom {
                        let boot_min = wiz.source.boot_required(d.sector) >> 20;
                        ui.horizontal(|ui| {
                            ui.label(format!("Boot MiB (at least {boot_min}):"));
                            let mut b = wiz.sizes[0].clone();
                            if b.is_empty() {
                                b = boot_min.to_string();
                            }
                            ui.add(egui::TextEdit::singleline(&mut b).desired_width(80.0));
                            wiz.sizes[0] = b;
                            ui.label("Data MiB:");
                            ui.add(egui::TextEdit::singleline(&mut wiz.sizes[1]).desired_width(80.0));
                        });
                    }
                }
                if let Some(e) = &wiz.error {
                    ui.colored_label(red, e);
                }
                ui.horizontal(|ui| {
                    if ui.button("Back").clicked() {
                        next = wiz.back();
                    }
                    if ui.button("Show the plan").clicked() {
                        wiz.req.mode = mode;
                        if mode == Mode::Custom {
                            let parse = |s: &str| s.trim().parse::<u64>().ok().filter(|v| *v > 0);
                            wiz.req.boot_mib = if wiz.req.keep_esp { None } else { parse(&wiz.sizes[0]) };
                            wiz.req.data_mib = parse(&wiz.sizes[1]);
                        }
                        wiz.make_plan();
                    }
                });
            }
            Page::Plan => {
                ui.heading("The plan");
                ui.label("Nothing has been written. This is exactly what will happen:");
                if let Some(p) = &wiz.plan {
                    ui.add_space(4.0);
                    egui::Grid::new("plan").striped(true).show(ui, |ui| {
                        for h in ["#", "Action", "Partition", "Start", "Size", "What"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for s in &p.steps {
                            let c = match s.action {
                                iyakki_usb::planner::Action::Format | iyakki_usb::planner::Action::Delete => red,
                                iyakki_usb::planner::Action::Create | iyakki_usb::planner::Action::AddFiles => green,
                                iyakki_usb::planner::Action::Keep => ui.visuals().text_color(),
                            };
                            ui.label(s.index.to_string());
                            ui.colored_label(c, s.action.word());
                            ui.label(&s.part);
                            ui.label(human_size(s.first_lba * p.sector as u64));
                            ui.label(human_size(s.bytes));
                            ui.label(if s.loses.is_empty() { s.what.clone() } else { format!("{}  [loses: {}]", s.what, s.loses) });
                            ui.end_row();
                        }
                    });
                    if p.destroyed().is_empty() {
                        ui.colored_label(green, "Nothing that exists now is erased or changed.");
                    } else {
                        ui.colored_label(red, format!("Erased or reformatted: {}", p.destroyed().join(", ")));
                    }
                    for w in &p.warnings {
                        ui.label(format!("Note: {w}"));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Back").clicked() {
                        next = wiz.back();
                    }
                    if ui.button("Continue to the confirmation").clicked() {
                        wiz.handle(WKey::Enter);
                    }
                });
            }
            Page::Confirm => {
                ui.heading(egui::RichText::new("Confirm").size(28.0));
                if let Some(c) = wiz.confirm_summary() {
                    ui.label(egui::RichText::new(format!("{}   {}   {}   ({})", c.name, c.size, c.model, c.path)).size(20.0).strong());
                    ui.add_space(6.0);
                    for (head, list, col) in [("Will be created", &c.create, Some(green)), ("Will be KEPT", &c.keep, None), ("Will be ERASED", &c.erase, Some(red))] {
                        ui.label(egui::RichText::new(head).size(18.0).strong().color(col.unwrap_or(ui.visuals().text_color())));
                        if list.is_empty() {
                            ui.label("   nothing");
                        }
                        for l in list {
                            ui.label(egui::RichText::new(format!("   {l}")).size(15.0).color(col.unwrap_or(ui.visuals().text_color())));
                        }
                        ui.add_space(4.0);
                    }
                    for w in &c.warnings {
                        ui.colored_label(egui::Color32::from_rgb(230, 180, 60), egui::RichText::new(format!("! {w}")).size(16.0).strong());
                    }
                    ui.add_space(8.0);
                    if c.needs_ack {
                        ui.checkbox(&mut wiz.erase_ack, egui::RichText::new("I understand that everything on this drive will be erased").size(18.0).color(red));
                    }
                    ui.label("Then an administrator prompt appears; this program never sees the password.");
                    if let Some(e) = &wiz.error {
                        ui.colored_label(red, e);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = wiz.confirm_ok();
                        let label = egui::RichText::new(&c.button).size(22.0).strong().color(egui::Color32::WHITE);
                        let fill = if c.needs_ack { egui::Color32::from_rgb(180, 40, 40) } else { egui::Color32::from_rgb(40, 140, 80) };
                        if ui.add_enabled(ok, egui::Button::new(label).fill(fill).min_size(egui::vec2(260.0, 52.0))).clicked() {
                            next = wiz.activate(iyakki_usb::wizard::CFocus::Install);
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("Back").size(20.0)).min_size(egui::vec2(120.0, 52.0))).clicked() {
                            next = wiz.activate(iyakki_usb::wizard::CFocus::Back);
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("Cancel").size(20.0)).min_size(egui::vec2(120.0, 52.0))).clicked() {
                            next = wiz.activate(iyakki_usb::wizard::CFocus::Cancel);
                        }
                    });
                }
            }
            Page::Progress => {
                ui.heading("Writing");
                let (label, done, total) = wiz.progress.clone();
                let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                ui.add(egui::ProgressBar::new(f).text(format!("{label}  {} of {}", human_size(done), human_size(total))));
                ui.label("Do not remove the drive or switch off. If the power fails now, the other system's partition table is still the old one until the very last step.");
            }
            Page::Done => {
                ui.heading("Done");
                ui.colored_label(green, &wiz.result);
            }
            Page::Failed => {
                ui.heading("The installation stopped");
                ui.colored_label(red, &wiz.result);
                if matches!(self.elevation, Elevation::Manual) {
                    if let Some(w) = &self.writer {
                        ui.label(elevate::manual_instructions(w, &self.plan_file));
                    }
                }
            }
        }
        if next == Next::Write {
            self.start_write();
        }
        if next == Next::Quit {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let _ = Path::new("");
    }
}

/// Make free space by shrinking a partition (F-INST-30): the same two steps as the chat tool and the command line.
    /// The plan is read first (nothing changes), then the drive's name typed again and the ticked box start the shrink.
fn shrink_panel(ui: &mut egui::Ui, st: &mut ShrinkUi, (writer, elevation, shared): (&Option<PathBuf>, &Elevation, &Arc<Mutex<Shared>>), d: &DiskModel) {
        let red = egui::Color32::from_rgb(220, 80, 80);
        let green = egui::Color32::from_rgb(90, 190, 120);
        ui.add_space(6.0);
        if ui.selectable_label(st.open, "Make free space by shrinking a partition").clicked() {
            st.open = !st.open;
        }
        if !st.open {
            return;
        }
        ui.label("Have a backup of what matters. Only an unmounted ext2/3/4, NTFS, FAT32 or exFAT partition can be shrunk (FAT32 and exFAT only down to just after its last file); its start never moves.");
        for c in iyakki_usb::resize_step::candidates(d) {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:<12} {:>9}  {}", c.name, human_size(c.bytes), c.fs));
                match &c.refused {
                    None => {
                        if ui.selectable_label(st.part == Some(c.part), "Shrink this").clicked() {
                            *st = ShrinkUi { open: true, part: Some(c.part), ..ShrinkUi::default() };
                        }
                    }
                    Some(why) => {
                        ui.colored_label(egui::Color32::GRAY, why);
                    }
                }
            });
        }
        let Some(part) = st.part else { return };
        ui.horizontal(|ui| {
            ui.label("New size of the partition (MiB):");
            ui.add(egui::TextEdit::singleline(&mut st.mib).desired_width(80.0));
            let mib = st.mib.trim().parse::<u64>().ok().filter(|m| *m > 0);
            if ui.add_enabled(mib.is_some() && !st.busy, egui::Button::new("Show the shrink plan")).clicked() {
                st.done = None;
                run_shrink(st, writer, elevation, shared, &d.name, part, mib.unwrap_or(0), None);
            }
        });
        if let Some(plan) = st.plan.clone() {
            ui.monospace(plan);
            ui.colored_label(red, "The file system of this partition will be changed. A power cut or a fault while it runs can damage it.");
            ui.checkbox(&mut st.ack, "I understand that this partition's file system is changed and could be damaged by a power cut or a fault");
            ui.horizontal(|ui| {
                ui.label(format!("Type the drive's name ({}) again:", d.name));
                ui.add(egui::TextEdit::singleline(&mut st.typed).desired_width(100.0));
                let ready = st.ack && st.typed.trim() == d.name && !st.busy;
                if ui.add_enabled(ready, egui::Button::new("Shrink now")).clicked() {
                    let mib = st.mib.trim().parse::<u64>().unwrap_or(0);
                    let name = st.typed.trim().to_string();
                    run_shrink(st, writer, elevation, shared, &d.name, part, mib, Some(name));
                }
            });
        }
        if st.busy {
            ui.label("Working (the administrator prompt asks for your password; this program never sees it)...");
        }
        match &st.done {
            Some(Ok(t)) => {
                ui.colored_label(green, t);
            }
            Some(Err(e)) => {
                ui.colored_label(red, e);
            }
            None => {}
        }
    }

fn main() -> eframe::Result {
    eframe::run_native("IYAKKI installer", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(App::new()))))
}
