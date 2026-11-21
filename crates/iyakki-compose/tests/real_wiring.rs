//! The real-wiring guard (KI-1010 class: the UI running on a fake). It builds the wiring exactly as `iyakki-core`'s `build_shell_env` does
//! (`iyakki_compose::wire()`, then `ShellEnv::from_paths`, then `Shell::new`) and asserts that the handles the UI got are the real ones, not the
//! `Fake*`/`No*` defaults, and that Developer mode switches on through them. Linux only: the real host facts read `/proc`.
#![cfg(target_os = "linux")]

use std::sync::Arc;

use iyakki_agent_api::AgentFacets;
use iyakki_sec::policy::Role;
use iyakki_shell::shell::{Shell, ShellEnv, ShellPaths};

struct Host;
impl iyakki_agent::HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

fn paths(d: &std::path::Path) -> ShellPaths {
    ShellPaths { users: d.join("users.json").display().to_string(), audit: d.join("audit.log").display().to_string(), ai_conf: d.join("ai.conf").display().to_string(), data: d.join("data").display().to_string(), hwcheck: false, headless: true }
}

#[test]
fn the_wiring_main_builds_is_the_real_one_and_developer_mode_switches_on_through_it() {
    let d = iyakki_agent::testdir::short("iyakki-realwire", "a");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("data")).unwrap();
    // main: wire first, then the environment, then the shell
    iyakki_compose::wire();
    let env: ShellEnv = ShellEnv::from_paths(&paths(&d), Arc::new(Host)).expect("the environment");
    let data = d.join("data");
    let shell = Shell::new(env);

    // the services handle the UI holds (`ShellEnv::services`, `AppCx::services`) is the agent's real `Services`, not a fake default (KI-1010 class)
    assert!(shell.services_for_demo().is_real(), "the services handle of the shell is not the real implementation");
    assert!(shell.services_for_demo().as_any().is::<iyakki_agent::services::Services>(), "the services handle is not the agent's Services");
    assert!(!shell.services_for_demo().kb().embedder_id().is_empty() && shell.services_for_demo().dir() == data.as_path(), "the handle does not read the data folder it was opened on");

    // the facets that take the services as `Any` work through the handle the UI holds (the gate failure of 2026-11-18: they downcast to the concrete type only)
    {
        let h = shell.services_for_demo().clone();
        let any: &dyn std::any::Any = &h;
        let a = iyakki_apps::agent();
        assert!(!a.profile_store().reset_for(any, "admin").contains("no services handle"), "profile store: the handle is not understood");
        assert!(a.dict_fix().set_enabled(any, false).is_ok(), "dict fix: the handle is not understood");
        assert!(iyakki_agent::services::Services::from_any(h.clone().as_any_arc()).is_some() && iyakki_agent::services::Services::from_any(Arc::new(h)).is_some());
    }

    // the agent handle
    assert!(iyakki_shell::wiring::is_wired(), "no core factory installed");
    assert!(AgentFacets::is_real(iyakki_apps::agent()), "the process-wide agent handle is not the real one");
    assert!(iyakki_apps::agent_arc().is_real(), "the agent Arc is not the real one");

    // the host facts and the file scope: the process-wide ones and the ones of the shell's core
    let core = shell.core_handle();
    assert!(iyakki_apps::host().mem_total_kb() > 0, "the process-wide host facts are the empty No* ones");
    assert!(core.host().mem_total_kb() > 0, "the shell's core has no host facts");
    let admin = Role::Admin.template();
    let home = iyakki_apps::file_scope().home_root("admin", &data);
    std::fs::create_dir_all(&home).unwrap();
    assert!(iyakki_apps::file_scope().check_read(&admin, "admin", &home).is_ok(), "the process-wide file scope refuses everything (NoFileScope)");
    assert!(core.file_scope().check_read(&admin, "admin", &home).is_ok(), "the shell's core file scope refuses everything (NoFileScope)");

    // sound: when the machine has players or recorders the installed handle must be the agent's, not the empty one
    let probe = iyakki_apps::agent_arc();
    if probe.sound_probe().system_players().is_some() {
        assert!(iyakki_apps::players().play("true", &[]).err().is_none_or(|e| !e.contains("this core cannot play sound")), "players: the empty NoPlayers is installed");
    }
    if probe.sound_probe().system_recorders().is_some() {
        assert!(iyakki_apps::recorders().start("true", 1).err().is_none_or(|e| !e.contains("this core cannot record")), "recorders: the empty NoRecorders is installed");
    }

    // developer mode switches on through the handles the UI uses (Settings page: `host_probe().devmode`, the shell: `devmode()`) and off again
    let dm = iyakki_apps::agent().host_probe().devmode(&data);
    let st = dm.enable(&admin, "admin", 1).expect("Developer mode must switch on through the real wiring");
    assert!(dm.is_on(), "Developer mode did not switch on: {st:?}");
    assert!(iyakki_apps::agent().devmode().is_on(&data), "the shell's developer-mode handle does not see it on");
    dm.disable(&admin, "admin").expect("switch off");
    assert!(!dm.is_on());
}
