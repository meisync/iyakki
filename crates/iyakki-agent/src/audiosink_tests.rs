use super::*;
use crate::mixer::{FakeSink, MixerCfg};

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-audiosink-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn mixer() -> (Mixer, FakeSink) {
    let sink = FakeSink::new();
    (Mixer::start(MixerCfg { idle_ms: 100, engine: crate::mixer::EngineCfg::default().no_prefill(), ..MixerCfg::default() }, Box::new(sink.clone())), sink)
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn with_the_mixer_off_sound_goes_to_the_play_command_as_before() {
    let d = tmp("off");
    let out = d.join("o.wav");
    let mut o = open_output_via(None, SourceKind::Radio, 44_100, 2, "radio", &format!("tee {}", out.display())).unwrap();
    assert!(!o.is_mixed());
    o.set_volume(100);
    o.write(&vec![0i16; 4608]).unwrap();
    o.finish(Duration::from_secs(4), &|| true);
    let wav = std::fs::read(&out).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 44_100);
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
    assert_eq!(wav.len(), 44 + 4608 * 2);
    // a player that is not there is an error with the old words
    let e = open_output_via(None, SourceKind::Radio, 44_100, 2, "radio", "/nonexistent/player").err().unwrap();
    assert!(e.contains("cannot start"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn with_the_mixer_on_sound_is_a_source_of_the_kind_and_reaches_the_card() {
    let (m, sink) = mixer();
    let mut o = open_output_via(Some(&m), SourceKind::Radio, 48_000, 2, "radio", "/nonexistent/player").unwrap();
    assert!(o.is_mixed() && o.kind() == SourceKind::Radio, "the play command is not started");
    o.set_volume(100);
    o.write(&vec![8000i16; 48_000 / 5 * 2]).unwrap();
    wait_for("samples at the card", || sink.lock().samples.iter().any(|&s| s > 7000));
    assert_eq!(m.sources().len(), 1);
    assert_eq!(m.sources()[0].kind, SourceKind::Radio);
    o.finish(Duration::from_secs(3), &|| true);
    wait_for("the source to end", || m.sources().is_empty());
    m.shutdown();
}

#[test]
fn a_wav_is_mixed_or_piped_and_stop_cuts_it_off() {
    let wav = crate::voice::encode_wav(&vec![6000i16; 4800], 48_000);
    let (m, sink) = mixer();
    let mut o = play_wav_via(Some(&m), SourceKind::Speech, "speech", &wav, "/nonexistent/player").unwrap();
    assert!(o.is_mixed() && o.kind() == SourceKind::Speech);
    wait_for("the speech at the card", || sink.lock().samples.iter().any(|&s| s > 5000));
    wait_for("the end of the speech", || !o.is_playing());
    m.shutdown();

    let d = tmp("wav");
    let out = d.join("o.wav");
    let mut p = play_wav_via(None, SourceKind::Speech, "speech", &wav, &format!("tee {}", out.display())).unwrap();
    assert!(!p.is_mixed());
    wait_for("tee to copy the WAV", || std::fs::read(&out).map(|b| b == wav).unwrap_or(false));
    wait_for("tee to end", || !p.is_playing());
    let mut long = play_wav_via(None, SourceKind::Speech, "speech", &wav, "sleep 30").unwrap();
    assert!(long.is_playing());
    long.stop();
    wait_for("stop", || !long.is_playing());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_blocking_cue_goes_to_the_mixer_or_to_the_play_command() {
    let wav = crate::voice::encode_wav(&vec![3000i16; 2205], 44_100);
    let (m, sink) = mixer();
    play_wav_blocking_via(Some(&m), SourceKind::Notification, "cue", &wav, "/nonexistent/player", Duration::from_secs(5)).unwrap();
    assert!(sink.lock().samples.iter().any(|&s| s > 2000));
    m.shutdown();
    let d = tmp("cue");
    let out = d.join("c.wav");
    play_wav_blocking_via(None, SourceKind::Notification, "cue", &wav, &format!("tee {}", out.display()), Duration::from_secs(5)).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), wav);
    assert!(play_wav_blocking_via(None, SourceKind::Notification, "cue", &wav, "", Duration::from_secs(1)).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_kinds_have_the_priorities_alarm_speech_notification_media_radio() {
    let order = [SourceKind::Radio, SourceKind::Media, SourceKind::Notification, SourceKind::Speech, SourceKind::Alarm];
    assert!(order.windows(2).all(|w| w[0].priority() < w[1].priority()));
    assert!(SourceKind::Speech.duck_to() < 1.0 && SourceKind::Radio.duck_to() == 1.0);
}

#[test]
fn only_the_entry_point_knows_about_the_mixer_setting() {
    // callers never read `sound.mixer` or ask the mixer for a source: they use `audiosink`. The audio output selector
    // (audioout.rs) applies the setting; the mixer's own files (webmix.rs feeds the browser's sound in as a source) are the mixer.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let needles = ["mixer::enabled(", "global_if_enabled(", "mixer::global()", "open_audio_source(", "prefs.mixer", "sound.mixer", "mixer_on"];
    let mut bad = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "mixer") {
                    continue;
                }
                stack.push(p);
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || ["audiosink.rs", "audiosink_tests.rs", "audioout.rs", "audioout_tests.rs", "webmix.rs"].contains(&name.as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for n in needles {
                if text.contains(n) {
                    bad.push(format!("{name}: {n}"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "these files talk to the mixer setting directly instead of through audiosink: {bad:?}");
}
