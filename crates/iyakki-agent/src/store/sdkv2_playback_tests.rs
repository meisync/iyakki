use super::*;
use iyakki_core_fake::FakeMedia;

fn who() -> Who {
    Who { app: "player".into(), user: "alice".into() }
}

fn rig() -> (Arc<FakeMedia>, Arc<FakeSound>, PlaybackBackend) {
    let media = Arc::new(FakeMedia::new());
    media.add_file("alice", "music/a.mp3", 3000, false);
    media.add_file("alice", "music/b.mp3", 2000, false);
    media.add_file("alice", "film.mp4", 9000, true);
    media.add_file("bob", "secret.mp3", 1000, false);
    let sound = Arc::new(FakeSound::default());
    let files: AppFile = Arc::new(|app, file| (app == "player" && file == "beep.wav").then(|| b"RIFF-not-really".to_vec()));
    let b = PlaybackBackend::new(media.clone(), sound.clone(), files);
    (media, sound, b)
}

fn ok(b: &PlaybackBackend, op: &str, a: Value) -> Value {
    b.call(&who(), op, &a).unwrap_or_else(|e| panic!("{op}: {e:?}"))
}

#[test]
fn the_library_lists_this_persons_titles_by_kind_and_never_another_persons() {
    let (_m, _s, b) = rig();
    let all = ok(&b, "media.library", json!({}));
    assert_eq!(all.as_array().unwrap().len(), 3);
    let video = ok(&b, "media.library", json!({"kind": "video"}));
    assert_eq!(video[0]["id"], "film.mp4");
    assert_eq!(ok(&b, "media.library", json!({"kind": "audio"})).as_array().unwrap().len(), 2);
    assert!(!all.to_string().contains("secret"));
    assert_eq!(ok(&b, "media.in_folders", json!({"file": "music/a.mp3"}))["inside"], true);
    assert_eq!(ok(&b, "media.in_folders", json!({"file": "../x.mp3"}))["inside"], false);
}

#[test]
fn a_playback_plays_pauses_resumes_ends_and_the_queue_goes_on_and_only_the_app_that_started_it_may_touch_it() {
    let (m, _s, b) = rig();
    let id = ok(&b, "media.play", json!({"id": "music/a.mp3", "start": 1}))["id"].as_u64().unwrap();
    assert_eq!(ok(&b, "media.pause", json!({"id": id}))["state"], "paused");
    assert_eq!(ok(&b, "media.resume", json!({"id": id}))["state"], "playing");
    assert_eq!(ok(&b, "media.queue.add", json!({"id": "music/b.mp3"}))["queued"], 1);
    assert_eq!(ok(&b, "media.queue.list", json!({})).as_array().unwrap().len(), 1);
    assert!(b.call(&who(), "media.queue.add", &json!({"id": "nope.mp3"})).is_err(), "only names the person's list gave");
    m.advance(5000);
    let next = ok(&b, "media.state", json!({"id": id}));
    assert_eq!(next["state"], "playing", "the end moves on to the queue: {next}");
    assert_ne!(next["id"].as_u64().unwrap(), id);
    // another app has none of these
    let other = Who { app: "other".into(), user: "alice".into() };
    assert!(b.call(&other, "media.pause", &json!({"id": next["id"]})).is_err());
    // a stop closes it
    let nid = next["id"].as_u64().unwrap();
    assert_eq!(ok(&b, "media.stop", json!({"id": nid}))["state"], "stopped");
    assert!(b.call(&who(), "media.pause", &json!({"id": nid})).is_err());
    assert_eq!(ok(&b, "media.state", json!({}))["state"], "stopped");
}

#[test]
fn another_persons_file_a_bad_name_and_a_wrong_kind_do_not_play() {
    let (_m, _s, b) = rig();
    assert!(b.call(&who(), "media.play", &json!({"id": "secret.mp3"})).is_err(), "bob's file is not in alice's home");
    assert!(b.call(&who(), "media.play", &json!({"id": "../etc/passwd.mp3"})).is_err());
    assert!(b.call(&who(), "media.play", &json!({"id": "notes.txt"})).is_err());
}

#[test]
fn a_sound_plays_from_a_file_inside_the_app_or_a_wav_and_the_busy_speaker_names_its_holder() {
    let (_m, s, b) = rig();
    let id = ok(&b, "audio.play", json!({"src": {"file": "beep.wav"}, "volume": 0.5, "loop": true, "channel": "cue"}))["id"].as_u64().unwrap();
    {
        let st = s.started.lock().unwrap();
        assert_eq!((st[0].volume, st[0].looped, st[0].channel.as_str(), st[0].app.as_str()), (0.5, true, "cue", "player"));
    }
    ok(&b, "audio.setVolume", json!({"id": id, "volume": 3.0}));
    ok(&b, "audio.pause", json!({"id": id}));
    ok(&b, "audio.resume", json!({"id": id}));
    ok(&b, "audio.stop", json!({"id": id}));
    assert_eq!(*s.log.lock().unwrap(), ["volume 1 1", "pause 1", "resume 1", "stop 1"]);
    assert!(b.call(&who(), "audio.stop", &json!({"id": id})).is_err(), "stopped once");
    assert!(b.call(&who(), "audio.play", &json!({"src": {"file": "other.wav"}})).is_err(), "a file the app does not have");
    assert!(b.call(&who(), "audio.play", &json!({"src": {"wav": "!!"}})).is_err());
    *s.busy.lock().unwrap() = Some("the radio".into());
    let e = b.call(&who(), "audio.play", &json!({"src": {"file": "beep.wav"}})).unwrap_err();
    assert_eq!((e.code, e.extra.get("holder").and_then(Value::as_str)), (code::BUSY, Some("the radio")));
    assert_eq!(ok(&b, "audio.outputs", json!({}))[0]["name"], "Speakers");
}

#[test]
fn the_system_backend_refuses_a_name_that_leaves_the_app_and_the_bundle_keeps_only_small_wavs() {
    use super::super::tile::{Bundle, FileData, TileManifest};
    let mut bundle = Bundle { manifest: TileManifest { id: "player".into(), name: "Player".into(), version: "0.1.0".into(), icon: String::new(), group: String::new(), caps: Default::default(), core_caps: vec![], ext: Default::default() }, files: Default::default() };
    bundle.files.insert("sounds/beep.wav".into(), FileData::Bytes(vec![1, 2, 3]));
    bundle.files.insert("sounds/big.wav".into(), FileData::Bytes(vec![0; 2 * 1024 * 1024 + 1]));
    bundle.files.insert("notes.txt".into(), FileData::Text("x".into()));
    let files = files_of_bundle("player", &bundle);
    assert_eq!(files("player", "sounds/beep.wav"), Some(vec![1, 2, 3]));
    assert_eq!(files("player", "sounds/big.wav"), None);
    assert_eq!(files("player", "notes.txt"), None);
    assert_eq!(files("other", "sounds/beep.wav"), None, "another app's name gets nothing");
    let media: Arc<dyn Media> = Arc::new(FakeMedia::new());
    // not `install_player`: that slot is the process's, and other tests expect the calls to be unavailable without it
    let backend = playback_over(media, Arc::new(FakeSound::default()), files);
    for bad in ["../beep.wav", "/sounds/beep.wav", "sounds//beep.wav"] {
        assert!(backend.call(&who(), "audio.play", &json!({"src": {"file": bad}})).is_err(), "{bad}");
    }
}

#[test]
fn an_mp3_inside_the_app_is_decoded_to_a_wav_for_the_sound_path_and_a_text_that_is_neither_is_refused() {
    let mp3 = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples/Music/IYAKKI Sample Beats.mp3")).unwrap();
    let wav = mp3_to_wav(&mp3[..24_000]).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
    assert!(matches!(rate, 8000 | 11025 | 12000 | 16000 | 22050 | 24000 | 32000 | 44100 | 48000), "{rate}");
    assert!(wav.len() > 44 + 2000, "some sound came out: {} bytes", wav.len());
    assert!(crate::voice::parse_wav(&wav).is_ok(), "the sound path accepts it");
    assert!(mp3_to_wav(b"just some words, no sound at all").is_err());
    assert!(mp3_to_wav(&[]).is_err());
    use super::super::tile::{Bundle, FileData, TileManifest};
    let mut bundle = Bundle { manifest: TileManifest { id: "player".into(), name: "Player".into(), version: "0.1.0".into(), icon: String::new(), group: String::new(), caps: Default::default(), core_caps: vec![], ext: Default::default() }, files: Default::default() };
    bundle.files.insert("a/tune.MP3".into(), FileData::Bytes(mp3[..2000].to_vec()));
    let files = files_of_bundle("player", &bundle);
    assert!(files("player", "a/tune.MP3").is_some());
}

#[test]
fn a_second_reader_of_the_state_finds_no_events_and_still_gets_the_true_state_and_a_new_playback_forgets_it() {
    // the playback viewer inside an app's page reads the state beside the app's own reads: the events go to whoever looks first
    let (_m, _s, b) = rig();
    let id = ok(&b, "media.play", json!({"id": "music/a.mp3"}))["id"].as_u64().unwrap();
    let first = ok(&b, "media.state", json!({"id": id}));
    assert_eq!(first["state"], "playing");
    let second = ok(&b, "media.state", json!({"id": id}));
    assert_eq!(second["state"], "playing", "no new event is not 'ready': {second}");
    assert_eq!(ok(&b, "media.pause", json!({"id": id}))["state"], "paused");
    assert_eq!(ok(&b, "media.state", json!({"id": id}))["state"], "paused");
    assert_eq!(ok(&b, "media.state", json!({"id": id}))["state"], "paused", "and again");
    let again = ok(&b, "media.play", json!({"id": "music/b.mp3"}))["id"].as_u64().unwrap();
    assert_ne!(again, id);
}
