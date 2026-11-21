//! The Settings -> Sound row for Bluetooth headphones and speakers (F-NET-28): the page asks the core, never the Bluetooth stack. This file
//! builds the same tools the assistant uses (`btaudiotools`) over the machine's real Bluetooth and asks them, so a click on the page and a
//! chat request do exactly the same thing: connect the paired pair, send ALL of the computer's sound to it (SBC), mute the computer's own
//! speakers while it runs, and give them back on stop. The pair is the one named, else the only one paired, else the remembered one
//! (`bluetooth-sink.json`); nothing connects by itself.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{ToolDef, Tools};
use crate::btaudiotools::BtAudioTools;

/// The tools below have nothing of their own to pass on.
struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        Vec::new()
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("{name} is not a tool here"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

pub fn tools(data: &Path) -> BtAudioTools {
    BtAudioTools::real(Arc::new(NoTools)).with_memory(data.join("bluetooth-sink.json"))
}

/// The tools for reading only (no remembered pair is needed to look).
pub fn view_tools() -> BtAudioTools {
    BtAudioTools::real(Arc::new(NoTools))
}

pub fn active(t: &BtAudioTools) -> bool {
    t.sending()
}

pub fn start(data: &Path, address: &str) -> Result<String, String> {
    start_with(&tools(data), address)
}

pub fn stop(data: &Path) -> Result<String, String> {
    stop_with(&tools(data))
}

pub fn start_with(t: &BtAudioTools, address: &str) -> Result<String, String> {
    let args = if address.trim().is_empty() { json!({}) } else { json!({"device": address.trim()}) };
    t.call("bluetooth_audio_send_start", &args)
}

pub fn stop_with(t: &BtAudioTools) -> Result<String, String> {
    t.call("bluetooth_audio_send_stop", &json!({}))
}

#[cfg(test)]
#[path = "btsound_tests.rs"]
mod tests;
