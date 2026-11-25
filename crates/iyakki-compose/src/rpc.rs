//! The root objects the real core serves over its UI socket (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md section 2).
//!
//! `iyakki-ui-rpc` knows only traits. The composition root is the one place that knows which real object answers each trait, so it builds the
//! [`Host`] the core's `uiserve::served` hands to the transport (`rpc: Some`): the agent's facets (`AgentApiImpl`), the member stores of the real
//! services (through their accessors, so the core's one store answers and nothing is copied), the audit log, and the chat runs once a
//! [`RunStarter`] is given. A UI program therefore reaches exactly the roots listed here; a trait not registered answers "no such object".

use std::sync::Arc;

use iyakki_agent::contract_local::uiserve::RpcFactory;
use iyakki_agent_api::{ChatRunTable, RunStarter, ServicesHandle};
use iyakki_ui_rpc::{Host, Obj, ServicesRoot};

/// The machine handle the UI program's Settings calls run over: the raw handle behind the broker, as the caller `settings` (a built-in app) for
/// the person `iyakki-ui`, so the broker's table and its access log decide every change exactly as they do for the Settings app in the core's own
/// screen (D-SPLIT-03-ENV). `data` holds the broker's approvals and log.
pub fn ui_os(raw: Arc<dyn iyakki_core_api::Os>, data: &std::path::Path) -> Arc<dyn iyakki_core_api::Os> {
    use iyakki_core_api::broker::{Caller, CallerKind};
    let (broker, _log) = iyakki_shared::accessbroker::local(data, None);
    let caller = Caller { app: "settings".into(), kind: CallerKind::Builtin, person: "iyakki-ui".into(), device: "this".into() };
    Arc::new(iyakki_core_api::guarded::GuardedOs::new(raw, broker, caller))
}

/// What the core serves as roots.
#[derive(Clone, Default)]
pub struct RpcRoots {
    audit: Option<Arc<dyn iyakki_agent_api::AuditApi>>,
    accounts: Option<Arc<dyn iyakki_agent_api::AccountsApi>>,
    runs: Option<Arc<dyn RunStarter>>,
    file_scope: Option<Arc<dyn iyakki_core_api::FileScope>>,
    host: Option<Arc<dyn iyakki_core_api::HostApi>>,
    core: Option<Arc<dyn iyakki_core_api::Core>>,
    env: Option<iyakki_ui_rpc::CoreEnv>,
}

impl RpcRoots {
    pub fn new() -> RpcRoots {
        RpcRoots::default()
    }

    /// Serve the audit log of the shell as the `AuditApi` root.
    pub fn with_audit(mut self, a: Arc<dyn iyakki_agent_api::AuditApi>) -> RpcRoots {
        self.audit = Some(a);
        self
    }

    /// Serve the machine's accounts as the `AccountsApi` root, wrapped in [`iyakki_agent_api::AccountsGuarded`]: the actor of a change is read again from
    /// the core's own store by name (the role in a `Session` sent over the socket is ignored) and the password hash is never on the wire.
    pub fn with_accounts(mut self, a: Arc<dyn iyakki_agent_api::AccountsApi>) -> RpcRoots {
        self.accounts = Some(Arc::new(iyakki_agent_api::AccountsGuarded(a)));
        self
    }

    /// The same, for the accounts a shell holds as a [`iyakki_agent_api::UsersRef`].
    pub fn with_users(self, u: iyakki_agent_api::UsersRef) -> RpcRoots {
        self.with_accounts(Arc::new(iyakki_agent_api::UsersAsApi(u)))
    }

    /// The core's own machine handle and accounts, passed in place of the objects and closures a UI program would supply for the fleet, router,
    /// SFTP and sound-preview calls (D-SPLIT-03-ENV). `os` should be the guarded handle of the identity those pages use, so the broker's table
    /// still decides what the UI program may change. Without it those calls answer "not served over the UI socket".
    pub fn with_env(mut self, os: Arc<dyn iyakki_core_api::Os>, users: iyakki_agent_api::UsersRef) -> RpcRoots {
        self.env = Some(iyakki_ui_rpc::CoreEnv::new(os, users));
        self
    }

    /// Serve chat runs through `starter` (the core's own tool chain).
    pub fn with_runs(mut self, s: Arc<dyn RunStarter>) -> RpcRoots {
        self.runs = Some(s);
        self
    }

    /// The machine's file rules, for the served forms that judge a path (`ServicesHandle::remote_kit` over the socket).
    pub fn with_file_scope(mut self, f: Arc<dyn iyakki_core_api::FileScope>) -> RpcRoots {
        self.file_scope = Some(f);
        self
    }

    /// The machine's host, for the terminals (the host a UI named is never used).
    pub fn with_host(mut self, h: Arc<dyn iyakki_core_api::HostApi>) -> RpcRoots {
        self.host = Some(h);
        self
    }

    /// The core's own handle, whose sound facets (mixer, output selector, cues, sound route) the UI program asks over the socket.
    pub fn with_core(mut self, c: Arc<dyn iyakki_core_api::Core>) -> RpcRoots {
        self.core = Some(c);
        self
    }

    /// What the served forms take from the core instead of from the UI. `svc` is the core's own services: the terminal of the core contract is
    /// built over them and over the machine's accounts (the policy of an account is read here, never taken from the UI).
    fn parts(&self, svc: Option<&Arc<iyakki_agent::services::Services>>) -> iyakki_ui_rpc::ServedParts {
        let core_terminal: Option<Arc<dyn iyakki_core_api::Terminal>> = match (svc, &self.accounts) {
            (Some(svc), Some(accounts)) => {
                let t = iyakki_agent::terminalfacet::Terminals::shared(svc.clone());
                t.set_policies(Arc::new(AccountPolicies(accounts.clone())));
                Some(t)
            }
            _ => None,
        };
        let secret_store: Option<iyakki_ui_rpc::SecretStoreOf> = svc.map(|svc| {
            let svc = svc.clone();
            let f: iyakki_ui_rpc::SecretStoreOf = Arc::new(move |user: &str| Arc::new(iyakki_agent::gitkeys::ServicesStore::new(&svc, user)));
            f
        });
        iyakki_ui_rpc::ServedParts { accounts: self.accounts.clone(), file_scope: self.file_scope.clone(), host: self.host.clone(), core: self.core.clone(), core_terminal, secret_store }
    }

    /// The names of the roots a host built over these services would register (what a UI can ask for), sorted.
    pub fn root_names(&self) -> Vec<&'static str> {
        let mut v = ROOTS.to_vec();
        if self.audit.is_some() {
            v.push("AuditApi");
        }
        if self.accounts.is_some() {
            v.push("AccountsApi");
        }
        if self.runs.is_some() {
            v.push("ChatRuns");
        }
        v.sort_unstable();
        v
    }

    /// The audit callback the core passes for the `ServicesHandle` calls that carry one in the trait (the UI never sends its own): a line of the
    /// shell's audit log under the UI program's name; without an audit log given, nothing is written (a core built for tests).
    fn audit_fn(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        match self.audit.clone() {
            Some(a) => Arc::new(move |event: &str, detail: &str| a.record("iyakki-ui", event, detail)),
            None => Arc::new(|_: &str, _: &str| {}),
        }
    }

    /// The host for one set of services (the terminal of the core contract is served only by [`RpcRoots::host_over`]).
    pub fn host(&self, svc: Arc<dyn ServicesHandle>) -> Arc<Host> {
        self.host_with(svc, None)
    }

    /// The host for the real services of this core.
    pub fn host_over(&self, svc: &Arc<iyakki_agent::services::Services>) -> Arc<Host> {
        let h: Arc<dyn ServicesHandle> = svc.clone();
        self.host_with(h, Some(svc))
    }

    fn host_with(&self, svc: Arc<dyn ServicesHandle>, real: Option<&Arc<iyakki_agent::services::Services>>) -> Arc<Host> {
        use iyakki_agent_api as api;
        let agent = Arc::new(iyakki_agent::AgentApiImpl);
        let mut h = Host::new()
            .with_root(Obj::AgentDisplay(agent.clone()))
            .with_root(Obj::AgentFiles(agent.clone()))
            .with_root(Obj::AgentStore(agent.clone()))
            .with_root(Obj::AgentFacetsB2(agent.clone()))
            .with_root(Obj::AgentFacetsB4(agent.clone()))
            .with_root(Obj::AgentFacetsB6(agent.clone()))
            .with_root(Obj::AgentFacetsB7(agent.clone()))
            .with_root(Obj::AgentFacetsB8(agent.clone()))
            .with_root(Obj::AgentFacetsS6B(agent))
            .with_root(Obj::InboxApi(Arc::new(api::InboxVia(svc.clone()))))
            .with_root(Obj::CalendarApi(Arc::new(api::CalendarVia(svc.clone()))))
            .with_root(Obj::StudyApi(Arc::new(api::StudyVia(svc.clone()))))
            .with_root(Obj::SecretsApi(Arc::new(api::SecretsVia(svc.clone()))))
            .with_root(Obj::DevWatchApi(Arc::new(api::DevWatchVia(svc.clone()))))
            .with_root(Obj::BrowserStateApi(Arc::new(api::BrowserStateVia(svc.clone()))))
            .with_root(Obj::SettingsApi(Arc::new(api::SettingsVia(svc.clone()))))
            .with_root(Obj::KnowledgeBaseApi(Arc::new(api::KnowledgeBaseVia(svc.clone()))))
            .with_root(Obj::ConsentApi(Arc::new(api::ConsentVia(svc.clone()))))
            .with_root(Obj::BrowserJobsApi(Arc::new(api::BrowserJobsVia(svc.clone()))))
            .with_root(Obj::ScreenCaptureApi(Arc::new(api::ScreenCaptureVia(svc.clone()))))
            .with_root(Obj::DevToolsApi(Arc::new(api::DevToolsVia(svc.clone()))))
            .with_root(Obj::SecretRequestsApi(Arc::new(api::SecretRequestsVia(svc.clone()))))
            .with_root(Obj::UsageLogApi(Arc::new(api::UsageLogVia(svc.clone()))))
            .with_root(Obj::ExtStatusApi(Arc::new(api::ExtStatusVia(svc.clone()))))
            .with_root(Obj::McpApi(Arc::new(api::McpVia(svc.clone()))))
            .with_root(Obj::ServicesHandle(Arc::new({
                let root = ServicesRoot::new(svc, self.audit_fn()).with_parts(self.parts(real));
                match self.env.clone() {
                    Some(e) => root.with_env(e),
                    None => root,
                }
            })));
        if let Some(real) = real {
            // the platform calls that name the install's services as an `Arc<dyn Any>` are given the core's own (D-SPLIT-03-APPS2)
            let any: Arc<dyn std::any::Any + Send + Sync> = ServicesHandle::as_any_arc(real.clone());
            h = h.with_services(any);
            // the store calls use the core's own fetch and audit, and read the person and the folder rules from the core's accounts (D-SPLIT-03-STORE)
            if let Some(accounts) = &self.accounts {
                let fetch: Arc<dyn iyakki_shared::store_catalog::Fetch> = Arc::new(iyakki_shared::store_catalog::HttpFetch(real.quick_upstream()));
                h = h.with_store_side(fetch, self.audit_fn(), accounts.clone(), self.file_scope.clone(), real.dir().to_path_buf());
                h = h.with_web_fetch(core_web(real.dir()));
                // the acting person is the one who signed in over the connection, and an approval is a core-issued ticket (alpha.8 SEC3)
                h = h.with_login_binding();
            }
        }
        if let Some(a) = &self.audit {
            h = h.with_root(Obj::AuditApi(a.clone()));
        }
        if let Some(a) = &self.accounts {
            h = h.with_root(Obj::AccountsApi(a.clone()));
        }
        if let Some(s) = &self.runs {
            h = h.with_root(Obj::ChatRuns(Arc::new(ChatRunTable::new(s.clone()))));
        }
        Arc::new(h)
    }

    /// What `uiserve::watch` needs: one host per core, built over the core's own services.
    pub fn factory(self) -> RpcFactory {
        Arc::new(move |core| {
            let h: Arc<dyn iyakki_ui_client::server::RpcServe> = self.host_over(&core.svc);
            h
        })
    }
}

/// The roots every host has (the audit log, the accounts and chat runs join when given).
pub const ROOTS: [&str; 26] = [
    "AgentDisplay", "AgentFacetsB2", "AgentFacetsB4", "AgentFacetsB6", "AgentFacetsB7", "AgentFacetsB8", "AgentFacetsS6B", "AgentFiles", "AgentStore", "BrowserJobsApi", "BrowserStateApi", "CalendarApi", "ConsentApi", "DevToolsApi", "DevWatchApi", "ExtStatusApi", "InboxApi", "KnowledgeBaseApi", "McpApi", "ScreenCaptureApi", "SecretRequestsApi", "ServicesHandle", "SettingsApi", "SecretsApi", "StudyApi", "UsageLogApi",
];

/// The policy of an account, read from the machine's own accounts (the embedded terminal never takes a policy from the UI).
struct AccountPolicies(Arc<dyn iyakki_agent_api::AccountsApi>);
impl iyakki_core_api::PolicySource for AccountPolicies {
    fn policy_of(&self, user: &str) -> Option<iyakki_sec::policy::Policy> {
        self.0.get_public(user).map(|u| u.policy())
    }
}

/// The network of the pages of the tile apps the core runs for the UI (D-SPLIT-03-TILES): the browser's HTTP client behind the connection limits, the
/// person's proxy setting, the ad and tracker filter and the known-bad site list. The app's own host list and the account's policy are checked by the
/// tile runtime before a request gets here; there is no page cache.
fn core_web(data: &std::path::Path) -> Arc<dyn iyakki_web::Fetch> {
    use iyakki_web::net::{LimitedFetch, Limiter, GLOBAL, PER_HOST};
    let fetch = match iyakki_proxy::proxyset::load(data).and_then(|px| iyakki_proxy::https::BrowserFetch::default().with_proxy(px).ok()) {
        Some(f) => f,
        None => iyakki_proxy::https::BrowserFetch::default(),
    };
    let limited: Arc<dyn iyakki_web::Fetch> = Arc::new(LimitedFetch::new(fetch, Arc::new(Limiter::new(PER_HOST, GLOBAL))));
    Arc::new(iyakki_web::badsites::BadSiteFetch::new(Arc::new(iyakki_web::blocker_state::FilteredFetch::new(limited, iyakki_web::blocker_state::Filter::global())), iyakki_web::badsites::Guard::global()))
}
