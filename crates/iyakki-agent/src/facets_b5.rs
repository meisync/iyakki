//! The real implementation of `iyakki_agent_api::facets_b5` (F-SPLIT-03, lane alpha.6/split-a part 2).

use std::sync::Arc;

use iyakki_agent_api::ServicesFacetB5;

use crate::services::Services;

/// The facets over the services of this install.
pub struct ServicesB5 {
    #[allow(dead_code)]
    svc: Arc<Services>,
}

impl Services {
    /// The facets of part 2 of the split-a lane (`cx.services.b5()`).
    pub fn b5(self: &Arc<Self>) -> Arc<dyn ServicesFacetB5> {
        Arc::new(ServicesB5 { svc: self.clone() })
    }
}

impl ServicesFacetB5 for ServicesB5 {
    fn radio_stations(&self, user: &str) -> Option<Arc<dyn iyakki_agent_api::RadioStations>> {
        Some(Arc::new(StationsHandle(crate::radio::StationStore::new(self.svc.dir(), user))))
    }
    fn radio_search(&self, policy: &iyakki_sec::policy::Policy, user: &str, words: &str, limit: usize) -> (Vec<(String, String)>, Option<String>) {
        crate::radiotools::search_stations(&self.svc, policy, user, words, limit)
    }
    fn doctor_gather(&self, user: &str, audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) -> iyakki_shared::doctor::Facts {
        let svc = &self.svc;
        let vpn_light = {
            let (a, u) = (audit.clone(), user.to_string());
            svc.b3().vpn_manager(user, Arc::new(move |x, d| a(&u, x, d))).summary().map(|s| s.light)
        };
        let live = crate::doctor_gather::Live { k3sdev: &svc.k3sdev, vault_locked: !svc.user_vault_unlocked(user), vpn_light };
        crate::doctor_gather::gather(&crate::doctor_gather::Gather { dir: svc.dir(), user, settings: &svc.settings, live: Some(live) })
    }
    fn doctor_slow_input(&self, audit_recent: iyakki_shared::doctor_gather::AuditRecent) -> iyakki_shared::doctor_gather::SlowIn {
        crate::doctor_gather::slow_input(self.svc.dir(), &self.svc.settings, audit_recent)
    }
    fn edit_test_run(&self, user: &str, plan: &iyakki_shared::edittest::TestPlan, seconds: u64, cancel: &std::sync::atomic::AtomicBool) -> Result<(bool, String), String> {
        use iyakki_agent_api::AgentFacetsB2;
        let mut out = None;
        crate::agentapi_gen::AgentApiImpl.proc_owner().as_user(user, &mut || out = Some(crate::edittest::run(plan, seconds, cancel)));
        out.unwrap_or_else(|| Err("the test run did not start".to_string())).map(|t| (t.ok, t.text(&plan.label)))
    }
    fn k8s_kit(&self, spec: iyakki_agent_api::K8sKitSpec) -> Option<Arc<dyn iyakki_agent_api::K8sKit>> {
        let mut t = crate::k8stools::K8sTools::new(spec.runner, spec.profile, &spec.namespace, &spec.workspace, &spec.root, spec.audit);
        t.cluster_phrase = spec.phrase;
        t.registry_setting = spec.registry;
        t.registry_pull = spec.registry_pull;
        t.registry_kind = spec.registry_kind;
        t.registry_probe = spec.registry_probe;
        Some(Arc::new(t))
    }
    fn work_report(&self, dir: &std::path::Path, user: &str) -> Option<Arc<dyn iyakki_shared::k8sexp::WorkReport + Send + Sync>> {
        Some(Arc::new(crate::pending::Ledger::open(&dir.join("pending"), user)))
    }
    fn study_pdf_export(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, item: &iyakki_shared::studypdf::Item, a4: bool, path: Option<&str>) -> Result<String, String> {
        let size = if a4 { iyakki_pdf::PageSize::A4 } else { iyakki_pdf::PageSize::Letter };
        crate::studypdf::export(&self.svc.study, policy, user, data_dir, item, size, path)
    }
    fn story_pdf(&self, story: &iyakki_shared::story::Story, o: &iyakki_shared::story::StoryOptions, pics: &[iyakki_shared::story::StoredPic], a4: bool, date: Option<String>) -> Result<iyakki_shared::storypdf::StoryPdf, String> {
        let size = if a4 { iyakki_pdf::PageSize::A4 } else { iyakki_pdf::PageSize::Letter };
        crate::storypdf::story_pdf(story, o, pics, size, date)
    }
    fn story_store(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> Option<iyakki_shared::story::StoryStore> {
        Some(crate::storykit::open(policy, user, data_dir))
    }
    fn story_store_at(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: std::path::PathBuf) -> Option<iyakki_shared::story::StoryStore> {
        Some(crate::storykit::open_at(policy, user, data_dir, home))
    }
    fn k3s_lookup(&self, st: &dyn iyakki_proxy::Streamer, layout: &iyakki_shared::k3sdev::Layout, backend: iyakki_shared::k3sdev::Backend, registry: &str, inst: Option<&iyakki_shared::k3sdev::Instance>) -> Result<iyakki_shared::k3sdev::Plan, String> {
        match inst {
            Some(i) => crate::k3sdev::lookup_for(&self.svc.clis, st, layout, backend, registry, i),
            None => crate::k3sdev::lookup(&self.svc.clis, st, layout, backend, registry),
        }
    }
    fn lab_catalogue(&self) -> iyakki_shared::lab::Catalogue {
        crate::labkit::catalogue(&self.svc)
    }
    fn lab_check_vision(&self, c: &iyakki_shared::lab::Catalogue, provider: &str, model: &str) -> Result<iyakki_shared::lab::Sees, String> {
        crate::labkit::check_vision_model(c, &self.svc, provider, model)
    }
    fn lab_backends(&self) -> Option<(Arc<dyn iyakki_shared::lab::PictureBackend>, Arc<dyn iyakki_shared::lab::VisionBackend>)> {
        let b = Arc::new(crate::labkit::ServicesBackend(self.svc.clone()));
        Some((b.clone(), b))
    }
    fn lab_frames(&self) -> Arc<dyn iyakki_shared::lab::FrameSource> {
        Arc::new(crate::labkit::Ffmpeg)
    }
    fn scene_agent_chat(
        &self,
        agent: Arc<dyn iyakki_shared::extagent::ExternalAgent>,
        workspace: std::path::PathBuf,
        model: String,
        key: Option<zeroize::Zeroizing<String>>,
        audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
        on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
        exe: std::path::PathBuf,
    ) -> Option<Arc<dyn iyakki_shared::chat::ChatBackend>> {
        Some(Arc::new(crate::scene::agentchat::AgentChat { agent, workspace, model, key, audit, on_usage, exe }))
    }
    fn media_queue(&self, user: &str) -> Option<Arc<dyn iyakki_agent_api::DownloadQueueApi>> {
        Some(Arc::new(QueueHandle { q: self.svc.media_queue(user), svc: self.svc.clone() }))
    }
    fn media_queue_reopen(&self, dir: &std::path::Path, user: &str) -> Option<Arc<dyn iyakki_agent_api::DownloadQueueApi>> {
        Some(Arc::new(QueueHandle { q: Arc::new(crate::mediaqueue::DownloadQueue::open(dir, user)), svc: self.svc.clone() }))
    }
}

/// The download queue of one person with the services its workers need.
struct QueueHandle {
    q: Arc<crate::mediaqueue::DownloadQueue>,
    svc: Arc<Services>,
}

impl QueueHandle {
    fn ctx(&self, w: iyakki_shared::mediaqueue::Who) -> crate::mediaqueue::Context {
        crate::mediaqueue::Context { svc: self.svc.clone(), policy: w.policy, user: w.user, folders: w.folders }
    }
}

impl iyakki_agent_api::DownloadQueueApi for QueueHandle {
    fn set_max_active(&self, n: usize) {
        self.q.set_max_active(n)
    }
    fn set_context(&self, who: iyakki_shared::mediaqueue::Who) {
        self.q.set_context(self.ctx(who))
    }
    fn enqueue(&self, who: iyakki_shared::mediaqueue::Who, req: iyakki_shared::mediaqueue::Request) -> Result<u64, String> {
        self.q.enqueue(self.ctx(who), req)
    }
    fn cancel(&self, id: u64) -> Result<(), String> {
        self.q.cancel(id)
    }
    fn retry(&self, id: u64) -> Result<(), String> {
        self.q.retry(id)
    }
    fn remove(&self, id: u64) -> Result<(), String> {
        self.q.remove(id)
    }
    fn clear_finished(&self) -> usize {
        self.q.clear_finished()
    }
    fn counts(&self) -> (usize, usize) {
        self.q.counts()
    }
    fn last_finished(&self) -> (u64, Option<(u64, iyakki_shared::mediaqueue::Status)>) {
        self.q.last_finished()
    }
    fn views(&self) -> Vec<iyakki_shared::mediaqueue::View> {
        self.q.views()
    }
    fn view(&self, id: u64) -> Option<iyakki_shared::mediaqueue::View> {
        self.q.view(id)
    }
    fn describe(&self) -> String {
        self.q.describe()
    }
}

impl iyakki_agent_api::K8sKit for crate::k8stools::K8sTools {
    fn ns(&self) -> String {
        self.ns.clone()
    }
    fn cluster_label(&self) -> String {
        crate::k8stools::K8sTools::cluster_label(self)
    }
    fn is_local(&self) -> bool {
        self.profile.local
    }
    fn saved_registry(&self) -> (String, String, String) {
        crate::k8stools::K8sTools::saved_registry(self)
    }
    fn root(&self) -> std::path::PathBuf {
        self.root.clone()
    }
    fn preview(&self, name: &str, args: &serde_json::Value) -> Option<String> {
        crate::k8stools::K8sTools::preview(self, name, args)
    }
    fn set_live(&self, sink: Option<iyakki_shared::wsexec::Sink>) {
        crate::k8stools::K8sTools::set_live(self, sink)
    }
    fn built_ref(&self) -> Option<String> {
        crate::k8stools::K8sTools::built_ref(self)
    }
    fn cluster_ref(&self, local: &str) -> Option<String> {
        crate::k8stools::K8sTools::cluster_ref(self, local)
    }
    fn context(&self) -> String {
        self.profile.context.clone()
    }
    fn starter_manifest(&self, name: &str, image: &str, port: u16) -> String {
        crate::k8stools::starter_manifest(name, image, port)
    }
    fn call(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        crate::k8stools::K8sTools::call(self, name, args)
    }
    fn describe(&self, name: &str, args: &serde_json::Value) -> String {
        crate::k8stools::K8sTools::describe(self, name, args)
    }
}

/// The saved stations of one person behind the facet.
pub struct StationsHandle(pub crate::radio::StationStore);

impl iyakki_agent_api::RadioStations for StationsHandle {
    fn list(&self) -> Vec<iyakki_tile_api::radio::Station> {
        self.0.list()
    }
    fn add(&self, name: &str, url: &str) -> Result<iyakki_tile_api::radio::Station, String> {
        self.0.add(name, url)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.remove(name)
    }
    fn trust(&self, station: &iyakki_tile_api::radio::Station, problem: &iyakki_tile_api::radio::CertProblem) -> Result<iyakki_tile_api::radio::Station, String> {
        self.0.trust(station, problem)
    }
    fn forget_trust(&self, name: &str) -> Result<iyakki_tile_api::radio::Station, String> {
        self.0.forget_trust(name)
    }
}
