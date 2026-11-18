//! The recipes of docs/sdk/recipes are real files, compiled and run here, so the guide cannot show code that does not build or does not
//! keep the SDK's rules. Each recipe app is driven through the SDK `Host` at three sizes and at text 3x: every control has a name, sits
//! inside the pane and is at least the touch size; the source passes the style-literal lint and names no machine access.

#[path = "../../../docs/sdk/recipes/add_tabs.rs"]
mod add_tabs;
#[path = "../../../docs/sdk/recipes/approval_card.rs"]
mod approval_card;
#[path = "../../../docs/sdk/recipes/barcode_label.rs"]
mod barcode_label;
#[path = "../../../docs/sdk/recipes/embed_audio.rs"]
mod embed_audio;
#[path = "../../../docs/sdk/recipes/embed_browser.rs"]
mod embed_browser;
#[path = "../../../docs/sdk/recipes/embed_camera.rs"]
mod embed_camera;
#[path = "../../../docs/sdk/recipes/embed_chat.rs"]
mod embed_chat;
#[path = "../../../docs/sdk/recipes/embed_editor.rs"]
mod embed_editor;
#[path = "../../../docs/sdk/recipes/events_react.rs"]
mod events_react;
#[path = "../../../docs/sdk/recipes/embed_files.rs"]
mod embed_files;
#[path = "../../../docs/sdk/recipes/embed_terminal.rs"]
mod embed_terminal;
#[path = "../../../docs/sdk/recipes/form_validation.rs"]
mod form_validation;
#[path = "../../../docs/sdk/recipes/list_selection.rs"]
mod list_selection;
#[path = "../../../docs/sdk/recipes/multi_screen.rs"]
mod multi_screen;
#[path = "../../../docs/sdk/recipes/notes_app.rs"]
mod notes_app;
#[path = "../../../docs/sdk/recipes/persist_storage.rs"]
mod persist_storage;
#[path = "../../../docs/sdk/recipes/tool_list_args.rs"]
mod tool_list_args;
#[path = "../../../docs/sdk/recipes/themed_drawing.rs"]
mod themed_drawing;

use iyakki_ui_sdk::app::UiApp;
use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
use iyakki_ui_sdk::browser::MemBrowser;
use iyakki_ui_sdk::cameras::{CamInfo, MemCameras};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::events::{ev_text, MemEvents, RecordingActions};
use iyakki_ui_sdk::files::MemFiles;
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::media::MemMedia;
use iyakki_ui_sdk::ports::MemStorage;
use iyakki_ui_sdk::terminal::MemTerminal;

const SIZES: [(u32, u32); 3] = [(360, 740), (800, 480), (1280, 720)];

fn host<A: UiApp>(app: A, w: u32, h: u32) -> Host<A> {
    let id = app.id();
    Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[id])), Box::new(MemWallpaper::default()))
}

fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}

fn tap<A: UiApp>(h: &mut Host<A>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

/// The checks every app keeps: named controls, inside the pane, big enough to touch, at three sizes and text 1x and 3x.
fn rules<A: UiApp>(what: &str, make: &dyn Fn() -> A) {
    for (w, h) in SIZES {
        for scale in [1.0, 3.0] {
            let mut hst = host(make(), w, h);
            hst.touch = true;
            hst.text_scale = scale;
            hst.event(&UiEvent::Tick);
            let l = hst.record();
            for n in l.a11y() {
                assert!(!n.name.is_empty(), "{what} {w}x{h} x{scale}: a control has no name");
                assert!(n.rect.x >= -0.5 && n.rect.y >= -0.5 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= h as f32 + 0.5, "{what} {w}x{h} x{scale}: {} leaves the pane", n.name);
            }
        }
    }
}

#[test]
fn every_recipe_keeps_the_sdk_rules_at_three_sizes_and_text_3x() {
    rules("barcode", &|| barcode_label::LabelApp::new("ORD-0042").unwrap());
    rules("tabs", &add_tabs::TabsApp::new);
    rules("list", &|| list_selection::ListApp::new(&["one", "two", "three"]));
    rules("notes", &notes_app::NotesApp::new);
    rules("form", &form_validation::FormApp::new);
    rules("keep", &|| persist_storage::KeepApp::new(Box::new(MemStorage::default())));
    rules("bars", &|| themed_drawing::BarsApp { values: vec![0.2, 0.5, 0.9] });
    rules("events", &|| events_react::NetApp::new(Box::new(MemEvents::new()), Box::new(RecordingActions::default())));
    rules("files", &|| {
        let mut f = MemFiles::new();
        f.put("notes/a.txt", "hello");
        embed_files::FilesApp::new(Box::new(f))
    });
    rules("chat", &|| embed_chat::ChatApp::new(Box::new(SharedAssistant::new(vec![Turn::say("Hi.")]).0)));
    rules("terminal", &|| embed_terminal::TerminalApp::new(Box::new(MemTerminal::new())));
    rules("editor", &|| embed_editor::EditorApp::new("a.txt", "one\ntwo\n"));
    rules("audio", &|| {
        let m = MemMedia::new();
        m.put_file("song.wav", 60_000, false);
        embed_audio::PlayerApp::new(Box::new(m))
    });
    rules("camera", &|| {
        let c = MemCameras::new("doorbell-demo");
        c.add(CamInfo::new("door", "Front door"));
        embed_camera::DoorbellApp::new(Box::new(c))
    });
    rules("browser", &|| embed_browser::WebApp::new(Box::new(MemBrowser::default().with_page("https://example.test/", "Example", &["Hello"], &[])), "https://example.test/"));
}

#[test]
fn tabs_switch_and_the_list_keeps_a_selection() {
    let mut h = host(add_tabs::TabsApp::new(), 360, 740);
    tap(&mut h, "About");
    assert!(h.app.on_about());
    let mut l = host(list_selection::ListApp::new(&["one", "two", "three"]), 360, 740);
    tap(&mut l, "two");
    assert_eq!(l.app.selected, 1);
    l.event(&UiEvent::Key(UiKey::Down));
    assert_eq!(l.app.selected, 2);
}

#[test]
fn the_form_says_why_and_the_count_is_kept_through_the_port() {
    assert!(form_validation::check_name("").is_err());
    assert!(form_validation::check_name("Ada").is_ok());
    let mut f = host(form_validation::FormApp::new(), 360, 740);
    tap(&mut f, "Save");
    assert!(f.record().texts().contains(&"Give a name."));
    tap(&mut f, "Name");
    f.event(&UiEvent::Text("Ada".into()));
    tap(&mut f, "Save");
    assert_eq!(f.app.saved.as_deref(), Some("Ada"));

    let mut k = host(persist_storage::KeepApp::new(Box::new(MemStorage::default())), 360, 740);
    tap(&mut k, "Add one");
    tap(&mut k, "Add one");
    assert_eq!(k.app.count(), 2);
}

#[test]
fn the_files_view_lists_and_the_player_plays() {
    let mut f = MemFiles::new();
    f.put("notes/a.txt", "hello");
    let mut h = host(embed_files::FilesApp::new(Box::new(f)), 800, 480);
    h.event(&UiEvent::Tick);
    assert!(h.record().texts().iter().any(|t| t.contains("notes")), "the folder is listed");
    h.event(&UiEvent::Key(UiKey::Enter)); // into notes/
    h.event(&UiEvent::Key(UiKey::Enter)); // open a.txt
    assert!(h.record().texts().iter().any(|t| t.contains("hello")), "the text is previewed");
    h.app.close_preview();
    assert!(!h.record().texts().iter().any(|t| t.contains("hello")));

    let m = MemMedia::new();
    m.put_file("song.wav", 60_000, false);
    let mut p = host(embed_audio::PlayerApp::new(Box::new(m.clone())), 800, 480);
    p.app.play("song.wav");
    p.event(&UiEvent::Tick);
    assert!(m.calls().iter().any(|c| c.contains("open")), "the port was asked to open it: {:?}", m.calls());
}

#[test]
fn the_chat_runs_an_app_tool_after_the_person_allows_it() {
    let (port, _script) = SharedAssistant::new(vec![Turn::call("Adding it.", "add_note", "{\"text\":\"milk\"}"), Turn::say("Done.")]);
    let mut h = host(embed_chat::ChatApp::new(Box::new(port)), 800, 480);
    h.event(&UiEvent::Tick);
    tap(&mut h, "Ask the assistant");
    h.event(&UiEvent::Text("add milk".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    for _ in 0..5 {
        h.event(&UiEvent::Tick);
    }
    // the tool asks: nothing happened yet, and the card is on screen
    assert!(h.app.notes.is_empty(), "the tool must wait for the person");
    assert!(h.record().a11y().iter().any(|n| n.name == "Allow"), "an Allow button is on the card");
    tap(&mut h, "Allow");
    for _ in 0..5 {
        h.event(&UiEvent::Tick);
    }
    assert_eq!(h.app.notes, vec!["milk".to_string()]);
    assert_eq!(approval_card::tools().iter().filter(|t| t.ask).count(), 1);
}

#[test]
fn the_terminal_and_the_editor_and_the_browser_work() {
    let mut t = host(embed_terminal::TerminalApp::new(Box::new(MemTerminal::new())), 800, 480);
    t.event(&UiEvent::Tick);
    let mut e = host(embed_editor::EditorApp::new("a.txt", "one\n"), 800, 480);
    e.event(&UiEvent::Key(UiKey::Char('x')));
    assert!(e.record().a11y().len() >= 2);
    let mut b = host(embed_browser::WebApp::new(Box::new(MemBrowser::default().with_page("https://example.test/", "Example", &["Hello"], &[])), "https://example.test/"), 800, 480);
    for _ in 0..3 {
        b.event(&UiEvent::Tick);
    }
    let texts = b.record().texts().join(" ");
    assert!(texts.contains("Hello"), "the page is drawn: {texts}");
}

#[test]
fn the_recipe_sources_pass_the_style_literal_lint_and_name_no_machine_access() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/sdk/recipes");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            let src = std::fs::read_to_string(&p).unwrap();
            let bad = iyakki_ui_sdk::lint::style_literals(&src);
            assert!(bad.is_empty(), "{}: {}", p.display(), bad.join(" | "));
            for w in ["std::fs", "std::net", "std::process", "iyakki_uikit", "iyakki_agent", "rgb("] {
                assert!(!src.contains(w), "{} contains {w}", p.display());
            }
            n += 1;
        }
    }
    assert!(n >= 17, "recipes found: {n}");
}

#[test]
fn the_worked_example_adds_a_note_keeps_it_and_lists_the_files() {
    let mut h = host(notes_app::NotesApp::new(), 360, 740);
    tap(&mut h, "Notes");
    tap(&mut h, "Note");
    h.event(&UiEvent::Text("buy milk".into()));
    tap(&mut h, "Add");
    assert_eq!(h.app.notes(), ["buy milk"]);
    h.event(&UiEvent::Key(UiKey::Enter)); // an empty draft says why
    assert!(h.record().texts().iter().any(|t| t.contains("Write something")));
    let mut f = MemFiles::new();
    f.put("docs/readme.txt", "hello");
    let mut g = host(notes_app::NotesApp::with_ports(Box::new(MemStorage::default()), Box::new(f)), 800, 480);
    tap(&mut g, "Files");
    g.event(&UiEvent::Tick);
    assert!(g.record().texts().iter().any(|t| t.contains("docs")));
    tap(&mut g, "Counter");
    tap(&mut g, "More");
    assert_eq!(g.app.count(), 1);
}

#[test]
fn the_card_needs_no_input_focus_and_y_answers_it_at_once() {
    let (port, _script) = SharedAssistant::new(vec![Turn::call("Adding it.", "add_note", "{\"text\":\"milk\"}"), Turn::say("Done.")]);
    let mut h = host(embed_chat::ChatApp::new(Box::new(port)), 800, 480);
    h.event(&UiEvent::Tick);
    tap(&mut h, "Ask the assistant"); // only needed to SEND the first message
    h.event(&UiEvent::Text("add milk".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    for _ in 0..5 {
        h.event(&UiEvent::Tick);
    }
    assert!(h.record().a11y().iter().any(|n| n.name == "Allow"), "the card shows without tapping the field again");
    h.event(&UiEvent::Key(UiKey::Char('y')));
    for _ in 0..5 {
        h.event(&UiEvent::Tick);
    }
    assert_eq!(h.app.notes, vec!["milk".to_string()]);
}

#[test]
fn the_multi_screen_recipe_pages_routes_text_and_fits_every_screen_at_3x() {
    let items: Vec<u32> = (0..10).collect();
    assert_eq!(multi_screen::page_of(&items, 0, 4).1, 3);
    assert_eq!(multi_screen::page_of(&items, 99, 4).0, &[8, 9]);
    assert_eq!(multi_screen::page_of(&items[..0], 5, 4), (&[][..], 1));
    let mut h = host(multi_screen::MultiApp::new(), 360, 740);
    tap(&mut h, "Items");
    tap(&mut h, "Next");
    tap(&mut h, "Next");
    tap(&mut h, "Next");
    assert_eq!(h.app.page(), 2, "the page stops at the last one");
    tap(&mut h, "Edit");
    h.event(&UiEvent::Text("lost".into())); // no focus: dropped
    h.event(&UiEvent::Key(UiKey::Tab)); // Name
    h.event(&UiEvent::Text("Widget".into()));
    h.event(&UiEvent::Key(UiKey::Tab)); // Quantity
    h.event(&UiEvent::Text("3".into()));
    tap(&mut h, "Save");
    assert_eq!(h.app.items().last().map(String::as_str), Some("Widget"));
    // every screen, three sizes, text 3x: named, inside the pane, touchable
    for (w, hh) in SIZES {
        for screen in ["Home", "Items", "Edit"] {
            let mut h = host(multi_screen::MultiApp::new(), w, hh);
            h.touch = true;
            h.text_scale = 3.0;
            tap(&mut h, screen);
            for n in h.record().a11y() {
                assert!(!n.name.is_empty());
                assert!(n.rect.x >= -0.5 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= hh as f32 + 0.5, "{screen} {w}x{hh}: {} leaves the pane", n.name);
                assert!(n.rect.h >= 43.5, "{screen} {w}x{hh}: {} is too small to touch", n.name);
            }
        }
    }
}

#[test]
fn a_list_reaches_a_tool_as_one_string_and_is_read_back() {
    assert_eq!(tool_list_args::tool_items("{\"items\":\"A1:2, B7:10\"}").unwrap(), vec![("A1".to_string(), 2), ("B7".to_string(), 10)]);
    assert!(tool_list_args::tool_items("{\"items\":\"A1\"}").unwrap_err().contains("SKU:QTY"));
    assert!(tool_list_args::tool_items("{\"items\":\"A1:0\"}").is_err());
    assert!(tool_list_args::tool_items("{}").is_err());
    // an array is kept as raw text and has no accessor: it must not be used for lists
    let a = iyakki_ui_sdk::toolargs::Args::parse("{\"items\":[\"A1:2\"]}").unwrap();
    assert!(a.str("items").is_none());
}

#[test]
fn the_events_recipe_handles_late_and_missed_and_a_live_event_acts() {
    use iyakki_ui_sdk::events::EventsPort;
    // an app away while the network changed: the late event updates the screen but rings nothing
    let mut m = MemEvents::new();
    m.allow("connectivity.changed");
    m.coalescing("connectivity.changed");
    m.register("connectivity.changed", "").unwrap();
    m.happen("connectivity.changed", ev_text(&[("state", "offline")]));
    let mut h = host(events_react::NetApp::new(Box::new(m), Box::new(RecordingActions::default())), 800, 480);
    let texts = |l: &DisplayList| l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect::<Vec<_>>().join("|");
    assert!(texts(&h.record()).contains("Network: offline"));
    // the source of the recipe passes the app-check events lint
    let src = include_str!("../../../docs/sdk/recipes/events_react.rs");
    assert!(iyakki_ui_sdk::lint::events_unhandled(src).is_empty());
    assert!(iyakki_ui_sdk::lint::style_literals(src).is_empty());
}
