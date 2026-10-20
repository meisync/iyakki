//! `iyakki-desktop`: the IYAKKI shell (login, tiles, CHAT/RAG/MCP/CLI/Users) in a normal desktop
//! window, for trying it out without booting a VM. It is the same state machine the bare-metal
//! framebuffer backend drives; here egui just supplies the window, keyboard and mouse.
//!
//! State lives in `~/.local/share/iyakki-dev` (users, audit log, documents), never in /var/lib.
//! Optional config: `~/.local/share/iyakki-dev/ai.conf` (chat model, see /etc/iyakki/ai.conf docs).

use std::sync::Arc;

use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv, ShellPaths};
use iyakki_shell::ui::{Event, Key, Nav};
use eframe::egui;

struct App {
    shell: Shell,
    gfx: Gfx,
    tex: Option<egui::TextureHandle>,
    /// The boot screen (logo over the galaxy, progress, `L` for the log) plays for a moment before the shell, as it
    /// does when the real system boots. Enter, Esc or a click skips it.
    started: std::time::Instant,
    splash_done: bool,
    splash_log: bool,
    /// The data folder, and another live window on it (asked about before this one starts; `None` once decided).
    dir: std::path::PathBuf,
    other_window: Option<iyakki_agent::instance::Instance>,
    /// The window title last set (it carries "Host access: n folders" while host folders are shared).
    title: String,
}

impl Drop for App {
    fn drop(&mut self) {
        iyakki_agent::instance::release(&self.dir);
    }
}

/// What the start-up checklist says, in order. Each is ticked a moment after the one before; the work behind them (opening the
/// accounts, settings and tools) is already done by the time the window exists, so the list is short and quick.
const SPLASH_STEPS: [&str; 4] = ["Core modules loaded", "Accounts and settings opened", "Models, tools and vaults ready", "Desktop ready"];
/// Seconds between one tick and the next.
const SPLASH_TICK: f32 = 0.18;
/// The screen stays this long after the last tick, then the login appears.
const SPLASH_HOLD: f32 = 0.3;
const SPLASH_SECONDS: f32 = SPLASH_STEPS.len() as f32 * SPLASH_TICK + SPLASH_HOLD;

impl App {
    fn splash(&mut self, ctx: &egui::Context, w: u32, h: u32) {
        let mut skip = false;
        for ev in ctx.input(|i| i.events.clone()) {
            match ev {
                egui::Event::Key { key: egui::Key::L, pressed: true, .. } => self.splash_log = !self.splash_log,
                egui::Event::Key { key: egui::Key::Enter | egui::Key::Escape | egui::Key::Space, pressed: true, .. } => skip = true,
                egui::Event::PointerButton { pressed: true, .. } => {
                    // a click along the bottom edge toggles the log, like a touch does on the real boot screen
                    let (from, _) = iyakki_shell::splash::hint_zone(w, h);
                    let y = ctx.input(|i| i.pointer.interact_pos()).map_or(0.0, |p| p.y * ctx.pixels_per_point());
                    if y >= from { self.splash_log = !self.splash_log } else { skip = true }
                }
                _ => {}
            }
        }
        let t = self.started.elapsed().as_secs_f32();
        if skip || t >= SPLASH_SECONDS {
            self.splash_done = true;
            return;
        }
        let done = ((t / SPLASH_TICK) as usize).min(SPLASH_STEPS.len());
        let steps: Vec<(&str, bool)> = SPLASH_STEPS.iter().enumerate().map(|(i, s)| (*s, i < done)).collect();
        let lines: Vec<String> = SPLASH_STEPS.iter().enumerate().filter(|(i, _)| *i < done).map(|(i, s)| format!("[{:>6.3}] iyakki: {s}", (i + 1) as f32 * SPLASH_TICK))
            .chain(std::iter::once(format!("[{t:>6.3}] iyakki: this window skips the real boot; the log of a real boot appears here"))).collect();
        iyakki_shell::splash::draw(&mut self.gfx, &iyakki_shell::splash::View { t, fraction: (t / (SPLASH_STEPS.len() as f32 * SPLASH_TICK)).min(1.0), stage: "", log: self.splash_log.then_some(&lines[..]), steps: &steps });
    }
}

fn map_key(k: egui::Key, shift: bool) -> Option<Key> {
    Some(match k {
        egui::Key::Enter => Key::Enter,
        egui::Key::Backspace => Key::Backspace,
        egui::Key::Delete => Key::Delete,
        egui::Key::Tab => {
            if shift {
                Key::BackTab
            } else {
                Key::Tab
            }
        }
        egui::Key::Escape => Key::Esc,
        egui::Key::ArrowUp => Key::Up,
        egui::Key::ArrowDown => Key::Down,
        egui::Key::ArrowLeft => Key::Left,
        egui::Key::ArrowRight => Key::Right,
        egui::Key::Home => Key::Home,
        egui::Key::End => Key::End,
        egui::Key::PageUp => Key::PageUp,
        egui::Key::PageDown => Key::PageDown,
        egui::Key::F1 => Key::F(1),
        egui::Key::F7 => Key::F(7),
        egui::Key::F8 => Key::F(8),
        egui::Key::F9 => Key::F(9),
        egui::Key::F10 => Key::F(10),
        egui::Key::F12 => Key::F(12),
        _ => return None,
    })
}

fn map_nav(k: egui::Key) -> Option<Nav> {
    Some(match k {
        egui::Key::ArrowUp => Nav::Up,
        egui::Key::ArrowDown => Nav::Down,
        egui::Key::ArrowLeft => Nav::Left,
        egui::Key::ArrowRight => Nav::Right,
        egui::Key::Home => Nav::Home,
        egui::Key::End => Nav::End,
        egui::Key::PageUp => Nav::PageUp,
        egui::Key::PageDown => Nav::PageDown,
        _ => return None,
    })
}

impl eframe::App for App {
    /// A panic inside the shell must not take the whole window down: it is written to `crash.log` (message, place and
    /// backtrace, by the panic hook set in `main`), the frame is skipped and the window carries on.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.ui_inner(ui, frame)));
        if r.is_err() {
            eprintln!("iyakki-desktop: an internal error was caught and logged (see crash.log in the data folder); the window carries on");
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl App {
    fn ui_inner(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(o) = self.other_window.clone() {
            let (mut go, mut quit) = (false, false);
            egui::Window::new("IYAKKI is already running").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label(iyakki_agent::instance::warning(&o));
                ui.horizontal(|ui| {
                    go = ui.button("Continue anyway").clicked();
                    quit = ui.button("Quit").clicked();
                });
            });
            if go {
                iyakki_agent::instance::claim(&self.dir);
                self.other_window = None;
                self.started = std::time::Instant::now();
            } else if quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        let ppp = ctx.pixels_per_point();
        let avail = ui.available_size();
        let (w, h) = (((avail.x * ppp) as u32).max(320), ((avail.y * ppp) as u32).max(240));
        let origin = ui.min_rect().min;
        self.gfx.resize(w, h);

        if !self.splash_done {
            self.splash(&ctx, w, h);
        }
        let events = if self.splash_done { ctx.input(|i| i.events.clone()) } else { Vec::new() };
        // while the window is not the system's focused one no input shows a caret or a focus frame
        iyakki_shell::ui::set_window_focused(ctx.input(|i| i.focused));
        let shift = ctx.input(|i| i.modifiers.shift);
        // a Shift+click extends a text selection: the shell reads it while it handles the press
        iyakki_shell::ui::set_shift_held(shift);
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    for c in t.chars().filter(|c| !c.is_control()) {
                        self.shell.handle(&Event::Key(Key::Char(c)));
                    }
                }
                // Ctrl+V from the system clipboard; with Shift held it opens IYAKKI's clipboard history instead
                egui::Event::Paste(t) => {
                    eprintln!("iyakki-desktop: paste event from the system clipboard, {} characters", t.chars().count());
                    if shift {
                        self.shell.handle(&Event::Key(Key::Ctrl('V')));
                    } else {
                        self.shell.handle(&Event::Paste(t));
                    }
                }
                egui::Event::Copy => self.shell.handle(&Event::Key(Key::Ctrl('c'))),
                egui::Event::Cut => self.shell.handle(&Event::Key(Key::Ctrl('x'))),
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    if modifiers.command && modifiers.shift && key == egui::Key::V {
                        self.shell.handle(&Event::Key(Key::Ctrl('V')));
                    } else if modifiers.command && key == egui::Key::A {
                        self.shell.handle(&Event::Key(Key::Ctrl('a')));
                    } else if let Some(nav) = map_nav(key).filter(|_| modifiers.shift || modifiers.command) {
                        self.shell.handle(&Event::Key(Key::Nav { nav, shift: modifiers.shift, ctrl: modifiers.command }));
                    } else if modifiers.command && key.name().len() == 1 && key.name().chars().all(|c| c.is_ascii_alphabetic()) && !matches!(key, egui::Key::C | egui::Key::X | egui::Key::V) {
                        // Ctrl+letter shortcuts (save, undo, find, agent mode, …); Copy/Cut/Paste arrive as their own events
                        let c = key.name().chars().next().unwrap_or('a');
                        self.shell.handle(&Event::Key(Key::Ctrl(if modifiers.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })));
                    } else if let Some(k) = map_key(key, modifiers.shift) {
                        self.shell.handle(&Event::Key(k));
                    }
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: true, .. } => {
                    self.shell.handle(&Event::MouseRight(((pos.x - origin.x) * ppp) as i32, ((pos.y - origin.y) * ppp) as i32));
                }
                egui::Event::PointerMoved(p) => {
                    self.shell.handle(&Event::MouseMove(((p.x - origin.x) * ppp) as i32, ((p.y - origin.y) * ppp) as i32));
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, .. } => {
                    self.shell.handle(&Event::MouseUp(((pos.x - origin.x) * ppp) as i32, ((pos.y - origin.y) * ppp) as i32));
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => {
                    self.shell.handle(&Event::MouseDown(((pos.x - origin.x) * ppp) as i32, ((pos.y - origin.y) * ppp) as i32));
                }
                egui::Event::MouseWheel { delta, .. } => {
                    self.shell.handle(&Event::Scroll(delta.y.signum() as i32 * delta.y.abs().ceil() as i32));
                }
                _ => {}
            }
        }
        if self.splash_done {
            // files dragged over or dropped on the window (window mode): the shell shows where they would go and imports them
            let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
            self.shell.set_drag_hover(hovering);
            let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
            if !dropped.is_empty() {
                eprintln!("iyakki-desktop: {} dropped item(s)", dropped.len());
                self.shell.drop_files(dropped);
            }
            let title = match self.shell.host_access_chip() {
                Some(chip) => format!("IYAKKI {} · {chip}", env!("CARGO_PKG_VERSION")),
                None => format!("IYAKKI {}", env!("CARGO_PKG_VERSION")),
            };
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
            self.shell.handle(&Event::Tick);
            // the browser voice: the window's web engine is asked whether it can speak, once, the first time somebody wants to read
            let voice = iyakki_agent::hostvoice::HostVoice::current();
            if voice.probe_wanted() {
                voice.clear_probe_wanted();
                start_speech_probe(voice.clone(), &self.dir);
            }
            // mirror anything copied inside IYAKKI to the system clipboard
            if let Some(text) = self.shell.take_host_copy() {
                ctx.copy_text(text);
            }
            // the clipboard queue was opened: ask the system for what it holds (it comes back as a paste)
            if self.shell.take_host_paste_request() {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                ctx.request_repaint();
                eprintln!("iyakki-desktop: asked the system clipboard for its text");
            }
            // and a screenshot, as a picture
            if let Some((w, h, rgba)) = iyakki_shell::ui::clipboard::take_host_image() {
                ctx.copy_image(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba));
            }
            self.shell.render(&mut self.gfx);
            // the mouse pointer follows what is under it: a hand over links and buttons, a text bar over fields
            use iyakki_shell::ui::Pointer;
            ctx.set_cursor_icon(match self.shell.pointer() {
                Pointer::Default => egui::CursorIcon::Default,
                Pointer::Hand => egui::CursorIcon::PointingHand,
                Pointer::Text => egui::CursorIcon::Text,
                Pointer::Wait => egui::CursorIcon::Progress,
                Pointer::Crosshair => egui::CursorIcon::Crosshair,
                Pointer::Move => egui::CursorIcon::Move,
                Pointer::NotAllowed => egui::CursorIcon::NotAllowed,
                Pointer::ResizeH => egui::CursorIcon::ResizeHorizontal,
                Pointer::ResizeV => egui::CursorIcon::ResizeVertical,
                Pointer::Grab => egui::CursorIcon::Grab,
                Pointer::Help => egui::CursorIcon::Help,
                Pointer::Hidden => egui::CursorIcon::None,
            });
        }

        // tiny-skia pixels are premultiplied RGBA; the shell paints opaque backgrounds, so alpha is 255
        let img = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], self.gfx.rgba());
        match &mut self.tex {
            Some(t) => t.set(img, egui::TextureOptions::NEAREST),
            None => self.tex = Some(ctx.load_texture("iyakki-shell", img, egui::TextureOptions::NEAREST)),
        }
        if let Some(t) = &self.tex {
            ui.image((t.id(), egui::vec2(w as f32 / ppp, h as f32 / ppp)));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// `./run.sh disk add 8G | list | rm vdb`: manage the virtual disks without opening the window.
fn disk_command(dir: &std::path::Path, a: &[String]) -> i32 {
    use iyakki_storage::virt::{parse_size, VirtualDisks};
    let v = match VirtualDisks::open(dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match a.first().map(String::as_str) {
        Some("add") => match a.get(1).and_then(|s| parse_size(s)) {
            Some(bytes) => match v.add(bytes) {
                Ok(n) => {
                    println!("made {n} ({}) in {}", iyakki_storage::human(bytes), dir.display());
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
            None => {
                eprintln!("usage: ./run.sh disk add SIZE   (for example 8G or 512M)");
                2
            }
        },
        Some("list") | None => {
            for b in iyakki_storage::list(&v.roots()) {
                println!("{}\t{}\t{}", b.name, iyakki_storage::human(b.size_bytes), b.content.describe());
            }
            0
        }
        Some("rm") => match a.get(1).map(|n| v.remove(n)) {
            Some(Ok(())) => 0,
            Some(Err(e)) => {
                eprintln!("{e}");
                1
            }
            None => {
                eprintln!("usage: ./run.sh disk rm NAME   (for example vda)");
                2
            }
        },
        _ => {
            eprintln!("usage: ./run.sh disk add SIZE | list | rm NAME");
            2
        }
    }
}

/// Ask the window's web engine (in the background) whether it has speech synthesis and tell the browser voice. Nothing is
/// spoken through the window: an engine that can speak is reported too, with the way to the browser front end.
fn start_speech_probe(voice: Arc<iyakki_agent::hostvoice::HostVoice>, dir: &std::path::Path) {
    let profile = dir.join("speech-probe");
    let spawned = std::thread::Builder::new().name("iyakki-speech-probe".into()).spawn(move || {
        let Some(helper) = iyakki_agent::webengine::embedded_helper() else {
            voice.set_unsupported("this window has no web engine helper (install python3-gi and gir1.2-webkit2-4.1, or set IYAKKI_WEBVIEW_HELPER), so it cannot speak. Open IYAKKI in a web browser to use the browser voice");
            return;
        };
        let env = iyakki_agent::webengine::helper_env(&profile);
        let r = iyakki_shell::speechprobe::probe(helper, env, profile.clone(), std::time::Duration::from_secs(40));
        let _ = std::fs::remove_dir_all(&profile);
        eprintln!("iyakki-desktop: speech synthesis in the window's web engine: {r:?}");
        match r {
            Ok(false) => voice.set_unsupported("this window's web engine has no speech synthesis (WebKitGTK is often built without it). Open IYAKKI in a web browser to use the browser voice"),
            Ok(true) => voice.set_unsupported("this window's web engine can speak, but reading through the window is not connected yet. Open IYAKKI in a web browser to use the browser voice"),
            Err(e) => voice.set_unsupported(&format!("the window's web engine could not be asked ({e}). Open IYAKKI in a web browser to use the browser voice")),
        }
    });
    if spawned.is_err() {
        eprintln!("iyakki-desktop: could not start the speech check");
    }
}

fn main() -> eframe::Result {
    // a window on a development machine's desktop: the browser's full engine is the WebKitGTK helper of this checkout
    // (dist/webview/host-helper.py, drawn inside the browser's page area), or $IYAKKI_WEBVIEW_HELPER
    iyakki_agent::webengine::set_host_mode(true);
    // speech engines already on this computer are found in the background and used for reading aloud (docs/VOICE.md)
    iyakki_agent::hostengines::enable_by_default();
    // window mode runs on a normal computer: host files (drop to import, share a folder read-only) may be offered, confined to
    // the host home folder (or IYAKKI_HOST_HOME)
    iyakki_agent::hostfiles::set_enabled(true);
    if let Ok(h) = std::env::var("IYAKKI_HOST_HOME") {
        iyakki_agent::hostfiles::set_host_home(Some(std::path::PathBuf::from(h)));
    }
    // the sample MCP server (the MCP tile starts this program again with --mcp-demo)
    if std::env::args().nth(1).as_deref() == Some("--mcp-demo") {
        iyakki_agent::mcp::demo_serve_stdio();
        return Ok(());
    }
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("iyakki-desktop {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // an external coding agent's view of IYAKKI's tools: a relay to the run's local socket (docs/EXTERNAL-AGENTS.md)
    if std::env::args().nth(1).as_deref() == Some("--mcp-tools") {
        std::process::exit(iyakki_agent::extagent::bridge::relay_stdio(&std::env::args().nth(2).unwrap_or_default()));
    }
    let dir = std::env::var("IYAKKI_DEV_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share/iyakki-dev")
    });
    let _ = std::fs::create_dir_all(dir.join("docs/shared"));
    // every panic (this thread or any other: runs, helpers, loaders) is logged with its backtrace to <data>/crash.log
    {
        let log = dir.join("crash.log");
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let bt = std::backtrace::Backtrace::force_capture();
            let thread = std::thread::current();
            let text = format!("[{}] panic in thread {:?}: {info}\n{bt}\n", iyakki_agent::clock::now_unix(), thread.name().unwrap_or("?"));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
                use std::io::Write;
                let _ = f.write_all(text.as_bytes());
            }
            previous(info);
        }));
    }
    // disks: the window shows virtual disks it makes itself (sparse files in <dir>/disks), never the computer's drives,
    // unless started with --real-disks
    let real_disks = std::env::args().any(|a| a == "--real-disks");
    let disks_dir = dir.join("disks");
    if std::env::args().nth(1).as_deref() == Some("disk") {
        std::process::exit(disk_command(&disks_dir, &std::env::args().skip(2).collect::<Vec<_>>()));
    }
    if !real_disks {
        if let Err(e) = iyakki_shell::apps::settings::use_virtual_disks(&disks_dir, &dir, None) {
            eprintln!("iyakki-desktop: virtual disks: {e}");
            std::process::exit(1);
        }
    }
    let paths = ShellPaths {
        users: dir.join("users.json").display().to_string(),
        audit: dir.join("audit.log").display().to_string(),
        ai_conf: dir.join("ai.conf").display().to_string(),
        data: dir.display().to_string(),
        hwcheck: std::env::args().any(|a| a == "--hwcheck"),
    };
    // Point RAG at the dev folder unless ai.conf says otherwise.
    if !dir.join("ai.conf").exists() {
        let _ = std::fs::write(
            dir.join("ai.conf"),
            format!("# IYAKKI dev config. Uncomment to enable chat:\n# chat.url = http://localhost:11434/v1/chat/completions\n# chat.model = llama3.2\nrag.docs = {}\nmcp.config = {}\n", dir.join("docs").display(), dir.join("mcp.json").display()),
        );
    }
    let host = Arc::new(iyakki_core::host::CoreHost::new());
    let env = match ShellEnv::from_paths(&paths, host) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("iyakki-desktop: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("iyakki-desktop: data in {}", dir.display());
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 760.0]).with_title(format!("IYAKKI {}", env!("CARGO_PKG_VERSION"))), ..Default::default() };
    // one window per data folder is a hint, not a rule: a second start is asked before it goes on
    let other_window = iyakki_agent::instance::other_running(&dir);
    if other_window.is_none() {
        iyakki_agent::instance::claim(&dir);
    }
    let app_dir = dir.clone();
    eframe::run_native("IYAKKI", opts, Box::new(move |_| {
        iyakki_agent::hostvoice::HostVoice::current().connect(iyakki_agent::hostvoice::Front::Window);
        Ok(Box::new(App { shell: Shell::new(env), gfx: Gfx::new(1280, 720), tex: None, started: std::time::Instant::now(), splash_done: false, splash_log: false, dir: app_dir, other_window, title: String::new() }))
    }))
}
