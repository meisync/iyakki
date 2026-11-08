//! Voice: Whisper tiny + Kokoro with fakes only: no cluster, no network, no model, no microphone, no speaker. The step runs against a
//! scripted [`StepEnv`], the self-test against a fake [`Upstream`], the settings against a real [`Services`] in a temporary folder.

use super::*;
use crate::lemonade::{Model, ModelType};
use crate::quickinstall::{order, presence_with, Group, Presence};
use iyakki_proxy::UpstreamResponse;
use std::sync::Mutex;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-vp-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn services(t: &Tmp) -> Arc<crate::services::Services> {
    let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(2)));
    crate::services::Services::open(&t.0, up.clone(), up).unwrap()
}

/// A scripted step environment: Lemonade answers, the models it has are listed, and each call is written down.
#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    have: Vec<String>,
    fail_pull: Option<String>,
    selftest: Option<Result<String, String>>,
    fail_delete: Vec<String>,
    list_fails: bool,
    lemonade_down: bool,
    /// A pull answers OK but Lemonade does not list the model afterwards.
    pulled_not_listed: bool,
}

impl StepEnv for Fake {
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, _dep: Item) -> crate::quickinstall::DepState {
        crate::quickinstall::DepState::Ready
    }
    fn k3s_present(&mut self) -> bool {
        true
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Err("not here".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        true
    }
    fn install_cli(&mut self, _id: &str) -> Result<String, String> {
        Err("not here".into())
    }
    fn kubectl_apply(&mut self, name: &str, _m: &str) -> Result<String, String> {
        self.calls.push(format!("apply {name}"));
        Ok(String::new())
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        Ok(())
    }
    fn sleep(&mut self, _d: Duration) {}
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.calls.push(format!("mark {} {detail}", item.id()));
        Ok(())
    }
    fn request_after(&mut self, what: &str) -> Result<(), String> {
        self.calls.push(format!("after {what}"));
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        !self.lemonade_down
    }
    fn lemonade_pull(&mut self, model: &str) -> Result<(), String> {
        self.calls.push(format!("pull {model}"));
        match &self.fail_pull {
            Some(m) if m == model => Err("the download stopped at 40%".into()),
            _ => {
                // a pulled model is listed afterwards (the step looks again before it marks)
                if !self.pulled_not_listed && !self.have.iter().any(|m| m == model) {
                    self.have.push(model.to_string());
                }
                Ok(())
            }
        }
    }
    fn lemonade_models(&mut self) -> Result<Vec<String>, String> {
        if self.list_fails { Err("Lemonade does not answer".into()) } else { Ok(self.have.clone()) }
    }
    fn lemonade_delete(&mut self, model: &str) -> Result<(), String> {
        self.calls.push(format!("delete {model}"));
        if self.fail_delete.iter().any(|m| m == model) { Err("HTTP 500".into()) } else { Ok(()) }
    }
    fn voice_selftest(&mut self) -> Result<String, String> {
        self.calls.push("selftest".into());
        self.selftest.clone().unwrap_or_else(|| Ok("Self-test passed: fake".into()))
    }
    fn record_pulled(&mut self, models: &[String]) -> Result<(), String> {
        self.calls.push(format!("record {}", models.join(",")));
        Ok(())
    }
}

const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(5);

#[test]
fn the_review_lists_the_downloads_where_it_runs_the_licence_boundary_and_the_espeak_ng_note() {
    for there in [true, false] {
        let lines = review_lines(there);
        assert!(review_has_required(&lines), "{there}");
        let t = lines.join("\n");
        // what is downloaded, with the numbers the code supports, marked as estimates
        assert!(t.contains("Whisper-Tiny") && t.contains("about 75 MB") && t.contains("kokoro-v1") && t.contains("about 330 MB") && t.contains("Together about 405 MB"), "{t}");
        assert!(t.contains("estimates") && t.contains("(estimate"), "sizes and memory are marked as estimates");
        // where it runs and what it needs
        assert!(t.contains("Through this machine's Lemonade in k3s (namespace iyakki-ai), on the CPU, on 127.0.0.1:13305 only"));
        assert!(t.contains("about 8 GB of memory"));
        assert!(t.contains("another cluster active, Setup refuses"));
        // languages
        assert!(t.contains("Kokoro reads English only") && t.contains("Whisper tiny understands many languages"));
        // settings, self-test, Undo
        assert!(t.contains("speech to text = Whisper-Tiny, reading voice = kokoro-v1") && t.contains("Remove puts the old ones back") && t.contains("plain pass or fail"));
        // the boundary: integration code only, the models keep their own licences
        assert!(t.contains("IYAKKI provides integration code only") && t.contains("keep their own licences") && t.contains("never redistributes them"));
        assert!(!t.contains("Apache License") && !t.contains("Permission is hereby granted"), "no licence text is shown as if it covered the models");
        assert!(t.contains("espeak-ng (GPL-3.0-or-later)") && t.contains("separate program") && t.contains("never links it and never ships it"));
        assert!(t.contains("Not shipped with IYAKKI; downloaded at your click"));
    }
    // Lemonade there or not changes only the line about it
    let (a, b) = (review_lines(true).join("\n"), review_lines(false).join("\n"));
    assert!(a.contains("nothing else is downloaded") && b.contains("it is installed first, with k3s"));
    // the card's one-line licence says the same
    let l = Item::VoicePack.licence();
    assert!(l.contains("integration code only") && l.contains("own licences") && l.contains("not shipped with IYAKKI; downloaded at your click"), "{l}");
}

#[test]
fn the_item_is_a_voice_item_that_needs_lemonade_in_the_usual_order_and_is_never_installed_by_the_image() {
    assert_eq!(Item::parse("voice-pack"), Some(Item::VoicePack));
    assert_eq!(Item::VoicePack.group(), Group::Voice);
    assert_eq!(crate::quickinstall::items_of(Group::Voice), [Item::Kokoro, Item::VoicePack]);
    assert_eq!(Item::VoicePack.needs(), &[Item::Lemonade]);
    assert_eq!(order(Item::VoicePack, &|_| false), [Item::K3s, Item::Lemonade, Item::VoicePack]);
    assert_eq!(order(Item::VoicePack, &|i| i == Item::K3s), [Item::Lemonade, Item::VoicePack]);
    assert_eq!(order(Item::VoicePack, &|i| matches!(i, Item::K3s | Item::Lemonade)), [Item::VoicePack]);
    assert_eq!(order(Item::VoicePack, &|i| matches!(i, Item::K3s | Item::Lemonade | Item::VoicePack)), [Item::VoicePack], "installing again repairs it");
    assert_eq!(Item::VoicePack.download_mb(), WHISPER_MB + KOKORO_MB);
    assert!(Item::VoicePack.runs_as_job() && !Item::VoicePack.is_hardware() && Item::VoicePack.min_ram_mb() >= 7500);
    assert!(Item::VoicePack.title().contains("Whisper tiny") && Item::VoicePack.title().contains("Kokoro"));
    assert!(Item::VoicePack.about().contains("English only") && Item::VoicePack.about().contains("Remove"));
    // a failed Lemonade stops the pack without waiting
    let mut f = Fake { lemonade_down: true, ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, Duration::from_secs(20), POLL).unwrap_err();
    assert!(e.contains("did not answer") || e.contains("Lemonade"), "{e}");
    assert!(!f.calls.iter().any(|c| c.starts_with("pull")), "{:?}", f.calls);
}

#[test]
fn the_step_pulls_both_models_records_them_tests_marks_and_asks_for_the_settings_in_that_order() {
    let mut f = Fake::default();
    crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap();
    assert_eq!(f.calls, ["pull Whisper-Tiny", "pull kokoro-v1", "record Whisper-Tiny,kokoro-v1", "selftest", "mark voice-pack Whisper-Tiny and kokoro-v1 in Lemonade", "after voice-pack"]);
    assert!(f.lines.iter().any(|l| l.contains("Self-test passed")));
    assert!(f.lines[0].contains("integration code only") && f.lines[0].contains("not shipped with IYAKKI; downloaded at your click"), "the log opens with the licences");
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn running_it_again_downloads_nothing_twice_and_a_partly_there_pack_pulls_only_what_is_missing() {
    let mut f = Fake { have: vec![WHISPER_MODEL.into(), KOKORO_MODEL.into()], ..Default::default() };
    crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap();
    assert_eq!(f.calls, ["selftest", "mark voice-pack Whisper-Tiny and kokoro-v1 in Lemonade", "after voice-pack"], "no pull, nothing recorded as pulled, still tested and set");
    assert!(f.lines.iter().any(|l| l.contains("already: not downloaded again")));
    let mut f = Fake { have: vec![KOKORO_MODEL.into()], ..Default::default() };
    crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap();
    assert_eq!(f.calls[..2], ["pull Whisper-Tiny", "record Whisper-Tiny"], "Kokoro was there before: it is not ours to delete");
}

#[test]
fn a_failed_download_takes_back_what_this_run_pulled_and_never_what_was_there_before() {
    // Kokoro fails after Whisper arrived: both are taken back (a half-pulled model too), no marker, no settings request
    let mut f = Fake { fail_pull: Some(KOKORO_MODEL.into()), ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.contains("Pulling kokoro-v1 failed") && e.contains("stopped at 40%") && e.contains("nothing is half installed"), "{e}");
    assert_eq!(f.calls, ["pull Whisper-Tiny", "pull kokoro-v1", "delete kokoro-v1", "delete Whisper-Tiny"]);
    // Whisper was there already, Kokoro fails: only the half-pulled Kokoro is removed
    let mut f = Fake { have: vec![WHISPER_MODEL.into()], fail_pull: Some(KOKORO_MODEL.into()), ..Default::default() };
    crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert_eq!(f.calls, ["pull kokoro-v1", "delete kokoro-v1"]);
    // the first pull fails: nothing else was pulled
    let mut f = Fake { fail_pull: Some(WHISPER_MODEL.into()), ..Default::default() };
    crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert_eq!(f.calls, ["pull Whisper-Tiny", "delete Whisper-Tiny"]);
    // a model that cannot be removed again is named, with the way to do it
    let mut f = Fake { fail_pull: Some(KOKORO_MODEL.into()), fail_delete: vec![WHISPER_MODEL.into()], ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.contains("Could not remove Whisper-Tiny again") && e.contains("Settings \u{2192} Models") && !e.contains("nothing is half installed"), "{e}");
    // Lemonade's list cannot be read: nothing is touched
    let mut f = Fake { list_fails: true, ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.contains("Nothing was installed") && !f.calls.iter().any(|c| c.starts_with("pull") || c.starts_with("delete")), "{e} {:?}", f.calls);
}

#[test]
fn a_failed_self_test_is_a_plain_failure_that_removes_what_was_pulled_and_sets_nothing() {
    let mut f = Fake { selftest: Some(Err("Self-test failed: Whisper tiny heard \"banana\" (0 of 6 words matched)".into())), ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.starts_with("Self-test failed") && e.contains("banana") && e.contains("removed again"), "{e}");
    assert_eq!(f.calls, ["pull Whisper-Tiny", "pull kokoro-v1", "record Whisper-Tiny,kokoro-v1", "selftest", "delete kokoro-v1", "delete Whisper-Tiny"]);
    assert!(!f.calls.iter().any(|c| c.starts_with("mark") || c.starts_with("after")));
    // models that were there before stay, whatever the test says
    let mut f = Fake { have: vec![WHISPER_MODEL.into(), KOKORO_MODEL.into()], selftest: Some(Err("Self-test failed: x".into())), ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.contains("Nothing was installed") && !f.calls.iter().any(|c| c.starts_with("delete")), "{e} {:?}", f.calls);
}

#[test]
fn a_model_lemonade_does_not_list_after_the_pull_fails_the_step_takes_back_the_pull_and_writes_no_marker() {
    let mut f = Fake { pulled_not_listed: true, ..Default::default() };
    let e = crate::quickinstall::run_step(Item::VoicePack, &mut f, WAIT, POLL).unwrap_err();
    assert!(e.starts_with("Verify failed: Lemonade does not list Whisper-Tiny and kokoro-v1 as downloaded"), "{e}");
    assert!(f.calls.iter().any(|c| c == "delete kokoro-v1") && !f.calls.iter().any(|c| c.starts_with("mark") || c.starts_with("after")), "{:?}", f.calls);
}

/// A fake speech server: Kokoro answers with a tone, Whisper with this text (or an error).
struct Speech {
    heard: Result<String, u16>,
    speech: Vec<u8>,
    seen: Mutex<Vec<(String, String)>>,
}
impl Upstream for Speech {
    fn request(&self, _m: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((url.to_string(), String::from_utf8_lossy(body).chars().take(400).collect()));
        if url.ends_with("/audio/speech") {
            return Ok(UpstreamResponse { status: 200, body: self.speech.clone() });
        }
        match &self.heard {
            Ok(t) => Ok(UpstreamResponse { status: 200, body: serde_json::json!({ "text": t }).to_string().into_bytes() }),
            Err(s) => Ok(UpstreamResponse { status: *s, body: br#"{"error":{"message":"model not loaded"}}"#.to_vec() }),
        }
    }
}

fn speech(heard: Result<&str, u16>) -> Arc<Speech> {
    Arc::new(Speech { heard: heard.map(str::to_string), speech: crate::voice::encode_wav(&crate::voice::tone(440.0, 1500), 16_000), seen: Default::default() })
}

#[test]
fn the_self_test_speaks_with_kokoro_listens_with_whisper_tiny_and_says_pass_or_fail_in_plain_words() {
    let s = speech(Ok("This is the Yakki voice test. [BLANK_AUDIO]"));
    let ok = selftest("http://127.0.0.1:13305/api/v1", s.clone()).unwrap();
    assert!(ok.starts_with("Self-test passed") && ok.contains("1.5 s") && ok.contains("This is the Yakki voice test."), "{ok}");
    let seen = s.seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, "http://127.0.0.1:13305/api/v1/audio/speech");
    assert!(seen[0].1.contains("kokoro-v1") && seen[0].1.contains("This is the IYAKKI voice test") && seen[0].1.contains("af_sky"));
    assert_eq!(seen[1].0, "http://127.0.0.1:13305/api/v1/audio/transcriptions");
    assert!(seen[1].1.contains("Whisper-Tiny"), "{}", seen[1].1);
    // heard something else: fail with what it heard
    let e = selftest("http://127.0.0.1:13305/api/v1", speech(Ok("banana split"))).unwrap_err();
    assert!(e.starts_with("Self-test failed") && e.contains("banana split") && e.contains("0 of 6"), "{e}");
    // nothing heard
    let e = selftest("http://127.0.0.1:13305/api/v1", speech(Ok("[BLANK_AUDIO]"))).unwrap_err();
    assert!(e.contains("heard nothing"), "{e}");
    // Whisper cannot answer
    let e = selftest("http://127.0.0.1:13305/api/v1", speech(Err(500))).unwrap_err();
    assert!(e.starts_with("Self-test failed: Whisper tiny did not answer") && e.contains("model not loaded"), "{e}");
    // Kokoro returns something that is not sound
    let bad = Arc::new(Speech { heard: Ok("x".into()), speech: b"not audio at all, just text".to_vec(), seen: Default::default() });
    let e = selftest("http://127.0.0.1:13305/api/v1", bad).unwrap_err();
    assert!(e.starts_with("Self-test failed: Kokoro did not speak"), "{e}");
    // silence
    let silent = Arc::new(Speech { heard: Ok("x".into()), speech: crate::voice::encode_wav(&vec![0i16; 8000], 16_000), seen: Default::default() });
    assert!(selftest("http://127.0.0.1:13305/api/v1", silent).unwrap_err().contains("silent"));
    // the verdict is lenient about a made-up word but not about nonsense
    assert!(verdict(SELFTEST_SENTENCE, "this is the voice test", 1.0).is_ok());
    assert!(verdict(SELFTEST_SENTENCE, "voice", 1.0).is_err());
}

fn http_provider(svc: &crate::services::Services) -> String {
    svc.use_local_lemonade("http://127.0.0.1:13305/api/v1", None);
    svc.settings.lock().unwrap().get().providers.iter().find(|p| p.kind == crate::providers::Kind::Lemonade).unwrap().id.clone()
}

#[test]
fn the_settings_get_whisper_tiny_and_kokoro_through_the_existing_paths_and_the_old_engines_are_remembered() {
    let t = Tmp::new("settings");
    let svc = services(&t);
    let id = http_provider(&svc);
    let piper = SpeechEngine::Command { command: "piper --model v.onnx --output_file {out}".into() };
    svc.settings.lock().unwrap().update(|s| s.voice.tts = Some(piper.clone())).unwrap();
    let line = configure(&svc, &id).unwrap();
    assert!(line.contains("Speech-to-text: Whisper-Tiny") && line.contains("Reading voice: kokoro-v1"), "{line}");
    let v = svc.settings.lock().unwrap().get().voice.clone();
    assert_eq!(v.stt, Some(crate::lemonade::stt_engine("http://127.0.0.1:13305/api/v1", WHISPER_MODEL, None)));
    assert_eq!(v.tts, Some(crate::lemonade::tts_engine("http://127.0.0.1:13305/api/v1", KOKORO_MODEL, None, None)));
    let rec = read_record(svc.dir());
    assert!(rec.configured && rec.previous_stt.is_none() && rec.previous_tts == Some(piper.clone()));
    // again: nothing changes, and the engines that are ours now are not taken for "the old ones"
    configure(&svc, &id).unwrap();
    assert_eq!(read_record(svc.dir()).previous_tts, Some(piper));
    assert_eq!(svc.settings.lock().unwrap().get().voice, v);
    // an unknown provider is refused and changes nothing
    assert!(configure(&svc, "nope").is_err());
}

#[test]
fn remove_puts_the_old_engines_back_deletes_only_what_the_pack_pulled_and_forgets_it() {
    let t = Tmp::new("remove");
    let svc = services(&t);
    let id = http_provider(&svc);
    let old_stt = SpeechEngine::Command { command: "whisper-cli -m tiny.bin -f {wav}".into() };
    svc.settings.lock().unwrap().update(|s| s.voice.stt = Some(old_stt.clone())).unwrap();
    record_pulled(svc.dir(), &[KOKORO_MODEL.to_string()]).unwrap();
    record_pulled(svc.dir(), &[KOKORO_MODEL.to_string()]).unwrap();
    assert_eq!(read_record(svc.dir()).pulled, [KOKORO_MODEL], "never twice");
    configure(&svc, &id).unwrap();
    crate::quickinstall::mark_done(svc.dir(), Item::VoicePack, "x").unwrap();
    let deleted = Mutex::new(Vec::<String>::new());
    let line = remove(&svc, &|m: &str| {
        deleted.lock().unwrap().push(m.to_string());
        Ok(())
    })
    .unwrap();
    assert!(line.contains("Your earlier speech engines are back") && line.contains("Deleted kokoro-v1"), "{line}");
    assert_eq!(*deleted.lock().unwrap(), [KOKORO_MODEL], "Whisper-Tiny was there before: not deleted");
    let v = svc.settings.lock().unwrap().get().voice.clone();
    assert_eq!((v.stt, v.tts), (Some(old_stt), None));
    assert_eq!(crate::quickinstall::marked(svc.dir(), Item::VoicePack), None);
    assert_eq!(read_record(svc.dir()), Record::default());
}

#[test]
fn remove_keeps_a_change_made_since_and_a_model_that_could_not_be_deleted_stays_on_the_record() {
    let t = Tmp::new("remove2");
    let svc = services(&t);
    let id = http_provider(&svc);
    record_pulled(svc.dir(), &[WHISPER_MODEL.to_string(), KOKORO_MODEL.to_string()]).unwrap();
    configure(&svc, &id).unwrap();
    // the person chose their own reading voice afterwards: it is kept
    let mine = SpeechEngine::Command { command: "espeak-ng --stdout".into() };
    svc.settings.lock().unwrap().update(|s| s.voice.tts = Some(mine.clone())).unwrap();
    let e = remove(&svc, &|m: &str| if m == WHISPER_MODEL { Err("HTTP 500".into()) } else { Ok(()) }).unwrap_err();
    assert!(e.contains("One speech engine was put back") && e.contains("Could not delete Whisper-Tiny") && e.contains("Deleted kokoro-v1"), "{e}");
    let v = svc.settings.lock().unwrap().get().voice.clone();
    assert_eq!((v.stt, v.tts), (None, Some(mine)));
    assert_eq!(read_record(svc.dir()).pulled, [WHISPER_MODEL], "it stays listed, so the next Remove tries again");
    // nothing configured, nothing pulled: Remove is harmless
    let t2 = Tmp::new("remove3");
    let svc2 = services(&t2);
    assert_eq!(remove(&svc2, &|_| Err("must not be called".into())).unwrap(), "");
}

fn lem_model(id: &str, kind: ModelType, downloaded: bool) -> Model {
    Model { id: id.into(), kind, recipe: String::new(), checkpoint: String::new(), labels: vec![], downloaded, loaded: false, suggested: false, size_gb: None, image_defaults: None }
}

#[test]
fn presence_follows_what_lemonade_really_has_and_the_marker_is_only_the_fallback() {
    let t = Tmp::new("presence");
    let svc = services(&t);
    let both = || Some(vec![lem_model(WHISPER_MODEL, ModelType::SpeechToText, true), lem_model(KOKORO_MODEL, ModelType::TextToSpeech, true)]);
    let only_kokoro = || Some(vec![lem_model(KOKORO_MODEL, ModelType::TextToSpeech, true), lem_model(WHISPER_MODEL, ModelType::SpeechToText, false)]);
    assert_eq!(presence_with(&svc, Item::VoicePack, &|| None), Presence::Missing);
    assert_eq!(presence_with(&svc, Item::VoicePack, &only_kokoro), Presence::Missing, "Kokoro alone is Kokoro's own card");
    assert!(matches!(presence_with(&svc, Item::VoicePack, &both), Presence::Installed(d) if d.contains("Whisper-Tiny") && d.contains("kokoro-v1")));
    crate::quickinstall::mark_done(svc.dir(), Item::VoicePack, "x").unwrap();
    assert!(matches!(presence_with(&svc, Item::VoicePack, &|| None), Presence::Earlier(_)));
    assert_eq!(presence_with(&svc, Item::VoicePack, &only_kokoro), Presence::Missing, "a marker outlives a reset volume; reality wins");
}
