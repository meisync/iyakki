//! The registry of item kinds and their schemas, and validation. Validation messages never repeat a value that was sent (a secret
//! field is never echoed, and nothing else needs to be).

use super::{Item, DEFAULT_NAME};
use crate::cloudroles::TenantRole;
use serde_json::Value;

/// How a string is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Free,
    Plain,
    Host,
    HostPattern,
    Registry,
    HttpsUrl,
    BaseUrl,
    Model,
    Label,
    Device,
    /// A host, `*.domain`, an IPv4 address or an IPv4 CIDR range.
    Range,
}

#[derive(Clone, Copy, Debug)]
pub enum Ty {
    Bool,
    Int(i64, i64),
    Str(Check, usize),
    /// Listed in order, weakest first for the merge that picks the stricter.
    Enum(&'static [&'static str]),
    /// Check, most items, longest item.
    List(Check, usize, usize),
    /// `vault:<ns>/<name>`, the namespace being the one given.
    SecretRef(&'static str),
}

/// How a lower layer combines with a higher one. A lower layer can only narrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// The highest layer that sets it wins.
    First,
    /// Both must say yes.
    And,
    /// Either saying yes is enough (restrictions: frozen, four eyes).
    Or,
    Min,
    /// The later in the enum's list wins (the stricter).
    MaxOrdered,
    Intersect,
    /// Keep the lower items a higher item covers (`*`, `*.example.com`, equal).
    IntersectCover,
    /// Keep the lower items that start with a higher item.
    IntersectPrefix,
    Union,
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    /// Required when another field has this (string) value.
    pub required_if: Option<(&'static str, &'static str)>,
    pub merge: Merge,
    /// A JSON literal used when no layer sets the field.
    pub default: Option<&'static str>,
    pub help: &'static str,
}

const fn f(name: &'static str, ty: Ty, merge: Merge, help: &'static str) -> Field {
    Field { name, ty, required: false, required_if: None, merge, default: None, help }
}
const fn req(mut x: Field) -> Field {
    x.required = true;
    x
}
const fn req_if(mut x: Field, other: &'static str, v: &'static str) -> Field {
    x.required_if = Some((other, v));
    x
}
const fn dflt(mut x: Field, d: &'static str) -> Field {
    x.default = Some(d);
    x
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// One value (name `default`).
    Single,
    /// Several named entries.
    Collection,
}

#[derive(Debug)]
pub struct KindSpec {
    pub id: &'static str,
    pub summary: &'static str,
    pub card: Card,
    pub fields: &'static [Field],
    /// A team (namespace) may have its own entries, which can only narrow the tenant's.
    pub team_level: bool,
    /// A person may narrow it further (resolver supports it; no API stores person layers in the first release).
    pub person_override: bool,
    /// The least tenant role that sees the values (everyone in the tenant sees the state).
    pub read_values: TenantRole,
    /// The least tenant role that writes (a team's own admin also writes its team entries when `team_level`).
    pub write: TenantRole,
    /// Where this kind's secrets live in the vault.
    pub secret_ns: &'static str,
}

pub const ROLES: &[&str] = &["viewer", "editor", "admin", "tenant_admin"];
const MODE: &[&str] = &["shared", "per_person"];

const fn agent_fields(ns: &'static str) -> [Field; 7] {
    [
        dflt(f("mode", Ty::Enum(MODE), Merge::MaxOrdered, "per_person: everybody signs in with their own account (the default). shared: one login of the tenant for all, which a tenant admin opts into"), "\"per_person\""),
        req_if(f("token", Ty::SecretRef(ns), Merge::First, "vault reference of the tenant's shared login or API key"), "mode", "shared"),
        req_if(f("terms_ack", Ty::Bool, Merge::And, "the admin confirms the provider's terms allow this shared use"), "mode", "shared"),
        f("allowed_models", Ty::List(Check::Model, 50, 80), Merge::IntersectCover, "models people may use; empty means none, * means any"),
        req_if(f("daily_requests_per_person", Ty::Int(1, 1_000_000), Merge::Min, "per person per day, shared mode"), "mode", "shared"),
        f("daily_requests_tenant", Ty::Int(1, 10_000_000), Merge::Min, "all people together per day, shared mode"),
        f("monthly_requests_team", Ty::Int(1, 100_000_000), Merge::Min, "the team's cap per 30 days"),
    ]
}
static CLAUDE: [Field; 7] = agent_fields("agent.claude");
static COPILOT: [Field; 7] = agent_fields("agent.copilot");
static OTHER_AGENT: [Field; 7] = agent_fields("agent.other");

static PROVIDER: [Field; 7] = [
    dflt(f("api", Ty::Enum(&["openai", "ollama", "anthropic", "other"]), Merge::First, "wire protocol"), "\"openai\""),
    req(f("base_url", Ty::Str(Check::BaseUrl, 300), Merge::First, "https, or http to localhost or a cluster-local name")),
    f("api_key", Ty::SecretRef("models.provider"), Merge::First, "vault reference of the key"),
    f("allowed_models", Ty::List(Check::Model, 100, 80), Merge::IntersectCover, "empty means none, * means any"),
    f("daily_tokens_per_person", Ty::Int(1, 1_000_000_000), Merge::Min, "per person per day"),
    f("terms_ack", Ty::Bool, Merge::And, "the team admin confirms billing goes to the key's owner and use is audited per person"),
    f("monthly_requests_team", Ty::Int(1, 100_000_000), Merge::Min, "the team's cap per 30 days"),
];

static INTEGRATION: [Field; 11] = [
    req(f("provider", Ty::Enum(super::catalog::IDS), Merge::First, "a provider of the catalog")),
    f("base_url", Ty::Str(Check::BaseUrl, 300), Merge::First, "https, or http to localhost or a cluster-local name"),
    req(f("api_key", Ty::SecretRef("integration.apikey"), Merge::First, "vault reference of the team's key (vault:team/<team>/<name>)")),
    req(f("terms_ack", Ty::Bool, Merge::And, "the team admin confirms billing goes to the key's owner and use is audited per person")),
    f("allowed_models", Ty::List(Check::Model, 100, 80), Merge::IntersectCover, "empty means none, * means any"),
    f("daily_requests_per_person", Ty::Int(1, 1_000_000), Merge::Min, "per person per day"),
    f("monthly_requests_team", Ty::Int(1, 100_000_000), Merge::Min, "the team's cap per 30 days"),
    f("deployment", Ty::Str(Check::Plain, 64), Merge::First, "Azure OpenAI deployment name"),
    f("api_version", Ty::Str(Check::Label, 40), Merge::First, "Azure OpenAI API version"),
    f("site_name", Ty::Str(Check::Free, 80), Merge::First, "OpenRouter site name (optional)"),
    f("referer", Ty::Str(Check::HttpsUrl, 300), Merge::First, "OpenRouter referer (optional)"),
];

static INTEGRATION_POLICY: [Field; 16] = [
    dflt(f("allow_private_endpoints", Ty::Bool, Merge::First, "teams may use http or loopback or private-range endpoints (self-hosted)"), "false"),
    dflt(f("team_integrations", Ty::Enum(&["allow", "deny"]), Merge::MaxOrdered, "deny: no team may add an integration with its own key"), "\"allow\""),
    dflt(f("allowed_providers", Ty::List(Check::Model, 50, 80), Merge::IntersectCover, "providers (entry names or the provider setting) teams may add; * any"), "[\"*\"]"),
    dflt(f("allowed_models", Ty::List(Check::Model, 100, 80), Merge::IntersectCover, "models teams may allow; * any"), "[\"*\"]"),
    dflt(f("new_provider_needs_approval", Ty::Bool, Merge::Or, "a team's new provider waits for a tenant admin"), "false"),
    dflt(f("warn_monthly_cap", Ty::Int(1, 1_000_000_000), Merge::First, "a team cap above this many requests per 30 days draws a warning"), "100000"),
    dflt(f("warn_daily_per_person", Ty::Int(1, 1_000_000_000), Merge::First, "a per-person daily cap above this draws a warning"), "2000"),
    dflt(f("key_warn_days", Ty::Int(1, 3650), Merge::First, "warn when a key has not been rotated for this many days"), "90"),
    dflt(f("key_strong_days", Ty::Int(1, 3650), Merge::First, "strong warning after this many days"), "180"),
    dflt(f("max_egress_domains", Ty::Int(1, 100_000), Merge::First, "warn when an egress allow-list is longer"), "50"),
    dflt(f("allow_wildcard_egress", Ty::Bool, Merge::First, "only honoured when the operator's file manages this item"), "false"),
    dflt(f("require_four_eyes_prod", Ty::Bool, Merge::First, "a prod binding without four eyes is refused"), "false"),
    f("internal_hosts", Ty::List(Check::Range, 200, 64), Merge::First, "hosts, *.domain patterns, IPv4 addresses or CIDR ranges teams may point a local model host at; empty means none"),
    f("deny_hosts", Ty::List(Check::Range, 200, 64), Merge::First, "ranges never allowed (other tenants' pods, internal services)"),
    dflt(f("allow_same_host", Ty::Bool, Merge::First, "loopback addresses for development"), "false"),
    dflt(f("audit_enabled", Ty::Bool, Merge::First, "false is refused while any item that handles secrets is on"), "true"),
];

static LOCAL: [Field; 16] = [
    req(f("server", Ty::Enum(super::catalog::LOCAL_IDS), Merge::First, "a server of the catalog; its port and API style are suggestions you can edit")),
    req(f("host", Ty::Str(Check::Host, 253), Merge::First, "address of the machine that runs it (private ranges the tenant lists)")),
    req(f("port", Ty::Int(1, 65535), Merge::First, "TCP port")),
    dflt(f("scheme", Ty::Enum(&["http", "https"]), Merge::First, "http inside the network is allowed with a warning"), "\"http\""),
    req(f("api_style", Ty::Enum(&["openai_compatible", "ollama"]), Merge::First, "ollama native (/api/tags, /api/chat) or OpenAI-compatible (/v1/models, /v1/chat/completions)")),
    f("token", Ty::SecretRef("models.local"), Merge::First, "optional bearer token; many local servers have none"),
    f("allowed_models", Ty::List(Check::Model, 100, 80), Merge::IntersectCover, "empty means none, * means any"),
    f("max_concurrent_per_person", Ty::Int(1, 64), Merge::Min, "requests one person may have running at once"),
    f("max_concurrent_team", Ty::Int(1, 1024), Merge::Min, "requests the team may have running at once"),
    f("queue_timeout_secs", Ty::Int(1, 3600), Merge::Min, "how long a request waits for a free slot"),
    f("max_request_kb", Ty::Int(1, 1_048_576), Merge::Min, "largest request"),
    f("max_request_secs", Ty::Int(1, 86_400), Merge::Min, "longest request"),
    dflt(f("use_role", Ty::Enum(ROLES), Merge::MaxOrdered, "least team role that may use it"), "\"viewer\""),
    f("owner_note", Ty::Str(Check::Free, 200), Merge::First, "who runs the machine"),
    dflt(f("publish_to_cluster", Ty::Bool, Merge::And, "publish as a Service in the team's namespace, as the Mac model host does (hook; not built)"), "false"),
    f("terms_ack", Ty::Bool, Merge::And, "the team admin confirms the licences of the models are the team's responsibility"),
];

static GIT_REMOTE: [Field; 4] = [
    req(f("url_prefixes", Ty::List(Check::HttpsUrl, 50, 300), Merge::IntersectPrefix, "https address prefixes people may use")),
    dflt(f("access", Ty::Enum(&["push", "read_only"]), Merge::MaxOrdered, "read_only or push"), "\"read_only\""),
    req_if(f("credential", Ty::SecretRef("git.remote"), Merge::First, "vault reference of the token or key; needed to push"), "access", "push"),
    f("protected_branches", Ty::List(Check::Label, 50, 100), Merge::Union, "never pushed to directly"),
];

static K3S_CLUSTER: [Field; 14] = [
    f("display_name", Ty::Str(Check::Free, 80), Merge::First, "shown in pages"),
    req(f("api_server", Ty::Str(Check::HttpsUrl, 300), Merge::First, "the cluster's API address")),
    req(f("credential", Ty::SecretRef("k3s.cluster"), Merge::First, "vault reference; its name must start with <team>-<environment>- so a dev credential cannot serve prod")),
    f("allowed_registries", Ty::List(Check::Registry, 50, 253), Merge::IntersectCover, "image registries that may be deployed from; empty means none"),
    f("quota_cpu_m", Ty::Int(100, 1_000_000), Merge::Min, "millicores"),
    f("quota_mem_mib", Ty::Int(64, 10_000_000), Merge::Min, "MiB"),
    f("quota_pods", Ty::Int(1, 100_000), Merge::Min, "pods"),
    f("node_labels", Ty::List(Check::Label, 20, 100), Merge::Union, "key=value constraints on nodes"),
    dflt(f("read_role", Ty::Enum(ROLES), Merge::MaxOrdered, "least team role that sees the binding"), "\"viewer\""),
    f("deploy_role", Ty::Enum(ROLES), Merge::MaxOrdered, "least team role that deploys; default editor for the first environment, admin after"),
    dflt(f("admin_role", Ty::Enum(ROLES), Merge::MaxOrdered, "least role that changes the binding"), "\"tenant_admin\""),
    f("four_eyes", Ty::Bool, Merge::Or, "a deploy needs approval by somebody else; default on for the last environment"),
    dflt(f("approver_role", Ty::Enum(&["admin", "tenant_admin"]), Merge::MaxOrdered, "admin: the team's approvers; tenant_admin: only tenant admins"), "\"admin\""),
    f("approver_mode", Ty::Enum(&["role_or_admin", "role_only"]), Merge::MaxOrdered, "role_only: only holders of the approver role approve (while any holder exists); default role_only for the last environment when the team has holders, else role_or_admin"),
];
// `frozen` is kept in a second array so the first stays readable
static K3S_FROZEN: Field = f("frozen", Ty::Bool, Merge::Or, "change freeze: every deploy is refused");

static K3S_ENVS: [Field; 1] = [dflt(f("order", Ty::List(Check::Plain, 10, 32), Merge::First, "promotion chain, first to last"), "[\"dev\",\"preprod\",\"prod\"]")];

static EGRESS: [Field; 2] = [
    dflt(f("mode", Ty::Enum(&["allow_list", "deny_all"]), Merge::MaxOrdered, "deny_all closes everything"), "\"allow_list\""),
    f("allowed_domains", Ty::List(Check::HostPattern, 500, 253), Merge::IntersectCover, "host names, or *.example.com; empty means none"),
];

static MEDIA: [Field; 2] = [
    dflt(f("media", Ty::Enum(&["allow", "deny"]), Merge::MaxOrdered, "browser microphone, camera, sound, clipboard; deny unless allowed"), "\"deny\""),
    f("deny_devices", Ty::List(Check::Device, 4, 16), Merge::Union, "mic, camera, audio, clipread"),
];


/// `sandbox.policy` (docs/development/CLOUD-SANDBOX-PLAN.md): what a run of an arbitrary command, script or browser session may be.
/// Read into one struct by `crate::sandbox::policy`. Off unless switched on; anything unset is the strict value.
static SANDBOX: [Field; 30] = [
    dflt(f("tiers", Ty::List(Check::Plain, 4, 16), Merge::Intersect, "isolation tiers a run may use: wasm, namespace, job, browser"), "[\"wasm\",\"job\"]"),
    dflt(f("browser", Ty::Bool, Merge::And, "the IYAKKI browser sandbox (a pod per session) is offered; off unless the tenant admin switches it on"), "false"),
    dflt(f("userns_ack", Ty::Bool, Merge::And, "the admin confirms the pods allow user namespaces (seccomp/AppArmor relaxed); needed for the namespace tier"), "false"),
    dflt(f("allowed_registries", Ty::List(Check::Registry, 50, 253), Merge::IntersectCover, "registries a run's image may come from; empty means none"), "[]"),
    dflt(f("require_digest", Ty::Bool, Merge::Or, "an image must be pinned by @sha256 digest"), "false"),
    dflt(f("max_cpu_m", Ty::Int(100, 64_000), Merge::Min, "millicores per run"), "1000"),
    dflt(f("max_mem_mib", Ty::Int(32, 262_144), Merge::Min, "MiB per run"), "512"),
    dflt(f("max_pids", Ty::Int(8, 4096), Merge::Min, "processes per run"), "128"),
    dflt(f("max_seconds", Ty::Int(1, 86_400), Merge::Min, "wall time per run"), "300"),
    dflt(f("max_workspace_mib", Ty::Int(1, 102_400), Merge::Min, "workspace size per run"), "256"),
    dflt(f("network_modes", Ty::List(Check::Plain, 3, 16), Merge::Intersect, "none, proxy (direct internet is never allowed)"), "[\"none\",\"proxy\"]"),
    f("runtime_class", Ty::Str(Check::Label, 63), Merge::First, "runtimeClassName of the run's pod (gvisor, kata)"),
    dflt(f("cli_role", Ty::Enum(ROLES), Merge::MaxOrdered, "least team role that may run commands"), "\"editor\""),
    f("approval_patterns", Ty::List(Check::Free, 50, 80), Merge::Union, "commands that always need approval, e.g. \"kubectl apply\", \"ssh\""),
    dflt(f("per_person_concurrency", Ty::Int(1, 64), Merge::Min, "runs one person may have at once"), "2"),
    dflt(f("per_team_concurrency", Ty::Int(1, 1024), Merge::Min, "runs the team may have at once"), "8"),
    dflt(f("queue_timeout_secs", Ty::Int(1, 3600), Merge::Min, "how long a run waits for a slot"), "120"),
    dflt(f("audit_retention_days", Ty::Int(1, 3650), Merge::First, "how long run audit lines are kept"), "90"),
    dflt(f("k3s_environment", Ty::Str(Check::Plain, 32), Merge::First, "the team's cluster binding (environment) that the run's Job is applied through"), "\"dev\""),
    f("browser_domains", Ty::List(Check::HostPattern, 200, 253), Merge::IntersectCover, "domains a browser session may open; empty means none"),
    dflt(f("browser_profile", Ty::Enum(&["named", "tmpfs"]), Merge::MaxOrdered, "tmpfs: nothing survives the session. named: a persistent profile volume"), "\"tmpfs\""),
    dflt(f("browser_download_mib", Ty::Int(1, 4096), Merge::Min, "download folder quota"), "64"),
    dflt(f("browser_session_secs", Ty::Int(10, 3600), Merge::Min, "session timeout"), "300"),
    f("cli_image", Ty::Str(Check::Free, 300), Merge::First, "the image that holds the command-line tools of run_command (aws, gcloud, kubectl)"),
    f("browser_image", Ty::Str(Check::Free, 300), Merge::First, "the image with the web engine helper"),
    dflt(f("allow_privileged", Ty::Bool, Merge::And, "privileged containers: blocked by a safety rule"), "false"),
    dflt(f("allow_host_network", Ty::Bool, Merge::And, "host network: blocked by a safety rule"), "false"),
    dflt(f("allow_host_path", Ty::Bool, Merge::And, "hostPath mounts: blocked by a safety rule"), "false"),
    dflt(f("allow_root", Ty::Bool, Merge::And, "running as root: blocked by a safety rule"), "false"),
    dflt(f("allow_cap_add", Ty::Bool, Merge::And, "added Linux capabilities: blocked by a safety rule"), "false"),
];

static STORAGE_QUOTA: [Field; 2] = [
    dflt(f("place_mb", Ty::Int(1, 100_000_000), Merge::Min, "most one folder may hold, in MiB: a person's folder in a namespace, a namespace's shared folder, a personal folder"), "10240"),
    dflt(f("tenant_mb", Ty::Int(1, 100_000_000), Merge::Min, "most all folders of the tenant may hold together, in MiB"), "102400"),
];

static FLOWS_QUOTA: [Field; 7] = [
    dflt(f("max_runs_tenant", Ty::Int(1, 1000), Merge::Min, "workflow runs going at once in the whole tenant"), "4"),
    dflt(f("max_runs_person", Ty::Int(1, 1000), Merge::Min, "workflow runs going at once for one person"), "2"),
    dflt(f("max_run_secs", Ty::Int(10, 604_800), Merge::Min, "a run longer than this is stopped"), "3600"),
    dflt(f("max_schedules", Ty::Int(1, 10_000), Merge::Min, "schedules (timed triggers) in the tenant"), "50"),
    dflt(f("history_keep", Ty::Int(5, 10_000), Merge::Min, "finished runs kept per workflow"), "200"),
    dflt(f("history_days", Ty::Int(1, 3650), Merge::Min, "days finished runs are kept"), "30"),
    dflt(f("missed_notify_secs", Ty::Int(60, 604_800), Merge::Min, "a run that started this much later than due draws a notification"), "900"),
];

static SCHEDULE_TZ: [Field; 1] = [dflt(f("timezone", Ty::Str(Check::Free, 64), Merge::First, "what `local` means in a cron trigger: UTC, an offset like +05:30, or a zone name when the image has zone files"), "\"UTC\"")];

static FLOWS_UNATTENDED: [Field; 2] = [
    req(f("secret", Ty::SecretRef("flows.unattended"), Merge::First, "vault:team/<team>/<name>: this secret can be used by scheduled runs while nobody is signed in")),
    req(f("terms_ack", Ty::Bool, Merge::And, "the team admin confirms scheduled runs may use this secret while nobody is signed in")),
];

const SEVERITIES: &[&str] = &["low", "normal", "high", "critical"];

static NOTIFY_CHANNEL: [Field; 15] = [
    req(f("type", Ty::Enum(&["webhook", "email"]), Merge::First, "webhook (a signed https POST) or email (SMTP over TLS)")),
    dflt(f("min_severity", Ty::Enum(SEVERITIES), Merge::MaxOrdered, "the lowest severity this channel carries"), "\"normal\""),
    dflt(f("include_details", Ty::Bool, Merge::And, "send titles and texts out of the tenant; off: only a short summary and a link"), "false"),
    dflt(f("details_ack", Ty::Bool, Merge::And, "the admin confirms that notification texts may leave the tenant (needed for include_details)"), "false"),
    f("max_per_hour", Ty::Int(1, 10_000), Merge::Min, "most messages this channel sends per hour (default 60)"),
    f("link_base", Ty::Str(Check::HttpsUrl, 300), Merge::First, "the address people sign in at; the message links to its Notifications page"),
    req_if(f("url", Ty::Str(Check::HttpsUrl, 300), Merge::First, "webhook address (https only)"), "type", "webhook"),
    req_if(f("signing_secret", Ty::SecretRef("notify.channel"), Merge::First, "vault reference of the key that signs each call (HMAC-SHA256 with a timestamp)"), "type", "webhook"),
    req_if(f("host", Ty::Str(Check::Host, 253), Merge::First, "SMTP server"), "type", "email"),
    dflt(f("port", Ty::Int(1, 65535), Merge::First, "587 with starttls, 465 with implicit TLS; plain SMTP does not exist here"), "587"),
    dflt(f("tls", Ty::Enum(&["starttls", "implicit"]), Merge::First, "how the connection is encrypted"), "\"starttls\""),
    req_if(f("from", Ty::Str(Check::Free, 254), Merge::First, "the From address; fixed here, never taken from a notification"), "type", "email"),
    f("username", Ty::Str(Check::Free, 120), Merge::First, "SMTP login name (optional)"),
    f("password", Ty::SecretRef("notify.channel"), Merge::First, "vault reference of the SMTP password"),
    f("addresses", Ty::List(Check::Free, 200, 260), Merge::Union, "person=address, one entry per person who gets email"),
];

static NOTIFY_ROUTING: [Field; 6] = [
    f("quiet_start", Ty::Str(Check::Free, 5), Merge::First, "HH:MM local; channels hold messages (not critical ones) until quiet_end"),
    f("quiet_end", Ty::Str(Check::Free, 5), Merge::First, "HH:MM local"),
    f("utc_offset_minutes", Ty::Int(-840, 840), Merge::First, "the tenant's time zone as a fixed offset from UTC"),
    f("digest_time", Ty::Str(Check::Free, 5), Merge::First, "HH:MM local; low-severity notifications go out once a day then"),
    f("escalate_after_minutes", Ty::Int(1, 10_080), Merge::Min, "no acknowledgement within this long: the team's admins hear of it, then the tenant's admins"),
    dflt(f("escalate_min_severity", Ty::Enum(SEVERITIES), Merge::MaxOrdered, "only notifications at or above this escalate"), "\"high\""),
];

pub static KINDS: &[KindSpec] = &[
    KindSpec { id: "flows.quota", summary: "Limits for workflows and schedules: runs at once, run time, schedules, history", card: Card::Single, fields: &FLOWS_QUOTA, team_level: true, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "storage.quota", summary: "Size limits for files: per folder and for the whole tenant (the tenant keeps one volume with a folder per namespace and person)", card: Card::Single, fields: &STORAGE_QUOTA, team_level: true, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "schedule.timezone", summary: "The time zone schedules use when a trigger says local", card: Card::Single, fields: &SCHEDULE_TZ, team_level: false, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "flows.unattended", summary: "Team secrets scheduled runs may use while nobody is signed in (a safety warning applies)", card: Card::Collection, fields: &FLOWS_UNATTENDED, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "flows.unattended" },
    KindSpec { id: "agent.claude", summary: "Claude Code for the tenant: per-person logins (default) or one shared login", card: Card::Single, fields: &CLAUDE, team_level: true, person_override: true, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "agent.claude" },
    KindSpec { id: "agent.copilot", summary: "GitHub Copilot for the tenant: per-person logins (default) or one shared seat", card: Card::Single, fields: &COPILOT, team_level: true, person_override: true, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "agent.copilot" },
    KindSpec { id: "agent.other", summary: "Another coding agent, by name", card: Card::Collection, fields: &OTHER_AGENT, team_level: true, person_override: true, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "agent.other" },
    KindSpec { id: "models.provider", summary: "Model providers the tenant offers, by name", card: Card::Collection, fields: &PROVIDER, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "models.provider" },
    KindSpec { id: "integration.apikey", summary: "API-key integrations of a team (git hosting tokens, other providers), by name", card: Card::Collection, fields: &INTEGRATION, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "integration.apikey" },
    KindSpec { id: "models.local", summary: "A local model host (Ollama, Lemonade, LM Studio, vLLM, llama.cpp) shared by a team, with fair-share limits", card: Card::Collection, fields: &LOCAL, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "models.local" },
    KindSpec { id: "integration.policy", summary: "What teams may integrate with their own keys (forbid, allowed providers and models, approval)", card: Card::Single, fields: &INTEGRATION_POLICY, team_level: false, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "git.remote", summary: "Git remotes people may reach, by name", card: Card::Collection, fields: &GIT_REMOTE, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "git.remote" },
    KindSpec { id: "k3s.cluster", summary: "Cluster bindings per team and environment (name = the environment)", card: Card::Collection, fields: &[], team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "k3s.cluster" },
    KindSpec { id: "k3s.environments", summary: "The promotion chain of environments", card: Card::Single, fields: &K3S_ENVS, team_level: false, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "egress.policy", summary: "Which hosts the tenant's machines may reach", card: Card::Single, fields: &EGRESS, team_level: true, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "notify.channel", summary: "A channel notifications leave the tenant by (webhook or email), by name; off unless configured", card: Card::Collection, fields: &NOTIFY_CHANNEL, team_level: true, person_override: false, read_values: TenantRole::Admin, write: TenantRole::Admin, secret_ns: "notify.channel" },
    KindSpec { id: "notify.routing", summary: "Quiet hours, the daily digest and the escalation time of notifications", card: Card::Single, fields: &NOTIFY_ROUTING, team_level: true, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "media.policy", summary: "Browser media (microphone, camera, sound, clipboard) for the tenant", card: Card::Single, fields: &MEDIA, team_level: true, person_override: true, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
    KindSpec { id: "sandbox.policy", summary: "What a run of a command, script or browser session may be: tiers, images, limits, network, approval, concurrency", card: Card::Single, fields: &SANDBOX, team_level: true, person_override: false, read_values: TenantRole::Member, write: TenantRole::Admin, secret_ns: "" },
];

/// Kinds named in the plan and not built yet. A request for one is refused as "planned".
pub const PLANNED: &[&str] = &["git.credentials", "mcp.server", "store.source", "quotas", "retention.audit", "sso"];

pub fn kind(id: &str) -> Option<&'static KindSpec> {
    KINDS.iter().find(|k| k.id == id)
}

/// A kind as it appears in a URL (`k3s-cluster`) or with its dot.
pub fn kind_from_url(s: &str) -> Option<&'static KindSpec> {
    kind(s).or_else(|| kind(&s.replacen('-', ".", 1)))
}

impl KindSpec {
    /// The fields; the cluster's `frozen` is added to its list.
    pub fn all_fields(&self) -> Vec<Field> {
        let mut v = if self.id == "k3s.cluster" { K3S_CLUSTER.to_vec() } else { self.fields.to_vec() };
        if self.id == "k3s.cluster" {
            v.push(K3S_FROZEN);
        }
        v
    }
    pub fn field(&self, name: &str) -> Option<Field> {
        self.all_fields().into_iter().find(|x| x.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

fn p(field: &str, m: &str) -> Problem {
    Problem { field: field.into(), message: m.into() }
}

// -------------------------------------------------------------------------------------------------------------- secret references

/// `vault:<ns>/<name>`: where a secret lives in the tenant's vault. Holds no secret itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecretRef {
    pub ns: String,
    pub name: String,
}

impl SecretRef {
    pub fn parse(s: &str) -> Option<SecretRef> {
        let rest = s.strip_prefix("vault:")?;
        if let Some(r) = rest.strip_prefix("team/") {
            // the team's slice of the tenant vault: vault:team/<team>/<name>
            let (team, name) = r.split_once('/')?;
            let ok = |x: &str, max: usize| !x.is_empty() && x.len() <= max && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            return (ok(team, 64) && ok(name, 96)).then(|| SecretRef { ns: "team".into(), name: format!("{team}/{name}") });
        }
        let (ns, name) = rest.split_once('/')?;
        let ns_ok = !ns.is_empty() && ns.len() <= 40 && ns.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.');
        let name_ok = !name.is_empty() && name.len() <= 96 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        (ns_ok && name_ok).then(|| SecretRef { ns: ns.into(), name: name.into() })
    }
    pub fn text(&self) -> String {
        format!("vault:{}/{}", self.ns, self.name)
    }
}

/// The reference a secret written through the API gets when the config names none.
pub fn default_ref(spec: &KindSpec, item: &Item, field: &str) -> String {
    if let (Some(team), true) = (&item.namespace, spec.id != "k3s.cluster") {
        // a team's key lives in the team's slice of the vault
        let mut parts = vec![spec.id.replace('.', "-")];
        if item.name != DEFAULT_NAME {
            parts.push(item.name.clone());
        }
        parts.push(field.into());
        return format!("vault:team/{team}/{}", parts.join("-").to_ascii_lowercase().replace('_', "-"));
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(ns) = &item.namespace {
        parts.push(ns.clone());
    }
    if item.name != DEFAULT_NAME {
        parts.push(item.name.clone());
    }
    parts.push(field.into());
    let name = parts.join("-").to_ascii_lowercase().replace('_', "-");
    format!("vault:{}/{}", spec.secret_ns, name)
}

/// For a cluster binding: the name its credential must start with.
pub fn k3s_ref_prefix(item: &Item) -> String {
    match &item.namespace {
        Some(t) => format!("{t}-{}-", item.name),
        None => format!("{}-", item.name),
    }
}

// -------------------------------------------------------------------------------------------------------------- checks

fn label_ok(l: &str) -> bool {
    !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !l.starts_with('-') && !l.ends_with('-')
}

pub fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.split('.').all(label_ok)
}

/// `example.com`, `*.example.com` or `*`.
pub fn host_pattern_ok(h: &str) -> bool {
    h == "*" || host_ok(h.strip_prefix("*.").unwrap_or(h))
}

fn authority_ok(a: &str) -> bool {
    if a.contains('@') {
        return false;
    }
    let (host, port) = match a.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (a, None),
    };
    port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit())) && host_ok(host)
}

fn url_ok(u: &str, allow_cluster_http: bool) -> bool {
    if u.len() > 300 || u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if let Some(rest) = u.strip_prefix("https://") {
        let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
        return authority_ok(auth);
    }
    if allow_cluster_http {
        if let Some(rest) = u.strip_prefix("http://") {
            let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = auth.rsplit_once(':').map_or(auth, |x| x.0);
            return authority_ok(auth) && super::catalog::is_private_host(host);
        }
    }
    false
}

fn check_str(c: Check, s: &str) -> bool {
    match c {
        Check::Free => !s.chars().any(|c| c.is_control()),
        Check::Plain => crate::cloudstorage::plain_name(s),
        Check::Host => host_ok(s),
        Check::HostPattern => host_pattern_ok(s),
        Check::Registry => {
            let (h, port) = s.rsplit_once(':').map_or((s, None), |(h, p)| (h, Some(p)));
            let h = h.strip_prefix("*.").unwrap_or(h);
            s == "*" || host_ok(h) && port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()))
        }
        Check::HttpsUrl => url_ok(s, false),
        Check::BaseUrl => url_ok(s, true),
        Check::Model => !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:/*".contains(c)),
        Check::Label => !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-/=*:".contains(c)),
        Check::Device => iyakki_sec::media::Device::parse(s).is_some(),
        Check::Range => match s.split_once('/') {
            Some((ip, len)) => ip.split('.').count() == 4 && ip.split('.').all(|x| x.parse::<u8>().is_ok()) && len.parse::<u8>().is_ok_and(|l| l <= 32),
            None => host_pattern_ok(s),
        },
    }
}

/// Check one value against a field. The message never contains the value.
pub fn check_value(fd: &Field, v: &Value) -> Result<(), String> {
    match fd.ty {
        Ty::Bool => v.is_boolean().then_some(()).ok_or_else(|| "must be true or false".to_string()),
        Ty::Int(lo, hi) => match v.as_i64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(()),
            _ => Err(format!("must be a whole number from {lo} to {hi}")),
        },
        Ty::Str(c, max) => match v.as_str() {
            Some(s) if !s.is_empty() && s.chars().count() <= max && check_str(c, s) => Ok(()),
            _ => Err(format!("must be a valid {c:?} text of at most {max} characters")),
        },
        Ty::Enum(opts) => match v.as_str() {
            Some(s) if opts.contains(&s) => Ok(()),
            _ => Err(format!("must be one of: {}", opts.join(", "))),
        },
        Ty::List(c, max_items, max_len) => {
            let Some(a) = v.as_array() else { return Err("must be a list".into()) };
            if a.len() > max_items {
                return Err(format!("at most {max_items} entries"));
            }
            for x in a {
                match x.as_str() {
                    Some(s) if !s.is_empty() && s.len() <= max_len && check_str(c, s) => {}
                    _ => return Err(format!("an entry is not a valid {c:?} text of at most {max_len} characters")),
                }
            }
            let mut seen: Vec<&str> = a.iter().filter_map(|x| x.as_str()).collect();
            seen.sort_unstable();
            seen.dedup();
            if seen.len() != a.len() {
                return Err("an entry is repeated".into());
            }
            Ok(())
        }
        Ty::SecretRef(ns) => match v.as_str().and_then(SecretRef::parse) {
            Some(r) if r.ns == ns || r.ns == "team" => Ok(()),
            _ => Err(format!("must be a vault reference like vault:{ns}/<name> or vault:team/<team>/<name>; secrets are never kept in the config")),
        },
    }
}

/// The value of a field of an item, or its default.
pub fn value_or_default(fd: &Field, item_values: &std::collections::BTreeMap<String, Value>) -> Option<Value> {
    item_values.get(fd.name).cloned().or_else(|| fd.default.and_then(|d| serde_json::from_str(d).ok()))
}

/// Validate an item on its own and against the other items of the document (credential reuse across clusters).
pub fn validate(item: &Item, others: &[Item]) -> Vec<Problem> {
    let mut out = Vec::new();
    let Some(spec) = kind(&item.kind) else {
        out.push(p("kind", if PLANNED.contains(&item.kind.as_str()) { "this kind is planned and not available yet" } else { "unknown kind" }));
        return out;
    };
    match spec.card {
        Card::Single if item.name != DEFAULT_NAME => out.push(p("name", "this kind has one value; its name is \"default\"")),
        Card::Collection if !crate::cloudstorage::plain_name(&item.name) || item.name == "validate" || item.name == "versions" => out.push(p("name", "a plain name: letters, digits, - and _")),
        _ => {}
    }
    if let Some(ns) = &item.namespace {
        if !spec.team_level {
            out.push(p("namespace", "this kind is for the whole tenant"));
        } else if !crate::cloudstorage::plain_name(ns) {
            out.push(p("namespace", "not a valid team name"));
        }
    }
    if item.kind == "k3s.cluster" && (item.name.chars().any(|c| c.is_ascii_uppercase() || c == '_') || item.name.len() > 32) {
        out.push(p("name", "an environment name is lower case letters, digits and -, at most 32"));
    }
    if serde_json::to_string(&item.values).map_or(usize::MAX, |s| s.len()) > 64 * 1024 {
        out.push(p("values", "too large"));
    }
    let fields = spec.all_fields();
    for k in item.values.keys() {
        if !fields.iter().any(|x| x.name == k) {
            out.push(p(k, "not a setting of this kind"));
        }
    }
    // a tenant-level cluster entry is a template (defaults and rules for the teams' bindings): nothing in it is required
    let template = item.kind == "k3s.cluster" && item.namespace.is_none();
    for fd in &fields {
        match item.values.get(fd.name) {
            Some(v) => {
                if let Err(m) = check_value(fd, v) {
                    out.push(p(fd.name, &m));
                }
            }
            None => {
                let by_cond = fd.required_if.is_some_and(|(o, want)| {
                    fields.iter().find(|x| x.name == o).and_then(|of| value_or_default(of, &item.values)).and_then(|v| v.as_str().map(|s| s == want)).unwrap_or(false)
                });
                if (fd.required || by_cond) && !template {
                    out.push(p(fd.name, "required"));
                }
            }
        }
    }
    // cross-field and cross-item rules
    if item.kind == "k3s.cluster" {
        if let Some(r) = item.values.get("credential").and_then(|v| v.as_str()).and_then(SecretRef::parse) {
            if !r.name.starts_with(&k3s_ref_prefix(item)) {
                out.push(p("credential", "the credential must belong to this team and environment (its name starts with <team>-<environment>-)"));
            }
            if others.iter().any(|o| o.key() != item.key() && o.kind == item.kind && o.values.get("credential").and_then(|v| v.as_str()) == Some(r.text().as_str())) {
                out.push(p("credential", "another binding already uses this credential; each binding has its own"));
            }
        }
    }
    // a team-level ref names the item's own team; a team that sets a key confirms the terms
    for fd in &fields {
        if let Some(r) = item.values.get(fd.name).and_then(|v| v.as_str()).and_then(SecretRef::parse) {
            if matches!(fd.ty, Ty::SecretRef(_)) && r.ns == "team" && r.name.split('/').next() != item.namespace.as_deref() {
                out.push(p(fd.name, "a vault:team/<team>/... reference must name this item's own team"));
            }
        }
    }
    if item.namespace.is_some() && matches!(item.kind.as_str(), "models.provider" | "integration.apikey") && fields.iter().any(|fd| matches!(fd.ty, Ty::SecretRef(_)) && item.values.contains_key(fd.name)) && item.values.get("terms_ack") != Some(&Value::Bool(true)) && !out.iter().any(|x| x.field == "terms_ack") {
        out.push(p("terms_ack", "the team admin must confirm the terms for a team key"));
    }
    if item.kind == "integration.apikey" {
        out.extend(super::catalog::check_entry(&item.values));
    }
    if item.kind == "k3s.environments" {
        if let Some(a) = item.values.get("order").and_then(|v| v.as_array()) {
            if a.is_empty() {
                out.push(p("order", "at least one environment"));
            }
        }
    }
    if item.enabled && item.values.get("mode").and_then(|v| v.as_str()) == Some("shared") && item.values.get("terms_ack") != Some(&Value::Bool(true)) && spec.id.starts_with("agent.") {
        out.push(p("terms_ack", "shared use needs the admin's confirmation that the provider's terms allow it"));
    }
    out
}

/// Names of the secret fields of a kind.
pub fn secret_fields(spec: &KindSpec) -> Vec<&'static str> {
    spec.all_fields().iter().filter(|x| matches!(x.ty, Ty::SecretRef(_))).map(|x| x.name).collect()
}

/// A description of the registry for pages: kinds, fields, types, which are secrets. No values.
pub fn describe() -> Value {
    let kinds: Vec<Value> = KINDS
        .iter()
        .map(|k| {
            let fields: Vec<Value> = k
                .all_fields()
                .iter()
                .map(|x| {
                    let ty = match x.ty {
                        Ty::Bool => "bool".to_string(),
                        Ty::Int(a, b) => format!("int {a}..{b}"),
                        Ty::Str(c, n) => format!("text({c:?}) <= {n}"),
                        Ty::Enum(o) => format!("one of {}", o.join("|")),
                        Ty::List(c, n, l) => format!("list({c:?}) <= {n} x {l}"),
                        Ty::SecretRef(ns) => format!("secret_ref {ns}"),
                    };
                    serde_json::json!({"name": x.name, "type": ty, "required": x.required, "required_if": x.required_if.map(|(a, b)| format!("{a}={b}")), "secret": matches!(x.ty, Ty::SecretRef(_)), "help": x.help})
                })
                .collect();
            serde_json::json!({"kind": k.id, "summary": k.summary, "collection": k.card == Card::Collection, "team_level": k.team_level, "person_override": k.person_override, "read_values": format!("{:?}", k.read_values).to_lowercase(), "write": format!("{:?}", k.write).to_lowercase(), "secret_ns": k.secret_ns, "fields": fields})
        })
        .collect();
    serde_json::json!({"kinds": kinds, "planned": PLANNED, "providers": super::catalog::describe()})
}
