//! What a workspace is FOR. A request like "make a calculator app" is ambiguous: a tile for the IYAKKI launcher, a web service on a
//! cluster, an MCP server, a skill. The answer is stored in the project (`.iyakki/project.json`, `target`), chosen in the IDE's New
//! form and changeable from the header chip, and it decides the skills, the board rows and which tools exist: a workspace that
//! targets a tile app has no deploy tools, and one that targets a service has them.
//!
//! [`resolve_intent`] is the rule for a request when no target is set yet; it is a plain function so a test table can hold it.

use std::path::Path;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    IyakkiTile,
    K3sService,
    McpServer,
    Skill,
    WasmTool,
    Library,
}

impl Target {
    /// Chip order in the New form and the target menu: the tile app first.
    pub const ALL: [Target; 6] = [Target::IyakkiTile, Target::K3sService, Target::McpServer, Target::Skill, Target::WasmTool, Target::Library];

    pub fn id(self) -> &'static str {
        match self {
            Target::IyakkiTile => "iyakki-tile",
            Target::K3sService => "k3s-service",
            Target::McpServer => "mcp-server",
            Target::Skill => "skill",
            Target::WasmTool => "wasm-tool",
            Target::Library => "library/other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::IyakkiTile => "IYAKKI tile app",
            Target::K3sService => "Web service on k3s",
            Target::McpServer => "MCP server",
            Target::Skill => "Skill",
            Target::WasmTool => "WASM tool",
            Target::Library => "Library or other",
        }
    }

    pub fn from_id(s: &str) -> Option<Target> {
        let s = s.trim().to_ascii_lowercase();
        Target::ALL.into_iter().find(|t| t.id() == s || t.label().to_ascii_lowercase() == s)
    }

    /// The `app_scaffold` kind that starts a workspace of this target (none for a service or a library).
    pub fn scaffold_kind(self) -> Option<&'static str> {
        match self {
            Target::IyakkiTile => Some("tile"),
            Target::McpServer => Some("mcp-python"),
            Target::Skill => Some("skill"),
            Target::WasmTool => Some("wasm-tool"),
            Target::K3sService | Target::Library => None,
        }
    }

    /// Does a workspace of this target have the cluster deploy tools?
    pub fn deploys_to_k3s(self) -> bool {
        self == Target::K3sService
    }

    /// Does it ship to the launcher or a store as a package?
    pub fn is_package(self) -> bool {
        matches!(self, Target::IyakkiTile | Target::McpServer | Target::Skill | Target::WasmTool)
    }
}

/// The answers the assistant offers when it has to ask (the same four, in this order).
pub const ASK_OPTIONS: [&str; 4] = ["IYAKKI tile app", "Web service on k3s", "MCP server", "Skill"];
pub const ASK_QUESTION: &str = "What should this be?";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Is(Target),
    /// Not clear: ask one short question with [`ASK_OPTIONS`] before scaffolding anything.
    Ask,
}

fn has_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().last();
        let after = text[i + w.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
    })
}

/// What a request means when the workspace has no target yet.
pub fn resolve_intent(prompt: &str) -> Intent {
    let p = prompt.to_lowercase();
    let any = |ws: &[&str]| ws.iter().any(|w| has_word(&p, w) || (w.contains(' ') && p.contains(w)));
    let iyakki = any(&["iyakki", "tile", "tiles", "launcher", "add tile", "in the os", "for iyakki", "iyakki app"]);
    let strong_k3s = any(&["k3s", "kubernetes", "k8s", "docker", "dockerfile", "helm", "container", "cluster", "namespace", "pod", "ingress"]);
    let weak_k3s = any(&["deploy", "service", "api", "backend", "server"]) && !any(&["mcp"]);
    if iyakki && strong_k3s {
        return Intent::Ask;
    }
    if any(&["mcp", "mcp server", "model context protocol"]) {
        return Intent::Is(Target::McpServer);
    }
    if any(&["skill", "skills"]) && !any(&["tile"]) {
        return Intent::Is(Target::Skill);
    }
    if any(&["wasm tool", "wasm"]) {
        return Intent::Is(Target::WasmTool);
    }
    if iyakki {
        return Intent::Is(Target::IyakkiTile);
    }
    if strong_k3s || weak_k3s {
        return Intent::Is(Target::K3sService);
    }
    Intent::Ask
}

// ------------------------------------------------------------------------------------------------ the project file

/// The target recorded in `.iyakki/project.json` (`None`: not chosen, an older workspace).
pub fn read(root: &Path) -> Option<Target> {
    let t = std::fs::read_to_string(crate::workspace::ProjectConfig::path(root)).ok()?;
    let v: Value = serde_json::from_str(&t).ok()?;
    Target::from_id(v.get("target")?.as_str()?)
}

/// Record the target (creates the project file when the folder has none yet). Returns the previous one.
pub fn write(root: &Path, name: &str, t: Target) -> Result<Option<Target>, String> {
    let mut c = crate::workspace::ProjectConfig::ensure(root, name)?;
    let old = Target::from_id(&c.target);
    c.target = t.id().into();
    c.save(root)?;
    Ok(old)
}

/// Do the deploy tools exist here? A workspace with no target keeps them (it was made before targets existed); one that targets
/// anything but a service does not.
pub fn deploy_tools_offered(root: &Path) -> bool {
    read(root).is_none_or(Target::deploys_to_k3s)
}

/// The dev namespace that is still to be removed after a workspace stopped being a service (set by the conversion, cleared by the teardown).
pub fn teardown_pending(root: &Path) -> Option<String> {
    crate::projectstate::noted(root, "teardown_pending")
}

pub fn mark_teardown(root: &Path, namespace: &str) {
    crate::projectstate::note(root, "teardown_pending", namespace);
}

pub fn teardown_done(root: &Path) {
    crate::projectstate::note(root, "teardown_pending", "");
}

/// The refusal for a deploy tool in a workspace that targets something else.
pub const GUARD_TILE: &str = "this workspace targets an IYAKKI tile app; change the target to deploy to k3s";

pub fn guard_message(root: &Path) -> String {
    match read(root) {
        Some(Target::IyakkiTile) | None => GUARD_TILE.to_string(),
        Some(t) => format!("this workspace targets {}; change the target to deploy to k3s", t.label().to_lowercase()),
    }
}

/// The k3s tools that stay available while a workspace is being converted away from k3s: removing the old dev namespace.
pub fn conversion_tools() -> &'static [&'static str] {
    &["k8s_teardown"]
}

// ------------------------------------------------------------------------------------------------ board rows

/// The rows a new workspace of this target starts with: (prefix, title).
pub fn board_template(t: Target) -> Vec<(&'static str, &'static str)> {
    match t {
        Target::IyakkiTile => vec![("APP", "Build the tile app (index.html, app.js, style.css, app.json)"), ("TEST", "Headless tests with tile_test, all passing"), ("INSTALL", "Add the tile to my launcher (tile_install_dev) and open it"), ("VERIFY", "Check the tile in the launcher: opens, works, Back returns")],
        Target::K3sService => vec![("APP", "Build the service"), ("TEST", "Tests pass"), ("DEPLOY", "Deploy to k3s and verify"), ("TEST", "Integration tests as a Job after rollout")],
        Target::McpServer => vec![("APP", "Build the MCP server"), ("TEST", "Fake-client test passes"), ("PUBLISH", "Validate, pack and sign")],
        Target::Skill => vec![("APP", "Write the skill"), ("TEST", "Skill test passes"), ("PUBLISH", "Validate, pack and sign")],
        Target::WasmTool => vec![("APP", "Write the tool"), ("TEST", "Tool check passes"), ("PUBLISH", "Validate, pack and sign")],
        Target::Library => vec![("APP", "Build it"), ("TEST", "Tests pass")],
    }
}

/// Is this row (title and notes) about a cluster, a registry or an image? Such rows have no place in a workspace that is not a service.
pub fn is_cluster_row(title: &str, notes: &str) -> bool {
    let l = format!("{title} {notes}").to_lowercase();
    ["k3s", "k8s", "kubernetes", "kubectl", "namespace", "registry", "dockerfile", "docker image", "container image", "deploy_build", "deploy_load", "k8s_apply", "rollout", "ingress", "helm", "deploy to the cluster", "calc-dev", "pod", "pods", "deployment", "nodeport", "loadbalancer"]
        .iter()
        .any(|w| has_word(&l, w))
        || (has_word(&l, "deploy") && !l.contains("launcher") && !l.contains("tile") && !l.contains("store"))
        || l.contains("integration tests as a job")
}

/// What the assistant is told about the target, added to the project state.
pub fn state_line(root: &Path) -> String {
    match read(root) {
        Some(Target::IyakkiTile) => "Target: IYAKKI tile app. Build index.html, app.js, style.css and app.json; test with tile_test (headless, the real engine); validate with tile_validate; add it to the launcher with tile_install_dev (the person approves); publish with tile_pack, store_validate and store_publish. There is NO cluster work in this workspace: no Dockerfile, no registry, no deploy rows, no deploy tools. Read the skill iyakki-tile-author.".into(),
        Some(Target::K3sService) => "Target: web service on k3s (the deploy tools are available).".into(),
        Some(t) => format!("Target: {}. There is no cluster work in this workspace: no deploy tools. Read the skill iyakki-app-author.", t.label()),
        None => "Target: not chosen. If the request mentions iyakki, a tile, the launcher or the OS, build an IYAKKI tile app; if it mentions k3s, kubernetes, deploy, service, api or docker, build a service for k3s; otherwise ask ONE short question first with ask_user (options: IYAKKI tile app, Web service on k3s, MCP server, Skill) before scaffolding anything.".into(),
    }
}

// ------------------------------------------------------------------------------------------------ changing the target

/// What changing a workspace's target would mean, for the person to read before agreeing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangePlan {
    pub from: Option<Target>,
    pub to: Target,
    /// Files and folders of the old kind of project that the conversion would remove (the assistant asks for each).
    pub drop_files: Vec<String>,
    /// Board rows that are about the cluster: (id, title); they would be marked deprecated.
    pub drop_rows: Vec<(String, String)>,
    /// The dev namespace the old project used; its removal is offered with `k8s_teardown` (which always asks).
    pub teardown_namespace: Option<String>,
    pub summary: String,
}

/// Look at the workspace and say what a change to `to` would do. Reads only.
pub fn plan_change(root: &Path, name: &str, to: Target) -> ChangePlan {
    let from = read(root);
    let leaving_service = to != Target::K3sService && from.is_none_or(Target::deploys_to_k3s);
    let mut drop_files = Vec::new();
    let mut drop_rows = Vec::new();
    let mut teardown_namespace = None;
    if leaving_service {
        for f in ["server.js", "server.py", "Dockerfile", "Containerfile", ".dockerignore", "deploy"] {
            if root.join(f).exists() {
                drop_files.push(if root.join(f).is_dir() { format!("{f}/") } else { f.to_string() });
            }
        }
        if let Ok(bf) = crate::featureboard::BoardFile::load(&crate::workspace::ProjectConfig::load(root).unwrap_or_default().features_path(root)) {
            for f in bf.board.features() {
                if !f.is_bug() && f.status != "deprecated" && is_cluster_row(&f.title, &f.notes) {
                    drop_rows.push((f.id.clone(), f.title.clone()));
                }
            }
        }
        let cfg = crate::workspace::ProjectConfig::load(root).unwrap_or_default();
        if !drop_files.is_empty() || !drop_rows.is_empty() || !cfg.k8s.namespace.trim().is_empty() || from == Some(Target::K3sService) {
            teardown_namespace = Some(cfg.namespace(name));
        }
    }
    let mut summary = format!("Change the target from {} to {}.", from.map_or("not chosen", Target::label), to.label());
    if to == Target::IyakkiTile {
        summary.push_str(" The assistant will convert the project: it keeps your user interface and logic, writes index.html, app.js, style.css and app.json for a tile app, and adds headless tests.");
    }
    if !drop_files.is_empty() {
        summary.push_str(&format!(" It will propose removing: {} (you approve each).", drop_files.join(", ")));
    }
    if !drop_rows.is_empty() {
        summary.push_str(&format!(" Board rows about the cluster become deprecated: {}.", drop_rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(ns) = &teardown_namespace {
        summary.push_str(&format!(" Afterwards it offers to remove the dev namespace {ns} from the cluster (it asks first; nothing else on the cluster is touched)."));
    }
    ChangePlan { from, to, drop_files, drop_rows, teardown_namespace, summary }
}

/// Record the new target and, when the project leaves k3s, that its dev namespace may be removed. Returns the plan and the message that
/// asks the assistant to do the conversion (nothing is deleted here: files and rows change through the assistant's own tools, with approvals).
pub fn apply_change(root: &Path, name: &str, to: Target) -> Result<(ChangePlan, String), String> {
    let plan = plan_change(root, name, to);
    if plan.from == Some(to) {
        return Err(format!("this workspace already targets {}", to.label()));
    }
    write(root, name, to)?;
    if let Some(ns) = &plan.teardown_namespace {
        mark_teardown(root, ns);
    }
    let mut msg = format!("The person changed this workspace's target from {} to {}. ", plan.from.map_or("not chosen", Target::label), to.label());
    if to == Target::IyakkiTile {
        msg.push_str("Convert the project into an IYAKKI tile app. First show the plan in a few lines and ask_user whether to go ahead (options: Convert, Not now). Plan: read the existing user interface and logic; write index.html, app.js, style.css and app.json (icon, name) and a headless test with tile_scaffold-style files, keeping the same behaviour; move any server-side calculation into app.js; ");
        if !plan.drop_files.is_empty() {
            msg.push_str(&format!("remove {} one by one (each asks); ", plan.drop_files.join(", ")));
        }
        if !plan.drop_rows.is_empty() {
            msg.push_str(&format!("mark the cluster rows {} deprecated with board_update; ", plan.drop_rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", ")));
        }
        msg.push_str("add board rows: build, test (headless), install to the launcher, verify; run tile_test and tile_validate; then tile_install_dev. ");
    } else {
        msg.push_str(&format!("Adapt the workspace to {}: use the matching skill and scaffold; remove what only a cluster deployment needs, with the person's approval. ", to.label()));
    }
    if let Some(ns) = &plan.teardown_namespace {
        msg.push_str(&format!("At the end offer to remove the old dev namespace {ns} with k8s_teardown (it always asks; touch nothing else on the cluster). "));
    }
    msg.push_str("There are no deploy tools in this workspace any more.");
    Ok((plan, msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_resolution_table() {
        use Intent::{Ask, Is};
        let cases: &[(&str, Intent)] = &[
            (r#"create a iyakki calculator app deploy as a title "calc""#, Is(Target::IyakkiTile)),
            ("make a calculator app for iyakki", Is(Target::IyakkiTile)),
            ("add tile for my notes", Is(Target::IyakkiTile)),
            ("I want a todo tile in the launcher", Is(Target::IyakkiTile)),
            ("build a clock in the OS", Is(Target::IyakkiTile)),
            ("create a calculator app and add it to the launcher", Is(Target::IyakkiTile)),
            ("create a REST api for orders and deploy it to k3s", Is(Target::K3sService)),
            ("build a web service with a Dockerfile", Is(Target::K3sService)),
            ("deploy a hello app to kubernetes", Is(Target::K3sService)),
            ("write an api for todos", Is(Target::K3sService)),
            ("create an MCP server that lists files", Is(Target::McpServer)),
            ("write a skill for meeting notes", Is(Target::Skill)),
            ("an iyakki skill for emails", Is(Target::Skill)),
            ("make a wasm tool that counts words", Is(Target::WasmTool)),
            ("create an app", Ask),
            ("calc app", Ask),
            ("make a calculator", Ask),
            ("", Ask),
            ("iyakki app that talks to my k3s cluster", Ask),
            ("a tile that deploys with docker", Ask),
        ];
        for (p, want) in cases {
            assert_eq!(resolve_intent(p), *want, "{p:?}");
        }
    }

    #[test]
    fn ids_labels_and_scaffold_kinds_round_trip() {
        for t in Target::ALL {
            assert_eq!(Target::from_id(t.id()), Some(t));
            assert_eq!(Target::from_id(t.label()), Some(t));
        }
        assert_eq!(Target::from_id("nope"), None);
        assert_eq!(Target::ALL[0], Target::IyakkiTile, "the tile app is the first option");
        assert_eq!(Target::IyakkiTile.scaffold_kind(), Some("tile"));
        assert!(Target::K3sService.deploys_to_k3s() && !Target::IyakkiTile.deploys_to_k3s());
        assert_eq!(ASK_OPTIONS.len(), 4);
    }

    #[test]
    fn cluster_rows_are_recognised_and_the_tile_template_has_none() {
        for (t, n) in [("Deploy calc to k3s (calc-dev) and verify", ""), ("Integration tests as a Job after rollout", ""), ("Push the image to the registry", ""), ("Write the Dockerfile", ""), ("Verify", "check the pod is ready")] {
            assert!(is_cluster_row(t, n), "{t}");
        }
        for (t, n) in [("Calculator web app titled calc (UI)", ""), ("Add the tile to my launcher", ""), ("Headless tests pass", ""), ("Publish to my private store", "")] {
            assert!(!is_cluster_row(t, n), "{t}");
        }
        for t in Target::ALL.into_iter().filter(|t| !t.deploys_to_k3s()) {
            for (_, title) in board_template(t) {
                assert!(!is_cluster_row(title, ""), "{t:?}: {title}");
            }
        }
        assert!(board_template(Target::K3sService).iter().any(|(_, t)| is_cluster_row(t, "")));
    }
}
