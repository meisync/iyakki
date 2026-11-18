//! The chat's read-aloud path with a real text model (qwen3-8b) and the real kokoro-v1 voice (ignored tests, a Lemonade server on
//! the LAN; see story_live_tests.rs for how to run them). The speaker is a script that keeps the sound it was given.
use iyakki_agent::facets_h::RealServices as _;

use super::*;
use crate::apps::tests::{cx_with_backend, services_in, temp_dir};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn env2() -> (String, String) {
    (std::env::var("IYAKKI_LIVE_LEMONADE_URL").expect("set IYAKKI_LIVE_LEMONADE_URL"), std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").expect("set IYAKKI_LIVE_LEMONADE_KEY_FILE"))
}

fn note(row: &str, name: &str, text: &str) {
    let (url, key_file) = env2();
    let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
    let mut t = text.to_string();
    for h in [key, iyakki_shared::lemonade::origin(&url).unwrap_or_default(), url.trim_end_matches('/').to_string()] {
        if !h.is_empty() {
            t = t.replace(h.as_str(), "<redacted>");
        }
    }
    eprintln!("[{row}] {name}: {}", t.lines().next().unwrap_or(""));
    if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
        let d = std::path::Path::new(&d).join(row);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(format!("{name}.txt")), t);
    }
}

#[test]
#[ignore = "live service"]
fn live_chat_reply_from_qwen3_is_read_aloud_sentence_by_sentence_by_kokoro() {
    let _route = iyakki_agent::mixer::pin_command_route();
    let (url, key_file) = env2();
    let (mut cx, _d) = cx_with_backend("live-chat-read", iyakki_shared::chat::Unconfigured);
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
    cx.services = services_in(&temp_dir("live-chat-read-svc"), up);
    let mut p = iyakki_shared::providers::ProviderCfg::new("lemon", iyakki_shared::providers::Kind::Lemonade);
    p.base_url = Some(url.clone());
    p.key_ref = Some(format!("file:{key_file}"));
    cx.settings().lock().unwrap().upsert_provider(p).unwrap();
    cx.services.refresh_lemonade("lemon").expect("typed list");
    let client = cx.services.real().client_for("lemon", false).unwrap();
    cx.chat = Arc::new(iyakki_agent::providers::ProviderChat { client, model: "qwen3-8b".into(), streamer: None });
    let dir = temp_dir("live-chat-read-speaker");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("rec.sh");
    std::fs::write(&script, format!("#!/bin/sh\ncat > '{d}'/piece.$$.wav\n", d = dir.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    cx.services
        .settings()
        .lock()
        .unwrap()
        .update(|s| {
            s.voice.tts = Some(iyakki_shared::lemonade::tts_engine(&url, "kokoro-v1", None, Some(format!("file:{key_file}"))));
            s.voice.play_cmd = script.display().to_string();
        })
        .unwrap();
    cx.services.allow_speech_server(&iyakki_shared::lemonade::api_base(&url)).unwrap();
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('o')), &cx);
    assert_eq!(app.speak_on, Some(true));
    let mut report = Vec::new();
    for attempt in 1..=3 {
        let before = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "wav"))).count();
        app.input.set("Tell me three short facts about the moon, one sentence each. Plain sentences, no lists.");
        let t0 = Instant::now();
        app.handle(&Event::Key(Key::Enter), &cx);
        let end = Instant::now() + Duration::from_secs(240);
        let mut saw_speaking = false;
        while Instant::now() < end {
            app.handle(&Event::Tick, &cx);
            saw_speaking |= app.is_speaking();
            if saw_speaking && !app.is_waiting() && !app.is_speaking() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut wavs: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect();
        wavs.sort();
        let new = wavs.len() - before;
        let reply = app.msgs.iter().rev().find(|m| m.role == iyakki_shared::chat::Role::Assistant).map(|m| m.content.clone()).unwrap_or_default();
        let bad = ["<think", "</think", "```", "**"].iter().filter(|n| reply.contains(**n)).collect::<Vec<_>>();
        report.push(format!("attempt {attempt}: {} ms, {new} sound pieces, reply {} chars, leaks in the reply {bad:?}: {:?}", t0.elapsed().as_millis(), reply.len(), reply.chars().take(160).collect::<String>()));
        let ok_wavs = wavs.iter().skip(before).filter(|p| iyakki_shared::voice::parse_wav(&std::fs::read(p).unwrap()).is_ok_and(|w| (w.rate, w.channels) == (48_000, 1) && iyakki_shared::voice::peak(&w.samples) > 1500)).count();
        if saw_speaking && new >= 2 && ok_wavs == new && bad.is_empty() {
            let log = cx.audit.recent(40).join("\n");
            assert!(log.contains("voice.speak"), "{log}");
            report.push(format!("   {ok_wavs} pieces, each a whole 48 kHz WAV; the audit has voice.speak"));
            note("F-CHAT-23", "chat-read-aloud-qwen3-kokoro", &report.join("\n"));
            return;
        }
    }
    note("F-CHAT-23", "chat-read-aloud-qwen3-kokoro", &report.join("\n"));
    panic!("{}", report.join("\n"));
}
