//! SAMPLE: a small to-do list that shows what the UI SDK gives an app: one layout for a phone-sized pane and another for a wide one,
//! colours from the Look, a text field, taps and keys, the wallpaper picker, a dock pin button and a dock status line.

use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::files::FilesPort;
use iyakki_ui_sdk::filesview::FilesView;
use iyakki_ui_sdk::audioview::AudioView;
use iyakki_ui_sdk::mediaview::MediaView;
use iyakki_ui_sdk::media::MediaPort;
use iyakki_ui_sdk::terminal::TerminalPort;
use iyakki_ui_sdk::terminalview::TerminalView;
use iyakki_ui_sdk::editorview::EditorView;
use iyakki_ui_sdk::browser::{BrowserEvent, BrowserPort, BrowserView};
use iyakki_ui_sdk::assistant::{AssistantPort, ContextItem, ContextKind, SessionConfig, ToolSpec};
use iyakki_ui_sdk::cameras::CameraPort;
use iyakki_ui_sdk::events::{EventActions, EventRouter, EventsPort};
use std::cell::RefCell;
use std::rc::Rc;
use iyakki_ui_sdk::cameraview::{CameraView, ViewConfig};
use iyakki_ui_sdk::chatview::{ChatView, ToolInvocation};
use iyakki_ui_sdk::toolargs::Args;
use iyakki_ui_sdk::canvas::{fit, Canvas, Hit, Role, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::desktop::DockStatus;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::ports::StoragePort;
use iyakki_ui_sdk::size::{SizeClass, TypeRole};
use iyakki_ui_sdk::tokens::{Radius, Space};
use iyakki_ui_sdk::widgets;
use iyakki_ui_sdk::wallpaper as wp;

/// The app id the dock, the launcher and the policy use.
pub const ID: &str = "sample";
/// The most notes kept and the longest one.
pub const MAX_ITEMS: usize = 200;
pub const MAX_LEN: usize = 120;

/// Layout proportions: the list's share of a wide pane, and button widths in text sizes.
const LIST_SHARE: f32 = 0.58;
const ADD_EM: f32 = 4.5;
const WIDE_BUTTON_EM: f32 = 10.0;
const A_FOCUS: u32 = 1;
const A_ADD: u32 = 2;
const A_CLEAR: u32 = 3;
const A_PIN: u32 = 4;
const A_ASSIST: u32 = 5;
/// The action numbers of the embedded chat view (`iyakki_ui_sdk::chatview::ACTIONS` of them).
const A_CHAT: u32 = 20_000;
const A_ROW: u32 = 100;
const A_WALL: u32 = 10_000;
// the browser pane (F-SDK-61): two tabs, each a `BrowserView` that owns an action range of its own
const A_TAB_NOTES: u32 = 20;
const A_TAB_BROWSER: u32 = 21;
const A_TAB_FILES: u32 = 22;
const A_TAB_TERM: u32 = 23;
const A_TAB_EDIT: u32 = 24;
const A_TAB_CAMERAS: u32 = 25;
/// The action numbers of the embedded camera view (`iyakki_ui_sdk::cameraview::ACTIONS` of them).
const A_CAMERAS: u32 = 50_000;
const A_TAB_SOUND: u32 = 26;
const A_TAB_EVENTS: u32 = 27;
const A_EDIT_CHAT: u32 = 25;
/// The action numbers of the editor's own assistant conversation.
const A_EDITCHAT: u32 = 53_000;
/// The action numbers of the embedded file browser, terminal and editor (`ACTIONS` of each view; the browser views use 20 000 and 30 000).
const A_FILEVIEW: u32 = 50_000;
const A_TERMVIEW: u32 = 51_000;
const A_EDITVIEW: u32 = 52_000;
const A_AUDIOVIEW: u32 = 54_000;
const A_B_BACK: u32 = 30;
const A_B_FWD: u32 = 31;
const A_B_RELOAD: u32 = 32;
const A_B_GO: u32 = 33;
const A_B_ADDR: u32 = 34;
const A_B_FIND: u32 = 35;
const A_B_NEXT: u32 = 36;
const A_B_VIEW: u32 = 40;
const VIEW_BASE: u32 = 20_000;
/// The two views of the browser pane.
pub const BROWSER_VIEWS: usize = 2;
/// The address a view with no page asks the person for (shown as the field's label).
const ADDR_LABEL: &str = "Address or search";

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Notes,
    Browser,
    Files,
    Terminal,
    Editor,
    Cameras,
    Sound,
    Events,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Addr,
    Find,
    Page,
}

/// One tab of the browser pane: the embedded view, what the person typed in the address field and the page title for the tab's label.
struct BrowserTab {
    view: BrowserView,
    addr: String,
    title: String,
}

pub struct SampleApp {
    pub items: Vec<Item>,
    pub input: String,
    pub focused: bool,
    pub selected: usize,
    /// What the last press of the dock button said (shown under it).
    pub note: String,
    /// The notes are kept here when the app was given a storage port (through the core); none: they live in memory only.
    storage: Option<Box<dyn StoragePort>>,
    pub pane: Pane,
    tabs: Vec<BrowserTab>,
    cur: usize,
    focus: Focus,
    find_q: String,
    /// The embedded assistant (given by the host through `with_assistant`); none: the app has no assistant pane.
    chat: Option<ChatView>,
    /// Where the assistant shows: on a narrow pane it replaces the list, on a wide one it replaces the side panel; a button switches each.
    chat_compact: bool,
    chat_wide: bool,
    /// The embedded file browser (given by the host through `with_files`); none: no Files pane.
    files: Option<FilesView>,
    /// The embedded terminal (`with_terminal`); none: no Terminal pane.
    term: Option<TerminalView>,
    /// The embedded editor over the notes as text (`with_editor`); none: no Edit pane.
    editor: Option<EditorView>,
    /// The assistant beside the editor (a second conversation, `with_editor_assistant`): it is given the document and the selection and its
    /// replies go to the editor as proposals to review.
    edit_chat: Option<ChatView>,
    edit_chat_seen: usize,
    edit_chat_shown: bool,
    /// The editor holds this file of the Files pane (from `e` on an opened file): its Save goes to the core as a change the person allows.
    editor_file: Option<String>,
    /// The embedded camera view (`with_cameras`); none: no Cameras pane.
    cams: Option<CameraView>,
    /// The events pane (`with_events`): what the app hears from the core, online and offline; none: no Events pane.
    events: Option<EventsPane>,
    /// The embedded sound and video player (`with_media`); none: no Media pane.
    audio: Option<MediaView>,
}

/// The storage key the notes are kept under: one line per note, `0` or `1` (done), a space, the text.
pub const NOTES_KEY: &str = "notes";

impl Default for SampleApp {
    fn default() -> Self {
        SampleApp::new()
    }
}

impl SampleApp {
    pub fn new() -> SampleApp {
        SampleApp { items: vec![Item { text: "Try the wallpaper picker".into(), done: false }, Item { text: "Resize the window".into(), done: false }], input: String::new(), focused: false, selected: 0, note: String::new(), storage: None, pane: Pane::Notes, tabs: Vec::new(), cur: 0, focus: Focus::None, find_q: String::new(), chat: None, chat_compact: false, chat_wide: true, files: None, term: None, editor: None, edit_chat: None, edit_chat_seen: 0, edit_chat_shown: false, cams: None, events: None, audio: None, editor_file: None }
    }

    /// The app with its notes kept through a storage port: what was saved comes back, and every change is saved.
    pub fn with_storage(mut port: Box<dyn StoragePort>) -> SampleApp {
        let mut a = SampleApp::new();
        if let Some(text) = port.get(NOTES_KEY) {
            a.items = text
                .lines()
                .filter_map(|l| l.split_once(' ').filter(|(d, _)| matches!(*d, "0" | "1")).map(|(d, t)| Item { text: t.chars().filter(|c| !c.is_control()).take(MAX_LEN).collect(), done: d == "1" }))
                .take(MAX_ITEMS)
                .collect();
        }
        a.storage = Some(port);
        a
    }

    /// Give the app a browser pane: `port` makes one core port per view (the core's browser sessions on the device, an in-memory one in a
    /// test). Without it the app has the notes only.
    pub fn with_browser(mut self, mut port: impl FnMut() -> Box<dyn BrowserPort>) -> SampleApp {
        self.tabs = (0..BROWSER_VIEWS).map(|i| BrowserTab { view: BrowserView::new(port(), VIEW_BASE + i as u32 * 10_000), addr: String::new(), title: String::new() }).collect();
        self
    }

    /// The app with a Files pane: the host hands it the port (the core's). Opening a text file puts its first line in the status line.
    pub fn with_files(mut self, port: Box<dyn FilesPort>) -> SampleApp {
        self.files = Some(FilesView::new(port, A_FILEVIEW));
        self
    }

    /// The app with a Media pane (sound and video): the host hands it the port (the core's media player). The core decodes; the sound is mixed by the core.
    pub fn with_media(mut self, port: Box<dyn MediaPort>) -> SampleApp {
        self.audio = Some(MediaView::new(port, A_AUDIOVIEW));
        self
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    pub fn audio_open(&self) -> bool {
        self.pane == Pane::Sound
    }

    /// The embedded sound view (tests read what it shows).
    pub fn audio(&self) -> Option<&AudioView> {
        self.audio.as_ref().map(|m| m.audio())
    }

    /// The app with a Terminal pane: the host hands it the port (the core's). Lines that change something wait for the person's yes.
    pub fn with_terminal(mut self, port: Box<dyn TerminalPort>) -> SampleApp {
        self.term = Some(TerminalView::new(port, A_TERMVIEW));
        self
    }

    /// The app with an Edit pane: the notes as text; Save turns the lines back into notes.
    pub fn with_editor(mut self) -> SampleApp {
        self.editor = Some(EditorView::new(A_EDITVIEW));
        self
    }

    /// The editor's assistant: the host hands it a port for a conversation of its own ("editor").
    pub fn with_editor_assistant(mut self, port: Box<dyn AssistantPort>) -> SampleApp {
        let cfg = SessionConfig { conversation: "editor".into(), title: "SAMPLE editor".into(), ..SessionConfig::default() };
        self.edit_chat = Some(ChatView::new(port, cfg, A_EDITCHAT));
        self.sync_editor_context();
        self
    }

    pub fn editor_chat(&self) -> Option<&ChatView> {
        self.edit_chat.as_ref()
    }

    fn sync_editor_context(&mut self) {
        if let (Some(c), Some(e)) = (self.edit_chat.as_mut(), self.editor.as_ref()) {
            c.set_context(e.assistant_context());
        }
    }

    /// A new answer of the editor's assistant goes to the editor as a proposal; a reply that cannot be used is said under the pin button.
    fn editor_reply(&mut self) {
        let Some(c) = self.edit_chat.as_ref() else { return };
        let answers: Vec<String> = c.entries.iter().filter_map(|e| if let iyakki_ui_sdk::chatview::Entry::Assistant(t) = e { Some(t.clone()) } else { None }).collect();
        if answers.len() <= self.edit_chat_seen || c.busy() {
            return;
        }
        self.edit_chat_seen = answers.len();
        let reply = answers.last().cloned().unwrap_or_default();
        if let Some(e) = self.editor.as_mut() {
            if let Err(why) = e.offer_reply(&reply) {
                self.note = format!("The assistant's change could not be used: {why}").chars().take(60).collect();
            }
        }
    }

    /// Give the app a cameras pane: the host hands it the port (the core's camera views). The person ticks cameras, allows the card and
    /// watches; the app keeps nothing of what it sees.
    pub fn with_cameras(mut self, port: Box<dyn CameraPort>) -> SampleApp {
        self.cams = Some(CameraView::new(port, ViewConfig { label: "Sample".into(), ..ViewConfig::default() }, A_CAMERAS));
        self
    }

    /// Like [`SampleApp::with_cameras`] with the pictures as JPEG, for a canvas that makes a display list for a browser or client.
    pub fn with_cameras_for_the_wire(mut self, port: Box<dyn CameraPort>) -> SampleApp {
        self.cams = Some(CameraView::new(port, ViewConfig { label: "Sample".into(), format: iyakki_ui_sdk::canvas::ImageFormat::Jpeg, ..ViewConfig::default() }, A_CAMERAS));
        self
    }

    pub fn has_cameras(&self) -> bool {
        self.cams.is_some()
    }

    /// The camera view (for the shell and for tests).
    pub fn cameras(&mut self) -> Option<&mut CameraView> {
        self.cams.as_mut()
    }

    pub fn has_files(&self) -> bool {
        self.files.is_some()
    }

    pub fn files_open(&self) -> bool {
        self.pane == Pane::Files
    }

    /// The embedded file view (tests read what it shows).
    pub fn files(&self) -> Option<&FilesView> {
        self.files.as_ref()
    }

    pub fn has_terminal(&self) -> bool {
        self.term.is_some()
    }

    pub fn terminal_open(&self) -> bool {
        self.pane == Pane::Terminal
    }

    pub fn terminal(&self) -> Option<&TerminalView> {
        self.term.as_ref()
    }

    pub fn editor_open(&self) -> bool {
        self.pane == Pane::Editor
    }

    pub fn editor(&self) -> Option<&EditorView> {
        self.editor.as_ref()
    }

    /// Give the app an events pane: the host hands it the port (the core's app events) and the actions its handlers may take. The app
    /// registers for the network and the power source, handles what waited while it was away (marked late) before the first frame, and
    /// shows what it heard. A refusal (a permission the person has not given) is shown as a sentence.
    pub fn with_events(mut self, mut port: Box<dyn EventsPort>, mut actions: Box<dyn EventActions>) -> SampleApp {
        let st = Rc::new(RefCell::new(EventsState::default()));
        let (a, b, c, d, e2, f) = (st.clone(), st.clone(), st.clone(), st.clone(), st.clone(), st.clone());
        let (g1, g2, g3, g4, g5) = (st.clone(), st.clone(), st.clone(), st.clone(), st.clone());
        let mut router = EventRouter::new()
            .on("connectivity.changed", move |e, acts| {
                let s = e.text("state").unwrap_or("?").to_string();
                a.borrow_mut().note(e.seq, e.late, format!("network {s}"));
                a.borrow_mut().network = s.clone();
                // a late event is old news: only a live one leaves a note
                if !e.late && s == "offline" {
                    let _ = acts.notify("The network went away");
                }
            })
            .on("power.source", move |e, _| {
                let s = e.text("source").unwrap_or("?").to_string();
                b.borrow_mut().note(e.seq, e.late, format!("power {s}"));
                b.borrow_mut().power = s;
            })
            .on("session.changed", move |e, _| {
                let s = e.text("state").unwrap_or("?").to_string();
                d.borrow_mut().note(e.seq, e.late, format!("session {s}"));
                d.borrow_mut().session = s;
            })
            .on("settings.changed", move |e, _| {
                let s = e.text("setting").unwrap_or("?").to_string();
                e2.borrow_mut().note(e.seq, e.late, format!("setting {s}"));
                e2.borrow_mut().setting = s;
            })
            // what happens around the app (names only; a late one is marked): which app is in front, a volume, a USB device, a sign of sharing
            .on("app.lifecycle", move |e, _| {
                let line = format!("app {} {}", e.text("app").unwrap_or("?"), e.text("change").unwrap_or("?"));
                g1.borrow_mut().note(e.seq, e.late, line);
            })
            .on("storage.volume", move |e, _| {
                let line = format!("volume {} {}", e.text("volume").unwrap_or("?"), e.text("change").unwrap_or("?"));
                g2.borrow_mut().note(e.seq, e.late, line);
            })
            .on("usb.device", move |e, _| {
                let line = format!("usb {} {}", e.text("change").unwrap_or("?"), e.text("device").unwrap_or("?"));
                g3.borrow_mut().note(e.seq, e.late, line);
            })
            .on("sharing.changed", move |e, _| {
                let line = format!("sharing {} {}", e.text("kind").unwrap_or("?"), e.text("state").unwrap_or("?"));
                g4.borrow_mut().note(e.seq, e.late, line);
            })
            .on_wake(move |tag, e, _| g5.borrow_mut().note(e.seq, e.late, format!("wake {tag}")))
            .on_permissions(move |ch, e, _| {
                let line = format!("{} held; +[{}] -[{}] by {}{}", ch.held, ch.granted.join(","), ch.revoked.join(","), ch.by, if ch.restart { "; restart needed" } else { "" });
                f.borrow_mut().note(e.seq, e.late, format!("permissions {line}"));
                f.borrow_mut().permissions = line;
            })
            .on_missed(move |e, _| c.borrow_mut().note(e.seq, true, format!("{} events were missed", e.missed_count())));
        st.borrow_mut().refusals = router.register_all(port.as_mut());
        router.begin(port.as_mut(), actions.as_mut());
        self.events = Some(EventsPane { port, actions, router, st });
        self
    }

    pub fn has_events(&self) -> bool {
        self.events.is_some()
    }

    /// The panes this app has, in tab order.
    fn panes(&self) -> Vec<(&'static str, u32, Pane)> {
        let mut v = vec![("Notes", A_TAB_NOTES, Pane::Notes)];
        if self.has_browser() {
            v.push(("Browser", A_TAB_BROWSER, Pane::Browser));
        }
        if self.files.is_some() {
            v.push(("Files", A_TAB_FILES, Pane::Files));
        }
        if self.term.is_some() {
            v.push(("Terminal", A_TAB_TERM, Pane::Terminal));
        }
        if self.editor.is_some() {
            v.push(("Edit", A_TAB_EDIT, Pane::Editor));
        }
        if self.cams.is_some() {
            v.push(("Cameras", A_TAB_CAMERAS, Pane::Cameras));
        }
        if self.audio.is_some() {
            v.push(("Media", A_TAB_SOUND, Pane::Sound));
        }
        if self.events.is_some() {
            v.push(("Events", A_TAB_EVENTS, Pane::Events));
        }
        v
    }

    fn has_tabs(&self) -> bool {
        self.panes().len() > 1
    }

    /// Show a pane: the terminal's session ends when it is left, the editor takes the notes as they are now.
    fn show(&mut self, pane: Pane) {
        if self.pane == Pane::Terminal && pane != Pane::Terminal {
            if let Some(t) = self.term.as_mut() {
                t.close();
            }
        }
        // leaving the cameras pane closes the view: nothing is pulled from a camera that is not on screen
        if self.pane == Pane::Cameras && pane != Pane::Cameras {
            if let Some(v) = self.cams.as_mut() {
                v.close();
            }
        }
        if pane == Pane::Editor && self.pane != Pane::Editor && self.editor_file.is_none() {
            let text: String = self.items.iter().map(|i| format!("{}{}\n", if i.done { "[x] " } else { "" }, i.text)).collect();
            if let Some(e) = self.editor.as_mut() {
                let _ = e.load("notes.txt", &text);
            }
        }
        if pane != Pane::Editor && pane != Pane::Files {
            self.editor_file = None;
        }
        self.pane = pane;
        if pane == Pane::Editor {
            self.sync_editor_context();
        }
    }

    /// The editor's Save: the lines become the notes again (at most the limits of the list).
    fn editor_saved(&mut self) {
        let Some((_, text)) = self.editor.as_mut().and_then(|e| e.take_save()) else { return };
        if let Some(path) = self.editor_file.clone() {
            // a file of the person's home: the core asks before it is written; the question shows in the Files pane
            if let Some(f) = self.files.as_mut() {
                f.save(&path, &text);
            }
            self.pane = Pane::Files;
            self.note = format!("Saving {path}: answer the question").chars().take(60).collect();
            return;
        }
        self.items = text
            .lines()
            .map(|l| l.trim_end())
            .filter(|l| !l.is_empty())
            .map(|l| match l.strip_prefix("[x] ") {
                Some(t) => Item { text: t.chars().filter(|c| !c.is_control()).take(MAX_LEN).collect(), done: true },
                None => Item { text: l.chars().filter(|c| !c.is_control()).take(MAX_LEN).collect(), done: false },
            })
            .take(MAX_ITEMS)
            .collect();
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        self.save();
        if let Some(e) = self.editor.as_mut() {
            e.mark_saved();
        }
        self.note = "Notes saved from the editor".into();
    }

    /// A text file the person opened in the Files pane: its first line goes under the pin button.
    fn note_opened(&mut self) {
        if let Some((path, text)) = self.files.as_mut().and_then(|f| f.take_opened()) {
            let first: String = text.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).take(40).collect();
            self.note = format!("{path}: {first}");
        }
    }

    /// Events of the Files, Terminal and Edit panes (and their tabs). `None`: not theirs.
    fn extra_event(&mut self, ev: &UiEvent) -> Option<Flow> {
        // the sound goes on when the person looks at another pane: its events are taken on every tick
        if matches!(ev, UiEvent::Tick) {
            if let Some(a) = self.audio.as_mut() {
                a.tick();
            }
        }
        match ev {
            UiEvent::Action(a) => {
                for (_, action, pane) in self.panes() {
                    if *a == action && pane != Pane::Browser {
                        self.show(pane);
                        return Some(Flow::Stay);
                    }
                }
            }
            _ => {}
        }
        match self.pane {
            Pane::Files => {
                let f = self.files.as_mut()?;
                match ev {
                    UiEvent::Tick => f.tick(),
                    UiEvent::Wheel(d) => f.wheel(*d),
                    UiEvent::Action(a) if f.owns(*a) => {
                        f.tick();
                        f.action(*a);
                        self.note_opened();
                    }
                    UiEvent::Text(x) => {
                        f.text(x);
                    }
                    UiEvent::Key(UiKey::Char('e')) if f.preview().is_some() && !f.wants_keyboard() && f.pending_question().is_none() && self.editor.is_some() => {
                        let (path, text) = f.preview().cloned().unwrap_or_default();
                        if let Some(e) = self.editor.as_mut() {
                            if e.load(&path, &text).is_ok() {
                                self.editor_file = Some(path);
                                self.pane = Pane::Editor;
                                self.sync_editor_context();
                            }
                        }
                    }
                    UiEvent::Key(k) => {
                        if f.key(*k) {
                            self.note_opened();
                        } else if *k == UiKey::Esc {
                            self.show(Pane::Notes);
                        }
                    }
                    _ => return None,
                }
                if let Some(e) = self.editor.as_mut() {
                    // the person answered the save question with yes: the editor's text is now the file's
                    if self.editor_file.is_some() && self.files.as_ref().is_some_and(|f| f.pending_question().is_none()) && self.files.as_ref().and_then(|f| f.note()).is_some_and(|n| n.starts_with("Saved")) {
                        e.mark_saved();
                    }
                }
                Some(Flow::Stay)
            }
            Pane::Cameras => {
                let v = self.cams.as_mut()?;
                match ev {
                    UiEvent::Key(k) if v.key(*k) => {}
                    UiEvent::Key(UiKey::Esc) => self.show(Pane::Notes),
                    UiEvent::Action(a) if v.owns(*a) => {
                        v.action(*a);
                    }
                    _ => return None,
                }
                Some(Flow::Stay)
            }
            Pane::Events => {
                match ev {
                    UiEvent::Tick => {
                        if let Some(e) = self.events.as_mut() {
                            e.pump();
                        }
                    }
                    UiEvent::Key(UiKey::Esc) => self.show(Pane::Notes),
                    _ => return None,
                }
                Some(Flow::Stay)
            }
            Pane::Sound => {
                let a = self.audio.as_mut()?;
                match ev {
                    UiEvent::Wheel(d) => a.wheel(*d),
                    UiEvent::Action(x) if a.owns(*x) => {
                        a.tick();
                        a.action(*x);
                    }
                    UiEvent::Key(k) => {
                        if !a.key(*k) && *k == UiKey::Esc {
                            self.show(Pane::Notes);
                        }
                    }
                    UiEvent::Tick => {}
                    _ => return None,
                }
                Some(Flow::Stay)
            }
            Pane::Terminal => {
                let t = self.term.as_mut()?;
                match ev {
                    UiEvent::Tick => t.tick(),
                    UiEvent::Wheel(d) => t.wheel(*d),
                    UiEvent::Text(x) => {
                        t.text(x);
                    }
                    UiEvent::Action(a) if t.owns(*a) => {
                        t.tick();
                        t.action(*a);
                    }
                    UiEvent::Key(k) => {
                        if !t.key(*k) && *k == UiKey::Esc {
                            self.show(Pane::Notes);
                        }
                    }
                    _ => return None,
                }
                Some(Flow::Stay)
            }
            Pane::Editor => {
                self.sync_editor_context();
                if let Some(c) = self.edit_chat.as_mut() {
                    match ev {
                        UiEvent::Tick => {
                            c.tick();
                        }
                        UiEvent::Key(k) if (c.focused || c.pending_approval().is_some()) && c.key(*k) => {
                            self.editor_reply();
                            return Some(Flow::Stay);
                        }
                        UiEvent::Text(t) if c.text(t) => return Some(Flow::Stay),
                        UiEvent::Action(A_EDIT_CHAT) => {
                            self.edit_chat_shown = !self.edit_chat_shown;
                            return Some(Flow::Stay);
                        }
                        UiEvent::Action(a) if c.owns(*a) => {
                            c.action(*a);
                            self.editor_reply();
                            return Some(Flow::Stay);
                        }
                        _ => {}
                    }
                    self.editor_reply();
                }
                self.sync_editor_context();
                let e = self.editor.as_mut()?;
                match ev {
                    UiEvent::Wheel(d) => e.wheel(*d),
                    UiEvent::Text(x) => {
                        e.text_in(x);
                    }
                    UiEvent::Action(a) if e.owns(*a) => {
                        e.action(*a);
                    }
                    UiEvent::Key(k) => {
                        if !e.key(*k) && *k == UiKey::Esc {
                            self.show(Pane::Notes);
                        }
                    }
                    _ => return None,
                }
                self.editor_saved();
                Some(Flow::Stay)
            }
            _ => None,
        }
    }

    pub fn has_browser(&self) -> bool {
        !self.tabs.is_empty()
    }

    /// Open an address in the current view of the browser pane and show the pane.
    pub fn browse(&mut self, url: &str) {
        if let Some(t) = self.tabs.get_mut(self.cur) {
            t.addr = url.to_string();
            t.view.load_url(url);
            self.pane = Pane::Browser;
        }
    }

    /// The view the pane shows now (for the shell and for tests).
    pub fn current_view(&mut self) -> Option<&mut BrowserView> {
        self.tabs.get_mut(self.cur).map(|t| &mut t.view)
    }

    /// The conversation name, and the tools the app offers to its assistant: adding a note and checking one off ask the person first, reading
    /// the list does not. The model can call nothing else.
    pub fn assistant_config() -> SessionConfig {
        SessionConfig {
            conversation: "notes".into(),
            title: "SAMPLE notes".into(),
            tools: vec![
                ToolSpec::new("add_note", "Add one note to the list.", "{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\",\"description\":\"the note\"}},\"required\":[\"text\"]}", true),
                ToolSpec::new("check_note", "Check off the note with this number (as numbered in list_notes).", "{\"type\":\"object\",\"properties\":{\"number\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"number\"]}", true),
                ToolSpec::new("list_notes", "List the notes with their numbers and whether they are done.", "{\"type\":\"object\",\"properties\":{}}", false),
            ],
            ..SessionConfig::default()
        }
    }

    /// The app with an assistant pane: the host hands it the port (the core's). The assistant sees the list and the selected note as
    /// context and changes the list only through the tools above.
    pub fn with_assistant(mut self, port: Box<dyn AssistantPort>) -> SampleApp {
        self.chat = Some(ChatView::new(port, SampleApp::assistant_config(), A_CHAT));
        self.sync_context();
        self
    }

    pub fn has_assistant(&self) -> bool {
        self.chat.is_some()
    }

    /// The embedded view (tests read its transcript).
    pub fn chat(&self) -> Option<&ChatView> {
        self.chat.as_ref()
    }

    fn sync_context(&mut self) {
        let Some(chat) = self.chat.as_mut() else { return };
        let list: String = self.items.iter().enumerate().map(|(i, it)| format!("{} [{}] {}\n", i + 1, if it.done { "x" } else { " " }, it.text)).collect();
        let mut ctx = vec![ContextItem::new(ContextKind::Document, "Notes", if list.is_empty() { "(no notes)" } else { &list })];
        if let Some(it) = self.items.get(self.selected) {
            ctx.push(ContextItem::new(ContextKind::Selection, "Selected note", &format!("{} {}", self.selected + 1, it.text)));
        }
        chat.set_context(ctx);
    }

    /// Run one tool of the assistant on the notes. The person has already allowed it where it asks; what it does is checked again here.
    pub fn run_tool(&mut self, inv: &ToolInvocation) -> Result<String, String> {
        let args = Args::parse(&inv.args)?;
        match inv.tool.as_str() {
            "add_note" => {
                let t: String = args.str("text").ok_or("add_note needs `text`")?.trim().chars().filter(|c| !c.is_control()).take(MAX_LEN).collect();
                if t.is_empty() {
                    return Err("the note is empty".into());
                }
                if self.items.len() >= MAX_ITEMS {
                    return Err(format!("the list is full ({MAX_ITEMS} notes)"));
                }
                self.items.push(Item { text: t.clone(), done: false });
                self.selected = self.items.len() - 1;
                self.save();
                Ok(format!("Added \"{t}\" as note {}", self.items.len()))
            }
            "check_note" => {
                let n = args.uint("number").ok_or("check_note needs a whole `number`")? as usize;
                let it = n.checked_sub(1).and_then(|i| self.items.get_mut(i)).ok_or_else(|| format!("there is no note {n}"))?;
                it.done = true;
                let text = it.text.clone();
                self.selected = n - 1;
                self.save();
                Ok(format!("Checked off note {n}: {text}"))
            }
            "list_notes" => Ok(if self.items.is_empty() { "No notes.".to_string() } else { self.items.iter().enumerate().map(|(i, it)| format!("{} [{}] {}", i + 1, if it.done { "x" } else { " " }, it.text)).collect::<Vec<_>>().join("\n") }),
            other => Err(format!("this app has no tool `{other}`")),
        }
    }

    /// Keep the notes (after a change); a refusal is said under the pin button, never lost silently.
    fn save(&mut self) {
        let Some(p) = self.storage.as_mut() else { return };
        let text: String = self.items.iter().map(|i| format!("{} {}\n", u8::from(i.done), i.text)).collect();
        if p.set(NOTES_KEY, &text).is_err() {
            self.note = "The notes could not be saved".into();
        }
    }

    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|i| !i.done).count()
    }

    fn add(&mut self) {
        let t: String = self.input.trim().chars().filter(|c| !c.is_control()).take(MAX_LEN).collect();
        if !t.is_empty() && self.items.len() < MAX_ITEMS {
            self.items.push(Item { text: t, done: false });
            self.selected = self.items.len() - 1;
            self.save();
        }
        self.input.clear();
    }

    fn toggle(&mut self, i: usize) {
        if let Some(it) = self.items.get_mut(i) {
            it.done = !it.done;
            self.selected = i;
            self.save();
        }
    }

    fn key(&mut self, k: UiKey) -> Flow {
        // the assistant's field (or its approval card) takes the keys first
        if let Some(c) = self.chat.as_mut() {
            if (c.focused || c.pending_approval().is_some()) && c.key(k) {
                return Flow::Stay;
            }
        }
        if self.focused {
            match k {
                UiKey::Char(c) if !c.is_control() && self.input.chars().count() < MAX_LEN => self.input.push(c),
                UiKey::Backspace => {
                    self.input.pop();
                }
                UiKey::Enter => self.add(),
                UiKey::Esc | UiKey::Tab => self.focused = false,
                _ => {}
            }
            return Flow::Stay;
        }
        match k {
            UiKey::Esc => return Flow::Back,
            UiKey::Tab | UiKey::Char('/') => self.focused = true,
            UiKey::Char('?') if self.chat.is_some() => {
                self.chat_compact = true;
                self.chat_wide = true;
                if let Some(c) = self.chat.as_mut() {
                    c.focused = true;
                }
            }
            UiKey::Char('f') if self.files.is_some() => self.show(Pane::Files),
            UiKey::Char('t') if self.term.is_some() => self.show(Pane::Terminal),
            UiKey::Char('e') if self.editor.is_some() => self.show(Pane::Editor),
            UiKey::Char('u') if self.audio.is_some() => self.show(Pane::Sound),
            UiKey::Up => self.selected = self.selected.saturating_sub(1),
            UiKey::Down => self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1)),
            UiKey::Enter | UiKey::Char(' ') => self.toggle(self.selected),
            UiKey::Delete | UiKey::Backspace if self.selected < self.items.len() => {
                self.items.remove(self.selected);
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.save();
            }
            _ => {}
        }
        Flow::Stay
    }
}

impl UiApp for SampleApp {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> String {
        "SAMPLE".into()
    }

    fn dock_status(&self) -> Option<DockStatus> {
        let n = self.open_count();
        Some(DockStatus::new(if n > 0 { Some(n as u32) } else { None }, &format!("{n} open")))
    }

    fn wants_keyboard(&self) -> bool {
        self.focused
            || self.chat.as_ref().is_some_and(ChatView::wants_keyboard)
            || (self.pane == Pane::Files && self.files.as_ref().is_some_and(|f| f.wants_keyboard()))
            || (self.pane == Pane::Terminal && self.term.as_ref().is_some_and(|t| t.wants_keyboard()))
            || (self.pane == Pane::Editor && (self.editor.as_ref().is_some_and(|e| e.wants_keyboard()) || self.edit_chat.as_ref().is_some_and(|c| c.wants_keyboard())))
    }

    fn event(&mut self, ev: &UiEvent, cx: &mut UiCx) -> Flow {
        let flow = self.event_main(ev, cx);
        self.sync_cameras(ev, cx, flow == Flow::Back);
        // leaving the app: what happens from now on waits for the next start, marked late
        if flow == Flow::Back {
            if let Some(e) = self.events.as_mut() {
                e.port.stop();
            }
        }
        flow
    }

    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        let m = cx.metrics();
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let body = Rect::new(g, g, (w - 2.0 * g).max(0.0), (h - 2.0 * g).max(0.0));
        // header: the title and what kind of space this is
        let ts = m.size(TypeRole::Heading);
        c.text(body.x, body.y, ts, th.text, "Sample", TextStyle::Bold);
        let sub = format!("{} pane, {} x {}, view {}", m.class.name(), w.round(), h.round(), cx.view().name);
        let ss = m.size(TypeRole::Caption);
        let sub = fit(c, ss, &sub, TextStyle::Regular, body.w);
        c.text(body.x, body.y + m.leading(ts), ss, th.muted, &sub, TextStyle::Regular);
        let (_, mut rest) = body.split_top(m.leading(ts) + m.leading(ss) + m.gap(Space::Md));
        if self.has_tabs() {
            let (tabs, r) = rest.split_top(m.min_target() + m.gap(Space::Sm));
            self.draw_pane_tabs(c, cx, tabs);
            rest = r;
            match self.pane {
                Pane::Files => {
                    if let Some(f) = self.files.as_mut() {
                        f.tick();
                        f.draw(c, th, &m, rest);
                    }
                    return;
                }
                Pane::Cameras => {
                    if let Some(v) = self.cams.as_mut() {
                        v.draw(c, th, &m, rest);
                    }
                    return;
                }
                Pane::Sound => {
                    if let Some(a) = self.audio.as_mut() {
                        a.draw(c, th, &m, rest);
                    }
                    return;
                }
                Pane::Events => {
                    if let Some(e) = self.events.as_ref() {
                        e.draw(c, cx, rest);
                    }
                    return;
                }
                Pane::Terminal => {
                    if let Some(t) = self.term.as_mut() {
                        t.tick();
                        t.draw(c, th, &m, rest);
                    }
                    return;
                }
                Pane::Editor => {
                    let wide = m.class != SizeClass::Compact;
                    match (self.editor.as_mut(), self.edit_chat.as_mut()) {
                        (Some(e), Some(ch)) if wide => {
                            let (l, r) = rest.split_left((rest.w * 0.62).floor());
                            e.draw(c, th, &m, l);
                            ch.draw(c, th, &m, Rect::new(r.x + m.gap(Space::Sm), r.y, (r.w - m.gap(Space::Sm)).max(0.0), r.h));
                        }
                        (Some(e), Some(ch)) => {
                            let (area, bar) = rest.split_bottom(m.min_target() + m.gap(Space::Sm));
                            if self.edit_chat_shown {
                                ch.draw(c, th, &m, area);
                            } else {
                                e.draw(c, th, &m, area);
                            }
                            widgets::button(c, th, &m, Rect::new(bar.x, bar.bottom() - m.min_target(), bar.w, m.min_target()), if self.edit_chat_shown { "Editor" } else { "Assistant" }, A_EDIT_CHAT, false);
                        }
                        (Some(e), None) => e.draw(c, th, &m, rest),
                        _ => {}
                    }
                    return;
                }
                _ => {}
            }
            if self.pane == Pane::Browser {
                self.draw_browser(c, cx, rest);
                return;
            }
        }
        // compact with the assistant showing: the assistant takes the whole pane under a button back to the notes
        if m.class == SizeClass::Compact && self.chat_compact && self.chat.is_some() {
            let (bar, rest) = rest.split_top(m.min_target());
            widgets::button(c, th, &m, bar, "Back to notes", A_ASSIST, false);
            let area = Rect::new(rest.x, rest.y + m.gap(Space::Sm), rest.w, (rest.h - m.gap(Space::Sm)).max(0.0));
            if let Some(ch) = self.chat.as_mut() {
                ch.draw(c, th, &m, area);
            }
            return;
        }
        let (list_area, side) = match m.class {
            SizeClass::Compact => (rest, None),
            _ => {
                let (l, r) = rest.split_left(rest.w * LIST_SHARE);
                (l, Some(Rect::new(r.x + m.gap(Space::Xl), r.y, (r.w - m.gap(Space::Xl)).max(0.0), r.h)))
            }
        };
        self.draw_list(c, cx, list_area);
        if let Some(s) = side {
            // wide: the assistant takes the side panel when the app has one
            match self.chat.as_mut() {
                Some(ch) if self.chat_wide => ch.draw(c, th, &m, s),
                _ => self.draw_side(c, cx, s),
            }
        } else {
            // compact: the pin button is a line at the bottom of the list's space (the assistant button shares it)
            let (_, bar) = list_area.split_bottom(m.min_target());
            let pinned = cx.dock.pinned().iter().any(|a| a == ID);
            let pin_label = if pinned { "Unpin from dock" } else { "Pin to dock" };
            if self.chat.is_some() {
                let (l, r) = bar.split_left(bar.w / 2.0);
                widgets::button(c, th, &m, Rect::new(l.x, l.y, (l.w - m.gap(Space::Xs)).max(0.0), l.h), pin_label, A_PIN, false);
                widgets::button(c, th, &m, Rect::new(r.x + m.gap(Space::Xs), r.y, (r.w - m.gap(Space::Xs)).max(0.0), r.h), "Assistant", A_ASSIST, true);
            } else {
                widgets::button(c, th, &m, bar, pin_label, A_PIN, false);
            }
        }
    }
}

impl SampleApp {
    fn draw_list(&mut self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) {
        let th = cx.colors();
        let m = cx.metrics();
        let t = m.min_target();
        let compact = m.class == SizeClass::Compact;
        // input line: a field and an Add button
        let add_w = m.em(ADD_EM).min(area.w * 0.35);
        let (f_area, a_area) = Rect::new(area.x, area.y, area.w, t).split_left(area.w - add_w - m.gap(Space::Md));
        widgets::field(c, th, &m, f_area, "New note", &self.input, self.focused, A_FOCUS);
        widgets::button(c, th, &m, Rect::new(a_area.x + m.gap(Space::Md), a_area.y, (a_area.w - m.gap(Space::Md)).max(0.0), a_area.h), "Add", A_ADD, true);
        let bottom_reserved = if compact { t * 2.0 + 2.0 * m.gap(Space::Xl) } else { t + m.gap(Space::Md) };
        let list_top = area.y + t + m.gap(Space::Lg);
        let list = Rect::new(area.x, list_top, area.w, (area.bottom() - list_top - bottom_reserved).max(0.0));
        let row_h = t.max(m.size(TypeRole::Display) * 1.25);
        let rows = (list.h / (row_h + m.gap(Space::Xs))).floor().max(0.0) as usize;
        // keep the selected row on screen
        let first = if self.selected >= rows && rows > 0 { self.selected + 1 - rows } else { 0 };
        let mut shown = 0;
        c.clipped(list, &mut |cc| {
            for (i, it) in self.items.iter().enumerate().skip(first).take(rows) {
                let r = Rect::new(list.x, list.y + shown as f32 * (row_h + m.gap(Space::Xs)), list.w, row_h);
                let label = format!("{} {}", if it.done { "[x]" } else { "[ ]" }, it.text);
                widgets::row(cc, th, &m, r, &label, i == self.selected, A_ROW + i as u32);
                shown += 1;
            }
            if self.items.is_empty() {
                cc.text(list.x + m.gap(Space::Xs), list.y + m.gap(Space::Sm), m.size(TypeRole::Body), th.dim, "Nothing here. Type a note above.", TextStyle::Regular);
            }
        });
        // compact: the pin button owns the last line of the area, so "Clear done" sits on the line above it
        let foot = Rect::new(area.x, area.bottom() - t - if compact { t + m.gap(Space::Md) } else { 0.0 }, area.w, t);
        let (clear, _) = foot.split_left(m.em(WIDE_BUTTON_EM).min(foot.w));
        let n = self.items.iter().filter(|i| i.done).count();
        widgets::button(c, th, &m, clear, &format!("Clear done ({n})"), A_CLEAR, false);
        // wide: a button on the right of the foot switches the side between the assistant and the screen panel
        if self.chat.is_some() && !compact {
            let w = m.em(WIDE_BUTTON_EM).min(foot.w - clear.w - m.gap(Space::Md)).max(0.0);
            if w > m.min_target() {
                widgets::button(c, th, &m, Rect::new(foot.right() - w, foot.y, w, foot.h), if self.chat_wide { "Screen panel" } else { "Assistant" }, A_ASSIST, false);
            }
        }
    }

    fn draw_side(&mut self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) {
        let th = cx.colors();
        let m = cx.metrics();
        widgets::panel(c, th, &m, area);
        let inner = area.inset(m.gap(Space::Lg));
        let mut y = inner.y;
        let line = |c: &mut dyn Canvas, y: &mut f32, bold: bool, s: &str| {
            let size = m.size(if bold { TypeRole::Body } else { TypeRole::Small });
            let t = fit(c, size, s, if bold { TextStyle::Bold } else { TextStyle::Regular }, inner.w);
            c.text(inner.x, *y, size, if bold { th.text } else { th.muted }, &t, if bold { TextStyle::Bold } else { TextStyle::Regular });
            *y += m.leading(size);
        };
        line(c, &mut y, true, "This screen");
        let s = cx.screen();
        line(c, &mut y, false, &format!("Screen {} ({}x{} px, {:.1}x)", s.name, s.width, s.height, s.scale));
        line(c, &mut y, false, &format!("Look: {}", cx.look().name));
        line(c, &mut y, false, &format!("Dock: {}", cx.arrangement.dock_for(cx.look, cx.pane.screen).key()));
        y += m.gap(Space::Sm);
        line(c, &mut y, true, "Wallpaper");
        let used = widgets::wallpaper_picker(c, th, &m, Rect::new(inner.x, y, inner.w, (inner.bottom() - y - m.min_target() * 2.0 - m.gap(Space::Lg)).max(0.0)), cx.wallpaper_here(), A_WALL);
        y += used.h + m.gap(Space::Md);
        let pinned = cx.dock.pinned().iter().any(|a| a == ID);
        let b = widgets::button(c, th, &m, Rect::new(inner.x, y, inner.w, m.min_target()), if pinned { "Unpin from dock" } else { "Pin to dock" }, A_PIN, false);
        if !self.note.is_empty() {
            let sz = m.size(TypeRole::Caption);
            c.text(inner.x, b.bottom() + m.gap(Space::Sm), sz, th.muted, &fit_note(&self.note), TextStyle::Regular);
        }
    }
}

impl SampleApp {
    /// Let every view hear the heartbeat (a page that loads while the notes are shown still finishes) and take what they say.
    fn pump(&mut self) {
        for t in &mut self.tabs {
            t.view.event(&UiEvent::Tick);
        }
        self.drain();
    }

    /// What the views say: the address and the title follow the page, a refusal or failure is said under the page.
    fn drain(&mut self) {
        let editing = self.focus == Focus::Addr;
        let cur = self.cur;
        let mut said = None;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            for e in t.view.take_events() {
                match e {
                    BrowserEvent::Url(u) if !(editing && i == cur) => t.addr = u,
                    BrowserEvent::Title(x) => t.title = x,
                    BrowserEvent::Failed(Some(w)) | BrowserEvent::Closed(w) => said = Some(w),
                    BrowserEvent::Failed(None) => said = Some(String::new()),
                    _ => {}
                }
            }
        }
        if let Some(w) = said {
            self.note = w;
        }
    }

    fn go(&mut self) {
        let url = self.tabs[self.cur].addr.trim().to_string();
        if !url.is_empty() {
            self.note.clear();
            self.tabs[self.cur].view.load_url(&url);
        }
    }

    fn find_now(&mut self, forward: bool) {
        let q = self.find_q.clone();
        let v = &mut self.tabs[self.cur].view;
        v.find(&q, forward);
    }

    /// Events while the browser pane is shown. `None`: not mine, the common handling applies (the pane tabs).
    fn browser_event(&mut self, ev: &UiEvent) -> Option<Flow> {
        let cur = self.cur;
        let out = match ev {
            UiEvent::Action(a) => {
                if let Some(i) = self.tabs.iter().position(|t| t.view.owns_action(*a)) {
                    self.cur = i;
                    self.focus = Focus::Page;
                    self.tabs[i].view.set_focused(true);
                    self.tabs[i].view.event(ev);
                } else {
                    match *a {
                        A_B_BACK => self.tabs[cur].view.back(),
                        A_B_FWD => self.tabs[cur].view.forward(),
                        A_B_RELOAD => self.tabs[cur].view.reload(),
                        A_B_GO => self.go(),
                        A_B_ADDR => self.focus = Focus::Addr,
                        A_B_FIND => self.focus = Focus::Find,
                        A_B_NEXT => self.find_now(true),
                        a if (A_B_VIEW..A_B_VIEW + BROWSER_VIEWS as u32).contains(&a) => {
                            self.cur = (a - A_B_VIEW) as usize;
                            self.focus = Focus::None;
                        }
                        _ => return None,
                    }
                }
                Flow::Stay
            }
            UiEvent::Key(k) => match (self.focus, *k) {
                (Focus::Addr, UiKey::Char(c)) if !c.is_control() => {
                    self.tabs[cur].addr.push(c);
                    Flow::Stay
                }
                (Focus::Addr, UiKey::Backspace) => {
                    self.tabs[cur].addr.pop();
                    Flow::Stay
                }
                (Focus::Addr, UiKey::Enter) => {
                    self.go();
                    self.focus = Focus::Page;
                    Flow::Stay
                }
                (Focus::Find, UiKey::Char(c)) if !c.is_control() => {
                    self.find_q.push(c);
                    self.find_now(true);
                    Flow::Stay
                }
                (Focus::Find, UiKey::Backspace) => {
                    self.find_q.pop();
                    self.find_now(true);
                    Flow::Stay
                }
                (Focus::Find, UiKey::Enter) => {
                    self.find_now(true);
                    Flow::Stay
                }
                (Focus::Addr | Focus::Find, UiKey::Esc | UiKey::Tab) => {
                    self.focus = Focus::None;
                    Flow::Stay
                }
                (Focus::None, UiKey::Esc) => Flow::Back,
                (Focus::None, UiKey::Tab | UiKey::Char('/')) => {
                    self.focus = Focus::Addr;
                    Flow::Stay
                }
                (Focus::None | Focus::Page, UiKey::Char('f')) => {
                    self.focus = Focus::Find;
                    Flow::Stay
                }
                (Focus::Page, UiKey::Esc) => {
                    self.focus = Focus::None;
                    Flow::Stay
                }
                (_, _) => {
                    let v = &mut self.tabs[cur].view;
                    v.set_focused(true);
                    v.event(ev);
                    Flow::Stay
                }
            },
            UiEvent::Text(t) => {
                match self.focus {
                    Focus::Addr => self.tabs[cur].addr.extend(t.chars().filter(|c| !c.is_control())),
                    Focus::Find => {
                        self.find_q.extend(t.chars().filter(|c| !c.is_control()));
                        self.find_now(true);
                    }
                    _ => {}
                }
                Flow::Stay
            }
            UiEvent::Wheel(_) => {
                let v = &mut self.tabs[cur].view;
                v.set_focused(true);
                v.event(ev);
                Flow::Stay
            }
            UiEvent::Tick => {
                self.pump();
                return None;
            }
            _ => return None,
        };
        self.tabs[self.cur].view.set_focused(self.focus == Focus::Page);
        self.drain();
        Some(out)
    }

    fn draw_pane_tabs(&mut self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) {
        let th = cx.colors();
        let m = cx.metrics();
        let panes = self.panes();
        let gap = m.gap(Space::Sm);
        let w = ((area.w - gap * (panes.len() as f32 - 1.0)) / panes.len() as f32).min(m.em(9.0));
        for (i, (label, action, pane)) in panes.into_iter().enumerate() {
            let r = Rect::new(area.x + i as f32 * (w + gap), area.y, w, m.min_target());
            let on = self.pane == pane;
            widgets::surface(c, th, &m, r, if on { th.accent } else { th.button }, Radius::Md, false);
            let size = m.size(TypeRole::Label);
            let ink = if on { iyakki_ui_sdk::skins::label_on(th, th.accent) } else { th.text };
            // a narrow strip with many panes at a large text size: the label is shortened, never wider than its tab
            let shown = fit(c, size, label, TextStyle::Bold, (r.w - m.gap(Space::Sm)).max(0.0));
            let tw = c.measure(size, &shown, TextStyle::Bold);
            c.text(r.x + ((r.w - tw) / 2.0).max(0.0), r.y + ((r.h - size) / 2.0).max(0.0), size, ink, &shown, TextStyle::Bold);
            c.hit(r, Hit { action, name: label.to_string(), role: Role::Tab });
        }
    }

    /// The browser pane: the address row, the two views, find, and the page itself (an embedded `BrowserView`).
    fn draw_browser(&mut self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) {
        let th = cx.colors();
        let m = cx.metrics();
        let t = m.min_target();
        let cur = self.cur;
        let nav_w = m.em(3.9).min(area.w * 0.2);
        let (back_can, fwd_can) = (self.tabs[cur].view.can_back(), self.tabs[cur].view.can_forward());
        // row 1: back, forward, the address, go
        let r1 = Rect::new(area.x, area.y, area.w, t);
        let go_w = m.em(3.5).min(area.w * 0.18);
        let addr_w = (r1.w - 2.0 * nav_w - go_w - m.gap(Space::Lg) - m.gap(Space::Sm)).max(0.0);
        widgets::button(c, th, &m, Rect::new(r1.x, r1.y, nav_w, t), "Back", A_B_BACK, back_can);
        widgets::button(c, th, &m, Rect::new(r1.x + nav_w + m.gap(Space::Sm), r1.y, nav_w, t), "Fwd", A_B_FWD, fwd_can);
        let addr = self.tabs[cur].addr.clone();
        widgets::field(c, th, &m, Rect::new(r1.x + 2.0 * nav_w + 2.0 * m.gap(Space::Sm), r1.y, addr_w, t), ADDR_LABEL, &addr, self.focus == Focus::Addr, A_B_ADDR);
        widgets::button(c, th, &m, Rect::new(r1.right() - go_w, r1.y, go_w, t), "Go", A_B_GO, true);
        // row 2: the views, as tabs with the page title
        let r2y = r1.bottom() + m.gap(Space::Sm);
        let vw = ((area.w - m.gap(Space::Sm)) / BROWSER_VIEWS as f32).max(0.0);
        for i in 0..BROWSER_VIEWS {
            let r = Rect::new(area.x + i as f32 * (vw + m.gap(Space::Sm)), r2y, vw, t);
            let label = if self.tabs[i].title.is_empty() { format!("View {}", i + 1) } else { format!("View {}: {}", i + 1, self.tabs[i].title) };
            widgets::surface(c, th, &m, r, if i == cur { th.card_sel } else { th.card }, Radius::Md, false);
            let size = m.size(TypeRole::Body);
            let shown = fit(c, size, &label, TextStyle::Regular, (r.w - 2.0 * m.gap(Space::Md)).max(0.0));
            c.text(r.x + m.gap(Space::Md), r.y + ((r.h - size) / 2.0).max(0.0), size, th.text, &shown, TextStyle::Regular);
            c.hit(r, Hit { action: A_B_VIEW + i as u32, name: label, role: Role::Tab });
        }
        // row 3: reload, find, next match
        let r3y = r2y + t + m.gap(Space::Sm);
        let rl_w = m.em(4.5).min(area.w * 0.28);
        let nx_w = m.em(4.0).min(area.w * 0.22);
        widgets::button(c, th, &m, Rect::new(area.x, r3y, rl_w, t), "Reload", A_B_RELOAD, false);
        let fq = self.find_q.clone();
        let fw = (area.w - rl_w - nx_w - 2.0 * m.gap(Space::Sm)).max(0.0);
        widgets::field(c, th, &m, Rect::new(area.x + rl_w + m.gap(Space::Sm), r3y, fw, t), "Find in page", &fq, self.focus == Focus::Find, A_B_FIND);
        widgets::button(c, th, &m, Rect::new(area.right() - nx_w, r3y, nx_w, t), "Next", A_B_NEXT, false);
        // the status line under the page, then the page
        let status_h = m.size(TypeRole::Large);
        let top = r3y + t + m.gap(Space::Md);
        let page = Rect::new(area.x, top, area.w, (area.bottom() - top - status_h - m.gap(Space::Sm)).max(0.0));
        self.tabs[cur].view.set_focused(self.focus == Focus::Page);
        let used = self.tabs[cur].view.draw(c, cx, page);
        let f = self.tabs[cur].view.find_state();
        let mut line = String::new();
        if self.tabs[cur].view.is_loading() {
            line += "Loading... ";
        }
        if !f.query.is_empty() {
            line += &format!("{} of {} found. ", f.current, f.matches);
        }
        if !self.note.is_empty() {
            line += &self.note;
        } else if !self.tabs[cur].title.is_empty() {
            line += &self.tabs[cur].title;
        }
        let ss = m.size(TypeRole::Caption);
        let shown = fit(c, ss, &line, TextStyle::Regular, area.w);
        c.text(area.x, used.bottom() + m.gap(Space::Xs), ss, th.muted, &shown, TextStyle::Regular);
        self.drain();
    }
}

/// What the events pane mirrors and shows.
#[derive(Default)]
struct EventsState {
    network: String,
    power: String,
    session: String,
    setting: String,
    permissions: String,
    log: Vec<String>,
    refusals: Vec<String>,
}

impl EventsState {
    fn note(&mut self, seq: u64, late: bool, what: String) {
        self.log.push(format!("#{seq} {what}{}", if late { " (late)" } else { "" }));
        if self.log.len() > 20 {
            self.log.remove(0);
        }
    }
}

struct EventsPane {
    port: Box<dyn EventsPort>,
    actions: Box<dyn EventActions>,
    router: EventRouter,
    st: Rc<RefCell<EventsState>>,
}

impl EventsPane {
    fn pump(&mut self) {
        self.router.pump(self.port.as_mut(), self.actions.as_mut());
    }

    fn draw(&self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) {
        let th = cx.colors();
        let m = cx.metrics();
        let st = self.st.borrow();
        let body = m.size(TypeRole::Body);
        let mut y = area.y;
        let line = |c: &mut dyn Canvas, text: &str, col, y: &mut f32| {
            let t = fit(c, body, text, TextStyle::Regular, area.w);
            c.text(area.x, *y, body, col, &t, TextStyle::Regular);
            *y += m.leading(body);
        };
        line(c, &format!("Network: {}", if st.network.is_empty() { "not heard yet" } else { &st.network }), th.text, &mut y);
        line(c, &format!("Power: {}", if st.power.is_empty() { "not heard yet" } else { &st.power }), th.text, &mut y);
        line(c, &format!("Session: {}", if st.session.is_empty() { "not heard yet" } else { &st.session }), th.text, &mut y);
        line(c, &format!("Last permission change: {}", if st.permissions.is_empty() { "none heard yet" } else { &st.permissions }), th.text, &mut y);
        line(c, &format!("Last setting changed: {}", if st.setting.is_empty() { "none heard yet" } else { &st.setting }), th.text, &mut y);
        for r in &st.refusals {
            line(c, r, th.muted, &mut y);
        }
        y += m.gap(Space::Sm);
        line(c, "Heard (newest last)", th.muted, &mut y);
        let room = (((area.bottom() - y) / m.leading(body)).floor() as usize).max(1);
        for l in st.log.iter().skip(st.log.len().saturating_sub(room)) {
            line(c, l, th.text, &mut y);
        }
    }
}

fn fit_note(s: &str) -> String {
    s.chars().take(60).collect()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod browser_tests;
#[cfg(test)]
mod cameras_tests;
#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod assistant_tests;

impl SampleApp {
    fn event_main(&mut self, ev: &UiEvent, cx: &mut UiCx) -> Flow {
        if let Some(f) = self.extra_event(ev) {
            return f;
        }
        if self.pane == Pane::Browser && self.has_browser() {
            if let Some(f) = self.browser_event(ev) {
                return f;
            }
        } else if matches!(ev, UiEvent::Tick) {
            self.pump();
        }
        if !matches!(ev, UiEvent::Tick | UiEvent::Pointer(_)) {
            self.sync_context();
        }
        match ev {
            UiEvent::Action(A_TAB_NOTES) => self.pane = Pane::Notes,
            UiEvent::Action(A_TAB_BROWSER) if self.has_browser() => self.pane = Pane::Browser,
            UiEvent::Key(k) => return self.key(*k),
            UiEvent::Text(t) if self.chat.as_mut().is_some_and(|c| c.text(t)) => {}
            UiEvent::Text(t) if self.focused => {
                for c in t.chars().filter(|c| !c.is_control()) {
                    if self.input.chars().count() < MAX_LEN {
                        self.input.push(c);
                    }
                }
            }
            UiEvent::Wheel(d) if self.chat.is_some() && self.chat_compact && cx.size_class() == SizeClass::Compact => {
                if let Some(c) = self.chat.as_mut() {
                    c.wheel(*d);
                }
            }
            UiEvent::Tick => {
                if let Some(mut c) = self.chat.take() {
                    for inv in c.tick() {
                        let r = self.run_tool(&inv);
                        c.answer(&inv, r);
                    }
                    self.chat = Some(c);
                    self.sync_context();
                }
            }
            UiEvent::Action(a) if self.chat.as_ref().is_some_and(|c| c.owns(*a)) => {
                if let Some(c) = self.chat.as_mut() {
                    c.action(*a);
                }
            }
            UiEvent::Action(A_ASSIST) => {
                if cx.size_class() == SizeClass::Compact {
                    self.chat_compact = !self.chat_compact;
                } else {
                    self.chat_wide = !self.chat_wide;
                }
            }
            UiEvent::Wheel(d) => {
                let max = self.items.len().saturating_sub(1);
                self.selected = if *d > 0 { self.selected.saturating_sub(1) } else { (self.selected + 1).min(max) };
            }
            UiEvent::Action(A_FOCUS) => self.focused = true,
            UiEvent::Action(A_ADD) => self.add(),
            UiEvent::Action(A_CLEAR) => {
                self.items.retain(|i| !i.done);
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.save();
            }
            UiEvent::Action(A_PIN) => {
                let pinned = cx.dock.pinned().iter().any(|a| a == ID);
                let r = if pinned { cx.dock.unpin(ID) } else { cx.dock.pin(ID) };
                self.note = match (r, pinned) {
                    (Ok(()), false) => "Pinned to the dock".into(),
                    (Ok(()), true) => "Removed from the dock".into(),
                    (Err(_), _) => "The dock did not allow that".into(),
                };
            }
            UiEvent::Action(a) if (A_ROW..A_ROW + MAX_ITEMS as u32).contains(a) => self.toggle((*a - A_ROW) as usize),
            UiEvent::Action(a) if *a >= A_WALL && ((*a - A_WALL) as usize) < wp::ALL.len() => {
                let screen = cx.pane.screen;
                let _ = cx.wallpaper.set(screen, wp::ALL[(*a - A_WALL) as usize]);
            }
            _ => {}
        }
        Flow::Stay
    }

    /// What the cameras pane needs after every event: shown or hidden, the host's clock on a heartbeat, and the view closed when the
    /// person leaves the app (nothing is pulled from a camera that is not on screen).
    fn sync_cameras(&mut self, ev: &UiEvent, cx: &UiCx, leaving: bool) {
        let shown = self.pane == Pane::Cameras;
        let Some(v) = self.cams.as_mut() else { return };
        if leaving {
            v.close();
        }
        v.set_visible(shown);
        if shown && matches!(ev, UiEvent::Tick) {
            v.tick(cx.now_ms);
        }
    }
}
#[cfg(test)]
mod media_tests;
