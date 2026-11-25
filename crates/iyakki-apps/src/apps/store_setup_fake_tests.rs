//! The STORE tile's SETUP section on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no `Services`, no session.
//! The agent facade, the quick-install facet and the core are the fakes of their crates, the audit log, the restart and the "show all AI" switch are
//! in memory, the data folder is the test's own. What these tests do not reach (the cluster switcher, the development cluster, the voice pack removal)
//! panics with a plain message instead of answering; the old tests of the Setup files run through the real port.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_agent_api::{AgentApi, B0Services, DevCluster, FakeAgentApi, NoB0, SettingsRef};
use iyakki_core_api::Core;
use iyakki_core_fake::FakeCore;

use super::*;

use crate::apps::store_port::StorePort;

pub(crate) struct Fake {
    admin: bool,
    power: bool,
    role: &'static str,
    data: PathBuf,
    audit: Mutex<Vec<(String, String)>>,
    reboots: AtomicUsize,
    show_all: Mutex<bool>,
    core: Arc<FakeCore>,
    agent: Arc<FakeAgentApi>,
    settings: SettingsRef,
    readable: PathBuf,
}

struct Offline;

impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

impl Fake {
    pub(crate) fn new(tag: &str, admin: bool) -> Fake {
        let data = std::fs::canonicalize(crate::apps::tests::temp_dir(tag)).unwrap();
        let store = iyakki_shared::settings::SettingsStore::open(&data.join("settings.json")).unwrap();
        Fake {
            admin,
            power: admin,
            role: if admin { "administrator" } else { "guest" },
            readable: data.clone(),
            data,
            audit: Mutex::new(vec![]),
            reboots: AtomicUsize::new(0),
            show_all: Mutex::new(false),
            core: Arc::new(FakeCore::new()),
            agent: Arc::new(FakeAgentApi::default()),
            settings: SettingsRef::of_store(&Arc::new(Mutex::new(store))),
        }
    }
    pub(crate) fn audited(&self, action: &str) -> Vec<String> {
        self.audit.lock().unwrap().iter().filter(|a| a.0 == action).map(|a| a.1.clone()).collect()
    }
}

impl StorePort for Fake {
    fn trusted_keys(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn user(&self) -> String {
        "tester".into()
    }
    fn role_name(&self) -> String {
        self.role.into()
    }
    fn manage_settings(&self) -> bool {
        self.admin
    }
    fn can_power(&self) -> bool {
        self.power
    }
    fn record(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push((action.into(), detail.into()));
    }
    fn data_dir(&self) -> &Path {
        &self.data
    }
    fn check_read(&self, path: &Path) -> Result<(), String> {
        if path.starts_with(&self.readable) {
            Ok(())
        } else {
            Err("outside the allowed folders".into())
        }
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        let role = if self.admin { iyakki_sec::policy::Role::Admin } else { iyakki_sec::policy::Role::Guest };
        iyakki_sec::policy::Policy::resolve(role, &Default::default())
    }
    fn recorder(&self) -> Arc<dyn iyakki_tile_store::engine::AuditSink> {
        unimplemented!("these tests do not reach the workers that record when they are done")
    }
    fn facets(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        Arc::new(iyakki_agent_api::FakeStoreFacets::default())
    }
    fn machine_facts(&self) -> iyakki_tile_store::shell::HostFacts {
        let c = self.core();
        let h = c.host();
        iyakki_tile_store::shell::HostFacts { mem_total_kb: h.mem_total_kb(), product_name: h.product_name(), sys_root: h.sys_root(), uptime_secs: h.uptime_secs() }
    }
    fn cloud_on(&self) -> bool {
        self.core().cloud().detect().on
    }
    fn voice_engines_set(&self) -> bool {
        let Ok(st) = self.settings().lock() else { return false };
        let v = &st.get().voice;
        v.stt.is_some() || v.tts.is_some()
    }
    fn shell_jobs(&self) -> Arc<dyn iyakki_tile_store::shell::ShellJobs> {
        Arc::new(crate::apps::store_port::AgentJobs { agent: self.agent.clone(), b4: self.b4(), up: self.quick_upstream() })
    }
    fn facts(&self) -> Arc<dyn iyakki_tile_store::facts::StoreFacts> {
        Arc::new(iyakki_tile_store::facts::FakeStoreFacts)
    }
    fn drivers_install(&self) -> Arc<dyn iyakki_tile_store::facts::DriversInstall> {
        Arc::new(crate::apps::store_port::AgentDriversInstall(self.facets().drivers()))
    }
    fn setup_svc(&self) -> Arc<dyn iyakki_tile_store::setup_svc::SetupSvcPort> {
        Arc::new(crate::apps::store_port::AgentSetupSvc { agent: self.agent.clone(), dev: self.k3sdev() })
    }
    fn setup_hw(&self) -> Arc<dyn iyakki_tile_store::setup_hw::SetupHwPort> {
        Arc::new(crate::apps::store_port::AgentSetupHw(self.agent.clone()))
    }
    fn testing(&self) -> bool {
        true
    }
    fn ai_profile(&self) -> Arc<dyn iyakki_tile_store::ai::AiProfilePort> {
        Arc::new(iyakki_tile_store::ai::FakeAiProfile::default())
    }
    fn drivers_read(&self) -> Arc<dyn iyakki_tile_store::drivers::DriversView> {
        unimplemented!("these tests do not read the machine's drivers")
    }
    fn b0(&self) -> Arc<dyn B0Services> {
        Arc::new(NoB0)
    }
    fn qi(&self) -> Arc<dyn iyakki_tile_api::store_qi::QuickInstall> {
        crate::apps::store_port::install_table();
        Arc::new(crate::apps::store_port::RealQi { b0: self.b0(), agent: self.agent.clone() })
    }
    fn b4(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB4> {
        unimplemented!("these tests do not reach the voice pack removal")
    }
    fn agent(&self) -> Arc<dyn AgentApi> {
        self.agent.clone()
    }
    fn core(&self) -> Arc<dyn Core> {
        self.core.clone()
    }
    fn reboot_machine(&self) {
        self.reboots.fetch_add(1, Ordering::SeqCst);
    }
    fn settings(&self) -> SettingsRef {
        self.settings.clone()
    }
    fn k3sdev(&self) -> Arc<dyn DevCluster> {
        unimplemented!("these tests do not reach the development cluster")
    }
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        Arc::new(Offline)
    }
    fn show_all_ai(&self) -> bool {
        *self.show_all.lock().unwrap()
    }
    fn set_show_all_ai(&self, on: bool) -> Result<(), String> {
        if !self.admin {
            return Err("not allowed".into());
        }
        *self.show_all.lock().unwrap() = on;
        Ok(())
    }
    fn cluster_host(&self, _base: usize, _workspace: &str) -> Box<dyn iyakki_tile_store::switch::ClusterSwitchView> {
        unimplemented!("these tests do not draw the cluster switcher")
    }
    fn manage_request(&self) -> (String, String) {
        (crate::apps::cluster_host::MANAGE_REQUEST.0.into(), crate::apps::cluster_host::MANAGE_REQUEST.1.into())
    }
    fn agent_login_state(&self, _agent: &str, _installed: bool) -> iyakki_tile_store::shell::LoginState {
        iyakki_tile_store::shell::LoginState::Unknown("fake".into())
    }
    fn agent_of_cli(&self, cli: &str) -> Option<&'static str> {
        crate::agent_login::agent_of_cli(cli)
    }
    fn login_request(&self, agent: &'static str, logout: bool) {
        crate::agent_login::request(agent, logout)
    }
}

fn setup(f: &Fake) -> Setup {
    let mut s = Setup::new();
    s.set_machine(Machine { ram_mb: 16_000, ..Default::default() });
    s.set_facts(Facts::default());
    s.enter(f);
    s
}

#[test]
fn entering_the_tab_reads_the_rights_and_the_jobs_through_the_port() {
    let f = Fake::new("ss-enter", true);
    let s = setup(&f);
    assert!(s.admin, "an administrator sees the actions");
    let g = Fake::new("ss-enter-guest", false);
    assert!(!setup(&g).admin);
}

#[test]
fn the_machine_is_read_from_the_core_and_the_agent_when_the_tab_is_not_fixed() {
    let f = Fake::new("ss-detect", true);
    let mut s = Setup::new();
    s.enter(&f);
    let want = f.core().host().mem_total_kb() / 1024;
    assert_eq!(s.machine.ram_mb, want);
    assert_eq!(s.machine.model, f.core().host().product_name());
    assert!(!s.machine.window, "the fake agent says this is no window session");
    assert!(f.agent.calls().iter().any(|c| c.contains("webengine") || c.contains("ai_profile")), "the agent facade was asked: {:?}", f.agent.calls());
}

#[test]
fn a_restart_needs_both_rights_and_is_audited_once() {
    let mut f = Fake::new("ss-reboot", true);
    let mut s = setup(&f);
    assert_eq!(s.reboot_now(&f).unwrap(), "Restarting…");
    assert_eq!(f.reboots.load(Ordering::SeqCst), 1);
    assert_eq!(f.audited("power.reboot"), vec!["restricted firmware pack".to_string()]);
    f.power = false;
    let e = s.reboot_now(&f).unwrap_err();
    assert!(e.contains("administrator") && e.contains("administrator)"), "{e}");
    assert_eq!(f.reboots.load(Ordering::SeqCst), 1, "no second restart");
}

#[test]
fn a_person_without_the_right_is_told_their_role_and_nothing_changes() {
    let f = Fake::new("ss-guest", false);
    let mut s = setup(&f);
    let e = s.hardware_remove(&f).unwrap_err();
    assert!(e.contains("Only an administrator") && e.contains("guest"), "{e}");
    s.show_all_ai_tools(&f);
    assert!(s.note.is_none() || !f.show_all_ai());
    assert!(!f.show_all_ai(), "the switch stays off");
    assert!(f.audited("settings.show_all_ai").is_empty());
}

#[test]
fn show_all_ai_tools_flips_the_switch_and_is_audited_for_an_administrator() {
    let f = Fake::new("ss-allai", true);
    let mut s = setup(&f);
    s.show_all_ai_tools(&f);
    assert!(f.show_all_ai());
    assert!(s.machine.show_all_ai);
    assert_eq!(f.audited("settings.show_all_ai"), vec!["on".to_string()]);
}

#[test]
fn removing_software_is_for_administrators_and_opens_the_review_card_for_an_installed_item() {
    let g = Fake::new("ss-rm-guest", false);
    let mut s = setup(&g);
    s.open_remove(Item::Ollama, &g);
    assert!(s.rm.is_none(), "a guest gets no card");
    let f = Fake::new("ss-rm", true);
    let mut s = setup(&f);
    s.open_remove(Item::Ollama, &f);
    assert!(s.rm.is_none(), "nothing installed: nothing to remove");
    let mut facts = Facts::default();
    facts.presence.push((Item::Ollama, Presence::Installed("0.1".into())));
    s.set_facts(facts);
    s.open_remove(Item::Ollama, &f);
    let card = s.rm.as_ref().expect("the review card opens");
    assert_eq!(card.item, Item::Ollama);
    assert!(!card.delete_data && !card.with_deps, "the safe choices are the defaults");
}

#[test]
fn the_category_colours_are_the_accents_of_the_tiles_they_open() {
    use super::{Gk, TILE_ACCENT_AI, TILE_ACCENT_DATA, TILE_ACCENT_HARDWARE, TILE_ACCENT_LINUX, TILE_ACCENT_MEDIA};
    assert_eq!(TILE_ACCENT_AI, crate::apps::flows::ACCENT);
    assert_eq!(TILE_ACCENT_DATA, crate::apps::data::ACCENT);
    assert_eq!(TILE_ACCENT_MEDIA, crate::apps::camera::ACCENT);
    assert_eq!(TILE_ACCENT_HARDWARE, crate::apps::dash::ACCENT);
    assert_eq!(TILE_ACCENT_LINUX, crate::apps::story::ACCENT);
    assert_eq!(Gk::Ai.accent(), TILE_ACCENT_AI);
}
