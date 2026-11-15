//! Chat tools for Settings -> Developer mode -> WASM tiles (experimental): `wasm_tiles_status` (free) and `wasm_tiles_set` (asks every
//! time, administrators). The switch is `Settings::wasm_tiles`, the same one the page's `W` key changes. It is off by default; while it is
//! off a tile app that carries a `.wasm` module shows why it does not start (F-WASM-09, F-APPS-45).

use serde_json::{json, Value};

use crate::settingstools::{arg_bool, no_args, Entry, Env};

const TAB: &str = "Developer";

pub const TOOLS: [&str; 2] = ["wasm_tiles_status", "wasm_tiles_set"];

pub const PROMPT_RULE: &str = "\nWASM TILES (Settings -> Developer mode): wasm_tiles_status shows whether tile apps that carry a WebAssembly module may run (experimental, off by default; free). wasm_tiles_set turns that on or off and asks first (administrators only). A module runs in the tile sandbox with time, fuel and memory limits and gets no tools or files.";

fn describe(on: bool) -> String {
    if on {
        "WASM tiles (experimental) are on: tile apps with a .wasm module run in the sandboxed tile host (time, fuel and memory limits, no tools, no files).".into()
    } else {
        "WASM tiles (experimental) are off (the default): a tile app with a .wasm module shows a message instead of running.".into()
    }
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "wasm_tiles_status", "Show whether tile apps that carry a WebAssembly module may run (experimental, off by default).", no_args(), move |_| {
            Ok(describe(e.settings.lock().map_err(|_| "the settings are busy".to_string())?.get().wasm_tiles))
        }));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "required": ["enabled"], "properties": {"enabled": {"type": "boolean", "description": "true: WASM tiles may run (experimental); false: off."}}});
        v.push(
            Entry::change(TAB, "wasm_tiles_set", "Turn the experimental WASM tiles on or off for this computer. The person approves a card first.", schema, |a| format!("Turn WASM tiles (experimental) {}", if a.get("enabled").and_then(Value::as_bool) == Some(true) { "on" } else { "off" }), move |a| {
                let on = arg_bool(a, "enabled")?;
                e.settings.lock().map_err(|_| "the settings are busy".to_string())?.update(|s| s.wasm_tiles = on).map_err(|x| x.to_string())?;
                e.audit("settings.wasm_tiles", if on { "on" } else { "off" });
                Ok(describe(on))
            })
            .admin()
            .with_preview(move |a| {
                let now = e2.settings.lock().map(|s| s.get().wasm_tiles).unwrap_or(false);
                format!("Now: {}\nAfter: {}", if now { "on" } else { "off" }, if a.get("enabled").and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" })
            })
            .with_refusal(|a| arg_bool(a, "enabled").err().map(|_| "say `enabled`: true or false".to_string())),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn the_switch_is_off_by_default_read_free_and_changed_by_administrators_with_a_card() {
        let r = rig("wasmtiles", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["wasm_tiles_status"], &["wasm_tiles_set"]);
        assert!(!r.env.settings.lock().unwrap().get().wasm_tiles, "off by default");
        assert!(t.call("wasm_tiles_status", &json!({})).unwrap().contains("are off"));
        assert!(t.refusal("wasm_tiles_set", &json!({})).is_some(), "needs enabled");
        assert!(t.preview("wasm_tiles_set", &json!({"enabled": true})).unwrap().contains("After: on"));
        t.call("wasm_tiles_set", &json!({"enabled": true})).unwrap();
        assert!(r.env.settings.lock().unwrap().get().wasm_tiles);
        assert!(t.call("wasm_tiles_status", &json!({})).unwrap().contains("are on"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.wasm_tiles on")));
        t.call("wasm_tiles_set", &json!({"enabled": false})).unwrap();
        assert!(!r.env.settings.lock().unwrap().get().wasm_tiles);
        // not offered to people who are not administrators
        let user = rig("wasmtiles-user", false);
        assert!(!tools(&user, entries(&user.env)).defs().iter().any(|d| d.spec.name == "wasm_tiles_set"));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }
}
