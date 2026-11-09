//! The assistant's way to the browser tiles (docs/development/TILE-ABI.md, "In a browser"): `tiles_list` (safe, no approval) names the
//! WASM tiles this machine can show in the Tiles page of the web UI and says where to open it. Opening and using a tile is the
//! person's own action in the browser; the tile's service calls are checked against the person's permissions there, so the assistant
//! needs no tool to drive one.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const LIST_TOOL: &str = "tiles_list";

pub const PROMPT_RULE: &str = "\nTILES IN A BROWSER: tiles_list names the small apps (WASM tiles) that run on this machine and can be opened in the Tiles page of the web UI (https://<this machine>:8443/ui/#/tiles, or the cloud address). Tell the person where to click; you cannot open or operate a tile for them.";

pub struct TilesTools {
    inner: Arc<dyn Tools>,
    /// (id, title) of each tile.
    catalog: Vec<(String, String)>,
}

impl TilesTools {
    pub fn new(inner: Arc<dyn Tools>, catalog: Vec<(String, String)>) -> TilesTools {
        TilesTools { inner, catalog }
    }

    /// The tiles every machine has (what the web service offers at `GET /api/v1/tiles`).
    pub fn builtin_catalog() -> Vec<(String, String)> {
        crate::webservice::tiles::describe(&crate::webservice::tiles::TileHost::new(crate::webservice::tiles::builtin()))
    }

    fn list(&self) -> String {
        if self.catalog.is_empty() {
            return "No tiles are installed on this machine.".into();
        }
        let mut s = String::from("Tiles that can be opened in the web UI, page Tiles (#/tiles):\n");
        for (id, title) in &self.catalog {
            s.push_str(&format!("- {title} (id {id})\n"));
        }
        s.push_str("They run on this machine in the tile sandbox; what each may do is decided by the person's permissions.");
        s
    }
}

impl Tools for TilesTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != LIST_TOOL);
        v.push(ToolDef { spec: ToolSpec { name: LIST_TOOL.into(), description: "List the WASM tiles (small apps) that can be opened in the Tiles page of the web UI of this machine. No approval needed.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == LIST_TOOL { Ok(self.list()) } else { self.inner.call(name, args) }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        if name == LIST_TOOL { "List the tiles that can be opened in the browser".into() } else { self.inner.describe(name, args) }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == LIST_TOOL { None } else { self.inner.preview(name, args) }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == LIST_TOOL { None } else { self.inner.refusal(name, args) }
    }
    fn pre_approved(&self, name: &str) -> bool {
        self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        self.inner.approved_always(name)
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Inner;
    impl Tools for Inner {
        fn defs(&self) -> Vec<ToolDef> {
            vec![ToolDef { spec: ToolSpec { name: "now".into(), description: "n".into(), schema: json!({"type": "object"}) }, risk: Risk::Safe, untrusted: false }]
        }
        fn call(&self, name: &str, _: &Value) -> Result<String, String> {
            Ok(format!("inner {name}"))
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            format!("inner {name}")
        }
    }

    #[test]
    fn tiles_list_is_a_safe_tool_that_names_the_example_tile_and_forwards_the_rest() {
        let t = TilesTools::new(Arc::new(Inner), TilesTools::builtin_catalog());
        let defs = t.defs();
        let d = defs.iter().find(|d| d.spec.name == LIST_TOOL).expect("offered");
        assert_eq!(d.risk, Risk::Safe, "no approval needed");
        assert!(defs.iter().any(|d| d.spec.name == "now"), "the other tools stay");
        let out = t.call(LIST_TOOL, &json!({})).unwrap();
        assert!(out.contains("counter") && out.contains("#/tiles"), "{out}");
        assert_eq!(t.call("now", &json!({})).unwrap(), "inner now");
        assert!(t.refusal(LIST_TOOL, &json!({})).is_none() && t.preview(LIST_TOOL, &json!({})).is_none());
        assert!(t.describe(LIST_TOOL, &json!({})).contains("tiles"));
        assert!(TilesTools::new(Arc::new(Inner), vec![]).call(LIST_TOOL, &json!({})).unwrap().contains("No tiles"));
    }
}
