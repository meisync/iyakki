//! The hand-over from the browser to other tiles at shell level (F-BROX-01): a saved file chosen in the Downloads list opens the right
//! tile for real (the shell swaps the screen, the tile receives the item), the browser is kept aside so Back returns to it, and the
//! screen reader says the title of a dialog a tile opens (F-A11Y-02). Driven with keys on the real shell; nothing is heard.

use super::tests::*;
use super::*;
use crate::access::SpeechSink;
use crate::apps::browser::BrowserApp;
use std::sync::Mutex;

fn signed_in(tag: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

/// Put the browser on screen showing a Downloads list with these files.
fn browser_with(sh: &mut Shell, files: &[(&str, &str)]) {
    let cx = sh.cx_for_test().expect("signed in");
    let app = BrowserApp::with_downloads_for_test(&cx, files);
    sh.show_app_for_test(Box::new(app), PApp::Browser);
}

/// Tab to the link of this saved file in the browser and press Enter on it.
fn choose(sh: &mut Shell, g: &mut Gfx, name: &str) {
    sh.render(g);
    let want = format!("iyakki-open:{name}");
    for _ in 0..16 {
        let at = match &sh.screen {
            Screen::App(a, _) => a.probe_link(),
            _ => None,
        };
        if at.as_deref() == Some(want.as_str()) {
            sh.handle(&Event::Key(Key::Enter));
            return;
        }
        sh.handle(&Event::Key(Key::Tab));
        sh.render(g);
    }
    panic!("no link for {name}");
}

#[test]
fn a_text_file_chosen_in_downloads_opens_in_edit_and_back_returns_to_the_browser() {
    let mut sh = signed_in("dlopen-edit");
    let mut g = Gfx::new(1280, 720);
    browser_with(&mut sh, &[("notes.txt", "hello from downloads"), ("table.csv", "a,b\n1,2\n"), ("song.mp3", "id3")]);
    assert_eq!(sh.screen_name(), "app:browser");
    choose(&mut sh, &mut g, "notes.txt");
    assert_eq!(sh.screen_name(), "app:edit", "the shell opened EDIT");
    assert!(sh.probe_for_test().contains("notes.txt"), "EDIT holds the file: {}", sh.probe_for_test());
    // Back (Esc in EDIT, nothing unsaved) returns to the browser that was kept aside
    sh.render(&mut g);
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_name(), "app:browser", "Back returns to the browser");
}

#[test]
fn a_table_chosen_in_downloads_opens_in_data_with_its_path() {
    let mut sh = signed_in("dlopen-data");
    let mut g = Gfx::new(1280, 720);
    browser_with(&mut sh, &[("table.csv", "a,b\n1,2\n")]);
    choose(&mut sh, &mut g, "table.csv");
    assert_eq!(sh.screen_name(), "app:data");
    assert!(sh.probe_for_test().contains("table.csv"), "DATA was handed the table: {}", sh.probe_for_test());
}

#[test]
fn a_song_chosen_in_downloads_is_queued_in_media_and_a_file_that_vanished_is_said() {
    let mut sh = signed_in("dlopen-media");
    let mut g = Gfx::new(1280, 720);
    browser_with(&mut sh, &[("song.mp3", "id3"), ("gone.txt", "x")]);
    choose(&mut sh, &mut g, "song.mp3");
    assert_eq!(sh.screen_name(), "app:media", "the shell opened MEDIA");
    let p = sh.probe_for_test();
    assert!(p.contains("song.mp3") && p.contains("at=0"), "MEDIA got the play: item: {p}");
    // the audit line names the tile, not the file
    let log = sh.env.audit.recent(50).join("\n");
    assert!(log.contains("browser.download.open"), "{log}");
}

// ---- F-A11Y-02: the title of a dialog a tile opens is spoken once

#[derive(Default)]
struct Log(Vec<String>);
struct Voice(Arc<Mutex<Log>>);
impl SpeechSink for Voice {
    fn say(&mut self, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().0.push(text.to_string());
        Ok(())
    }
    fn busy(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
}

#[test]
fn the_reader_says_the_title_of_a_dialog_once_when_a_tile_opens_one() {
    let mut sh = signed_in("dlg-title");
    let log = Arc::new(Mutex::new(Log::default()));
    sh.access_sink_for_test(Box::new(Voice(log.clone())));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::ScreenReader, true));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::ScreenReader, false));
    assert!(sh.access_config().screen_reader);
    let mut g = Gfx::new(1280, 720);
    sh.open_for_test(PApp::Edit);
    for _ in 0..4 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    let before = log.lock().unwrap().0.len();
    sh.handle(&Event::Key(Key::Ctrl('o')));
    for _ in 0..6 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    let said: Vec<String> = log.lock().unwrap().0[before..].to_vec();
    let n = said.iter().filter(|s| s.starts_with("Dialog. Open")).count();
    assert_eq!(n, 1, "the open dialog is announced once: {said:?}");
    // closing it and opening another announces again; staying open does not repeat
    sh.handle(&Event::Key(Key::Esc));
    for _ in 0..4 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    sh.handle(&Event::Key(Key::Ctrl('o')));
    for _ in 0..6 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    let all: Vec<String> = log.lock().unwrap().0[before..].to_vec();
    assert_eq!(all.iter().filter(|s| s.starts_with("Dialog. Open")).count(), 2, "{all:?}");
}

#[test]
fn with_the_reader_off_a_dialog_is_not_announced() {
    let mut sh = signed_in("dlg-off");
    let log = Arc::new(Mutex::new(Log::default()));
    sh.access_sink_for_test(Box::new(Voice(log.clone())));
    let mut g = Gfx::new(1280, 720);
    sh.open_for_test(PApp::Edit);
    sh.render(&mut g);
    sh.handle(&Event::Key(Key::Ctrl('o')));
    for _ in 0..6 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    assert!(log.lock().unwrap().0.is_empty(), "silent while the reader is off: {:?}", log.lock().unwrap().0);
}
