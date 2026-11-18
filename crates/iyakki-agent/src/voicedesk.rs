//! The logic behind the VOICE tile: which reading voices exist, where the text goes, how it is cut into pieces, the
//! microphones, and the recorder and speech-to-text calls. The screen (`iyakki-shell` `apps/voice*.rs`) only draws and clicks.
//!
//! Reading uses the same pieces as TALES and the chat (`story::Reader`, `voice::synthesize_with`); listening uses the
//! speech-to-text engine of Settings -> Voice, else a Lemonade whisper model. Text and sound go to another machine only after
//! the person said yes to that machine (`Services::allow_speech_server`, the decision Settings -> Models stores).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::lemonade::{self, ModelType};
use crate::services::Services;
pub use iyakki_shared::voicedesk::{SPEED_MIN, SPEED_MAX, ReadVoice, engine_url, Dest, voice_key, Hint, language_names, detect_language, language_warning, parse_mics, t2_card_numbers, t2_capture_allowed, parse_mics_with, list_mics, t2_mic, set_t2_capture, t2_mic_hidden, record_cmd_for, record_cmd_for_device, BROWSER_MIC_CHOICE, BROWSER_MIC_STATUS, share_mic_choice, browser_mic_failure, mic_failure_text, no_mic_text, no_mic_text_t2, product_name, NO_SPEECH, ONLY_SILENCE, is_no_words, proc_root, reword_storage_error};
use crate::speechtime::{self, Locality};
use crate::story::{self, Chunk};
use crate::voice::{self, SpeakOpts, SpeechEngine};
pub use iyakki_shared::voicedesk::{Capture, PAUSE_END_MS, PauseDetector, Recorders, SPEECH_MIN_LEVEL, TALK_CAP_MS, TALK_MIN_MS, pick_mic};
pub use iyakki_shared::voicedesk::{MAX_RECORD_SECS, MAX_TEXT_CHARS, Mic, T2Mic};







/// The speed range of the slider.
fn providers(svc: &Services) -> Vec<crate::providers::ProviderCfg> {
    svc.settings.lock().map(|s| s.get().providers.clone()).unwrap_or_default()
}

/// found on this computer.
pub fn read_voices(svc: &Services) -> Vec<ReadVoice> {
    let mut out: Vec<ReadVoice> = Vec::new();
    let push = |v: ReadVoice, out: &mut Vec<ReadVoice>| {
        if !out.iter().any(|o| o.id == v.id || (o.engine == v.engine && o.voice == v.voice)) {
            out.push(v);
        }
    };
    let set = svc.voice_settings().tts;
    if let Some(e) = set.clone().filter(|e| !matches!(e, SpeechEngine::Browser)) {
        let voice = match &e {
            SpeechEngine::Http { voice, .. } | SpeechEngine::Host { voice, .. } => voice.clone().filter(|v| !v.is_empty()),
            _ => None,
        };
        let english = matches!(&e, SpeechEngine::Http { model, .. } if model.to_ascii_lowercase().contains("kokoro"));
        let label = format!("{} (set in Settings -> Voice)", e.label());
        push(ReadVoice { id: "settings".into(), label, source: "Settings -> Voice".into(), engine: e, voice, english_only: english }, &mut out);
    }
    let all = providers(svc);
    for (prov, model) in svc.lemonade_models(ModelType::TextToSpeech) {
        let Some(p) = all.iter().find(|p| p.id == prov) else { continue };
        let Ok(base) = p.base() else { continue };
        let kokoro = model.to_ascii_lowercase().contains("kokoro");
        for v in lemonade::voices_for(&model) {
            let engine = lemonade::tts_engine(&base, &model, Some(v), p.key_ref.clone());
            let name = if kokoro { "Kokoro".to_string() } else { model.clone() };
            push(ReadVoice { id: format!("lemonade:{prov}:{model}:{v}"), label: format!("{name} · {v}"), source: format!("{name} on {prov}"), engine, voice: Some(v.to_string()), english_only: kokoro }, &mut out);
        }
    }
    for p in all.iter().filter(|p| p.enabled && p.kind != crate::providers::Kind::Lemonade) {
        let Ok(base) = p.base() else { continue };
        for (m, _) in p.model_kinds.iter().filter(|(_, k)| k.as_str() == "tts") {
            let probe = SpeechEngine::Http { url: format!("{base}/audio/speech"), model: m.clone(), key_ref: p.key_ref.clone(), voice: None, language: None };
            for v in story::voice_choices(&probe) {
                let engine = SpeechEngine::Http { url: format!("{base}/audio/speech"), model: m.clone(), key_ref: p.key_ref.clone(), voice: Some(v.clone()), language: None };
                push(ReadVoice { id: format!("provider:{}:{m}:{v}", p.id), label: format!("{m} · {v}"), source: format!("{m} on {}", p.id), engine, voice: Some(v), english_only: false }, &mut out);
            }
        }
    }
    svc.host_tts.ensure();
    if let Some(r) = svc.host_tts.report() {
        for e in r.engines.iter().filter(|e| e.usable()) {
            let engine = crate::hostengines::engine_setting(&e);
            let label = format!("{} (on this computer)", e.kind.label());
            push(ReadVoice { id: format!("host:{}", e.kind.id()), label, source: "this computer".into(), engine, voice: None, english_only: false }, &mut out);
        }
    }
    out
}

pub fn destination(svc: &Services, engine: &SpeechEngine) -> Option<Dest> {
    let url = engine_url(engine)?;
    let server = speechtime::server_of(&url);
    let address = server.split_once("://").map_or(server.clone(), |(_, r)| r.to_string());
    let name = providers(svc).iter().find(|p| p.base().ok().is_some_and(|b| speechtime::server_of(&b) == server)).map(|p| p.id.clone()).unwrap_or_else(|| iyakki_sec::url_host(&url).unwrap_or_default());
    Some(Dest { name, address, server, place: speechtime::locality(&url) })
}

/// The line that says where the text goes: "Sent to lemonade-local at localhost:13305 (this computer)".
pub fn where_text(svc: &Services, engine: &SpeechEngine, what: &str) -> String {
    match destination(svc, engine) {
        Some(d) => format!("{what} sent to {} at {} ({})", d.name, d.address, d.place_words()),
        None => format!("{what} stays on this computer (read with {})", engine.label()),
    }
}

/// The card shown before text or sound goes to a machine the person has not agreed to yet (None: nothing to ask).
pub fn consent_text(svc: &Services, engine: &SpeechEngine, listening: bool) -> Option<(Dest, String)> {
    let d = destination(svc, engine)?;
    if svc.speech_server_allowed(&engine_url(engine)?) {
        return None;
    }
    let place = match d.place {
        Locality::ThisComputer => "on this computer",
        Locality::Network => "on the network",
        Locality::Internet => "on the internet",
    };
    let host = iyakki_sec::url_host(&d.server).unwrap_or_else(|| d.address.clone());
    let text = if listening {
        format!("{} ({host}) is {place}. Speech-to-text with it sends what the microphone hears to that machine. Send sound to {host} from now on?", d.name)
    } else {
        format!("{} ({host}) is {place}. Reading aloud with it sends the text being read to that machine, and its sound comes back from there. Send text to {host} from now on?", d.name)
    };
    Some((d, text))
}

// ------------------------------------------------------------------------------------------------------------ reading

/// The pieces to read: one per sentence (long ones cut at a comma or a space), without the ones that have nothing to say
/// (a bare address, a line of marks). Positions are characters of `text`, so the lit sentence can be drawn in the box.
pub fn pieces(text: &str) -> Vec<Chunk> {
    story::sentence_chunks(text, story::MAX_CHUNK_CHARS).into_iter().filter(|c| !voice::speakable(&c.text).is_empty()).collect()
}

/// Makes the sound of one piece with the chosen voice and speed (the engine is asked directly: the choice on the page wins over
/// Settings -> Voice for this reading only).
pub struct ChoiceSynth {
    pub svc: Arc<Services>,
    pub engine: SpeechEngine,
    pub opts: SpeakOpts,
}

impl story::Synth for ChoiceSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        let key = speechtime::cache_key(&self.engine, &self.opts, text);
        if let Some((w, _)) = speechtime::cached(&key) {
            return Ok((*w).clone());
        }
        let up = self.svc.slow_upstream();
        let wav = voice::synthesize_with(&self.engine, text, &self.opts, &self.svc.secrets, up.as_ref()).map_err(|e| explain_tts(&self.engine, &e))?;
        speechtime::remember(&key, Arc::new(wav.clone()), None);
        Ok(wav)
    }
}

/// An error of the speech server in plain words.
pub fn explain_tts(engine: &SpeechEngine, e: &str) -> String {
    if speechtime::unreachable(e) {
        let at = engine_url(engine).map_or("the speech server".to_string(), |u| speechtime::server_of(&u));
        return format!("The reading voice did not answer at {at}. Is the server running?");
    }
    match engine {
        SpeechEngine::Http { model, .. } => lemonade::explain_error(model, e),
        _ => e.to_string(),
    }
}

/// The key under which sound made for one voice is kept by the reader (a different voice or speed makes new sound).
impl iyakki_shared::voicedesk::Playback for voice::Player {
    fn is_playing(&mut self) -> bool {
        voice::Player::is_playing(self)
    }
    fn wait(&mut self, timeout: Duration) -> bool {
        voice::Player::wait(self, timeout)
    }
    fn stop(&mut self) {
        voice::Player::stop(self)
    }
    fn played_ms(&self) -> Option<u64> {
        voice::Player::played_ms(self)
    }
}

/// The real speaker: the player of Settings -> Voice (through the mixer when it is on).
pub struct SystemPlayers;

impl iyakki_shared::voicedesk::Players for SystemPlayers {
    fn play(&self, cmd: &str, wav: &[u8]) -> Result<Box<dyn iyakki_shared::voicedesk::Playback>, String> {
        Ok(Box::new(voice::Player::start(cmd, wav)?))
    }
}

impl Capture for voice::Recorder {
    fn drain(&mut self) -> Vec<i16> {
        self.drain_samples()
    }
    fn level(&self) -> f32 {
        voice::Recorder::level(self)
    }
    fn elapsed(&self) -> Duration {
        voice::Recorder::elapsed(self)
    }
    fn is_full(&self) -> bool {
        voice::Recorder::is_full(self)
    }
    fn is_alive(&mut self) -> bool {
        voice::Recorder::is_alive(self)
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        voice::Recorder::stop(*self)
    }
}


/// The real thing: `/proc/asound` and the record command of Settings -> Voice (`arecord`).
pub struct SystemRecorders;

impl Recorders for SystemRecorders {
    fn mics(&self) -> Vec<Mic> {
        list_mics(Path::new("/proc"))
    }
    fn product(&self) -> String {
        product_name(Path::new("/sys"))
    }
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String> {
        Ok(Box::new(voice::Recorder::start_max(cmd, max_secs)?))
    }
}

// ------------------------------------------------------------------------------------------------------------ listening

/// The speech-to-text engine to use: the one of Settings -> Voice, else a downloaded Lemonade whisper model (the smallest one
/// first is not assumed: the first listed).
pub fn stt_engine(svc: &Services) -> Option<SpeechEngine> {
    if let Some(e) = svc.voice_settings().stt {
        return Some(e);
    }
    lemonade_stt_engine(svc)
}

/// The Lemonade Whisper model, if one is set up (the automatic speech-to-text when Settings -> Voice chose none).
pub fn lemonade_stt_engine(svc: &Services) -> Option<SpeechEngine> {
    let (prov, model) = svc.lemonade_models(ModelType::SpeechToText).into_iter().next()?;
    let p = providers(svc).into_iter().find(|p| p.id == prov)?;
    Some(lemonade::stt_engine(&p.base().ok()?, &model, p.key_ref.clone()))
}

/// Recorded samples to text with `engine`. A silent recording is told apart from a failure.
pub fn transcribe(svc: &Services, engine: &SpeechEngine, pcm: &[i16]) -> Result<String, String> {
    if pcm.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    if voice::peak(pcm) < 150 {
        return Err(ONLY_SILENCE.into());
    }
    let up = svc.slow_upstream();
    let text = voice::transcribe(engine, &voice::encode_wav(pcm, voice::SAMPLE_RATE), &svc.secrets, up.as_ref()).map_err(|e| match engine {
        SpeechEngine::Http { model, .. } if speechtime::unreachable(&e) => format!("The speech-to-text server did not answer ({model}). Is it running?"),
        SpeechEngine::Http { model, .. } => lemonade::explain_error(model, &e),
        _ => e,
    })?;
    if text.is_empty() { Err(NO_SPEECH.into()) } else { Ok(text) }
}

/// Recorded samples to text through one voice backend (`voicebackend::VoiceBackend`): the same silence check and the same wording
/// of failures as [`transcribe`], but the engine is the backend, so the voice session and the `voice_backends` tool name one place.
pub fn transcribe_backend(b: &dyn crate::voicebackend::VoiceBackend, pcm: &[i16]) -> Result<String, String> {
    if pcm.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    if voice::peak(pcm) < 150 {
        return Err(ONLY_SILENCE.into());
    }
    let model = match b.engine() {
        Some(SpeechEngine::Http { model, .. }) => Some(model.clone()),
        _ => None,
    };
    let text = b.transcribe(&voice::encode_wav(pcm, voice::SAMPLE_RATE), "").map_err(|e| match &model {
        Some(m) if speechtime::unreachable(&e) => format!("The speech-to-text server did not answer ({m}). Is it running?"),
        Some(m) => lemonade::explain_error(m, &e),
        None => e,
    })?;
    if text.is_empty() { Err(NO_SPEECH.into()) } else { Ok(text) }
}

/// What speech-to-text says when it heard no words.
/// The folder both sides use: `IYAKKI_MIC_DIR` when set (tests, the plugin reads the same variable), else the shared memory folder.
pub fn mic_share_dir() -> PathBuf {
    match std::env::var("IYAKKI_MIC_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::sysinfo::shared_memory_dir(),
    }
}

#[cfg(test)]
#[path = "voicedesk_tests.rs"]
mod tests;

/// The voice desk facet over these services (`cx.services.voicedesk_facet()`).
impl Services {
    pub fn voicedesk_facet(self: &Arc<Self>) -> Arc<dyn iyakki_agent_api::VoiceDeskFacet> {
        Arc::new(RealVoiceDesk { svc: self.clone() })
    }
}

struct RealVoiceDesk {
    svc: Arc<Services>,
}

impl iyakki_agent_api::VoiceDeskFacet for RealVoiceDesk {
    fn read_voices(&self) -> Vec<ReadVoice> {
        read_voices(&self.svc)
    }
    fn stt_engine(&self) -> Option<SpeechEngine> {
        stt_engine(&self.svc)
    }
    fn where_text(&self, engine: &SpeechEngine, what: &str) -> String {
        where_text(&self.svc, engine, what)
    }
    fn consent_text(&self, engine: &SpeechEngine, listening: bool) -> Option<(Dest, String)> {
        consent_text(&self.svc, engine, listening)
    }
    fn transcribe(&self, engine: &SpeechEngine, pcm: &[i16]) -> Result<String, String> {
        transcribe(&self.svc, engine, pcm)
    }
    fn pieces(&self, text: &str) -> Vec<Chunk> {
        pieces(text)
    }
    fn command_sink(&self, play_cmd: String) -> Option<Arc<dyn story::Sink>> {
        Some(Arc::new(crate::storykit::CommandSink { play_cmd }))
    }
    fn choice_synth(&self, engine: SpeechEngine, opts: SpeakOpts) -> Option<Arc<dyn story::Synth>> {
        Some(Arc::new(ChoiceSynth { svc: self.svc.clone(), engine, opts }))
    }
}
