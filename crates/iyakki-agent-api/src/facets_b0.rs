//! Hand-written facets of bucket b0 of the UI/agent split (F-SPLIT-03, alpha.5): the agent's functions and small handles that the UI used to
//! name directly (`iyakki_agent::providers`, `quiet`, `target`, `idebrowser`, `ocipush`, `linuxapps`, ...). One narrow trait per module; every
//! method has a default that refuses or answers empty, so a core without the facet (and the fake) never panics.
//!
//! Two doors:
//! * [`B0Facets`]: stateless calls, `cx.agent.b0()` (or `iyakki_apps::agent().b0()` where no `cx` is in reach).
//! * [`B0Services`]: calls that need the person's services (settings, jobs), `cx.services.b0()`; the real handle is built by the agent over
//!   the services, a test gives [`NoB0`] or its own recording fake.
//!
//! The real implementation is `iyakki-agent/src/facets_b0.rs`.

use std::path::Path;
use std::sync::Arc;

use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::idebrowser::OpenRequest;
use iyakki_shared::checkup::{Inventory, ScreenOut};
use iyakki_shared::disks::Actor;
use iyakki_shared::mcp::{ServerSpec, StoredServer};
use iyakki_shared::jobs::Job;
use iyakki_shared::media::{Check, QueueState};
use iyakki_shared::quickinstall_uninstall::{Opts, Plan};
use iyakki_shared::quickinstall_ollama::HostInfo;
use iyakki_shared::quickinstall::{HwFirmwareOpts, Facts, Item, Latest, Launch, Presence};
use iyakki_shared::ocipush::RegistryHttp;
use iyakki_shared::quiet::Env as QuietEnv;
use iyakki_shared::target::{ChangePlan, Target};

/// The reasoning stream of a model answer (`iyakki_agent::providers`).
pub trait ProvidersFacet: Send + Sync {
    /// Where the model's thinking goes while its answer is read on THIS thread; `None` clears it.
    fn set_thinking_sink(&self, _sink: Option<Box<dyn FnMut(&str)>>) {}
}

/// Stopping what plays on the sound card before a shutdown or a hand-over (`iyakki_agent::quiet`).
pub trait QuietFacet: Send + Sync {
    /// The machine as the shutdown sees it (`amixer`, signals); the default does nothing and says it is no process.
    fn system(&self) -> Arc<dyn QuietEnv> {
        Arc::new(NoQuietEnv)
    }
}

/// What a workspace is for (`iyakki_agent::target`): the plan of a change and its recording.
pub trait TargetFacet: Send + Sync {
    /// Look at the workspace and say what a change to `to` would do (reads only).
    fn plan_change(&self, _root: &Path, _name: &str, to: Target) -> ChangePlan {
        ChangePlan { from: None, to, drop_files: Vec::new(), drop_rows: Vec::new(), teardown_namespace: None, summary: "unavailable: this core does not change targets".into() }
    }
    /// Record the new target; the plan and the message that asks the assistant to convert the project.
    fn apply_change(&self, _root: &Path, _name: &str, _to: Target) -> Result<(ChangePlan, String), String> {
        Err("unavailable: this core does not change targets".into())
    }
}

/// The IDE's Browser tab as the assistant drives it (`iyakki_agent::idebrowser`).
pub trait IdeBrowserFacet: Send + Sync {
    /// Is the IDE browser offered to this account?
    fn offered(&self, _policy: &Policy) -> bool {
        false
    }
    /// What the IDE has been asked to show since it last looked.
    fn take_requests(&self, _user: &str, _ws: &str) -> Vec<OpenRequest> {
        Vec::new()
    }
}

/// How the UI reaches a local registry (`iyakki_agent::ocipush`).
pub trait OciPushFacet: Send + Sync {
    /// The HTTP the registry calls go through; the default refuses every request.
    fn http(&self) -> Arc<dyn RegistryHttp> {
        Arc::new(NoRegistryHttp)
    }
}

/// The Linux sandboxes (`iyakki_agent::linuxapps`), the one call the IDE's command line makes.
pub trait LinuxAppsFacet: Send + Sync {
    /// Make the workspace's commands run in the sandbox `name` (`None`: on this machine again); the sentence for the person.
    fn set_workspace_sandbox(&self, _data: &Path, _user: &str, _exe: &Path, _root: &Path, _name: Option<&str>) -> Result<String, String> {
        Err("unavailable: this core does not serve sandboxes".into())
    }
}

/// The one media player of the shell (`iyakki_agent::media::MediaPlayer`): at most one thing plays; the queue goes on when the MEDIA app is left.
/// The default is a player that plays nothing and refuses.
pub trait MediaPlay: Send + Sync {
    /// MP3 plays inside IYAKKI (no program is started for it).
    fn plays_mp3_in_process(&self) -> bool {
        false
    }
    /// Play `target` for `user`, labelled `label`; stops what played before.
    fn play(&self, _user: &str, _label: &str, _target: &str) -> Result<(), String> {
        Err("unavailable: this core does not play media".into())
    }
    /// Replace the queue; the generation number of the new queue.
    fn set_queue(&self, _user: &str, _items: Vec<String>, _pos: usize, _active: bool) -> u64 {
        0
    }
    fn queue_state(&self) -> QueueState {
        QueueState::default()
    }
    /// Move `delta` places in the queue; the target now playing.
    fn skip(&self, _delta: i64) -> Option<String> {
        None
    }
    fn stop(&self) {}
    /// Is it still playing, and how did it end?
    fn check(&self) -> Check {
        Check::Idle
    }
    fn now_target(&self) -> Option<String> {
        None
    }
    fn elapsed(&self) -> Option<u64> {
        None
    }
    /// The label of what plays, `None` when nothing does.
    fn now_playing(&self) -> Option<String> {
        None
    }
}

/// Google Gemini's live voice settings (`iyakki_agent::gemini`): what the shell reads before it offers the key and the button.
pub trait GeminiFacet: Send + Sync {
    /// The assistant is switched on in this data folder.
    fn voice_enabled(&self, _data: &Path) -> bool {
        false
    }
    /// The start key's name (`Super+V`); empty when none.
    fn voice_key(&self, _data: &Path) -> String {
        String::new()
    }
    /// A wake phrase hands over to the live assistant.
    fn voice_wake_handoff(&self, _data: &Path) -> bool {
        false
    }
}

/// The machine check-up (`iyakki_agent::checkup`): what needs the real machine; the steps themselves are pure (`iyakki_shared::checkup`).
/// Quick install calls that need nothing but their arguments (`iyakki_agent::quickinstall`).
pub trait QuickInstallFacet: Send + Sync {
    /// The step titles of an install order with the runtimes the catalogue installs underneath (`before(item)` gives the catalogue ids).
    fn order_titles_with(&self, steps: &[Item], _before: &dyn Fn(Item) -> Vec<&'static str>) -> Vec<String> {
        steps.iter().map(|s| s.title().to_string()).collect()
    }
    /// What the machine says about an Ollama installed on the host (its models folder, the room left).
    fn ollama_host_info(&self, _data: &Path) -> HostInfo {
        HostInfo::default()
    }
}

/// The machine check-up (`iyakki_agent::checkup`): what needs the real machine; the steps themselves are pure (`iyakki_shared::checkup`).
pub trait CheckupFacet: Send + Sync {
    /// Everything the steps read about the machine; `screens` come from the shell. The default is an empty machine.
    fn inventory(&self, _screens: Vec<ScreenOut>) -> Inventory {
        Inventory::default()
    }
    /// The speaker test's tone as a WAV file.
    fn tone_wav(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// The MCP list (`iyakki_agent::mcp`).
pub trait McpFacet: Send + Sync {
    /// The sample server (`--mcp-demo` of this program).
    fn demo_spec(&self) -> ServerSpec {
        ServerSpec { name: "iyakki-demo".into(), command: String::new(), args: vec!["--mcp-demo".into()], env: Default::default() }
    }
    /// Where a server of the list comes from: `sample`, `added here` or `mcp.json`.
    fn origin_of(&self, _stored: &[StoredServer], _name: &str) -> &'static str {
        "mcp.json"
    }
}

/// The shell's media player, as one facet.
pub trait MediaFacet: Send + Sync {
    /// The player the whole shell shares.
    fn player(&self) -> Arc<dyn MediaPlay> {
        Arc::new(NoMedia)
    }
}

/// Everything of bucket b0 that needs nothing but its arguments, one accessor each.
pub trait B0Facets: Send + Sync {
    fn providers(&self) -> &dyn ProvidersFacet {
        &NoB0
    }
    fn quiet(&self) -> &dyn QuietFacet {
        &NoB0
    }
    fn target(&self) -> &dyn TargetFacet {
        &NoB0
    }
    fn idebrowser(&self) -> &dyn IdeBrowserFacet {
        &NoB0
    }
    fn ocipush(&self) -> &dyn OciPushFacet {
        &NoB0
    }
    fn linuxapps(&self) -> &dyn LinuxAppsFacet {
        &NoB0
    }
    fn media(&self) -> &dyn MediaFacet {
        &NoB0
    }
    fn gemini(&self) -> &dyn GeminiFacet {
        &NoB0
    }
    fn mcp(&self) -> &dyn McpFacet {
        &NoB0
    }
    fn checkup(&self) -> &dyn CheckupFacet {
        &NoB0
    }
    fn quickinstall(&self) -> &dyn QuickInstallFacet {
        &NoB0
    }
}

/// The calls of bucket b0 that run over one person's services (settings, jobs); built by `Services::b0()` in the agent.
pub trait B0Services: Send + Sync {
    /// Look the time zone up on a thread of its own when it is due (or now, with `force`) and save it; `on_change` is told the new zone.
    /// `false` when nothing was started.
    fn tz_refresh_in_background(&self, _force: bool, _on_change: Arc<dyn Fn(&str) + Send + Sync>) -> bool {
        false
    }
    /// The IDE browser's tools over `inner` (the answer is `inner` itself in a core without the facet).
    fn ide_browser_tools(&self, inner: Arc<dyn Tools>, _policy: Policy, _user: &str, _ws: &str, _root: &Path) -> Arc<dyn Tools> {
        inner
    }
    // ---- Quick install (`iyakki_agent::quickinstall`): the installer jobs of the person's services
    /// Where an item stands on this machine.
    fn qi_presence(&self, _item: Item) -> Presence {
        Presence::Missing
    }
    /// The installer jobs of a person (running first, newest first).
    fn qi_install_jobs(&self, _owner: &str) -> Vec<Job> {
        Vec::new()
    }
    /// Start the item and what it needs that `present` says is missing (detached jobs).
    fn qi_start(&self, _owner: &str, _target: Item, _present: &dyn Fn(Item) -> bool, _launch: &Launch) -> Result<Vec<Job>, String> {
        Err("unavailable: this core does not install".into())
    }
    /// Start a restricted firmware pack (its licence must have been accepted).
    fn qi_start_firmware_pack(&self, _owner: &str, _item: Item, _from: Option<&Path>, _launch: &Launch) -> Result<Job, String> {
        Err("unavailable: this core does not install".into())
    }
    /// Whether a newer version exists for each installed item.
    fn qi_check_upgrades(&self) -> Vec<(Item, Latest)> {
        Vec::new()
    }
    /// What the Setup page needs to know about the machine.
    fn qi_gather_facts(&self) -> Facts {
        Facts::default()
    }
    /// Follow-up requests of finished installs (vault hand-offs and starts); one line each.
    fn qi_service_requests(&self) -> Vec<Result<String, String>> {
        Vec::new()
    }
    /// The plan to remove an item (what goes, what stays) and the warning about the CLI catalogue's runtimes, if any.
    fn qi_uninstall_plan(&self, item: Item, _user: &str) -> (Plan, Option<String>) {
        (Plan { item, steps: Vec::new(), data_requires: false, always_stays: Vec::new(), claim_gb: 0, data_bytes: 0, notes: vec!["unavailable: this core does not remove software".into()] }, None)
    }
    /// Remove the item the way the plan said (the person approved it); the line to show.
    fn qi_uninstall(&self, item: Item, _user: &str, _opts: Opts, _progress: Option<Arc<std::sync::Mutex<String>>>) -> Result<String, String> {
        Err(format!("{}: unavailable: this core does not remove software", item.title()))
    }
    /// The job of Apple's camera firmware (its own job, not a step of the order).
    fn qi_start_camera_firmware(&self, _owner: &str, _launch: &Launch) -> Result<Job, String> {
        Err("unavailable: this core does not install".into())
    }
    /// The job of Apple's Wi-Fi and Bluetooth firmware, read from the person's own Mac or Apple's recovery download.
    fn qi_start_mac_firmware(&self, _owner: &str, _source: &iyakki_shared::macfw::Source, _launch: &Launch) -> Result<Job, String> {
        Err("unavailable: this core does not install".into())
    }
    /// The GPU page's test prompt (a detached job; its log is the answer).
    fn qi_start_gpu_test(&self, _owner: &str, _launch: &Launch) -> Result<Job, String> {
        Err("unavailable: this core does not install".into())
    }
    /// The job of the hardware firmware (the kernel log names the files; the licence was accepted).
    fn qi_start_hardware_firmware(&self, _owner: &str, _opts: &HwFirmwareOpts, _launch: &Launch) -> Result<Job, String> {
        Err("unavailable: this core does not install".into())
    }
    /// The shared-folder tools over `inner` (the list is free, every change asks; the manager enforces the folder rule: an administrator
    /// any folder, others their home). `inner` itself in a core without the facet.
    fn share_tools(&self, inner: Arc<dyn Tools>, _admin: bool, _policy: Policy, _user: &str, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        inner
    }
    /// Shares marked "keep running" start again for this person (after sign-in).
    fn shares_restore(&self, _admin: bool, _policy: Policy, _user: &str, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) {}
    /// The assistant's disk format and resize tools over `inner` (plan free, the change asks and the DISKS tile confirms).
    fn disks_format_tools(&self, inner: Arc<dyn Tools>, _who: Actor, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        inner
    }
}

/// The "nothing here" implementation every default answers with.
pub struct NoB0;
impl ProvidersFacet for NoB0 {}
impl QuietFacet for NoB0 {}
impl TargetFacet for NoB0 {}
impl IdeBrowserFacet for NoB0 {}
impl OciPushFacet for NoB0 {}
impl LinuxAppsFacet for NoB0 {}
impl MediaFacet for NoB0 {}
impl GeminiFacet for NoB0 {}
impl McpFacet for NoB0 {}
impl CheckupFacet for NoB0 {}
impl QuickInstallFacet for NoB0 {}
impl B0Facets for NoB0 {}
impl B0Services for NoB0 {}

/// A quiet environment that does nothing.
pub struct NoQuietEnv;
impl QuietEnv for NoQuietEnv {
    fn amixer(&self, _args: &[String]) -> Result<(), String> {
        Err("unavailable: this core does not touch the sound card".into())
    }
    fn signal(&self, _pid: u32, _sig: i32) {}
    fn sleep(&self, _d: std::time::Duration) {}
    fn silence(&self, _max: std::time::Duration) {}
    fn own_pid(&self) -> u32 {
        0
    }
}

/// A registry connection that refuses every request.
pub struct NoRegistryHttp;
impl RegistryHttp for NoRegistryHttp {
    fn request(&self, _method: &str, _host: &str, _path: &str, _headers: &[(String, String)], _body: &[u8]) -> Result<iyakki_shared::ocipush::Reply, String> {
        Err("unavailable: this core does not reach a registry".into())
    }
}


/// A player that plays nothing.
pub struct NoMedia;
impl MediaPlay for NoMedia {}
