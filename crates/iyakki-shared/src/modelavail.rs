//! Which models of a provider the person's key can really use, learned from reality (docs/development/NVIDIA-MODELS.md).
//!
//! NVIDIA's `GET /v1/models` lists the whole catalog, also models the key cannot call; a call to one of those answers
//! `404 {"status":404,"title":"Not Found","detail":"Function '<id>': Not found for account '<id>'"}`. A chat or test call that fails
//! that way is remembered here for 24 hours (per key hash, provider and model) and the picker hides the model; a success clears it.
//! The file holds a salted hash of the key (never the key), the provider id, the model id and a time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// How long "this key cannot use this model" is believed.
pub const TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// How many models one "Check my models" run tries at most, and the gap between two requests (NVIDIA's free tier: about 40 a minute).
pub const CHECK_CAP: usize = 40;
pub const CHECK_GAP_MS: u64 = 1600;
const FILE: &str = "model-availability.json";

/// Words in a model id that mark it as not a chat/completion model (embedding, ranking, safety, parsing, vision encoders ...).
/// Matched on the lower-case id; one place to extend.
pub const NOT_CHAT: &[&str] = &[
    // embeddings, ranking, retrieval, safety, reward, parsing
    "embed", "rerank", "ranking", "reward", "guard", "safety", "parse", "retriev", "clip", "bge-", "calibration", "content-safety", "topic-control", "jailbreak", "pii", "gliner",
    // vision-only, detectors, document and video models
    "detector", "cosmos", "neva", "vila", "kosmos", "deplot", "paligemma", "fuyu", "vision", "-vl", "vlm", "paddleocr", "-ocr", "synthetic", "streampetr", "bevformer",
    // speech, translation, pictures
    "translate", "riva", "whisper", "parakeet", "canary", "fastpitch", "diffusion", "sdxl", "flux",
];

/// Whether a model id looks like a chat/completion model.
pub fn is_chat_model(id: &str) -> bool {
    let l = id.to_ascii_lowercase();
    !NOT_CHAT.iter().any(|w| l.contains(w))
}

/// The chat models only, NVIDIA's own (`nvidia/`, the Nemotron family) first, each part in alphabetical order.
pub fn chat_listing(mut ids: Vec<String>) -> Vec<String> {
    ids.retain(|m| is_chat_model(m));
    ids.sort_by(|a, b| (!a.starts_with("nvidia/"), a).cmp(&(!b.starts_with("nvidia/"), b)));
    ids
}

/// The answer that says "this account cannot use this model": 404 (or 403) AND a body that says so. A bare 404 or 403 is
/// something else (a wrong address, a blocked network) and proves nothing about the model.
pub fn is_unavailable_error(e: &str) -> bool {
    (e.starts_with("HTTP 404") || e.starts_with("HTTP 403")) && e.to_ascii_lowercase().contains("not found for account")
}

/// A rate-limit answer.
pub fn is_rate_limited(e: &str) -> bool {
    e.starts_with("HTTP 429")
}

/// A salted, shortened hash of the key: enough to tell two keys apart, useless to get the key back.
pub fn key_hash(key: &str) -> String {
    let d = Sha256::digest(format!("iyakki-model-availability\0{key}").as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn entry(hash: &str, provider: &str, model: &str) -> String {
    format!("{hash}|{provider}|{model}")
}

fn read(dir: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(path(dir)).ok().and_then(|t| serde_json::from_str::<BTreeMap<String, u64>>(&t).ok()).unwrap_or_default()
}

fn write(dir: &Path, m: &BTreeMap<String, u64>) {
    let p = path(dir);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string(m).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Remember that the key cannot use the model (`usable` false), or forget that (`usable` true: a call worked). Old entries go.
pub fn note(dir: &Path, hash: &str, provider: &str, model: &str, usable: bool, now_ms: u64) {
    let mut m = read(dir);
    let before = m.len();
    m.retain(|_, at| now_ms.saturating_sub(*at) < TTL_MS);
    let k = entry(hash, provider, model);
    let changed = if usable { m.remove(&k).is_some() } else { m.insert(k, now_ms).map_or(true, |t| t != now_ms) } || m.len() != before;
    if changed {
        write(dir, &m);
    }
}

/// The models of the provider the key could not use within the last 24 hours.
pub fn hidden(dir: &Path, hash: &str, provider: &str, now_ms: u64) -> Vec<String> {
    let prefix = entry(hash, provider, "");
    read(dir).into_iter().filter(|(k, at)| now_ms.saturating_sub(*at) < TTL_MS && k.starts_with(&prefix)).map(|(k, _)| k[prefix.len()..].to_string()).collect()
}

/// What a "Check my models" run found.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub usable: Vec<String>,
    pub unavailable: Vec<String>,
    /// Models whose probe failed for another reason (model, text).
    pub failed: Vec<(String, String)>,
    /// The run stopped at a rate-limit answer.
    pub rate_limited: bool,
    /// Candidates left unchecked because of the cap or the stop.
    pub skipped: usize,
}

impl Report {
    /// The result in plain words.
    pub fn words(&self) -> String {
        let mut s = format!("Checked {} models: {} work with your key, {} are not available to your key", self.usable.len() + self.unavailable.len() + self.failed.len(), self.usable.len(), self.unavailable.len());
        if !self.failed.is_empty() {
            s.push_str(&format!(", {} could not be checked ({})", self.failed.len(), self.failed.iter().map(|(m, e)| format!("{m}: {e}")).take(3).collect::<Vec<_>>().join("; ")));
        }
        s.push('.');
        if self.rate_limited {
            s.push_str(&format!(" NVIDIA said you are sending too many requests, so the check stopped; {} models were not checked. Wait a minute and run it again.", self.skipped));
        } else if self.skipped > 0 {
            s.push_str(&format!(" {} more models were not checked (the check does at most the cap you gave at a time).", self.skipped));
        }
        if !self.unavailable.is_empty() {
            s.push_str(&format!(" Hidden from the model list: {}.", self.unavailable.join(", ")));
        }
        s
    }
}
