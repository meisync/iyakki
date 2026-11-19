//! A strategy written as a WebAssembly module (F-ROUTER-13), the second kind beside the script of `router_js`.
//!
//! The module exports `decide() -> i32`. It reads the strategy input (JSON) with `iyakki.stdin_read` and writes the decision (JSON) with
//! `iyakki.log`. It is run with NO capability token, no network and no file, so the only things it can do are compute and answer. Fuel,
//! memory and wall-clock time are limited, a fresh sandbox is made for every call, and what it answers is held to the strategy contract by
//! [`crate::router::Guarded`] exactly like a script's. The source is the module's text format (WAT) or `wasm-hex:` and the binary module in
//! hexadecimal, so it can be shown on the approval card and kept in the person's strategies folder like a script.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_sec::CapabilityStore;
use iyakki_vfs::MemFs;
use iyakki_wasm::{Sandbox, SandboxConfig, WasmError};
use serde_json::Value;

use crate::router::{Strategy, MAX_DECISION_BYTES, MAX_INPUT_BYTES};

/// The longest module text or hex a strategy may have.
pub const MAX_WASM_SOURCE_BYTES: usize = 256 * 1024;
/// The prefix of a binary module written in hexadecimal.
pub const HEX_PREFIX: &str = "wasm-hex:";

/// The sample the store lists (dist/samples/router-strategies): the first listed model plans and runs the first ready step.
pub const SAMPLE_FIRST_MODEL: &str = include_str!("../../../dist/samples/router-strategies/first-model.wat");

/// Whether a strategy source is a WebAssembly one (module text or hex) and not a script.
pub fn is_wasm_source(source: &str) -> bool {
    let t = source.trim_start();
    t.starts_with(HEX_PREFIX) || t.starts_with("(module") || (t.starts_with(";;") && t.contains("(module"))
}

#[derive(Debug)]
pub struct WasmStrategy {
    name: String,
    module: Vec<u8>,
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if s.len() % 2 != 0 {
        return None;
    }
    s.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
}

impl WasmStrategy {
    pub fn new(name: &str, source: &str) -> Result<WasmStrategy, String> {
        if !crate::router_js::valid_name(name) {
            return Err("a strategy name is lowercase letters, digits, - and _, starting with a letter, at most 32, and not `default`".into());
        }
        if source.len() > MAX_WASM_SOURCE_BYTES {
            return Err(format!("the module is longer than {MAX_WASM_SOURCE_BYTES} bytes"));
        }
        let t = source.trim();
        let module = match t.strip_prefix(HEX_PREFIX) {
            Some(h) => unhex(h).ok_or("the hexadecimal module is not valid")?,
            None => t.as_bytes().to_vec(),
        };
        // load it once now so a module that does not load, or has no `decide`, is refused when it is added and not in the middle of a run
        let me = WasmStrategy { name: name.into(), module };
        me.decide(&serde_json::json!({})).map_err(|e| if e.starts_with("garbage") { e } else { e })?;
        Ok(me)
    }

    fn sandbox(&self) -> Result<Sandbox, String> {
        Sandbox::new(SandboxConfig { fuel: 50_000_000, max_memory_bytes: 16 << 20 }).map_err(classify)
    }
}

fn classify(e: WasmError) -> String {
    match e {
        WasmError::OutOfFuel | WasmError::Timeout => "timeout: the module ran too long".into(),
        WasmError::Cancelled => "error: the module was stopped".into(),
        WasmError::NoEntry(_) => "error: the module has no decide function".into(),
        other => format!("error: the sandbox stopped the module ({})", other.to_string().chars().take(80).collect::<String>()),
    }
}

impl Strategy for WasmStrategy {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn decide(&self, input: &Value) -> Result<Value, String> {
        let text = serde_json::to_string(input).map_err(|e| format!("error: {e}"))?;
        if text.len() > MAX_INPUT_BYTES {
            return Err("size: the input is too big for a module".into());
        }
        let sb = self.sandbox()?.with_wall_time(Duration::from_millis(1500)).with_stdin(text.into_bytes());
        let out = sb.run(&self.module, "decide", Arc::new(Mutex::new(MemFs::new())), CapabilityStore::new(), vec![]).map_err(classify)?;
        if out.log.len() > MAX_DECISION_BYTES {
            return Err("size: the answer is too big".into());
        }
        serde_json::from_slice(&out.log).map_err(|_| "garbage: the answer is not JSON".to_string())
    }
}

/// The strategy a source describes, a script or a module.
pub fn make(name: &str, source: &str) -> Result<Arc<dyn Strategy>, String> {
    if is_wasm_source(source) {
        Ok(Arc::new(WasmStrategy::new(name, source)?))
    } else {
        Ok(Arc::new(crate::router_js::JsStrategy::new(name, source)?))
    }
}
