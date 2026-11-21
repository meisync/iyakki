//! The real implementation of `iyakki_agent_api::facets_b6` (F-SPLIT-03, lane alpha.6/split-c).

use std::sync::Arc;

use iyakki_agent_api::{GeminiLive, LiveSpec, ServicesB6, SpeechKit};
use iyakki_shared::geminilive_host::Running;

use crate::services::Services;

fn steady_ms() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// The Gemini live voice session over the services of this install.
pub struct GeminiLiveReal {
    svc: Arc<Services>,
}

/// The speech of this install over its services.
pub struct SpeechKitReal {
    svc: Arc<Services>,
}

impl SpeechKit for SpeechKitReal {
    fn recording_sink(&self) -> Arc<dyn iyakki_shared::story::Sink> {
        Arc::new(crate::recordings::ServicesSink(self.svc.clone()))
    }
    fn revise_target(&self, ctx: &iyakki_shared::revise::Context) -> iyakki_shared::dictfix::Target {
        crate::revise::target(&self.svc, ctx)
    }
    fn revise(&self, text: &str, kind: &iyakki_shared::revise::Kind, ctx: &iyakki_shared::revise::Context) -> iyakki_shared::revise::Proposal {
        crate::revise::revise(&self.svc, text, kind, ctx)
    }
}

/// The router panel over a backend.
pub struct RouterPanelReal {
    be: Arc<dyn crate::routertools::Backend>,
}

impl RouterPanelReal {
    fn tools(&self) -> crate::routertools::RouterTools {
        crate::routertools::RouterTools::standalone(self.be.clone())
    }
}

impl iyakki_agent_api::RouterPanel for RouterPanelReal {
    fn config(&self) -> iyakki_shared::router::RouterConfig {
        self.be.config()
    }
    fn profiles(&self) -> Vec<iyakki_shared::router_registry::Profile> {
        crate::routertools::profiles_of(self.be.as_ref())
    }
    fn strategies(&self) -> Vec<(String, usize)> {
        self.be.strategies()
    }
    fn progress(&self) -> Option<String> {
        self.be.slot().progress()
    }
    fn last_run(&self) -> Option<String> {
        self.be.last_run()
    }
    fn set(&self, args: &serde_json::Value) -> Result<String, String> {
        self.tools().set(args)
    }
    fn strategy_set(&self, args: &serde_json::Value) -> Result<String, String> {
        self.tools().strategy_set(args)
    }
    fn strategy_remove(&self, args: &serde_json::Value) -> Result<String, String> {
        self.tools().strategy_remove(args)
    }
    fn strategy_add(&self, args: &serde_json::Value) -> Result<String, String> {
        self.tools().strategy_add(args)
    }
    fn cancel(&self) -> Result<String, String> {
        self.tools().cancel()
    }
}

/// The Git engine over the services of this install.
pub struct GitKitReal {
    svc: Arc<Services>,
}

/// A batch with the jobs of this install.
pub struct GitBatchReal {
    b: crate::gitjobs::Batch,
    svc: Arc<Services>,
}

impl iyakki_agent_api::GitBatch for GitBatchReal {
    fn items(&self) -> Vec<iyakki_shared::gitjobs::Item> {
        self.b.items.clone()
    }
    fn phase(&self) -> iyakki_shared::gitjobs::Phase {
        self.b.phase.clone()
    }
    fn prompts(&self) -> Vec<iyakki_shared::gitjobs::HostPrompt> {
        self.b.prompts.clone()
    }
    fn done(&self) -> usize {
        self.b.done()
    }
    fn failed(&self) -> usize {
        self.b.failed()
    }
    fn pending(&self) -> bool {
        self.b.pending()
    }
    fn running_count(&self) -> usize {
        self.b.running_count()
    }
    fn tick(&mut self) {
        self.b.tick(&self.svc.jobs);
    }
    fn trust_host(&mut self, host: &str, port: u16) -> Result<(), String> {
        self.b.trust_host(host, port)
    }
    fn refuse_host(&mut self, host: &str, port: u16) {
        self.b.refuse_host(host, port)
    }
    fn retry(&mut self, item: usize) -> bool {
        self.b.retry(item)
    }
    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.b.changed)
    }
    fn agent_dir(&self) -> Option<std::path::PathBuf> {
        self.b.agent_dir().map(std::path::Path::to_path_buf)
    }
    fn set_state(&mut self, item: usize, state: iyakki_shared::gitjobs::ItemState) {
        if let Some(it) = self.b.items.get_mut(item) {
            it.state = state;
        }
    }
}

impl GitKitReal {
    fn wrap(&self, r: Result<crate::gitjobs::Batch, Vec<(usize, String)>>) -> Result<Box<dyn iyakki_agent_api::GitBatch>, Vec<(usize, String)>> {
        r.map(|b| Box::new(GitBatchReal { b, svc: self.svc.clone() }) as Box<dyn iyakki_agent_api::GitBatch>)
    }
}

impl iyakki_agent_api::GitKit for GitKitReal {
    fn check_git(&self, search: &str) -> iyakki_shared::gitjobs::GitCheck {
        crate::gitjobs::check_git(search)
    }
    fn detect_tools(&self, search: &str) -> Result<iyakki_shared::gitjobs::Tools, String> {
        crate::gitjobs::detect_tools(search)
    }
    fn real_probe(&self, search: &str) -> iyakki_shared::gitjobs::Probe {
        crate::gitjobs::real_probe(search.to_string())
    }
    fn user_ctx(&self, a: iyakki_shared::gitjobs::CtxFor) -> Result<iyakki_shared::gitjobs::Ctx, String> {
        crate::gitjobs::user_ctx(&self.svc, a)
    }
    fn expand_instance(&self, dir: &std::path::Path, user: &str, text: &str) -> Option<Result<iyakki_shared::gitjobs::InstanceRef, String>> {
        crate::gitjobs::expand_instance(dir, user, text)
    }
    fn removal_risks(&self, git: &std::path::Path, dir: &std::path::Path) -> String {
        crate::gitjobs::removal_risks(git, dir)
    }
    fn remove_repo(&self, root: &std::path::Path, folder: &str, typed: &str) -> Result<(), String> {
        crate::gitjobs::remove_repo(root, folder, typed)
    }
    fn repo_status(&self, git: &std::path::Path, dir: &std::path::Path) -> Result<iyakki_shared::gitjobs::RepoStatus, String> {
        crate::gitjobs::status(git, dir)
    }
    fn plan_clone(&self, ctx: iyakki_shared::gitjobs::Ctx, specs: &[iyakki_shared::gitjobs::CloneSpec]) -> Result<Box<dyn iyakki_agent_api::GitBatch>, Vec<(usize, String)>> {
        self.wrap(crate::gitjobs::Batch::plan_clone(ctx, specs))
    }
    fn plan_update(&self, ctx: iyakki_shared::gitjobs::Ctx, repos: &[iyakki_shared::gitsite::RepoEntry], op: iyakki_shared::gitjobs::Op) -> Result<Box<dyn iyakki_agent_api::GitBatch>, Vec<(usize, String)>> {
        self.wrap(crate::gitjobs::Batch::plan_update(ctx, repos, op))
    }
    fn plan_push(&self, ctx: iyakki_shared::gitjobs::Ctx, url: &str, folder: &str, branch: &str) -> Result<Box<dyn iyakki_agent_api::GitBatch>, Vec<(usize, String)>> {
        self.wrap(crate::gitjobs::Batch::plan_push(ctx, url, folder, branch))
    }
}

/// The contract of this install.
pub struct ContractLocalReal(Arc<crate::contract_local::Core>);

impl iyakki_agent_api::ContractLocal for ContractLocalReal {
    fn settings(&self) -> Option<&dyn iyakki_core_api::contract::Settings> {
        Some(&self.0.settings)
    }
    fn vault(&self) -> Option<&dyn iyakki_core_api::contract::Vault> {
        Some(&self.0.vault)
    }
    fn flows_for_env(&self, env: Arc<dyn std::any::Any + Send + Sync>) -> Option<Arc<dyn iyakki_core_api::contract::Flows>> {
        let env = env.downcast::<crate::flows::FlowEnv>().ok()?;
        Some(self.0.flows_for_env(env))
    }
}

/// The terminal of one person.
pub struct TerminalReal(crate::cli::Cli);

impl iyakki_agent_api::TerminalCall for crate::cli::SshCall {
    fn command(&self) -> &str {
        crate::cli::SshCall::command(self)
    }
    fn run(&self, sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String> {
        crate::cli::SshCall::run(self, sink)
    }
}

impl iyakki_agent_api::Terminal for TerminalReal {
    fn prompt(&self) -> String {
        self.0.prompt()
    }
    fn exec(&mut self, line: &str) -> iyakki_shared::cli::CliOut {
        self.0.exec(line)
    }
    fn ssh_call(&self, line: &str) -> Option<Box<dyn iyakki_agent_api::TerminalCall>> {
        self.0.ssh_call(line).map(|c| Box::new(c) as Box<dyn iyakki_agent_api::TerminalCall>)
    }
    fn full(&self) -> bool {
        self.0.policy.cli == iyakki_sec::policy::CliLevel::Full
    }
}

/// The documents search of one person over the services.
pub struct RagKitReal {
    svc: Arc<Services>,
    user: String,
}

impl iyakki_agent_api::RagKit for RagKitReal {
    fn research(&self, question: &str, k: usize, model: Option<&dyn iyakki_shared::chat::ChatBackend>) -> iyakki_shared::agenticrag::Ledger {
        let access = crate::ragtools::RagAccess::for_services(&self.svc, &self.user);
        let l = crate::agenticrag::research(&access, model, &crate::agenticrag::Request { question, collections: &[], budget: crate::agenticrag::Budget { k, ..crate::agenticrag::Budget::default() } }, &std::sync::atomic::AtomicBool::new(false));
        crate::ragtools::LedgerStore::open(self.svc.dir(), &self.user).push(&l);
        l
    }
    fn runs(&self) -> Vec<iyakki_shared::agenticrag::Ledger> {
        crate::ragtools::LedgerStore::open(self.svc.dir(), &self.user).runs()
    }
}

/// The SFTP server behind the facet.
pub struct SftpReal(pub Arc<crate::sftp::Sftp>);

impl iyakki_agent_api::SftpServer for SftpReal {
    fn dir(&self) -> std::path::PathBuf {
        self.0.dir().to_path_buf()
    }
    fn config(&self) -> iyakki_shared::sftp::Config {
        self.0.config()
    }
    fn status(&self) -> iyakki_shared::sftp::Status {
        self.0.status()
    }
    fn problem(&self) -> Option<String> {
        self.0.problem()
    }
    fn enable(&self, by: &str) -> Result<iyakki_shared::sftp::Status, String> {
        self.0.enable(by)
    }
    fn disable(&self, by: &str) -> bool {
        self.0.disable(by)
    }
    fn configure_dyn(&self, f: &mut dyn FnMut(&mut iyakki_shared::sftp::Config) -> Result<(), String>) -> Result<iyakki_shared::sftp::Config, String> {
        self.0.configure(|c| f(c))
    }
    fn add_key(&self, user: &str, line: &str) -> Result<String, String> {
        crate::sftp::add_key(self.0.dir(), user, line)
    }
    fn remove_key(&self, user: &str, fingerprint: &str) -> Result<usize, String> {
        crate::sftp::remove_key(self.0.dir(), user, fingerprint)
    }
}

impl ServicesB6 for Services {
    fn sftp_server(self: &Arc<Self>, data: &std::path::Path, login: iyakki_agent_api::SftpLogin, lookup: iyakki_agent_api::SftpLookup, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::SftpServer> {
        let auth = crate::sftp::FnAuth::new(login, lookup);
        Arc::new(SftpReal(crate::sftp::Sftp::shared(data, Arc::new(auth), audit)))
    }
    fn sftp_status_lines(self: &Arc<Self>, data: &std::path::Path) -> Vec<String> {
        crate::sftp::status_lines(data)
    }
    fn rag_kit(self: &Arc<Self>, user: &str) -> Arc<dyn iyakki_agent_api::RagKit> {
        Arc::new(RagKitReal { svc: self.clone(), user: user.to_string() })
    }
    fn terminal(self: &Arc<Self>, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>) -> Box<dyn iyakki_agent_api::Terminal> {
        Box::new(TerminalReal(crate::cli::Cli::new(user, policy, host).with_skills(self.skills.clone(), self.packages.clone()).with_services(self.clone())))
    }
    fn contract_local(self: &Arc<Self>) -> Arc<dyn iyakki_agent_api::ContractLocal> {
        Arc::new(ContractLocalReal(crate::contract_local::core_for(self)))
    }
    fn git_kit(self: &Arc<Self>) -> Arc<dyn iyakki_agent_api::GitKit> {
        Arc::new(GitKitReal { svc: self.clone() })
    }
    fn remote_kit(self: &Arc<Self>, user: &str, data: &std::path::Path, downloads: &str, local: Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::RemoteKit> {
        let secrets = Arc::new(crate::remotefiles::VaultSecrets { svc: self.clone(), user: user.to_string() });
        Arc::new(RemoteKitReal(crate::remotefiles::shared(data, user, secrets, local, audit, downloads)))
    }
    fn router_panel(self: &Arc<Self>, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, gate: Arc<dyn iyakki_core_api::router::Router>) -> Arc<dyn iyakki_agent_api::RouterPanel> {
        let be = crate::routertools::FileBackend::for_services(self.clone(), user, audit)
            .with_gate(gate)
            .with_pool(Arc::new(crate::router_pool::ProviderPool::for_services(self.clone())))
            .with_sink(Arc::new(crate::router::ServicesSink { svc: self.clone(), user: user.to_string() }));
        Arc::new(RouterPanelReal { be: Arc::new(be) })
    }
    fn router_panel_over(self: &Arc<Self>, backend: Box<dyn std::any::Any + Send>) -> Option<Arc<dyn iyakki_agent_api::RouterPanel>> {
        let be = backend.downcast::<Arc<dyn crate::routertools::Backend>>().ok()?;
        Some(Arc::new(RouterPanelReal { be: *be }))
    }
    fn speech_kit(self: &Arc<Self>) -> Arc<dyn SpeechKit> {
        Arc::new(SpeechKitReal { svc: self.clone() })
    }

    fn gemini_live(self: &Arc<Self>) -> Arc<dyn GeminiLive> {
        Arc::new(GeminiLiveReal { svc: self.clone() })
    }
}

impl GeminiLive for GeminiLiveReal {
    fn spawn(&self, spec: LiveSpec) -> Running {
        let svc = self.svc.clone();
        let user = spec.user.clone();
        let svc_key = svc.clone();
        let connector: Arc<dyn crate::geminilive::LiveConnector> = match spec.connector.and_then(|c| c.downcast::<Arc<dyn crate::geminilive::LiveConnector>>().ok()) {
            Some(c) => *c,
            None => Arc::new(crate::geminilive_ws::WsConnector::default()),
        };
        let key_override = spec.key;
        let live = crate::geminilive::LiveDeps {
            dir: svc.dir().to_path_buf(),
            user: Arc::new(move || user.clone()),
            key: match key_override {
                Some(k) => Arc::new(move || Ok(k.clone())),
                None => Arc::new(move || svc_key.secrets.resolve(iyakki_shared::gemini::KEY_REF, None::<&iyakki_shared::vault::LocalVault>).map(|k| k.to_string()).map_err(|e| e.to_string())),
            },
            connector,
            now_ms: Arc::new(steady_ms),
            today: Arc::new(iyakki_clock::today),
        };
        let sink = spec.sink.unwrap_or_else(|| Arc::new(crate::geminilive_audio::SpeechSink(svc.clone())));
        crate::geminilive_host::spawn(crate::geminilive_host::Parts {
            live,
            tools: spec.tools,
            board: spec.board,
            audit: spec.audit,
            auto: spec.auto,
            mic: spec.mic,
            sink,
            speaker: spec.speaker,
            log: spec.log,
            held: crate::geminilive_audio::product_held(),
            stop_holder: crate::geminilive_audio::product_stop_holder(),
            echo_clock: Arc::new(steady_ms),
        })
    }
}

/// The stateless helpers of the real core.
pub struct RealHelpersB6;

impl iyakki_agent_api::HelpersB6 for RealHelpersB6 {
    fn git_http(&self) -> Arc<dyn iyakki_shared::gitconn::GitHttp> {
        Arc::new(crate::gitnet::NetGit::default())
    }
    fn access_revoke(&self, data: &std::path::Path, installation: &str, capability: &str) -> Result<bool, String> {
        crate::accessbroker_revoke::revoke(data, installation, capability)
    }
    fn access_limit(&self, data: &std::path::Path, installation: &str, capability: &str, hours: Option<u32>) -> Result<bool, String> {
        crate::accessbroker_revoke::limit(data, installation, capability, hours, crate::clock::now_unix())
    }
    fn dl_fetch_start(&self, dir: &std::path::Path, force: bool) -> bool {
        crate::dlmanifest::fetch::start(dir, force)
    }
    fn dl_fetch_take_audit(&self) -> Vec<(String, String)> {
        crate::dlmanifest::fetch::take_audit()
    }
    fn dl_fetch_status(&self, dir: &std::path::Path) -> iyakki_shared::dlmanifest_fetch::CheckStatus {
        crate::dlmanifest::fetch::status(dir)
    }
    fn camera_feed(&self, hub: Arc<dyn std::any::Any + Send + Sync>, who: Arc<dyn Fn(&str) -> Box<dyn std::any::Any + Send> + Send + Sync>) -> Option<Arc<dyn iyakki_shared::clientmedia::CameraFeed>> {
        let hub = hub.downcast::<crate::cameras::Hub>().ok()?;
        let who: Arc<dyn Fn(&str) -> crate::cameras::Who + Send + Sync> = Arc::new(move |p| *who(p).downcast::<crate::cameras::Who>().expect("the camera person of the agent core is the agent's Who"));
        Some(Arc::new(crate::clientmedia::HubFeed { hub, who }))
    }
    fn avaddon_plan(&self, data: &std::path::Path) -> iyakki_agent_api::AvaddonPlan {
        let found = crate::avaddon_install::plan(&crate::avaddon_install::Places::here(data));
        iyakki_agent_api::AvaddonPlan { has_source: found.source.is_some(), review: found.review }
    }
    fn router_zone(&self, cfg: &iyakki_shared::providers::ProviderCfg) -> iyakki_shared::router_registry::Zone {
        crate::router_registry::zone_of(cfg)
    }
    fn deploy_kit(&self) -> Arc<dyn iyakki_agent_api::DeployKit> {
        Arc::new(DeployKitReal)
    }
}

/// The agent's launch of what the card asked for: the tenant's example configuration with the card's image.
fn launch_of(l: &iyakki_shared::cloudlaunch::LaunchSpec) -> crate::cloudlaunch::Launch {
    let mut tenant = crate::cloudtenant::TenantConfig::example(&l.tenant);
    match &l.step {
        iyakki_shared::cloudlaunch::ImageStep::Push { dest } => tenant.image = dest.clone(),
        _ if !l.image.is_empty() => tenant.image = l.image.clone(),
        _ => {}
    }
    crate::cloudlaunch::Launch { tenant, vault: l.vault.clone(), routes: l.routes.clone(), image: l.step.clone(), cluster: l.cluster.clone() }
}

/// The deploy and registry steps over this machine.
pub struct DeployKitReal;

impl iyakki_agent_api::DeployKit for DeployKitReal {
    fn reg_status(&self, c: &iyakki_shared::workspace::K8sConfig, run: &dyn iyakki_shared::wsexec::Runner) -> iyakki_shared::regsetup::RegStatus {
        crate::regsetup::status(c, run)
    }
    fn reg_probe(&self, run: &dyn iyakki_shared::wsexec::Runner, k: &iyakki_shared::regsetup::KubeCtx) -> iyakki_shared::regsetup::Facts {
        crate::regsetup::probe(run, k)
    }
    fn reg_apply(&self, p: &iyakki_shared::regsetup::SetupPlan, run: &dyn iyakki_shared::wsexec::Runner, approved: bool) -> Result<iyakki_shared::regsetup::Applied, String> {
        crate::regsetup::apply(p, run, approved)
    }
    fn localreg_target(&self, p: &iyakki_shared::devcluster::ClusterProfile) -> iyakki_shared::localreg::Target {
        crate::localreg::target_of(p)
    }
    fn localreg_inspect(&self, run: &Arc<dyn iyakki_shared::wsexec::Runner>, t: &iyakki_shared::localreg::Target) -> iyakki_shared::localreg::State {
        crate::localreg::inspect(run, &crate::ocipush::TcpHttp::default(), t)
    }
    fn localreg_check_dependency(&self, run: &Arc<dyn iyakki_shared::wsexec::Runner>, t: &iyakki_shared::localreg::Target) -> iyakki_shared::localreg::Dependency {
        crate::localreg::check_dependency(run, t)
    }
    fn localreg_remove(&self, run: &Arc<dyn iyakki_shared::wsexec::Runner>, t: &iyakki_shared::localreg::Target, typed: &str, say: &mut dyn FnMut(&str)) -> Result<(), String> {
        crate::localreg::remove(run, &crate::ocipush::TcpHttp::default(), t, typed, say)
    }
    fn cloud_default_image(&self) -> String {
        crate::cloudtenant::TenantConfig::example("x").image
    }
    fn cloud_plan(&self, l: &iyakki_shared::cloudlaunch::LaunchSpec) -> iyakki_shared::cloudlaunch::Plan {
        crate::cloudlaunch::plan(&launch_of(l))
    }
    fn cloud_preview(&self, l: &iyakki_shared::cloudlaunch::LaunchSpec, p: &iyakki_shared::cloudlaunch::Plan) -> String {
        crate::cloudlaunch::preview(&launch_of(l), p)
    }
    fn cloud_execute(&self, l: &iyakki_shared::cloudlaunch::LaunchSpec, p: &iyakki_shared::cloudlaunch::Plan, work: &std::path::Path, kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        crate::cloudlaunch::execute(&launch_of(l), p, work, kubectl, push)
    }
    fn cloud_cli(&self, args: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
        crate::cloudcli::run(args, out)
    }
}

impl iyakki_agent_api::AgentFacetsB6 for crate::AgentApiImpl {
    fn helpers_b6(&self) -> &dyn iyakki_agent_api::HelpersB6 {
        &RealHelpersB6
    }
}

/// The person's remote files manager (sessions, host key pins, transfer queue; shared with the assistant's tools) behind the tile's
/// source trait: the conversion between this crate's types and the tile API's.
pub struct RemoteKitReal(pub Arc<crate::remotefiles::Manager>);

fn proto_to_api(p: crate::remotefiles::Proto) -> iyakki_tile_api::remote::Proto {
    match p {
        crate::remotefiles::Proto::Sftp => iyakki_tile_api::remote::Proto::Sftp,
        crate::remotefiles::Proto::Ftps => iyakki_tile_api::remote::Proto::Ftps,
        crate::remotefiles::Proto::Ftp => iyakki_tile_api::remote::Proto::Ftp,
    }
}
fn proto_from_api(p: iyakki_tile_api::remote::Proto) -> crate::remotefiles::Proto {
    match p {
        iyakki_tile_api::remote::Proto::Sftp => crate::remotefiles::Proto::Sftp,
        iyakki_tile_api::remote::Proto::Ftps => crate::remotefiles::Proto::Ftps,
        iyakki_tile_api::remote::Proto::Ftp => crate::remotefiles::Proto::Ftp,
    }
}

fn conn_to_api(c: crate::remotefiles::Conn) -> iyakki_tile_api::remote::Conn {
    iyakki_tile_api::remote::Conn { name: c.name, host: c.host, port: c.port, user: c.user, proto: proto_to_api(c.proto), auth: if c.auth == crate::remotefiles::Auth::Key { iyakki_tile_api::remote::Auth::Key } else { iyakki_tile_api::remote::Auth::Password }, key: c.key, plain_ok: c.plain_ok, has_password: c.has_password }
}

fn conn_from_api(c: &iyakki_tile_api::remote::Conn) -> crate::remotefiles::Conn {
    crate::remotefiles::Conn { name: c.name.clone(), host: c.host.clone(), port: c.port, user: c.user.clone(), proto: proto_from_api(c.proto), auth: if c.auth == iyakki_tile_api::remote::Auth::Key { crate::remotefiles::Auth::Key } else { crate::remotefiles::Auth::Password }, key: c.key.clone(), plain_ok: c.plain_ok, has_password: false }
}

fn entry_to_api(e: crate::remotefiles::Entry) -> iyakki_tile_api::remote::Entry {
    iyakki_tile_api::remote::Entry { name: e.name, is_dir: e.is_dir, is_link: e.is_link, size: e.size, modified: e.modified, perms: e.perms }
}

fn card_to_api(c: crate::remotefiles::HostCard) -> iyakki_tile_api::remote::HostCard {
    iyakki_tile_api::remote::HostCard { host: c.host, port: c.port, kind: c.kind, fingerprint: c.fingerprint, pinned: c.pinned }
}

fn result_to_api(r: crate::remotefiles::JobResult) -> iyakki_tile_api::remote::JobResult {
    match r {
        crate::remotefiles::JobResult::Opened(crate::remotefiles::Opened::Ready(id)) => iyakki_tile_api::remote::JobResult::Opened(iyakki_tile_api::remote::Opened::Ready(id)),
        crate::remotefiles::JobResult::Opened(crate::remotefiles::Opened::Unknown(c)) => iyakki_tile_api::remote::JobResult::Opened(iyakki_tile_api::remote::Opened::Unknown(card_to_api(c))),
        crate::remotefiles::JobResult::Opened(crate::remotefiles::Opened::Changed(c)) => iyakki_tile_api::remote::JobResult::Opened(iyakki_tile_api::remote::Opened::Changed(card_to_api(c))),
        crate::remotefiles::JobResult::Listing(p, e) => iyakki_tile_api::remote::JobResult::Listing(p, e.into_iter().map(entry_to_api).collect()),
        crate::remotefiles::JobResult::Done(m) => iyakki_tile_api::remote::JobResult::Done(m),
        crate::remotefiles::JobResult::Failed(m) => iyakki_tile_api::remote::JobResult::Failed(m),
    }
}

fn job_from_api(j: iyakki_tile_api::remote::Job) -> crate::remotefiles::Job {
    match j {
        iyakki_tile_api::remote::Job::Open(n) => crate::remotefiles::Job::Open(n),
        iyakki_tile_api::remote::Job::List(s, p) => crate::remotefiles::Job::List(s, p),
        iyakki_tile_api::remote::Job::Mkdir(s, p) => crate::remotefiles::Job::Mkdir(s, p),
        iyakki_tile_api::remote::Job::Rename(s, a, b) => crate::remotefiles::Job::Rename(s, a, b),
        iyakki_tile_api::remote::Job::Delete(s, p, d) => crate::remotefiles::Job::Delete(s, p, d),
    }
}

fn info_to_api(t: crate::remotefiles::TransferInfo) -> iyakki_tile_api::remote::TransferInfo {
    let state = match t.state {
        crate::remotefiles::State::Queued => iyakki_tile_api::remote::State::Queued,
        crate::remotefiles::State::Running => iyakki_tile_api::remote::State::Running,
        crate::remotefiles::State::Done => iyakki_tile_api::remote::State::Done,
        crate::remotefiles::State::Failed => iyakki_tile_api::remote::State::Failed,
        crate::remotefiles::State::Cancelled => iyakki_tile_api::remote::State::Cancelled,
    };
    iyakki_tile_api::remote::TransferInfo { id: t.id, download: t.download, remote: t.remote, local: t.local, done: t.done, total: t.total, state, note: t.note, rate: t.rate, resumed: t.resumed }
}

impl iyakki_tile_api::remote::RemoteSource for RemoteKitReal {
    fn connections(&self) -> Vec<iyakki_tile_api::remote::Conn> {
        self.0.connections().into_iter().map(conn_to_api).collect()
    }
    fn save(&self, conn: &iyakki_tile_api::remote::Conn, password: Option<&str>, replacing: Option<&str>) -> Result<(), String> {
        self.0.save(&conn_from_api(conn), password, replacing)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.remove(name)
    }
    fn key_names(&self) -> Vec<String> {
        self.0.key_names()
    }
    fn submit(&self, job: iyakki_tile_api::remote::Job) -> u64 {
        self.0.submit(job_from_api(job))
    }
    fn take(&self, ticket: u64) -> Option<iyakki_tile_api::remote::JobResult> {
        self.0.take(ticket).map(result_to_api)
    }
    fn trust(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        self.0.trust(name, fingerprint)
    }
    fn forget_host(&self, name: &str) -> Result<(), String> {
        self.0.forget_host(name)
    }
    fn close(&self, session: u64) {
        self.0.close(session)
    }
    fn start_transfer(&self, req: &iyakki_tile_api::remote::TransferReq) -> Result<u64, String> {
        self.0.start_transfer(&crate::remotefiles::TransferReq { session: req.session, download: req.download, remote: req.remote.clone(), local: req.local.clone(), overwrite: req.overwrite })
    }
    fn transfers(&self) -> Vec<iyakki_tile_api::remote::TransferInfo> {
        self.0.transfers().into_iter().map(info_to_api).collect()
    }
    fn cancel_transfer(&self, id: u64) {
        self.0.cancel_transfer(id)
    }
    fn retry_transfer(&self, id: u64) -> Result<u64, String> {
        self.0.retry_transfer(id)
    }
    fn clear_finished(&self) {
        self.0.clear_finished()
    }
    fn download_dir(&self) -> String {
        self.0.download_dir()
    }
}


impl iyakki_agent_api::RemoteKit for RemoteKitReal {
    fn wrap_tools(&self, inner: Arc<dyn iyakki_shared::agent::Tools>) -> Arc<dyn iyakki_shared::agent::Tools> {
        Arc::new(crate::remotefilestools::RemoteTools::new(inner, self.0.clone()))
    }
}
