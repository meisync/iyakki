//! Types moved out of `iyakki_agent::gemini` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// The id of the provider this module keeps in Settings -> Models.
pub const PROVIDER_ID: &str = "google-gemini";

pub use iyakki_voice_model::KEY_REF;

pub use iyakki_voice_model::Tier;

/// Where the consent of one person stands for the tier chosen now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsentState {
    Missing,
    /// Given for the other tier: the card is shown again.
    OtherTier { given: Tier, date: String },
    Given { date: String },
}

/// The advice after a "too many requests" answer, by tier.
pub fn limit_advice(tier: Tier) -> &'static str {
    match tier {
        Tier::Free => "The free Gemini limit is reached. Wait a few minutes (or until tomorrow if the day's limit is used up), or put your own paid key in Settings -> Models -> Google Gemini and choose the paid tier.",
        Tier::Paid => "Google says your key has reached its limit. Wait a little, or look at the quota of your Google project.",
    }
}

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// Google Gemini as an OPTIONAL cloud provider (EXPERIMENTAL, off by default; docs/GEMINI-PROVIDER.md).
//
// Two uses, each only when the person chose it: speech-to-text (`voice.stt` is a [`SpeechEngine::Gemini`]: the recording goes to
// `generateContent` as inline data) and the chat model (the provider [`PROVIDER_ID`], the OpenAI-compatible endpoint of Google,
// through the ordinary provider code). The on-device engines stay the default; nothing else is ever sent.
//
// * The key lives in the vault (`vault:gemini-key`), typed by the person in Settings -> Models. It is sent in a header
//   (`x-goog-api-key`), never in an address, and it is removed from every error text before the text is shown or logged.
// * Nothing leaves this computer until the person answered YES to the consent card ([`consent_card`]) for the tier now chosen. The
//   answer is kept per person and per computer with its date (`gemini.json` in the data folder); changing the tier asks again;
//   the person can take the answer back. The assistant can read the state and revoke, but can never give the consent.
// * [`GuardedUpstream`] is the one gate: every request to Google passes it, so no code path can send without the consent.
// * The tier (`free` or `paid`) only changes the notice and the advice after a "too many requests" answer: it cannot be detected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::Upstream;
use serde_json::{json, Value};

use crate::providers::{valid_model_name, Kind, ProviderCfg};
use crate::settings::ModelChoice;
use crate::vault::{LocalVault, Secrets};
use crate::voice::SpeechEngine;

/// The host every request goes to.
pub const HOST: &str = "generativelanguage.googleapis.com";
/// The native API (speech-to-text).
pub const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
/// The OpenAI-compatible endpoint (chat with tools): the provider code adds `/chat/completions`.
pub const OPENAI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
/// The vault entry of the key (typed in Settings -> Models; the assistant cannot set it).
pub const KEY_NAME: &str = "gemini-key";
/// Editable defaults, kept in `gemini.json`; no other code names a model.
pub const DEFAULT_CHAT_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_STT_MODEL: &str = "gemini-2.5-flash";
/// The largest recording sent in one request (inline data, before base64).
pub const MAX_AUDIO_BYTES: usize = 15 * 1024 * 1024;
/// After a 429 the request is repeated at most this many times, each after the wait Google asked for (at most [`MAX_WAIT_SECS`]).
pub const MAX_RETRIES: u32 = 2;
pub const MAX_WAIT_SECS: u64 = 20;
/// The file in the data folder.
pub const FILE: &str = "gemini.json";
/// The most consents kept (one per person).
const MAX_PEOPLE: usize = 500;

/// Whether the address is one of Google's Gemini API.
pub fn is_google_url(url: &str) -> bool {
    iyakki_sec::url_host(url).is_some_and(|h| h == HOST)
}



/// The person's answer to the consent card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    /// The tier that was on the card.
    pub tier: Tier,
    /// The day it was given (`2026-11-13`).
    pub date: String,
}


/// What is kept in `gemini.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The managed provider and engine are available (the switch of Settings -> Models -> Google Gemini).
    pub enabled: bool,
    pub tier: Tier,
    pub chat_model: String,
    pub stt_model: String,
    /// What the chat model and the speech engine were before Gemini took them over (put back when the person turns it off).
    pub prev_chat: Option<ModelChoice>,
    pub prev_stt: Option<SpeechEngine>,
    /// Per person (this computer's file, so also per computer).
    pub consent: BTreeMap<String, Consent>,
    /// The live voice assistant (EXPERIMENTAL, off by default): its settings and its own consent (a yes to speech-to-text is not a yes
    /// to an open microphone), kept per person like [`Config::consent`].
    pub voice: crate::geminilive::LiveCfg,
    pub live_consent: BTreeMap<String, Consent>,
}

impl Default for Config {
    fn default() -> Config {
        Config { enabled: false, tier: Tier::Free, chat_model: DEFAULT_CHAT_MODEL.into(), stt_model: DEFAULT_STT_MODEL.into(), prev_chat: None, prev_stt: None, consent: BTreeMap::new(), voice: Default::default(), live_consent: BTreeMap::new() }
    }
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Reads the file (a missing or damaged one is the default: off).
pub fn load(dir: &Path) -> Config {
    let _g = FILE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    load_unlocked(dir)
}

fn load_unlocked(dir: &Path) -> Config {
    std::fs::read(dir.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_unlocked(dir: &Path, c: &Config) -> Result<(), String> {
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.tmp"));
    let bytes = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot save the Gemini settings: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot save the Gemini settings: {e}"))
}

/// Changes the file under the lock.
pub fn update(dir: &Path, f: impl FnOnce(&mut Config) -> Result<(), String>) -> Result<Config, String> {
    let _g = FILE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut c = load_unlocked(dir);
    f(&mut c)?;
    save_unlocked(dir, &c)?;
    Ok(c)
}

/// Where `user`'s consent stands for the tier chosen now.
pub fn consent_state(dir: &Path, user: &str) -> ConsentState {
    let c = load(dir);
    match c.consent.get(user) {
        None => ConsentState::Missing,
        Some(k) if k.tier == c.tier => ConsentState::Given { date: k.date.clone() },
        Some(k) => ConsentState::OtherTier { given: k.tier, date: k.date.clone() },
    }
}

/// The person answered YES to the card for the tier chosen now. Only the Settings page (a key press by the person) calls this; no
/// assistant tool does.
pub fn grant(dir: &Path, user: &str) -> Result<(), String> {
    grant_on(dir, user, &iyakki_clock::today())
}

pub fn grant_on(dir: &Path, user: &str, date: &str) -> Result<(), String> {
    if user.is_empty() {
        return Err("nobody is signed in".into());
    }
    update(dir, |c| {
        if !c.consent.contains_key(user) && c.consent.len() >= MAX_PEOPLE {
            return Err("too many people have answered on this computer".into());
        }
        let tier = c.tier;
        c.consent.insert(user.to_string(), Consent { tier, date: date.to_string() });
        Ok(())
    })
    .map(|_| ())
}

/// The person takes the consent back. Returns whether there was one.
pub fn revoke(dir: &Path, user: &str) -> Result<bool, String> {
    let mut had = false;
    update(dir, |c| {
        let a = c.consent.remove(user).is_some();
        let b = c.live_consent.remove(user).is_some();
        had = a || b;
        Ok(())
    })?;
    Ok(had)
}

/// `Ok` only when `user` said yes for the tier chosen now; else the plain reason.
pub fn allowed(dir: &Path, user: &str) -> Result<(), String> {
    match consent_state(dir, user) {
        ConsentState::Given { .. } => Ok(()),
        ConsentState::Missing => Err(NEEDS_CONSENT.into()),
        ConsentState::OtherTier { .. } => Err(format!("{NEEDS_CONSENT} (the tier changed, so the question is asked again)")),
    }
}

/// Where the live voice consent of `user` stands for the tier chosen now.
pub fn live_consent_state(dir: &Path, user: &str) -> ConsentState {
    let c = load(dir);
    match c.live_consent.get(user) {
        None => ConsentState::Missing,
        Some(k) if k.tier == c.tier => ConsentState::Given { date: k.date.clone() },
        Some(k) => ConsentState::OtherTier { given: k.tier, date: k.date.clone() },
    }
}

/// The person answered YES to the live voice card (a key press by the person on the Settings page; no assistant tool calls this).
pub fn grant_live_on(dir: &Path, user: &str, date: &str) -> Result<(), String> {
    if user.is_empty() {
        return Err("nobody is signed in".into());
    }
    update(dir, |c| {
        if !c.live_consent.contains_key(user) && c.live_consent.len() >= MAX_PEOPLE {
            return Err("too many people have answered on this computer".into());
        }
        let tier = c.tier;
        c.live_consent.insert(user.to_string(), Consent { tier, date: date.to_string() });
        Ok(())
    })
    .map(|_| ())
}

/// `Ok` only when `user` said yes to the live voice card for the tier chosen now.
pub fn allowed_live(dir: &Path, user: &str) -> Result<(), String> {
    match live_consent_state(dir, user) {
        ConsentState::Given { .. } => Ok(()),
        ConsentState::Missing => Err(crate::geminilive::NEEDS_LIVE_CONSENT.into()),
        ConsentState::OtherTier { .. } => Err(format!("{} (the tier changed, so the question is asked again)", crate::geminilive::NEEDS_LIVE_CONSENT)),
    }
}

/// What the person sees when something would go to Google before they agreed. Nothing was sent.
pub const NEEDS_CONSENT: &str = "Google Gemini is not allowed yet, so nothing was sent to Google. Open Settings -> Models, press C on Google Gemini and answer the question there (the assistant cannot answer it for you)";

/// What the free tier means, for the Settings page and the assistant (no numbers: Google changes them).
pub const FREE_LIMITS: &str = "The free tier has small limits per minute and per day; when they are used up Google answers \"too many requests\" and the next try works after a wait.";

/// The consent card. `tier` is the one chosen now.
pub fn consent_card(tier: Tier) -> String {
    let mine = match tier {
        Tier::Free => "You chose the FREE tier: according to Google's published terms (check them), Google may use what you send to improve its products, and people may review it. Do not send anything private.",
        Tier::Paid => "You chose your own PAID key: according to Google's published terms (check them), what you send is not used to train its models.",
    };
    let other = match tier {
        Tier::Free => "With a paid key, according to Google's published terms (check them), it is not used for training.",
        Tier::Paid => "On the free tier, according to Google's published terms (check them), Google may use what is sent to improve its products and people may review it.",
    };
    format!("Send audio and text to Google? While Google Gemini is chosen for speech-to-text or for the assistant, your recordings and your messages (with the results of the tools) are sent to Google's servers ({HOST}). {mine} {other} Answer Yes to allow it, No to keep everything on this computer. You can take your answer back in Settings -> Models at any time.")
}

// -------------------------------------------------------------------------------------------------------------- the gate

/// Wraps a transport: a request to Google passes only while the signed-in person's consent is on file for the tier chosen now. Any
/// other request passes untouched. The consent file is read on every request, so taking the answer back works at once.
pub struct GuardedUpstream {
    inner: Arc<dyn Upstream>,
    dir: PathBuf,
    user: Arc<dyn Fn() -> String + Send + Sync>,
}

impl GuardedUpstream {
    pub fn new(inner: Arc<dyn Upstream>, dir: &Path, user: Arc<dyn Fn() -> String + Send + Sync>) -> GuardedUpstream {
        GuardedUpstream { inner, dir: dir.to_path_buf(), user }
    }

    pub fn check(&self, url: &str) -> Result<(), String> {
        if is_google_url(url) { allowed(&self.dir, &(self.user)()) } else { Ok(()) }
    }
}

/// The consent gate of the live voice session: the same [`GuardedUpstream`], over a transport that sends nothing (the session
/// asks [`GuardedUpstream::check_live`] before every byte it writes).
pub fn live_gate(dir: &Path, user: Arc<dyn Fn() -> String + Send + Sync>) -> GuardedUpstream {
    GuardedUpstream::new(Arc::new(NoRequests), dir, user)
}

impl GuardedUpstream {
    /// The gate for the live voice connection: its own yes (naming the open microphone and the tool results) must be on file.
    pub fn check_live(&self, url: &str) -> Result<(), String> {
        if is_google_url(url) { allowed_live(&self.dir, &(self.user)()) } else { Ok(()) }
    }
}

impl Upstream for GuardedUpstream {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        self.check(url)?;
        self.inner.request(method, url, headers, body)
    }

    fn request_full(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<(iyakki_proxy::UpstreamResponse, Vec<(String, String)>), String> {
        self.check(url)?;
        self.inner.request_full(method, url, headers, body)
    }
}

/// The same gate for the transport that reads an answer as it is written (the chat).
pub struct GuardedStreamer {
    inner: Arc<dyn iyakki_proxy::Streamer>,
    guard: GuardedUpstream,
}

struct NoRequests;
impl Upstream for NoRequests {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("not a request".into())
    }
}

impl GuardedStreamer {
    pub fn new(inner: Arc<dyn iyakki_proxy::Streamer>, dir: &Path, user: Arc<dyn Fn() -> String + Send + Sync>) -> GuardedStreamer {
        GuardedStreamer { inner, guard: GuardedUpstream::new(Arc::new(NoRequests), dir, user) }
    }
}

impl iyakki_proxy::Streamer for GuardedStreamer {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        self.guard.check(url)?;
        self.inner.open(url, headers)
    }

    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
        self.guard.check(url)?;
        self.inner.post(url, headers, body)
    }

    fn open_tls(&self, url: &str, headers: &[(String, String)], mode: &iyakki_proxy::TlsMode) -> Result<iyakki_proxy::StreamHead, String> {
        self.guard.check(url)?;
        self.inner.open_tls(url, headers, mode)
    }

    fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
        self.inner.peer_chain(host, port)
    }
}

// -------------------------------------------------------------------------------------------------------------- requests

/// Takes the key out of a text that is about to be shown or logged.
pub fn redact(text: &str, key: &str) -> String {
    if key.len() >= 8 { text.replace(key, "[key]") } else { text.to_string() }
}

fn model_path(model: &str) -> Result<String, String> {
    let m = model.trim().trim_start_matches("models/");
    if !valid_model_name(m) || m.contains('/') {
        return Err("the Gemini model name has a character that is not allowed".into());
    }
    Ok(format!("{API_BASE}/models/{m}:generateContent"))
}

/// How long Google asked to wait: the `retry-after` header (seconds), else the `retryDelay` of the error details (`"34s"`).
pub fn retry_after(headers: &[(String, String)], body: &[u8]) -> Option<u64> {
    if let Some(s) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("retry-after")).and_then(|(_, v)| v.trim().parse::<u64>().ok()) {
        return Some(s);
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    let details = v.pointer("/error/details")?.as_array()?;
    details.iter().find_map(|d| d.get("retryDelay").and_then(Value::as_str)).and_then(|s| s.trim_end_matches('s').parse::<f64>().ok()).map(|f| f.ceil() as u64)
}

/// A status and body in plain words (never the key). `429` is told apart by the caller.
pub fn explain(status: u16, body: &[u8], key: &str, tier: Tier) -> String {
    let said: String = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
    let said = crate::providererr::mask_ids(&redact(&said, key));
    let said: String = said.chars().filter(|c| !c.is_control()).take(160).collect();
    let l = said.to_ascii_lowercase();
    let head = match status {
        400 if l.contains("api key") => "Google does not accept the key. Type it again in Settings -> Models -> Google Gemini.".to_string(),
        400 if l.contains("location") => "Google Gemini is not offered in this country for this key.".to_string(),
        400 => "Google did not accept the request. Check the model name in Settings -> Models -> Google Gemini.".to_string(),
        401 | 403 => "Google refused the key. Type it again in Settings -> Models -> Google Gemini; the key may be wrong, switched off, or its project may not have access to this model.".to_string(),
        404 => "Google does not know that model. Check the model name in Settings -> Models -> Google Gemini.".to_string(),
        413 => "The recording is too big for one request. Record less.".to_string(),
        429 => limit_advice(tier).to_string(),
        500..=599 => "Google is having trouble right now. Try again in a little while.".to_string(),
        s => format!("Google answered with an unexpected status ({s})."),
    };
    if said.is_empty() { head } else { format!("{head} Google said: {said}") }
}

/// One `generateContent` request with the 429 rule: wait as Google asked (at most [`MAX_WAIT_SECS`], at most [`MAX_RETRIES`] times),
/// then say the limit is reached. `sleep` is a parameter so tests do not wait.
fn generate(model: &str, key_ref: &str, tier: Tier, body: &Value, secrets: &Secrets, up: &dyn Upstream, sleep: &dyn Fn(Duration)) -> Result<Value, String> {
    let url = model_path(model)?;
    let key = secrets.resolve(key_ref, None::<&LocalVault>).map_err(|e| format!("The Gemini key is not available ({e}). Type it in Settings -> Models -> Google Gemini."))?;
    let headers = vec![("Content-Type".to_string(), "application/json".to_string()), ("x-goog-api-key".to_string(), key.to_string())];
    let payload = body.to_string();
    let mut tries = 0;
    loop {
        let (r, rh) = up.request_full("POST", &url, &headers, payload.as_bytes()).map_err(|e| redact(&crate::providererr::plain_error("Google Gemini", &e), &key))?;
        if (200..300).contains(&r.status) {
            return serde_json::from_slice(&r.body).map_err(|_| "Google answered with something that is not JSON.".to_string());
        }
        if r.status == 429 && tries < MAX_RETRIES {
            let wait = retry_after(&rh, &r.body).unwrap_or(2 * (u64::from(tries) + 1));
            if wait <= MAX_WAIT_SECS {
                sleep(Duration::from_secs(wait.max(1)));
                tries += 1;
                continue;
            }
            return Err(format!("{} Google asked to wait about {wait} seconds.", explain(429, &r.body, &key, tier)));
        }
        return Err(explain(r.status, &r.body, &key, tier));
    }
}

/// The text of a `generateContent` answer, or why there is none.
pub fn answer_text(v: &Value) -> Result<String, String> {
    if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(format!("Google declined to handle this audio ({reason})."));
    }
    let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array);
    let text: String = parts.map(|p| p.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("")).unwrap_or_default();
    if text.trim().is_empty() {
        return match v.pointer("/candidates/0/finishReason").and_then(Value::as_str) {
            Some(r) if r != "STOP" => Err(format!("Google gave no text ({r}).")),
            _ if v.get("candidates").is_some() => Ok(String::new()),
            _ => Err("Google answered without any text.".into()),
        };
    }
    Ok(text)
}

fn instruction(language: Option<&str>) -> String {
    let mut s = "Transcribe this audio verbatim. Reply with the spoken words only: no commentary, no timestamps, no speaker labels, no quotation marks. If there is no speech, reply with nothing.".to_string();
    if let Some(l) = language.map(str::trim).filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        s.push_str(&format!(" The language spoken is {l}."));
    }
    s
}

/// Speech to text through Gemini: the WAV goes inline (base64) in one request. Over [`MAX_AUDIO_BYTES`] it is refused with a
/// plain message. The caller has already passed the consent gate (the transport is a [`GuardedUpstream`]).
pub fn transcribe(model: &str, language: Option<&str>, key_ref: &str, tier: Tier, wav: &[u8], secrets: &Secrets, up: &dyn Upstream) -> Result<String, String> {
    transcribe_with(model, language, key_ref, tier, wav, secrets, up, &std::thread::sleep)
}

#[allow(clippy::too_many_arguments)]
pub fn transcribe_with(model: &str, language: Option<&str>, key_ref: &str, tier: Tier, wav: &[u8], secrets: &Secrets, up: &dyn Upstream, sleep: &dyn Fn(Duration)) -> Result<String, String> {
    if wav.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    if wav.len() > MAX_AUDIO_BYTES {
        return Err(format!("The recording is too long for Google Gemini ({} MB; the limit is {} MB). Record less at a time.", wav.len() / (1024 * 1024), MAX_AUDIO_BYTES / (1024 * 1024)));
    }
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": instruction(language)}, {"inline_data": {"mime_type": "audio/wav", "data": crate::b64::base64_encode(wav)}}]}],
        "generationConfig": {"temperature": 0}
    });
    let v = generate(model, key_ref, tier, &body, secrets, up, sleep)?;
    Ok(answer_text(&v)?.trim().to_string())
}

/// The Test button: a tiny harmless text request. Returns Google's short answer.
pub fn ping(model: &str, key_ref: &str, tier: Tier, secrets: &Secrets, up: &dyn Upstream) -> Result<String, String> {
    let body = json!({"contents": [{"role": "user", "parts": [{"text": "Reply with the single word: OK"}]}], "generationConfig": {"temperature": 0, "maxOutputTokens": 16}});
    let v = generate(model, key_ref, tier, &body, secrets, up, &std::thread::sleep)?;
    Ok(answer_text(&v)?.chars().take(40).collect::<String>().trim().to_string())
}

// -------------------------------------------------------------------------------------------------------------- settings

/// A change from the Settings page or the assistant's `gemini_set`. The key is never part of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub tier: Option<Tier>,
    pub enabled: Option<bool>,
    pub chat_model: Option<String>,
    pub stt_model: Option<String>,
    /// Use Gemini as the chat model / for speech-to-text (false puts the earlier choice back).
    pub use_chat: Option<bool>,
    pub use_stt: Option<bool>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        *self == Change::default()
    }
}

/// The provider this module keeps in Settings -> Models.
pub fn provider_cfg(c: &Config, enabled: bool) -> ProviderCfg {
    let mut p = ProviderCfg::new(PROVIDER_ID, Kind::Compat);
    p.base_url = Some(OPENAI_BASE.into());
    p.key_ref = Some(KEY_REF.into());
    p.default_model = Some(c.chat_model.clone());
    p.enabled = enabled;
    p
}

pub fn stt_engine(c: &Config) -> SpeechEngine {
    SpeechEngine::Gemini { model: c.stt_model.clone(), language: None, key_ref: KEY_REF.into(), tier: c.tier }
}

fn names_gemini_stt(e: &Option<SpeechEngine>) -> bool {
    matches!(e, Some(SpeechEngine::Gemini { .. }))
}

/// Applies `ch` to the file and to the machine settings in one step; returns the new state. Turning it off puts back what the
/// chat model and the speech engine were. Nothing here sends anything.
pub fn apply(dir: &Path, settings: &crate::settings_ref::SettingsRef, ch: &Change) -> Result<Config, String> {
    for m in [&ch.chat_model, &ch.stt_model].into_iter().flatten() {
        let t = m.trim().trim_start_matches("models/");
        if !valid_model_name(t) || t.contains('/') {
            return Err("the model name may only have letters, digits and . _ : - + @".into());
        }
    }
    let mut st = settings.lock().unwrap_or_else(|e| e.into_inner());
    update(dir, |c| {
        if let Some(t) = ch.tier {
            c.tier = t;
        }
        if let Some(m) = &ch.chat_model {
            c.chat_model = m.trim().trim_start_matches("models/").to_string();
        }
        if let Some(m) = &ch.stt_model {
            c.stt_model = m.trim().trim_start_matches("models/").to_string();
        }
        if let Some(e) = ch.enabled {
            c.enabled = e;
        }
        if (ch.use_chat == Some(true) || ch.use_stt == Some(true)) && !c.enabled {
            return Err("turn Google Gemini on first".into());
        }
        let (use_chat, use_stt) = if !c.enabled { (Some(false), Some(false)) } else { (ch.use_chat, ch.use_stt) };
        let snapshot = c.clone();
        let mut next_prev_chat = c.prev_chat.clone();
        let mut next_prev_stt = c.prev_stt.clone();
        let r = st.update(|s| {
            let has = s.providers.iter().any(|p| p.id == PROVIDER_ID);
            if snapshot.enabled || has {
                let p = provider_cfg(&snapshot, snapshot.enabled);
                match s.providers.iter_mut().find(|x| x.id == PROVIDER_ID) {
                    Some(slot) => {
                        let models = std::mem::take(&mut slot.models);
                        *slot = p;
                        slot.models = models;
                    }
                    None => s.providers.push(p),
                }
            }
            let chat_is_ours = s.chat.as_ref().is_some_and(|m| m.provider == PROVIDER_ID);
            match use_chat {
                Some(true) => {
                    if !chat_is_ours {
                        next_prev_chat = s.chat.clone();
                    }
                    s.chat = Some(ModelChoice { provider: PROVIDER_ID.into(), model: snapshot.chat_model.clone() });
                }
                Some(false) if chat_is_ours => s.chat = next_prev_chat.take().filter(|m| m.provider != PROVIDER_ID),
                _ if chat_is_ours => s.chat = Some(ModelChoice { provider: PROVIDER_ID.into(), model: snapshot.chat_model.clone() }),
                _ => {}
            }
            let stt_is_ours = names_gemini_stt(&s.voice.stt);
            match use_stt {
                Some(true) => {
                    if !stt_is_ours {
                        next_prev_stt = s.voice.stt.clone();
                    }
                    s.voice.stt = Some(stt_engine(&snapshot));
                }
                Some(false) if stt_is_ours => s.voice.stt = next_prev_stt.take().filter(|e| !matches!(e, SpeechEngine::Gemini { .. })),
                _ if stt_is_ours => s.voice.stt = Some(stt_engine(&snapshot)),
                _ => {}
            }
        });
        r.map_err(|e| e.to_string())?;
        c.prev_chat = next_prev_chat;
        c.prev_stt = next_prev_stt;
        Ok(())
    })
}

/// The state in plain words for the Settings page and `gemini_status`. Never the key: only whether one is stored.
pub fn status_text(dir: &Path, user: &str, key_present: bool, settings: &crate::settings_ref::SettingsRef) -> String {
    let c = load(dir);
    let (chat_on, stt_on) = {
        let g = settings.lock().unwrap_or_else(|e| e.into_inner());
        (g.get().chat.as_ref().is_some_and(|m| m.provider == PROVIDER_ID), names_gemini_stt(&g.get().voice.stt))
    };
    let consent = match consent_state(dir, user) {
        ConsentState::Given { date } => format!("You said yes on {date} for the {} tier; nothing is sent without it, and you can take it back.", c.tier.label()),
        ConsentState::OtherTier { given, date } => format!("You said yes on {date} for the {} tier, but the {} tier is chosen now, so the question is asked again before anything is sent.", given.label(), c.tier.label()),
        ConsentState::Missing => "You have not said yes yet, so nothing is sent to Google.".to_string(),
    };
    format!(
        "Google Gemini (EXPERIMENTAL, optional, off by default): {}. Key: {} (the key is typed in Settings -> Models and is never shown). Tier: {} (only changes the notice and the advice when the limit is reached). Chat model: {} ({}). Speech-to-text: {} ({}). {consent} {}",
        if c.enabled { "on" } else { "off" },
        if key_present { "stored" } else { "not stored" },
        c.tier.label(),
        c.chat_model,
        if chat_on { "the assistant uses it" } else { "not used; the assistant uses its own choice" },
        c.stt_model,
        if stt_on { "speech-to-text uses it" } else { "not used; speech-to-text stays on this computer" },
        if c.tier == Tier::Free { FREE_LIMITS } else { "" }
    )
    .trim_end()
    .to_string()
}
