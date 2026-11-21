//! Chat tools for the web-bundle tiles of a tile host (F-WEBSVC-12, KI-821): `tile_bundle_list` (safe), `tile_bundle_install` and `tile_bundle_remove`
//! (both ask on a card every time). They are a wrapper over the tile host of the web service: the same rules as `POST /api/v1/tile-bundles`
//! (names, types, sizes, no inline code, calls only from the short list, never `tools.*`), the same folder, the same audit lines (the bundle id and
//! the sizes, never the files). Only an administrator is offered them, as the routes are for administrators. The card shows the id, the title, the
//! calls the page may make and every file with its size, so the person sees what is being put on the service before saying yes.

use super::server::Fail;
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use serde_json::{json, Value};
use std::sync::Arc;

pub const LIST_TOOL: &str = "tile_bundle_list";
pub const INSTALL_TOOL: &str = "tile_bundle_install";
pub const REMOVE_TOOL: &str = "tile_bundle_remove";
pub const NAMES: [&str; 3] = [LIST_TOOL, INSTALL_TOOL, REMOVE_TOOL];

/// What the tools need from a tile host. A fake in the tests; [`HostBundles`] in the service.
pub trait BundleBackend: Send + Sync {
    /// (id, title, caps, file names) of each installed bundle.
    fn list(&self) -> Vec<(String, String, Vec<String>, Vec<String>)>;
    /// Install or replace; the line to tell the person.
    fn install(&self, id: &str, title: &str, caps: &[String], files: &[(String, Vec<u8>)]) -> Result<String, String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}

/// The real backend: the service's own tile host and its data folder.
pub struct HostBundles {
    pub host: Arc<super::tiles::TileHost>,
    pub data_dir: String,
    pub audit: Arc<dyn iyakki_core_api::AuditSink>,
    pub user: String,
}

fn fail(f: Fail) -> String {
    f.message
}

impl BundleBackend for HostBundles {
    fn list(&self) -> Vec<(String, String, Vec<String>, Vec<String>)> {
        self.host.bundles.list().iter().map(|b| (b.id.clone(), b.title.clone(), b.caps.clone(), b.files.keys().cloned().collect())).collect()
    }
    fn install(&self, id: &str, title: &str, caps: &[String], files: &[(String, Vec<u8>)]) -> Result<String, String> {
        let host = &self.host;
        let b = host.bundles.install(&super::webbundle::bundles_dir(&self.data_dir), &|id| host.list().iter().any(|d| d.id == id), id, title, caps, files).map_err(fail)?;
        self.audit.record(&self.user, "tile_bundle_install", &format!("bundle={} files={} bytes={} by=chat", b.id, b.files.len(), b.size()));
        Ok(format!("Installed the tile \"{}\" (id {}, {} files). Open it at /tiles/{}/w on the tile host; sign in there.", b.title, b.id, b.files.len(), b.id))
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.host.bundles.remove(&super::webbundle::bundles_dir(&self.data_dir), id).map_err(fail)?;
        self.audit.record(&self.user, "tile_bundle_remove", &format!("bundle={id} by=chat"));
        Ok(())
    }
}

/// The wrapper the run manager applies for a tile host: administrators get the bundle tools, everyone else the set as it was.
pub fn wrap(host: Arc<super::tiles::TileHost>, data_dir: String, audit: Arc<dyn iyakki_core_api::AuditSink>) -> super::runs::ToolWrap {
    Arc::new(move |inner, p| {
        if !p.admin {
            return inner;
        }
        Arc::new(BundleTools::new(inner, Arc::new(HostBundles { host: host.clone(), data_dir: data_dir.clone(), audit: audit.clone(), user: p.user.clone() })))
    })
}

pub struct BundleTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn BundleBackend>,
}

/// The files of an install call: `[{name, text}]` or `[{name, base64}]`.
fn files_of(args: &Value) -> Result<Vec<(String, Vec<u8>)>, String> {
    let Some(list) = args.get("files").and_then(Value::as_array) else { return Err("files is a list of {name, text} (or {name, base64} for a picture)".into()) };
    let mut out = Vec::new();
    for f in list {
        let name = f.get("name").and_then(Value::as_str).ok_or("every file has a name")?.to_string();
        let bytes = match (f.get("text").and_then(Value::as_str), f.get("base64").and_then(Value::as_str)) {
            (Some(t), None) => t.as_bytes().to_vec(),
            (None, Some(b)) => crate::models::b64_decode(b).ok_or_else(|| format!("{name}: base64 is not valid"))?,
            _ => return Err(format!("{name}: give either text or base64")),
        };
        out.push((name, bytes));
    }
    Ok(out)
}

fn caps_of(args: &Value) -> Result<Vec<String>, String> {
    match args.get("caps") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a.iter().map(|c| c.as_str().map(String::from).ok_or_else(|| "caps is a list of names like clipboard.paste".to_string())).collect(),
        _ => Err("caps is a list of names like clipboard.paste".into()),
    }
}

impl BundleTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn BundleBackend>) -> BundleTools {
        BundleTools { inner, backend }
    }

    fn mine(name: &str) -> bool {
        NAMES.contains(&name)
    }
}

impl Tools for BundleTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !Self::mine(&d.spec.name));
        let def = |name: &str, description: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: description.into(), schema }, risk, untrusted: false };
        v.push(def(LIST_TOOL, "List the web-bundle tiles installed on this tile host (id, title, the calls each may make, its files). No approval needed.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v.push(def(
            INSTALL_TOOL,
            "Install (or replace) a web-bundle tile on this tile host: a small web page made of plain files (index.html and its .js, .css, .json, .txt, .png, .jpg, .gif, .webp, .woff2 files; at most 48 files, 1 MiB each, 4 MiB in all). No inline script or on... handlers: code goes in .js files. The page loads /tiles/bridge.js and uses window.iyakki.call(name, args); it may call only the `caps` listed (for example clipboard.copy, clipboard.paste, files.read, settings.get; never tools.*). It runs in a sandboxed frame with no access to the service. Always asks the person.",
            json!({"type": "object", "properties": {"id": {"type": "string", "description": "a-z, 0-9, - and _, at most 40"}, "title": {"type": "string"}, "caps": {"type": "array", "items": {"type": "string"}}, "files": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "text": {"type": "string"}, "base64": {"type": "string"}}, "required": ["name"]}}}, "required": ["id", "title", "files"]}),
            Risk::Ask,
        ));
        v.push(def(REMOVE_TOOL, "Remove a web-bundle tile from this tile host (its files are deleted). Always asks the person.", json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}), Risk::Ask));
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST_TOOL => {
                let rows = self.backend.list();
                if rows.is_empty() {
                    return Ok("No web-bundle tiles are installed on this tile host.".into());
                }
                let mut s = String::from("Web-bundle tiles on this tile host:\n");
                for (id, title, caps, files) in rows {
                    s.push_str(&format!("- {title} (id {id}, open at /tiles/{id}/w): may call [{}]; files: {}\n", caps.join(", "), files.join(", ")));
                }
                Ok(s)
            }
            INSTALL_TOOL => {
                let id = args["id"].as_str().unwrap_or("");
                let title = args["title"].as_str().unwrap_or("");
                self.backend.install(id, title, &caps_of(args)?, &files_of(args)?)
            }
            REMOVE_TOOL => {
                let id = args["id"].as_str().filter(|i| !i.is_empty()).ok_or("say which tile (id)")?;
                self.backend.remove(id)?;
                Ok(format!("Removed the tile {id}."))
            }
            _ => self.inner.call(name, args),
        }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            LIST_TOOL => "List the web-bundle tiles of this tile host".into(),
            INSTALL_TOOL => format!("Install the web tile \"{}\" ({}) on this tile host", args["title"].as_str().unwrap_or("?").chars().take(60).collect::<String>(), args["id"].as_str().unwrap_or("?").chars().take(40).collect::<String>()),
            REMOVE_TOOL => format!("Remove the web tile {} from this tile host", args["id"].as_str().unwrap_or("?").chars().take(40).collect::<String>()),
            _ => self.inner.describe(name, args),
        }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            INSTALL_TOOL => {
                let files = files_of(args).ok()?;
                let caps = caps_of(args).unwrap_or_default();
                let mut s = format!("Tile {} \"{}\"\nThe page may call: {}\nFiles:\n", args["id"].as_str().unwrap_or("?"), args["title"].as_str().unwrap_or("?"), if caps.is_empty() { "nothing".into() } else { caps.join(", ") });
                for (n, b) in &files {
                    s.push_str(&format!("  {n} ({} bytes)\n", b.len()));
                }
                s.push_str("It runs in a sandboxed frame; anyone who opens it still signs in, and each call is checked against that person's permissions.");
                Some(s)
            }
            LIST_TOOL | REMOVE_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if Self::mine(name) {
            return None;
        }
        self.inner.refusal(name, args)
    }
    fn pre_approved(&self, name: &str) -> bool {
        !Self::mine(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        !Self::mine(name) && self.inner.approved_always(name)
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if Self::mine(name) {
            return Some("installing or removing a tile always asks; it cannot be allowed for later".into());
        }
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
    use std::sync::Mutex;

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

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }
    impl BundleBackend for Fake {
        fn list(&self) -> Vec<(String, String, Vec<String>, Vec<String>)> {
            vec![("notes".into(), "Notes".into(), vec!["clipboard.copy".into()], vec!["index.html".into(), "app.js".into()])]
        }
        fn install(&self, id: &str, title: &str, caps: &[String], files: &[(String, Vec<u8>)]) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("install {id} {title} {} {}", caps.join(","), files.iter().map(|f| format!("{}={}", f.0, f.1.len())).collect::<Vec<_>>().join(",")));
            Ok("installed".into())
        }
        fn remove(&self, id: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("remove {id}"));
            Ok(())
        }
    }

    fn tools() -> (BundleTools, Arc<Fake>) {
        let f = Arc::new(Fake::default());
        (BundleTools::new(Arc::new(Inner), f.clone()), f)
    }

    #[test]
    fn list_is_safe_install_and_remove_always_ask_and_cannot_be_granted_for_later() {
        let (t, _) = tools();
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
        assert_eq!((risk(LIST_TOOL), risk(INSTALL_TOOL), risk(REMOVE_TOOL)), (Risk::Safe, Risk::Ask, Risk::Ask));
        assert!(defs.iter().any(|d| d.spec.name == "now"));
        for n in [INSTALL_TOOL, REMOVE_TOOL] {
            assert!(t.grant_blocker(n, &json!({})).is_some(), "{n}");
            assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
        }
        assert!(t.grant_blocker("now", &json!({})).is_none());
        assert_eq!(t.call("now", &json!({})).unwrap(), "inner now");
    }

    #[test]
    fn an_install_passes_the_files_and_calls_to_the_host_and_the_card_shows_what_will_be_put_there() {
        let (t, f) = tools();
        let args = json!({"id": "notes", "title": "Notes", "caps": ["clipboard.copy"], "files": [{"name": "index.html", "text": "<p>x</p>"}, {"name": "dot.png", "base64": "AAEC"}]});
        assert_eq!(t.call(INSTALL_TOOL, &args).unwrap(), "installed");
        assert_eq!(f.calls.lock().unwrap()[0], "install notes Notes clipboard.copy index.html=8,dot.png=3");
        let card = t.preview(INSTALL_TOOL, &args).unwrap();
        assert!(card.contains("index.html (8 bytes)") && card.contains("dot.png (3 bytes)") && card.contains("clipboard.copy") && card.contains("sandboxed"), "{card}");
        assert!(t.describe(INSTALL_TOOL, &args).contains("Notes"));
        // bad arguments never reach the host
        for bad in [json!({"id": "n", "title": "t"}), json!({"id": "n", "title": "t", "files": [{"name": "a.js"}]}), json!({"id": "n", "title": "t", "files": [{"name": "a.js", "text": "x", "base64": "AA=="}]}), json!({"id": "n", "title": "t", "caps": "all", "files": []}), json!({"id": "n", "title": "t", "files": [{"name": "a.js", "base64": "!!"}]})] {
            assert!(t.call(INSTALL_TOOL, &bad).is_err(), "{bad}");
        }
        assert_eq!(f.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn list_and_remove_work_and_other_tools_go_to_the_inner_set() {
        let (t, f) = tools();
        let text = t.call(LIST_TOOL, &json!({})).unwrap();
        assert!(text.contains("Notes") && text.contains("/tiles/notes/w") && text.contains("clipboard.copy") && text.contains("app.js"), "{text}");
        assert!(t.call(REMOVE_TOOL, &json!({})).is_err());
        assert!(t.call(REMOVE_TOOL, &json!({"id": "notes"})).unwrap().contains("Removed"));
        assert_eq!(f.calls.lock().unwrap().last().unwrap(), "remove notes");
        assert_eq!(t.describe("now", &json!({})), "inner now");
    }

    #[derive(Default)]
    struct Lines(Mutex<Vec<String>>);
    impl iyakki_core_api::AuditSink for Lines {
        fn record(&self, user: &str, event: &str, detail: &str) {
            self.0.lock().unwrap().push(format!("{user} {event} {detail}"));
        }
    }

    #[test]
    fn the_real_backend_installs_through_the_hosts_rules_lists_removes_and_audits_without_the_files() {
        let dir = std::env::temp_dir().join(format!("iyakki-bundletools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(super::super::webbundle::bundles_dir(dir.to_str().unwrap())).unwrap();
        let host = Arc::new(super::super::tiles::TileHost::new(super::super::tiles::builtin()));
        let audit = Arc::new(Lines::default());
        let admin = iyakki_core_api::Principal { user: "root".into(), role: "admin".into(), admin: true, policy: iyakki_sec::policy::Role::Admin.template() };
        let guest = iyakki_core_api::Principal { user: "ann".into(), role: "user".into(), admin: false, policy: iyakki_sec::policy::Role::User.template() };
        let w = wrap(host.clone(), dir.to_string_lossy().into_owned(), audit.clone());
        // a person who is not an administrator is not offered the tools
        let plain = w(Arc::new(Inner), &guest);
        assert!(!plain.defs().iter().any(|d| NAMES.contains(&d.spec.name.as_str())));
        let t = w(Arc::new(Inner), &admin);
        assert!(NAMES.iter().all(|n| t.defs().iter().any(|d| d.spec.name == *n)));
        let page = json!({"id": "pastebox", "title": "Paste box", "caps": ["clipboard.paste"], "files": [{"name": "index.html", "text": "<script src=\"/tiles/bridge.js\"></script>"}, {"name": "app.js", "text": "window.iyakki.onready(function(){})"}]});
        let said = t.call(INSTALL_TOOL, &page).unwrap();
        assert!(said.contains("/tiles/pastebox/w"), "{said}");
        assert!(host.bundles.get("pastebox").is_some() && dir.join("tilebundles").join("pastebox").join("index.html").exists());
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("Paste box"));
        // the host's own rules hold: inline code, the id of a built-in tile, a tool call in the manifest
        let inline = json!({"id": "bad", "title": "t", "files": [{"name": "index.html", "text": "<script>x()</script>"}]});
        assert!(t.call(INSTALL_TOOL, &inline).unwrap_err().contains("inline"));
        let builtin = host.list()[0].id.clone();
        assert!(t.call(INSTALL_TOOL, &json!({"id": builtin, "title": "t", "files": [{"name": "index.html", "text": "<p>"}]})).unwrap_err().contains("already"));
        assert!(t.call(INSTALL_TOOL, &json!({"id": "tool", "title": "t", "caps": ["tools.run_command"], "files": [{"name": "index.html", "text": "<p>"}]})).is_err());
        t.call(REMOVE_TOOL, &json!({"id": "pastebox"})).unwrap();
        assert!(host.bundles.get("pastebox").is_none() && t.call(REMOVE_TOOL, &json!({"id": "pastebox"})).is_err());
        let lines = audit.0.lock().unwrap().join("\n");
        assert!(lines.contains("root tile_bundle_install bundle=pastebox files=2") && lines.contains("tile_bundle_remove bundle=pastebox") && !lines.contains("window.iyakki") && !lines.contains("bridge.js"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
