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
    Route { id: "approvals_list", method: "GET", path: "/api/v1/approvals", access: User, needs: Need::Chat, profiles: AGENT, audit: false, summary: "Tool calls waiting for the caller's answer.", request: "", response: "ApprovalList" },
    Route { id: "approvals_decide", method: "POST", path: "/api/v1/approvals/{run}/{call}", access: User, needs: Need::Chat, profiles: AGENT, audit: true, summary: "Approve or deny one waiting tool call. The verdict comes only from here, never from model text.", request: "Verdict", response: "ApprovalDecided" },
    Route { id: "admin_status", method: "GET", path: "/api/v1/admin/status", access: Admin, needs: Need::Nothing, profiles: AGENT, audit: false, summary: "Signed-in tokens, runs going, pending approvals (counts).", request: "", response: "AdminStatus" },
    Route { id: "flows_list", method: "GET", path: "/api/v1/flows", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "The caller's flows (names and counts).", request: "", response: "FlowList" },
    Route { id: "flows_get", method: "GET", path: "/api/v1/flows/{id}", access: User, needs: Need::Flows, profiles: CLOUD, audit: false, summary: "One flow exported as JSON.", request: "", response: "Flow" },
    Route { id: "data_call", method: "POST", path: "/api/v1/data/call", access: User, needs: Need::Data, profiles: CLOUD, audit: true, summary: "Run one read-only data tool (data_open, data_describe, data_query) inside the caller's data jail.", request: "DataCall", response: "DataResult" },
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
                if g.len() > 64 || !g.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
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
