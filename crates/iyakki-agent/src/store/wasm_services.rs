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
}

impl WasmServices {
    pub fn new(host: TileHost) -> Arc<WasmServices> {
        Arc::new(WasmServices { host: Mutex::new(host) })
    }
}

impl ServiceBackend for WasmServices {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain != "sdk" {
            return Err(format!("no {domain} service for a WebAssembly tile here"));
        }
        let args: Value = if json.trim().is_empty() { Value::Object(Default::default()) } else { serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())? };
        let mut h = self.host.lock().unwrap_or_else(|e| e.into_inner());
        h.call(method, &args).map(|v| v.to_string())
    }
}

#[cfg(test)]
#[path = "wasm_services_tests.rs"]
mod tests;
