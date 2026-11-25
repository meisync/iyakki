//! The permission layer between an app and the machine (docs/development/CORE-ACCESS-MODEL.md section 6c; F-ARCH-05/06, F-CORE-12/13).
//!
//! The owner's rule: the app layer does not talk to core Linux or to devices directly, it asks permission. A [`GuardedOs`] wraps an [`Os`]
//! for one app identity and puts every call through the [`Broker`] first, so the capability catalogue decides and the access log records it:
//!
//! * a **read** (figures, lists, states) needs `system.read`, which every built-in app holds;
//! * a **change** needs its own capability: `system.radio.set`, `system.brightness.set`, `system.volume.set`, `system.memory.set`
//!   (the last is a hard ask: a per-use card with Allow, whatever was granted), held only by the apps the built-in table names.
//!
//! A refused read answers with the empty figure (nothing, as if the machine had none); a refused change answers with the denial in words
//! (`denied (not_granted): ...`). Nothing a refused call asked for reaches the inner `Os`. A tile is handed the guarded handle only, so it
//! has no path to the raw one (the dependency ratchet in `iyakki-core` fails if an app names `iyakki_core_local` outside its list).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::{Audio, PlaybackVolume, SoundCard};
use crate::broker::{Broker, Call, Caller, CardAnswer, CardAsk, Denial};
use crate::devices::{BlockDev, Disks, Mount, Net, NetCounters};
use crate::error::{ApiError, ApiResult};
use crate::hardware::{btstate, mac, Card, Hardware, Link, Rfkill, Screen, Volume};
use crate::memory::{config::Config, DataPlace, Memory, MemStats, Step};
use crate::sysinfo::{LoadAvg, MemInfo, Os, Sysinfo, Uptime};

pub const READ: &str = "system.read";
pub const RADIO_SET: &str = "system.radio.set";
pub const BRIGHTNESS_SET: &str = "system.brightness.set";
pub const VOLUME_SET: &str = "system.volume.set";
pub const LINK_SET: &str = "system.link.set";
pub const MEMORY_SET: &str = "system.memory.set";
pub const DISK_INSTALL: &str = "system.disk.install";
pub const DISK_MOUNT: &str = "system.disk.mount";
pub const DISK_PARTITION: &str = "system.disk.partition";
pub const DISK_UNLOCK: &str = "system.disk.unlock";
pub const HOSTNAME_SET: &str = "system.hostname.set";
pub const BLUETOOTH_USE: &str = "bluetooth.use";
pub const STARTUP_SET: &str = "system.startup.set";
pub const CRASHLOG_WRITE: &str = "system.crashlog.write";
pub const NET_SCAN: &str = "net.scan";
pub const ROUTER_USE: &str = "router.use";
pub const ROUTER_STRATEGY_SET: &str = "router.strategy.set";
pub const K8S_PORTFORWARD: &str = "k8s.portforward";
pub const FLEET_READ: &str = "fleet.read";
pub const FLEET_JOIN: &str = "fleet.join";
pub const FLEET_MANAGE: &str = "fleet.manage";
pub const FLEET_ASSIGN: &str = "fleet.assign";
pub const FLEET_RECEIVE: &str = "fleet.receive";

/// One app's door to the broker.
struct Gate {
    broker: Arc<Broker>,
    caller: Caller,
}

impl Gate {
    fn check(&self, capability: &str, label: &str) -> Result<(), Denial> {
        self.broker.decide(&Call { caller: &self.caller, capability, unit: None, label })
    }

    fn note(&self, capability: &str, text: &str) -> bool {
        self.broker.note(&Call { caller: &self.caller, capability, unit: None, label: text }, text)
    }
}

thread_local! {
    /// The card whose dispatch this thread is inside (its address), 0 when none; and how deep.
    static IN_DISPATCH: std::cell::Cell<(usize, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// The per-use card of the shell, bound to the event being handled. The person's own press (a key, a click) is the answer to the card, but
/// only for the work that press starts: the shell wraps the handling of each key or click in [`GestureCard::dispatch`], and a hard ask
/// is allowed only for a call made **inside that dispatch** on the same thread. A call after the dispatch returned, from another thread, or
/// when nobody pressed anything is refused with `no_card`; an earlier press never counts. Work that must continue on a thread the press
/// started (an Apply that runs as a job) takes a [`GestureTicket`] inside the dispatch and runs its work through [`GestureTicket::redeem`]:
/// a ticket is single-use and good for `ttl` (the shell uses one second) after it was issued.
pub struct GestureCard {
    ttl: Duration,
}

/// One press, handed to the thread that does the work it started. Single-use.
pub struct GestureTicket {
    card: usize,
    issued: Instant,
    ttl: Duration,
}

impl GestureTicket {
    /// Run `work` as the change the press asked for: hard asks inside it are allowed. Spent after the first call; an old or already spent
    /// ticket runs `work` without the gesture (its hard asks are then refused).
    pub fn redeem<R>(self, work: impl FnOnce() -> R) -> R {
        if self.issued.elapsed() > self.ttl {
            return work();
        }
        let _scope = Scope::enter(self.card);
        work()
    }
}

struct Scope(usize, u32);

impl Scope {
    fn enter(card: usize) -> Scope {
        let before = IN_DISPATCH.with(|c| c.get());
        IN_DISPATCH.with(|c| c.set((card, before.1 + 1)));
        Scope(before.0, before.1)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        IN_DISPATCH.with(|c| c.set((self.0, self.1)));
    }
}

/// The ticket of the dispatch this thread is in, if it is in one (None otherwise: nobody pressed). Call it where the press is handled,
/// before handing the work to a thread.
pub fn gesture_ticket() -> Option<GestureTicket> {
    let (card, depth) = IN_DISPATCH.with(|c| c.get());
    (depth > 0 && card != 0).then(|| GestureTicket { card, issued: Instant::now(), ttl: TICKET_TTL.with(|t| t.get()) })
}

thread_local! {
    static TICKET_TTL: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::from_secs(1)) };
}

impl GestureCard {
    /// `ttl`: how long a ticket issued in a dispatch stays good for a job thread.
    pub fn new(ttl: Duration) -> GestureCard {
        GestureCard { ttl }
    }

    fn id(&self) -> usize {
        self as *const GestureCard as usize
    }

    /// Handle one key or click: calls made inside `handle` (on this thread) may answer a hard-ask card with the person's press.
    pub fn dispatch<R>(&self, handle: impl FnOnce() -> R) -> R {
        let old = TICKET_TTL.with(|t| t.replace(self.ttl));
        let _scope = Scope::enter(self.id());
        let r = handle();
        TICKET_TTL.with(|t| t.set(old));
        r
    }
}

impl CardAsk for GestureCard {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        let (card, depth) = IN_DISPATCH.with(|c| c.get());
        if depth > 0 && card == self.id() {
            CardAnswer::Allow
        } else {
            CardAnswer::Unavailable
        }
    }
}

/// An [`Os`] for one app identity: every call is checked first.
pub struct GuardedOs {
    inner: Arc<dyn Os>,
    gate: Arc<Gate>,
    sysinfo: GuardedSysinfo,
    audio: GuardedAudio,
    disks: GuardedDisks,
    net: GuardedNet,
}

impl GuardedOs {
    pub fn new(inner: Arc<dyn Os>, broker: Arc<Broker>, caller: Caller) -> GuardedOs {
        let gate = Arc::new(Gate { broker, caller });
        GuardedOs {
            sysinfo: GuardedSysinfo { gate: gate.clone(), os: inner.clone() },
            audio: GuardedAudio { gate: gate.clone(), os: inner.clone() },
            disks: GuardedDisks { gate: gate.clone(), os: inner.clone() },
            net: GuardedNet { gate: gate.clone(), os: inner.clone() },
            inner,
            gate,
        }
    }
}

fn denied<T>(d: Denial) -> ApiResult<T> {
    Err(ApiError::Denied(d.to_string()))
}

struct GuardedSysinfo {
    gate: Arc<Gate>,
    os: Arc<dyn Os>,
}

impl Sysinfo for GuardedSysinfo {
    fn cmdline(&self) -> ApiResult<String> {
        match self.gate.check(READ, "kernel command line") {
            Ok(()) => self.os.sysinfo().cmdline(),
            Err(d) => denied(d),
        }
    }
    fn meminfo(&self) -> ApiResult<MemInfo> {
        match self.gate.check(READ, "memory figures") {
            Ok(()) => self.os.sysinfo().meminfo(),
            Err(d) => denied(d),
        }
    }
    fn uptime(&self) -> ApiResult<Uptime> {
        match self.gate.check(READ, "uptime") {
            Ok(()) => self.os.sysinfo().uptime(),
            Err(d) => denied(d),
        }
    }
    fn loadavg(&self) -> ApiResult<LoadAvg> {
        match self.gate.check(READ, "load") {
            Ok(()) => self.os.sysinfo().loadavg(),
            Err(d) => denied(d),
        }
    }
    fn product_name(&self) -> ApiResult<String> {
        match self.gate.check(READ, "product name") {
            Ok(()) => self.os.sysinfo().product_name(),
            Err(d) => denied(d),
        }
    }
    fn cpu_count(&self) -> ApiResult<u32> {
        match self.gate.check(READ, "processor count") {
            Ok(()) => self.os.sysinfo().cpu_count(),
            Err(d) => denied(d),
        }
    }
}

struct GuardedAudio {
    gate: Arc<Gate>,
    os: Arc<dyn Os>,
}

impl Audio for GuardedAudio {
    fn sound_cards(&self) -> ApiResult<Vec<SoundCard>> {
        match self.gate.check(READ, "sound cards") {
            Ok(()) => self.os.audio().sound_cards(),
            Err(d) => denied(d),
        }
    }
    fn volume(&self, card: u32) -> ApiResult<PlaybackVolume> {
        match self.gate.check(READ, "volume") {
            Ok(()) => self.os.audio().volume(card),
            Err(d) => denied(d),
        }
    }
    fn set_volume(&self, card: u32, percent: u32) -> ApiResult<PlaybackVolume> {
        match self.gate.check(VOLUME_SET, "set the volume") {
            Ok(()) => self.os.audio().set_volume(card, percent),
            Err(d) => denied(d),
        }
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> ApiResult<PlaybackVolume> {
        match self.gate.check(VOLUME_SET, "change the volume") {
            Ok(()) => self.os.audio().change_volume(card, delta),
            Err(d) => denied(d),
        }
    }
}

struct GuardedDisks {
    gate: Arc<Gate>,
    os: Arc<dyn Os>,
}

impl Disks for GuardedDisks {
    fn block_devices(&self) -> ApiResult<Vec<BlockDev>> {
        match self.gate.check(READ, "disks") {
            Ok(()) => self.os.disks().block_devices(),
            Err(d) => denied(d),
        }
    }
    fn reword_failure(&self, msg: &str) -> String {
        match self.gate.check(READ, "drive state") {
            Ok(()) => self.os.disks().reword_failure(msg),
            Err(_) => msg.to_string(),
        }
    }
    fn mounts(&self) -> ApiResult<Vec<Mount>> {
        match self.gate.check(READ, "mounts") {
            Ok(()) => self.os.disks().mounts(),
            Err(d) => denied(d),
        }
    }
    fn space(&self, path: &str) -> ApiResult<(u64, u64)> {
        match self.gate.check(READ, "free space") {
            Ok(()) => self.os.disks().space(path),
            Err(d) => denied(d),
        }
    }
}

struct GuardedNet {
    gate: Arc<Gate>,
    os: Arc<dyn Os>,
}

impl Net for GuardedNet {
    fn interfaces(&self) -> ApiResult<Vec<NetCounters>> {
        match self.gate.check(READ, "network counters") {
            Ok(()) => self.os.net().interfaces(),
            Err(d) => denied(d),
        }
    }
    fn ipv4_addresses(&self) -> ApiResult<Vec<(String, String)>> {
        match self.gate.check(READ, "addresses") {
            Ok(()) => self.os.net().ipv4_addresses(),
            Err(d) => denied(d),
        }
    }
}

struct GuardedAudioRoute {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::audioroute::AudioRoute>,
}

impl GuardedAudioRoute {
    fn ok(&self, what: &str) -> bool {
        self.gate.check(READ, what).is_ok()
    }
}

impl crate::audioroute::AudioRoute for GuardedAudioRoute {
    fn asound_text(&self, rel: &str) -> Option<String> {
        self.ok("sound card facts").then(|| self.inner.asound_text(rel)).flatten()
    }
    fn asound_dir(&self, rel: &str) -> Vec<String> {
        if self.ok("sound card facts") { self.inner.asound_dir(rel) } else { Vec::new() }
    }
    fn asound_has(&self, rel: &str) -> bool {
        self.ok("sound card facts") && self.inner.asound_has(rel)
    }
    fn product_name(&self) -> Option<String> {
        self.ok("machine model").then(|| self.inner.product_name()).flatten()
    }
    fn pcm_holders(&self, card: u32, device: u32) -> Vec<crate::audioroute::PcmHolder> {
        if self.ok("who plays on a sound device") { self.inner.pcm_holders(card, device) } else { Vec::new() }
    }
    fn is_machine(&self) -> bool {
        self.inner.is_machine()
    }
    fn process_parent(&self, pid: u32) -> Option<u32> {
        self.ok("who runs a program").then(|| self.inner.process_parent(pid)).flatten()
    }
    fn process_name(&self, pid: u32) -> Option<String> {
        self.ok("who runs a program").then(|| self.inner.process_name(pid)).flatten()
    }
    fn mixer_contents(&self, card: u32) -> Option<String> {
        self.ok("mixer controls").then(|| self.inner.mixer_contents(card)).flatten()
    }
}

impl Os for GuardedOs {
    fn sysinfo(&self) -> &dyn Sysinfo {
        &self.sysinfo
    }
    fn audio(&self) -> &dyn Audio {
        &self.audio
    }
    fn disks(&self) -> &dyn Disks {
        &self.disks
    }
    fn net(&self) -> &dyn Net {
        &self.net
    }
    fn memory(&self) -> Arc<dyn Memory> {
        Arc::new(GuardedMemory { gate: self.gate.clone(), inner: self.inner.memory() })
    }
    fn hardware(&self) -> Arc<dyn Hardware> {
        Arc::new(GuardedHardware { gate: self.gate.clone(), inner: self.inner.hardware() })
    }
    fn installer(&self) -> Arc<dyn crate::installer::Installer> {
        Arc::new(GuardedInstaller { gate: self.gate.clone(), inner: self.inner.installer() })
    }
    fn audio_route(&self) -> Arc<dyn crate::audioroute::AudioRoute> {
        Arc::new(GuardedAudioRoute { gate: self.gate.clone(), inner: self.inner.audio_route() })
    }
    fn drivers(&self) -> Arc<dyn crate::drivers::Drivers> {
        Arc::new(GuardedDrivers { gate: self.gate.clone(), inner: self.inner.drivers() })
    }
    fn disk_engine(&self) -> Arc<dyn crate::diskmodel::DiskEngine> {
        Arc::new(GuardedDiskEngine { gate: self.gate.clone(), inner: self.inner.disk_engine() })
    }
    fn screen_facts(&self) -> Arc<dyn crate::screenfacts::ScreenFacts> {
        Arc::new(GuardedScreenFacts { gate: self.gate.clone(), inner: self.inner.screen_facts() })
    }
    fn output_state(&self) -> Arc<dyn crate::soundctl::OutputState> {
        Arc::new(GuardedOutputState { gate: self.gate.clone(), inner: self.inner.output_state() })
    }
    fn bt_audio(&self) -> Arc<dyn crate::btaudio::BtAudio> {
        Arc::new(GuardedBtAudio { gate: self.gate.clone(), inner: self.inner.bt_audio() })
    }
    fn startup_disk(&self) -> Arc<dyn crate::startupdisk::StartupDisk> {
        Arc::new(GuardedStartupDisk { gate: self.gate.clone(), inner: self.inner.startup_disk() })
    }
    fn crashlog(&self) -> Arc<dyn crate::crashlog::CrashLog> {
        Arc::new(GuardedCrashLog { gate: self.gate.clone(), inner: self.inner.crashlog() })
    }
    fn netscan(&self) -> Arc<dyn crate::netscan::NetScan> {
        Arc::new(GuardedNetScan { gate: self.gate.clone(), inner: self.inner.netscan() })
    }
    fn router(&self) -> Arc<dyn crate::router::Router> {
        Arc::new(GuardedRouter { gate: self.gate.clone(), inner: self.inner.router() })
    }
    fn fleet(&self) -> Arc<dyn crate::fleet::Fleet> {
        Arc::new(GuardedFleet { gate: self.gate.clone(), inner: self.inner.fleet() })
    }
    fn port_forwards(&self) -> Arc<dyn crate::portforward::PortForwards> {
        Arc::new(GuardedPortForwards { gate: self.gate.clone(), inner: self.inner.port_forwards() })
    }
}

struct GuardedPortForwards {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::portforward::PortForwards>,
}

impl crate::portforward::PortForwards for GuardedPortForwards {
    fn start(&self, req: &crate::portforward::Request) -> Result<crate::portforward::Info, String> {
        let label = format!("forward {}/{}:{} in {}", req.kind.prefix(), req.name, req.remote, req.namespace);
        self.gate.check(K8S_PORTFORWARD, &label).map_err(|d| d.to_string())?;
        self.inner.start(req)
    }
    fn list(&self) -> Vec<crate::portforward::Info> {
        match self.gate.check(READ, "the running port-forwards") {
            Ok(()) => self.inner.list(),
            Err(_) => Vec::new(),
        }
    }
    fn stop(&self, id: u32) -> bool {
        self.gate.check(READ, "stop a port-forward").is_ok() && self.inner.stop(id)
    }
    fn stop_all(&self) -> usize {
        match self.gate.check(READ, "stop the port-forwards") {
            Ok(()) => self.inner.stop_all(),
            Err(_) => 0,
        }
    }
}

struct GuardedFleet {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::fleet::Fleet>,
}

impl crate::fleet::Fleet for GuardedFleet {
    fn read(&self, label: &str) -> Result<(), String> {
        self.gate.check(FLEET_READ, label).map_err(|d| d.to_string())?;
        self.inner.read(label)
    }
    fn join(&self, label: &str) -> Result<(), String> {
        self.gate.check(FLEET_JOIN, label).map_err(|d| d.to_string())?;
        self.inner.join(label)
    }
    fn manage(&self, label: &str) -> Result<(), String> {
        self.gate.check(FLEET_MANAGE, label).map_err(|d| d.to_string())?;
        self.inner.manage(label)
    }
    fn assign(&self, label: &str) -> Result<(), String> {
        self.gate.check(FLEET_ASSIGN, label).map_err(|d| d.to_string())?;
        self.inner.assign(label)
    }
    fn receive(&self, label: &str) -> Result<(), String> {
        self.gate.check(FLEET_RECEIVE, label).map_err(|d| d.to_string())?;
        self.inner.receive(label)
    }
    fn record(&self, what: &str, detail: &str) {
        // only a caller that holds the capability writes lines
        if self.gate.note(FLEET_READ, &format!("{what} {detail}")) {
            self.inner.record(what, detail);
        }
    }
}

struct GuardedRouter {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::router::Router>,
}

impl crate::router::Router for GuardedRouter {
    fn begin_run(&self, label: &str) -> Result<(), String> {
        self.gate.check(ROUTER_USE, label).map_err(|d| d.to_string())?;
        self.inner.begin_run(label)
    }
    fn allow_listen(&self, label: &str) -> Result<(), String> {
        self.gate.check("microphone.listen", label).map_err(|d| d.to_string())?;
        self.inner.allow_listen(label)
    }
    fn ask_step_tool(&self, label: &str) -> Result<(), String> {
        self.gate.check(ROUTER_USE, label).map_err(|d| d.to_string())?;
        self.inner.ask_step_tool(label)
    }
    fn allow_strategy(&self, name: &str) -> Result<(), String> {
        self.gate.check(ROUTER_STRATEGY_SET, name).map_err(|d| d.to_string())?;
        self.inner.allow_strategy(name)
    }
    fn models(&self) -> Vec<crate::router::RouteModel> {
        match self.gate.check(READ, "the models the router may use") {
            Ok(()) => self.inner.models(),
            Err(_) => Vec::new(),
        }
    }
    fn route(&self, need: &crate::router::RouteNeed) -> crate::router::RouteDecision {
        // the decision is made here over the list the guard lets through, so a caller without the capability gets an empty answer
        crate::router::choose(&self.models(), need)
    }
    fn record(&self, what: &str, detail: &str) {
        // only a caller that holds the capability writes lines, and only after a run was let start
        if self.gate.note(ROUTER_USE, &format!("{what} {detail}")) {
            self.inner.record(what, detail);
        }
    }
}

struct GuardedNetScan {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::netscan::NetScan>,
}

impl crate::netscan::NetScan for GuardedNetScan {
    fn subnets(&self) -> Vec<crate::netscan::net::Subnet> {
        match self.gate.check(READ, "this machine's subnets") {
            Ok(()) => self.inner.subnets(),
            Err(_) => Vec::new(),
        }
    }
    fn start(&self, targets: Vec<std::net::Ipv4Addr>, opts: crate::netscan::Options, cancel: Arc<std::sync::atomic::AtomicBool>, tx: std::sync::mpsc::Sender<crate::netscan::Event>) -> Result<(), String> {
        self.gate.check(NET_SCAN, "scan the local network").map_err(|d| d.to_string())?;
        self.inner.start(targets, opts, cancel, tx)
    }
}

struct GuardedScreenFacts {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::screenfacts::ScreenFacts>,
}

impl crate::screenfacts::ScreenFacts for GuardedScreenFacts {
    fn connectors(&self) -> Vec<crate::screenfacts::Connector> {
        match self.gate.check(READ, "the display connectors") {
            Ok(()) => self.inner.connectors(),
            Err(_) => Vec::new(),
        }
    }
    fn signature(&self) -> String {
        match self.gate.check(READ, "the display signature") {
            Ok(()) => self.inner.signature(),
            Err(_) => String::new(),
        }
    }
    fn fb_signature(&self, fb_path: &str) -> Option<String> {
        self.gate.check(READ, "the firmware screen").ok()?;
        self.inner.fb_signature(fb_path)
    }
    fn card_with(&self, at_least: usize) -> Option<String> {
        self.gate.check(READ, "the graphics cards").ok()?;
        self.inner.card_with(at_least)
    }
}

struct GuardedOutputState {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::soundctl::OutputState>,
}

impl crate::soundctl::OutputState for GuardedOutputState {
    fn device(&self) -> Option<String> {
        self.gate.check(READ, "which sound output plays").ok()?;
        self.inner.device()
    }
    fn generation(&self) -> u64 {
        match self.gate.check(READ, "whether the sound output changed") {
            Ok(()) => self.inner.generation(),
            Err(_) => 0,
        }
    }
    fn gain(&self) -> u32 {
        // a refused read leaves the sound at full volume: it must never silence a player
        match self.gate.check(READ, "the sound volume") {
            Ok(()) => self.inner.gain(),
            Err(_) => 100,
        }
    }
}

struct GuardedBtAudio {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::btaudio::BtAudio>,
}

impl crate::btaudio::BtAudio for GuardedBtAudio {
    fn receive_view(&self) -> crate::btaudio::AudioView {
        match self.gate.check(READ, "the Bluetooth audio state") {
            Ok(()) => self.inner.receive_view(),
            Err(d) => crate::btaudio::AudioView { why_not: d.to_string(), ..Default::default() },
        }
    }
    fn send_view(&self) -> crate::btaudio::SendView {
        match self.gate.check(READ, "the Bluetooth sound sending state") {
            Ok(()) => self.inner.send_view(),
            Err(d) => crate::btaudio::SendView { why: d.to_string(), ..Default::default() },
        }
    }
    fn sinks(&self) -> Vec<(String, String)> {
        match self.gate.check(READ, "the paired headphones") {
            Ok(()) => self.inner.sinks(),
            Err(_) => Vec::new(),
        }
    }
    fn sound_line(&self) -> String {
        match self.gate.check(READ, "where the sound goes") {
            Ok(()) => self.inner.sound_line(),
            Err(d) => d.to_string(),
        }
    }
    fn remembered(&self) -> Option<(String, bool)> {
        self.gate.check(READ, "the remembered headphones").ok()?;
        self.inner.remembered()
    }
    fn plan(&self, action: &crate::btaudio::BtAudioAction) -> Result<String, String> {
        self.gate.check(BLUETOOTH_USE, &format!("plan a Bluetooth audio change ({})", action.tool())).map_err(|d| d.to_string())?;
        self.inner.plan(action)
    }
    fn apply(&self, action: &crate::btaudio::BtAudioAction) -> Result<String, String> {
        self.gate.check(BLUETOOTH_USE, &format!("change Bluetooth audio ({})", action.tool())).map_err(|d| d.to_string())?;
        self.inner.apply(action)
    }
    fn clear_remote(&self) {
        // only ever takes something away: the keys of a phone stop acting
        self.inner.clear_remote();
    }
}

struct GuardedStartupDisk {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::startupdisk::StartupDisk>,
}

impl crate::startupdisk::StartupDisk for GuardedStartupDisk {
    fn status_lines(&self, reboot_setting: bool) -> Vec<String> {
        match self.gate.check(READ, "the firmware's startup entries") {
            Ok(()) => self.inner.status_lines(reboot_setting),
            Err(d) => vec![d.to_string()],
        }
    }
    fn status_line(&self, reboot_setting: bool) -> String {
        match self.gate.check(READ, "the startup disk") {
            Ok(()) => self.inner.status_line(reboot_setting),
            Err(d) => d.to_string(),
        }
    }
    fn plan(&self, action: crate::startupdisk::StartupAction, administrator: bool) -> Result<crate::startupdisk::StartupPlan, String> {
        self.gate.check(STARTUP_SET, &format!("plan the startup disk change `{}`", action.name())).map_err(|d| d.to_string())?;
        self.inner.plan(action, administrator)
    }
    fn apply(&self, action: crate::startupdisk::StartupAction, administrator: bool) -> Result<String, String> {
        self.gate.check(STARTUP_SET, &format!("change the startup disk: {}", action.name())).map_err(|d| d.to_string())?;
        self.inner.apply(action, administrator)
    }
    fn before_dev_reboot(&self, setting_on: bool) -> Option<String> {
        self.gate.check(STARTUP_SET, "start IYAKKI at the next restart").ok()?;
        self.inner.before_dev_reboot(setting_on)
    }
}

struct GuardedCrashLog {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::crashlog::CrashLog>,
}

impl crate::crashlog::CrashLog for GuardedCrashLog {
    fn read_unclean(&self, dir: &std::path::Path) -> Option<crate::crashlog::Unclean> {
        self.gate.check(READ, "the note about the last crash").ok().and_then(|_| self.inner.read_unclean(dir))
    }
    fn latest_crash(&self, dir: &std::path::Path) -> Option<String> {
        self.gate.check(READ, "the kept kernel logs").ok().and_then(|_| self.inner.latest_crash(dir))
    }
    fn current_file(&self, dir: &std::path::Path) -> Option<String> {
        self.gate.check(READ, "the kept kernel logs").ok().and_then(|_| self.inner.current_file(dir))
    }
    fn has_file(&self, dir: &std::path::Path, name: &str) -> bool {
        self.gate.check(READ, "the kept kernel logs").is_ok() && self.inner.has_file(dir, name)
    }
    fn last_lines(&self, dir: &std::path::Path, name: &str, n: usize) -> Vec<String> {
        match self.gate.check(READ, "a kept kernel log") {
            Ok(()) => self.inner.last_lines(dir, name, n),
            Err(_) => Vec::new(),
        }
    }
    fn mark_seen(&self, dir: &std::path::Path) {
        if self.gate.check(CRASHLOG_WRITE, "mark the crash note as seen").is_ok() {
            self.inner.mark_seen(dir);
        }
    }
    fn copy_to_folder(&self, dir: &std::path::Path, name: &str, dest_dir: &std::path::Path, now_unix: i64) -> Result<std::path::PathBuf, String> {
        self.gate.check(CRASHLOG_WRITE, "copy a kernel log to a folder").map_err(|d| d.to_string())?;
        self.inner.copy_to_folder(dir, name, dest_dir, now_unix)
    }
}

struct GuardedInstaller {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::installer::Installer>,
}

impl crate::installer::Installer for GuardedInstaller {
    fn disks(&self) -> Vec<crate::installer::DiskInfo> {
        match self.gate.check(READ, "list the disks") {
            Ok(()) => self.inner.disks(),
            Err(_) => Vec::new(),
        }
    }
    fn source(&self, disks: &[crate::installer::DiskInfo]) -> Result<crate::installer::SourceInfo, String> {
        self.gate.check(READ, "find what to install from").map_err(|d| d.to_string())?;
        self.inner.source(disks)
    }
    fn uefi(&self) -> bool {
        self.gate.check(READ, "see how the computer started").is_ok() && self.inner.uefi()
    }
    fn execute(&self, plan: &crate::installer::Plan, progress: &mut dyn FnMut(crate::installer::Progress)) -> Result<crate::installer::InstallReport, String> {
        self.gate.check(DISK_INSTALL, &format!("erase {} and install IYAKKI on it", plan.target.drive.path)).map_err(|d| d.to_string())?;
        self.inner.execute(plan, progress)
    }
}

struct GuardedDiskEngine {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::diskmodel::DiskEngine>,
}

impl crate::diskmodel::DiskEngine for GuardedDiskEngine {
    fn scan(&self) -> crate::diskmodel::Snapshot {
        match self.gate.check(READ, "look at the drives") {
            Ok(()) => self.inner.scan(),
            Err(_) => Default::default(),
        }
    }
    fn fingerprint(&self) -> u64 {
        match self.gate.check(READ, "see whether a drive came or went") {
            Ok(()) => self.inner.fingerprint(),
            Err(_) => 0,
        }
    }
    fn forget(&self) {
        if self.gate.check(READ, "look at the drives again").is_ok() {
            self.inner.forget()
        }
    }
    fn users_of(&self, mountpoint: &str) -> Vec<(u32, String)> {
        match self.gate.check(READ, "see who uses a drive") {
            Ok(()) => self.inner.users_of(mountpoint),
            Err(_) => Vec::new(),
        }
    }
    fn mounted_in(&self, home: &str) -> Vec<(String, String)> {
        match self.gate.check(READ, "see which drives are open") {
            Ok(()) => self.inner.mounted_in(home),
            Err(_) => Vec::new(),
        }
    }
    fn plan_mount(&self, snap: &crate::diskmodel::Snapshot, who: &crate::diskmodel::Actor, part: &str, opts: &crate::diskmodel::MountOpts) -> Result<crate::diskmodel::MountPlan, String> {
        self.gate.check(READ, &format!("see what opening {part} would do")).map_err(|d| d.to_string())?;
        self.inner.plan_mount(snap, who, part, opts)
    }
    fn mount(&self, who: &crate::diskmodel::Actor, part: &str, opts: &crate::diskmodel::MountOpts, audit: crate::diskmodel::AuditFn) -> Result<crate::diskmodel::Mounted, String> {
        self.gate.check(DISK_MOUNT, &format!("open the drive {part}")).map_err(|d| d.to_string())?;
        self.inner.mount(who, part, opts, audit)
    }
    fn unmount(&self, who: &crate::diskmodel::Actor, part: &str, audit: crate::diskmodel::AuditFn) -> Result<String, String> {
        self.gate.check(DISK_MOUNT, &format!("close the drive {part}")).map_err(|d| d.to_string())?;
        self.inner.unmount(who, part, audit)
    }
    fn eject(&self, who: &crate::diskmodel::Actor, disk: &str, audit: crate::diskmodel::AuditFn) -> Result<String, String> {
        self.gate.check(DISK_MOUNT, &format!("eject the drive {disk}")).map_err(|d| d.to_string())?;
        self.inner.eject(who, disk, audit)
    }
    fn at_sign_out(&self, who: &crate::diskmodel::Actor, audit: crate::diskmodel::AuditFn) -> Option<String> {
        // closing the person's drives at sign-out is a change: without the capability nothing is closed and nothing is said
        self.gate.check(DISK_MOUNT, "close the drives of the person who signs out").ok()?;
        self.inner.at_sign_out(who, audit)
    }
    fn after_unplug(&self, who: &crate::diskmodel::Actor, audit: crate::diskmodel::AuditFn) -> Option<String> {
        // letting go of a mount of a drive that is gone is an unmount: without the capability nothing is touched and nothing is said
        self.gate.check(DISK_MOUNT, "close what a pulled-out drive left open").ok()?;
        self.inner.after_unplug(who, audit)
    }
    fn refusal(&self, who: &crate::diskmodel::Actor, d: &crate::diskmodel::Disk) -> Option<String> {
        match self.gate.check(READ, "see whether a drive may be changed") {
            Ok(()) => self.inner.refusal(who, d),
            Err(denied) => Some(denied.to_string()),
        }
    }
    fn plan_partition(&self, who: &crate::diskmodel::Actor, disk: &str, op: &crate::diskmodel::PartOp) -> Result<crate::diskmodel::Plan, String> {
        self.gate.check(READ, &format!("see what changing the drive {disk} would do")).map_err(|d| d.to_string())?;
        self.inner.plan_partition(who, disk, op)
    }
    fn apply_partition(&self, who: &crate::diskmodel::Actor, disk: &str, op: &crate::diskmodel::PartOp, confirm: &crate::diskmodel::Confirm, audit: crate::diskmodel::AuditFn) -> Result<String, String> {
        self.gate.check(DISK_PARTITION, &format!("change the partitions of the drive {disk} ({})", op.name())).map_err(|d| d.to_string())?;
        self.inner.apply_partition(who, disk, op, confirm, audit)
    }
    fn plan_unlock(&self, snap: &crate::diskmodel::Snapshot, who: &crate::diskmodel::Actor, part: &str) -> Result<iyakki_diskcrypt::CryptKind, String> {
        self.gate.check(READ, &format!("see whether {part} can be unlocked")).map_err(|d| d.to_string())?;
        self.inner.plan_unlock(snap, who, part)
    }
    fn unlock(&self, who: &crate::diskmodel::Actor, part: &str, secret: &iyakki_diskcrypt::Secret, audit: crate::diskmodel::AuditFn) -> Result<crate::diskmodel::Mounted, String> {
        self.gate.check(DISK_UNLOCK, &format!("unlock the drive {part}")).map_err(|d| d.to_string())?;
        self.inner.unlock(who, part, secret, audit)
    }
    fn allow_writing(&self, who: &crate::diskmodel::Actor, part: &str, audit: crate::diskmodel::AuditFn) -> Result<crate::diskmodel::Mounted, String> {
        self.gate.check(DISK_UNLOCK, &format!("open the unlocked drive {part} for writing")).map_err(|d| d.to_string())?;
        self.inner.allow_writing(who, part, audit)
    }
    fn lock(&self, who: &crate::diskmodel::Actor, part: &str, audit: crate::diskmodel::AuditFn) -> Result<String, String> {
        self.gate.check(DISK_UNLOCK, &format!("lock the drive {part}")).map_err(|d| d.to_string())?;
        self.inner.lock(who, part, audit)
    }
    fn take_format_request(&self, dir: &Path, user: &str) -> Option<crate::diskmodel::FormatRequest> {
        self.gate.check(READ, "the assistant's request to format or resize").ok()?;
        self.inner.take_format_request(dir, user)
    }
    fn use_programs(&self, bin_dir: &str) {
        if self.gate.check(READ, "tell the drive engine where the installed programs are").is_ok() {
            self.inner.use_programs(bin_dir)
        }
    }
}

struct GuardedDrivers {
    gate: Arc<Gate>,
    inner: Arc<dyn crate::drivers::Drivers>,
}

impl crate::drivers::Drivers for GuardedDrivers {
    fn machine(&self) -> crate::drivers::Machine {
        match self.gate.check(READ, "see what this machine is") {
            Ok(()) => self.inner.machine(),
            Err(_) => Default::default(),
        }
    }
    fn rows(&self) -> Vec<crate::drivers::Row> {
        match self.gate.check(READ, "list the drivers of this machine") {
            Ok(()) => self.inner.rows(),
            Err(_) => Vec::new(),
        }
    }
    fn installed(&self) -> Vec<crate::drivers::Record> {
        match self.gate.check(READ, "list the installed driver packs") {
            Ok(()) => self.inner.installed(),
            Err(_) => Vec::new(),
        }
    }
    fn is_disabled(&self, id: &str) -> bool {
        self.gate.check(READ, "see whether a driver pack is off").is_ok() && self.inner.is_disabled(id)
    }
}

struct GuardedMemory {
    gate: Arc<Gate>,
    inner: Arc<dyn Memory>,
}

fn refused_steps(d: Denial) -> Vec<Step> {
    vec![Step { ok: false, msg: d.to_string() }]
}

impl Memory for GuardedMemory {
    fn stats(&self) -> MemStats {
        match self.gate.check(READ, "memory statistics") {
            Ok(()) => self.inner.stats(),
            Err(_) => MemStats::default(),
        }
    }
    fn conf_path(&self) -> PathBuf {
        self.inner.conf_path()
    }
    fn read_conf(&self) -> String {
        match self.gate.check(READ, "memory settings") {
            Ok(()) => self.inner.read_conf(),
            Err(_) => String::new(),
        }
    }
    fn save_conf(&self, cfg: &Config) -> Result<(), String> {
        self.gate.check(MEMORY_SET, "save the memory settings").map_err(|d| d.to_string())?;
        self.inner.save_conf(cfg)
    }
    fn set_conf_key(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        self.gate.check(MEMORY_SET, "change a memory setting").map_err(|d| d.to_string())?;
        self.inner.set_conf_key(key, value)
    }
    fn reconfigure(&self, cfg: &Config, ram: u64) -> Result<Vec<Step>, String> {
        self.gate.check(MEMORY_SET, "reconfigure compressed memory and swap").map_err(|d| d.to_string())?;
        self.inner.reconfigure(cfg, ram)
    }
    fn apply_swapfile(&self, bytes: Option<u64>, on_disk: bool) -> Vec<Step> {
        match self.gate.check(MEMORY_SET, "change the swap file") {
            Ok(()) => self.inner.apply_swapfile(bytes, on_disk),
            Err(d) => refused_steps(d),
        }
    }
    fn data_place(&self, dir: &Path) -> DataPlace {
        match self.gate.check(READ, "where the data lives") {
            Ok(()) => self.inner.data_place(dir),
            Err(_) => DataPlace::Other,
        }
    }
    fn data_free(&self, dir: &Path) -> Option<u64> {
        self.gate.check(READ, "free space").ok()?;
        self.inner.data_free(dir)
    }
}

struct GuardedHardware {
    gate: Arc<Gate>,
    inner: Arc<dyn Hardware>,
}

impl Hardware for GuardedHardware {
    fn links(&self) -> Vec<Link> {
        match self.gate.check(READ, "network links") {
            Ok(()) => self.inner.links(),
            Err(_) => Vec::new(),
        }
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        match self.gate.check(READ, "addresses") {
            Ok(()) => self.inner.ipv4(),
            Err(_) => Vec::new(),
        }
    }
    fn rfkill(&self) -> Vec<Rfkill> {
        match self.gate.check(READ, "radio switches") {
            Ok(()) => self.inner.rfkill(),
            Err(_) => Vec::new(),
        }
    }
    fn set_rfkill(&self, id: &str, blocked: bool) -> Result<(), String> {
        self.gate.check(RADIO_SET, if blocked { "switch a radio off" } else { "switch a radio on" }).map_err(|d| d.to_string())?;
        self.inner.set_rfkill(id, blocked)
    }
    fn broadcom_wifi_without_driver(&self) -> bool {
        self.gate.check(READ, "wifi chip").is_ok() && self.inner.broadcom_wifi_without_driver()
    }
    fn screen(&self) -> Screen {
        match self.gate.check(READ, "screen") {
            Ok(()) => self.inner.screen(),
            Err(_) => Screen { framebuffer: None, outputs: Vec::new(), backlights: Vec::new() },
        }
    }
    fn volume(&self, card: u32) -> Option<Volume> {
        self.gate.check(READ, "the volume of a sound card").ok()?;
        self.inner.volume(card)
    }
    fn set_volume(&self, card: u32, percent: u32) -> Result<Volume, String> {
        self.gate.check(VOLUME_SET, "change the sound volume").map_err(|d| d.to_string())?;
        self.inner.set_volume(card, percent)
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> Result<Volume, String> {
        self.gate.check(VOLUME_SET, if delta.is_some() { "change the sound volume" } else { "mute or unmute the sound" }).map_err(|d| d.to_string())?;
        self.inner.change_volume(card, delta)
    }
    fn set_link(&self, name: &str, up: bool) -> Result<(), String> {
        self.gate.check(LINK_SET, &format!("switch the network connection {name} {}", if up { "on" } else { "off" })).map_err(|d| d.to_string())?;
        self.inner.set_link(name, up)
    }
    fn dhcp(&self, name: &str) -> Result<(), String> {
        self.gate.check(LINK_SET, &format!("ask for an address on {name}")).map_err(|d| d.to_string())?;
        self.inner.dhcp(name)
    }
    fn set_brightness(&self, name: &str, percent: u32) -> Result<(), String> {
        self.gate.check(BRIGHTNESS_SET, "set the screen brightness").map_err(|d| d.to_string())?;
        self.inner.set_brightness(name, percent)
    }
    fn cards(&self) -> Vec<(Card, Option<Volume>)> {
        match self.gate.check(READ, "sound cards") {
            Ok(()) => self.inner.cards(),
            Err(_) => Vec::new(),
        }
    }
    fn mac_infos(&self, cfg: &mac::MacConfig) -> Vec<mac::MacInfo> {
        match self.gate.check(READ, "Mac hardware") {
            Ok(()) => self.inner.mac_infos(cfg),
            Err(_) => Vec::new(),
        }
    }
    fn bluetooth(&self) -> btstate::BtState {
        match self.gate.check(READ, "Bluetooth hardware") {
            Ok(()) => self.inner.bluetooth(),
            Err(d) => btstate::BtState { adapters: Vec::new(), usb_present: false, switches: Vec::new(), unstarted: Vec::new(), controllers: Err(d.to_string()), product: String::new(), fw_file: false, held: false, needs_firmware: false },
        }
    }
    fn set_hostname(&self, name: &str, etc_hostname: &std::path::Path) -> Result<(), String> {
        self.gate.check(HOSTNAME_SET, "change the computer's name").map_err(|d| d.to_string())?;
        self.inner.set_hostname(name, etc_hostname)
    }
    fn mac_summary(&self) -> Option<String> {
        match self.gate.check(READ, "private address summary") {
            Ok(()) => self.inner.mac_summary(),
            Err(_) => None,
        }
    }
    fn hardware_report(&self) -> Vec<String> {
        match self.gate.check(READ, "hardware report") {
            Ok(()) => self.inner.hardware_report(),
            Err(_) => Vec::new(),
        }
    }
    fn cs8409_line(&self) -> Option<String> {
        self.gate.check(READ, "sound chip").ok()?;
        self.inner.cs8409_line()
    }
    fn request_class(&self, class: &str, who: &str) -> String {
        match self.gate.check(READ, "load the drivers of a hardware class") {
            Ok(()) => self.inner.request_class(class, who),
            Err(d) => d.to_string(),
        }
    }
}
