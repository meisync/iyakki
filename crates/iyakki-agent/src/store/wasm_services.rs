//! The device backend of a WebAssembly tile (F-SDK-14): `call("sdk", op, json)` goes through the same bridge an HTML app's page uses
//! ([`super::tile::TileHost`] with the SDK behind it), so a WASM tile has the markdown viewer, embeds, dashboards, flows, playback and the rest
//! of SDK 1.2 with the same permissions, first-use cards, limits and audit lines, and the same refusals. Domains other than `sdk` are not served
//! (a WASM tile has no tool access from this loader): they answer an error the tile reads as unavailable.
//!
//! A call that needs the person's answer first returns `{"pending": id}` like it does on a page; the tile asks again after the person has
//! answered (the card is drawn by the tile's screen). Nothing here keeps a request longer than the call.

use std::sync::{Arc, Mutex};

use iyakki_tile_host::ServiceBackend;
use serde_json::Value;

use iyakki_js::modules::Bridge;

use super::tile::TileHost;

pub struct WasmServices {
    host: Mutex<TileHost>,
    /// The person's policy, for `dock.open`: a tile may ask for an app this person may use, nothing else (D-SDK-34-03).
    policy: Option<iyakki_sec::policy::Policy>,
    /// The app a tile asked the dock to open; the screen takes it once and hands it to the shell.
    asked: Mutex<Option<String>>,
}

impl WasmServices {
    pub fn new(host: TileHost) -> Arc<WasmServices> {
        Arc::new(WasmServices { host: Mutex::new(host), policy: None, asked: Mutex::new(None) })
    }

    /// As [`WasmServices::new`] and `dock.open` answers for the apps the person's policy allows.
    pub fn with_dock(host: TileHost, policy: iyakki_sec::policy::Policy) -> Arc<WasmServices> {
        Arc::new(WasmServices { host: Mutex::new(host), policy: Some(policy), asked: Mutex::new(None) })
    }

    /// `dock.open {"app": id}`: the id must be an app the person's policy allows; the request is kept for the shell (the screen audits it
    /// when it takes it). Nothing opens here.
    fn dock_open(&self, json: &str) -> Result<String, String> {
        let policy = self.policy.as_ref().ok_or("no dock service for a WebAssembly tile here")?;
        let v: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
        let id = v.get("app").and_then(Value::as_str).ok_or("dock.open needs an app")?;
        let app = iyakki_sec::policy::App::ALL.into_iter().find(|a| a.id() == id).ok_or("unknown app")?;
        if !policy.allows_app(app) {
            return Err("that app is not allowed for this account".into());
        }
        *self.asked.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
        Ok("{}".into())
    }
}

impl ServiceBackend for WasmServices {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain == "dock" && method == "open" {
            return self.dock_open(json);
        }
        if domain != "sdk" {
            return Err(format!("no {domain} service for a WebAssembly tile here"));
        }
        let args: Value = if json.trim().is_empty() { Value::Object(Default::default()) } else { serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())? };
        let mut h = self.host.lock().unwrap_or_else(|e| e.into_inner());
        h.call(method, &args).map(|v| v.to_string())
    }

    fn requests(&self) -> Vec<(u32, String, String)> {
        let h = self.host.lock().unwrap_or_else(|e| e.into_inner());
        h.take_requests().into_iter().map(|r| (r.id, r.kind, r.args.to_string())).collect()
    }

    fn take_dock_open(&self) -> Option<String> {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn answer(&self, id: u32, allow: bool) -> bool {
        let h = self.host.lock().unwrap_or_else(|e| e.into_inner());
        h.answer_request(id, allow)
    }
}

#[cfg(test)]
#[path = "wasm_services_tests.rs"]
mod tests;
