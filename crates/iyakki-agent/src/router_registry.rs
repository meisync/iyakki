//! The model router's registry (F-ROUTER-01): one profile per configured model, built from the provider settings the person already has.
//! Nothing here reaches a model or a network: it only reads [`ProviderCfg`] values and the person's overrides, so there is no new way to
//! reach a model (docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 4.1).
//!
//! A provider does not say how big a model is, what it costs or whether it can plan, so the profile starts from plain hints (the size
//! written in the name, where the server runs, the words `mini`, `flash`, ...) and the person can correct any field with an override
//! (`router_set`). The hints are guesses and the profile says so by showing the same fields the person can change.

pub use iyakki_shared::router_registry::{ProfileOverride};
use std::collections::BTreeMap;


use crate::providers::{model_sees_pictures, Kind, ProviderCfg};
pub use iyakki_shared::router_registry::{Caps, Cost, Profile, Role, Zone};











/// Parameter count in billions when the name says so (`llama3:70b`, `qwen-1.5b-instruct`), the largest one it names.
pub fn size_billions(model: &str) -> Option<f32> {
    let m = model.to_ascii_lowercase();
    m.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.')).filter_map(|t| t.strip_suffix('b').and_then(|n| n.parse::<f32>().ok())).filter(|n| *n > 0.0).fold(None, |a: Option<f32>, n| Some(a.map_or(n, |x| x.max(n))))
}

fn light_name(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    ["mini", "nano", "flash", "lite", "small", "haiku", "instant", "tiny"].iter().any(|k| m.contains(k))
}

pub fn zone_of(cfg: &ProviderCfg) -> Zone {
    match cfg.kind {
        Kind::Ollama | Kind::Lemonade if cfg.base_url.is_none() => return Zone::Computer,
        _ => {}
    }
    let host = cfg.base().ok().and_then(|b| iyakki_sec::url_host(&b));
    match host {
        Some(h) if crate::utcp::is_loopback_host(&h) => Zone::Computer,
        Some(h) if crate::utcp::is_local_host(&h) => Zone::Network,
        Some(_) => Zone::Internet,
        // an address nobody can read is treated as the least private place
        None => Zone::Internet,
    }
}

const NOT_CHAT: [&str; 5] = ["embeddings", "reranking", "image", "stt", "tts"];

/// The profile of one model of a provider, before the person's override.
pub fn hint(cfg: &ProviderCfg, model: &str) -> Profile {
    let zone = zone_of(cfg);
    let local = zone != Zone::Internet;
    let size = size_billions(model);
    let light = light_name(model);
    let big = match size {
        Some(b) => b >= 30.0,
        None => !local && !light,
    };
    let role = if big { Role::Both } else { Role::Executor };
    let cost = if local {
        Cost::Free
    } else if light || size.is_some_and(|b| b < 30.0) {
        Cost::Low
    } else {
        Cost::High
    };
    let kind = cfg.model_kinds.get(model).map(String::as_str).unwrap_or("");
    let lower = model.to_ascii_lowercase();
    let caps = Caps {
        tools: size.is_none_or(|b| b >= 3.0),
        vision: model_sees_pictures(model) || kind == "vision",
        audio: kind == "stt" || ["audio", "omni", "whisper"].iter().any(|k| lower.contains(k)),
        context: if local { 8192 } else { 128_000 },
    };
    Profile { id: format!("{}/{}", cfg.id, model), provider: cfg.id.clone(), model: model.to_string(), role, caps, cost, zone, local, enabled: cfg.enabled }
}

/// Apply an override on top of a hint.
pub fn apply(mut p: Profile, o: &ProfileOverride) -> Profile {
    if let Some(r) = o.role {
        p.role = r;
    }
    if let Some(c) = o.cost {
        p.cost = c;
    }
    if let Some(c) = o.context {
        p.caps.context = c;
    }
    if let Some(v) = o.tools {
        p.caps.tools = v;
    }
    if let Some(v) = o.vision {
        p.caps.vision = v;
    }
    if let Some(v) = o.audio {
        p.caps.audio = v;
    }
    if let Some(v) = o.enabled {
        p.enabled = p.enabled && v;
    }
    if let Some(z) = o.zone {
        p.zone = z;
        p.local = z != Zone::Internet;
    }
    p
}

/// Every chat model of every provider, as a profile, sorted by id. Models a server labelled as not chat (embeddings, pictures, speech)
/// are left out: the router plans and executes with chat models.
pub fn build(providers: &[ProviderCfg], overrides: &BTreeMap<String, ProfileOverride>) -> Vec<Profile> {
    let mut out: BTreeMap<String, Profile> = BTreeMap::new();
    for cfg in providers {
        let mut names: Vec<&String> = cfg.models.iter().collect();
        if let Some(d) = &cfg.default_model {
            if !names.contains(&d) {
                names.push(d);
            }
        }
        for m in names {
            if cfg.model_kinds.get(m).is_some_and(|k| NOT_CHAT.contains(&k.as_str())) {
                continue;
            }
            let h = hint(cfg, m);
            let o = overrides.get(&h.id).cloned().unwrap_or_default();
            out.insert(h.id.clone(), apply(h, &o));
        }
    }
    out.into_values().collect()
}
