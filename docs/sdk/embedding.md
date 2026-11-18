# Embedding recipes

Seven components can sit inside your own layout. All have the same shape: a **port** the host hands you, a **view** you keep in your state, a **base action number**, and four things you do: `tick` in `Tick`, route `Action`, `Key`, `Text` and `Wheel` to the view, and `draw` in a rectangle. Every example below is a file in `recipes/`, compiled and run by `crates/iyakki-ui-sdk/tests/docs_recipes.rs`.

How the person's data stays safe: the port is the core's; the core limits what the view can reach (the files view sees the person's home folder only, no links, no hidden names), and it asks before any change.

## Chat (the assistant, with your own tools)

You give the view the tools your app offers. `ask = true` makes the person confirm each call on a card; the core logs it. Run the tool when `tick()` returns an invocation and `answer` it. Use `set_context` to tell the assistant what the person is looking at. Test with `SharedAssistant` (a scripted port with the core's approval rules).

<!-- snippet: embed_chat.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::assistant::{AssistantPort, ContextItem, ContextKind, SessionConfig, ToolSpec};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::chatview::ChatView;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::toolargs::Args;

const CHAT: u32 = 200; // the view owns CHAT .. CHAT + chatview::ACTIONS

pub struct ChatApp {
    chat: ChatView,
    pub notes: Vec<String>,
}

impl ChatApp {
    pub fn new(port: Box<dyn AssistantPort>) -> ChatApp {
        let tools = vec![
            // true: the person must allow each call on a card (anything that changes data asks)
            ToolSpec::new("add_note", "Add one note.", r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#, true),
            ToolSpec::new("list_notes", "List the notes.", r#"{"type":"object"}"#, false),
        ];
        let cfg = SessionConfig { conversation: "notes".into(), title: "Notes".into(), tools, ..SessionConfig::default() };
        ChatApp { chat: ChatView::new(port, cfg, CHAT), notes: Vec::new() }
    }
}

impl UiApp for ChatApp {
    fn id(&self) -> &'static str { "chat-demo" }
    fn title(&self) -> String { "Chat demo".into() }
    fn wants_keyboard(&self) -> bool { self.chat.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => {
                for inv in self.chat.tick() {
                    let result = match inv.tool.as_str() {
                        "add_note" => Args::parse(&inv.args).ok().and_then(|a| a.str("text").map(String::from)).map(|t| { self.notes.push(t); "added".to_string() }).ok_or_else(|| "give a text".to_string()),
                        _ => Ok(self.notes.join("; ")),
                    };
                    self.chat.answer(&inv, result);
                }
                self.chat.set_context(vec![ContextItem::new(ContextKind::Document, "Notes", &self.notes.join("\n"))]);
            }
            UiEvent::Action(a) if self.chat.owns(*a) => { self.chat.action(*a); }
            UiEvent::Text(t) => { self.chat.text(t); }
            UiEvent::Key(k) => { if !self.chat.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            UiEvent::Wheel(n) => self.chat.wheel(*n),
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.chat.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## Browser

The core opens a session for the view and checks every address. The app draws its own chrome (address field, back, reload) around the page; the view draws the page. When the person has embedded views switched off, the view says so in words.

<!-- snippet: embed_browser.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::browser::{BrowserPort, BrowserView};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const WEB: u32 = 10_000; // a BrowserView takes WEB .. WEB + browser::ACTION_SPAN

pub struct WebApp {
    web: BrowserView,
    started: bool,
    start_url: String,
}

impl WebApp {
    pub fn new(port: Box<dyn BrowserPort>, url: &str) -> WebApp {
        WebApp { web: BrowserView::new(port, WEB), started: false, start_url: url.to_string() }
    }
}

impl UiApp for WebApp {
    fn id(&self) -> &'static str { "web-demo" }
    fn title(&self) -> String { "Web demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let UiEvent::Tick = ev {
            if !self.started {
                self.started = true;
                self.web.load_url(&self.start_url); // the core checks the address and the person's rules
            }
            self.web.refresh();
        }
        if !self.web.event(ev) {
            if let UiEvent::Key(UiKey::Esc) = ev { return Flow::Back; }
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let g = cx.metrics().gutter();
        self.web.draw(c, cx, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## Files

The person's home folder, listed and read as text. The view hands you the file the person chose once through `take_opened()`. Changing files (a new folder, rename, delete) goes through the core's approvals.

<!-- snippet: embed_files.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::files::FilesPort;
use iyakki_ui_sdk::filesview::{self, FilesView};
use iyakki_ui_sdk::geom::Rect;

const FILES: u32 = 100; // the view owns the numbers FILES .. FILES + filesview::ACTIONS

pub struct FilesApp {
    files: FilesView,
}

impl FilesApp {
    /// `port` is the core's file port on a device, `MemFiles` in a test or the preview.
    pub fn new(port: Box<dyn FilesPort>) -> FilesApp {
        FilesApp { files: FilesView::new(port, FILES) }
    }
}

impl FilesApp {
    /// Close the text the person opened (for example when an assistant tool asks for it).
    pub fn close_preview(&mut self) {
        self.files.close();
    }
}

impl UiApp for FilesApp {
    fn id(&self) -> &'static str { "files-demo" }
    fn title(&self) -> String { "Files demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.files.tick(),
            UiEvent::Action(a) if self.files.owns(*a) => { self.files.action(*a); }
            UiEvent::Wheel(n) => self.files.wheel(*n),
            UiEvent::Key(k) => {
                if !self.files.key(*k) && *k == UiKey::Esc { return Flow::Back; }
            }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.files.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
        let _ = filesview::ACTIONS;
    }
}
```
<!-- /snippet -->

## Terminal

The IYAKKI command line, under the person's own policy. A command that changes something shows a question and waits for the person.

<!-- snippet: embed_terminal.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::terminal::TerminalPort;
use iyakki_ui_sdk::terminalview::TerminalView;

const TERM: u32 = 300; // TerminalView uses TERM .. TERM + terminalview::ACTIONS

pub struct TerminalApp {
    term: TerminalView,
}

impl TerminalApp {
    pub fn new(port: Box<dyn TerminalPort>) -> TerminalApp {
        TerminalApp { term: TerminalView::new(port, TERM) }
    }
}

impl UiApp for TerminalApp {
    fn id(&self) -> &'static str { "terminal-demo" }
    fn title(&self) -> String { "Terminal demo".into() }
    fn wants_keyboard(&self) -> bool { self.term.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.term.tick(),
            UiEvent::Action(a) if self.term.owns(*a) => { self.term.action(*a); }
            UiEvent::Text(t) => { self.term.text(t); }
            UiEvent::Wheel(n) => self.term.wheel(*n),
            UiEvent::Key(k) => { if !self.term.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.term.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## Editor

A text editor with undo, highlighting and a review of the assistant's edit proposals. It has no port: it edits text you give it. Saving is a request the app answers (`take_save`); where the text goes is the app's decision, through a port.

<!-- snippet: embed_editor.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::editorview::EditorView;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const EDIT: u32 = 400; // EditorView uses EDIT .. EDIT + editorview::ACTIONS

pub struct EditorApp {
    editor: EditorView,
    /// What the person saved (the app decides where it goes: through a port, never a file).
    pub saved: Option<(String, String)>,
}

impl EditorApp {
    pub fn new(path: &str, text: &str) -> EditorApp {
        let mut editor = EditorView::new(EDIT);
        let _ = editor.load(path, text); // Err: too big or binary, the view shows the sentence
        EditorApp { editor, saved: None }
    }
}

impl UiApp for EditorApp {
    fn id(&self) -> &'static str { "editor-demo" }
    fn title(&self) -> String { "Editor demo".into() }
    fn wants_keyboard(&self) -> bool { self.editor.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(a) if self.editor.owns(*a) => { self.editor.action(*a); }
            UiEvent::Text(t) => { self.editor.text_in(t); }
            UiEvent::Wheel(n) => self.editor.wheel(*n),
            UiEvent::Key(k) => { if !self.editor.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        if let Some(s) = self.editor.take_save() {
            self.saved = Some(s);
            self.editor.mark_saved();
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.editor.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## Audio and media

Sound files of the person's home folder and stream addresses (the person is asked first). `MemMedia` is the fake: `put_file`, `advance(ms)` moves its clock, `calls()` lists what the view asked. Video is `MediaView` over the same port (see reference/media.md; frames reach the screen as images through the core).

<!-- snippet: embed_audio.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::audioview::AudioView;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::media::{MediaPort, Source};

const AUDIO: u32 = 500; // AudioView uses AUDIO .. AUDIO + audioview::ACTIONS

pub struct PlayerApp {
    audio: AudioView,
}

impl PlayerApp {
    pub fn new(port: Box<dyn MediaPort>) -> PlayerApp {
        PlayerApp { audio: AudioView::new(port, AUDIO) }
    }
    /// Play a file of the person's home folder (the name the port listed). A stream address asks the person first.
    pub fn play(&mut self, name: &str) {
        self.audio.open(Source::Home(name.to_string()), name);
    }
}

impl UiApp for PlayerApp {
    fn id(&self) -> &'static str { "player-demo" }
    fn title(&self) -> String { "Player demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.audio.tick(),
            UiEvent::Action(a) if self.audio.owns(*a) => { self.audio.action(*a); }
            UiEvent::Wheel(n) => self.audio.wheel(*n),
            UiEvent::Key(k) => { if !self.audio.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.audio.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## Camera

Live pictures from the cameras the person may watch. Nothing is pulled before the person allows it on the consent card; a snapshot asks every time; the owner's motion zones are drawn read only. Pictures are never queued (the view asks for the newest). Test with `MemCameras` (`add`, `push_solid`, `pulled`, `snapshots`, `audit`).

<!-- snippet: embed_camera.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::cameras::CameraPort;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cameraview::{CameraView, ViewConfig};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const CAM: u32 = 600; // the view uses CAM .. CAM + cameraview::ACTIONS

pub struct DoorbellApp {
    cams: CameraView,
    now_ms: u64,
}

impl DoorbellApp {
    pub fn new(port: Box<dyn CameraPort>) -> DoorbellApp {
        // the person sees a consent card (Allow, Deny) before any picture is pulled
        let cfg = ViewConfig { label: "Doorbell panel".into(), ..ViewConfig::default() };
        DoorbellApp { cams: CameraView::new(port, cfg, CAM), now_ms: 0 }
    }
}

impl UiApp for DoorbellApp {
    fn id(&self) -> &'static str { "doorbell-demo" }
    fn title(&self) -> String { "Doorbell demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => { self.now_ms += 33; self.cams.tick(self.now_ms); } // about 30 ticks a second
            UiEvent::Action(a) if self.cams.owns(*a) => { self.cams.action(*a); }
            UiEvent::Key(k) => { if !self.cams.key(*k) && *k == UiKey::Esc { self.cams.close(); return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.cams.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
```
<!-- /snippet -->

## The shell side

On a device the ports are made by an adapter in `crates/iyakki-apps/src/apps/` (`CoreFilesPort`, `sdk_browser.rs`, `sdk_media.rs`): the SDK port over `cx.core.*()`, bound to the person and the app. Your app does not write that adapter unless you add a new component; see `.claude/skills/iyakki-sdk-component`.
