//! VOICE, RECORD with a fake microphone and a fake speaker: saving, the list, rename, delete (with its question), no
//! microphone, one microphone for recorder and dictation, and the picture at four sizes.

use super::*;
use crate::apps::voice_rec::*;

fn folder(tag: &str) -> std::path::PathBuf {
    temp_dir(&format!("{tag}-recs")).join("Recordings")
}

fn rec_app(b: &Bench, dir: &std::path::Path) -> VoiceApp {
    let mut a = b.app().with_recordings_dir(dir.to_path_buf());
    key(&mut a, &b.cx, Key::F(4));
    a
}

fn names(a: &VoiceApp) -> Vec<String> {
    a.recordings().iter().map(|e| e.name.clone()).collect()
}

fn record_one(a: &mut VoiceApp, cx: &AppCx) {
    click(a, cx, T_RR_REC);
    assert!(a.is_recording_to_file());
    tick(a, cx);
    click(a, cx, T_RR_STOP);
    assert!(!a.is_recording_to_file());
}

#[test]
fn nothing_is_recorded_until_record_is_pressed_then_stop_saves_a_16_bit_wav() {
    let b = bench("rec-save");
    let dir = folder("rec-save");
    let mut a = rec_app(&b, &dir);
    assert_eq!(a.area(), Area::Record);
    for _ in 0..5 {
        tick(&mut a, &b.cx);
    }
    assert!(b.recs.started.lock().unwrap().is_empty() && !a.is_recording_to_file(), "opening the tab and waiting records nothing");
    assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
    record_one(&mut a, &b.cx);
    assert_eq!(b.recs.started.lock().unwrap().len(), 1, "the same recorders dictation uses, started once");
    assert_eq!(b.recs.started.lock().unwrap()[0].1, iyakki_agent::voicedesk::MAX_RECORD_SECS);
    let list = a.recordings().to_vec();
    assert_eq!(list.len(), 1);
    assert!(list[0].name.starts_with("recording-") && status(&a).starts_with("Saved recording-"), "{}", status(&a));
    let bytes = std::fs::read(dir.join(list[0].file())).unwrap();
    let w = iyakki_agent::voice::parse_wav(&bytes).unwrap();
    assert_eq!((w.rate, w.channels, w.samples.len()), (16_000, 1, speech().len()));
    assert_eq!(&bytes[34..36], &16u16.to_le_bytes());
    // a second one is a second file; the audit trail names the recording, not the sound
    std::thread::sleep(Duration::from_millis(5));
    record_one(&mut a, &b.cx);
    assert_eq!(a.recordings().len(), 2);
    // a new tile lists what is there
    let mut again = rec_app(&b, &dir);
    tick(&mut again, &b.cx);
    assert_eq!(again.recordings().len(), 2);
}

#[test]
fn a_recording_is_played_through_the_speaker_and_stopped() {
    let mut b = bench("rec-play");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_secs(60) });
    let dir = folder("rec-play");
    let mut a = rec_app(&b, &dir);
    click(&mut a, &b.cx, T_RR_PLAY);
    assert!(status(&a).contains("no recording"), "{}", status(&a));
    record_one(&mut a, &b.cx);
    click(&mut a, &b.cx, T_RR_PLAY);
    assert!(a.is_playing_recording());
    let played = b.sink.played.lock().unwrap().clone();
    assert_eq!(played.len(), 1);
    assert_eq!(played[0], std::fs::read(dir.join(a.recordings()[0].file())).unwrap());
    click(&mut a, &b.cx, T_RR_PSTOP);
    assert!(!a.is_playing_recording());
    // Enter plays, Esc stops (and a second Esc goes back)
    key(&mut a, &b.cx, Key::Enter);
    assert!(a.is_playing_recording());
    key(&mut a, &b.cx, Key::Esc);
    assert!(!a.is_playing_recording());
    // recording stops the playback: one speaker, one microphone
    key(&mut a, &b.cx, Key::Enter);
    key(&mut a, &b.cx, Key::Char(' '));
    assert!(a.is_recording_to_file() && !a.is_playing_recording());
    click(&mut a, &b.cx, T_RR_PLAY);
    assert!(status(&a).contains("Stop recording first"), "{}", status(&a));
    key(&mut a, &b.cx, Key::Char(' '));
    // no sound output: said in words
    let mut quiet = {
        let mut q = b.app_with_output(Err("none".into())).with_recordings_dir(dir.clone());
        key(&mut q, &b.cx, Key::F(4));
        q
    };
    click(&mut quiet, &b.cx, T_RR_PLAY);
    assert!(status(&quiet).contains("No sound output") && !quiet.is_playing_recording());
}

#[test]
fn a_recording_that_ends_by_itself_is_noticed() {
    let mut b = bench("rec-ends");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(30) });
    let dir = folder("rec-ends");
    let mut a = rec_app(&b, &dir);
    record_one(&mut a, &b.cx);
    click(&mut a, &b.cx, T_RR_PLAY);
    wait(&mut a, &b.cx, |a| !a.is_playing_recording());
    assert!(status(&a).starts_with("Finished playing"), "{}", status(&a));
}

#[test]
fn rename_works_by_mouse_and_keyboard_and_never_overwrites_or_leaves_the_folder() {
    let b = bench("rec-rename");
    let dir = folder("rec-rename");
    let mut a = rec_app(&b, &dir);
    record_one(&mut a, &b.cx);
    std::thread::sleep(Duration::from_millis(1100));
    record_one(&mut a, &b.cx);
    assert_eq!(a.recordings().len(), 2);
    // by keyboard: n opens the box with the name in it, type, Enter
    key(&mut a, &b.cx, Key::Char('n'));
    key(&mut a, &b.cx, Key::Ctrl('a'));
    for c in "Meeting notes".chars() {
        key(&mut a, &b.cx, Key::Char(c));
    }
    key(&mut a, &b.cx, Key::Enter);
    assert!(names(&a).contains(&"Meeting notes".to_string()), "{:?} {}", names(&a), status(&a));
    assert!(dir.join("Meeting notes.wav").is_file());
    // a name that is taken, one that names a folder: refused, the box stays open to fix it
    let other = names(&a).into_iter().find(|n| n != "Meeting notes").unwrap();
    click(&mut a, &b.cx, T_RR_RENAME);
    a.rec.name.set(&other);
    click(&mut a, &b.cx, T_RR_SAVE_NAME);
    assert!(status(&a).contains("already exists"), "{}", status(&a));
    a.rec.name.set("sub/escape");
    key(&mut a, &b.cx, Key::Enter);
    assert!(status(&a).contains("cannot contain"), "{}", status(&a));
    assert!(!dir.parent().unwrap().join("escape.wav").exists());
    // Esc cancels, nothing changed
    key(&mut a, &b.cx, Key::Esc);
    assert_eq!(names(&a).len(), 2);
    // by mouse: pick the other row, Rename, Save name
    let row = a.recordings().iter().position(|e| e.name == other).unwrap();
    a.handle(&Event::Tab(T_RR_ROW + row), &b.cx);
    click(&mut a, &b.cx, T_RR_RENAME);
    a.rec.name.set("Second");
    click(&mut a, &b.cx, T_RR_SAVE_NAME);
    assert!(dir.join("Second.wav").is_file() && !dir.join(format!("{other}.wav")).exists());
    assert_eq!(status(&a), "Renamed to Second.");
}

#[test]
fn delete_asks_first_and_only_yes_removes_the_file() {
    let b = bench("rec-delete");
    let dir = folder("rec-delete");
    let mut a = rec_app(&b, &dir);
    record_one(&mut a, &b.cx);
    let file = dir.join(a.recordings()[0].file());
    // the question is on screen with both answers
    click(&mut a, &b.cx, T_RR_DELETE);
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "Delete") && label_hit(&g, "Keep it"));
    click(&mut a, &b.cx, T_RR_NO);
    assert!(file.is_file());
    // Esc, n and an unrelated key all keep it
    for k in [Key::Esc, Key::Char('n'), Key::Enter, Key::Char(' ')] {
        key(&mut a, &b.cx, Key::Delete);
        key(&mut a, &b.cx, k);
        assert!(file.is_file() && a.recordings().len() == 1);
    }
    assert_eq!(a.recordings().len(), 1);
    key(&mut a, &b.cx, Key::Delete);
    key(&mut a, &b.cx, Key::Char('y'));
    assert!(!file.exists() && a.recordings().is_empty());
    assert!(status(&a).starts_with("Deleted"), "{}", status(&a));
    // by mouse
    record_one(&mut a, &b.cx);
    let file = dir.join(a.recordings()[0].file());
    click(&mut a, &b.cx, T_RR_DELETE);
    click(&mut a, &b.cx, T_RR_YES);
    assert!(!file.exists());
}

#[test]
fn no_microphone_is_said_plainly_and_nothing_is_started() {
    let b = bench("rec-nomic");
    let r = recs(vec![], "MacBookPro14,1");
    let dir = folder("rec-nomic");
    let mut a = b.app_with_recs(r.clone()).with_recordings_dir(dir.clone());
    key(&mut a, &b.cx, Key::F(4));
    click(&mut a, &b.cx, T_RR_REC);
    key(&mut a, &b.cx, Key::Char(' '));
    assert!(!a.is_recording_to_file() && r.started.lock().unwrap().is_empty());
    let s = status(&a);
    assert!(s.contains("No microphone found") && s.contains("USB microphones work"), "{s}");
    for (w, h) in SIZES {
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "no microphone");
        assert!(!label_hit(&g, "● Record"), "the Record button is off without a microphone");
    }
}

#[test]
fn the_recorder_and_dictation_never_hold_the_microphone_together_and_leaving_saves() {
    let b = bench("rec-exclusive");
    let dir = folder("rec-exclusive");
    let mut a = rec_app(&b, &dir);
    // dictation first
    key(&mut a, &b.cx, Key::F(3));
    click(&mut a, &b.cx, T_REC);
    assert!(a.is_recording());
    key(&mut a, &b.cx, Key::F(4));
    click(&mut a, &b.cx, T_RR_REC);
    assert!(!a.is_recording_to_file() && status(&a).contains("Dictation is using the microphone"), "{}", status(&a));
    assert_eq!(b.recs.started.lock().unwrap().len(), 1, "no second way to open the device");
    key(&mut a, &b.cx, Key::F(3));
    a.stop_record(&b.cx, false);
    // the recorder first
    key(&mut a, &b.cx, Key::F(4));
    click(&mut a, &b.cx, T_RR_REC);
    assert!(a.is_recording_to_file());
    key(&mut a, &b.cx, Key::F(3));
    assert!(!a.is_recording_to_file(), "leaving Record ends the recording");
    assert_eq!(a.recordings().len(), 1, "and keeps it");
    click(&mut a, &b.cx, T_REC);
    assert!(a.is_recording());
    a.stop_record(&b.cx, false);
    // the recorder blocks dictation
    key(&mut a, &b.cx, Key::F(4));
    click(&mut a, &b.cx, T_RR_REC);
    a.start_record(&b.cx);
    assert!(!a.is_recording() && status(&a).contains("recorder is using the microphone"));
    // closing the app ends the recording
    let r = recs(vec![usb()], "x");
    let mut c = b.app_with_recs(r.clone()).with_recordings_dir(dir);
    key(&mut c, &b.cx, Key::F(4));
    click(&mut c, &b.cx, T_RR_REC);
    drop(c);
    assert!(r.cfg.dropped.load(Ordering::SeqCst));
}

#[test]
fn a_dead_microphone_and_the_time_limit_are_handled() {
    let b = bench("rec-limits");
    let dir = folder("rec-limits");
    let mut a = rec_app(&b, &dir);
    click(&mut a, &b.cx, T_RR_REC);
    b.recs.cfg.full.store(true, Ordering::SeqCst);
    tick(&mut a, &b.cx);
    assert!(!a.is_recording_to_file() && a.recordings().len() == 1 && status(&a).contains("limit"), "{}", status(&a));
    b.recs.cfg.full.store(false, Ordering::SeqCst);
    // the program ended by itself and captured nothing: its words are the message
    let mut dead_cfg = cap_cfg();
    dead_cfg.pcm = vec![];
    let dead = Arc::new(FakeRecs { mics: vec![usb()], product: "x".into(), cfg: dead_cfg, started: Mutex::new(vec![]) });
    let mut d = b.app_with_recs(dead).with_recordings_dir(dir);
    key(&mut d, &b.cx, Key::F(4));
    click(&mut d, &b.cx, T_RR_REC);
    key(&mut d, &b.cx, Key::Char(' '));
    assert!(status(&d).contains("audio open error"), "{}", status(&d));
    assert_eq!(d.recordings().len(), 1, "nothing new was saved");
}

#[test]
fn record_mode_draws_in_every_state_at_four_sizes_and_every_control_is_on_screen() {
    let mut b = bench("rec-draw");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_secs(60) });
    for (w, h) in SIZES {
        let dir = folder(&format!("rec-draw-{w}"));
        let mut a = rec_app(&b, &dir);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "empty");
        assert!(colours(&g) > 12);
        for l in ["READ ALOUD", "DICTATE", "RECORD", "● Record", "microphone"] {
            assert!(label_hit(&g, l), "{l} at {w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        // recording
        click(&mut a, &b.cx, T_RR_REC);
        for _ in 0..3 {
            tick(&mut a, &b.cx);
        }
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "recording");
        assert!(label_hit(&g, "Stop"), "{w}x{h}");
        click(&mut a, &b.cx, T_RR_STOP);
        // a long list with a long name
        for i in 0..12 {
            iyakki_agent::recordings::save(&dir, &format!("A rather long recording name number {i} for the narrow screens"), &speech(), 16_000).unwrap();
        }
        a.rec_refresh(&b.cx);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "list");
        for l in ["Play", "Rename", "Delete", "recording"] {
            assert!(label_hit(&g, l), "{l} at {w}x{h}");
        }
        for _ in 0..14 {
            key(&mut a, &b.cx, Key::Down);
        }
        inside(&draw(&mut a, &b.cx, w, h), w, h, "selection at the end");
        // playing, renaming, the delete question
        click(&mut a, &b.cx, T_RR_PLAY);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "playing");
        assert!(label_hit(&g, "Stop playing"));
        click(&mut a, &b.cx, T_RR_PSTOP);
        click(&mut a, &b.cx, T_RR_RENAME);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "rename");
        assert!(label_hit(&g, "Save name") && label_hit(&g, "Cancel"));
        key(&mut a, &b.cx, Key::Esc);
        click(&mut a, &b.cx, T_RR_DELETE);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "delete question");
        assert!(label_hit(&g, "Keep it"));
        click(&mut a, &b.cx, T_RR_NO);
        a.say("The microphone could not be opened: the device is busy, so nothing was recorded this time.", false);
        inside(&draw(&mut a, &b.cx, w, h), w, h, "error");
        // the microphone picker still works from here
        click(&mut a, &b.cx, T_MIC);
        inside(&draw(&mut a, &b.cx, w, h), w, h, "microphone picker");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
