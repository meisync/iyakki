//! Hand-written facets of lane b2 (F-SPLIT-03, alpha.5 item 0a): the agent modules `profilestore`, `ntp`, `startupdisk`, `procowner`,
//! `keyboard` (the Apple driver files), `dictfix` and `webgraphics`, each as one narrow trait with default answers ("unavailable"). The real
//! implementation is `iyakki-agent/src/facets_b2.rs`; the fake is the defaults (`NoFacetsB2`), so a UI test never needs the agent.
//!
//! A call that needs the agent's `Services` takes `&dyn Any` and the caller passes `&cx.services` (an `&Arc<Services>`): the agent
//! downcasts it, the UI never names the type, and the fake ignores it.

use std::any::Any;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_core_api::sysinfo::{DiskUse, NetUse, Proc, SysRoots};
use iyakki_shared::dictfix::{Context, Fixed, Target};
use iyakki_shared::keyboard::{AppleState, KeyboardSettings};
use iyakki_shared::ntp::Status as NtpStatus;
use iyakki_shared::profilestore::Status as ProfileStatus;
use iyakki_shared::webgraphics::{Mode as GraphicsMode, Pending as GraphicsPending, Status as GraphicsStatus};

const UNAVAILABLE: &str = "unavailable: this core does not serve this";

/// The protection of the person's browser data (cookies, sign-ins, site storage) with a key in the vault.
pub trait ProfileStoreFacet: Send + Sync {
    /// The state for `user` (`svc` is the agent's services handle).
    fn status_of(&self, _svc: &dyn Any, _user: &str) -> ProfileStatus {
        ProfileStatus::Off
    }
    /// "Protect my browser data": make or reuse the key, seal, prove it reads back.
    fn protect_for(&self, _svc: &dyn Any, _user: &str) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    fn unprotect_for(&self, _svc: &dyn Any, _user: &str) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    /// Delete the browser data and the key.
    fn reset_for(&self, _svc: &dyn Any, _user: &str) -> String {
        UNAVAILABLE.into()
    }
    /// The state read from the data folder.
    fn status(&self, _data: &Path, _user: &str) -> ProfileStatus {
        ProfileStatus::Off
    }
    fn is_protected(&self, _data: &Path, _user: &str) -> bool {
        false
    }
    /// Copy the sealed data to a new file; the number of bytes.
    fn export_sealed(&self, _data: &Path, _user: &str, _to: &Path, _now: u64) -> Result<u64, String> {
        Err(UNAVAILABLE.into())
    }
    /// Take in a sealed file made elsewhere; `key_hex` is the person's browser key as it is kept in the vault.
    fn import_sealed(&self, _data: &Path, _user: &str, _from: &Path, _key_hex: &str, _now: u64) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    /// Sign-out: write the data back and wipe the decrypted copy. `Ok(true)` when something was sealed.
    fn on_sign_out(&self, _data: &Path, _user: &str) -> Result<bool, String> {
        Ok(false)
    }
    /// The lock screen was left: open the data again if the lock sealed it (`key_hex` is the vault's key when it has one).
    fn on_screen_unlock(&self, _data: &Path, _user: &str, _key_hex: Option<&str>) -> Result<bool, String> {
        Ok(false)
    }
}

/// The clock from the network (SNTP).
pub trait NtpFacet: Send + Sync {
    /// What the last attempt did.
    fn status(&self) -> NtpStatus {
        NtpStatus::default()
    }
    /// Set the system clock to `unix` seconds (root only).
    fn set_clock(&self, _unix: f64) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    /// One round of asking the servers; `apply` sets the clock.
    fn sync_once(&self, _servers: &[String], _force_back: bool, _timeout: Duration, _apply: &dyn Fn(f64) -> Result<(), String>) -> NtpStatus {
        NtpStatus { message: UNAVAILABLE.into(), ..NtpStatus::default() }
    }
    /// The status text of the clock bar goes back to "never asked" (a test's tool).
    fn reset_status(&self) {}
}

/// Which disk the firmware starts from (Settings -> Computer): every call writes the firmware's boot order or only reads it.
pub trait StartupDiskFacet: Send + Sync {
    fn register(&self, _backup: &Path) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    fn first(&self, _backup: &Path) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    fn next(&self, _backup: &Path) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    fn revert(&self, _backup: &Path) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    /// One line about the boot order and the "restart into IYAKKI" setting.
    fn status_line(&self, _backup: &Path, _reboot_setting: bool) -> String {
        String::new()
    }
}

/// A process with its owner and its network and disk use (the System tile's "My processes").
#[derive(Clone, Debug)]
pub struct OwnedProc {
    pub proc: Proc,
    pub owner: String,
    /// What the claimed program at the top of it was started as.
    pub what: String,
    pub net: NetUse,
    pub disk: DiskUse,
}

/// Whose process is it: the programs started for a person.
pub trait ProcOwnerFacet: Send + Sync {
    /// The processes of `who` (everyone's claimed ones for an administrator).
    fn mine(&self, _roots: &SysRoots, _who: &str, _admin: bool) -> Vec<OwnedProc> {
        Vec::new()
    }
    /// Ask one of them to stop (`hard` kills); the sentence to show.
    fn stop(&self, _roots: &SysRoots, _who: &str, _admin: bool, _pid: u32, _hard: bool) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    /// Run `f` with the programs it starts claimed for `user`.
    fn as_user(&self, _user: &str, f: &mut dyn FnMut()) {
        f()
    }
}

/// The Apple keyboard driver files (`hid-apple`, `applespi`, the Touch Bar): what the kernel has now and writing the person's choice.
pub trait KeyboardAppleFacet: Send + Sync {
    fn apple_state(&self, _sys: &Path) -> AppleState {
        AppleState::default()
    }
    /// Write the chosen values where the kernel's files exist; what was changed.
    fn apply_apple(&self, _sys: &Path, _k: &KeyboardSettings) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    /// The mode the shell asks for while it draws the Touch Bar strip (None leaves the chosen one).
    fn set_touchbar_override(&self, _mode: Option<u32>) {}
    /// The strip stopped: put the firmware's keys back; the number of files written.
    fn restore_touchbar_mode(&self, _sys: &Path, _k: &KeyboardSettings) -> usize {
        0
    }
}

/// Correcting dictated wording with a model.
pub trait DictFixFacet: Send + Sync {
    /// The model to use and whether it may be used now.
    fn target(&self, _svc: &dyn Any) -> Target {
        Target::NoModel
    }
    /// Correct `raw`; blocks, so call it from a worker thread.
    fn fix_wording(&self, _svc: &dyn Any, raw: &str, _ctx: &Context) -> Fixed {
        Fixed::kept(raw, iyakki_shared::dictfix::Reason::NoModel, "unavailable: this core does not correct wording")
    }
    fn set_enabled(&self, _svc: &dyn Any, _on: bool) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}

/// How the browser engine draws (hardware or software).
pub trait WebGraphicsFacet: Send + Sync {
    fn status(&self, _data: &Path) -> GraphicsStatus {
        GraphicsStatus { mode: GraphicsMode::Automatic, software: false, gpu: String::new(), line: String::new(), detail: String::new(), fallback: false, offer: false, read_only: None }
    }
    fn load_mode(&self, _data: &Path) -> (GraphicsMode, Option<String>) {
        (GraphicsMode::Automatic, None)
    }
    fn set_mode(&self, _data: &Path, _mode: GraphicsMode) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    fn try_hardware_again(&self, _data: &Path, _gpu: &str) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    /// The sentences the last decision queued for the shell (toast, status line, audit).
    fn take_pending(&self) -> Vec<GraphicsPending> {
        Vec::new()
    }
}

/// The choice of Lemonade's compute (CPU or GPU) kept by the GPU steps.
pub trait LemonadeComputeFacet: Send + Sync {
    fn lemonade_compute(&self, _data: &Path) -> iyakki_shared::gpu_steps::Compute {
        iyakki_shared::gpu_steps::Compute::Gpu
    }
}

/// Wrapping the chat's tool list with a tool family whose backend needs the agent's services.
pub trait ToolWrapFacet: Send + Sync {
    /// The tools of "make a Linux app image from a root folder" around `inner` (they ask every time). The default leaves `inner` as it is.
    /// `svc` is the services handle (`cx.services.clone()`).
    fn linux_appimage_tools(&self, inner: Arc<dyn Tools>, _svc: Arc<dyn Any + Send + Sync>, _policy: Policy, _user: String, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        inner
    }
}

/// The parts of the cluster registry that need the machine: switching also writes the old remembered choice, the GPU badges read the AI
/// hardware cache. The registry itself (`iyakki_shared::clusters::Registry`) is plain data.
pub trait ClusterFacet: Send + Sync {
    /// Role-checked switch of the account's active cluster, mirrored into the old remembered-choice file.
    fn switch(&self, _dir: &Path, _data_dir: &Path, _user: &str, _role: iyakki_sec::policy::Role, _id: &str) -> Result<iyakki_shared::clusters::ClusterEntry, String> {
        Err(UNAVAILABLE.into())
    }
    /// The GPU summary of a cluster for its badge (reads a cache).
    fn gpu_summary(&self, _cluster_id: &str) -> Option<String> {
        None
    }
    /// Ask the clusters for their GPUs (on a thread when `background`); the number of clusters asked.
    fn refresh_gpus(&self, _run: &Arc<dyn iyakki_shared::wsexec::Runner>, _entries: &[iyakki_shared::clusters::ClusterEntry], _background: bool) -> usize {
        0
    }
}

/// The GPU page of the IYAKKI menu: what is read at once, and the slow part (kernel log, pods) started on a thread.
pub trait GpuPageFacet: Send + Sync {
    /// The facts that come from files and the last install markers.
    fn quick_report(&self, _root: &Path, _data: &Path) -> iyakki_shared::gpu::Report {
        iyakki_shared::gpu::Report::default()
    }
    /// Read the kernel log and ask the managed cluster for its pods on a thread; `iyakki_shared::gpu_steps::take_background` returns the answer once.
    /// `svc` is the services handle (`cx.services.clone()`).
    fn refresh_async(&self, _svc: Arc<dyn Any + Send + Sync>) {}
}

struct NoFacetsB2;
impl ProfileStoreFacet for NoFacetsB2 {}
impl NtpFacet for NoFacetsB2 {}
impl StartupDiskFacet for NoFacetsB2 {}
impl ProcOwnerFacet for NoFacetsB2 {}
impl KeyboardAppleFacet for NoFacetsB2 {}
impl DictFixFacet for NoFacetsB2 {}
impl WebGraphicsFacet for NoFacetsB2 {}
impl LemonadeComputeFacet for NoFacetsB2 {}
impl ToolWrapFacet for NoFacetsB2 {}
impl GpuPageFacet for NoFacetsB2 {}
impl ClusterFacet for NoFacetsB2 {}

/// The facets of lane b2, one accessor each (a supertrait of `AgentFacets`, so `cx.agent.profile_store()` needs nothing more).
pub trait AgentFacetsB2: Send + Sync {
    fn profile_store(&self) -> &dyn ProfileStoreFacet {
        &NoFacetsB2
    }
    fn ntp(&self) -> &dyn NtpFacet {
        &NoFacetsB2
    }
    fn startup_disk(&self) -> &dyn StartupDiskFacet {
        &NoFacetsB2
    }
    fn proc_owner(&self) -> &dyn ProcOwnerFacet {
        &NoFacetsB2
    }
    fn keyboard_apple(&self) -> &dyn KeyboardAppleFacet {
        &NoFacetsB2
    }
    fn dict_fix(&self) -> &dyn DictFixFacet {
        &NoFacetsB2
    }
    fn web_graphics(&self) -> &dyn WebGraphicsFacet {
        &NoFacetsB2
    }
    fn lemonade_compute(&self) -> &dyn LemonadeComputeFacet {
        &NoFacetsB2
    }
    fn tool_wraps(&self) -> &dyn ToolWrapFacet {
        &NoFacetsB2
    }
    fn cluster_registry(&self) -> &dyn ClusterFacet {
        &NoFacetsB2
    }
    fn gpu_page(&self) -> &dyn GpuPageFacet {
        &NoFacetsB2
    }
}

impl AgentFacetsB2 for crate::FakeAgentApi {}

/// A recording fake for tests that need to see what the UI asked: the answers are the defaults and every call is noted by name.
#[derive(Default)]
pub struct RecordingFacetsB2 {
    pub calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingFacetsB2 {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn note(&self, what: &str) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(what.to_string());
    }
}

impl ProfileStoreFacet for RecordingFacetsB2 {
    fn status(&self, _data: &Path, user: &str) -> ProfileStatus {
        self.note(&format!("profile_store.status {user}"));
        ProfileStatus::Off
    }
}

impl NtpFacet for RecordingFacetsB2 {}
impl ProcOwnerFacet for RecordingFacetsB2 {}
impl KeyboardAppleFacet for RecordingFacetsB2 {}
impl DictFixFacet for RecordingFacetsB2 {}
impl LemonadeComputeFacet for RecordingFacetsB2 {}
impl ToolWrapFacet for RecordingFacetsB2 {}
impl ClusterFacet for RecordingFacetsB2 {}
impl GpuPageFacet for RecordingFacetsB2 {}

impl StartupDiskFacet for RecordingFacetsB2 {
    fn register(&self, _backup: &Path) -> Result<String, String> {
        self.note("startup_disk.register");
        Ok("registered".into())
    }
}

impl WebGraphicsFacet for RecordingFacetsB2 {
    fn set_mode(&self, _data: &Path, mode: GraphicsMode) -> Result<(), String> {
        self.note(&format!("web_graphics.set_mode {}", mode.id()));
        Ok(())
    }
}
