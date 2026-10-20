//! Role-based access control: which apps a user may open, and which files, hosts, MCP
//! servers and compute budget their sessions get. Roles give sensible defaults; an admin
//! can refine one user with a [`PolicyOverride`]. Policies are *data*; enforcement happens
//! where the work runs (sandbox limits, capability tokens, app gating).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Capability, CapabilityStore, SecError, SyscallToken};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum App {
    Chat,
    Rag,
    Mcp,
    Cli,
    Users,
    Settings,
    Runs,
    Edit,
    Browser,
    Mesh,
    Radio,
    Scan,
    Skills,
    Usage,
    Calendar,
    System,
    Media,
    SrePilot,
    Inbox,
    Ide,
    Data,
    Tales,
    Store,
    Dash,
    Flows,
    Camera,
}

impl App {
    pub const ALL: [App; 26] = [App::Chat, App::Rag, App::Mcp, App::Cli, App::Users, App::Settings, App::Runs, App::Edit, App::Browser, App::Mesh, App::Radio, App::Scan, App::Skills, App::Usage, App::Calendar, App::System, App::Media, App::SrePilot, App::Inbox, App::Ide, App::Data, App::Tales, App::Store, App::Dash, App::Flows, App::Camera];

    pub fn id(self) -> &'static str {
        match self {
            App::Chat => "chat",
            App::Rag => "rag",
            App::Mcp => "mcp",
            App::Cli => "cli",
            App::Users => "users",
            App::Settings => "settings",
            App::Runs => "runs",
            App::Edit => "edit",
            App::Browser => "browser",
            App::Mesh => "mesh",
            App::Radio => "radio",
            App::Scan => "scan",
            App::Skills => "skills",
            App::Usage => "usage",
            App::Calendar => "calendar",
            App::System => "system",
            App::Media => "media",
            App::SrePilot => "srepilot",
            App::Inbox => "inbox",
            App::Ide => "ide",
            App::Data => "data",
            App::Store => "store",
            App::Tales => "tales",
            App::Dash => "dash",
            App::Flows => "flows",
            App::Camera => "camera",
        }
    }

    /// The name shown on tiles and in the user editor (the terminal's saved id stays `cli`).
    pub fn label(self) -> String {
        match self {
            App::Cli => "CLI".to_string(),
            App::Scan => "NET SCAN".to_string(),
            App::SrePilot => "SRE PILOT".to_string(),
            App::Ide => "AI IDE".to_string(),
            other => other.id().to_uppercase(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    User,
    Guest,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Admin, Role::Operator, Role::User, Role::Guest];

    pub fn name(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::User => "user",
            Role::Guest => "guest",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == s)
    }

    /// Next role up/down, for simple UI cycling.
    pub fn cycle(self) -> Role {
        match self {
            Role::Admin => Role::Operator,
            Role::Operator => Role::User,
            Role::User => Role::Guest,
            Role::Guest => Role::Admin,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliLevel {
    /// No terminal.
    Off,
    /// Allow-listed, read-mostly commands confined to the user's file scopes.
    Restricted,
    /// All built-in commands.
    Full,
}

/// What the assistant may do in the person's own browser tab (the full web engine): see `docs/BROWSER-AGENT.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAgent {
    /// No browser tools at all.
    Off,
    /// Open pages, snapshots, screenshots, scroll, wait: nothing is clicked or typed.
    ReadOnly,
    /// As read-only, and click/type/select/submit after the person's per-domain approval.
    Interactive,
}

impl BrowserAgent {
    pub const ALL: [BrowserAgent; 3] = [BrowserAgent::Off, BrowserAgent::ReadOnly, BrowserAgent::Interactive];
    pub fn name(self) -> &'static str {
        match self {
            BrowserAgent::Off => "off",
            BrowserAgent::ReadOnly => "read_only",
            BrowserAgent::Interactive => "interactive",
        }
    }
    /// The next level when cycling in Settings (off, read-only, interactive).
    pub fn next(self) -> BrowserAgent {
        match self {
            BrowserAgent::Off => BrowserAgent::ReadOnly,
            BrowserAgent::ReadOnly => BrowserAgent::Interactive,
            BrowserAgent::Interactive => BrowserAgent::Off,
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            BrowserAgent::Off => "the assistant cannot use your browser",
            BrowserAgent::ReadOnly => "the assistant can look at pages in your browser, not click or type",
            BrowserAgent::Interactive => "the assistant can click and type in your browser after you approve a site",
        }
    }
}

fn default_browser_actions() -> u32 {
    60
}
fn default_browser_minutes() -> u32 {
    20
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub apps: BTreeSet<App>,
    pub cli: CliLevel,
    /// Path prefixes; `{home}` expands to `/home/<user>`.
    pub fs_read: Vec<String>,
    pub fs_write: Vec<String>,
    /// Hosts sessions may reach (`"*"` = any host the system egress proxy itself allows).
    pub net_hosts: Vec<String>,
    /// MCP server names (`"*"` = all configured).
    pub mcp_servers: Vec<String>,
    pub max_fuel: u64,
    pub max_memory_mb: u32,
    pub vfs_quota_mb: u32,
    pub manage_users: bool,
    /// May change system settings (models, RAG, secret providers) and the system vault.
    pub manage_settings: bool,
    /// May use the microphone (speech-to-text) and the speaker (text-to-speech).
    pub voice: bool,
    /// May run the tool-using agent (every risky tool call still needs the user's approval).
    pub agent: bool,
    /// May keep personal skills (reusable instructions for the assistant).
    pub budget: AgentBudget,
    pub skills: bool,
    /// May edit the skills shared with every user.
    pub manage_skills: bool,
    pub power: bool,
    pub swarm: bool,
    /// What the assistant may do in the person's browser tab.
    #[serde(default = "BrowserAgent::default_read_only")]
    pub browser_agent: BrowserAgent,
    /// Most browser actions and minutes one run may use.
    #[serde(default = "default_browser_actions")]
    pub browser_max_actions: u32,
    #[serde(default = "default_browser_minutes")]
    pub browser_max_minutes: u32,
}

impl BrowserAgent {
    fn default_read_only() -> BrowserAgent {
        BrowserAgent::ReadOnly
    }
}

/// How much one agent run may consume. `0` means "no limit" for that dimension.
/// Tokens are estimated from text length (about four characters per token) because not every
/// provider reports usage; cost is that estimate times the admin-set price per million tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentBudget {
    pub tokens: u64,
    pub seconds: u64,
    /// Files the agent may create or overwrite.
    pub files: u32,
    /// US cents (needs a price to be set in Settings, otherwise unmetered).
    pub cost_cents: u32,
}

impl AgentBudget {
    pub const TINY: AgentBudget = AgentBudget { tokens: 20_000, seconds: 120, files: 3, cost_cents: 10 };
    pub const SMALL: AgentBudget = AgentBudget { tokens: 60_000, seconds: 300, files: 10, cost_cents: 50 };
    pub const STANDARD: AgentBudget = AgentBudget { tokens: 200_000, seconds: 600, files: 25, cost_cents: 200 };
    pub const LARGE: AgentBudget = AgentBudget { tokens: 1_000_000, seconds: 1800, files: 200, cost_cents: 1000 };
    pub const UNLIMITED: AgentBudget = AgentBudget { tokens: 0, seconds: 0, files: 0, cost_cents: 0 };
    pub const PRESETS: [(&'static str, AgentBudget); 5] = [("tiny", Self::TINY), ("small", Self::SMALL), ("standard", Self::STANDARD), ("large", Self::LARGE), ("unlimited", Self::UNLIMITED)];

    pub fn preset_name(&self) -> &'static str {
        Self::PRESETS.iter().find(|(_, b)| b == self).map_or("custom", |(n, _)| n)
    }

    /// The next preset after this one (wraps; a custom value starts from "tiny").
    pub fn next_preset(&self) -> AgentBudget {
        match Self::PRESETS.iter().position(|(_, b)| b == self) {
            Some(i) => Self::PRESETS[(i + 1) % Self::PRESETS.len()].1,
            None => Self::TINY,
        }
    }

    pub fn describe(&self) -> String {
        let f = |v: u64, unit: &str| if v == 0 { "∞".to_string() } else { format!("{v}{unit}") };
        format!("{} tokens · {} · {} files · {}", f(self.tokens, ""), f(self.seconds, " s"), if self.files == 0 { "∞".into() } else { self.files.to_string() }, if self.cost_cents == 0 { "no cost cap".to_string() } else { format!("${}.{:02}", self.cost_cents / 100, self.cost_cents % 100) })
    }
}

/// Admin-set refinements for one user, applied on top of the role template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyOverride {
    pub deny_apps: Vec<App>,
    pub allow_apps: Vec<App>,
    pub net_hosts: Option<Vec<String>>,
    pub mcp_servers: Option<Vec<String>>,
    pub fs_read: Option<Vec<String>>,
    pub fs_write: Option<Vec<String>>,
    pub max_fuel: Option<u64>,
    pub max_memory_mb: Option<u32>,
    pub vfs_quota_mb: Option<u32>,
    pub voice: Option<bool>,
    pub agent: Option<bool>,
    pub skills: Option<bool>,
    pub budget: Option<AgentBudget>,
    pub browser_agent: Option<BrowserAgent>,
    pub browser_max_actions: Option<u32>,
    pub browser_max_minutes: Option<u32>,
}

fn set(a: &[App]) -> BTreeSet<App> {
    a.iter().copied().collect()
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

impl Role {
    pub fn template(self) -> Policy {
        match self {
            Role::Admin => Policy {
                apps: set(&App::ALL),
                cli: CliLevel::Full,
                fs_read: strs(&["/"]),
                fs_write: strs(&["/"]),
                net_hosts: strs(&["*"]),
                mcp_servers: strs(&["*"]),
                max_fuel: 10_000_000_000,
                max_memory_mb: 1024,
                vfs_quota_mb: 1024,
                manage_users: true,
                manage_settings: true,
                voice: true,
                agent: true,
                budget: AgentBudget::LARGE,
                skills: true,
                manage_skills: true,
                power: true,
                swarm: true,
                browser_agent: BrowserAgent::ReadOnly,
                browser_max_actions: 60,
                browser_max_minutes: 20,
            },
            Role::Operator => Policy {
                apps: set(&[App::Chat, App::Rag, App::Mcp, App::Cli, App::Settings, App::Runs, App::Edit, App::Browser, App::Mesh, App::Radio, App::Scan, App::Skills, App::Usage, App::Calendar, App::System, App::Media, App::SrePilot, App::Inbox, App::Ide, App::Data, App::Tales, App::Store, App::Dash, App::Flows, App::Camera]),
                cli: CliLevel::Full,
                fs_read: strs(&["{home}", "/shared"]),
                fs_write: strs(&["{home}", "/shared"]),
                net_hosts: strs(&["*"]),
                mcp_servers: strs(&["*"]),
                max_fuel: 1_000_000_000,
                max_memory_mb: 256,
                vfs_quota_mb: 256,
                manage_users: false,
                manage_settings: false,
                voice: true,
                agent: true,
                budget: AgentBudget::LARGE,
                skills: true,
                manage_skills: true,
                power: true,
                swarm: true,
                browser_agent: BrowserAgent::ReadOnly,
                browser_max_actions: 60,
                browser_max_minutes: 20,
            },
            Role::User => Policy {
                apps: set(&[App::Chat, App::Rag, App::Mcp, App::Cli, App::Settings, App::Runs, App::Edit, App::Browser, App::Radio, App::Skills, App::Usage, App::Calendar, App::System, App::Media, App::SrePilot, App::Inbox, App::Ide, App::Data, App::Tales, App::Store, App::Dash, App::Flows, App::Camera]),
                cli: CliLevel::Restricted,
                fs_read: strs(&["{home}", "/shared"]),
                fs_write: strs(&["{home}"]),
                net_hosts: strs(&["*"]),
                mcp_servers: vec![],
                max_fuel: 200_000_000,
                max_memory_mb: 64,
                vfs_quota_mb: 64,
                manage_users: false,
                manage_settings: false,
                voice: true,
                agent: true,
                budget: AgentBudget::STANDARD,
                skills: true,
                manage_skills: false,
                power: false,
                swarm: false,
                browser_agent: BrowserAgent::ReadOnly,
                browser_max_actions: 60,
                browser_max_minutes: 20,
            },
            Role::Guest => Policy {
                apps: set(&[App::Chat, App::SrePilot, App::Inbox]),
                cli: CliLevel::Off,
                fs_read: vec![],
                fs_write: vec![],
                net_hosts: vec![],
                mcp_servers: vec![],
                max_fuel: 20_000_000,
                max_memory_mb: 16,
                vfs_quota_mb: 4,
                manage_users: false,
                manage_settings: false,
                voice: false,
                agent: false,
                budget: AgentBudget::TINY,
                skills: false,
                manage_skills: false,
                power: false,
                swarm: false,
                browser_agent: BrowserAgent::Off,
                browser_max_actions: 60,
                browser_max_minutes: 20,
            },
        }
    }
}

impl Policy {
    /// Role template + overrides, with the invariants that overrides cannot grant user
    /// management or power control to a non-admin, and the Users app follows `manage_users`.
    pub fn resolve(role: Role, ov: &PolicyOverride) -> Policy {
        let mut p = role.template();
        for a in &ov.allow_apps {
            p.apps.insert(*a);
        }
        for a in &ov.deny_apps {
            p.apps.remove(a);
        }
        if let Some(v) = &ov.net_hosts {
            p.net_hosts = v.clone();
        }
        if let Some(v) = &ov.mcp_servers {
            p.mcp_servers = v.clone();
        }
        if let Some(v) = &ov.fs_read {
            p.fs_read = v.clone();
        }
        if let Some(v) = &ov.fs_write {
            p.fs_write = v.clone();
        }
        if let Some(v) = ov.max_fuel {
            p.max_fuel = v.max(1);
        }
        if let Some(v) = ov.max_memory_mb {
            p.max_memory_mb = v.max(1);
        }
        if let Some(v) = ov.vfs_quota_mb {
            p.vfs_quota_mb = v;
        }
        if let Some(v) = ov.voice {
            p.voice = v;
        }
        if let Some(v) = ov.agent {
            p.agent = v;
        }
        if let Some(v) = ov.budget {
            p.budget = v;
        }
        if let Some(v) = ov.skills {
            p.skills = v;
        }
        if let Some(v) = ov.browser_agent {
            p.browser_agent = v;
        }
        if let Some(v) = ov.browser_max_actions {
            p.browser_max_actions = v.clamp(1, 500);
        }
        if let Some(v) = ov.browser_max_minutes {
            p.browser_max_minutes = v.clamp(1, 240);
        }
        // no browser app, no assistant in it
        if !p.apps.contains(&App::Browser) {
            p.browser_agent = BrowserAgent::Off;
        }
        if !p.skills {
            p.manage_skills = false;
        }
        if role != Role::Admin {
            p.manage_users = false;
            p.manage_settings = false;
            p.apps.remove(&App::Users);
        } else {
            // admins always keep the management apps unless an override explicitly denies them
            for admin_app in [App::Users, App::Settings] {
                if !ov.deny_apps.contains(&admin_app) {
                    p.apps.insert(admin_app);
                }
            }
        }
        // A terminal app that is switched off cannot run commands.
        if !p.apps.contains(&App::Cli) {
            p.cli = CliLevel::Off;
        } else if p.cli == CliLevel::Off {
            p.cli = CliLevel::Restricted;
        }
        p
    }

    pub fn allows_app(&self, app: App) -> bool {
        self.apps.contains(&app)
    }

    fn expand(list: &[String], user: &str) -> Vec<String> {
        list.iter().map(|s| s.replace("{home}", &format!("/home/{user}"))).collect()
    }

    pub fn read_scopes(&self, user: &str) -> Vec<String> {
        Self::expand(&self.fs_read, user)
    }

    pub fn write_scopes(&self, user: &str) -> Vec<String> {
        Self::expand(&self.fs_write, user)
    }

    fn within(scopes: &[String], path: &str) -> bool {
        // `crate::Capability` carries the exact prefix semantics; reuse it.
        scopes.iter().any(|s| Capability::FsRead(s.clone()).permits(&crate::Access::FsRead(path)))
    }

    /// `path` must already be normalized (no `..`).
    pub fn allows_read(&self, user: &str, path: &str) -> bool {
        Self::within(&self.read_scopes(user), path)
    }

    pub fn allows_write(&self, user: &str, path: &str) -> bool {
        Self::within(&self.write_scopes(user), path)
    }

    pub fn allows_host(&self, host: &str) -> bool {
        self.net_hosts.iter().any(|h| h == "*" || h.eq_ignore_ascii_case(host))
    }

    pub fn allows_mcp(&self, server: &str) -> bool {
        self.mcp_servers.iter().any(|s| s == "*" || s == server)
    }

    /// Capability tokens for a sandboxed task run on this user's behalf. `known_hosts` is the
    /// concrete set the system egress proxy serves; a `"*"` entry expands to all of them.
    pub fn grant_tokens(&self, store: &CapabilityStore, user: &str, known_hosts: &[String]) -> Result<Vec<SyscallToken>, SecError> {
        let mut t = Vec::new();
        for p in self.read_scopes(user) {
            t.push(store.issue(Capability::FsRead(p))?);
        }
        for p in self.write_scopes(user) {
            t.push(store.issue(Capability::FsWrite(p))?);
        }
        for h in known_hosts.iter().filter(|h| self.allows_host(h)) {
            t.push(store.issue(Capability::NetConnect(h.to_ascii_lowercase()))?);
        }
        Ok(t)
    }
}

/// Valid account names: lowercase letter first, then letters, digits, `_` or `-`; at most 32.
/// Restricting this keeps `/home/<name>` free of traversal and odd characters.
pub fn valid_username(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase()) && n.len() <= 32 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_control_defaults_are_read_only_and_off_for_guests_and_overrides_apply() {
        for r in [Role::Admin, Role::Operator, Role::User] {
            let p = r.template();
            assert_eq!((p.browser_agent, p.browser_max_actions, p.browser_max_minutes), (BrowserAgent::ReadOnly, 60, 20), "{r:?}");
        }
        assert_eq!(Role::Guest.template().browser_agent, BrowserAgent::Off);
        let ov = PolicyOverride { browser_agent: Some(BrowserAgent::Interactive), browser_max_actions: Some(5000), browser_max_minutes: Some(0), ..Default::default() };
        let p = Policy::resolve(Role::User, &ov);
        assert_eq!((p.browser_agent, p.browser_max_actions, p.browser_max_minutes), (BrowserAgent::Interactive, 500, 1), "limits are clamped");
        // no BROWSER app, no browser control
        let ov = PolicyOverride { browser_agent: Some(BrowserAgent::Interactive), deny_apps: vec![App::Browser], ..Default::default() };
        assert_eq!(Policy::resolve(Role::User, &ov).browser_agent, BrowserAgent::Off);
        // the settings cycle visits every level
        assert_eq!(BrowserAgent::Off.next().next().next(), BrowserAgent::Off);
        assert_eq!(BrowserAgent::ALL.map(|b| b.name()), ["off", "read_only", "interactive"]);
    }

    use crate::Access;

    #[test]
    fn role_templates_are_ordered_by_power() {
        let (a, o, u, g) = (Role::Admin.template(), Role::Operator.template(), Role::User.template(), Role::Guest.template());
        assert!(a.max_fuel > o.max_fuel && o.max_fuel > u.max_fuel && u.max_fuel > g.max_fuel);
        assert!(a.manage_users && !o.manage_users && !u.manage_users && !g.manage_users);
        assert!(a.manage_settings && !o.manage_settings && !u.manage_settings && !g.manage_settings);
        assert_eq!(g.apps, [App::Chat, App::SrePilot, App::Inbox].into());
        assert_eq!(g.cli, CliLevel::Off);
        assert!(a.apps.contains(&App::Users) && !o.apps.contains(&App::Users));
        assert!(o.apps.contains(&App::Settings) && u.apps.contains(&App::Settings) && !g.apps.contains(&App::Settings), "settings app = personal vault + password for everyone but guests");
    }

    #[test]
    fn overrides_refine_but_cannot_escalate() {
        let ov = PolicyOverride {
            deny_apps: vec![App::Mcp],
            allow_apps: vec![App::Users, App::Settings], // must be ignored for non-admins
            net_hosts: Some(vec!["api.example.com".into()]),
            max_memory_mb: Some(8),
            ..Default::default()
        };
        let p = Policy::resolve(Role::User, &ov);
        assert!(!p.allows_app(App::Mcp) && !p.allows_app(App::Users) && !p.manage_users && !p.manage_settings, "allowing Settings to a user opens only their personal tabs");
        assert!(p.allows_host("API.example.com") && !p.allows_host("evil.com"));
        assert_eq!(p.max_memory_mb, 8);
        // denying the terminal turns the CLI off; re-enabling gives at least Restricted
        let p = Policy::resolve(Role::Operator, &PolicyOverride { deny_apps: vec![App::Cli], ..Default::default() });
        assert_eq!(p.cli, CliLevel::Off);
        let p = Policy::resolve(Role::Guest, &PolicyOverride { allow_apps: vec![App::Cli], ..Default::default() });
        assert_eq!(p.cli, CliLevel::Restricted);
        // admin keeps Users unless explicitly denied
        assert!(Policy::resolve(Role::Admin, &Default::default()).allows_app(App::Users));
    }

    #[test]
    fn voice_is_per_role_and_overridable() {
        assert!(Role::Admin.template().voice && Role::Operator.template().voice && Role::User.template().voice);
        assert!(!Role::Guest.template().voice, "guests get no microphone by default");
        let off = Policy::resolve(Role::User, &PolicyOverride { voice: Some(false), ..Default::default() });
        assert!(!off.voice);
        let on = Policy::resolve(Role::Guest, &PolicyOverride { voice: Some(true), ..Default::default() });
        assert!(on.voice, "an admin can grant a guest voice");
    }

    #[test]
    fn budgets_have_role_defaults_presets_and_overrides() {
        assert_eq!(Role::User.template().budget, AgentBudget::STANDARD);
        assert!(Role::Guest.template().budget.tokens < Role::User.template().budget.tokens);
        let big = Policy::resolve(Role::User, &PolicyOverride { budget: Some(AgentBudget::LARGE), ..Default::default() });
        assert_eq!(big.budget.preset_name(), "large");
        // cycling visits every preset and wraps; a hand-edited value is reported as custom
        let mut b = AgentBudget::TINY;
        let names: Vec<_> = (0..5).map(|_| { let n = b.preset_name(); b = b.next_preset(); n }).collect();
        assert_eq!(names, ["tiny", "small", "standard", "large", "unlimited"]);
        assert_eq!(b, AgentBudget::TINY);
        assert_eq!(AgentBudget { tokens: 7, ..AgentBudget::TINY }.preset_name(), "custom");
        assert!(AgentBudget::STANDARD.describe().contains("200000 tokens") && AgentBudget::UNLIMITED.describe().contains('∞'));
    }

    #[test]
    fn skills_permissions() {
        assert!(Role::User.template().skills && !Role::User.template().manage_skills);
        assert!(Role::Operator.template().manage_skills && Role::Admin.template().manage_skills);
        assert!(!Role::Guest.template().skills);
        let off = Policy::resolve(Role::Operator, &PolicyOverride { skills: Some(false), ..Default::default() });
        assert!(!off.skills && !off.manage_skills, "no skills means no shared editing either");
        let on = Policy::resolve(Role::Guest, &PolicyOverride { skills: Some(true), ..Default::default() });
        assert!(on.skills && !on.manage_skills);
    }

    #[test]
    fn agent_permission_and_runs_app() {
        assert!(Role::Admin.template().agent && Role::Operator.template().agent && Role::User.template().agent);
        assert!(!Role::Guest.template().agent, "guests cannot run tool-using agents");
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert!(r.template().apps.contains(&App::Runs), "{r:?} can review their runs");
        }
        assert!(!Role::Guest.template().apps.contains(&App::Runs));
        let off = Policy::resolve(Role::User, &PolicyOverride { agent: Some(false), ..Default::default() });
        assert!(!off.agent);
        assert!(Policy::resolve(Role::Guest, &PolicyOverride { agent: Some(true), ..Default::default() }).agent);
    }

    #[test]
    fn file_scopes_are_per_user() {
        let p = Role::User.template();
        assert!(p.allows_read("alice", "/home/alice/notes.txt"));
        assert!(p.allows_write("alice", "/home/alice/notes.txt"));
        assert!(!p.allows_read("alice", "/home/bob/notes.txt"));
        assert!(!p.allows_read("alice", "/home/alicex/x")); // prefix boundary
        assert!(p.allows_read("alice", "/shared/x") && !p.allows_write("alice", "/shared/x"));
        assert!(!Role::Guest.template().allows_read("g", "/home/g/x"));
        assert!(Role::Admin.template().allows_write("root", "/etc/anything"));
    }

    #[test]
    fn tokens_follow_policy() {
        let store = CapabilityStore::new();
        let hosts = vec!["a.com".to_string(), "b.com".to_string()];
        let mut p = Role::User.template();
        p.net_hosts = vec!["a.com".into()];
        let toks = p.grant_tokens(&store, "alice", &hosts).unwrap();
        // 2 read scopes + 1 write scope + 1 host
        assert_eq!(toks.len(), 4);
        let ok = |t: &SyscallToken, a: Access| store.check(t, &a).is_ok();
        assert!(ok(&toks[0], Access::FsRead("/home/alice/x")));
        assert!(ok(&toks[2], Access::FsWrite("/home/alice/x")) && !ok(&toks[2], Access::FsWrite("/shared/x")));
        assert!(ok(&toks[3], Access::NetConnect("a.com")) && !ok(&toks[3], Access::NetConnect("b.com")));
        // wildcard expands to every known host
        let all = Role::Operator.template().grant_tokens(&store, "o", &hosts).unwrap();
        assert_eq!(all.len(), 2 + 2 + 2);
        assert!(Role::Guest.template().grant_tokens(&store, "g", &hosts).unwrap().is_empty());
    }

    #[test]
    fn usernames() {
        for ok in ["alice", "a", "bob_2", "x-y"] {
            assert!(valid_username(ok), "{ok}");
        }
        for bad in ["", "Alice", "1abc", "a b", "../x", "a/b", &"a".repeat(33), "ünï"] {
            assert!(!valid_username(bad), "{bad}");
        }
    }
}
