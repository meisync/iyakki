//! A strategy written by the person, run in the JavaScript sandbox (F-ROUTER-05).
//!
//! The script defines one function, `decide(input)`, which gets the strategy contract's input (see `router.rs`) as an object and returns
//! the decision object. It runs in the same sandbox the browser uses for page scripts (QuickJS compiled to WebAssembly, `iyakki-js`),
//! started with NO host modules: no `fetch`, no storage, no timers, no console, no file, program or network access, nothing but the
//! language. Memory, instruction count and wall-clock time are limited, the input and the answer are size limited, and a fresh engine is
//! made for every call so nothing is remembered between calls. A script that throws, loops, runs out of time or memory, answers too much
//! or answers something that is not JSON is reported as an error to [`crate::router::Guarded`], which stops it for the run.

use std::time::Duration;

use iyakki_js::{JsEngine, JsError, Limits};
use serde_json::Value;

use crate::router::{Strategy, MAX_DECISION_BYTES, MAX_INPUT_BYTES};
pub use iyakki_shared::router_js::{MAX_SOURCE_BYTES};


/// The limits of one `decide` call.
pub fn limits() -> Limits {
    // fuel is counted in interrupt ticks of about 150 000 WebAssembly instructions each
    Limits { memory_bytes: 64 << 20, fuel: 150_000 * 3000, wall: Duration::from_millis(1500), stack_bytes: 256 << 10 }
}

pub struct JsStrategy {
    name: String,
    source: String,
}

/// A strategy name: lowercase letters, digits, - and _, at most 32; `default` is the built-in one and cannot be used.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s != crate::router::DEFAULT_STRATEGY && s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl JsStrategy {
    pub fn new(name: &str, source: &str) -> Result<JsStrategy, String> {
        if !valid_name(name) {
            return Err("a strategy name is lowercase letters, digits, - and _, starting with a letter, at most 32, and not `default`".into());
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err(format!("the script is longer than {MAX_SOURCE_BYTES} bytes"));
        }
        if source.trim().is_empty() {
            return Err("the script is empty".into());
        }
        Ok(JsStrategy { name: name.into(), source: source.into() })
    }
}

fn classify(e: JsError) -> String {
    match e {
        JsError::OutOfFuel | JsError::Timeout => "timeout: the script ran too long".into(),
        JsError::Sandbox(m) => format!("error: the sandbox stopped the script ({})", m.chars().take(80).collect::<String>()),
        JsError::Exception(m) => format!("error: the script threw ({})", m.lines().next().unwrap_or("").chars().take(120).collect::<String>()),
    }
}

impl Strategy for JsStrategy {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn decide(&self, input: &Value) -> Result<Value, String> {
        let text = serde_json::to_string(input).map_err(|e| format!("error: {e}"))?;
        if text.len() > MAX_INPUT_BYTES {
            return Err("size: the input is too big for a script".into());
        }
        // the input travels as a string literal (JSON text is a valid JSON string), so a script cannot be confused by its content
        let literal = serde_json::to_string(&text).map_err(|e| format!("error: {e}"))?;
        let script = format!(
            "var __iyakki_in = {literal};\n{src}\n;(function () {{ if (typeof decide !== 'function') {{ throw new Error('the script defines no decide function'); }} var r = decide(JSON.parse(__iyakki_in)); return JSON.stringify(r === undefined ? null : r); }})()",
            src = self.source
        );
        let mut engine = JsEngine::new(limits(), vec![]).map_err(|e| format!("error: the script engine could not start ({})", e.chars().take(80).collect::<String>()))?;
        let out = engine.eval(&format!("strategy:{}", self.name), &script).map_err(classify)?;
        if out.len() > MAX_DECISION_BYTES {
            return Err("size: the answer is too big".into());
        }
        serde_json::from_str(&out).map_err(|_| "garbage: the answer is not JSON".to_string())
    }
}
