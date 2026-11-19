//! Serves the real policy-bridge Gate with one harmless read-only tool, prints the path of the MCP config a coding agent
//! should be given, and keeps serving for N seconds:
//!   bridge_probe /path/to/iyakki 120
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::agent::{Risk, ToolDef, Tools};
use iyakki_agent::chat::ToolSpec;
use iyakki_agent::extagent::bridge::{Bridge, Correlator, Decisions, Gate};
use serde_json::{json, Value};

struct T;
impl Tools for T {
    fn defs(&self) -> Vec<ToolDef> {
        vec![ToolDef { spec: ToolSpec { name: "list_dir".into(), description: "List the workspace folder".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false }]
    }
    fn call(&self, _n: &str, _a: &Value) -> Result<String, String> {
        Ok("PROBE-LISTING a.txt b.txt".into())
    }
    fn describe(&self, n: &str, _a: &Value) -> String {
        n.into()
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    // `bridge_probe relay SOCKET`: the relay of `iyakki --mcp-tools`, to try the code in this tree
    if matches!(a.first().map(String::as_str), Some("relay" | "--mcp-tools")) {
        std::process::exit(iyakki_agent::extagent::bridge::relay_stdio(a.get(1).map(String::as_str).unwrap_or("")));
    }
    let exe = std::path::PathBuf::from(a.first().cloned().unwrap_or("/usr/bin/iyakki".into()));
    let secs: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let (etx, _erx) = channel();
    let (_dtx, drx) = channel();
    let gate = Arc::new(Gate::new(Arc::new(T), false, etx, Decisions::new(drx), Arc::new(Correlator::default()), Arc::new(AtomicBool::new(false)), Duration::from_secs(5), "Probe", Arc::new(|k, d| eprintln!("audit {k} {d}")), std::path::Path::new("/tmp"), 12_000));
    let b = Bridge::start(gate, &exe).unwrap();
    println!("{}", iyakki_agent::extagent::bridge::copilot_config(&b.config).display());
    println!("{}", b.config.display());
    std::thread::sleep(Duration::from_secs(secs));
}
