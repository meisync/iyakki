//! EDIT as an SDK app (F-SDK-34, batch 4): the folder list (`FilesView`), the editor (`EditorView`) and the assistant beside it (`ChatView`),
//! written only against `iyakki-ui-sdk`, so the same source runs in the shell, in a browser or client and as a WASM tile (`src/guest.rs`).
//!
//! * Files: choosing a file opens it in the editor. The app never sees where the home is; the core's files port reads and, after the person's
//!   yes, writes (Save asks through the files view, never by itself).
//! * The editor: the engine and the review of the assistant's changes are the view's (`EditorView`): a change is never applied by itself.
//! * The assistant: a conversation of its own named `edit`; it is given the document and the selection, and a reply with changes becomes a
//!   proposal in the editor.
//! * Text size: two buttons and the pinch. The pinch is delivered to a tile only when the tile declares tile ABI 3 (this one does, feature
//!   `tile`); the size is kept through the storage port when there is one.
//!
//! The old EDIT screen (`iyakki-apps` `edit*.rs`) stays until the apps move over (the owner's order: not before Impl is 100).

use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::assistant::{AssistantPort, SessionConfig};
use iyakki_ui_sdk::canvas::{Canvas, Hit, Role};
use iyakki_ui_sdk::chatview::{ChatView, Entry};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::desktop::DockStatus;
use iyakki_ui_sdk::editorview::{self, EditorView};
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::files::FilesPort;
use iyakki_ui_sdk::filesview::{self, FilesView};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::ports::StoragePort;
use iyakki_ui_sdk::size::{SizeClass, TypeRole};
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;
use iyakki_ui_sdk::zoom::ZoomChange;

#[cfg(all(target_arch = "wasm32", feature = "tile"))]
mod guest;

/// `tile.wasm`: the wasm32 build of this app (`cargo build --release --lib --target wasm32-unknown-unknown -p iyakki-app-edit --features tile`,
/// recorded in `tile.wasm.sha256`), the module the browser host serves as the EDIT tile. `tests/wasm32_guest.rs` builds it again and checks it behaves like this file.
#[cfg(not(target_arch = "wasm32"))]
pub const TILE_WASM: &[u8] = include_bytes!("../tile.wasm");

/// The capabilities the tile asks for, the same as `tile_caps` in `app.toml`: the browser host checks them before any call reaches the core.
pub const TILE_CAPS: [&str; 4] = ["home", "assistant", "storage", "dock"];

/// The app id the launcher, the dock and the policy know it by.
pub const ID: &str = "edit";
/// The storage key of the text size.
pub const ZOOM_KEY: &str = "text-size";
/// The text size as a thousandth of the person's own size: the smallest, the largest, the start and one button press.
pub use iyakki_ui_sdk::zoom::TextZoom;
pub const ZOOM_MIN: u32 = TextZoom::MIN;
pub const ZOOM_MAX: u32 = TextZoom::MAX;
pub const ZOOM_START: u32 = TextZoom::START;
pub const ZOOM_STEP: u32 = TextZoom::STEP;

const A_FILES: u32 = 1;
const A_ASSIST: u32 = 2;
const A_SMALLER: u32 = 3;
const A_BIGGER: u32 = 4;
/// How many buttons the bar holds, with and without the assistant.
const BAR_WITH_ASSISTANT: f32 = 4.0;
const BAR_PLAIN: f32 = 3.0;
const FILES_BASE: u32 = 1_000;
const EDITOR_BASE: u32 = 2_000;
const CHAT_BASE: u32 = 3_000;
const _: () = assert!(filesview::ACTIONS < 1_000 && editorview::ACTIONS < 1_000 && iyakki_ui_sdk::chatview::ACTIONS < 1_000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Files,
    Editor,
}

pub struct EditApp {
    files: FilesView,
    editor: EditorView,
    chat: Option<ChatView>,
    chat_seen: usize,
    chat_shown: bool,
    pane: Pane,
    zoom: TextZoom,
    /// The Save question is on screen in the file list: when it is answered the editor goes on.
    awaiting_save: bool,
    note: String,
    storage: Option<Box<dyn StoragePort>>,
}

impl EditApp {
    pub fn new(files: Box<dyn FilesPort>) -> EditApp {
        EditApp {
            files: FilesView::new(files, FILES_BASE),
            editor: EditorView::new(EDITOR_BASE),
            chat: None,
            chat_seen: 0,
            chat_shown: false,
            pane: Pane::Files,
            zoom: TextZoom::default(),
            awaiting_save: false,
            note: String::new(),
            storage: None,
        }
    }

    /// The assistant beside the editor: the host hands it a port for a conversation of its own.
    pub fn with_assistant(mut self, port: Box<dyn AssistantPort>) -> EditApp {
        let cfg = SessionConfig { conversation: "edit".into(), title: "Edit".into(), ..SessionConfig::default() };
        let mut chat = ChatView::new(port, cfg, CHAT_BASE);
        chat.set_context(self.editor.assistant_context());
        self.chat = Some(chat);
        self
    }

    /// The text size is read from, and kept in, this port (the core's, in a tile).
    pub fn with_storage(mut self, mut port: Box<dyn StoragePort>) -> EditApp {
        self.zoom = TextZoom::from_text(port.get(ZOOM_KEY).as_deref());
        self.storage = Some(port);
        self
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    pub fn editor(&self) -> &EditorView {
        &self.editor
    }

    pub fn files(&self) -> &FilesView {
        &self.files
    }

    pub fn assistant(&self) -> Option<&ChatView> {
        self.chat.as_ref()
    }

    /// The text size as a thousandth of the person's own size.
    pub fn zoom(&self) -> u32 {
        self.zoom.milli()
    }

    /// What the app last said about itself in words (a refused file, a reply that could not be used).
    pub fn note(&self) -> &str {
        &self.note
    }

    /// Open a file in the editor (the text came from the files port): shows the editor, or says why not.
    pub fn open(&mut self, path: &str, text: &str) {
        match self.editor.load(path, text) {
            Ok(()) => {
                // the list keeps its place but not the preview: Esc there goes back to the editor
                self.files.close();
                self.pane = Pane::Editor;
                self.note.clear();
                self.sync_context();
            }
            Err(e) => self.note = e.chars().take(80).collect(),
        }
    }

    fn keep_zoom(&mut self) {
        if let Some(s) = self.storage.as_mut() {
            let _ = s.set(ZOOM_KEY, &self.zoom.keep_text());
        }
    }

    fn sync_context(&mut self) {
        if let Some(c) = self.chat.as_mut() {
            c.set_context(self.editor.assistant_context());
        }
    }

    /// A new answer of the assistant goes to the editor as a proposal; a reply that cannot be used is said in the note.
    fn assistant_reply(&mut self) {
        let Some(c) = self.chat.as_ref() else { return };
        let answers: Vec<&String> = c.entries.iter().filter_map(|e| if let Entry::Assistant(t) = e { Some(t) } else { None }).collect();
        if answers.len() <= self.chat_seen || c.busy() {
            return;
        }
        self.chat_seen = answers.len();
        let reply = answers.last().map(|s| s.to_string()).unwrap_or_default();
        if let Err(why) = self.editor.offer_reply(&reply) {
            self.note = format!("The assistant's change could not be used: {why}").chars().take(80).collect();
        }
    }

    /// What follows an editor event: a Save the person pressed goes to the files view (which asks first), and the answer to its question
    /// brings the editor back with the text marked saved.
    fn after_editor(&mut self) {
        if let Some((path, text)) = self.editor.take_save() {
            if self.files.save(&path, &text) {
                self.awaiting_save = true;
                self.pane = Pane::Files;
            } else {
                self.note = self.files.error().unwrap_or("The file could not be saved").chars().take(80).collect();
            }
        }
    }

    fn after_files(&mut self) {
        if self.awaiting_save && self.files.pending_question().is_none() {
            self.awaiting_save = false;
            if self.files.note().is_some_and(|n| n.starts_with("Saved")) {
                self.editor.mark_saved();
                self.note.clear();
            } else if let Some(e) = self.files.error() {
                self.note = e.chars().take(80).collect();
            }
            self.pane = Pane::Editor;
        }
        if let Some((path, text)) = self.files.take_opened() {
            self.open(&path, &text);
        }
    }
}

impl UiApp for EditApp {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> String {
        "Edit".into()
    }

    fn dock_status(&self) -> Option<DockStatus> {
        let dirty = self.editor.dirty();
        Some(DockStatus::new(dirty.then_some(1), if dirty { "unsaved changes" } else { "" }))
    }

    fn wants_keyboard(&self) -> bool {
        match self.pane {
            Pane::Files => self.files.wants_keyboard(),
            Pane::Editor => self.editor.wants_keyboard() || self.chat.as_ref().is_some_and(ChatView::wants_keyboard),
        }
    }

    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Gesture(g) => {
                if self.zoom.gesture(g) == ZoomChange::Ended {
                    self.keep_zoom();
                }
                return Flow::Stay;
            }
            UiEvent::Tick => {
                self.files.tick();
                if let Some(c) = self.chat.as_mut() {
                    c.tick();
                }
                self.assistant_reply();
                return Flow::Stay;
            }
            UiEvent::Action(A_FILES) => {
                self.pane = Pane::Files;
                return Flow::Stay;
            }
            UiEvent::Action(A_ASSIST) if self.chat.is_some() => {
                self.chat_shown = !self.chat_shown;
                self.pane = Pane::Editor;
                return Flow::Stay;
            }
            UiEvent::Action(A_SMALLER) => {
                self.zoom.smaller();
                self.keep_zoom();
                return Flow::Stay;
            }
            UiEvent::Action(A_BIGGER) => {
                self.zoom.bigger();
                self.keep_zoom();
                return Flow::Stay;
            }
            _ => {}
        }
        match self.pane {
            Pane::Files => {
                match ev {
                    UiEvent::Wheel(d) => self.files.wheel(*d),
                    UiEvent::Action(a) if self.files.owns(*a) => {
                        self.files.tick();
                        self.files.action(*a);
                    }
                    UiEvent::Text(t) => {
                        self.files.text(t);
                    }
                    UiEvent::Key(k) => {
                        if !self.files.key(*k) && *k == UiKey::Esc && self.files.pending_question().is_none() {
                            // Esc in the folder list: back to the editor when a file is open, otherwise out of the app
                            if self.editor.path.is_empty() {
                                return Flow::Back;
                            }
                            self.pane = Pane::Editor;
                        }
                    }
                    _ => {}
                }
                self.after_files();
            }
            Pane::Editor => {
                self.sync_context();
                if let Some(c) = self.chat.as_mut() {
                    match ev {
                        UiEvent::Key(k) if (c.focused || c.pending_approval().is_some()) && c.key(*k) => {
                            self.assistant_reply();
                            return Flow::Stay;
                        }
                        UiEvent::Text(t) if c.text(t) => return Flow::Stay,
                        UiEvent::Action(a) if c.owns(*a) => {
                            c.action(*a);
                            self.assistant_reply();
                            return Flow::Stay;
                        }
                        UiEvent::Wheel(d) if self.chat_shown && c.focused => {
                            c.wheel(*d);
                            return Flow::Stay;
                        }
                        _ => {}
                    }
                }
                match ev {
                    UiEvent::Wheel(d) => self.editor.wheel(*d),
                    UiEvent::Text(t) => {
                        self.editor.text_in(t);
                    }
                    UiEvent::Action(a) if self.editor.owns(*a) => {
                        self.editor.action(*a);
                    }
                    UiEvent::Key(k) => {
                        if !self.editor.key(*k) && *k == UiKey::Esc {
                            self.pane = Pane::Files;
                        }
                    }
                    _ => {}
                }
                self.after_editor();
            }
        }
        Flow::Stay
    }

    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        let m = self.zoom.metrics(cx);
        let (sm, md) = (m.gap(Space::Sm), m.gap(Space::Md));
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let area = Rect::new(0.0, 0.0, w, h).inset(md);
        // the bar: Files, Assistant, the two size buttons
        let t = m.min_target();
        let (bar, rest) = area.split_top(t);
        let n = if self.chat.is_some() { BAR_WITH_ASSISTANT } else { BAR_PLAIN };
        let bw = ((bar.w - sm * (n - 1.0)) / n).clamp(0.0, m.em(8.0));
        let mut x = bar.x;
        let mut place = |c: &mut dyn Canvas, label: &str, action: u32, on: bool| {
            widgets::button(c, th, &m, Rect::new(x, bar.y, bw, t), label, action, on);
            x += bw + sm;
        };
        place(c, "Files", A_FILES, self.pane == Pane::Files);
        if self.chat.is_some() {
            place(c, "Assistant", A_ASSIST, self.chat_shown && self.pane == Pane::Editor);
        }
        place(c, "A-", A_SMALLER, false);
        place(c, "A+", A_BIGGER, false);
        c.hit(Rect::new(x, bar.y, (bar.right() - x).max(0.0), t), Hit::new(0, self.zoom.label(), Role::Text));
        let body = Rect::new(rest.x, rest.y + sm, rest.w, (rest.h - sm).max(0.0));
        match self.pane {
            Pane::Files => self.files.draw(c, th, &m, body),
            Pane::Editor => {
                let wide = m.class != SizeClass::Compact;
                match self.chat.as_mut() {
                    Some(ch) if wide => {
                        let (l, r) = body.split_left((body.w * 0.62).floor());
                        self.editor.draw(c, th, &m, l);
                        ch.draw(c, th, &m, Rect::new(r.x + sm, r.y, (r.w - sm).max(0.0), r.h));
                    }
                    Some(ch) if self.chat_shown => ch.draw(c, th, &m, body),
                    _ => self.editor.draw(c, th, &m, body),
                }
            }
        }
        if !self.note.is_empty() {
            let s = m.size(TypeRole::Caption);
            c.text(area.x, (area.bottom() - m.leading(s)).max(0.0), s, th.err, &self.note, iyakki_ui_sdk::canvas::TextStyle::Regular);
        }
    }
}

#[cfg(test)]
mod tests;
