# Worked example: counter, notes and a file browser

The skill `iyakki-app-development` was followed to build this app from the scaffold, in a folder outside the repository. The finished app is `recipes/notes_app.rs` (tested by `crates/iyakki-ui-sdk/tests/docs_recipes.rs`, `the_worked_example_adds_a_note_keeps_it_and_lists_the_files`).

<!-- snippet: notes_app.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::desktop::DockStatus;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::files::{FilesPort, MemFiles};
use iyakki_ui_sdk::filesview::FilesView;
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

use iyakki_ui_sdk::ports::{MemStorage, PortError, StoragePort};

/// A worked example: a counter, a notes pane with persistence and an embedded file browser, three tabs.
pub const ID: &str = "notes-demo";
const A_TAB_COUNT: u32 = 1;
const A_TAB_NOTES: u32 = 2;
const A_TAB_FILES: u32 = 3;
const A_MORE: u32 = 4;
const A_FIELD: u32 = 5;
const A_ADD: u32 = 6;
const A_ROW0: u32 = 20;
const FILES: u32 = 100;
const COUNT_KEY: &str = "count";
const NOTES_KEY: &str = "notes";
const MAX_NOTES: usize = 100;
const TABS: f32 = 3.0; // a count is named in a const: the lint lets const lines through

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Counter,
    Notes,
    Files,
}

pub struct NotesApp {
    pane: Pane,
    count: u32,
    notes: Vec<String>,
    draft: String,
    editing: bool,
    said: String,
    storage: Box<dyn StoragePort>,
    files: FilesView,
}

impl Default for NotesApp {
    fn default() -> Self {
        NotesApp::new()
    }
}

impl NotesApp {
    pub fn new() -> NotesApp {
        NotesApp::with_ports(Box::new(MemStorage::default()), Box::new(MemFiles::new()))
    }

    /// The app over the storage the host gives it.
    pub fn with_storage(storage: Box<dyn StoragePort>) -> NotesApp {
        NotesApp::with_ports(storage, Box::new(MemFiles::new()))
    }

    pub fn with_ports(mut storage: Box<dyn StoragePort>, files: Box<dyn FilesPort>) -> NotesApp {
        let count = storage.get(COUNT_KEY).and_then(|s| s.parse().ok()).unwrap_or(0);
        let notes = storage.get(NOTES_KEY).map(|s| s.split('\n').filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default();
        NotesApp { pane: Pane::Counter, count, notes, draft: String::new(), editing: false, said: String::new(), storage, files: FilesView::new(files, FILES) }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    fn save(&mut self) {
        let r = self.storage.set(COUNT_KEY, &self.count.to_string()).and_then(|_| self.storage.set(NOTES_KEY, &self.notes.join("\n")));
        self.said = match r {
            Ok(()) => String::new(),
            Err(PortError::TooBig) => "That is too much to keep.".into(),
            Err(_) => "The core did not allow saving.".into(),
        };
    }

    pub fn add_note(&mut self) {
        let t = self.draft.trim().to_string();
        if t.is_empty() {
            self.said = "Write something first.".into();
        } else if self.notes.len() >= MAX_NOTES {
            self.said = "That is enough notes: delete one first.".into();
        } else {
            self.notes.push(t);
            self.draft.clear();
            self.save();
        }
    }
}

impl UiApp for NotesApp {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "Notes demo".into()
    }
    fn dock_status(&self) -> Option<DockStatus> {
        Some(DockStatus::new(if self.notes.is_empty() { None } else { Some(self.notes.len() as u32) }, &format!("{} notes", self.notes.len())))
    }
    fn wants_keyboard(&self) -> bool {
        self.editing
    }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.files.tick(),
            UiEvent::Action(A_TAB_COUNT) => self.pane = Pane::Counter,
            UiEvent::Action(A_TAB_NOTES) => self.pane = Pane::Notes,
            UiEvent::Action(A_TAB_FILES) => self.pane = Pane::Files,
            UiEvent::Action(A_MORE) => {
                self.count = self.count.saturating_add(1);
                self.save();
            }
            UiEvent::Action(A_FIELD) => self.editing = true,
            UiEvent::Action(A_ADD) => self.add_note(),
            UiEvent::Action(a) if self.files.owns(*a) => {
                self.files.action(*a);
            }
            UiEvent::Wheel(n) if self.pane == Pane::Files => self.files.wheel(*n),
            UiEvent::Text(t) if self.editing && self.pane == Pane::Notes => self.draft.push_str(t),
            UiEvent::Key(k) => {
                if self.pane == Pane::Files && self.files.key(*k) {
                    return Flow::Stay;
                }
                match k {
                    UiKey::Backspace if self.editing => {
                        self.draft.pop();
                    }
                    UiKey::Enter if self.editing => self.add_note(),
                    UiKey::Esc if self.editing => self.editing = false,
                    UiKey::Esc => return Flow::Back,
                    _ => {}
                }
            }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let tab_w = (w - g - g - m.gap(Space::Md) - m.gap(Space::Md)) / TABS;
        let t = m.min_target();
        for (i, (label, a, p)) in [("Counter", A_TAB_COUNT, Pane::Counter), ("Notes", A_TAB_NOTES, Pane::Notes), ("Files", A_TAB_FILES, Pane::Files)].into_iter().enumerate() {
            widgets::button(c, th, &m, Rect::new(g + i as f32 * (tab_w + m.gap(Space::Md)), g, tab_w, t), label, a, self.pane == p);
        }
        let top = g + t + m.gap(Space::Lg);
        let area = Rect::new(g, top, w - 2.0 * g, (h - top - g).max(0.0));
        let body = m.size(TypeRole::Body);
        match self.pane {
            Pane::Counter => {
                let line = fit(c, m.size(TypeRole::Large), &format!("Pressed {} times", self.count), TextStyle::Bold, area.w);
                c.text(area.x, area.y, m.size(TypeRole::Large), th.text, &line, TextStyle::Bold);
                widgets::button(c, th, &m, Rect::new(area.x, area.y + 2.0 * m.size(TypeRole::Large), area.w, t), "More", A_MORE, true);
            }
            Pane::Notes => {
                widgets::field(c, th, &m, Rect::new(area.x, area.y, area.w - 2.0 * t, t), "Note", &self.draft, self.editing, A_FIELD);
                widgets::button(c, th, &m, Rect::new(area.x + area.w - 2.0 * t, area.y, 2.0 * t, t), "Add", A_ADD, true);
                let mut y = area.y + t + m.gap(Space::Md);
                for (i, n) in self.notes.iter().enumerate() {
                    if y + t > area.y + area.h - body * 2.0 {
                        break;
                    }
                    widgets::row(c, th, &m, Rect::new(area.x, y, area.w, t), n, false, A_ROW0 + i as u32);
                    y += t + m.gap(Space::Xs);
                }
                if !self.said.is_empty() {
                    let s = fit(c, body, &self.said, TextStyle::Regular, area.w);
                    c.text(area.x, area.y + area.h - body * 1.5, body, th.muted, &s, TextStyle::Regular);
                }
            }
            Pane::Files => {
                self.files.draw(c, th, &m, area);
            }
        }
    }
}
```
<!-- /snippet -->

## What the dry run found (2026-11-16) and what was changed

| Friction | Fix |
|---|---|
| The golden files could only be written with `IYAKKI_GOLDEN_WRITE=1`, which a remote build wrapper does not pass on | the first `cargo test` writes missing golden files itself (the template and its test were changed) |
| The scaffold's drawing code used literals (`m.font(22.0)`, `8.0`) and would fail its own lint | the template uses `m.size(TypeRole::..)` and `m.gap(Space::..)`; the generated tests include the lint |
| No way to see a new app in a browser tab (the preview binary served SAMPLE only) | the scaffold has `examples/preview.rs` and a README with the commands |
| `/ 3.0` for three tabs failed the styling-number lint | documented: name a count in a `const` (`const TABS: f32 = 3.0;`); `app-check` says so |
| The generated `tests/golden.rs` is written for the starter counter (Enter and Backspace, `with_storage`) and fails when the app changes | a comment in the test says to rewrite it; the skill says so |
| Failure messages said what, not where or how to fix | the generated tests print WHAT and FIX with before and after; `app-check` prints a numbered list with file and line |
| `--sdk` writes an absolute path into the new `Cargo.toml` | left as is (it is the developer's own project); noted in troubleshooting |

## Timing

On the build host used for this work (16 cores, warm cache): `app-check` on the finished app, including `cargo test` of three test files and the preview example, took about two seconds when only the app changed; the first build of a fresh scaffold about eleven seconds. The static part of `app-check` takes milliseconds. A cold build of the SDK and its toolkit on a clean machine takes longer (minutes); it is paid once.
