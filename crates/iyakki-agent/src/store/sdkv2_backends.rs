//! What the SDK 1.2 calls do on the machine (docs/development/SDK-BRIDGE-V2.md rule 8). The bridge side (`sdkv2.rs`) checks arguments, scope,
//! approval, limits and audit; a [`Backend`] only does the work, after all of that has passed. One small trait serves every namespace, so a new one
//! adds no trait, and a backend that is not wired answers `Unavailable` with a line saying what to do ([`Missing`]).
//!
//! Ops handed to a backend: `markdown.view`, `markdown.openFile`, `embed.mount|update|call|unmount`, `dash.sources|query|panel.add`,
//! `flows.list|run|status|cancel`, `k3s.status|get|logs|plan|apply|restart`, `media.library|play|pause|resume|stop|state|queue.add|queue.list|in_folders`,
//! `audio.play|pause|resume|stop|setVolume|outputs`, `speech.available|listen|cancel`, `money.charge`. Arguments are already checked and bounded.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::tilesdk::{code, SdkError};

/// Who is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub app: String,
    pub user: String,
}

pub trait Backend: Send + Sync {
    fn call(&self, who: &Who, op: &str, args: &Value) -> Result<Value, SdkError>;
}

/// A backend that is not wired: every call is `Unavailable`, with the fix in words. This is what a build without the shell surface or engine uses.
pub struct Missing {
    pub what: &'static str,
    pub fix: &'static str,
}

impl Backend for Missing {
    fn call(&self, _: &Who, op: &str, _: &Value) -> Result<Value, SdkError> {
        // asking whether it could work is not an error
        if op == "speech.available" {
            return Err(SdkError::new(code::UNAVAILABLE, format!("{} is not set up on this computer", self.what)).with("fix", self.fix));
        }
        Err(SdkError::new(code::UNAVAILABLE, format!("{} is not available on this computer yet", self.what)).with("fix", self.fix))
    }
}

#[derive(Default)]
struct FakeState {
    calls: Vec<(String, Value)>,
    replies: HashMap<String, VecDeque<Result<Value, SdkError>>>,
    always: HashMap<String, Result<Value, SdkError>>,
}

/// A backend for tests and the emulator: records every call and answers from what a test queued, else a plain default. It never touches the machine.
#[derive(Default)]
pub struct FakeBackend(Mutex<FakeState>);

impl FakeBackend {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The next call of `op` answers `v`.
    pub fn reply(&self, op: &str, v: Value) {
        self.st().replies.entry(op.into()).or_default().push_back(Ok(v));
    }

    /// The next call of `op` fails with `code`.
    pub fn fail(&self, op: &str, code: &'static str, message: &str) {
        self.st().replies.entry(op.into()).or_default().push_back(Err(SdkError::new(code, message)));
    }

    /// Every call of `op` answers `v` from now on.
    pub fn always(&self, op: &str, v: Value) {
        self.st().always.insert(op.into(), Ok(v));
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.st().calls.clone()
    }

    pub fn calls_of(&self, op: &str) -> Vec<Value> {
        self.st().calls.iter().filter(|(o, _)| o == op).map(|(_, a)| a.clone()).collect()
    }
}

impl Backend for FakeBackend {
    fn call(&self, _: &Who, op: &str, args: &Value) -> Result<Value, SdkError> {
        let mut st = self.st();
        st.calls.push((op.into(), args.clone()));
        if let Some(r) = st.replies.get_mut(op).and_then(VecDeque::pop_front) {
            return r;
        }
        if let Some(r) = st.always.get(op) {
            return r.clone();
        }
        Ok(default_reply(op, args))
    }
}

/// What a fake answers when a test did not say: small, shaped like the real answer, clearly made up.
fn default_reply(op: &str, args: &Value) -> Value {
    match op {
        "markdown.view" | "markdown.openFile" => json!({"id": 1}),
        "embed.mount" | "embed.update" | "embed.unmount" => json!({}),
        "embed.call" => json!({"ok": true}),
        "dash.sources" => json!([{"name": "prom", "kind": "prometheus", "language": "promql"}]),
        "dash.query" => json!({"columns": [{"name": "time", "type": "time"}, {"name": "value", "type": "number"}], "rows": [[0, 1.0]], "truncated": false}),
        "dash.panel.add" => json!({"id": "p1"}),
        "flows.list" => json!([{"id": "f1", "name": "backup", "version": 3, "inputs": [{"name": "target", "type": "string", "required": false}]}]),
        "flows.run" => json!({"run": "r1"}),
        "flows.status" => json!({"state": "running", "steps": [{"node": "start", "state": "done"}]}),
        "flows.cancel" => json!({"state": "cancelled"}),
        "k3s.status" => json!({"cluster": "dev", "namespace": "app-ns", "ready": true}),
        "k3s.get" => json!({"items": []}),
        "k3s.logs" => json!({"lines": []}),
        "k3s.plan" => json!({"plan": "1 object would be created", "lint": []}),
        "k3s.apply" => json!({"applied": 1}),
        "k3s.restart" => json!({"restarted": args.get("name").cloned().unwrap_or(Value::Null)}),
        "media.library" => json!([{"id": "m1", "title": "Song", "artist": "Band", "secs": 180, "kind": "audio"}]),
        "media.in_folders" => json!({"inside": true}),
        "media.play" => json!({"id": 1}),
        "media.pause" | "media.resume" | "media.stop" => json!({"state": op.trim_start_matches("media.")}),
        "media.state" => json!({"state": "playing", "position": 0}),
        "media.queue.add" => json!({"queued": 1}),
        "media.queue.list" => json!([]),
        "audio.play" => json!({"id": 1}),
        "audio.pause" | "audio.resume" | "audio.stop" | "audio.setVolume" => json!({}),
        "audio.outputs" => json!([{"name": "Speakers", "active": true}]),
        "speech.available" => json!({"microphone": "Fake microphone"}),
        "speech.listen" | "speech.cancel" => json!({}),
        "money.charge" => json!({"receipt": "fake-receipt-1"}),
        _ => Value::Null,
    }
}

/// The backends of one app. Cloning shares them.
#[derive(Clone)]
pub struct Backends {
    pub viewer: Arc<dyn Backend>,
    pub embed: Arc<dyn Backend>,
    pub dash: Arc<dyn Backend>,
    pub flows: Arc<dyn Backend>,
    pub k3s: Arc<dyn Backend>,
    pub playback: Arc<dyn Backend>,
    pub listener: Arc<dyn Backend>,
    pub payments: Arc<dyn Backend>,
}

impl Backends {
    /// Nothing wired: every call that needs the machine is `Unavailable` with its fix line.
    pub fn missing() -> Backends {
        let m = |what, fix| -> Arc<dyn Backend> { Arc::new(Missing { what, fix }) };
        Backends {
            viewer: m("the Markdown viewer", "Open the file in EDIT or the assistant for now; the viewer tile is not built into this screen yet."),
            embed: m("showing a viewer inside an app", "Show the content on its own with markdown.view or browser.open; embedding is not built into this screen yet."),
            dash: m("reading dashboard data for apps", "Open the DASHBOARD tile; apps cannot read its sources on this computer yet."),
            flows: m("starting flows for apps", "Open the FLOWS tile and start the flow there; apps cannot start flows on this computer yet."),
            k3s: m("the cluster for apps", "Use the K8S tile; apps get no cluster namespace on this computer yet."),
            playback: m("playing sound and media for apps", "Play it from the MEDIA tile; apps cannot control the player on this computer yet."),
            listener: m("listening through the microphone for apps", "Use dictation in the chat; apps cannot listen on this computer yet."),
            payments: m("a payment provider", "No payment provider is set up: IYAKKI does not move money for apps."),
        }
    }

    /// The same fake behind every namespace (one place to read the calls, as the emulator does). Returns the fake too.
    pub fn fake() -> (Backends, Arc<FakeBackend>) {
        let f = Arc::new(FakeBackend::default());
        let b: Arc<dyn Backend> = f.clone();
        (Backends { viewer: b.clone(), embed: b.clone(), dash: b.clone(), flows: b.clone(), k3s: b.clone(), playback: b.clone(), listener: b.clone(), payments: b }, f)
    }
}
