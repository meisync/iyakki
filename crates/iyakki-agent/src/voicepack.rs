//! **Voice: Whisper tiny + Kokoro**, one quick-install item of Store \u{2192} Setup (docs/VOICE.md, "Voice in one click").
//!
//! * **The route.** Lemonade (the machine's model server in k3s, `lemonade.rs`) runs whisper.cpp for speech to text and Kokoro for text to
//!   speech, and the speech engines of Settings \u{2192} Voice already know how to talk to it (`lemonade::stt_engine`,
//!   `lemonade::tts_engine`). So the item needs Lemonade (installed first when missing, the usual order k3s, Lemonade, this), pulls the two
//!   models into it and sets the two engines through the existing paths (`Services::use_lemonade_stt` / `use_lemonade_voice`). No second
//!   server, no local program and nothing is added to the image.
//! * **Review first.** [`review_lines`] is what the person reads before the item starts: what is downloaded and how big (estimates), where
//!   it runs, the memory, the languages, the licences with their full texts, and that nothing here is shipped with IYAKKI.
//! * **Nothing half-installed.** [`run`] pulls only what is not there; when a pull or the self-test fails it removes what this run
//!   pulled and says so. Models that were already in Lemonade are never removed.
//! * **Undo.** [`configure`] remembers the engines that were set before ([`Record`]); [`remove`] puts them back (only when the engines
//!   are still the ones this item set) and deletes the models this item pulled.
//! * **Tests touch nothing real.** The step talks to the world through [`crate::quickinstall::StepEnv`]; the self-test through an
//!   [`Upstream`]; both are faked in `voicepack_tests.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};

use crate::quickinstall::{Item, StepEnv};
use crate::voice::SpeechEngine;

/// Lemonade's id of Whisper tiny (whisper.cpp, `ggml-tiny.bin`).
pub const WHISPER_MODEL: &str = "Whisper-Tiny";
/// Lemonade's id of Kokoro v1 (82M).
pub const KOKORO_MODEL: &str = "kokoro-v1";
/// About how much Whisper tiny is (megabytes; the project's own figure is about 75 MB). An estimate.
pub const WHISPER_MB: u32 = 75;
/// About how much Kokoro is (megabytes; the Kokoro step says about 330 MB). An estimate.
pub const KOKORO_MB: u32 = 330;
/// What the self-test has Kokoro say.
pub const SELFTEST_SENTENCE: &str = "This is the IYAKKI voice test.";

/// The licence line of the Setup card (the review has the texts).
pub const LICENCE_SHORT: &str = "IYAKKI provides integration code only. Lemonade (Apache-2.0) is the separate server this item uses; the Whisper and Kokoro models are pulled by Lemonade at your click, under their own licences, which Lemonade or each model's page states. They are not shipped with IYAKKI; downloaded at your click, and IYAKKI gives no guarantee about them.";

/// The review the person reads before the item starts (and the Details of the tile): one string per line. `lemonade_there` says
/// whether Lemonade answers on this machine now (the line about it changes; nothing else does).
pub fn review_lines(lemonade_there: bool) -> Vec<String> {
    let total = WHISPER_MB + KOKORO_MB;
    let v: Vec<String> = vec![
        "Voice: Whisper tiny + Kokoro: listening and a reading voice in one action".into(),
        String::new(),
        "What is downloaded (sizes are estimates, from the projects' own figures)".into(),
        format!("  Whisper tiny ({WHISPER_MODEL}): speech to text, about {WHISPER_MB} MB"),
        format!("  Kokoro ({KOKORO_MODEL}, 82M): text to speech, about {KOKORO_MB} MB"),
        format!("  Together about {total} MB, kept in Lemonade's volume in the cluster (not in the image)"),
        if lemonade_there {
            "  Lemonade already answers on this machine: nothing else is downloaded".into()
        } else {
            "  Lemonade is not answering on this machine yet: it is installed first, with k3s when that is missing (its image is about 1 GB, the cluster about 70 MB; estimates)".into()
        },
        String::new(),
        "Where it runs".into(),
        format!("  Through this machine's Lemonade in k3s (namespace {}), on the CPU, on 127.0.0.1:{} only. No second server and no local program are installed.", crate::quickinstall::LEMONADE_NAMESPACE, crate::lemonade::DEFAULT_PORT),
        "  It goes to the cluster on this machine. With another cluster active, Setup refuses and says how to switch back to \"This machine\".".into(),
        "  Needs about 8 GB of memory on the machine (Lemonade and k3s); the two models use about 1 GB more while they run (estimate).".into(),
        String::new(),
        "Languages".into(),
        "  Whisper tiny understands many languages, but it is the smallest Whisper model: expect more mistakes outside English (estimate, not measured here).".into(),
        "  Kokoro reads English only (American and British voices). Other languages need another voice; the voice test warns when a text is not English.".into(),
        String::new(),
        "What is set up".into(),
        format!("  Settings \u{2192} Voice: speech to text = {WHISPER_MODEL}, reading voice = {KOKORO_MODEL} (voice {}), both on lemonade-local.", crate::lemonade::DEFAULT_KOKORO_VOICE),
        "  They replace the engines set now; Remove puts the old ones back (when you have not changed these since) and deletes the two models this item downloaded.".into(),
        "  Then a self-test: Kokoro says a short sentence, Whisper tiny listens to it, and the result is plain pass or fail. On fail nothing stays installed.".into(),
        "  The sentence is spoken by Kokoro itself; a recording is not bundled. The microphone and the speaker are not used by the test.".into(),
        String::new(),
        "Licences and who provides what".into(),
        "  IYAKKI provides integration code only. It ships and downloads no model weights and no engine of its own for this item.".into(),
        "  Lemonade (Apache-2.0) is a separate server; when you confirm, Lemonade pulls the models above itself. Model weights and engines that Lemonade or you download keep their own licences, which Lemonade or each model's page states. IYAKKI never redistributes them and gives no guarantee about them.".into(),
        "  Some voices use a phonemizer such as espeak-ng (GPL-3.0-or-later) inside Lemonade's own image, as a separate program. IYAKKI never links it and never ships it.".into(),
        "  Not shipped with IYAKKI; downloaded at your click. Nothing of this is in the image.".into(),
    ];
    v
}

/// Does the review carry what the person must see before confirming? (Used by the tests and by a debug check; the lines are fixed text.)
pub fn review_has_required(lines: &[String]) -> bool {
    let t = lines.join("\n");
    ["integration code only", "own licences", "espeak-ng", "GPL-3.0", "Not shipped with IYAKKI; downloaded at your click", "English only", "estimates"].iter().all(|k| t.contains(k))
}

// ------------------------------------------------------------------------------------------------ the record

/// What Setup remembers about this item (`<data>/quick-install/voice-pack.json`): the models this item pulled (so Remove deletes only
/// those) and the engines that were set before (so Remove puts them back).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub pulled: Vec<String>,
    /// The engines before [`configure`] changed them; `configured` says whether it did.
    #[serde(default)]
    pub previous_stt: Option<SpeechEngine>,
    #[serde(default)]
    pub previous_tts: Option<SpeechEngine>,
    #[serde(default)]
    pub configured: bool,
}

fn record_path(data: &Path) -> PathBuf {
    crate::quickinstall::state_dir(data).join("voice-pack.json")
}

pub fn read_record(data: &Path) -> Record {
    std::fs::read_to_string(record_path(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_record(data: &Path, r: &Record) -> Result<(), String> {
    std::fs::create_dir_all(crate::quickinstall::state_dir(data)).map_err(|e| e.to_string())?;
    let t = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    let tmp = record_path(data).with_extension("json.tmp");
    std::fs::write(&tmp, t).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, record_path(data)).map_err(|e| e.to_string())
}

/// Remember models this item pulled (added to what an earlier run pulled, never twice).
pub fn record_pulled(data: &Path, models: &[String]) -> Result<(), String> {
    let mut r = read_record(data);
    for m in models {
        if !r.pulled.contains(m) {
            r.pulled.push(m.clone());
        }
    }
    write_record(data, &r)
}

// ------------------------------------------------------------------------------------------------ the step

/// The self-test of this item: Kokoro says [`SELFTEST_SENTENCE`], Whisper tiny transcribes it. `base` is Lemonade's API address.
/// `Ok` is the plain pass line, `Err` the plain fail line.
pub fn selftest(base: &str, up: Arc<dyn Upstream>) -> Result<String, String> {
    let secrets = crate::vault::Secrets::new(None, up.clone());
    let tts = crate::lemonade::tts_engine(base, KOKORO_MODEL, None, None);
    let stt = crate::lemonade::stt_engine(base, WHISPER_MODEL, None);
    let wav = crate::voice::synthesize(&tts, SELFTEST_SENTENCE, &secrets, up.as_ref()).map_err(|e| format!("Self-test failed: Kokoro did not speak: {e}"))?;
    let parsed = crate::voice::parse_wav(&wav).map_err(|e| format!("Self-test failed: Kokoro's sound is not a WAV file: {e}"))?;
    let mono = crate::voice::to_mono_16k(&parsed);
    if mono.is_empty() || crate::voice::peak(&mono) == 0 {
        return Err("Self-test failed: Kokoro's sound is silent".into());
    }
    let secs = mono.len() as f32 / 16_000.0;
    let heard = crate::voice::transcribe(&stt, &wav, &secrets, up.as_ref()).map_err(|e| format!("Self-test failed: Whisper tiny did not answer: {e}"))?;
    verdict(SELFTEST_SENTENCE, &heard, secs)
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// Pass when Whisper heard at least half of the words that were spoken (a tiny model may miss a made-up word like IYAKKI).
pub fn verdict(expected: &str, heard: &str, secs: f32) -> Result<String, String> {
    let want = words(expected);
    let got = words(heard);
    let hit = want.iter().filter(|w| got.contains(w)).count();
    if got.is_empty() {
        return Err(format!("Self-test failed: Kokoro spoke {secs:.1} s but Whisper tiny heard nothing"));
    }
    if hit * 2 >= want.len() {
        Ok(format!("Self-test passed: Kokoro spoke {secs:.1} s, Whisper tiny heard \"{}\" ({hit} of {} words)", heard.trim(), want.len()))
    } else {
        Err(format!("Self-test failed: Whisper tiny heard \"{}\" ({hit} of {} words matched)", heard.trim(), want.len()))
    }
}

/// Take back what this run pulled (a failed step leaves nothing behind). Returns the models that could not be removed.
fn roll_back(env: &mut dyn StepEnv, pulled: &[String]) -> Vec<String> {
    let mut stuck = Vec::new();
    for m in pulled.iter().rev() {
        env.say(&format!("taking back {m}"));
        if let Err(e) = env.lemonade_delete(m) {
            env.say(&format!("  could not remove {m}: {e}"));
            stuck.push(m.clone());
        }
    }
    stuck
}

fn fail(env: &mut dyn StepEnv, pulled: &[String], why: String) -> Result<(), String> {
    if pulled.is_empty() {
        return Err(format!("{why}. Nothing was installed."));
    }
    let stuck = roll_back(env, pulled);
    if stuck.is_empty() {
        Err(format!("{why}. What this run downloaded was removed again; nothing is half installed."))
    } else {
        Err(format!("{why}. Could not remove {} again: delete it in Settings \u{2192} Models.", stuck.join(", ")))
    }
}

/// The step of the item. Lemonade must answer (the caller deploys it); then: pull what is missing, record it, self-test, mark, and ask
/// the shell to set the engines.
pub fn run(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    env.say(&format!("[0/6] Voice: Whisper tiny + Kokoro. Licences: {}", LICENCE_SHORT));
    crate::quickinstall::ensure_lemonade(env, wait, poll)?;
    let have = env.lemonade_models().map_err(|e| format!("could not read Lemonade's models: {e}. Nothing was installed."))?;
    let mut pulled: Vec<String> = Vec::new();
    for (n, (model, mb)) in [(WHISPER_MODEL, WHISPER_MB), (KOKORO_MODEL, KOKORO_MB)].into_iter().enumerate() {
        if have.iter().any(|m| m == model) {
            env.say(&format!("[{}/6] {model} is in Lemonade already: not downloaded again", n + 1));
            continue;
        }
        env.say(&format!("[{}/6] pulling {model} (about {mb} MB)", n + 1));
        match env.lemonade_pull(model) {
            Ok(()) => pulled.push(model.to_string()),
            Err(e) => {
                // a half-pulled model is taken back too
                let mut all = pulled.clone();
                all.push(model.to_string());
                return fail(env, &all, format!("Pulling {model} failed: {e}"));
            }
        }
    }
    if !pulled.is_empty() {
        env.record_pulled(&pulled)?;
    }
    env.say("[3/6] both models are in Lemonade; running the self-test (Kokoro speaks, Whisper tiny listens)");
    match env.voice_selftest() {
        Ok(line) => env.say(&format!("[4/6] {line}")),
        Err(e) => return fail(env, &pulled, e),
    }
    env.mark(Item::VoicePack, &format!("{WHISPER_MODEL} and {KOKORO_MODEL} in Lemonade"))?;
    env.request_after("voice-pack")?;
    env.say("[5/6] asked IYAKKI to set the engines in Settings \u{2192} Voice (speech to text and reading voice)");
    env.say("[6/6] done: listening uses Whisper tiny, reading uses Kokoro (English only). Remove in the tile's details puts your old settings back");
    Ok(())
}

// ------------------------------------------------------------------------------------------------ settings

/// Set the speech engines of Settings \u{2192} Voice to Whisper tiny and Kokoro on the Lemonade provider `provider`, remembering what was
/// set before (first time only: running it again changes nothing). `Ok` is the plain line.
pub fn configure(svc: &crate::services::Services, provider: &str) -> Result<String, String> {
    let data = svc.dir().to_path_buf();
    let mut rec = read_record(&data);
    let (stt, tts) = {
        let st = svc.settings.lock().unwrap();
        let v = &st.get().voice;
        (v.stt.clone(), v.tts.clone())
    };
    let cfg = svc.settings.lock().unwrap().get().provider(provider).cloned().ok_or("unknown provider")?;
    let base = cfg.base()?;
    let ours_stt = crate::lemonade::stt_engine(&base, WHISPER_MODEL, cfg.key_ref.clone());
    let ours_tts = crate::lemonade::tts_engine(&base, KOKORO_MODEL, None, cfg.key_ref.clone());
    if !rec.configured {
        rec.previous_stt = stt.filter(|e| *e != ours_stt);
        rec.previous_tts = tts.filter(|e| *e != ours_tts);
        rec.configured = true;
        write_record(&data, &rec)?;
    }
    let a = svc.use_lemonade_stt(provider, WHISPER_MODEL)?;
    let b = svc.use_lemonade_voice(provider, KOKORO_MODEL, None)?;
    Ok(format!("{a} {b}"))
}

/// Called by the shell when the job asked for it: find Lemonade, add it to Models, and [`configure`]. True when settled, false to try again.
pub fn configure_when_ready(svc: &Arc<crate::services::Services>) -> bool {
    let ports = crate::lemonade::local_ports_override().map(|o| o.1).unwrap_or_else(|| crate::lemonade::local_ports().to_vec());
    let Some((base, version)) = ports.into_iter().find_map(crate::lemonade::probe_local) else { return false };
    svc.use_local_lemonade(&base, version.as_deref());
    let want = crate::lemonade::api_base(&base);
    let id = {
        let st = svc.settings.lock().unwrap();
        st.get().providers.iter().find(|p| p.kind == crate::providers::Kind::Lemonade && p.base_url.as_deref().is_some_and(|u| crate::lemonade::api_base(u) == want)).map(|p| p.id.clone())
    };
    let Some(id) = id else { return false };
    if svc.refresh_lemonade(&id).is_err() {
        return false;
    }
    configure(svc, &id).is_ok()
}

/// Undo: put the engines that were set before back (only when the engines are still the ones this item set; a change you made since is
/// kept), delete the models this item pulled with `delete`, forget the marker and the record. Returns the plain lines to show.
pub fn remove(svc: &crate::services::Services, delete: &dyn Fn(&str) -> Result<(), String>) -> Result<String, String> {
    let data = svc.dir().to_path_buf();
    let mut rec = read_record(&data);
    let mut said: Vec<String> = Vec::new();
    if rec.configured {
        let (stt, tts) = {
            let st = svc.settings.lock().unwrap();
            let v = &st.get().voice;
            (v.stt.clone(), v.tts.clone())
        };
        let is_ours = |e: &Option<SpeechEngine>, model: &str| matches!(e, Some(SpeechEngine::Http { model: m, url, .. }) if m == model && url.contains("/api/v1/audio/"));
        let (back_stt, back_tts) = (is_ours(&stt, WHISPER_MODEL), is_ours(&tts, KOKORO_MODEL));
        let (prev_stt, prev_tts) = (rec.previous_stt.clone(), rec.previous_tts.clone());
        svc.settings
            .lock()
            .unwrap()
            .update(|s| {
                if back_stt {
                    s.voice.stt = prev_stt;
                }
                if back_tts {
                    s.voice.tts = prev_tts;
                }
            })
            .map_err(|e| e.to_string())?;
        said.push(match (back_stt, back_tts) {
            (true, true) => "Your earlier speech engines are back in Settings \u{2192} Voice.".into(),
            (false, false) => "Settings \u{2192} Voice was changed by you since, so it was left as it is.".into(),
            _ => "One speech engine was put back; the other was changed by you since and was left as it is.".into(),
        });
    }
    let mut stuck: Vec<String> = Vec::new();
    for m in rec.pulled.clone() {
        match delete(&m) {
            Ok(()) => said.push(format!("Deleted {m} from Lemonade.")),
            Err(e) => {
                said.push(format!("Could not delete {m}: {e}."));
                stuck.push(m);
            }
        }
    }
    rec.pulled = stuck.clone();
    rec.configured = false;
    rec.previous_stt = None;
    rec.previous_tts = None;
    if stuck.is_empty() {
        let _ = std::fs::remove_file(record_path(&data));
    } else {
        write_record(&data, &rec)?;
    }
    crate::quickinstall::unmark(&data, Item::VoicePack);
    let line = said.join(" ");
    if stuck.is_empty() { Ok(line) } else { Err(line) }
}

/// Delete one model of the local Lemonade (`POST /delete`): for [`remove`] and for the rollback of a failed step.
pub fn delete_local_model(model: &str) -> Result<(), String> {
    let base = crate::lemonade::local_ports().iter().find_map(|p| crate::lemonade::probe_local(*p)).map(|b| b.0).ok_or("Lemonade does not answer")?;
    let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(120));
    let body = serde_json::json!({ "model_name": model }).to_string();
    let r = Upstream::request(&up, "POST", &format!("{base}/delete"), &[("Content-Type".into(), "application/json".into())], body.as_bytes())?;
    if r.status / 100 == 2 { Ok(()) } else { Err(format!("Lemonade answered HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(200).collect::<String>())) }
}

#[cfg(test)]
#[path = "voicepack_tests.rs"]
mod tests;
