//! Moved out of `iyakki_agent::target` (F-SPLIT-03, bucket b0); the agent re-exports it.

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
