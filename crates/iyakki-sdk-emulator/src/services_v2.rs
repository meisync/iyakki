//! Fake services for the SDK 1.2 namespaces (docs/development/SDK-BRIDGE-V2.md): discovery, `markdown`, `embed`, `dash`, `flows`, `k3s`, `media`,
//! `audio` control, `speech.listen` and `money`. The pure rules are the device's own, taken from `iyakki_core_api::sdkbridge` (the manifest table,
//! the Markdown analysis, the money limit arithmetic), so a tile cannot pass here and fail on the device for those. Everything that would touch the
//! machine (a viewer, a cluster, a flow engine, a microphone, a payment provider) answers from `iyakki-emulator.json` (`sdk` section) or a made-up
//! value that says it is emulated. Nothing is played, listened to or paid.
//!
//! `iyakki-emulator.json`, section `"sdk"` (all optional):
//! `{"dash": {"sources": ["prom"], "rows": 3}, "flows": ["backup"], "k3s": {"objects": {"pods": [{"metadata": {"name": "web-1"}}]}},
//!   "heard": "turn on the light", "money": {"currency": "USD", "per_day_minor": 1000, "provider": true}}`.
//! Money starts at nothing, as on the device: without a `money` limit every spend is refused.

use iyakki_core_api::sdkbridge::{self as bridge, money};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct V2 {
    next: u32,
    embeds: BTreeMap<u32, String>,
    runs: Vec<String>,
    listening: bool,
    spent: BTreeMap<u64, u64>,
    currency: String,
    per_day: u64,
    configured: bool,
}

const DAY_MS: u64 = 86_400_000;
const EMBED_KINDS: [&str; 4] = ["markdown", "browser", "media", "dash"];

/// Domains of SDK 1.2 and discovery (`audio` is also answered by the older stubs for `play` and `record`).
pub fn is_v2_domain(d: &str) -> bool {
    matches!(d, "sdk" | "markdown" | "embed" | "dash" | "flows" | "k3s" | "media" | "money") || d == "speech"
}

fn ignored(a: &Value, known: &[&str]) -> Vec<String> {
    a.as_object().map(|o| o.keys().filter(|k| !known.contains(&k.as_str())).take(10).cloned().collect()).unwrap_or_default()
}

fn done(mut v: Value, ig: Vec<String>) -> Value {
    if !ig.is_empty() {
        v["ignored"] = json!(ig);
    }
    v
}

fn text<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("BadArgument: {k}: must be a string"))
}

impl super::services::FakeServices {
    fn sdk_cfg(&self) -> &Value {
        &self.config().sdk
    }

    /// Does the manifest hold the capability key (the emulator's manifest names domains, not capability keys)?
    fn holds(&self, key: &str) -> bool {
        let dom = match key {
            "microphone" => "speech",
            "embed_hosts" => "embed",
            "notifications" => "notify",
            "agent" | "llm" => "chat",
            "network" => "net",
            k => k,
        };
        self.manifest_caps().iter().any(|c| c.split('.').next() == Some(dom))
    }

    /// `Some(answer)` for a domain of SDK 1.2; `None` for any other call.
    pub(crate) fn dispatch_v2(&self, domain: &str, method: &str, a: &Value) -> Option<Result<Value, String>> {
        let audio_control = domain == "audio" && !matches!(method, "play" | "record");
        if !is_v2_domain(domain) && !audio_control {
            return None;
        }
        // a name the table does not know is Unsupported here as on the device
        let full = format!("{domain}.{method}");
        if bridge::find_op(&full).is_none() {
            return Some(Err(format!("Unsupported: iyakki.{full} is not available in this version of IYAKKI (SDK {})", bridge::SDK_VERSION)));
        }
        Some(self.v2(domain, method, a))
    }

    fn v2(&self, domain: &str, method: &str, a: &Value) -> Result<Value, String> {
        let mut v = self.v2_state();
        match (domain, method) {
            ("sdk", "capabilities") => Ok(bridge::listing(bridge::Version::CURRENT, &|k: &str| self.holds(k))),
            ("sdk", "has") => Ok(json!(bridge::has(bridge::Version::CURRENT, text(a, "name")?, &|k: &str| self.holds(k)))),
            ("markdown", "render") => {
                let ig = ignored(a, &["text", "width"]);
                let r = bridge::markdown::analyze(text(a, "text")?, a.get("width").and_then(Value::as_u64).unwrap_or(0) as u32).map_err(|e| format!("BadArgument: text: {e}"))?;
                Ok(done(r.to_json(), ig))
            }
            ("markdown", "view") | ("markdown", "openFile") => Ok(json!({"id": 1, "emulated": true, "note": "no viewer is opened here"})),
            ("embed", "mount") => {
                let kind = text(a, "kind")?.to_string();
                if !EMBED_KINDS.contains(&kind.as_str()) {
                    return Err("BadArgument: kind: markdown, browser, media or dash".into());
                }
                if let Some(t) = a.get("target").filter(|t| t.is_object()) {
                    let n = |k: &str| t.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    if n("w") < 120.0 || n("h") < 80.0 || n("w") > 900.0 || n("h") > 2880.0 {
                        return Err("BadArgument: target: at least 120 x 80 and at most 900 x 2880".into());
                    }
                }
                if v.embeds.len() >= 2 || (kind == "browser" && v.embeds.values().any(|k| k == "browser")) {
                    return Err("QuotaExceeded: too many: at most 2 embedded viewers and 1 browser".into());
                }
                v.next += 1;
                let id = v.next;
                v.embeds.insert(id, kind);
                Ok(json!({"id": id, "emulated": true}))
            }
            ("embed", "update") | ("embed", "call") | ("embed", "unmount") => {
                let id = a.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
                if !v.embeds.contains_key(&id) {
                    return Err("BadArgument: id: not an embedded viewer of this app".into());
                }
                if method == "unmount" {
                    v.embeds.remove(&id);
                }
                Ok(json!({"id": id, "result": null, "emulated": true}))
            }
            ("embed", "list") => Ok(json!({"items": v.embeds.iter().map(|(i, k)| json!({"id": i, "kind": k})).collect::<Vec<_>>()})),
            ("dash", "sources") => {
                let names = self.sdk_cfg()["dash"]["sources"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_else(|| vec!["demo".into()]);
                Ok(json!({"items": names.iter().map(|n| json!({"name": n, "kind": "emulated", "language": "none"})).collect::<Vec<_>>()}))
            }
            ("dash", "query") => {
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(1000).clamp(1, 1000) as usize;
                let n = self.sdk_cfg()["dash"]["rows"].as_u64().unwrap_or(3) as usize;
                let rows: Vec<Value> = (0..n.min(limit)).map(|i| json!([i, i as f64])).collect();
                Ok(json!({"columns": [{"name": "time", "type": "time"}, {"name": "value", "type": "number"}], "rows": rows, "truncated": n > limit, "emulated": true}))
            }
            ("dash", "panel.add") => Ok(json!({"id": "emulated-panel", "emulated": true})),
            ("flows", "list") => {
                let names = self.sdk_cfg()["flows"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_else(|| vec!["demo".into()]);
                Ok(json!({"items": names.iter().map(|n| json!({"id": n, "name": n, "version": 1, "inputs": []})).collect::<Vec<_>>()}))
            }
            ("flows", "run") => {
                let run = format!("emulated-run-{}", v.runs.len() + 1);
                v.runs.push(run.clone());
                Ok(json!({"run": run, "emulated": true}))
            }
            ("flows", "status") | ("flows", "cancel") => {
                let run = text(a, "run")?;
                if !v.runs.iter().any(|r| r == run) {
                    return Err("BadArgument: run: not a run this app started".into());
                }
                Ok(if method == "status" { json!({"state": "succeeded", "steps": [], "error": null}) } else { json!({"run": run, "state": "cancelled"}) })
            }
            ("k3s", "status") => Ok(json!({"cluster": "emulated", "namespace": "emulated-ns", "ready": true, "writable": true})),
            ("k3s", "get") => {
                let kind = text(a, "kind")?.to_ascii_lowercase();
                let items = self.sdk_cfg()["k3s"]["objects"][&kind].as_array().cloned().unwrap_or_default();
                Ok(json!({"items": items}))
            }
            ("k3s", "logs") => Ok(json!({"lines": ["(emulated log)"]})),
            ("k3s", "apply") => Ok(json!({"plan": "(emulated plan: nothing is applied)"})),
            ("k3s", "restart") => Ok(json!({"restarted": text(a, "name")?})),
            ("media", "library") => Ok(json!({"items": [{"id": "emulated-1", "title": "Emulated song", "secs": 60, "kind": "audio"}]})),
            ("media", "play") => Ok(json!({"id": 1, "emulated": true})),
            ("media", "pause" | "resume" | "stop") => Ok(json!({"id": a.get("id").cloned().unwrap_or(Value::Null), "state": method})),
            ("media", "state") => Ok(json!({"id": a.get("id").cloned().unwrap_or(Value::Null), "state": "playing", "position": 0})),
            ("media", "queue.add") => Ok(json!({"queued": 1})),
            ("media", "queue.list") => Ok(json!({"items": []})),
            ("audio", "outputs") => Ok(json!({"items": [{"name": "Emulated speakers", "active": true}]})),
            ("audio", _) => Ok(json!({"id": a.get("id").cloned().unwrap_or(Value::Null), "emulated": true})),
            ("speech", "listen") => {
                if v.listening {
                    return Err("QuotaExceeded: too many: at most one microphone use at a time".into());
                }
                v.listening = true;
                let heard = self.sdk_cfg()["heard"].as_str().unwrap_or("(emulated speech)").to_string();
                Ok(json!({"handle": 1, "emulated": true, "text": heard, "note": "the emulator does not listen; this is the text from iyakki-emulator.json"}))
            }
            ("speech", "say") | ("speech", "stop") => Ok(json!({"id": 1, "emulated": true, "note": "nothing is spoken"})),
            ("speech", "cancel") => {
                v.listening = false;
                Ok(json!({}))
            }
            ("money", _) => self.v2_money(&mut v, method, a),
            _ => Err(format!("Unsupported: iyakki.{domain}.{method} is not emulated")),
        }
    }

    fn v2_money(&self, v: &mut V2, method: &str, a: &Value) -> Result<Value, String> {
        if !v.configured {
            v.configured = true;
            let m = &self.sdk_cfg()["money"];
            v.currency = m["currency"].as_str().unwrap_or("USD").to_string();
            v.per_day = m["per_day_minor"].as_u64().unwrap_or(0);
        }
        let day = self.now_ms() / DAY_MS;
        let spent = v.spent.get(&day).copied().unwrap_or(0);
        let lim = money::Limit { currency: v.currency.clone(), per_day_minor: v.per_day };
        match method {
            "limits" => Ok(json!({"currency": v.currency, "perDayMinor": v.per_day, "spentTodayMinor": spent, "remainingMinor": v.per_day.saturating_sub(spent), "confirmEach": true, "resetsAt": (day + 1) * DAY_MS / 1000})),
            "spend" => {
                let amount = a.get("amountMinor").and_then(Value::as_u64).unwrap_or(0);
                let cur = a.get("currency").and_then(Value::as_str).unwrap_or("");
                match money::check_spend(Some(&lim), spent, cur, amount) {
                    Err(r) => Err(format!("{}: {}", if r == money::Refusal::BadAmount { "BadArgument" } else { "BudgetStop" }, r.message())),
                    Ok(left) => {
                        if self.sdk_cfg()["money"]["provider"] != true {
                            return Err("Unavailable: no payment provider is set up (add \"provider\": true under sdk.money in iyakki-emulator.json to emulate a successful charge; nothing is ever paid)".into());
                        }
                        *v.spent.entry(day).or_insert(0) += amount;
                        Ok(json!({"receipt": "emulated-receipt", "remainingMinor": left, "emulated": true}))
                    }
                }
            }
            _ => {
                let want = a.get("perDayMinor").and_then(Value::as_u64).unwrap_or(0);
                if want == 0 || want > money::MAX_MINOR || want <= v.per_day {
                    return Err("BadArgument: perDayMinor: more than the current limit".into());
                }
                v.per_day = want;
                Ok(json!({"perDayMinor": want}))
            }
        }
    }
}
