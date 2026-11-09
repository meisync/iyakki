//! The route table: the single source for dispatch, authorisation, auditing and the OpenAPI document.
//! A route that is not here does not exist (404); a route whose profile is not running does not exist either.

use super::Profile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// No sign-in (health, version, login). Nothing else is ever public.
    Public,
    /// Any signed-in person whose role allows the profile's apps.
    User,
    /// Administrators only.
    Admin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Needs the Chat permission and the agent (`policy.agent`).
    Chat,
    /// Needs the Flows permission.
    Flows,
    /// Needs the Data permission.
    Data,
    Nothing,
}

#[derive(Debug)]
pub struct Route {
    /// Stable operation id (also the handler's name and the audit event `api.<id>`).
    pub id: &'static str,
    pub method: &'static str,
    /// Segments in `{braces}` are path parameters.
    pub path: &'static str,
    pub access: Access,
    pub needs: Need,
    pub profiles: &'static [Profile],
    /// Written to the audit log.
    pub audit: bool,
    pub summary: &'static str,
    /// Name of a schema in the OpenAPI document ("" for none).
    pub request: &'static str,
    pub response: &'static str,
}

use Access::{Admin, Public, User};
const AGENT: &[Profile] = &[Profile::AgentApi, Profile::Cloud];
const CLOUD: &[Profile] = &[Profile::Cloud];

pub const ROUTES: &[Route] = &[
    Route { id: "healthz", method: "GET", path: "/healthz", access: Public, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Liveness: the process is up (for a load balancer).", request: "", response: "Health" },
    Route { id: "readyz", method: "GET", path: "/readyz", access: Public, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Readiness: 200 when the data directory is writable and the services started, else 503.", request: "", response: "Health" },
    Route { id: "version", method: "GET", path: "/api/v1/version", access: Public, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Product, API version and profile name.", request: "", response: "Version" },
    Route { id: "login", method: "POST", path: "/api/v1/login", access: Public, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Sign in with a machine account; returns a bearer token.", request: "LoginRequest", response: "LoginResponse" },
    Route { id: "logout", method: "POST", path: "/api/v1/logout", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "End this token.", request: "", response: "Ok" },
    Route { id: "openapi", method: "GET", path: "/api/v1/openapi.json", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "This API, as an OpenAPI 3.1 document (only the routes of the running profile).", request: "", response: "OpenApi" },
    Route { id: "whoami", method: "GET", path: "/api/v1/whoami", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Who the token belongs to and what the profile offers them.", request: "", response: "Whoami" },
    Route { id: "models", method: "GET", path: "/api/v1/models", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "The models the machine's providers offer (names only; keys and addresses are never shown).", request: "", response: "Models" },
    Route { id: "tools", method: "GET", path: "/api/v1/tools", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "The tools a run can use in this profile.", request: "", response: "Tools" },
    Route { id: "runs_list", method: "GET", path: "/api/v1/chat/runs", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "The caller's own run history.", request: "", response: "RunList" },
    Route { id: "runs_start", method: "POST", path: "/api/v1/chat/runs", access: User, needs: Need::Chat, profiles: AGENT, audit: true, summary: "Start a chat run; returns its id at once (poll the events).", request: "RunRequest", response: "RunStarted" },
    Route { id: "runs_get", method: "GET", path: "/api/v1/chat/runs/{id}", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "One run: status, and the events so far.", request: "", response: "Run" },
    Route { id: "runs_events", method: "GET", path: "/api/v1/chat/runs/{id}/events", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "Events after `?after=N` (long-poll up to `?wait=` seconds).", request: "", response: "Events" },
    Route { id: "runs_stop", method: "POST", path: "/api/v1/chat/runs/{id}/stop", access: User, needs: Need::Chat, profiles: AGENT, audit: true, summary: "Stop a run of the caller's.", request: "", response: "Ok" },
    Route { id: "approvals_list", method: "GET", path: "/api/v1/approvals", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Everything waiting for the caller's answer, each with a `kind`: `tool_call` (a run of theirs; only the owner answers) or `tenant` (a configuration approval; only eligible approvers see it). Decisions go through the endpoints named in `decide`.", request: "", response: "ApprovalList" },
    Route { id: "approvals_decide", method: "POST", path: "/api/v1/approvals/{run}/{call}", access: User, needs: Need::Chat, profiles: AGENT, audit: true, summary: "Approve or deny one waiting tool call. The verdict comes only from here, never from model text.", request: "Verdict", response: "ApprovalDecided" },
    Route { id: "admin_status", method: "GET", path: "/api/v1/admin/status", access: Admin, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Signed-in tokens, runs going, pending approvals (counts).", request: "", response: "AdminStatus" },
    Route { id: "flows_list", method: "GET", path: "/api/v1/flows", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "The caller's flows (names and counts).", request: "", response: "FlowList" },
    Route { id: "flows_get", method: "GET", path: "/api/v1/flows/{id}", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "One flow exported as JSON.", request: "", response: "Flow" },
    Route { id: "flows_run", method: "POST", path: "/api/v1/flows/{id}/run", access: User, needs: Need::Flows, profiles: CLOUD, audit: true, summary: "Start a run of one of the caller's own flows (the published version, or the draft with `draft`); returns the run id. A step that needs a person stays waiting until that person approves or denies it themselves.", request: "FlowRun", response: "FlowRunStarted" },
    Route { id: "flows_runs", method: "GET", path: "/api/v1/flows/{id}/runs", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "The runs of one flow, newest first. Query: `page` (from 0), `per` (at most 100).", request: "", response: "FlowRunList" },
    Route { id: "flows_run_get", method: "GET", path: "/api/v1/flows/{id}/runs/{run}", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "One run of a flow: status, steps done, the error or what it waits for, and each step's state.", request: "", response: "FlowRunView" },
    Route { id: "sched_status", method: "GET", path: "/api/v1/schedules/status", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "Counters of the tenant scheduler: leader or standby, runs going, queued and blocked, quota refusals, owner pauses, scheduler lag. No names of people or flows.", request: "", response: "ScheduleStatus" },
    Route { id: "sched_preview", method: "GET", path: "/api/v1/schedules/preview", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "The next fire times (at most 10) of a cron expression in the tenant's time zone; nothing is stored. Query: `expr`, `tz`, `count`, `from_ms`.", request: "", response: "SchedulePreviewed" },
    Route { id: "sched_list", method: "GET", path: "/api/v1/schedules", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "The caller's schedules (timed triggers of their own flows) with team, run identity, next runs and whether the service paused them.", request: "", response: "ScheduleList" },
    Route { id: "sched_save", method: "POST", path: "/api/v1/schedules", access: User, needs: Need::Flows, profiles: CLOUD, audit: true, summary: "Create or change a schedule (a cron, interval or once trigger) of one of the caller's flows. The caller needs the editor role in `team`; the schedule runs as the caller (or the team identity, team admins only) with nobody signed in. Refused when the flow needs a person's own login or an unmarked secret.", request: "ScheduleSave", response: "ScheduleSaved" },
    Route { id: "sched_delete", method: "DELETE", path: "/api/v1/schedules/{id}/{name}", access: User, needs: Need::Flows, profiles: CLOUD, audit: true, summary: "Remove one timed trigger of the caller's flow.", request: "", response: "ScheduleSaved" },
    Route { id: "sched_pause", method: "POST", path: "/api/v1/schedules/{id}/pause", access: User, needs: Need::Flows, profiles: CLOUD, audit: true, summary: "Pause all schedules of one of the caller's flows (runs started by hand are not affected).", request: "", response: "ScheduleSaved" },
    Route { id: "sched_resume", method: "POST", path: "/api/v1/schedules/{id}/resume", access: User, needs: Need::Flows, profiles: CLOUD, audit: true, summary: "Resume them; the trigger's missed-run policy applies. Refused while the owner has lost the access the schedule needs.", request: "", response: "ScheduleSaved" },
    Route { id: "files_list", method: "GET", path: "/api/v1/files", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "List a folder of the caller's files on the tenant's volume. Query: `scope` (person, shared or personal), `namespace`, `path`. The caller's role in the namespace decides; the person is always the signed-in account.", request: "", response: "FileList" },
    Route { id: "files_read", method: "GET", path: "/api/v1/files/content", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "Read one UTF-8 text file (at most 512 KiB). Query: `scope`, `namespace`, `path`.", request: "", response: "FileContent" },
    Route { id: "files_write", method: "POST", path: "/api/v1/files/write", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Write one UTF-8 text file (editors and above in a namespace); parent folders are made.", request: "FileWrite", response: "Ok" },
    Route { id: "files_delete", method: "POST", path: "/api/v1/files/delete", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Delete a file, or a folder with `recursive` (editors and above in a namespace).", request: "FileDelete", response: "Ok" },
    Route { id: "files_move", method: "POST", path: "/api/v1/files/move", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Rename or move a file or folder inside ONE space (scope and namespace are shared by both paths; editors and above in a namespace). Refuses to replace an existing name unless `overwrite` is true (files only, never a folder). Moves no data, so the space used does not change.", request: "FileMove", response: "Ok" },
    Route { id: "files_mkdir", method: "POST", path: "/api/v1/files/mkdir", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Make a folder (and the folders above it) in the caller's files (editors and above in a namespace). A name that exists is a 409.", request: "FileMkdir", response: "Ok" },
    Route { id: "git_clone", method: "POST", path: "/api/v1/git/clone", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Clone a public https repository into a folder of the caller's files (editors and above in a namespace). No credentials; links in the checkout are removed.", request: "GitClone", response: "GitCloned" },
    Route { id: "git_clones", method: "GET", path: "/api/v1/git/clones", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "The checkouts in the caller's folder for a scope. Query: `scope`, `namespace`.", request: "", response: "GitCloneList" },
    Route { id: "git_status", method: "GET", path: "/api/v1/git/status", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "Read-only: the branch, ahead and behind, and the changed files of a clone. Query: `folder`, `scope`, `namespace`. No address with a password is ever shown.", request: "", response: "GitStatus" },
    Route { id: "git_branches", method: "GET", path: "/api/v1/git/branches", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "Read-only: the local branches of a clone and the one checked out. Query: `folder`, `scope`, `namespace`.", request: "", response: "GitBranches" },
    Route { id: "git_log", method: "GET", path: "/api/v1/git/log", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "Read-only: commits of a clone, newest first, paged. Query: `folder`, `ref`, `page` (from 0), `per` (at most 200), `scope`, `namespace`.", request: "", response: "GitLog" },
    Route { id: "git_diff", method: "GET", path: "/api/v1/git/diff", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "Read-only: `path` = one changed file against the last commit (a binary file is a 415), or `commit` = one commit's patch (binary files listed without content). At most 256 KiB, else 413. Query also: `folder`, `scope`, `namespace`.", request: "", response: "GitDiff" },
    Route { id: "files_upload", method: "PUT", path: "/api/v1/files/upload", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Store any bytes as a file (editors and above in a namespace). The body is the file itself (not JSON), `Content-Length` is required and at most 128 MiB, and it is streamed to disk, not held in memory. Query: `scope`, `namespace`, `path`; optional header `X-Content-SHA256` (hex) is checked against what arrived. Answers `{ok, bytes, sha256}`.", request: "", response: "FileUploaded" },
    Route { id: "files_download", method: "GET", path: "/api/v1/files/download", access: User, needs: Need::Data, profiles: CLOUD, audit: false, summary: "The bytes of one regular file, streamed with its exact `Content-Length` (`application/octet-stream`, as an attachment). Query: `scope`, `namespace`, `path`. Folders and links are refused. No range requests.", request: "", response: "FileBytes" },
    Route { id: "tenants_list", method: "GET", path: "/api/v1/tenants", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "The tenants the signed-in person may use (id, name, their role, which one is current) and whether there is more than one to switch to. A machine that is not a pod has one, `this-machine`. Query `scope=all` (service administrators only) lists every tenant of the directory.", request: "", response: "TenantList" },
    Route { id: "tenant_switch", method: "POST", path: "/api/v1/tenant/switch", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Work in another tenant the person belongs to, without signing in again. Checked on the server (a tenant they are not in is 404), limited per person; stops the person's running chats and locks the previous tenant's vault. Not on a machine that is not a pod.", request: "TenantSwitch", response: "TenantSwitched" },
    Route { id: "tenant_view", method: "GET", path: "/api/v1/tenant", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The caller's role in the tenant and the namespaces they may see (a tenant admin also sees the names of those they are not in); the member list for tenant admins.", request: "", response: "TenantView" },
    Route { id: "tenant_bootstrap", method: "POST", path: "/api/v1/tenant/bootstrap", access: Admin, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Make the caller the tenant's first owner. Works once, while the tenant has no lists.", request: "", response: "Ok" },
    Route { id: "tenant_member", method: "POST", path: "/api/v1/tenant/members", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Add a tenant member, change their role or remove them (tenant admins and owners).", request: "TenantMemberChange", response: "Ok" },
    Route { id: "ns_manage", method: "POST", path: "/api/v1/namespaces", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Create or delete a namespace (tenant admins and owners).", request: "NamespaceChange", response: "Ok" },
    Route { id: "ns_members_list", method: "GET", path: "/api/v1/namespaces/{id}/members", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The members of a namespace the caller is in, with their roles (a namespace the caller cannot see answers as one that is not there).", request: "", response: "NsMembers" },
    Route { id: "ns_member", method: "POST", path: "/api/v1/namespaces/{id}/members", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Add a person to a namespace, change their role or remove them (the namespace's administrators; a tenant admin may add themselves).", request: "NsMemberChange", response: "Ok" },
    Route { id: "webaccess_get", method: "GET", path: "/api/v1/webaccess", access: Admin, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Serve this desktop to browsers: the switch, the port, the addresses to open, the certificate fingerprint and what it exposes (administrators). A cloud pod has no desktop: 501 there.", request: "", response: "WebAccess" },
    Route { id: "webaccess_set", method: "POST", path: "/api/v1/webaccess", access: Admin, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Switch serving this desktop to browsers on or off and/or choose its port (1024 to 65535, 0 for the default); always HTTPS (administrators).", request: "WebAccessChange", response: "Ok" },
    Route { id: "data_call", method: "POST", path: "/api/v1/data/call", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Run one read-only data tool (data_open, data_describe, data_query) inside the caller's data jail.", request: "DataCall", response: "DataResult" },
    Route { id: "tcfg_list", method: "GET", path: "/api/v1/tenant/config", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The tenant configuration: every item the caller may see with its effective state (ready, secret_missing, vault_locked, disabled, invalid), its source (file or api) and what the caller may do; tenant admins also get the registry of kinds and fields.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_versions", method: "GET", path: "/api/v1/tenant/config/versions", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The last ten versions of the API-edited configuration (tenant admins).", request: "", response: "TenantConfig" },
    Route { id: "tcfg_export", method: "GET", path: "/api/v1/tenant/config/export", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The configuration as a document without any secret (vault references only); tenant admins.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_rollback", method: "POST", path: "/api/v1/tenant/config/rollback", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Restore a kept version as a new version (tenant admins).", request: "TenantConfigRollback", response: "TenantConfig" },
    Route { id: "tcfg_import", method: "POST", path: "/api/v1/tenant/config/import", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Import an exported document, all or nothing; `dry_run` only checks (tenant admins).", request: "TenantConfigImport", response: "TenantConfig" },
    Route { id: "tcfg_validate", method: "POST", path: "/api/v1/tenant/config/{kind}/validate", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Dry run of a PUT: problems and the state the item would have; nothing is stored. `namespace` and `name` in the body.", request: "TenantConfigPut", response: "TenantConfig" },
    Route { id: "tcfg_test", method: "POST", path: "/api/v1/tenant/config/{kind}/{team}/{name}/test", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Test the connection of an API-key integration or a local model host: one cheap authenticated call to its model-list endpoint, at most once a minute per entry; stores ok or failed, the time and the HTTP status only.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_discover", method: "POST", path: "/api/v1/tenant/config/{kind}/{team}/{name}/models", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Ask an API-key integration or a local model host for the models it offers (Ollama /api/tags, OpenAI-style /v1/models, parsed to names); stores nothing, admins only.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_get", method: "GET", path: "/api/v1/tenant/config/{kind}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "One item of a single-value kind, or every entry of a collection. Secret fields read back as [set], [not set] or [vault locked].", request: "", response: "TenantConfig" },
    Route { id: "tcfg_get_n", method: "GET", path: "/api/v1/tenant/config/{kind}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "One entry by name (a collection) or the team's value (a single-value kind).", request: "", response: "TenantConfig" },
    Route { id: "tcfg_get_t", method: "GET", path: "/api/v1/tenant/config/{kind}/{team}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "One entry of a team, for example k3s.cluster/{team}/{environment}. Another team's entries are not found.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_put", method: "PUT", path: "/api/v1/tenant/config/{kind}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Set a single-value kind: enabled, values and write-only secrets (stored in the tenant vault, the config keeps a reference).", request: "TenantConfigPut", response: "TenantConfig" },
    Route { id: "tcfg_put_n", method: "PUT", path: "/api/v1/tenant/config/{kind}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Set a named entry, or a team's value of a single-value kind.", request: "TenantConfigPut", response: "TenantConfig" },
    Route { id: "tcfg_put_t", method: "PUT", path: "/api/v1/tenant/config/{kind}/{team}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Set a team's entry, for example a cluster binding per environment. An item the operator's file manages answers 409.", request: "TenantConfigPut", response: "TenantConfig" },
    Route { id: "tcfg_delete", method: "DELETE", path: "/api/v1/tenant/config/{kind}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Remove a single-value kind's item (vault entries stay).", request: "", response: "TenantConfig" },
    Route { id: "tcfg_delete_n", method: "DELETE", path: "/api/v1/tenant/config/{kind}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Remove a named entry.", request: "", response: "TenantConfig" },
    Route { id: "tcfg_delete_t", method: "DELETE", path: "/api/v1/tenant/config/{kind}/{team}/{name}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Remove a team's entry.", request: "", response: "TenantConfig" },
    Route { id: "tapprovals_list", method: "GET", path: "/api/v1/tenant/approvals", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The approval requests the caller may decide (an admin or approver-role holder of the requester's team, never the requester; tenant admins when escalated). Nobody else sees them. Requests expire after 24 hours by default.", request: "", response: "TenantConfig" },
    Route { id: "tapprovals_approve", method: "POST", path: "/api/v1/tenant/approvals/{id}/approve", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Approve one request. Refused for the requester, for people outside the route, and after expiry. Audited with who approved whom.", request: "", response: "TenantConfig" },
    Route { id: "tapprovals_deny", method: "POST", path: "/api/v1/tenant/approvals/{id}/deny", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Deny one request (same rules as approve).", request: "", response: "TenantConfig" },
    Route { id: "tteam_approvers_get", method: "GET", path: "/api/v1/namespaces/{id}/approvers", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The approver roles of a team (scope = an environment or a kind): people, groups and the expanded people. Team admins and tenant admins.", request: "", response: "TenantConfig" },
    Route { id: "tteam_approver_grant", method: "PUT", path: "/api/v1/namespaces/{id}/approvers/{scope}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Give the approver role for a scope to people and/or groups (team admins, tenant admins). The role grants approving only.", request: "TeamApprovers", response: "TenantConfig" },
    Route { id: "tteam_approver_revoke", method: "DELETE", path: "/api/v1/namespaces/{id}/approvers/{scope}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Take the approver role for a scope from the people and groups listed in the body.", request: "TeamApprovers", response: "TenantConfig" },
    Route { id: "tteam_groups_get", method: "GET", path: "/api/v1/namespaces/{id}/groups", access: User, needs: Need::Nothing, profiles: CLOUD, audit: false, summary: "The team's groups and the tenant-wide ones it may use (`_tenant` lists the tenant-wide groups; tenant admins).", request: "", response: "TenantConfig" },
    Route { id: "tteam_group_set", method: "PUT", path: "/api/v1/namespaces/{id}/groups/{group}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Set the people of a group (people only, no nesting). Audited with who was added and removed.", request: "TeamGroup", response: "TenantConfig" },
    Route { id: "tteam_group_delete", method: "DELETE", path: "/api/v1/namespaces/{id}/groups/{group}", access: User, needs: Need::Nothing, profiles: CLOUD, audit: true, summary: "Delete a group (it also leaves the approver roles that named it).", request: "", response: "TenantConfig" },
    Route { id: "notif_list", method: "GET", path: "/api/v1/notifications", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "The caller's own notifications, newest first. Query: `unread`, `snoozed`, `limit` (at most 200), `before` (a time cursor from `next_before`). Nobody sees another person's.", request: "", response: "NotifList" },
    Route { id: "notif_count", method: "GET", path: "/api/v1/notifications/count", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "The bell: unread, approvals waiting for the caller, overdue tasks; `red` while an approval waits. Read from the stored inbox.", request: "", response: "NotifCount" },
    Route { id: "notif_read_all", method: "POST", path: "/api/v1/notifications/read-all", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Mark all of the caller's notifications read.", request: "", response: "Ok" },
    Route { id: "notif_read", method: "POST", path: "/api/v1/notifications/{id}/read", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Mark one notification read.", request: "", response: "Ok" },
    Route { id: "notif_unread", method: "POST", path: "/api/v1/notifications/{id}/unread", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Mark one notification unread.", request: "", response: "Ok" },
    Route { id: "notif_snooze", method: "POST", path: "/api/v1/notifications/{id}/snooze", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Hide one notification until later (`minutes` or `until`, at most 30 days); it comes back unread.", request: "NotifSnooze", response: "Ok" },
    Route { id: "notif_ack", method: "POST", path: "/api/v1/notifications/{id}/ack", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Acknowledge one notification: marks it read and stops its escalation.", request: "", response: "Ok" },
    Route { id: "notif_delete", method: "DELETE", path: "/api/v1/notifications/{id}", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Delete one of the caller's notifications.", request: "", response: "Ok" },
    Route { id: "notif_channels", method: "GET", path: "/api/v1/notifications/channels", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "The channels (mail) that write to the caller, with whether the caller switched each off for themselves. A shared webhook is the tenant's choice and is not listed.", request: "", response: "NotifChannels" },
    Route { id: "notif_optout", method: "POST", path: "/api/v1/notifications/channels/optout", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Switch one of the caller's own channels off or on: `channel` (its key) and `out`. The INBOX note is always made.", request: "NotifOptout", response: "Ok" },
    Route { id: "tile_list", method: "GET", path: "/api/v1/tiles", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "The WASM tiles the caller may open in the browser (they run on this machine, in the tile sandbox).", request: "", response: "TileList" },
    Route { id: "tile_events", method: "POST", path: "/api/v1/tiles/{id}/events", access: User, needs: Need::Nothing, profiles: AGENT, audit: true, summary: "Send input events (pointer, key, text, resize, tick) to the caller's own copy of a tile and get its new display list. The first call needs `width` and `height`; `reset` starts again. Body at most 64 KiB, 128 events.", request: "TileEvents", response: "TileFrame" },
    Route { id: "events", method: "GET", path: "/api/v1/events", access: User, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "WebSocket (Upgrade; the upgrade itself is answered before the ordinary sign-in, because a browser cannot send a header, and signs in by its first message). The first message is {\"type\":\"auth\",\"token\":...}; the token is never in the address. Then only the caller's own `notification`, `count` and `approval` events, a ping every 25 seconds. Capped per person and per tenant.", request: "", response: "Ok" },
];

/// Split a path into segments (no empty ones).
fn segs(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

impl Route {
    pub fn in_profile(&self, p: Profile) -> bool {
        self.profiles.contains(&p)
    }

    /// The path parameters, in order, when `path` fits this route's pattern.
    pub fn matches(&self, path: &str) -> Option<Vec<String>> {
        let (pat, got) = (segs(self.path), segs(path));
        if pat.len() != got.len() || (path.len() > 1 && path.ends_with('/')) || path.contains("//") {
            return None;
        }
        let mut params = Vec::new();
        for (p, g) in pat.iter().zip(&got) {
            if p.starts_with('{') {
                if g.len() > 64 || g.starts_with('.') || g.contains("..") || !g.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
                    return None;
                }
                params.push(g.to_string());
            } else if p != g {
                return None;
            }
        }
        Some(params)
    }
}

/// What the table says about a request.
#[derive(Debug)]
pub enum Found<'a> {
    Route(&'a Route, Vec<String>),
    /// The path exists in this profile for other methods.
    WrongMethod,
    /// Not a route of this profile (or not a route at all): 404.
    Nothing,
}

pub fn find<'a>(profile: Profile, method: &str, path: &str) -> Found<'a> {
    let mut path_hit = false;
    for r in ROUTES.iter().filter(|r| r.in_profile(profile)) {
        if let Some(params) = r.matches(path) {
            if r.method == method {
                return Found::Route(r, params);
            }
            path_hit = true;
        }
    }
    if path_hit {
        Found::WrongMethod
    } else {
        Found::Nothing
    }
}

/// OpenAPI's form of the path (`{id}` stays `{id}`) and the names of its parameters.
pub fn param_names(path: &str) -> Vec<&str> {
    segs(path).into_iter().filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}'))).collect()
}
