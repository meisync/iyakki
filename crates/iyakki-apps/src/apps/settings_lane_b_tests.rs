//! Settings tests of lane ST-B moved out of `settings.rs` (S0).

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_sec::policy::Role;
use std::sync::Mutex;
use super::tests::*;

#[test]
fn the_nvidia_quick_add_shows_the_steps_refuses_a_key_that_is_not_one_and_stores_a_good_one_in_the_vault() {
    let (cx, _) = admin("setnv");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('N'));
    assert!(a.has_modal());
    // a pasted web address or a short word is not a key: the form stays open and says what a key looks like
    typ(&mut a, &cx, "https://build.nvidia.com");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a wrong key keeps the form open");
    assert!(cx.settings().lock().unwrap().get().providers.is_empty(), "nothing is stored for a rejected key");
    for _ in 0..30 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "nvapi-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    settle(&mut a, &cx);
    let p = cx.settings().lock().unwrap().get().providers.clone();
    assert_eq!((p.len(), p[0].id.as_str(), p[0].kind, p[0].key_ref.as_deref()), (1, "nvidia", Kind::Nvidia, Some("vault:nvidia-key")));
    assert!(!format!("{p:?}").contains("nvapi-ABCDEF"), "the key itself is never in the settings");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.provider.save") && l.contains("nvidia")));
}

#[test]
fn the_settings_test_and_test_chat_results_tell_a_failure_in_plain_words_without_keys_or_ids() {
    let mode = Arc::new(Mutex::new(401u16));
    let m2 = mode.clone();
    let mut cx = cx_with_admin_store("set-plainerr");
    let d = temp_dir("set-plainerr-svc");
    cx.services = services_in(&d, Arc::new(Mock {
        calls: Default::default(),
        reply: Box::new(move |_, _| match *m2.lock().unwrap() {
            401 => (401, r#"{"error":{"message":"Incorrect API key provided: sk-abcdef1234567890"}}"#.into()),
            404 => (404, "404 page not found".into()),
            n => (n, "overloaded".into()),
        }),
    }));
    cx.services.on_login("root", "rootpass12", true);
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "local");
    for _ in 0..5 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(!a.has_modal(), "{}", status(&a));
    cx.settings().lock().unwrap().update(|s| s.chat = Some(iyakki_shared::settings::ModelChoice { provider: "local".into(), model: "qwen3:32b".into() })).unwrap();
    let ok = |a: &SettingsApp| a.status().map(|s| s.1);
    // a refused key points at Settings and never prints the key
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("local: local refused the key") && status(&a).contains("Settings -> Models") && !status(&a).contains("sk-abcdef"), "{}", status(&a));
    assert_eq!(ok(&a), Some(false), "shown as a failure");
    key(&mut a, &cx, Key::Char('c'));
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("local: local refused the key") && !status(&a).contains("sk-abcdef"), "{}", status(&a));
    // a wrong address is told apart from a missing model
    *mode.lock().unwrap() = 404;
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("did not find that address") && !status(&a).contains("HTTP 404: 404 page not found\n"), "{}", status(&a));
    // trouble at the other end
    *mode.lock().unwrap() = 503;
    key(&mut a, &cx, Key::Char('c'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("having trouble right now"), "{}", status(&a));
    assert_eq!(ok(&a), Some(false));
}

#[test]
fn admin_adds_ollama_tests_it_and_picks_a_model() {
    let (cx, _) = admin("set1");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('a'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "local");
    for _ in 0..5 {
        key(&mut a, &cx, Key::Enter); // id → type → url → key → model → submit
    }
    assert!(!a.has_modal(), "{}", status(&a));
    let p = cx.settings().lock().unwrap().get().providers.clone();
    assert_eq!((p.len(), p[0].id.as_str(), p[0].kind), (1, "local", Kind::Ollama));
    assert_eq!(p[0].base_url.as_deref(), Some("http://localhost:11434"));
    // test connection fetches and caches the model list
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("2 model(s)"), "{}", status(&a));
    assert_eq!(cx.settings().lock().unwrap().get().providers[0].models, vec!["llama3.2:3b", "qwen3:32b"]);
    // choose the chat model by filtering the picker
    key(&mut a, &cx, Key::Char('m'));
    typ(&mut a, &cx, "qwen");
    key(&mut a, &cx, Key::Enter);
    let chat = cx.settings().lock().unwrap().get().chat.clone().unwrap();
    assert_eq!((chat.provider.as_str(), chat.model.as_str()), ("local", "qwen3:32b"));
    // and the chat app's live backend now answers through it
    key(&mut a, &cx, Key::Char('c'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("replied: OK"), "{}", status(&a));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.chat_model") && l.contains("local/qwen3:32b")));
}

#[test]
fn api_keys_go_to_the_vault_never_to_settings_or_logs() {
    let (cx, _d) = admin("set2");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "openai");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right); // Ollama → OpenAI: URL default follows
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "sk-super-secret-value");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{:?}", a.status());
    let st = cx.settings().lock().unwrap().get().clone();
    assert_eq!(st.providers[0].base_url.as_deref(), Some("https://api.openai.com/v1"));
    assert_eq!(st.providers[0].key_ref.as_deref(), Some("vault:openai-key"));
    assert_eq!(&*cx.secrets().resolve("vault:openai-key", None).unwrap(), "sk-super-secret-value");
    let settings_file = std::fs::read_to_string(cx.data_dir().join("settings.json")).unwrap();
    assert!(!settings_file.contains("sk-super-secret-value"));
    let audit = std::fs::read_to_string(cx.data_dir().join("audit.log")).unwrap_or_default();
    assert!(!audit.contains("sk-super-secret-value"));
    assert!(cx.audit.recent(50).iter().all(|l| !l.contains("sk-super-secret-value")));
    // the vault file itself is encrypted
    let vault = std::fs::read_to_string(cx.data_dir().join("vault/system.afv")).unwrap();
    assert!(!vault.contains("sk-super-secret-value"));
}

#[test]
fn a_rejected_form_stores_nothing() {
    let (cx, _) = admin("set3");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    // OpenAI needs a key: submitting without one is refused and the form stays open
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "openai");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right);
    for _ in 0..4 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(a.has_modal());
    assert!(cx.settings().lock().unwrap().get().providers.is_empty());
    // a locked system vault makes saving a key fail cleanly (no half-saved provider)
    cx.services.lock_system();
    key(&mut a, &cx, Key::Tab); // wrap focus
    key(&mut a, &cx, Key::Tab);
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "sk-abc");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal());
    assert!(cx.settings().lock().unwrap().get().providers.is_empty(), "no provider without its key");
    // Esc closes the dialog
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal());
}

#[test]
fn host_access_says_why_it_is_unavailable_and_in_window_mode_lists_mounts_with_actions_and_the_audit_trail() {
    let _g = iyakki_shared::hostfiles::test_guard();
    let (cx, _) = admin("hostacc");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::HostAccess));
    // not window mode: a plain explanation, no actions
    iyakki_shared::hostfiles::set_enabled(false);
    goto(&mut a, &cx, Tab::HostAccess);
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(!g.hits.iter().any(|h| h.label == "Add folder…"), "no actions off window mode");
    assert!(SettingsApp::host_rows(&cx).is_empty());
    // window mode
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-hostacc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Music/Rock")).unwrap();
    std::fs::write(base.join("Music/Rock/a.mp3"), vec![0u8; 2048]).unwrap();
    iyakki_shared::hostfiles::set_enabled(true);
    iyakki_shared::hostfiles::set_host_home(Some(base.clone()));
    let g = draw(&mut a, &cx, 1280, 960);
    for want in ["Add folder…", "Options…", "Change mode", "Convert to copy", "Unlink", "Import files…", "Audit trail"] {
        assert!(g.hits.iter().any(|h| h.label == want), "{want}");
    }
    let mut m = iyakki_shared::hostfiles::HostMounts::load(cx.data_dir(), &cx.session.user);
    m.add(&base, "~/Music", None, false).unwrap();
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(g.hits.iter().any(|h| h.label == "host:Music" && matches!(h.ev, Event::Row(0))), "the folder is a clickable row");
    // change mode (audited), open the options, and the audit trail
    key(&mut a, &cx, Key::Char('m'));
    assert!(SettingsApp::host_rows(&cx)[0].0.write, "now read-write");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("host.mount.mode")));
    key(&mut a, &cx, Key::Char('m'));
    assert!(!SettingsApp::host_rows(&cx)[0].0.write);
    // the first use explains itself; once agreed the options open
    key(&mut a, &cx, Key::Char('o'));
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(g.hits.iter().any(|h| h.label == "I understand, continue"), "the explanation comes first");
    key(&mut a, &cx, Key::Esc);
    std::fs::write(cx.data_dir().join(format!("host-consent-{}", cx.session.user)), "agreed\n").unwrap();
    key(&mut a, &cx, Key::Char('o'));
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(g.hits.iter().any(|h| h.label.contains("Stop sharing")) && g.hits.iter().any(|h| h.label.contains("Convert to a copy")), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('l'));
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(!g.hits.is_empty());
    // unplugged: listed with its status
    std::fs::rename(base.join("Music"), base.join("Music-away")).unwrap();
    let rows = SettingsApp::host_rows(&cx);
    assert_eq!((rows.len(), rows[0].1), (1, false), "listed, not available");
    key(&mut a, &cx, Key::Char('l'));
    let _ = draw(&mut a, &cx, 1280, 960);
    iyakki_shared::hostfiles::set_host_home(None);
    iyakki_shared::hostfiles::set_enabled(false);
}

#[test]
fn rag_tuning_external_backends_ingest_and_reindex() {
    let (mut cx, d) = admin("set7");
    let docs = d.join("docs");
    std::fs::create_dir_all(docs.join("shared")).unwrap();
    std::fs::write(docs.join("shared/a.md"), "Deploy keys live in the vault under prod.").unwrap();
    cx.cfg.rag_docs = docs.display().to_string();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Rag);
    key(&mut a, &cx, Key::Char(']'));
    key(&mut a, &cx, Key::Char('='));
    key(&mut a, &cx, Key::Char('.'));
    key(&mut a, &cx, Key::Char('u'));
    let r = cx.settings().lock().unwrap().get().rag.clone();
    assert_eq!((r.top_k, r.chunk_max, r.overlap, r.use_external), (6, 800, 120, false));
    key(&mut a, &cx, Key::Char('u'));
    // add a generic HTTP backend
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "corp");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Right); // → HTTP
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "http://rag.corp/search");
    for _ in 0..3 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(!a.has_modal(), "{:?}", a.status());
    assert_eq!(cx.settings().lock().unwrap().get().external_rag.len(), 1);
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("endpoint responded"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('x'));
    assert!(!cx.settings().lock().unwrap().get().external_rag[0].enabled);
    // ingest the shared folder, then reindex
    key(&mut a, &cx, Key::Char('i'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("1 new document"), "{}", status(&a));
    assert_eq!(cx.kb().stats("shared").unwrap().docs, 1);
    key(&mut a, &cx, Key::Char('i'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("0 new document"), "idempotent: {}", status(&a));
    key(&mut a, &cx, Key::Char('r'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("Re-embedded"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(cx.settings().lock().unwrap().get().external_rag.is_empty());
    let mut g = Gfx::new(1100, 650);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
}

#[test]
fn voice_tab_visibility_follows_the_role() {
    let (cx, _) = admin("sv0");
    assert!(SettingsApp::new(&cx).tabs().contains(&Tab::Voice));
    let ucx = user("sv0u");
    assert!(SettingsApp::new(&ucx).tabs().contains(&Tab::Voice), "users with voice may test their hardware");
    let mut gcx = user("sv0g");
    gcx.session.policy.voice = false;
    assert!(!SettingsApp::new(&gcx).tabs().contains(&Tab::Voice), "no voice permission, no tab");
}

#[test]
fn admin_configures_engines_and_the_key_goes_to_the_vault() {
    let (cx, _) = admin("sv1");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    // HTTP speech-to-text with an API key
    key(&mut a, &cx, Key::Char('s'));
    key(&mut a, &cx, Key::Enter); // type: HTTP (default) -> URL
    key(&mut a, &cx, Key::Ctrl('a'));
    typ(&mut a, &cx, "http://192.168.1.9:8080/v1/audio/transcriptions");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "whisper-large");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "sk-voice-secret");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "en");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{:?}", a.status());
    let v = cx.voice_settings();
    match v.stt.unwrap() {
        SpeechEngine::Http { url, model, key_ref, language, .. } => {
            assert_eq!((url.as_str(), model.as_str(), key_ref.as_deref(), language.as_deref()), ("http://192.168.1.9:8080/v1/audio/transcriptions", "whisper-large", Some("vault:stt-key"), Some("en")));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(&*cx.secrets().resolve("vault:stt-key", None).unwrap(), "sk-voice-secret");
    assert!(!std::fs::read_to_string(cx.data_dir().join("settings.json")).unwrap().contains("sk-voice-secret"));
    // local command text-to-speech
    key(&mut a, &cx, Key::Char('v'));
    key(&mut a, &cx, Key::Right); // Local command
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Ctrl('a'));
    typ(&mut a, &cx, "piper -m voice.onnx -f {out}");
    for _ in 0..4 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(!a.has_modal(), "{:?}", a.status());
    assert_eq!(cx.voice_settings().tts, Some(SpeechEngine::Command { command: "piper -m voice.onnx -f {out}".into() }));
    // invalid input keeps the dialog open
    key(&mut a, &cx, Key::Char('s'));
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Ctrl('a'));
    typ(&mut a, &cx, "ftp://nope");
    for _ in 0..4 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(a.has_modal(), "bad URL is rejected");
    key(&mut a, &cx, Key::Esc);
    // toggles and clearing
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('r'));
    let v = cx.voice_settings();
    assert!(v.auto_send && v.speak_replies);
    key(&mut a, &cx, Key::Char('S'));
    key(&mut a, &cx, Key::Char('V'));
    let v = cx.voice_settings();
    assert!(v.stt.is_none() && v.tts.is_none());
    let mut g = Gfx::new(1280, 700);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
}

#[test]
fn hardware_tests_report_what_they_find() {
    let _route = iyakki_agent::mixer::pin_command_route();
    let (cx, d) = admin("sv2");
    // the test tone goes through the configured play command; the mixer (on by default) would take it instead
    cx.services.update_sound(|s| s.mixer = false).unwrap();
    iyakki_agent::mixer::set_enabled(false); // the process-wide switch the players read
    let dir = d.join("audio");
    std::fs::create_dir_all(&dir).unwrap();
    cx.settings().lock().unwrap().update(|s| s.voice = fake_audio(&dir, false)).unwrap();
    let mut a = SettingsApp::new(&cx);
    a.test_record_ms = 300;
    goto(&mut a, &cx, Tab::Voice);
    key(&mut a, &cx, Key::Char('m'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("Microphone works") && status(&a).contains("peak level"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('p'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("test tone"), "{}", status(&a));
    assert!(std::fs::read(dir.join("played.wav")).unwrap().starts_with(b"RIFF"), "a real WAV reached the speaker command");
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert_eq!(status(&a), "Heard: “hello from the test”");
    let _ = std::fs::remove_file(dir.join("played.wav"));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("Synthesised"), "{}", status(&a));
    assert!(dir.join("played.wav").exists());
    // a silent microphone is reported as silent
    cx.settings().lock().unwrap().update(|s| s.voice = fake_audio(&dir, true)).unwrap();
    key(&mut a, &cx, Key::Char('m'));
    settle(&mut a, &cx);
    assert_eq!(a.status().unwrap().1, false);
    assert!(status(&a).contains("silent"), "{}", status(&a));
    // a missing tool names the fix
    cx.settings().lock().unwrap().update(|s| s.voice.record_cmd = "/no/such/recorder".into()).unwrap();
    key(&mut a, &cx, Key::Char('m'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("cannot start") && status(&a).contains("Settings"), "{}", status(&a));
}

#[test]
fn users_can_test_but_not_change_voice_settings() {
    let cx = user("sv3");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    for k in ['s', 'v', 'c', 'a', 'r', 'S', 'V'] {
        key(&mut a, &cx, Key::Char(k));
        assert!(!a.has_modal(), "{k}");
    }
    assert!(status(&a).contains("administrators"));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
    // testing needs the voice permission (and is audited)
    key(&mut a, &cx, Key::Char('p'));
    assert!(a.is_busy() || status(&a).contains("Played") || a.status().is_some());
    settle(&mut a, &cx);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("voice.test")));
    let mut no_voice = user("sv3b");
    no_voice.session.policy.voice = false;
    let mut b = SettingsApp::new(&no_voice);
    b.tab = 0;
    b.tabs.push(Tab::Voice); // even if the tab were reached, the tests refuse
    b.tab = b.tabs.len() - 1;
    key(&mut b, &no_voice, Key::Char('m'));
    assert!(!b.is_busy() && status(&b).contains("not enabled"));
    assert!(no_voice.audit.recent(10).iter().any(|l| l.contains("voice.denied")));
}

/// The voice test plays through the stand-in player, and no test in this binary leaves a real `aplay` child of the test process (stand-in scripts of other tests do not count).
#[test]
fn the_voice_test_never_opens_the_real_sound_device() {
    let aplay_children = || -> Vec<u32> {
        let me = std::process::id();
        std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| {
            let pid = e.file_name().to_string_lossy().parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse::<u32>().ok()?;
            let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
            // Other tests in this binary run stand-in players: shell scripts named `aplay`, whose process name is also "aplay" but
            // whose program file is the shell. Only a process running the real `aplay` program counts.
            let exe = std::fs::read_link(e.path().join("exe")).ok()?;
            (ppid == me && comm.trim() == "aplay" && exe.file_name().is_some_and(|n| n == "aplay")).then_some(pid)
        }).collect()
    };
    let cx = user("sv-noaplay");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    key(&mut a, &cx, Key::Char('p'));
    settle(&mut a, &cx);
    assert!(aplay_children().is_empty(), "a test started the real aplay: {:?}", aplay_children());
}

#[test]
fn users_keep_personal_skills_from_clipboard_form_and_file() {
    let _clip = iyakki_testkit::process_lock(iyakki_testkit::names::CLIPBOARD); // the clipboard is one per process
    let cx = user("skills-user");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Skills);
    // from the clipboard (newlines preserved)
    crate::ui::clipboard::copy(SKILL_MD);
    key(&mut a, &cx, Key::Char('c'));
    assert!(status(&a).contains("Saved mine skill `meeting-notes`"), "{}", status(&a));
    let l = cx.skills().list("amy");
    assert_eq!((l.len(), l[0].body.as_str()), (1, "List decisions first."));
    assert_eq!(a.list_len(&cx), 1);
    // quick-create form (a user has no Scope choice beyond "Mine")
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "code-review");
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "Review a diff for bugs");
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "review, diff");
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "Look for bugs before style.");
    // Ctrl+S submits
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(!a.has_modal(), "{}", status(&a));
    assert!(cx.skills().get(&iyakki_shared::skills::Scope::User("amy".into()), "code-review").is_some());
    // import from a file inside the user's read scope; outside it is refused
    let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-skill-import-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("tone.md"), SKILL_MD.replace("meeting-notes", "tone")).unwrap();
    let mut cx2 = cx.clone();
    cx2.session.policy.fs_read = vec![home.display().to_string()];
    let mut b = SettingsApp::new(&cx2);
    goto(&mut b, &cx2, Tab::Skills);
    key(&mut b, &cx2, Key::Char('i'));
    typ(&mut b, &cx2, &home.join("tone.md").display().to_string());
    b.handle(&Event::Key(Key::Ctrl('s')), &cx2);
    assert!(cx2.services.skills().get(&iyakki_shared::skills::Scope::User("amy".into()), "tone").is_some(), "{}", status(&b));
    key(&mut b, &cx2, Key::Char('i'));
    typ(&mut b, &cx2, "/etc/passwd");
    b.handle(&Event::Key(Key::Ctrl('s')), &cx2);
    assert!(b.has_modal(), "a refused import keeps the form open");
    assert!(cx2.audit.recent(20).iter().any(|l| l.contains("skills.add")));
}

#[test]
fn shared_skills_need_the_permission_and_deleting_asks_first() {
    let _clip = iyakki_testkit::process_lock(iyakki_testkit::names::CLIPBOARD); // the clipboard is one per process
    let cx = user("skills-shared");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Skills);
    crate::ui::clipboard::copy(SKILL_MD);
    key(&mut a, &cx, Key::Char('C'));
    assert!(cx.skills().list_scope(&iyakki_shared::skills::Scope::Shared).is_empty(), "plain users cannot add shared skills");
    assert!(!status(&a).starts_with("Saved") && a.status().unwrap().1 == false);
    // a forged form that asks for the shared scope is refused too
    assert!(SettingsApp::add_skill(&mut a, &cx, SKILL_MD, true).is_err());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.denied")));
    // operators can; users then see it but cannot delete it
    let mut op = cx.clone();
    op.session.role = Role::Operator;
    op.session.policy = Role::Operator.template();
    let mut o = SettingsApp::new(&op);
    goto(&mut o, &op, Tab::Skills);
    crate::ui::clipboard::copy(SKILL_MD);
    key(&mut o, &op, Key::Char('C'));
    assert_eq!(cx.skills().list_scope(&iyakki_shared::skills::Scope::Shared).len(), 1, "{}", status(&o));
    let mut a2 = SettingsApp::new(&cx);
    goto(&mut a2, &cx, Tab::Skills);
    assert_eq!(a2.list_len(&cx), 1);
    key(&mut a2, &cx, Key::Char('d'));
    assert!(!a2.has_modal() && status(&a2).contains("Only operators"));
    // the operator confirms before deleting
    key(&mut o, &op, Key::Char('d'));
    assert!(o.has_modal());
    key(&mut o, &op, Key::Char('n'));
    assert_eq!(cx.skills().list_scope(&iyakki_shared::skills::Scope::Shared).len(), 1, "declined");
    key(&mut o, &op, Key::Char('d'));
    key(&mut o, &op, Key::Char('y'));
    assert!(cx.skills().list_scope(&iyakki_shared::skills::Scope::Shared).is_empty());
}

#[test]
fn skills_tab_follows_the_permission_and_renders() {
    let mut cx = user("skills-vis");
    cx.session.policy.skills = false;
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Skills));
    let cx = user("skills-vis2");
    cx.skills().add(iyakki_shared::skills::Scope::User("amy".into()), SKILL_MD, false, true).unwrap();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Skills);
    let mut g = Gfx::new(1100, 650);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
    assert!(g.hits.iter().any(|h| h.label == "meeting-notes"), "the skill row is clickable");
}

#[test]
fn admin_sets_the_token_price_and_bad_values_are_rejected() {
    let (cx, _) = admin("set-price");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('p'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "250");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.settings().lock().unwrap().get().token_price_cents_per_mtok, 250);
    assert!(cx.audit.recent(5).iter().any(|l| l.contains("settings.token_price") && l.contains("250")));
    key(&mut a, &cx, Key::Char('p'));
    for _ in 0..3 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "abc");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a non-number keeps the form open");
    assert_eq!(cx.settings().lock().unwrap().get().token_price_cents_per_mtok, 250);
    key(&mut a, &cx, Key::Esc);
    // users cannot
    let ucx = user("set-price-u");
    let mut u = SettingsApp::new(&ucx).on(Tab::Models);
    key(&mut u, &ucx, Key::Char('p'));
    assert!(!u.has_modal());
    assert_eq!(ucx.services.settings().lock().unwrap().get().token_price_cents_per_mtok, 0);
}

#[test]
fn admin_switches_tool_routing_off_and_on_and_users_cannot() {
    let (cx, _) = admin("tool-routing");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    assert!(!cx.settings().lock().unwrap().get().no_tool_routing, "on by default");
    key(&mut a, &cx, Key::Char('T'));
    assert!(cx.settings().lock().unwrap().get().no_tool_routing);
    assert!(cx.audit.recent(5).iter().any(|l| l.contains("settings.tool_routing") && l.contains("off")));
    key(&mut a, &cx, Key::Char('T'));
    assert!(!cx.settings().lock().unwrap().get().no_tool_routing);
    let ucx = user("tool-routing-u");
    let mut u = SettingsApp::new(&ucx).on(Tab::Models);
    key(&mut u, &ucx, Key::Char('T'));
    assert!(!ucx.services.settings().lock().unwrap().get().no_tool_routing);
}

#[test]
fn the_voice_page_finds_engines_with_no_setup_and_says_why_others_are_unavailable() {
    let (cx, _) = admin("sv-host1");
    let d = eng_dir("sv-host1-eng");
    host_fakes(&d, &["espeak-ng", "spd-say", "piper"]);
    cx.host_tts().configure(host_env(&d));
    let report = cx.host_tts().scan_now();
    assert_eq!(cx.voice_settings().tts, None, "nothing was set");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    match a.vo_engine(&cx) {
        Some(SpeechEngine::Host { engine, .. }) => assert_eq!(engine, "espeak-ng", "the best usable engine is used: piper has no voice file"),
        other => panic!("{other:?}"),
    }
    let lines = vo_engine_lines(&report, "ta-IN");
    let text: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    assert!(text.iter().any(|l| l.starts_with("● espeak-ng") && l.contains("speaks Tamil") && l.contains("can save audio")), "{text:?}");
    assert!(text.iter().any(|l| l.starts_with("● speech-dispatcher") && l.contains("play only: Save audio is unavailable")), "{text:?}");
    assert!(text.iter().any(|l| l.starts_with("○ piper") && l.contains("no voice file was found")), "missing voice files are named: {text:?}");
    assert!(text.iter().any(|l| l.starts_with("○ festival") && l.contains("not found") && l.contains("install festival")), "a missing program says how to get it: {text:?}");
    assert!(vo_engine_lines(&report, "te-IN").iter().any(|l| l.0.starts_with("● espeak-ng") && l.0.contains("no Telugu voice")), "no voice for the language is said");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, w, h);
        let what = format!("voice page at {w}x{h}");
        let mine = |l: &str| ["Test voice", "Re-check engines", "Open voice helper in your browser", "Engine: ", "Voice: ", "Language: ", "Speed: "].iter().any(|p| l.starts_with(p));
        for hit in g.hits.iter().filter(|h| mine(&h.label)) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{what}: {:?} is outside the window", (&hit.label, r.x, r.y, r.w, r.h));
        }
        for needed in ["Test voice", "Re-check engines", "Open voice helper in your browser"] {
            assert!(g.hits.iter().any(|h| h.label == needed), "{what}: {needed} is there: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        for prefix in ["Engine: ", "Voice: ", "Language: ", "Speed: "] {
            assert!(g.hits.iter().any(|h| h.label.starts_with(prefix)), "{what}: the {prefix} chooser is there");
        }
        // the controls are above the bottom button bar and do not sit on each other
        let btns: Vec<&crate::ui::Hit> = g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char(c)) if "eglfkoy".contains(c)) && ["Test voice", "Re-check engines", "Open voice helper in your browser"].iter().chain(["Engine: ", "Voice: ", "Language: ", "Speed: "].iter()).any(|p| h.label.starts_with(p))).collect();
        for (i, x) in btns.iter().enumerate() {
            for y in &btns[i + 1..] {
                let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} and {:?} overlap", x.label, y.label);
            }
        }
        // and none sits on the bottom button bar
        for x in &btns {
            for y in g.hits.iter().filter(|y| matches!(y.ev, Event::Key(Key::Char(_))) && !mine(&y.label)) {
                let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} sits on the bar button {:?}", x.label, y.label);
            }
        }
        // clicking each control does what its key does
        let test = g.hits.iter().find(|h| h.label == "Test voice").unwrap();
        assert_eq!(test.ev, Event::Key(Key::Char('y')));
    }
}

#[test]
fn the_highlight_choice_cycles_sentence_and_word_sentence_only_off_and_is_saved() {
    use iyakki_shared::voice::Highlight;
    let (cx, _) = admin("sv-highlight");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    assert_eq!(cx.voice_settings().highlight, Highlight::SentenceAndWord, "sentence + word is the default");
    key(&mut a, &cx, Key::Char('h'));
    assert_eq!(cx.voice_settings().highlight, Highlight::Sentence);
    assert!(status(&a).contains("sentence only"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('h'));
    assert_eq!(cx.voice_settings().highlight, Highlight::Off);
    key(&mut a, &cx, Key::Char('h'));
    assert_eq!(cx.voice_settings().highlight, Highlight::SentenceAndWord);
    // an old settings file without the field loads with the default
    let v: iyakki_shared::voice::VoiceSettings = serde_json::from_str("{\"speed_pct\": 120}").unwrap();
    assert_eq!(v.highlight, Highlight::SentenceAndWord);
    assert_eq!(serde_json::from_str::<iyakki_shared::voice::VoiceSettings>("{\"highlight\":\"sentence\"}").unwrap().highlight, Highlight::Sentence);
    // word timing from the sound is on by default (also for an old settings file) and toggles with w
    assert!(v.align_words && v.remote_ok.is_empty());
    key(&mut a, &cx, Key::Char('w'));
    assert!(!cx.voice_settings().align_words && status(&a).contains("estimated"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('w'));
    assert!(cx.voice_settings().align_words);
}

#[test]
fn test_voice_speaks_a_sentence_in_the_chosen_language_with_the_chosen_voice_and_speed() {
    let (cx, _) = admin("sv-host2");
    let d = eng_dir("sv-host2-eng");
    host_fakes(&d, &["espeak-ng"]);
    cx.host_tts().configure(host_env(&d));
    cx.host_tts().scan_now();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    // English first
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("Spoke a sentence in English with espeak-ng"), "{}", status(&a));
    let said = std::fs::read_to_string(d.join("said.log")).unwrap();
    assert!(said.contains("This is the IYAKKI voice test."), "{said}");
    // Tamil: the Tamil voice, and a Tamil sentence
    key(&mut a, &cx, Key::Char('l'));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("in Tamil") && status(&a).contains("voice ta"), "{}", status(&a));
    let args = std::fs::read_to_string(d.join("args.log")).unwrap();
    assert!(args.lines().last().unwrap().contains("-v ta "), "{args}");
    assert!(std::fs::read_to_string(d.join("said.log")).unwrap().contains("வணக்கம்"), "the sentence is in Tamil");
    // speed: 125 percent is 218 words per minute
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(cx.voice_settings().speed_pct, 125);
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(std::fs::read_to_string(d.join("args.log")).unwrap().lines().last().unwrap().contains("-s 219"), "{}", std::fs::read_to_string(d.join("args.log")).unwrap());
    // a language without a voice is said, and the default voice is used
    key(&mut a, &cx, Key::Char('l')); // Hindi
    key(&mut a, &cx, Key::Char('l')); // Telugu
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("no Telugu voice") && status(&a).contains("default voice"), "{}", status(&a));
    // a voice chosen by name is kept for every language
    key(&mut a, &cx, Key::Char('g'));
    match cx.voice_settings().tts {
        Some(SpeechEngine::Host { engine, voice: Some(v), .. }) => assert_eq!((engine.as_str(), v.as_str()), ("espeak-ng", "en-us")),
        other => panic!("{other:?}"),
    }
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("voice.test") && log.contains("settings.voice.tts") && log.contains("settings.voice.speed"), "{log}");
}

#[test]
fn engine_choice_cycles_through_automatic_the_found_engines_and_the_browser_and_is_for_admins() {
    let (cx, _) = admin("sv-host3");
    let d = eng_dir("sv-host3-eng");
    host_fakes(&d, &["espeak-ng", "spd-say"]);
    cx.host_tts().configure(host_env(&d));
    cx.host_tts().scan_now();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    let hv = HostVoice::current();
    hv.connect(iyakki_shared::hostvoice::Front::Browser);
    hv.event(iyakki_shared::hostvoice::FromClient::Voices { supported: true, voices: vec![] });
    let id = |cx: &AppCx| match cx.voice_settings().tts {
        Some(SpeechEngine::Host { engine, .. }) => engine,
        Some(SpeechEngine::Browser) => "browser".to_string(),
        Some(other) => other.label(),
        None => "automatic".to_string(),
    };
    let mut seen = vec![id(&cx)];
    for _ in 0..4 {
        key(&mut a, &cx, Key::Char('e'));
        seen.push(id(&cx));
    }
    assert_eq!(seen, ["automatic", "espeak-ng", "spd-say", "browser", "automatic"], "the browser voice is the last choice before automatic");
    // with a configured engine, it wins over what was found
    let own = SpeechEngine::Command { command: "sh -c 'cat > /dev/null'".into() };
    cx.settings().lock().unwrap().update(|s| s.voice.tts = Some(own.clone())).unwrap();
    assert_eq!(a.vo_engine(&cx), Some(own));
    // the default order when nothing is set: engine of this computer, then the browser voice, then nothing
    cx.settings().lock().unwrap().update(|s| s.voice.tts = None).unwrap();
    assert!(matches!(a.vo_engine(&cx), Some(SpeechEngine::Host { .. })));
    std::fs::remove_file(d.join("espeak-ng")).unwrap();
    std::fs::remove_file(d.join("spd-say")).unwrap();
    cx.host_tts().scan_now();
    assert_eq!(a.vo_engine(&cx), Some(SpeechEngine::Browser), "no engine here, the screen can speak");
    hv.disconnect();
    assert_eq!(a.vo_engine(&cx), None);
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("No speech engine is ready") && status(&a).contains("voice helper"), "{}", status(&a));
    // people who are not administrators can test but not change
    let ucx = user("sv-host3u");
    ucx.services.host_tts().configure(host_env(&d));
    let mut u = SettingsApp::new(&ucx);
    goto(&mut u, &ucx, Tab::Voice);
    key(&mut u, &ucx, Key::Char('e'));
    assert!(status(&u).contains("Only administrators"), "{}", status(&u));
    key(&mut u, &ucx, Key::Char('f'));
    assert_eq!(ucx.services.voice_settings().speed_pct, 100);
}

#[test]
fn re_check_finds_an_engine_installed_meanwhile() {
    let (cx, _) = admin("sv-host4");
    let d = eng_dir("sv-host4-eng");
    cx.host_tts().configure(host_env(&d));
    cx.host_tts().scan_now();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    assert_eq!(a.vo_engine(&cx), None);
    host_fakes(&d, &["espeak-ng"]);
    key(&mut a, &cx, Key::Char('k'));
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if !cx.host_tts().scanning() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(a.vo_engine(&cx), Some(SpeechEngine::Host { ref engine, .. }) if engine == "espeak-ng"));
    assert!(std::fs::read_to_string(cx.audit.path_buf()).unwrap().contains("voice.recheck"));
}

#[test]
fn the_browser_voice_helper_is_opened_and_disconnected_from_the_voice_page() {
    let (cx, _) = admin("sv-host5");
    let d = eng_dir("sv-host5-eng");
    let opener = d.join("fakeopen");
    std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > {}/opened\n", d.display())).unwrap();
    std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    cx.host_tts().configure(host_env(&d));
    cx.host_tts().set_opener(Some(vec![opener.to_string_lossy().to_string()]));
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Voice);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Open voice helper in your browser"));
    key(&mut a, &cx, Key::Char('o'));
    assert!(status(&a).contains("Opening the voice helper"), "{}", status(&a));
    assert_eq!(cx.host_tts().helper_state(), Some(iyakki_shared::voicehelper::HelperState::Waiting));
    let url = (0..200).find_map(|_| { std::thread::sleep(std::time::Duration::from_millis(20)); std::fs::read_to_string(d.join("opened")).ok().filter(|u| !u.is_empty()) }).expect("the browser was asked to open it");
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Disconnect voice helper"), "the button turns into Disconnect");
    // a second press disconnects: the listener is closed
    key(&mut a, &cx, Key::Char('o'));
    assert!(status(&a).contains("disconnected"), "{}", status(&a));
    assert_eq!(cx.host_tts().helper_state(), None);
    let addr = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
    let closed = (0..200).any(|_| { std::thread::sleep(std::time::Duration::from_millis(20)); std::net::TcpStream::connect(&addr).is_err() });
    assert!(closed, "the helper's port is closed");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("voice.helper"), "{log}");
    // without the voice permission it is refused
    let mut gcx = user("sv-host5g");
    gcx.session.policy.voice = false;
    gcx.services.host_tts().configure(host_env(&d));
    let mut g2 = SettingsApp::new(&gcx);
    g2.tab = 0;
    g2.vo_helper_toggle(&gcx);
    assert!(status(&g2).contains("not enabled"), "{}", status(&g2));
    assert_eq!(gcx.services.host_tts().helper_state(), None);
}

#[test]
fn settings_opens_on_the_voice_page_when_the_story_asks() {
    let (cx, _) = admin("sv-host6");
    let mut a = SettingsApp::new(&cx);
    assert_ne!(a.tab(), Tab::Voice);
    a.open_item("voice", &cx);
    assert_eq!(a.tab(), Tab::Voice);
}

#[test]
fn a_click_on_a_rag_slider_sets_that_value_through_the_settings_checks_and_is_audited() {
    let (cx, _) = admin("sld1");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Rag);
    let _ = draw(&mut a, &cx, 1280, 720);
    let w = |id: usize| a.slider_w.iter().find(|(i, _)| *i == id).map(|(_, w)| *w).expect("slider drawn");
    let (wc, wo, wt) = (w(SLIDER_CHUNK), w(SLIDER_OVERLAP), w(SLIDER_TOPK));
    let rag = |cx: &AppCx| cx.settings().lock().unwrap().get().rag.clone();
    a.handle(&Event::AreaAt(SLIDER_TOPK, wt * 0.5, 5.0), &cx);
    assert_eq!(rag(&cx).top_k, 26, "halfway along 1..50");
    a.handle(&Event::AreaAt(SLIDER_CHUNK, wc, 5.0), &cx);
    assert_eq!(rag(&cx).chunk_max, 4000, "the far end");
    a.handle(&Event::AreaAt(SLIDER_CHUNK, 0.0, 5.0), &cx);
    assert_eq!(rag(&cx).chunk_max, 100, "the near end");
    // overlap must stay under half the chunk size: with chunk 100 the slider's range is 0..49, and an overlap above
    // that is refused by the settings checks, never saved
    a.handle(&Event::AreaAt(SLIDER_OVERLAP, wo, 5.0), &cx);
    let r = rag(&cx);
    assert!(r.overlap < r.chunk_max / 2, "{r:?}");
    a.handle(&Event::AreaAt(SLIDER_CHUNK, wc * 0.5, 5.0), &cx);
    a.handle(&Event::AreaAt(SLIDER_OVERLAP, wo * 0.5, 5.0), &cx);
    let r = rag(&cx);
    assert_eq!((r.chunk_max, r.overlap % 20), (2100, 0), "values snap to their step: {r:?}");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.rag")));
    // the buttons on the card do what they always did
    let before = rag(&cx).top_k;
    key(&mut a, &cx, Key::Char(']'));
    assert_eq!(rag(&cx).top_k, before + 1);
    // a click on a switch's key toggles external search
    let ext = rag(&cx).use_external;
    key(&mut a, &cx, Key::Char('u'));
    assert_eq!(rag(&cx).use_external, !ext);
}
