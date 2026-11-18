//! MARKDOWN in the shell: the viewer itself is the crate `iyakki-app-markdown`, written only against the UI SDK. This file is the shell
//! side: it maps the shell's events and draws the SDK frame like the SAMPLE tile does, and it is where a document an app handed over with
//! `iyakki.markdown.view` arrives. The core keeps the documents waiting for a person in the app data of this app (`PENDING_KEY`, written
//! by the bridge's viewer backend after the app's own checks and quota); the tile takes the oldest one when it opens and again while it is
//! open, shows it and removes it. An app never opens the viewer itself and never sees where the document is shown.

use iyakki_app_markdown::MarkdownApp;
use iyakki_ui_sdk::app::Flow as UiFlow;
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper, Screen};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::raster::RasterCanvas;

use super::sample::{look_now, SampleTile};
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx, Icon};
use crate::ui::{Event, R};

pub const ACCENT: [u8; 4] = rgb(140, 190, 230);

/// The app data key (of the person, in the namespace `markdown`) that holds the documents waiting: a JSON list of `{"title","text"}`.
pub const PENDING_KEY: &str = "pending";
/// Most documents waiting at once; the oldest is shown first.
pub const MAX_WAITING: usize = 5;

pub struct MarkdownTile {
    host: Host<MarkdownApp>,
    shape: (u32, u32, f32),
    /// The names the file list offered (the tile keeps them to know which file a choice means; the app only sees the names).
    names: Vec<String>,
    /// Ticks since the waiting list was last read (reading is a small file, so not on every frame).
    since: u32,
}

impl Default for MarkdownTile {
    fn default() -> Self {
        MarkdownTile::new()
    }
}

impl MarkdownTile {
    pub fn new() -> MarkdownTile {
        let host = Host::new(MarkdownApp::default(), look_now(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[iyakki_app_markdown::ID])), Box::new(MemWallpaper::default()));
        MarkdownTile { host, shape: (1280, 720, 1.0), names: Vec::new(), since: u32::MAX }
    }

    /// Take the oldest waiting document of this person and show it. True when there was one.
    fn take_waiting(&mut self, cx: &AppCx) -> bool {
        let user = cx.session.user.clone();
        if user.is_empty() {
            return false;
        }
        let data = cx.core.app_data();
        let Some(list) = data.get(&user, iyakki_app_markdown::ID, PENDING_KEY).and_then(|t| serde_json::from_str::<Vec<serde_json::Value>>(&t).ok()) else { return false };
        let Some(first) = list.first() else { return false };
        // `markdown.openFile`: show the person their own Markdown files; they choose one (nothing is read before that)
        if first["pick"].as_bool() == Some(true) {
            let names = cx.core.home_files().list_markdown(&user);
            let rest: Vec<&serde_json::Value> = list.iter().skip(1).collect();
            let _ = if rest.is_empty() { data.remove(&user, iyakki_app_markdown::ID, PENDING_KEY) } else { data.set(&user, iyakki_app_markdown::ID, PENDING_KEY, &serde_json::to_string(&rest).unwrap_or_default()) };
            cx.audit.record(&user, "markdown.pick.offered", &format!("{} files", names.len()));
            if names.is_empty() {
                self.host.app.open("No Markdown files", "You have no Markdown files (.md) in your home folder to open.");
            } else {
                self.names = names.clone();
                self.host.app.offer(names);
            }
            return true;
        }
        let (title, text) = (first["title"].as_str().unwrap_or(""), first["text"].as_str().unwrap_or(""));
        self.host.app.open(title, text);
        let rest: Vec<&serde_json::Value> = list.iter().skip(1).collect();
        let _ = if rest.is_empty() { data.remove(&user, iyakki_app_markdown::ID, PENDING_KEY) } else { data.set(&user, iyakki_app_markdown::ID, PENDING_KEY, &serde_json::to_string(&rest).unwrap_or_default()) };
        cx.audit.record(&user, "markdown.shown", &format!("{} bytes", text.len()));
        true
    }

    /// The person chose a name in the file list: read that file through the core (their own home folder only) and show it.
    fn open_choice(&mut self, cx: &AppCx) {
        let Some(i) = self.host.app.take_choice() else { return };
        let (user, Some(name)) = (cx.session.user.clone(), self.names.get(i).cloned()) else { return };
        match cx.core.home_files().read_markdown(&user, &name) {
            Ok(text) => {
                cx.audit.record(&user, "markdown.file.opened", &name);
                self.host.app.open(name.rsplit('/').next().unwrap_or(&name), &text);
            }
            Err(e) => {
                cx.audit.record(&user, "markdown.file.refused", &name);
                self.host.app.open("Cannot open the file", &e);
            }
        }
    }
}

impl App for MarkdownTile {
    fn title(&self) -> &'static str {
        "MARKDOWN"
    }
    fn icon(&self) -> Icon {
        Icon::Data
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "Up Down scroll · Space next page · b back a page · l links · Esc back".into()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let flow = SampleTile::map(ev).map(|e| self.host.event(&e));
        self.open_choice(cx);
        match flow {
            Some(UiFlow::Back) => Flow::Back,
            _ => {
                if matches!(ev, Event::Tick) {
                    self.since = self.since.saturating_add(1);
                    if self.since >= 15 {
                        self.since = 0;
                        self.take_waiting(cx);
                    }
                }
                Flow::Stay
            }
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        if self.since == u32::MAX {
            self.since = 0;
            self.take_waiting(cx);
        }
        let want = (area.w.round() as u32, area.h.round() as u32, s);
        if want != self.shape {
            self.shape = want;
            self.host.arrangement = Arrangement::new(vec![Screen { id: 0, name: "screen".into(), width: want.0, height: want.1, scale: want.2, x: 0, y: 0, primary: true }]);
        }
        self.host.look = look_now();
        self.host.type_style = super::sample::type_now();
        self.host.draw(&mut RasterCanvas::new(g, area, s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::Core as _;
    use std::sync::Arc;

    fn cx_with_core(tag: &str) -> (AppCx, Arc<iyakki_core_fake::FakeCore>) {
        let mut cx = crate::apps::tests::cx_with_admin_store(tag);
        let core = Arc::new(iyakki_core_fake::FakeCore::new());
        cx.core = core.clone();
        (cx, core)
    }

    fn text_of(g: &Gfx) -> String {
        g.text_log.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn a_document_an_app_handed_over_is_shown_once_in_order_and_audited() {
        let (cx, core) = cx_with_core("md-tile");
        let user = cx.session.user.clone();
        core.app_data().set(&user, "markdown", PENDING_KEY, r##"[{"title":"First","text":"# Hello\n\nbody one"},{"title":"Second","text":"# Again\n\nbody two"}]"##).unwrap();
        let mut t = MarkdownTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 1);
        assert!(text_of(&g).contains("Hello") && text_of(&g).contains("body one"), "{}", text_of(&g));
        // the second waits until the tile looks again
        let left = core.app_data().get(&user, "markdown", PENDING_KEY).unwrap();
        assert!(left.contains("Second") && !left.contains("First"), "{left}");
        for _ in 0..15 {
            t.handle(&Event::Tick, &cx);
        }
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 2);
        assert!(text_of(&g).contains("body two"), "{}", text_of(&g));
        assert_eq!(core.app_data().get(&user, "markdown", PENDING_KEY), None, "nothing waits any more");
        assert_eq!(cx.audit.recent(10).iter().filter(|l| l.contains("markdown.shown")).count(), 2);
    }

    #[test]
    fn a_file_request_shows_the_persons_own_markdown_files_and_the_one_chosen_opens_and_is_audited() {
        let (cx, core) = cx_with_core("md-pick");
        let user = cx.session.user.clone();
        core.files_fake().put(&user, "notes/day-1.md", "# Day one\n\nfirst words");
        core.files_fake().put(&user, "todo.md", "# Todo\n\nbuy milk");
        core.files_fake().put("somebody-else", "secret.md", "# Secret\n\nprivate words");
        core.app_data().set(&user, "markdown", PENDING_KEY, r#"[{"pick":true}]"#).unwrap();
        let mut t = MarkdownTile::new();
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 0);
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 1);
        let said = text_of(&g);
        assert!(said.contains("notes/day-1.md") && said.contains("todo.md") && !said.contains("secret"), "{said}");
        assert_eq!(core.app_data().get(&user, "markdown", PENDING_KEY), None, "the request is taken");
        // Down, Enter: the second file (todo.md) opens
        t.handle(&Event::Key(crate::ui::Key::Down), &cx);
        t.handle(&Event::Key(crate::ui::Key::Enter), &cx);
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 2);
        assert!(text_of(&g).contains("buy milk"), "{}", text_of(&g));
        let log = cx.audit.recent(20).join("\n");
        assert!(log.contains("markdown.pick.offered 2 files") && log.contains("markdown.file.opened todo.md"), "{log}");
        // no files: a plain note instead of an empty list
        let (cx2, core2) = cx_with_core("md-pick2");
        let u2 = cx2.session.user.clone();
        core2.app_data().set(&u2, "markdown", PENDING_KEY, r#"[{"pick":true}]"#).unwrap();
        let mut t2 = MarkdownTile::new();
        let mut g = Gfx::new(800, 480);
        t2.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx2, 0);
        let mut g = Gfx::new(800, 480);
        t2.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx2, 1);
        assert!(text_of(&g).contains("no Markdown files") || text_of(&g).contains("No Markdown files"), "{}", text_of(&g));
    }

    #[test]
    fn another_persons_document_is_not_shown_and_a_broken_list_is_ignored() {
        let (cx, core) = cx_with_core("md-tile2");
        core.app_data().set("somebody-else", "markdown", PENDING_KEY, r#"[{"title":"Secret","text":"private words"}]"#).unwrap();
        let user = cx.session.user.clone();
        core.app_data().set(&user, "markdown", PENDING_KEY, "not json").unwrap();
        let mut t = MarkdownTile::new();
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 0);
        assert!(!text_of(&g).contains("private words"));
        assert!(core.app_data().get("somebody-else", "markdown", PENDING_KEY).is_some());
    }

    #[test]
    fn the_tile_fits_at_the_small_and_the_large_sizes_and_the_back_key_leaves() {
        let (cx, _core) = cx_with_core("md-tile3");
        let mut t = MarkdownTile::new();
        for (w, h, s) in [(360.0f32, 740.0f32, 1.0f32), (800.0, 480.0, 1.0), (1280.0, 720.0, 1.5)] {
            let mut g = Gfx::new(w as u32, h as u32);
            t.render(&mut g, R { x: 0.0, y: 0.0, w, h }, s, &cx, 0);
            assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h}: something is drawn");
        }
        assert!(matches!(t.handle(&Event::Key(crate::ui::Key::Esc), &cx), Flow::Back));
    }
}
