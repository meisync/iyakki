//! `config` scope: change device settings from a paired computer through a WHITELIST (`iyakki dev config list|get|set|export|import`
//! and `provider add|remove|key`). A key is only settable if it is in [`DEFS`] with its type and range; anything else is refused by
//! name, never guessed. Secrets (provider API keys) are WRITE-ONLY: they go straight into the system vault and no call can read them
//! back, no export holds them, no log line shows them. Settings that live outside `settings.json` (host name, display size, the
//! firewall) come in through [`ConfigExt`], implemented where those parts live.

use std::collections::BTreeMap;

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use crate::providers::{valid_provider_id, Kind, ProviderCfg};
use crate::services::Services;
use crate::settings::{ModelChoice, Settings};

/// What a value may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int(i64, i64),
    Enum(&'static [&'static str]),
    /// Text up to this many characters.
    Text(usize),
}

impl Ty {
    pub fn name(self) -> String {
        match self {
            Ty::Bool => "bool".into(),
            Ty::Int(a, b) => format!("int {a}..{b}"),
            Ty::Enum(v) => format!("one of {}", v.iter().map(|x| if x.is_empty() { "\"\"" } else { x }).collect::<Vec<_>>().join("|")),
            Ty::Text(n) => format!("text up to {n}"),
        }
    }

    /// Check a JSON value against the type; the value comes back normalised.
    pub fn check(self, v: &Value) -> Result<Value, String> {
        match self {
            Ty::Bool => v.as_bool().map(Value::Bool).ok_or_else(|| "expected true or false".to_string()),
            Ty::Int(lo, hi) => {
                let n = v.as_i64().ok_or("expected a whole number")?;
                if (lo..=hi).contains(&n) { Ok(json!(n)) } else { Err(format!("expected a number from {lo} to {hi}")) }
            }
            Ty::Enum(opts) => {
                let s = v.as_str().ok_or("expected text")?;
                if opts.contains(&s) { Ok(json!(s)) } else { Err(format!("expected one of: {}", opts.iter().map(|x| if x.is_empty() { "\"\"" } else { x }).collect::<Vec<_>>().join(", "))) }
            }
            Ty::Text(max) => {
                let s = v.as_str().ok_or("expected text")?;
                if s.chars().count() > max || s.chars().any(|c| c.is_control()) {
                    return Err(format!("expected text of at most {max} characters, without control characters"));
                }
                Ok(json!(s))
            }
        }
    }
}

pub struct Def {
    pub key: &'static str,
    pub ty: Ty,
    pub desc: &'static str,
}

const FN_MODES: &[&str] = &["", "media", "fkeys", "auto"];
const CLOUD: &[&str] = &["auto", "on", "off"];

/// Every settings key a paired computer may read and write. All of them are machine-wide, so all need an administrator account.
pub const DEFS: &[Def] = &[
    Def { key: "time.zone", ty: Ty::Text(64), desc: "time zone name such as Europe/Paris (empty: what the machine says)" },
    Def { key: "time.hour12", ty: Ty::Bool, desc: "12-hour clock" },
    Def { key: "time.seconds", ty: Ty::Bool, desc: "show seconds in the top bar" },
    Def { key: "time.no_ntp", ty: Ty::Bool, desc: "do not set the clock from the network" },
    Def { key: "time.ntp_server", ty: Ty::Text(253), desc: "time server host name (empty: built-in)" },
    Def { key: "sound.output", ty: Ty::Text(64), desc: "output key such as PCH:3 (empty: automatic)" },
    Def { key: "sound.volume", ty: Ty::Int(0, 100), desc: "IYAKKI's volume in percent" },
    Def { key: "sound.muted", ty: Ty::Bool, desc: "mute IYAKKI's sound" },
    Def { key: "voice.speed_pct", ty: Ty::Int(50, 200), desc: "speaking speed in percent" },
    Def { key: "voice.speak_replies", ty: Ty::Bool, desc: "read assistant replies aloud" },
    Def { key: "voice.auto_send", ty: Ty::Bool, desc: "send transcribed speech straight away" },
    Def { key: "voice.auto_stop", ty: Ty::Bool, desc: "a dictation ends by itself after a pause in the talking (default on)" },
    Def { key: "voice.keep_listening", ty: Ty::Bool, desc: "listen again after each dictation that ended on a pause, until Stop (default off)" },
    Def { key: "voice.fix_wording", ty: Ty::Bool, desc: "correct dictated wording with the chat assistant as you talk (default off)" },
    Def { key: "voice.assistant_open", ty: Ty::Bool, desc: "the Assistant pane beside the Voice tile's transcript is open (default on)" },
    Def { key: "voice.my_iyakki", ty: Ty::Bool, desc: "My IYAKKI, the push-to-talk voice assistant (Voice tile, Assistant tab), is on (default off)" },
    Def { key: "voice.my_iyakki_muted", ty: Ty::Bool, desc: "My IYAKKI's microphone is muted (kept across restarts)" },
    Def { key: "voice.my_iyakki_wake", ty: Ty::Bool, desc: "My IYAKKI's wake phrase: a local spotter listens for \"My IYAKKI\" (default off; opt-in; sound stays in memory)" },
    Def { key: "voice.my_iyakki_wake_sensitivity", ty: Ty::Int(0, 2), desc: "how easily the wake phrase triggers: 0 low, 1 normal (default), 2 high" },
    Def { key: "voice.my_iyakki_follow_ms", ty: Ty::Int(0, 20000), desc: "My IYAKKI's follow-up window after an answer, in milliseconds (0 off, default 8000)" },
    Def { key: "voice.pause_ms", ty: Ty::Int(500, 4000), desc: "the pause that ends a dictation, in milliseconds (default 1500)" },
    Def { key: "voice.align_words", ty: Ty::Bool, desc: "time the lit word from the sound" },
    Def { key: "voice.tts", ty: Ty::Text(200), desc: "reading voice: PROVIDER/MODEL[/VOICE] of a Lemonade provider (empty: built-in)" },
    Def { key: "voice.stt", ty: Ty::Text(200), desc: "speech-to-text: PROVIDER/MODEL of a Lemonade provider (empty: built-in)" },
    Def { key: "keyboard.profile", ty: Ty::Enum(crate::inputprofile::PROFILES), desc: "input profile: empty = automatic, mac (Command is the shortcut key) or pc" },
    Def { key: "keyboard.fn_mode", ty: Ty::Enum(FN_MODES), desc: "Fn keys of Apple keyboards" },
    Def { key: "keyboard.iso_layout", ty: Ty::Bool, desc: "Apple ISO layout fix" },
    Def { key: "keyboard.swap_opt_cmd", ty: Ty::Bool, desc: "swap Option and Command" },
    Def { key: "keyboard.trackpad_tap", ty: Ty::Bool, desc: "tap to click" },
    Def { key: "keyboard.trackpad_natural", ty: Ty::Bool, desc: "natural scrolling" },
    Def { key: "keyboard.scroll_speed_pct", ty: Ty::Int(50, 300), desc: "scrolling speed in percent (default 100)" },
    Def { key: "keyboard.wheel_lines", ty: Ty::Int(1, 10), desc: "lines per mouse wheel click (default 3)" },
    Def { key: "keyboard.wheel_natural", ty: Ty::Bool, desc: "mouse wheel: natural direction" },
    Def { key: "keyboard.scroll_smooth", ty: Ty::Bool, desc: "smooth (eased) scrolling" },
    Def { key: "keyboard.scroll_momentum", ty: Ty::Bool, desc: "trackpad scrolling keeps going after the fingers lift" },
    Def { key: "cloud.mode", ty: Ty::Enum(CLOUD), desc: "cloud mode: hide what needs local hardware (auto follows the machine)" },
    Def { key: "model.chat", ty: Ty::Text(120), desc: "chat model: PROVIDER/MODEL (empty: none)" },
    Def { key: "model.vision", ty: Ty::Text(120), desc: "vision model: PROVIDER/MODEL (empty: the chat model if it can see pictures, else picked automatically)" },
    Def { key: "model.tidy", ty: Ty::Text(120), desc: "model that tidies dictated text and revises text: PROVIDER/MODEL (empty: the chat model; a small fast model answers sooner than a remote reasoning one)" },
    Def { key: "model.embedding", ty: Ty::Text(120), desc: "embedding model: PROVIDER/MODEL (empty: built-in)" },
    Def { key: "rag.top_k", ty: Ty::Int(1, 50), desc: "passages retrieved per question" },
    Def { key: "rag.use_external", ty: Ty::Bool, desc: "query external knowledge backends too" },
    Def { key: "store.daily_check", ty: Ty::Bool, desc: "look for app and system updates once a day (only the stores and update list address that were added; false: only when asked)" },
    Def { key: "limits.tool_log_days", ty: Ty::Int(0, 365), desc: "days the assistant's tool log is kept (0: default 3)" },
    Def { key: "limits.media_download_max_mb", ty: Ty::Int(0, 102_400), desc: "largest media download in MB (0: default 2048)" },
    Def { key: "limits.think_limit_secs", ty: Ty::Int(0, 3600), desc: "seconds of pure thinking before a retry (0: default 60)" },
    Def { key: "limits.silence_limit_secs", ty: Ty::Int(0, 3600), desc: "seconds of silence before a model call is abandoned (0: default 120)" },
];

/// Settings that live outside `settings.json`; the device's shell side implements them (host name, display size, firewall...).
pub trait ConfigExt: Send + Sync {
    fn defs(&self) -> Vec<(String, Ty, String, bool)>;
    /// `(key, type, description, needs an administrator)`; `get` returns None for keys it does not know.
    fn get(&self, key: &str, account: &str) -> Option<Result<Value, String>>;
    fn set(&self, key: &str, v: &Value, account: &str, who: &str) -> Option<Result<String, String>>;
    /// Actions that are not a plain value (`firewall.keep`...). None: not mine.
    fn call(&self, _action: &str, _args: &Value, _who: &str) -> Option<Result<Value, String>> {
        None
    }
}

fn choice_text(c: &Option<ModelChoice>) -> String {
    c.as_ref().map(|c| format!("{}/{}", c.provider, c.model)).unwrap_or_default()
}

fn get_value(s: &Settings, key: &str) -> Option<Value> {
    Some(match key {
        "time.zone" => json!(s.time.zone),
        "time.hour12" => json!(s.time.hour12),
        "time.seconds" => json!(s.time.seconds),
        "time.no_ntp" => json!(s.time.no_ntp),
        "time.ntp_server" => json!(s.time.ntp_server),
        "sound.output" => json!(s.sound.output),
        "sound.volume" => json!(s.sound.volume_pct),
        "sound.muted" => json!(s.sound.muted),
        "voice.speed_pct" => json!(s.voice.speed_pct),
        "voice.speak_replies" => json!(s.voice.speak_replies),
        "voice.auto_send" => json!(s.voice.auto_send),
        "voice.auto_stop" => json!(s.voice.auto_stop),
        "voice.keep_listening" => json!(s.voice.keep_listening),
        "voice.fix_wording" => json!(s.voice.fix_wording),
        "voice.assistant_open" => json!(s.voice.assistant_open),
        "voice.my_iyakki" => json!(s.voice.my_on),
        "voice.my_iyakki_muted" => json!(s.voice.my_muted),
        "voice.my_iyakki_wake" => json!(s.voice.my_wake),
        "voice.my_iyakki_wake_sensitivity" => json!(s.voice.my_wake_sens),
        "voice.my_iyakki_follow_ms" => json!(s.voice.my_follow_ms),
        "voice.pause_ms" => json!(s.voice.pause_ms),
        "voice.align_words" => json!(s.voice.align_words),
        "voice.tts" => json!(s.voice.tts.as_ref().map(|e| e.label()).unwrap_or_default()),
        "voice.stt" => json!(s.voice.stt.as_ref().map(|e| e.label()).unwrap_or_default()),
        "keyboard.profile" => json!(s.keyboard.profile),
        "keyboard.fn_mode" => json!(s.keyboard.fn_mode),
        "keyboard.iso_layout" => json!(s.keyboard.iso_layout.unwrap_or(false)),
        "keyboard.swap_opt_cmd" => json!(s.keyboard.swap_opt_cmd.unwrap_or(false)),
        "keyboard.trackpad_tap" => json!(s.keyboard.trackpad_tap.unwrap_or(true)),
        "keyboard.trackpad_natural" => json!(s.keyboard.trackpad_natural.unwrap_or(true)),
        "keyboard.scroll_speed_pct" => json!(s.keyboard.scroll().speed_pct),
        "keyboard.wheel_lines" => json!(s.keyboard.scroll().wheel_lines),
        "keyboard.wheel_natural" => json!(s.keyboard.scroll().wheel_natural),
        "keyboard.scroll_smooth" => json!(s.keyboard.scroll().smooth),
        "keyboard.scroll_momentum" => json!(s.keyboard.scroll().momentum),
        "cloud.mode" => json!(match s.cloud {
            crate::cloudmode::Setting::Auto => "auto",
            crate::cloudmode::Setting::On => "on",
            crate::cloudmode::Setting::Off => "off",
        }),
        "model.chat" => json!(choice_text(&s.chat)),
        "model.vision" => json!(choice_text(&s.vision)),
        "model.tidy" => json!(choice_text(&s.tidy)),
        "model.embedding" => json!(choice_text(&s.embedding)),
        "rag.top_k" => json!(s.rag.top_k),
        "rag.use_external" => json!(s.rag.use_external),
        "store.daily_check" => json!(s.update_check.mode == crate::store::updatecheck::Mode::Daily),
        "limits.tool_log_days" => json!(s.tool_log_days),
        "limits.media_download_max_mb" => json!(s.media_download_max_mb),
        "limits.think_limit_secs" => json!(s.think_limit_secs),
        "limits.silence_limit_secs" => json!(s.silence_limit_secs),
        _ => return None,
    })
}

fn choice_of(s: &Settings, text: &str) -> Result<Option<ModelChoice>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    let (p, m) = text.split_once('/').ok_or("write it as PROVIDER/MODEL")?;
    if s.provider(p).is_none() {
        return Err(format!("there is no provider called {p}: add it with `iyakki dev config provider add`"));
    }
    if !crate::providers::valid_model_name(m) {
        return Err("that is not a model name".into());
    }
    Ok(Some(ModelChoice { provider: p.into(), model: m.into() }))
}

/// Set one validated value in the settings (voice engines are set through the services, not here).
fn apply(s: &mut Settings, key: &str, v: &Value) -> Result<(), String> {
    let b = v.as_bool();
    let n = v.as_i64().unwrap_or(0);
    let t = v.as_str().unwrap_or("");
    match key {
        "time.zone" => {
            if !t.is_empty() && crate::clock::has_zone_files() && !crate::clock::zone_names().iter().any(|z| z == t) {
                return Err(format!("{t:?} is not a time zone this device knows (look at /usr/share/zoneinfo)"));
            }
            s.time.zone = t.into();
        }
        "time.hour12" => s.time.hour12 = b.unwrap_or(false),
        "time.seconds" => s.time.seconds = b.unwrap_or(false),
        "time.no_ntp" => s.time.no_ntp = b.unwrap_or(false),
        "time.ntp_server" => {
            if !t.is_empty() && !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                return Err("a time server is a host name: letters, digits, dots and dashes".into());
            }
            s.time.ntp_server = t.into();
        }
        "sound.output" => {
            if !t.chars().all(|c| c.is_ascii_alphanumeric() || ":._-".contains(c)) {
                return Err("an output key is letters, digits and : . _ -".into());
            }
            s.sound.output = t.into();
        }
        "sound.volume" => s.sound.volume_pct = n as u32,
        "sound.muted" => s.sound.muted = b.unwrap_or(false),
        "voice.speed_pct" => s.voice.speed_pct = n as u32,
        "voice.speak_replies" => s.voice.speak_replies = b.unwrap_or(false),
        "voice.auto_send" => s.voice.auto_send = b.unwrap_or(false),
        "voice.auto_stop" => s.voice.auto_stop = b.unwrap_or(true),
        "voice.keep_listening" => s.voice.keep_listening = b.unwrap_or(false),
        "voice.fix_wording" => s.voice.fix_wording = b.unwrap_or(false),
        "voice.assistant_open" => s.voice.assistant_open = b.unwrap_or(true),
        "voice.my_iyakki" => s.voice.my_on = b.unwrap_or(false),
        "voice.my_iyakki_muted" => s.voice.my_muted = b.unwrap_or(false),
        "voice.my_iyakki_wake" => s.voice.my_wake = b.unwrap_or(false),
        "voice.my_iyakki_wake_sensitivity" => s.voice.my_wake_sens = n as u8,
        "voice.my_iyakki_follow_ms" => s.voice.my_follow_ms = n as u32,
        "voice.pause_ms" => s.voice.pause_ms = n as u32,
        "voice.align_words" => s.voice.align_words = b.unwrap_or(false),
        "keyboard.profile" => s.keyboard.profile = t.into(),
        "keyboard.fn_mode" => s.keyboard.fn_mode = t.into(),
        "keyboard.iso_layout" => s.keyboard.iso_layout = b,
        "keyboard.swap_opt_cmd" => s.keyboard.swap_opt_cmd = b,
        "keyboard.trackpad_tap" => s.keyboard.trackpad_tap = b,
        "keyboard.trackpad_natural" => s.keyboard.trackpad_natural = b,
        "keyboard.scroll_speed_pct" => s.keyboard.scroll_speed_pct = n as u32,
        "keyboard.wheel_lines" => s.keyboard.wheel_lines = n as u32,
        "keyboard.wheel_natural" => s.keyboard.wheel_natural = b,
        "keyboard.scroll_smooth" => s.keyboard.scroll_smooth = b,
        "keyboard.scroll_momentum" => s.keyboard.scroll_momentum = b,
        "cloud.mode" => {
            s.cloud = match t {
                "on" => crate::cloudmode::Setting::On,
                "off" => crate::cloudmode::Setting::Off,
                _ => crate::cloudmode::Setting::Auto,
            }
        }
        "model.chat" => s.chat = choice_of(s, t)?,
        "model.vision" => s.vision = choice_of(s, t)?,
        "model.tidy" => s.tidy = choice_of(s, t)?,
        "model.embedding" => s.embedding = choice_of(s, t)?,
        "rag.top_k" => s.rag.top_k = n as usize,
        "rag.use_external" => s.rag.use_external = b.unwrap_or(false),
        "store.daily_check" => {
            use crate::store::updatecheck::Mode;
            if b.unwrap_or(false) {
                s.update_check.mode = Mode::Daily;
            } else if s.update_check.mode == Mode::Daily {
                s.update_check.mode = Mode::Manual;
            }
        }
        "limits.tool_log_days" => s.tool_log_days = n as u32,
        "limits.media_download_max_mb" => s.media_download_max_mb = n as u32,
        "limits.think_limit_secs" => s.think_limit_secs = n as u32,
        "limits.silence_limit_secs" => s.silence_limit_secs = n as u32,
        other => return Err(format!("{other} is not in the list of settings a computer may change")),
    }
    Ok(())
}

#[cfg(test)]
pub fn set_for_test(s: &mut Settings, key: &str, value: &str) -> Result<(), String> {
    apply(s, key, &json!(value))
}

pub fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// A key from the shell side that its definition marks administrator-only.
fn ext_admin_check(ext: Option<&dyn ConfigExt>, policy: &Policy, key: &str) -> Result<(), String> {
    if ext.is_some_and(|e| e.defs().iter().any(|(k, _, _, admin)| k == key && *admin)) {
        need_admin(policy)?;
    }
    Ok(())
}

fn need_admin(policy: &Policy) -> Result<(), String> {
    if policy.manage_settings { Ok(()) } else { Err("these settings are machine-wide: they need an administrator account".into()) }
}

/// The whitelist with the current values (never a secret).
pub fn list(svc: &Services, ext: Option<&dyn ConfigExt>, account: &str) -> Value {
    let s = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows: Vec<Value> = DEFS.iter().map(|d| json!({ "key": d.key, "type": d.ty.name(), "desc": d.desc, "value": get_value(s.get(), d.key), "admin": true })).collect();
    if let Some(e) = ext {
        for (k, ty, desc, admin) in e.defs() {
            let v = e.get(&k, account).and_then(|r| r.ok());
            rows.push(json!({ "key": k, "type": ty.name(), "desc": desc, "value": v, "admin": admin }));
        }
    }
    let computer = s.computers().map(|c| json!({ "id": c.me.id, "label": c.me.label }));
    json!({ "settings": rows, "providers": providers(&s.get().providers), "computer": computer })
}

fn providers(list: &[ProviderCfg]) -> Vec<Value> {
    list.iter().map(|p| json!({ "id": p.id, "kind": format!("{:?}", p.kind).to_lowercase(), "base_url": p.base_url, "enabled": p.enabled, "has_key": p.key_ref.is_some(), "models": p.models.len() })).collect()
}

pub fn get(svc: &Services, ext: Option<&dyn ConfigExt>, policy: &Policy, account: &str, key: &str) -> Result<Value, String> {
    if def(key).is_some() {
        need_admin(policy)?;
        return Ok(json!({ "key": key, "value": get_value(svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get(), key) }));
    }
    ext_admin_check(ext, policy, key)?;
    match ext.and_then(|e| e.get(key, account)) {
        Some(r) => r.map(|v| json!({ "key": key, "value": v })),
        None => Err(format!("{key} is not in the list of settings a computer may read (see `iyakki dev config list`)")),
    }
}

/// Set one key. Returns a sentence saying what changed.
pub fn set(svc: &Services, ext: Option<&dyn ConfigExt>, policy: &Policy, account: &str, who: &str, key: &str, value: &Value) -> Result<String, String> {
    if let Some(d) = def(key) {
        need_admin(policy)?;
        let v = d.ty.check(value).map_err(|e| format!("{key}: {e}"))?;
        set_checked(svc, key, &v)?;
        return Ok(format!("{key} = {}", shown(&v)));
    }
    ext_admin_check(ext, policy, key)?;
    if let Some(ty) = ext.and_then(|e| e.defs().into_iter().find(|(k, ..)| k == key).map(|(_, t, ..)| t)) {
        let v = ty.check(value).map_err(|e| format!("{key}: {e}"))?;
        return ext.and_then(|e| e.set(key, &v, account, who)).unwrap_or_else(|| Err(format!("{key} cannot be changed here")));
    }
    match ext.and_then(|e| e.set(key, value, account, who)) {
        Some(r) => r,
        None => Err(format!("{key} is not in the list of settings a computer may change (see `iyakki dev config list`)")),
    }
}

fn shown(v: &Value) -> String {
    match v {
        Value::String(s) if s.is_empty() => "\"\"".into(),
        Value::String(s) => s.clone(),
        o => o.to_string(),
    }
}

fn split_engine(t: &str, min: usize) -> Result<Vec<&str>, String> {
    let parts: Vec<&str> = t.split('/').collect();
    if parts.len() < min || parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return Err("write it as PROVIDER/MODEL".into());
    }
    Ok(parts)
}

fn set_checked(svc: &Services, key: &str, v: &Value) -> Result<(), String> {
    let t = v.as_str().unwrap_or("");
    match key {
        "voice.tts" if t.is_empty() => svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.tts = None).map_err(|e| e.to_string()),
        "voice.stt" if t.is_empty() => svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.stt = None).map_err(|e| e.to_string()),
        "voice.tts" => {
            let p = split_engine(t, 2)?;
            svc.use_lemonade_voice(p[0], p[1], p.get(2).copied()).map(|_| ())
        }
        "voice.stt" => {
            let p = split_engine(t, 2)?;
            svc.use_lemonade_stt(p[0], p[1]).map(|_| ())
        }
        _ => {
            let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            // validate on a copy first: the saved settings change only when the whole thing is valid
            let mut copy = st.get().clone();
            apply(&mut copy, key, v)?;
            copy.validate().map_err(|e| e.to_string())?;
            let mut err = None;
            st.update(|s| {
                if let Err(e) = apply(s, key, v) {
                    err = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
            drop(st);
            if key == "time.zone" {
                crate::clock::set_chosen_zone(t);
            }
            err.map_or(Ok(()), Err)
        }
    }
}

// -------------------------------------------------------------------------------------------- providers

/// `add`, `remove` or `key` for a model provider. The key is WRITE-ONLY: it goes into the system vault and is never returned.
pub fn provider(svc: &Services, policy: &Policy, args: &Value) -> Result<Value, String> {
    need_admin(policy)?;
    let id = args["id"].as_str().unwrap_or("");
    if !valid_provider_id(id) {
        return Err("a provider id is lower-case letters, digits, - and _ (start with a letter, at most 32)".into());
    }
    let vault_name = format!("provider.{id}");
    let set_key = |key: &str| -> Result<(), String> {
        if key.is_empty() || key.len() > 4096 || key.chars().any(|c| c.is_control()) {
            return Err("the key is 1 to 4096 characters without control characters".into());
        }
        svc.system_set(&vault_name, key).map_err(|e| match e {
            crate::vault::VaultError::Locked => "the system vault is locked: an administrator must be signed in on the device for a key to be stored".to_string(),
            other => other.to_string(),
        })
    };
    match args["action"].as_str().unwrap_or("") {
        "add" => {
            let kind = Kind::ALL.into_iter().find(|k| format!("{k:?}").to_lowercase() == args["kind"].as_str().unwrap_or("").to_lowercase()).ok_or_else(|| format!("kind: one of {}", Kind::ALL.iter().map(|k| format!("{k:?}").to_lowercase()).collect::<Vec<_>>().join(", ")))?;
            let mut p = ProviderCfg::new(id, kind);
            if let Some(u) = args["url"].as_str().filter(|u| !u.is_empty()) {
                p.base_url = Some(u.into());
            }
            let key = args["key"].as_str().filter(|k| !k.is_empty());
            if key.is_some() {
                p.key_ref = Some(format!("vault:{vault_name}"));
            }
            // validate before the key is stored
            let mut copy = svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().clone();
            copy.providers.retain(|x| x.id != id);
            copy.providers.push(p.clone());
            copy.validate().map_err(|e| e.to_string())?;
            if let Some(k) = key {
                set_key(k)?;
            }
            svc.settings.lock().unwrap_or_else(|e| e.into_inner()).upsert_provider(p).map_err(|e| e.to_string())?;
            Ok(json!({ "added": id, "has_key": key.is_some() }))
        }
        "key" => {
            let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            let mut p = st.get().provider(id).cloned().ok_or_else(|| format!("there is no provider called {id}"))?;
            drop(st);
            set_key(args["key"].as_str().unwrap_or(""))?;
            p.key_ref = Some(format!("vault:{vault_name}"));
            st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            st.upsert_provider(p).map_err(|e| e.to_string())?;
            Ok(json!({ "key_stored_for": id }))
        }
        "remove" => {
            let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            if st.get().provider(id).is_none() {
                return Err(format!("there is no provider called {id}"));
            }
            st.remove_provider(id).map_err(|e| e.to_string())?;
            drop(st);
            let _ = svc.system_delete(&vault_name);
            Ok(json!({ "removed": id }))
        }
        other => Err(format!("unknown provider action {other:?}: add, key or remove")),
    }
}

// -------------------------------------------------------------------------------------------- profiles

/// A profile of every whitelisted setting and the providers (no keys, no key references).
pub fn export(svc: &Services, ext: Option<&dyn ConfigExt>, policy: &Policy, account: &str) -> Result<Value, String> {
    need_admin(policy)?;
    let s = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let mut m: BTreeMap<String, Value> = DEFS.iter().filter_map(|d| get_value(s.get(), d.key).map(|v| (d.key.to_string(), v))).collect();
    if let Some(e) = ext {
        for (k, _, _, _) in e.defs() {
            if let Some(Ok(v)) = e.get(&k, account) {
                m.insert(k, v);
            }
        }
    }
    let provs: Vec<Value> = s.get().providers.iter().map(|p| json!({ "id": p.id, "kind": format!("{:?}", p.kind).to_lowercase(), "base_url": p.base_url, "enabled": p.enabled })).collect();
    Ok(json!({ "iyakki_profile": 1, "settings": m, "providers": provs, "note": "no secrets: API keys are not part of a profile" }))
}

/// Apply a profile: every key and type is checked FIRST (nothing changes unless the whole profile is valid), then each setting is
/// applied. Providers in the profile are added without keys; a provider already there is left as it is.
pub fn import(svc: &Services, ext: Option<&dyn ConfigExt>, policy: &Policy, account: &str, who: &str, profile: &Value) -> Result<Value, String> {
    need_admin(policy)?;
    if profile["iyakki_profile"].as_i64() != Some(1) {
        return Err("this is not an IYAKKI profile (iyakki_profile: 1)".into());
    }
    let settings = profile["settings"].as_object().ok_or("the profile has no settings object")?;
    if settings.len() > 200 {
        return Err("a profile has at most 200 settings".into());
    }
    let ext_defs = ext.map(|e| e.defs()).unwrap_or_default();
    let mut todo: Vec<(&str, Value)> = Vec::new();
    for (k, v) in settings {
        let ty = def(k).map(|d| d.ty).or_else(|| ext_defs.iter().find(|(x, ..)| x == k).map(|(_, t, ..)| *t)).ok_or_else(|| format!("{k} is not in the list of settings a computer may change"))?;
        todo.push((k.as_str(), ty.check(v).map_err(|e| format!("{k}: {e}"))?));
    }
    // dry run of the settings.json part on a copy
    {
        let st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
        let mut copy = st.get().clone();
        for (k, v) in &todo {
            if def(k).is_some() && !matches!(*k, "voice.tts" | "voice.stt") {
                apply(&mut copy, k, v).map_err(|e| format!("{k}: {e}"))?;
            }
        }
        copy.validate().map_err(|e| e.to_string())?;
    }
    let mut provs: Vec<ProviderCfg> = Vec::new();
    for p in profile["providers"].as_array().into_iter().flatten() {
        let id = p["id"].as_str().unwrap_or("");
        if !valid_provider_id(id) {
            return Err(format!("provider id {id:?} is not valid"));
        }
        let kind = Kind::ALL.into_iter().find(|k| format!("{k:?}").to_lowercase() == p["kind"].as_str().unwrap_or("")).ok_or_else(|| format!("provider {id}: unknown kind"))?;
        let mut c = ProviderCfg::new(id, kind);
        c.base_url = p["base_url"].as_str().map(str::to_string).or(c.base_url);
        c.enabled = p["enabled"].as_bool().unwrap_or(true);
        provs.push(c);
    }
    let mut applied = 0;
    let mut added = Vec::new();
    // providers first (models may name them)
    for c in provs {
        let exists = svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().provider(&c.id).is_some();
        if !exists {
            added.push(c.id.clone());
            svc.settings.lock().unwrap_or_else(|e| e.into_inner()).upsert_provider(c).map_err(|e| e.to_string())?;
        }
    }
    let mut failed = Vec::new();
    for (k, v) in todo {
        match set(svc, ext, policy, account, who, k, &v) {
            Ok(_) => applied += 1,
            Err(e) => failed.push(format!("{k}: {e}")),
        }
    }
    Ok(json!({ "applied": applied, "providers_added": added, "failed": failed }))
}
