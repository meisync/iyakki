//! The assistant's radio tools: "play a tamil online fm" in the chat finds a station and plays it in the same player
//! the RADIO app uses (so the app shows what plays, and its stop key stops it).
//!
//! Stations come from three places, searched together: the person's own saved stations, the built-in list by language
//! (`radio_stations`), and the community Radio Browser directory on the internet (`Directory`). The directory is only
//! asked through an injectable [`Fetcher`] (so tests never need the network), only on hosts the account may reach,
//! with short timeouts, and results are kept in memory for an hour. Offline, or when the account may not reach the
//! directory, the built-in and saved stations still work. Only MP3 streams are kept, because that is what the
//! player can play; dead stations and HLS playlists are dropped.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_proxy::Upstream;
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::radio::{self, RadioState, Station, StationStore, LANGUAGES};
use crate::services::Services;

/// How long a directory answer is reused.
pub const CACHE_SECS: u64 = 3600;
/// The community directory: several mirrors, tried in this order.
pub const MIRRORS: &[&str] = &["https://de1.api.radio-browser.info", "https://all.api.radio-browser.info", "https://nl1.api.radio-browser.info", "https://at1.api.radio-browser.info"];
/// How many mirrors are tried before the directory counts as unreachable.
const MAX_MIRROR_TRIES: usize = 3;
/// How long one station gets to start playing.
const START_WAIT: Duration = Duration::from_secs(8);
/// How many stations `radio_play` tries before giving up.
const MAX_TRIES: usize = 4;

// ------------------------------------------------------------------ stations found

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Saved,
    BuiltIn,
    Directory,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Saved => "saved",
            Source::BuiltIn => "built-in",
            Source::Directory => "directory",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub url: String,
    pub source: Source,
    pub language: String,
    pub kbps: u32,
    pub tags: String,
    pub country: String,
    pub score: f64,
}

impl Found {
    fn facts(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if !self.language.is_empty() {
            v.push(self.language.clone());
        }
        if self.kbps > 0 {
            v.push(format!("{} kbps", self.kbps));
        }
        v.join(", ")
    }

    fn line(&self) -> String {
        let mut extra = vec![self.source.label().to_string()];
        if !self.country.is_empty() {
            extra.push(self.country.clone());
        }
        let facts = self.facts();
        format!("[{}] {}{} ({})", self.id, self.name, if facts.is_empty() { String::new() } else { format!(" — {facts}") }, extra.join(", "))
    }
}

fn slug(s: &str) -> String {
    let t: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    t.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// A station name from outside, made safe to show a model: one line, no control characters, not too long.
fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(60).collect()
}

/// Whether the player can play this address: no HLS playlist, and not a file type it does not decode.
fn playable_url(u: &str) -> bool {
    if radio::valid_url(u).is_err() {
        return false;
    }
    let path = u.split(['?', '#']).next().unwrap_or(u).to_ascii_lowercase();
    ![".m3u8", ".aac", ".ogg", ".opus", ".mp4", ".flac", ".m4a", ".aacp", ".wma"].iter().any(|e| path.ends_with(e))
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace(['’', '‘'], "'")
}

// ------------------------------------------------------------------ understanding what was asked

/// What a request like "play a tamil online fm" means: a language, genre tags, and the words left for the name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub words: Vec<String>,
    pub raw: String,
}

const NOISE: &[&str] = &[
    "play", "a", "an", "the", "some", "me", "please", "online", "live", "fm", "radio", "radios", "station", "stations", "channel", "channels", "internet", "web", "stream", "streaming", "music", "songs", "song", "on", "to", "listen", "put", "start", "find", "for", "i", "want", "can", "you", "tune", "in", "of", "and", "my", "any", "good", "best", "new", "something", "with", "from", "turn", "tamil-fm",
];

const GENRES: &[&str] = &["news", "pop", "rock", "jazz", "classical", "devotional", "melody", "hits", "kuthu", "oldies", "folk", "hiphop", "dance", "sports", "talk", "rap", "electronic", "lofi", "metal", "country", "blues", "reggae", "ambient", "christian", "comedy", "carnatic"];

const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("kollywood", "Tamil"),
    ("tamizh", "Tamil"),
    ("தமிழ்", "Tamil"),
    ("தமிழ", "Tamil"),
    ("bollywood", "Hindi"),
    ("हिंदी", "Hindi"),
    ("हिन्दी", "Hindi"),
    ("tollywood", "Telugu"),
    ("తెలుగు", "Telugu"),
    ("mollywood", "Malayalam"),
    ("മലയാളം", "Malayalam"),
    ("sandalwood", "Kannada"),
    ("ಕನ್ನಡ", "Kannada"),
    ("español", "Spanish"),
    ("francais", "French"),
    ("français", "French"),
    ("deutsch", "German"),
];

fn canonical_language(word: &str) -> Option<String> {
    let w = word.to_lowercase();
    if let Some((_, l)) = LANGUAGE_ALIASES.iter().find(|(a, _)| *a == w) {
        return Some((*l).to_string());
    }
    LANGUAGES.iter().find(|l| l.name.to_lowercase() == w).map(|l| l.name.to_string())
}

fn canonical_tag(word: &str) -> Option<String> {
    let w = match word {
        "kutthu" | "koothu" | "kuthu" => "kuthu",
        "hit" => "hits",
        "bhakti" | "bhajan" | "spiritual" | "prayer" => "devotional",
        "oldie" | "retro" | "golden" => "oldies",
        "hip-hop" => "hiphop",
        "sport" => "sports",
        other => other,
    };
    GENRES.contains(&w).then(|| w.to_string())
}

impl Query {
    pub fn parse(raw: &str, language: Option<&str>, tag: Option<&str>) -> Query {
        let mut q = Query { raw: raw.trim().to_string(), ..Query::default() };
        let trim = |c: char| c.is_ascii_punctuation() || matches!(c, '“' | '”' | '‘' | '’' | '«' | '»' | '…' | '।');
        if let Some(l) = language.map(str::trim).filter(|l| !l.is_empty()) {
            q.language = Some(canonical_language(l).unwrap_or_else(|| l.to_string()));
        }
        if let Some(t) = tag.map(str::trim).filter(|t| !t.is_empty()) {
            let t = t.to_lowercase();
            q.tags.push(canonical_tag(&t).unwrap_or(t));
        }
        for w in raw.split_whitespace().map(|w| w.trim_matches(trim)).filter(|w| !w.is_empty()) {
            let lw = w.to_lowercase();
            if let Some(l) = canonical_language(w) {
                // "bollywood" and "kollywood" say a language; the first one named wins
                if q.language.is_none() {
                    q.language = Some(l);
                }
            } else if let Some(t) = canonical_tag(&lw) {
                if !q.tags.contains(&t) {
                    q.tags.push(t);
                }
            } else if !NOISE.contains(&lw.as_str()) && !q.words.contains(&lw) {
                q.words.push(lw);
            }
        }
        q
    }

    fn is_empty(&self) -> bool {
        self.language.is_none() && self.tags.is_empty() && self.words.is_empty()
    }

    fn name_matches(&self, name: &str) -> bool {
        let n = norm(name);
        !self.words.is_empty() && self.words.iter().all(|w| n.contains(&norm(w)))
    }
}

// ------------------------------------------------------------------ the online directory

/// How the directory is asked: one GET returning the body. Tests give a fake.
pub trait Fetcher: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
}

/// The real thing: the system's short-timeout web client.
pub struct UpstreamFetcher(pub Arc<dyn Upstream>);

impl Fetcher for UpstreamFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        let r = self.0.request("GET", url, &[("User-Agent".into(), "Iyakki/0.1".into()), ("Accept".into(), "application/json".into())], b"")?;
        if !(200..300).contains(&r.status) {
            return Err(format!("HTTP {}", r.status));
        }
        let body: Vec<u8> = r.body.into_iter().take(2 * 1024 * 1024).collect();
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirError {
    /// The account may not reach any directory host.
    Refused,
    /// No mirror answered.
    Offline(String),
}

fn percent_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn search_path(language: Option<&str>, tag: Option<&str>, name: Option<&str>) -> String {
    let mut p = String::from("/json/stations/search?");
    if let Some(l) = language {
        p.push_str(&format!("language={}&", percent_encode(&l.to_lowercase())));
    }
    if let Some(t) = tag {
        p.push_str(&format!("tag={}&", percent_encode(t)));
    }
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        p.push_str(&format!("name={}&", percent_encode(n)));
    }
    p.push_str("codec=MP3&hidebroken=true&order=clickcount&reverse=true&limit=40");
    p
}

/// The directory's stations out of its JSON answer: working MP3 streams only, best first.
pub fn parse_stations(json_text: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("the directory sent something unreadable: {e}"))?;
    let arr = v.as_array().ok_or("the directory sent something unexpected")?;
    let num = |s: &Value, k: &str| s.get(k).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f.max(0.0) as u64))).unwrap_or(0);
    let text = |s: &Value, k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut out: Vec<Found> = Vec::new();
    for s in arr {
        let ok = s.get("lastcheckok").is_some_and(|x| x.as_i64() == Some(1) || x.as_bool() == Some(true));
        let codec = text(s, "codec").to_lowercase();
        if !ok || !codec.contains("mp3") || s.get("hls").and_then(Value::as_i64) == Some(1) {
            continue;
        }
        let url = Some(text(s, "url_resolved")).filter(|u| !u.is_empty()).unwrap_or_else(|| text(s, "url"));
        let name = clean_name(&text(s, "name"));
        if name.is_empty() || !playable_url(&url) || out.iter().any(|f| f.url == url || norm(&f.name) == norm(&name)) {
            continue;
        }
        let kbps = num(s, "bitrate") as u32;
        let (clicks, votes) = (num(s, "clickcount") as f64, num(s, "votes") as f64);
        let bonus = if (64..=320).contains(&kbps) { kbps.min(192) as f64 / 96.0 } else { 0.0 };
        let language = text(s, "language");
        let language = language.split(',').next().unwrap_or("").trim().to_string();
        let language: String = language.chars().take(1).flat_map(char::to_uppercase).chain(language.chars().skip(1)).collect();
        out.push(Found {
            id: format!("o-{:08x}", fnv(&url)),
            name,
            url,
            source: Source::Directory,
            language,
            kbps,
            tags: text(s, "tags").chars().take(80).collect(),
            country: text(s, "countrycode"),
            score: (1.0 + clicks).ln() + (1.0 + votes).ln() + bonus,
        });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// The online directory with an hour of memory. One per system (in `Services`), shared by every account; whether an
/// account may use it is decided per call.
pub struct Directory {
    fetcher: Mutex<Arc<dyn Fetcher>>,
    cache: Mutex<HashMap<String, (Instant, Vec<Found>)>>,
    now: Mutex<Option<Found>>,
}

impl Directory {
    pub fn new(fetcher: Arc<dyn Fetcher>) -> Directory {
        Directory { fetcher: Mutex::new(fetcher), cache: Mutex::new(HashMap::new()), now: Mutex::new(None) }
    }

    pub fn set_fetcher(&self, f: Arc<dyn Fetcher>) {
        *self.fetcher.lock().unwrap() = f;
        self.cache.lock().unwrap().clear();
    }

    fn fetch(&self, path: &str, allowed: &dyn Fn(&str) -> bool) -> Result<Vec<Found>, DirError> {
        if let Some((at, v)) = self.cache.lock().unwrap().get(path) {
            if at.elapsed() < Duration::from_secs(CACHE_SECS) {
                return Ok(v.clone());
            }
        }
        let mirrors: Vec<&str> = MIRRORS.iter().copied().filter(|m| allowed(radio::host_of(m).as_str())).collect();
        if mirrors.is_empty() {
            return Err(DirError::Refused);
        }
        let fetcher = self.fetcher.lock().unwrap().clone();
        let mut last = String::from("no answer");
        for m in mirrors.into_iter().take(MAX_MIRROR_TRIES) {
            match fetcher.get(&format!("{m}{path}")).and_then(|body| parse_stations(&body)) {
                Ok(v) => {
                    self.cache.lock().unwrap().insert(path.to_string(), (Instant::now(), v.clone()));
                    return Ok(v);
                }
                Err(e) => last = e,
            }
        }
        Err(DirError::Offline(last))
    }

    /// Stations matching the request, best first.
    pub fn search(&self, q: &Query, allowed: &dyn Fn(&str) -> bool) -> Result<Vec<Found>, DirError> {
        let lang = q.language.as_deref();
        let words = q.words.join(" ");
        let name = Some(words.as_str()).filter(|w| !w.is_empty());
        let mut paths: Vec<String> = Vec::new();
        if let Some(t) = q.tags.first() {
            paths.push(search_path(lang, Some(t), name));
            // a station called "Kuthu FM" carries no such tag: look by name too
            let both = if words.is_empty() { t.clone() } else { format!("{words} {t}") };
            paths.push(search_path(lang, None, Some(&both)));
        } else if lang.is_some() || name.is_some() {
            paths.push(search_path(lang, None, name));
        }
        if paths.is_empty() {
            return Ok(vec![]);
        }
        let mut merged: Vec<Found> = Vec::new();
        let mut err: Option<DirError> = None;
        let mut any_ok = false;
        for p in &paths {
            match self.fetch(p, allowed) {
                Ok(v) => {
                    any_ok = true;
                    for f in v {
                        if !merged.iter().any(|m| m.url == f.url) {
                            merged.push(f);
                        }
                    }
                }
                Err(e) => err = Some(e),
            }
        }
        if !any_ok {
            return Err(err.unwrap_or(DirError::Offline("no answer".into())));
        }
        if merged.is_empty() && lang.is_some() && name.is_some() {
            // the words were too narrow: the language alone
            merged = self.fetch(&search_path(lang, None, None), allowed).unwrap_or_default();
        }
        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(merged)
    }

    fn by_id(&self, id: &str) -> Option<Found> {
        self.cache.lock().unwrap().values().flat_map(|(_, v)| v.iter()).find(|f| f.id == id).cloned()
    }

    fn by_name(&self, name: &str) -> Option<Found> {
        let n = norm(name);
        self.cache.lock().unwrap().values().flat_map(|(_, v)| v.iter()).find(|f| norm(&f.name) == n).cloned()
    }

    /// The station the assistant started last.
    pub fn now_playing(&self) -> Option<Found> {
        self.now.lock().unwrap().clone()
    }
}

// ------------------------------------------------------------------ the built-in and saved lists

fn builtin_all() -> Vec<Found> {
    let mut v = Vec::new();
    for l in LANGUAGES {
        for (i, (name, url)) in l.stations.iter().enumerate() {
            if playable_url(url) {
                v.push(Found { id: format!("b-{}-{}", slug(l.name), i + 1), name: clean_name(name), url: url.to_string(), source: Source::BuiltIn, language: l.name.to_string(), kbps: 0, tags: String::new(), country: String::new(), score: 0.0 });
            }
        }
    }
    v
}

fn saved_all(store: &StationStore) -> Vec<Found> {
    store.list().into_iter().map(|s| Found { id: format!("s-{}", slug(&s.name)), name: clean_name(&s.name), url: s.url, source: Source::Saved, language: String::new(), kbps: 0, tags: String::new(), country: String::new(), score: 0.0 }).collect()
}

fn builtin_matches(q: &Query) -> Vec<Found> {
    let all = builtin_all();
    if q.is_empty() {
        let first = LANGUAGES.first().map_or("", |l| l.name);
        return all.into_iter().filter(|f| f.language == first).collect();
    }
    let in_lang: Vec<Found> = match &q.language {
        Some(l) => all.iter().filter(|f| f.language.eq_ignore_ascii_case(l)).cloned().collect(),
        None => all,
    };
    let by_words = |f: &&Found| q.words.is_empty() || q.name_matches(&f.name);
    let by_tags = |f: &&Found| q.tags.iter().all(|t| norm(&f.name).contains(t.as_str()));
    let strict: Vec<Found> = in_lang.iter().filter(by_words).filter(by_tags).cloned().collect();
    if !strict.is_empty() {
        return strict;
    }
    // a language with a genre the list does not know still gives that language's stations
    if q.language.is_some() { in_lang.iter().filter(by_words).cloned().collect() } else { vec![] }
}

fn saved_matches(store: &StationStore, q: &Query) -> Vec<Found> {
    let all = saved_all(store);
    if q.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|f| {
            let n = norm(&f.name);
            q.name_matches(&f.name) || q.language.as_ref().is_some_and(|l| n.contains(&l.to_lowercase())) || (q.words.is_empty() && q.tags.iter().any(|t| n.contains(t.as_str())))
        })
        .collect()
}

/// Everything that matches, the person's own first, then the built-in list, then the directory. The second value says
/// why the directory is missing, when it is.
fn search_all(svc: &Services, p: &Policy, user: &str, q: &Query) -> (Vec<Found>, Option<String>) {
    let store = StationStore::new(svc.dir(), user);
    let mut out: Vec<Found> = Vec::new();
    let push = |v: Vec<Found>, out: &mut Vec<Found>| {
        for f in v {
            if !out.iter().any(|o| o.url == f.url || (norm(&o.name) == norm(&f.name) && o.source == f.source)) {
                out.push(f);
            }
        }
    };
    push(saved_matches(&store, q), &mut out);
    push(builtin_matches(q), &mut out);
    let mut note = None;
    if !q.is_empty() {
        match svc.radio_dir.search(q, &|h| p.allows_host(h)) {
            Ok(v) => push(v, &mut out),
            Err(DirError::Refused) => note = Some("Your account may not reach the online radio directory, so only built-in and saved stations are listed.".to_string()),
            Err(DirError::Offline(_)) => note = Some("No internet: the online radio directory could not be reached, so only built-in and saved stations are listed.".to_string()),
        }
    }
    // a name that is exactly a station's name is the one meant
    let exact = norm(q.raw.trim());
    if let Some(i) = out.iter().position(|f| norm(&f.name) == exact) {
        let f = out.remove(i);
        out.insert(0, f);
    }
    (out, note)
}

fn find_by_id(svc: &Services, user: &str, id: &str) -> Option<Found> {
    saved_all(&StationStore::new(svc.dir(), user)).into_iter().chain(builtin_all()).find(|f| f.id == id).or_else(|| svc.radio_dir.by_id(id))
}

fn find_by_name(svc: &Services, user: &str, name: &str) -> Option<Found> {
    let n = norm(name.trim());
    saved_all(&StationStore::new(svc.dir(), user)).into_iter().chain(builtin_all()).find(|f| norm(&f.name) == n).or_else(|| svc.radio_dir.by_name(name))
}

// ------------------------------------------------------------------ the tools

pub fn offered(p: &Policy) -> bool {
    p.apps.contains(&App::Radio)
}

pub fn is_radio_tool(name: &str) -> bool {
    matches!(name, "radio_search" | "radio_play" | "radio_stop" | "radio_pause" | "radio_resume" | "radio_volume" | "radio_save" | "radio_status")
}

/// The tool definitions for an account (none without the RADIO app).
pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !offered(p) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let tool = |name: &str, desc: &str, schema: Value, untrusted: bool| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Safe, untrusted };
    vec![
        tool(
            "radio_search",
            "Search internet radio stations: the person's saved stations, IYAKKI's built-in list by language, and the online Radio Browser directory (when there is internet). `query` is free text such as \"tamil\", \"tamil fm\", \"kuthu\", \"news\", \"bbc\" or \"jazz\"; words like fm, radio, online and live are ignored. Optional `language` (Tamil, Hindi, ...), `tag` (a genre such as news or devotional) and `limit` (default 8, at most 10). Returns numbered stations with an id for radio_play. Read-only; only MP3 stations that were working at the last check are listed.",
            obj(json!({"query": {"type": "string"}, "language": {"type": "string"}, "tag": {"type": "string"}, "limit": {"type": "integer"}}), &[]),
            true,
        ),
        tool(
            "radio_play",
            "Play a radio station in IYAKKI's own player (the same one as the RADIO app, which shows it). Give the `id` from radio_search, or a station `name`, or just a `query` like \"tamil online fm\" or \"news radio\" to play the best working match (stations that fail to start are skipped). If several stations match, just play the top one and tell the person which. Replaces anything already playing.",
            obj(json!({"id": {"type": "string"}, "name": {"type": "string"}, "query": {"type": "string"}, "language": {"type": "string"}}), &[]),
            true,
        ),
        tool("radio_stop", "Stop the radio.", obj(json!({}), &[]), false),
        tool("radio_pause", "Pause the radio; radio_resume carries on (live stations continue live).", obj(json!({}), &[]), false),
        tool("radio_resume", "Carry on with a paused radio.", obj(json!({}), &[]), false),
        tool("radio_volume", "Set the radio volume: `level` 0 to 100, or `change` (for example 10 or -10) from where it is now.", obj(json!({"level": {"type": "integer"}, "change": {"type": "integer"}}), &[]), false),
        tool("radio_status", "What the radio is playing now (station, song title when the station sends one, volume).", obj(json!({}), &[]), true),
        tool("radio_save", "Keep a station in the person's own list (My stations in the RADIO app). `name` is a station name or id from radio_search; leave it out to keep the station that is playing now.", obj(json!({"name": {"type": "string"}}), &[]), false),
    ]
}

/// The one-line meaning of a call, for the tool log and approval prompts.
pub fn describe(name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    match name {
        "radio_play" => Some(format!("play the radio: {}", s("id").or_else(|| s("name")).or_else(|| s("query")).unwrap_or("the best match"))),
        "radio_search" => Some(format!("search radio stations for “{}”", s("query").or_else(|| s("language")).or_else(|| s("tag")).unwrap_or("everything"))),
        "radio_save" => Some(format!("keep {} in My stations", s("name").unwrap_or("the playing station"))),
        _ => None,
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, user: &str) -> Result<String, String> {
    if !offered(p) {
        return Err("your account may not use the radio".into());
    }
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let radio = &svc.radio;
    match name {
        "radio_search" => {
            let q = Query::parse(s("query").unwrap_or(""), s("language"), s("tag"));
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(8).clamp(1, 10) as usize;
            let (found, note) = search_all(svc, p, user, &q);
            if found.is_empty() {
                let mut m = format!("No radio station matches “{}”.", q.raw);
                if let Some(n) = note {
                    m.push(' ');
                    m.push_str(&n);
                }
                return Ok(m);
            }
            let total = found.len();
            let mut out = format!("{} station{} for “{}”{}:\n", total.min(limit), if total.min(limit) == 1 { "" } else { "s" }, q.raw, if q.language.is_some() { format!(" ({})", q.language.as_deref().unwrap_or("")) } else { String::new() });
            for (i, f) in found.iter().take(limit).enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, f.line()));
            }
            if let Some(n) = note {
                out.push_str(&format!("{n}\n"));
            }
            out.push_str("Play one with radio_play (its id). If the person did not choose, play the first and say which one it is.");
            Ok(out)
        }
        "radio_play" => {
            let (cands, note, wanted) = if let Some(id) = s("id") {
                let f = find_by_id(svc, user, id).ok_or_else(|| format!("no station with the id {id}; run radio_search first"))?;
                (vec![f], None, id.to_string())
            } else if let Some(q) = s("name").or_else(|| s("query")) {
                let query = Query::parse(q, s("language"), None);
                let (found, note) = search_all(svc, p, user, &query);
                (found, note, q.to_string())
            } else if let Some(l) = s("language") {
                let (found, note) = search_all(svc, p, user, &Query::parse("", Some(l), None));
                (found, note, l.to_string())
            } else {
                let (found, note) = search_all(svc, p, user, &Query::default());
                (found, note, "radio".to_string())
            };
            if cands.is_empty() {
                return Err(format!("No radio station matches “{wanted}”.{}", note.map_or(String::new(), |n| format!(" {n}"))));
            }
            let mut refused: Vec<String> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            let mut tried = 0;
            for (i, c) in cands.iter().enumerate() {
                let host = radio::host_of(&c.url);
                if !p.allows_host(&host) {
                    if !refused.contains(&host) {
                        refused.push(host);
                    }
                    continue;
                }
                if tried >= MAX_TRIES {
                    break;
                }
                tried += 1;
                match start_and_wait(svc, p, user, c) {
                    Ok(kbps) => {
                        *svc.radio_dir.now.lock().unwrap() = Some(c.clone());
                        let mut shown = c.clone();
                        if shown.kbps == 0 {
                            shown.kbps = kbps;
                        }
                        let mut m = format!("Playing {}", c.name);
                        let facts = shown.facts();
                        if !facts.is_empty() {
                            m.push_str(&format!(" ({facts})"));
                        }
                        m.push_str(&format!(" from the {} list. The RADIO app shows it; radio_stop stops it.", c.source.label()));
                        let others: Vec<&str> = cands.iter().skip(i + 1).filter(|o| !refused.contains(&radio::host_of(&o.url))).take(3).map(|o| o.name.as_str()).collect();
                        if !others.is_empty() {
                            m.push_str(&format!(" Other matches: {}.", others.join(", ")));
                        }
                        if !failed.is_empty() {
                            m.push_str(&format!(" Skipped (would not start): {}.", failed.join("; ")));
                        }
                        if let Some(n) = note {
                            m.push_str(&format!(" {n}"));
                        }
                        return Ok(m);
                    }
                    Err(why) => failed.push(format!("{}: {why}", c.name)),
                }
            }
            if failed.is_empty() && !refused.is_empty() {
                return Err(format!("your account may not reach {}", refused.join(", ")));
            }
            Err(format!("Could not start a station for “{wanted}”: {}.{}", failed.join("; "), if refused.is_empty() { String::new() } else { format!(" Not allowed to reach: {}.", refused.join(", ")) }))
        }
        "radio_stop" => {
            let was = radio.is_active();
            radio.stop();
            Ok(if was { "Stopped the radio.".into() } else { "The radio was not playing.".into() })
        }
        "radio_pause" => {
            if radio.pause() {
                Ok("Paused the radio. Say \"resume\" to carry on.".into())
            } else if radio.is_paused() {
                Ok("The radio is already paused.".into())
            } else {
                Err("The radio is not playing.".into())
            }
        }
        "radio_resume" => {
            if radio.resume() { Ok("The radio plays again.".into()) } else { Err("The radio is not paused.".into()) }
        }
        "radio_volume" => {
            let now = radio.volume() as i64;
            let level = match (args.get("level").and_then(Value::as_i64), args.get("change").and_then(Value::as_i64)) {
                (Some(l), _) => l,
                (None, Some(c)) => now + c,
                _ => return Err("give a `level` from 0 to 100, or a `change` such as 10 or -10".into()),
            };
            radio.set_volume(level.clamp(0, 100) as u8);
            Ok(format!("Radio volume is {}%.", radio.volume()))
        }
        "radio_status" => Ok(match radio.state() {
            RadioState::Idle => "The radio is not playing.".into(),
            RadioState::Connecting(n) => format!("Connecting to {}.", clean_name(&n)),
            RadioState::Playing { station, title, kbps } => format!(
                "{} {}{}{}. Volume {}%.",
                if radio.is_paused() { "Paused:" } else { "Playing" },
                clean_name(&station),
                if kbps > 0 { format!(" ({kbps} kbps)") } else { String::new() },
                if title.is_empty() { String::new() } else { format!(", now: {}", clean_name(&title)) },
                radio.volume()
            ),
            RadioState::Failed { station, why } => format!("{} would not play: {why}", clean_name(&station)),
        }),
        "radio_save" => {
            let target = match s("name") {
                Some(n) => find_by_id(svc, user, n).or_else(|| find_by_name(svc, user, n)).ok_or_else(|| format!("I do not know a station called “{n}”; run radio_search first"))?,
                None => match svc.radio_dir.now_playing() {
                    Some(f) => f,
                    None => match radio.state() {
                        RadioState::Playing { station, .. } | RadioState::Connecting(station) => find_by_name(svc, user, &station).ok_or("say which station to keep (a name or id from radio_search)")?,
                        _ => return Err("nothing is playing; say which station to keep (a name or id from radio_search)".into()),
                    },
                },
            };
            if target.source == Source::Saved {
                return Ok(format!("{} is already in My stations.", target.name));
            }
            let name: String = target.name.chars().take(40).collect();
            let st = StationStore::new(svc.dir(), user).add(&name, &target.url)?;
            svc.toollog.record(svc.tool_log_days(), user, "radio.add", &radio::host_of(&st.url), true, "");
            Ok(format!("Kept {} in My stations (RADIO app).", st.name))
        }
        _ => Err(format!("unknown radio tool {name}")),
    }
}

/// Starts one station and waits until it plays or fails. The audit entry names the host only.
fn start_and_wait(svc: &Services, p: &Policy, user: &str, c: &Found) -> Result<u32, String> {
    svc.toollog.record(svc.tool_log_days(), user, "radio.play", &radio::host_of(&c.url), true, "");
    let pol = p.clone();
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |h| pol.allows_host(h));
    svc.radio.play(&Station { name: c.name.clone(), url: c.url.clone() }, allow, &svc.voice_settings().play_cmd);
    let t0 = Instant::now();
    loop {
        match svc.radio.state() {
            RadioState::Playing { kbps, .. } => return Ok(kbps),
            RadioState::Failed { why, .. } => return Err(why),
            _ => {}
        }
        if t0.elapsed() > START_WAIT {
            svc.radio.stop();
            return Err(format!("did not start within {} seconds", START_WAIT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{start, RunEvent, RunLimits, Tools, UserTools, AGENT_SYSTEM_PROMPT};
    use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall};
    use iyakki_proxy::{StreamHead, Streamer, UpstreamResponse};
    use iyakki_sec::policy::Role;
    use std::collections::VecDeque;

    struct Dead;
    impl Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
            Err("no internet".into())
        }
    }

    /// The directory: the fixture for any search, or "offline".
    struct FakeDir {
        body: Mutex<Option<String>>,
        asked: Mutex<Vec<String>>,
    }
    impl FakeDir {
        fn new(body: Option<&str>) -> Arc<FakeDir> {
            Arc::new(FakeDir { body: Mutex::new(body.map(str::to_string)), asked: Mutex::new(vec![]) })
        }
    }
    impl Fetcher for FakeDir {
        fn get(&self, url: &str) -> Result<String, String> {
            self.asked.lock().unwrap().push(url.to_string());
            self.body.lock().unwrap().clone().ok_or_else(|| "no internet".into())
        }
    }

    /// Streams: silent MP3 for every address except ones containing "dead".
    struct Streams;
    impl Streamer for Streams {
        fn open(&self, url: &str, _: &[(String, String)]) -> Result<StreamHead, String> {
            if url.contains("dead") {
                return Err("connection refused".into());
            }
            let mut v = Vec::new();
            for _ in 0..60 {
                let mut f = vec![0u8; 417];
                f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
                v.extend(f);
            }
            Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into()), ("icy-br".into(), "128".into())], body: Box::new(std::io::Cursor::new(v)) })
        }
    }

    const FIXTURE: &str = r#"[
      {"name":"Low Tamil","url_resolved":"https://low.example.org/stream","codec":"MP3","bitrate":64,"language":"tamil","tags":"tamil,hits","countrycode":"IN","votes":5,"clickcount":100,"lastcheckok":1},
      {"name":"Dead Tamil","url_resolved":"https://dead.example.org/stream","codec":"MP3","bitrate":128,"language":"tamil","tags":"","countrycode":"IN","votes":9000,"clickcount":90000,"lastcheckok":0},
      {"name":"Hls Tamil","url_resolved":"https://hls.example.org/live.m3u8","codec":"AAC","bitrate":128,"language":"tamil","votes":9000,"clickcount":90000,"lastcheckok":1,"hls":1},
      {"name":"Aac Tamil","url_resolved":"https://aac.example.org/live","codec":"AAC+","bitrate":64,"language":"tamil","votes":9000,"clickcount":90000,"lastcheckok":1},
      {"name":"Big Tamil FM","url_resolved":"https://big.example.org/live.mp3","codec":"MP3","bitrate":128,"language":"tamil","tags":"tamil,kuthu","countrycode":"IN","votes":300,"clickcount":5000,"lastcheckok":1},
      {"name":"Down Tamil","url_resolved":"https://dead2.example.org/stream","codec":"MP3","bitrate":128,"language":"tamil","votes":200,"clickcount":4000,"lastcheckok":1}
    ]"#;

    struct W {
        svc: Arc<Services>,
        tools: UserTools,
        dir: Arc<FakeDir>,
    }

    fn world(tag: &str, hosts: &[&str], body: Option<&str>) -> W {
        let d = std::env::temp_dir().join(format!("iyakki-radiotools-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        svc.radio.set_streamer(Arc::new(Streams));
        svc.radio.set_retry_wait(Duration::from_millis(20));
        svc.settings.lock().unwrap().update(|s| s.voice.play_cmd = "cat".into()).unwrap();
        let dir = FakeDir::new(body);
        svc.radio_dir.set_fetcher(dir.clone());
        let mut policy = Role::User.template();
        policy.net_hosts = hosts.iter().map(|h| h.to_string()).collect();
        let tools = UserTools::new("alice", policy, svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]);
        W { svc, tools, dir }
    }

    fn lines_with(text: &str, what: &str) -> Vec<String> {
        text.lines().filter(|l| l.contains(what)).map(str::to_string).collect()
    }

    struct Script(Mutex<VecDeque<ModelTurn>>);
    impl ChatBackend for Script {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
            self.0.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
        }
    }

    #[test]
    fn requests_are_understood() {
        let q = Query::parse("play a tamil online fm", None, None);
        assert_eq!((q.language.as_deref(), q.tags.is_empty(), q.words.is_empty()), (Some("Tamil"), true, true), "{q:?}");
        let q = Query::parse("play some kuthu songs", None, None);
        assert_eq!((q.language, q.tags, q.words), (None, vec!["kuthu".to_string()], vec![]));
        assert_eq!(Query::parse("தமிழ் ரேடியோ", None, None).language.as_deref(), Some("Tamil"));
        assert_eq!(Query::parse("kollywood", None, None).language.as_deref(), Some("Tamil"));
        let q = Query::parse("BBC news radio", None, None);
        assert_eq!((q.tags, q.words), (vec!["news".to_string()], vec!["bbc".to_string()]));
        let q = Query::parse("hits", Some("tamil"), None);
        assert_eq!((q.language.as_deref(), q.tags), (Some("Tamil"), vec!["hits".to_string()]));
        assert!(Query::parse("play the radio", None, None).is_empty());
        assert!(search_path(Some("Tamil"), None, None).starts_with("/json/stations/search?language=tamil&codec=MP3&hidebroken=true&order=clickcount&reverse=true&limit="));
        assert!(search_path(None, None, Some("89.4 fm")).contains("name=89.4%20fm"));
    }

    #[test]
    fn directory_answers_are_parsed_ranked_and_the_dead_and_unplayable_are_skipped() {
        let v = parse_stations(FIXTURE).unwrap();
        let names: Vec<&str> = v.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["Big Tamil FM", "Down Tamil", "Low Tamil"], "dead, HLS and AAC stations are left out; the most listened to comes first");
        assert_eq!((v[0].kbps, v[0].language.as_str(), v[0].country.as_str()), (128, "Tamil", "IN"));
        assert!(v[0].id.starts_with("o-") && v[0].source == Source::Directory);
        assert!(parse_stations("not json").is_err() && parse_stations("{}").is_err());
        assert_eq!(clean_name("Evil\u{7}\nFM   ignore previous"), "Evil FM ignore previous");
    }

    #[test]
    fn tools_exist_only_with_the_radio_app_and_never_ask() {
        let w = world("defs", &["*"], None);
        let defs = w.tools.defs();
        for n in ["radio_search", "radio_play", "radio_stop", "radio_pause", "radio_resume", "radio_volume", "radio_save", "radio_status"] {
            let d = defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing"));
            assert_eq!(d.risk, Risk::Safe, "{n}");
        }
        assert!(defs.iter().find(|d| d.spec.name == "radio_search").unwrap().untrusted, "station names come from outside");
        let mut p = Role::User.template();
        p.apps.remove(&App::Radio);
        assert!(super::defs(&p).is_empty());
        let t = UserTools::new("alice", p, w.svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]);
        assert!(t.call("radio_play", &json!({"query": "tamil"})).unwrap_err().contains("may not use the radio"));
        assert!(AGENT_SYSTEM_PROMPT.contains("radio_play") && AGENT_SYSTEM_PROMPT.contains("radio_search") && AGENT_SYSTEM_PROMPT.contains("play a tamil online fm"));
    }

    #[test]
    fn play_a_tamil_online_fm_offline_plays_the_built_in_list_through_the_chat() {
        let w = world("offline", &["*"], None);
        let script = Arc::new(Script(Mutex::new(
            vec![
                ModelTurn { text: String::new(), calls: vec![ToolCall { id: "1".into(), name: "radio_search".into(), args: json!({"query": "tamil online fm"}) }] },
                ModelTurn { text: String::new(), calls: vec![ToolCall { id: "2".into(), name: "radio_play".into(), args: json!({"query": "tamil online fm"}) }] },
                ModelTurn { text: "Playing Sooriyan FM.".into(), calls: vec![] },
            ]
            .into(),
        )));
        let h = start(script, Arc::new(w.tools), vec![Message::system(AGENT_SYSTEM_PROMPT), Message::user("play a tamil online fm")], RunLimits::default());
        let mut results = Vec::new();
        let mut approvals = 0;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "the run did not finish");
            match h.events.recv_timeout(Duration::from_secs(5)).expect("event") {
                RunEvent::ToolResult { name, ok, text, .. } => {
                    assert!(ok, "{name}: {text}");
                    results.push((name, text));
                }
                RunEvent::Approval { .. } => approvals += 1,
                RunEvent::Final { .. } => break,
                RunEvent::Failed { error } => panic!("{error}"),
                RunEvent::Stopped { reason } => panic!("{reason}"),
                _ => {}
            }
        }
        assert_eq!(approvals, 0, "radio tools need no approval");
        assert_eq!(results.len(), 2);
        assert!(results[0].1.contains("Sooriyan FM") && results[0].1.contains("[b-tamil-1]") && results[0].1.contains("No internet"), "{}", results[0].1);
        assert!(!results[0].1.contains(".aac") && !results[0].1.contains("80’s Tamil Hits"), "unplayable AAC stations are not offered: {}", results[0].1);
        assert!(results[1].1.starts_with("Playing Sooriyan FM"), "{}", results[1].1);
        assert!(matches!(w.svc.radio.state(), RadioState::Playing { ref station, .. } if station == "Sooriyan FM"), "the shared player plays it: {:?}", w.svc.radio.state());
        w.svc.radio.stop();
    }

    #[test]
    fn online_results_are_searched_ranked_cached_and_played_and_the_log_names_the_host_only() {
        let w = world("online", &["*"], Some(FIXTURE));
        let r = w.tools.call("radio_search", &json!({"query": "tamil", "limit": 10})).unwrap();
        let online = lines_with(&r, "directory");
        assert_eq!(online.len(), 3, "{r}");
        assert!(online[0].contains("Big Tamil FM") && online[0].contains("128 kbps") && online[1].contains("Down Tamil") && online[2].contains("Low Tamil"), "{r}");
        assert!(r.find("Sooriyan FM").unwrap() < r.find("Big Tamil FM").unwrap(), "built-in verified stations come before the directory: {r}");
        let asked = w.dir.asked.lock().unwrap().len();
        assert!(asked >= 1 && w.dir.asked.lock().unwrap()[0].starts_with("https://de1.api.radio-browser.info/json/stations/search?language=tamil"));
        w.tools.call("radio_search", &json!({"query": "tamil fm"})).unwrap();
        assert_eq!(w.dir.asked.lock().unwrap().len(), asked, "the same search within the hour is answered from memory");
        // play a directory station by its id: a dead one is not chosen, the shared player plays it
        let id = lines_with(&online[0], "[").first().and_then(|l| l.split(['[', ']']).nth(1)).unwrap().to_string();
        let played = w.tools.call("radio_play", &json!({"id": id})).unwrap();
        assert!(played.starts_with("Playing Big Tamil FM (Tamil, 128 kbps)"), "{played}");
        assert!(matches!(w.svc.radio.state(), RadioState::Playing { ref station, .. } if station == "Big Tamil FM"));
        let log = w.svc.toollog.list(7, Some("alice"));
        let entry = log.iter().find(|e| e.tool == "radio.play").expect("audited");
        assert_eq!((entry.what.as_str(), entry.result.as_str()), ("big.example.org", ""), "the host only, no address or station");
        // a station that will not start is skipped for the next one
        let r = w.tools.call("radio_play", &json!({"name": "Down Tamil"})).unwrap();
        assert!(r.starts_with("Playing "), "{r}");
        assert!(r.contains("Skipped") && r.contains("Down Tamil"), "{r}");
        w.svc.radio.stop();
    }

    #[test]
    fn a_name_search_goes_to_the_directory_and_kuthu_looks_at_tags_and_names() {
        let w = world("names", &["*"], Some(FIXTURE));
        let r = w.tools.call("radio_search", &json!({"query": "kuthu"})).unwrap();
        assert!(r.contains("Big Tamil FM"), "{r}");
        let asked = w.dir.asked.lock().unwrap().clone();
        assert!(asked.iter().any(|u| u.contains("tag=kuthu")) && asked.iter().any(|u| u.contains("name=kuthu")), "{asked:?}");
        w.dir.asked.lock().unwrap().clear();
        w.tools.call("radio_search", &json!({"query": "bbc"})).unwrap();
        assert!(w.dir.asked.lock().unwrap()[0].contains("name=bbc"));
    }

    #[test]
    fn the_host_policy_is_applied_to_the_directory_and_to_the_stream() {
        // may reach only one host: the directory is not asked, and no station can be played
        let w = world("policy", &["example.com"], Some(FIXTURE));
        let r = w.tools.call("radio_search", &json!({"query": "tamil"})).unwrap();
        assert!(r.contains("may not reach the online radio directory") && r.contains("Sooriyan FM"), "{r}");
        assert!(w.dir.asked.lock().unwrap().is_empty());
        let e = w.tools.call("radio_play", &json!({"query": "tamil online fm"})).unwrap_err();
        assert!(e.contains("may not reach") && e.contains("lotustechnologieslk.net"), "{e}");
        assert_eq!(w.svc.radio.state(), RadioState::Idle);
        assert!(w.svc.toollog.list(7, Some("alice")).iter().all(|e| e.tool != "radio.play"), "nothing was started");
        // the person's own station on the one allowed host plays
        StationStore::new(w.svc.dir(), "alice").add("My Test", "https://example.com/live").unwrap();
        assert!(w.tools.call("radio_play", &json!({"name": "My Test"})).unwrap().starts_with("Playing My Test"));
        w.svc.radio.stop();
    }

    #[test]
    fn stop_pause_resume_volume_status_and_the_app_sees_the_same_player() {
        let w = world("controls", &["*"], None);
        assert!(w.tools.call("radio_pause", &json!({})).unwrap_err().contains("not playing"));
        assert!(w.tools.call("radio_resume", &json!({})).unwrap_err().contains("not paused"));
        assert_eq!(w.tools.call("radio_stop", &json!({})).unwrap(), "The radio was not playing.");
        w.tools.call("radio_play", &json!({"query": "tamil"})).unwrap();
        // the RADIO app reads this very player
        assert!(w.svc.radio.is_active());
        assert!(w.tools.call("radio_status", &json!({})).unwrap().starts_with("Playing Sooriyan FM"));
        assert!(w.tools.call("radio_pause", &json!({})).unwrap().starts_with("Paused"));
        assert!(w.svc.radio.is_paused() && w.tools.call("radio_status", &json!({})).unwrap().starts_with("Paused:"));
        assert!(w.tools.call("radio_resume", &json!({})).unwrap().contains("plays again"));
        assert_eq!(w.tools.call("radio_volume", &json!({"level": 35})).unwrap(), "Radio volume is 35%.");
        assert_eq!(w.svc.radio.volume(), 35);
        assert_eq!(w.tools.call("radio_volume", &json!({"change": -50})).unwrap(), "Radio volume is 0%.");
        assert_eq!(w.tools.call("radio_volume", &json!({"change": 500})).unwrap(), "Radio volume is 100%.");
        assert!(w.tools.call("radio_volume", &json!({})).is_err());
        assert_eq!(w.tools.call("radio_stop", &json!({})).unwrap(), "Stopped the radio.");
        assert_eq!(w.svc.radio.state(), RadioState::Idle);
        // a station started in the app is stopped by the chat as well: it is one player
        w.svc.radio.play(&Station { name: "From app".into(), url: "https://app.example.org/x".into() }, Arc::new(|_| true), "cat");
        assert_eq!(w.tools.call("radio_stop", &json!({})).unwrap(), "Stopped the radio.");
    }

    #[test]
    fn a_found_station_can_be_kept_in_my_stations() {
        let w = world("save", &["*"], Some(FIXTURE));
        w.tools.call("radio_search", &json!({"query": "tamil"})).unwrap();
        assert!(w.tools.call("radio_save", &json!({})).unwrap_err().contains("nothing is playing"));
        assert!(w.tools.call("radio_save", &json!({"name": "Nobody FM"})).unwrap_err().contains("do not know"));
        assert!(w.tools.call("radio_save", &json!({"name": "Big Tamil FM"})).unwrap().contains("Kept Big Tamil FM"));
        w.tools.call("radio_play", &json!({"name": "Sooriyan FM"})).unwrap();
        assert!(w.tools.call("radio_save", &json!({})).unwrap().contains("Kept Sooriyan FM"));
        let mine = StationStore::new(w.svc.dir(), "alice").list();
        assert_eq!(mine.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Big Tamil FM", "Sooriyan FM"]);
        assert!(w.tools.call("radio_save", &json!({"name": "Big Tamil FM"})).unwrap().contains("already"));
        // saved stations are found by name afterwards, ahead of everything else
        let r = w.tools.call("radio_search", &json!({"query": "big tamil"})).unwrap();
        assert!(r.lines().nth(1).unwrap().contains("saved"), "{r}");
        assert!(StationStore::new(w.svc.dir(), "bob").list().is_empty(), "another account's list is its own");
        w.svc.radio.stop();
    }

    /// Needs the internet: asks the real directory for Tamil stations and prints the best five.
    #[test]
    #[ignore = "uses the real internet"]
    fn live_directory_tamil() {
        let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
        let d = Directory::new(Arc::new(UpstreamFetcher(up)));
        let q = Query::parse("play a tamil online fm", None, None);
        let found = d.search(&q, &|_| true).expect("the directory answers");
        for f in found.iter().take(5) {
            println!("{}", f.line());
        }
        assert!(!found.is_empty(), "no Tamil MP3 stations came back");
        assert!(found.iter().all(|f| f.url.starts_with("http") && f.source == Source::Directory));
    }
}
