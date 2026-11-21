//! `FakeOs`: IYAKKI core in memory, for tests. Canned answers in, every call recorded, nothing touches the machine.
//! Implements [`Sysinfo`] and [`HostApi`] today; each slice of docs/development/UI-OS-SPLIT-PLAN.md adds its trait here.

pub mod contract;
mod fakecore;
mod files;
mod terminal;
pub use terminal::FakeTerminal;
pub use files::FakeFileTree;
mod fakebrowser;
pub use fakebrowser::{FakeBrowser, FakePage};
mod assistant;
mod appactions;
mod appevents;
pub use appactions::FakeAppActions;
mod cameras;
pub use appevents::FakeAppEvents;
pub use cameras::{FakeCam, FakeCameras};
mod media;
mod sound;
pub use sound::{FakeCues, FakeSound};
pub use assistant::{FakeAssistant, FakeTurn};
pub use media::FakeMedia;
mod crashlog;
mod startupdisk;
mod btaudio;
mod outputstate;
mod screenfacts;
pub use fakecore::{FakeCore, FakeMixer, FakeMonitors, FakeResume};
mod netscan;
mod router;
mod fleet;
mod hardware;
mod drivers;
pub mod diskengine;
pub use diskengine::FakeDiskEngine;
mod audioroute;
mod installer;
mod disktools;
mod memory;
pub use crashlog::FakeCrashLog;
pub use startupdisk::FakeStartupDisk;
pub use btaudio::{FakeBtAudio, Paired as BtPaired};
pub use outputstate::FakeOutputState;
pub use screenfacts::FakeScreenFacts;
pub use netscan::FakeNetScan;
pub use router::FakeRouter;
pub use fleet::FakeFleet;
pub use hardware::FakeHardware;
pub use drivers::FakeDrivers;
pub use audioroute::FakeAudioRoute;
pub use installer::FakeInstaller;
pub use disktools::FakeDiskTools;
pub use memory::FakeMemory;

use iyakki_core_api::{Audio, PlaybackVolume, SoundCard, BlockDev, Disks, Mount, Net, NetCounters, ApiError, ApiResult, Capabilities, HostApi, Identity, LoadAvg, MemInfo, Sysinfo, Uptime};
use iyakki_sec::policy::Policy;
use std::sync::Mutex;

/// What the fake answers with. `None` makes the matching call fail with `Unsupported`.
#[derive(Clone, Debug)]
pub struct Canned {
    pub cmdline: Option<String>,
    pub meminfo: Option<MemInfo>,
    pub uptime: Option<Uptime>,
    pub loadavg: Option<LoadAvg>,
    pub product_name: Option<String>,
    pub cpu_count: Option<u32>,
    pub block_devices: Option<Vec<BlockDev>>,
    pub mounts: Option<Vec<Mount>>,
    pub interfaces: Option<Vec<NetCounters>>,
    pub space: Option<(u64, u64)>,
    /// The data drive is gone: every failure message is reworded to this text (`None`: unchanged, a healthy drive).
    pub drive_gone: Option<String>,
    pub ipv4: Option<Vec<(String, String)>>,
    pub sound_cards: Option<Vec<SoundCard>>,
    /// The playback volume every card has in the fake (`set_volume` and `change_volume` move it).
    pub volume: Option<PlaybackVolume>,
    pub env: String,
    pub net: String,
    pub wasm_result: Result<String, String>,
    pub can_power: bool,
    pub capabilities: Capabilities,
    pub identity: Identity,
}

impl Default for Canned {
    fn default() -> Canned {
        Canned {
            cmdline: Some("quiet iyakki.test=1".into()),
            meminfo: Some(MemInfo { total_kb: 8_000_000, free_kb: 2_000_000, available_kb: 4_000_000, swap_total_kb: 0, swap_free_kb: 0 }),
            uptime: Some(Uptime { up_secs: 3600.0, idle_secs: 14000.0 }),
            loadavg: Some(LoadAvg { one: 0.25, five: 0.2, fifteen: 0.1 }),
            product_name: Some("Test Machine".into()),
            cpu_count: Some(4),
            block_devices: Some(vec![BlockDev { name: "sda".into(), size_bytes: 64_000_000_000, major: 8, minor: 0 }]),
            mounts: Some(vec![Mount { source: "/dev/sda1".into(), target: "/".into(), fstype: "ext4".into(), read_only: false }]),
            interfaces: Some(vec![NetCounters { name: "eth0".into(), ..Default::default() }]),
            space: Some((50_000_000_000, 100_000_000_000)),
            drive_gone: None,
            ipv4: Some(vec![("lo".into(), "127.0.0.1".into()), ("eth0".into(), "192.168.1.20".into())]),
            sound_cards: Some(vec![SoundCard { index: 0, name: "PCH".into(), long_name: "HDA Intel PCH".into() }]),
            volume: Some(PlaybackVolume { control: "Master".into(), percent: 40, muted: false }),
            env: "baremetal".into(),
            net: "eth0 Ethernet".into(),
            wasm_result: Ok("ok".into()),
            can_power: true,
            capabilities: Capabilities::all(),
            identity: Identity::local("tester", true),
        }
    }
}

#[derive(Default)]
pub struct FakeOs {
    canned: Mutex<Canned>,
    calls: Mutex<Vec<String>>,
    memory: std::sync::Arc<FakeMemory>,
    installer: std::sync::Arc<FakeInstaller>,
    drivers: std::sync::Arc<FakeDrivers>,
    disk_engine: std::sync::Arc<diskengine::FakeDiskEngine>,
    audio_route: std::sync::Arc<FakeAudioRoute>,
    hardware: std::sync::Arc<FakeHardware>,
    crashlog: std::sync::Arc<FakeCrashLog>,
    startup_disk: std::sync::Arc<FakeStartupDisk>,
    bt_audio: std::sync::Arc<FakeBtAudio>,
    output_state: std::sync::Arc<FakeOutputState>,
    screen_facts: std::sync::Arc<FakeScreenFacts>,
    netscan: std::sync::Arc<FakeNetScan>,
    router: std::sync::Arc<FakeRouter>,
    fleet: std::sync::Arc<FakeFleet>,
}

impl FakeOs {
    /// The drive engine of this fake, to set the scan and read what the page did.
    pub fn disk_engine_fake(&self) -> std::sync::Arc<diskengine::FakeDiskEngine> {
        self.disk_engine.clone()
    }
    pub fn new() -> FakeOs {
        FakeOs::default()
    }
    pub fn with(canned: Canned) -> FakeOs {
        FakeOs { canned: Mutex::new(canned), calls: Mutex::new(vec![]), memory: Default::default(), installer: Default::default(), drivers: Default::default(), disk_engine: Default::default(), audio_route: Default::default(), hardware: Default::default(), crashlog: Default::default(), startup_disk: Default::default(), bt_audio: Default::default(), output_state: Default::default(), screen_facts: Default::default(), netscan: Default::default(), router: Default::default(), fleet: Default::default() }
    }
    /// Change the canned answers while a test runs.
    pub fn set(&self, f: impl FnOnce(&mut Canned)) {
        f(&mut self.canned.lock().unwrap());
    }
    /// Every call so far, in order, as `method` or `method(argument)`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// The memory part of this fake, to set figures and read what the page did.
    pub fn memory_fake(&self) -> std::sync::Arc<FakeMemory> {
        self.memory.clone()
    }
    /// The sound-facts part of this fake (a made-up `/proc/asound`, device holders, mixer answers).
    pub fn audio_route_fake(&self) -> std::sync::Arc<FakeAudioRoute> {
        self.audio_route.clone()
    }
    /// The drivers part of this fake (machine, rows, installed packs, the calls).
    pub fn drivers_fake(&self) -> std::sync::Arc<FakeDrivers> {
        self.drivers.clone()
    }
    /// The install-to-disk part of this fake (disks, source, the plans it was asked to write).
    pub fn installer_fake(&self) -> std::sync::Arc<FakeInstaller> {
        self.installer.clone()
    }
    /// The network-scan part of this fake (subnets, the hosts it finds, the calls).
    pub fn netscan_fake(&self) -> std::sync::Arc<FakeNetScan> {
        self.netscan.clone()
    }
    /// The router part of this fake (what was asked, a switch to refuse).
    pub fn router_fake(&self) -> std::sync::Arc<FakeRouter> {
        self.router.clone()
    }
    /// The fleet part of this fake (what was asked, a switch to refuse).
    pub fn fleet_fake(&self) -> std::sync::Arc<FakeFleet> {
        self.fleet.clone()
    }
    /// The kept-logs part of this fake (a note, files with lines, the calls).
    /// The screens of this fake (connectors, the firmware framebuffer, the reads).
    pub fn screen_facts_fake(&self) -> std::sync::Arc<FakeScreenFacts> {
        self.screen_facts.clone()
    }
    /// The sound output state of this fake (device, generation, gain).
    pub fn output_state_fake(&self) -> std::sync::Arc<FakeOutputState> {
        self.output_state.clone()
    }
    /// The Bluetooth audio part of this fake (paired devices, the connection, the stream, the calls).
    pub fn bt_audio_fake(&self) -> std::sync::Arc<FakeBtAudio> {
        self.bt_audio.clone()
    }
    /// The startup-disk part of this fake (firmware entries, the rules, the calls).
    pub fn startup_disk_fake(&self) -> std::sync::Arc<FakeStartupDisk> {
        self.startup_disk.clone()
    }
    pub fn crashlog_fake(&self) -> std::sync::Arc<FakeCrashLog> {
        self.crashlog.clone()
    }
    /// The hardware part of this fake (links, radios, brightness, cards, Bluetooth).
    pub fn hardware_fake(&self) -> std::sync::Arc<FakeHardware> {
        self.hardware.clone()
    }
    pub fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }
    pub fn capabilities(&self) -> Capabilities {
        self.canned.lock().unwrap().capabilities
    }
    pub fn identity(&self) -> Identity {
        self.canned.lock().unwrap().identity.clone()
    }
    fn rec(&self, what: impl Into<String>) {
        self.calls.lock().unwrap().push(what.into());
    }
    fn answer<T: Clone>(&self, name: &str, pick: impl FnOnce(&Canned) -> Option<T>) -> ApiResult<T> {
        self.rec(name);
        pick(&self.canned.lock().unwrap()).ok_or_else(|| ApiError::Unsupported(format!("{name} is not available on this fake")))
    }
}

impl iyakki_core_api::Os for FakeOs {
    fn sysinfo(&self) -> &dyn Sysinfo {
        self
    }
    fn audio(&self) -> &dyn Audio {
        self
    }
    fn disks(&self) -> &dyn Disks {
        self
    }
    fn net(&self) -> &dyn Net {
        self
    }
    fn memory(&self) -> std::sync::Arc<dyn iyakki_core_api::memory::Memory> {
        self.memory.clone()
    }
    fn installer(&self) -> std::sync::Arc<dyn iyakki_core_api::installer::Installer> {
        self.installer.clone()
    }
    fn audio_route(&self) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> {
        self.audio_route.clone()
    }
    fn drivers(&self) -> std::sync::Arc<dyn iyakki_core_api::drivers::Drivers> {
        self.drivers.clone()
    }
    fn disk_engine(&self) -> std::sync::Arc<dyn iyakki_core_api::diskmodel::DiskEngine> {
        self.disk_engine.clone()
    }
    fn hardware(&self) -> std::sync::Arc<dyn iyakki_core_api::hardware::Hardware> {
        self.hardware.clone()
    }
    fn screen_facts(&self) -> std::sync::Arc<dyn iyakki_core_api::screenfacts::ScreenFacts> {
        self.screen_facts.clone()
    }
    fn output_state(&self) -> std::sync::Arc<dyn iyakki_core_api::soundctl::OutputState> {
        self.output_state.clone()
    }
    fn bt_audio(&self) -> std::sync::Arc<dyn iyakki_core_api::btaudio::BtAudio> {
        self.bt_audio.clone()
    }
    fn startup_disk(&self) -> std::sync::Arc<dyn iyakki_core_api::startupdisk::StartupDisk> {
        self.startup_disk.clone()
    }
    fn crashlog(&self) -> std::sync::Arc<dyn iyakki_core_api::crashlog::CrashLog> {
        self.crashlog.clone()
    }
    fn netscan(&self) -> std::sync::Arc<dyn iyakki_core_api::netscan::NetScan> {
        self.netscan.clone()
    }
    fn router(&self) -> std::sync::Arc<dyn iyakki_core_api::router::Router> {
        self.router.clone()
    }
    fn fleet(&self) -> std::sync::Arc<dyn iyakki_core_api::fleet::Fleet> {
        self.fleet.clone()
    }
}

impl Audio for FakeOs {
    fn sound_cards(&self) -> ApiResult<Vec<SoundCard>> {
        self.answer("sound_cards", |c| c.sound_cards.clone())
    }
    fn volume(&self, card: u32) -> ApiResult<PlaybackVolume> {
        self.rec(format!("volume({card})"));
        self.canned.lock().unwrap().volume.clone().ok_or_else(|| ApiError::Unsupported("this card has no volume control".into()))
    }
    fn set_volume(&self, card: u32, percent: u32) -> ApiResult<PlaybackVolume> {
        self.rec(format!("set_volume({card},{percent})"));
        let mut c = self.canned.lock().unwrap();
        let v = c.volume.as_mut().ok_or_else(|| ApiError::Unsupported("this card has no volume control".into()))?;
        v.percent = percent.min(100);
        v.muted = false;
        Ok(v.clone())
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> ApiResult<PlaybackVolume> {
        self.rec(format!("change_volume({card},{delta:?})"));
        let mut c = self.canned.lock().unwrap();
        let v = c.volume.as_mut().ok_or_else(|| ApiError::Unsupported("this card has no volume control".into()))?;
        match delta {
            Some(d) => v.percent = (v.percent as i32 + d).clamp(0, 100) as u32,
            None => v.muted = !v.muted,
        }
        Ok(v.clone())
    }
}

impl Sysinfo for FakeOs {
    fn cmdline(&self) -> ApiResult<String> {
        self.answer("cmdline", |c| c.cmdline.clone())
    }
    fn meminfo(&self) -> ApiResult<MemInfo> {
        self.answer("meminfo", |c| c.meminfo)
    }
    fn uptime(&self) -> ApiResult<Uptime> {
        self.answer("uptime", |c| c.uptime)
    }
    fn loadavg(&self) -> ApiResult<LoadAvg> {
        self.answer("loadavg", |c| c.loadavg)
    }
    fn product_name(&self) -> ApiResult<String> {
        self.answer("product_name", |c| c.product_name.clone())
    }
    fn cpu_count(&self) -> ApiResult<u32> {
        self.answer("cpu_count", |c| c.cpu_count)
    }
}

impl Disks for FakeOs {
    fn space(&self, path: &str) -> ApiResult<(u64, u64)> {
        self.rec(format!("space({path})"));
        self.canned.lock().unwrap().space.ok_or_else(|| ApiError::Unsupported("space is not available on this fake".into()))
    }
    fn block_devices(&self) -> ApiResult<Vec<BlockDev>> {
        self.answer("block_devices", |c| c.block_devices.clone())
    }
    fn mounts(&self) -> ApiResult<Vec<Mount>> {
        self.answer("mounts", |c| c.mounts.clone())
    }
    fn reword_failure(&self, msg: &str) -> String {
        self.rec("reword_failure".to_string());
        self.canned.lock().unwrap().drive_gone.clone().unwrap_or_else(|| msg.to_string())
    }
}

impl Net for FakeOs {
    fn ipv4_addresses(&self) -> ApiResult<Vec<(String, String)>> {
        self.answer("ipv4_addresses", |c| c.ipv4.clone())
    }
    fn interfaces(&self) -> ApiResult<Vec<NetCounters>> {
        self.answer("interfaces", |c| c.interfaces.clone())
    }
}

impl HostApi for FakeOs {
    fn detect_env(&self) -> String {
        self.rec("detect_env");
        self.canned.lock().unwrap().env.clone()
    }
    fn net_status(&self) -> String {
        self.rec("net_status");
        self.canned.lock().unwrap().net.clone()
    }
    fn run_wasm(&self, user: &str, _policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
        self.rec(format!("run_wasm({user},{} bytes,{entry})", wasm.len()));
        self.canned.lock().unwrap().wasm_result.clone()
    }
    fn power_off(&self) {
        self.rec("power_off");
    }
    fn reboot(&self) {
        self.rec("reboot");
    }
    fn can_power(&self) -> bool {
        self.canned.lock().unwrap().can_power
    }
}

/// In-memory accounts and audit for the web service's tests: a plain list of (name, password, role) and a list of audit lines.
pub struct FakeAccounts {
    users: std::sync::Mutex<Vec<(String, String, iyakki_sec::policy::Role)>>,
    audit: std::sync::Mutex<Vec<String>>,
}

impl FakeAccounts {
    pub fn new(users: &[(&str, &str, iyakki_sec::policy::Role)]) -> FakeAccounts {
        FakeAccounts { users: std::sync::Mutex::new(users.iter().map(|(n, p, r)| (n.to_string(), p.to_string(), *r)).collect()), audit: Default::default() }
    }
    pub fn remove(&self, name: &str) {
        self.users.lock().unwrap_or_else(|e| e.into_inner()).retain(|u| u.0 != name);
    }
    /// Every audit line so far, as `user event detail`.
    pub fn lines(&self) -> Vec<String> {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn principal(name: &str, role: iyakki_sec::policy::Role) -> iyakki_core_api::Principal {
        iyakki_core_api::Principal { user: name.into(), role: role.name().into(), admin: role == iyakki_sec::policy::Role::Admin, policy: role.template() }
    }
}

impl iyakki_core_api::Authenticator for FakeAccounts {
    fn login(&self, name: &str, password: &str) -> ApiResult<iyakki_core_api::Principal> {
        let g = self.users.lock().unwrap_or_else(|e| e.into_inner());
        match g.iter().find(|u| u.0 == name && u.1 == password) {
            Some(u) => Ok(FakeAccounts::principal(&u.0, u.2)),
            None => Err(ApiError::Denied("invalid username or password".into())),
        }
    }
    fn lookup(&self, user: &str) -> Option<iyakki_core_api::Principal> {
        let g = self.users.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().find(|u| u.0 == user).map(|u| FakeAccounts::principal(&u.0, u.2))
    }
}

impl iyakki_core_api::AuditSink for FakeAccounts {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{user} {event} {detail}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn disks_and_net_answer_from_the_canned_data_record_calls_and_can_be_unsupported() {
        let os = FakeOs::new();
        assert_eq!(os.block_devices().unwrap()[0].name, "sda");
        assert_eq!(os.mounts().unwrap()[0].target, "/");
        assert_eq!(os.interfaces().unwrap()[0].name, "eth0");
        assert_eq!(os.calls(), ["block_devices", "mounts", "interfaces"]);
        os.set(|c| c.interfaces = None);
        assert_eq!(os.interfaces().unwrap_err().code(), "unsupported");
    }

    #[test]
    fn sysinfo_answers_from_the_canned_data_and_records_each_call() {
        let os = FakeOs::new();
        assert_eq!(os.cmdline().unwrap(), "quiet iyakki.test=1");
        assert_eq!(os.meminfo().unwrap().total_kb, 8_000_000);
        assert_eq!(os.cpu_count().unwrap(), 4);
        assert_eq!(os.calls(), ["cmdline", "meminfo", "cpu_count"]);
        os.clear_calls();
        assert!(os.calls().is_empty());
    }

    #[test]
    fn a_missing_answer_is_unsupported_and_still_recorded() {
        let os = FakeOs::new();
        os.set(|c| c.product_name = None);
        let e = os.product_name().unwrap_err();
        assert_eq!(e.code(), "unsupported");
        assert_eq!(os.calls(), ["product_name"]);
    }

    #[test]
    fn canned_data_can_change_while_a_test_runs() {
        let os = FakeOs::new();
        os.set(|c| c.uptime = Some(Uptime { up_secs: 5.0, idle_secs: 1.0 }));
        assert_eq!(os.uptime().unwrap().up_secs, 5.0);
    }

    #[test]
    fn the_host_api_is_recorded_and_power_calls_do_nothing() {
        let os = FakeOs::new();
        let p = Role::Admin.template();
        assert_eq!(os.detect_env(), "baremetal");
        assert_eq!(os.net_status(), "eth0 Ethernet");
        assert_eq!(os.run_wasm("amy", &p, b"abc", "main"), Ok("ok".into()));
        os.power_off();
        os.reboot();
        assert!(os.can_power());
        assert_eq!(os.calls(), ["detect_env", "net_status", "run_wasm(amy,3 bytes,main)", "power_off", "reboot"]);
        os.set(|c| {
            c.can_power = false;
            c.wasm_result = Err("trap".into());
        });
        assert!(!os.can_power());
        assert_eq!(os.run_wasm("amy", &p, b"", "main"), Err("trap".into()));
    }

    #[test]
    fn it_works_as_a_trait_object_for_both_traits() {
        let os = std::sync::Arc::new(FakeOs::new());
        let h: std::sync::Arc<dyn HostApi> = os.clone();
        let s: std::sync::Arc<dyn Sysinfo> = os.clone();
        assert_eq!(h.detect_env(), "baremetal");
        assert!(s.loadavg().is_ok());
        assert_eq!(os.capabilities(), Capabilities::all());
        assert_eq!(os.identity().user, "tester");
    }
}
