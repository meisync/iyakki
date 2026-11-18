//! Facets of lane alpha.6/split-a, part 2 (F-SPLIT-03): one handle per stateful object the UI used to name through the agent (`K8sKit`, ...).
//! The real implementation is `iyakki_agent::facets_b5`; the defaults say "nothing here" (a core that serves none of them).

use std::path::PathBuf;
use std::sync::Arc;

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B5: &str = "unavailable: this core does not serve this call";

/// The facets over the services of this install (`cx.services.b5()`).
pub trait ServicesFacetB5: Send + Sync {
    /// The saved radio stations of `user` (Radio tile): the list, add, remove and the certificate choices.
    fn radio_stations(&self, _user: &str) -> Option<Arc<dyn RadioStations>> {
        None
    }
    /// Search the stations directory for the words (the answer: (station name, url) pairs and a note on why the list may be short).
    fn radio_search(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, _words: &str, _limit: usize) -> (Vec<(String, String)>, Option<String>) {
        (Vec::new(), Some(UNAVAILABLE_B5.into()))
    }
    /// DOCTOR: the cheap reads of the running system (the dev cluster, the vault, the VPN); `slow` fields are left empty. Read only.
    fn doctor_gather(&self, _user: &str, _audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) -> iyakki_shared::doctor::Facts {
        iyakki_shared::doctor::Facts::default()
    }
    /// DOCTOR: what the slow reads need (the enabled model servers, the audit tail, developer mode's port).
    fn doctor_slow_input(&self, _audit_recent: iyakki_shared::doctor_gather::AuditRecent) -> iyakki_shared::doctor_gather::SlowIn {
        iyakki_shared::doctor_gather::SlowIn { servers: Vec::new(), audit_recent: Arc::new(|_| Vec::new()), dev_port: None }
    }
    /// Run the tests of `plan` for `user` (the test program is the person's own in SYSTEM, "My processes", while it runs); `(passed, text)`.
    fn edit_test_run(&self, _user: &str, _plan: &iyakki_shared::edittest::TestPlan, _seconds: u64, _cancel: &std::sync::atomic::AtomicBool) -> Result<(bool, String), String> {
        Err(UNAVAILABLE_B5.into())
    }
    /// The `k8s_*` and `deploy_*` tools of a workspace over its connected cluster (the AI IDE's deploy flows drive them step by step).
    fn k8s_kit(&self, _spec: K8sKitSpec) -> Option<Arc<dyn K8sKit>> {
        None
    }
    /// Where a cluster action that takes a while (a rollout) is reported to the ledger of waiting work of `user` (data dir `dir`).
    fn work_report(&self, _dir: &std::path::Path, _user: &str) -> Option<Arc<dyn iyakki_shared::k8sexp::WorkReport + Send + Sync>> {
        None
    }
    /// LEARN: write a plan, quiz or lesson as a PDF in the person's Downloads (the same code as the chat's `study_export_pdf`);
    /// the answer says where the file went.
    fn study_pdf_export(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, _data_dir: &std::path::Path, _item: &iyakki_shared::studypdf::Item, _a4: bool, _path: Option<&str>) -> Result<String, String> {
        Err(UNAVAILABLE_B5.into())
    }
    /// TALES: the story as a PDF (the pictures decoded and put in, videos and undecodable files left out).
    fn story_pdf(&self, _story: &iyakki_shared::story::Story, _o: &iyakki_shared::story::StoryOptions, _pics: &[iyakki_shared::story::StoredPic], _a4: bool, _date: Option<String>) -> Result<iyakki_shared::storypdf::StoryPdf, String> {
        Err(UNAVAILABLE_B5.into())
    }
    /// TALES: the story store of the person (home folder and writable folders as the account has them).
    fn story_store(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, _data_dir: &std::path::Path) -> Option<iyakki_shared::story::StoryStore> {
        None
    }
    /// The same with a home folder given (tests).
    fn story_store_at(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, _data_dir: &std::path::Path, _home: PathBuf) -> Option<iyakki_shared::story::StoryStore> {
        None
    }
    /// SETTINGS -> Dev cluster: look up the latest k3s release for `backend` and make the plan for the managed folder `layout`
    /// (nothing is downloaded but the vendor's checksum file; `inst` is the cluster of the fleet, `None` the primary one).
    fn k3s_lookup(&self, _st: &dyn iyakki_proxy::Streamer, _layout: &iyakki_shared::k3sdev::Layout, _backend: iyakki_shared::k3sdev::Backend, _registry: &str, _inst: Option<&iyakki_shared::k3sdev::Instance>) -> Result<iyakki_shared::k3sdev::Plan, String> {
        Err(UNAVAILABLE_B5.into())
    }
    /// LAB: the models of every tab as the services know them (providers, Lemonade lists).
    fn lab_catalogue(&self) -> iyakki_shared::lab::Catalogue {
        iyakki_shared::lab::Catalogue::default()
    }
    /// LAB: may this model take pictures in (asks the provider's list, nothing is sent).
    fn lab_check_vision(&self, _c: &iyakki_shared::lab::Catalogue, _provider: &str, _model: &str) -> Result<iyakki_shared::lab::Sees, String> {
        Err(UNAVAILABLE_B5.into())
    }
    /// LAB: the picture and vision backends over the services.
    fn lab_backends(&self) -> Option<(Arc<dyn iyakki_shared::lab::PictureBackend>, Arc<dyn iyakki_shared::lab::VisionBackend>)> {
        None
    }
    /// LAB: the video frame source (ffmpeg).
    fn lab_frames(&self) -> Arc<dyn iyakki_shared::lab::FrameSource> {
        Arc::new(NoFrames)
    }
    /// TALES pictures: an external agent (Claude Code and the like) used as a plain text model, one request in and one text out.
    #[allow(clippy::too_many_arguments)]
    fn scene_agent_chat(
        &self,
        _agent: Arc<dyn iyakki_shared::extagent::ExternalAgent>,
        _workspace: PathBuf,
        _model: String,
        _key: Option<zeroize::Zeroizing<String>>,
        _audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
        _on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
        _exe: PathBuf,
    ) -> Option<Arc<dyn iyakki_shared::chat::ChatBackend>> {
        None
    }
    /// The person's media download queue (one list per person, shared by the MEDIA app and the assistant's tools).
    fn media_queue(&self, _user: &str) -> Option<Arc<dyn DownloadQueueApi>> {
        None
    }
    /// A queue read afresh from its file in `dir` (a restart of the core; tests).
    fn media_queue_reopen(&self, _dir: &std::path::Path, _user: &str) -> Option<Arc<dyn DownloadQueueApi>> {
        None
    }
}

/// The media download queue of one person (MEDIA app, Downloads page).
pub trait DownloadQueueApi: Send + Sync {
    /// A different limit of simultaneous downloads (tests; 1 to 8).
    fn set_max_active(&self, n: usize);
    /// Remember who is asking (retries and later steps use it).
    fn set_context(&self, who: iyakki_shared::mediaqueue::Who);
    /// Add one address at the end of the list; the answer is its number.
    fn enqueue(&self, who: iyakki_shared::mediaqueue::Who, req: iyakki_shared::mediaqueue::Request) -> Result<u64, String>;
    fn cancel(&self, id: u64) -> Result<(), String>;
    fn retry(&self, id: u64) -> Result<(), String>;
    fn remove(&self, id: u64) -> Result<(), String>;
    fn clear_finished(&self) -> usize;
    /// (running, waiting)
    fn counts(&self) -> (usize, usize);
    fn last_finished(&self) -> (u64, Option<(u64, iyakki_shared::mediaqueue::Status)>);
    fn views(&self) -> Vec<iyakki_shared::mediaqueue::View>;
    fn view(&self, id: u64) -> Option<iyakki_shared::mediaqueue::View>;
    fn describe(&self) -> String;
}

/// The fake: every method keeps its default.
pub struct NoB5;
impl ServicesFacetB5 for NoB5 {}


/// What the cluster tools of a workspace need.
pub struct K8sKitSpec {
    pub runner: Arc<dyn iyakki_shared::wsexec::Runner>,
    pub profile: iyakki_shared::devcluster::ClusterProfile,
    pub namespace: String,
    /// The workspace's name.
    pub workspace: String,
    pub root: PathBuf,
    /// `(action, detail)` goes to the audit log as `k8s.<action>`.
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// What the registry calls this cluster (`cluster "gpu-box" (remote)`); empty: the profile's own name.
    pub phrase: String,
    /// `project.json` `k8s.registry`, `registry_pull`, `registry_kind`.
    pub registry: String,
    pub registry_pull: String,
    pub registry_kind: String,
    /// Test hook: answers "is the system registry up" instead of the network.
    pub registry_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// The cluster tools of a workspace as the deploy flows drive them (the real one is the agent's `K8sTools`).
pub trait K8sKit: Send + Sync {
    fn ns(&self) -> String;
    fn cluster_label(&self) -> String;
    /// The kubeconfig context of the cluster.
    fn context(&self) -> String;
    /// A starter manifest (deployment and service) for an image, for the deploy flow to write when the project has none.
    fn starter_manifest(&self, name: &str, image: &str, port: u16) -> String;
    /// Is the cluster this computer's own?
    fn is_local(&self) -> bool;
    /// The registry settings in force: (registry, pull address, kind).
    fn saved_registry(&self) -> (String, String, String);
    /// The workspace folder.
    fn root(&self) -> std::path::PathBuf;
    /// What a call would do, shown in the card before the person approves (`None`: nothing to show).
    fn preview(&self, name: &str, args: &serde_json::Value) -> Option<String>;
    /// Where the output of the running command goes line by line (`None`: nowhere).
    fn set_live(&self, sink: Option<iyakki_shared::wsexec::Sink>);
    /// The image reference of the last build.
    fn built_ref(&self) -> Option<String>;
    /// The reference the cluster knows an image by after loading it.
    fn cluster_ref(&self, local: &str) -> Option<String>;
    /// Run a `k8s_*` / `deploy_*` tool.
    fn call(&self, name: &str, args: &serde_json::Value) -> Result<String, String>;
    /// One line describing what a call will do (shown in the approval prompt).
    fn describe(&self, name: &str, args: &serde_json::Value) -> String;
}

/// The saved radio stations of one person.
pub trait RadioStations: Send + Sync {
    fn list(&self) -> Vec<iyakki_tile_api::radio::Station>;
    fn add(&self, name: &str, url: &str) -> Result<iyakki_tile_api::radio::Station, String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    fn trust(&self, station: &iyakki_tile_api::radio::Station, problem: &iyakki_tile_api::radio::CertProblem) -> Result<iyakki_tile_api::radio::Station, String>;
    fn forget_trust(&self, name: &str) -> Result<iyakki_tile_api::radio::Station, String>;
}


/// One managed local cluster (k3s) as the UI sees it: the supervisor behind it stays in the agent.
pub trait DevCluster: Send + Sync {
    fn layout(&self) -> &iyakki_shared::k3sdev::Layout;
    fn installed(&self) -> Option<iyakki_shared::k3sdev::State>;
    fn state(&self) -> iyakki_shared::k3sdev::Svc;
    fn health(&self) -> iyakki_shared::k3sdev::Health;
    fn install(&self, plan: &iyakki_shared::k3sdev::Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String>;
    fn uninstall(&self, typed: &str, user: &str) -> Result<(), String>;
    fn stop(&self, user: &str) -> Result<(), String>;
    fn start(self: Arc<Self>, user: &str) -> Result<(), String>;
    fn restart(self: Arc<Self>, user: &str) -> Result<(), String>;
    fn upgrade(self: Arc<Self>, plan: &iyakki_shared::k3sdev::Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String>;
    fn upgrade_plan(&self, st: &dyn iyakki_proxy::Streamer) -> Result<Option<iyakki_shared::k3sdev::Plan>, String>;
    fn reset(&self, typed: &str, user: &str) -> Result<(), String>;
    fn refresh_health(&self) -> iyakki_shared::k3sdev::Health;
    fn situation(&self) -> iyakki_shared::k3sdev::Situation;
    fn describe(&self, plan: &iyakki_shared::k3sdev::Plan) -> String;
    fn log_tail(&self, lines: usize) -> String;
    fn take_alerts(&self) -> Vec<iyakki_shared::k3sdev::Alert>;
    fn set_audit(&self, f: Arc<dyn Fn(&str, &str, &str) + Send + Sync>);
    fn set_fixed_node_ip(&self, on: bool, user: &str) -> Result<(), String>;
    fn fixed_node_ip_enabled(&self) -> bool;
    fn set_autostart(&self, on: bool, user: &str) -> Result<(), String>;
    /// `kubectl` on this cluster (read only calls, the usual output limit).
    fn kubectl(&self, args: &[&str]) -> iyakki_shared::wsexec::Out;
    /// The same cluster as the explorer of the model hosts sees it.
    fn modelhost_kube(self: Arc<Self>) -> Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync>;
    /// The same cluster as the pods and processes pages of SYSTEM read it.
    fn kubectl_reader(self: Arc<Self>) -> Arc<dyn iyakki_shared::kubectl::Kubectl>;
    /// `kubectl` on this cluster with an output limit (read only calls).
    fn kubectl_cap(&self, args: &[&str], cap: usize) -> iyakki_shared::wsexec::Out;
}

/// The managed local clusters of this install (the primary and the extra ones) as the Clusters panel sees them.
pub trait FleetApi: Send + Sync {
    /// The supervisor of an entry (`None` for a remote entry).
    fn get(&self, e: &iyakki_shared::clusters::ClusterEntry) -> Option<Arc<dyn DevCluster>>;
    fn status(&self, e: &iyakki_shared::clusters::ClusterEntry, test_remote: bool) -> iyakki_shared::clusters::Status;
    fn start(&self, reg: &iyakki_shared::clusters::Registry, id: &str, user: &str, role: iyakki_sec::policy::Role) -> Result<(), String>;
    fn stop(&self, reg: &iyakki_shared::clusters::Registry, id: &str, user: &str, role: iyakki_sec::policy::Role) -> Result<(), String>;
    fn drop_instance(&self, id: &str);
    fn data_dir(&self) -> PathBuf;
    /// The plan to set a local cluster up (the latest k3s release for `backend`; `None` for the primary).
    fn lookup(&self, st: &dyn iyakki_proxy::Streamer, layout: &iyakki_shared::k3sdev::Layout, backend: iyakki_shared::k3sdev::Backend, registry: &str, inst: Option<&iyakki_shared::k3sdev::Instance>) -> Result<iyakki_shared::k3sdev::Plan, String>;
}

struct NoFrames;
impl iyakki_shared::lab::FrameSource for NoFrames {
    fn available(&self) -> Result<(), String> {
        Err(UNAVAILABLE_B5.into())
    }
    fn info(&self, _file: &str) -> Result<iyakki_shared::lab::VideoInfo, String> {
        Err(UNAVAILABLE_B5.into())
    }
    fn frame_rgba(&self, _file: &str, _t: f64, _size: (u32, u32), _cancel: &Arc<std::sync::atomic::AtomicBool>) -> Result<Vec<u8>, String> {
        Err(UNAVAILABLE_B5.into())
    }
}


/// The Ollama server IYAKKI starts for the person (the CLI page): the process stays in the agent.
pub trait OllamaApi: Send + Sync {
    fn state(&self) -> iyakki_shared::ollama::OllamaState;
    fn url(&self) -> String;
    fn models_dir(&self) -> PathBuf;
    fn start(self: Arc<Self>) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    fn set_port(&self, port: u16);
}

/// The local k3s-in-a-sandbox cluster of the CLI page (not the managed dev cluster): its state and the start and stop of it.
pub trait SandboxCluster: Send + Sync {
    fn state(&self) -> iyakki_shared::cluster::ClusterState;
    fn requirements(&self) -> Vec<String>;
    fn start(self: Arc<Self>) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    /// Run `code` in the sandbox (LAB, the editor's Run); `cancel` stops it for real.
    fn sandbox_run_io(&self, language_name: &str, code: &str, stdin: &str, timeout: u64, network: bool, cancel: &std::sync::atomic::AtomicBool) -> Result<iyakki_shared::cluster::SandboxResult, String>;
}


/// The downloaded command line tools (kubectl, ffmpeg, git ...) of the CLI page and the places that look for a tool.
pub trait CliApi: Send + Sync {
    /// Make what is installed here found everywhere (jobs, the terminal, agents) without links in /usr/bin.
    fn export_path(&self);
    /// A program that is on the machine already counts as installed.
    fn set_detect_present(&self, on: bool);
    fn installed(&self) -> Vec<iyakki_shared::clis::Installed>;
    fn is_installed(&self, id: &str) -> Option<iyakki_shared::clis::Installed>;
    fn system_status(&self, id: &str) -> Option<iyakki_shared::clis::SystemStatus>;
    fn job(&self, id: &str) -> Option<iyakki_shared::clis::Job>;
    fn running(&self) -> bool;
    fn install_plan(&self, id: &str) -> Option<String>;
    fn manifest_dir(&self) -> PathBuf;
    fn bin_dir(&self) -> PathBuf;
    fn search_path(&self) -> String;
    fn install(self: Arc<Self>, id: &str, by: &str) -> Result<(), String>;
    fn rollback(&self, id: &str) -> Result<String, String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}
