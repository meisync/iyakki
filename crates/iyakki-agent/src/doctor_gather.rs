//! DOCTOR: reading the real machine into [`doctor::Facts`], shared by the DOCTOR tile and `iyakki doctor` (the command line).
//! Everything here is read-only. The cheap reads are [`gather`]; the slow ones (a connection to each local model server, the
//! name lookup, the audit log) are [`slow_probe`] and run on a thread of the caller's own.
//!
//! **Two callers, one reader.** The shell passes the running system's state ([`Live`]: the dev cluster supervisor, whether the
//! signed-in person's vault is locked, the VPN light). The command line is a separate process that does not hold any of that, so
//! it passes `live: None`: the dev cluster, the vault and the VPN are then *not looked at* (a new process would see "stopped" and
//! "locked" for everything, which is false), and [`not_checked_note`] says so in the printed report.

pub use iyakki_shared::doctor_gather::{AUDIT_LOOKBACK, AuditRecent, SlowIn, SlowOut, not_checked_note};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::doctor::{self, FailedService, Facts, NetProbe};
use crate::enginelog;
use crate::settings::SettingsStore;


/// State that only the running system holds.
pub struct Live<'a> {
    pub k3sdev: &'a crate::k3sdev::K3sDev,
    pub vault_locked: bool,
    pub vpn_light: Option<crate::vpn_mgr::Light>,
}

pub struct Gather<'a> {
    /// The data folder (`/var/lib/iyakki`).
    pub dir: &'a Path,
    /// The person the vault and update notes belong to.
    pub user: &'a str,
    pub settings: &'a Mutex<SettingsStore>,
    /// `None` from the command line (see the module text).
    pub live: Option<Live<'a>>,
}






pub type SlowProbe = Arc<dyn Fn(SlowIn) -> SlowOut + Send + Sync>;

/// The real slow reads.
pub fn slow_probe(inp: SlowIn) -> SlowOut {
    let models_unreachable = inp.servers.iter().filter(|(_, hp)| !doctor::answers(hp, Duration::from_millis(600))).map(|(id, _)| id.clone()).collect();
    let route = doctor::has_default_route();
    // one name lookup (the time server's name), bounded; nothing is sent to it
    let dns = route && doctor::resolves(crate::ntp::DEFAULT_SERVERS[0], Duration::from_secs(2));
    let dev_port_answers = inp.dev_port.map(|p| doctor::answers(&format!("127.0.0.1:{p}"), Duration::from_millis(600)));
    SlowOut { models_unreachable, exposure: doctor::exposure_from_audit(&(inp.audit_recent)(AUDIT_LOOKBACK)), net: Some(NetProbe { route, dns }), dev_port_answers }
}

pub fn slow_input(dir: &Path, settings: &Mutex<SettingsStore>, audit_recent: AuditRecent) -> SlowIn {
    let servers = settings
        .lock()
        .map(|s| s.get().providers.iter().filter(|p| p.enabled).filter_map(|p| p.base_url.as_deref().and_then(doctor::local_endpoint).map(|hp| (p.id.clone(), hp))).collect())
        .unwrap_or_default();
    let d = crate::devmode::DevMode::open(dir);
    let dev_port = d.is_on().then(|| d.state().port());
    SlowIn { servers, audit_recent, dev_port }
}

/// The cheap reads. `slow` fields are left empty.
pub fn gather(g: &Gather) -> Facts {
    use crate::devmode::DevMode;
    let now = crate::clock::now_unix();
    // the dev cluster (only the running system knows it)
    let (h, node_ip_wanted) = match &g.live {
        Some(l) => {
            let h = l.k3sdev.health();
            let st = l.k3sdev.installed();
            (h, st.as_ref().is_some_and(|s| !s.node_ip_disabled && s.node_ip == crate::k3sdev::NODE_IP))
        }
        None => (Default::default(), false),
    };
    let node_ip_present = doctor::has_address(crate::k3sdev::NODE_IP);
    // developer mode and the firewall's opinion of its port
    let d = DevMode::open(g.dir);
    let dev = d.state();
    let dev_on = d.is_on();
    let blocked = iyakki_fw::service::block_log().recent();
    let dev_firewall_hint = iyakki_fw::devrules::hint(&iyakki_fw::service::Handle::Global.lock(), &blocked, dev.port(), dev_on);
    // the vault and what waits for it
    let vault_waiting: Vec<String> = match &g.live {
        Some(_) => crate::vaultpending::list(g.dir).into_iter().filter(|p| p.user == g.user).map(|p| p.what).collect(),
        None => Vec::new(),
    };
    let vault_locked = g.live.as_ref().is_some_and(|l| l.vault_locked);
    // the web engine's kept messages
    let engine = enginelog::health(&enginelog::read_in(&enginelog::dir()), now - doctor::ENGINE_WINDOW_SECS);
    // sound
    let sound_cards = doctor::sound_card_count();
    let sound_chosen = g.settings.lock().map(|s| s.get().sound.output.clone()).unwrap_or_default();
    let sound_chosen_present = sound_chosen.is_empty() || {
        let outs = crate::doctor_sys::sound_outputs();
        crate::audioout::choose(&outs, &sound_chosen).chosen
    };
    // the disk that holds the data
    let (disk_total, disk_free) = doctor::disk_space(g.dir).or_else(|| doctor::disk_space(Path::new("/"))).unwrap_or((0, 0));
    let network_time_on = g.settings.lock().map(|s| !s.get().time.no_ntp).unwrap_or(true);
    let failed_services = crate::startup::global()
        .snapshot()
        .into_iter()
        .filter_map(|r| match r.state {
            crate::startup::StartupState::Failed(reason) => Some(FailedService { id: r.id.into(), name: r.name.into(), reason, open: r.goes.item() }),
            _ => None,
        })
        .collect();
    // memory through the core-API trait (F-ARCH-03): a machine without the figures gives zeros, which the rules read as "unknown"
    let mem = <crate::localos::LocalOs as iyakki_core_api::sysinfo::Sysinfo>::meminfo(&crate::localos::LocalOs::real()).unwrap_or_default();
    let (mem_total, mem_available, swap_total, swap_free) = (mem.total_kb * 1024, mem.available_kb * 1024, mem.swap_total_kb * 1024, mem.swap_free_kb * 1024);
    let graphics = crate::webgraphics::status(g.dir);
    Facts {
        mem_total,
        mem_available,
        swap_total,
        swap_free,
        cameras: Some(doctor::camera_count()),
        firewall_on: Some(iyakki_fw::service::Handle::Global.lock().config().enabled),
        update_note: crate::devmode::update::interrupted_text(g.dir),
        crash_note: doctor::unseen_unclean_day(g.dir),
        vpn: g.live.as_ref().and_then(|l| l.vpn_light.clone()),
        now,
        uptime_secs: crate::doctor_sys::uptime_secs(),
        cluster_installed: h.installed,
        cluster_state: h.state.clone(),
        cluster_autostart: h.autostart,
        cluster_failed_reason: h.failed_reason.clone(),
        cluster_last_exit_at: h.last_exit_at,
        node_ip_wanted,
        node_ip_present,
        dev_on,
        dev_until: dev.until,
        dev_port: dev.port(),
        dev_firewall_hint,
        vault_locked,
        vault_waiting,
        engine,
        sound_cards,
        sound_chosen,
        sound_chosen_present,
        disk_total,
        disk_free,
        route: doctor::has_default_route(),
        clock_synced: crate::ntp::synced(),
        network_time_on,
        clock_ahead_secs: crate::ntp::status().pending_back.map(|s| s.max(0.0) as u64),
        exposure_unexpected: None,
        models_unreachable: Vec::new(),
        net: None,
        dev_port_answers: None,
        failed_services,
        graphics_line: graphics.line.clone(),
        graphics_fallback: graphics.fallback,
    }
}

/// The report text for already-gathered facts and the exit code (0: nothing wrong, 1: at least one problem or warning, so a script
/// can test it). `detail` adds the technical lines the tile shows to administrators only; the command line is for the person at
/// the machine. `live` is false from the command line: the report then ends with [`not_checked_note`].
pub fn report_text(facts: &Facts, detail: bool, live: bool) -> (String, i32) {
    let found = doctor::diagnose(facts);
    let mut lines = doctor::report_lines(&found, detail);
    if !live {
        lines.push(String::new());
        lines.push(not_checked_note().to_string());
    }
    let bad = found.iter().any(|f| f.severity != doctor::Severity::Note);
    (lines.join("\n") + "\n", i32::from(bad))
}

/// `iyakki doctor [--detail]`: gather, probe (with `slow`, the real one in production), diagnose, print.
pub fn cli_report(g: &Gather, audit_recent: AuditRecent, detail: bool, slow: &SlowProbe) -> (String, i32) {
    let mut facts = gather(g);
    slow(slow_input(g.dir, g.settings, audit_recent)).apply(&mut facts);
    report_text(&facts, detail, g.live.is_some())
}

/// The DOCTOR report of the running system (F-DOCTOR-03): the same reads as the tile, with the dev cluster, the vault and the VPN light
/// that only the running system knows. Reads only. `user` is the account whose vault is asked. Used by `iyakki dev doctor` (developer
/// mode) and the `doctor_report` chat tool.
pub fn live_report(svc: &crate::services::Services, user: &str, detail: bool, audit_recent: AuditRecent, slow: &SlowProbe) -> (String, i32) {
    let live = Live { k3sdev: &svc.k3sdev, vault_locked: !svc.user_vault_unlocked(user), vpn_light: crate::vpn_mgr::Manager::peek(svc.dir(), user).and_then(|m| m.summary()).map(|s| s.light) };
    let g = Gather { dir: svc.dir(), user, settings: &svc.settings, live: Some(live) };
    cli_report(&g, audit_recent, detail, slow)
}

/// `iyakki doctor` arguments: `--detail`, `--json`-less on purpose (the tile and the bundle use the same text). Unknown words are an
/// error so a typo does not silently print the short form.
pub fn parse_cli(args: &[String]) -> Result<bool, String> {
    let mut detail = false;
    for a in args {
        match a.as_str() {
            "--detail" | "-d" => detail = true,
            other => return Err(format!("iyakki doctor: unknown option {other} (usage: iyakki doctor [--detail])")),
        }
    }
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-docgather-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn healthy() -> Facts {
        Facts { mem_total: 8 << 30, mem_available: 4 << 30, disk_total: 100 << 30, disk_free: 50 << 30, route: true, clock_synced: true, network_time_on: true, sound_cards: 1, sound_chosen_present: true, net: Some(NetProbe { route: true, dns: true }), cameras: Some(1), firewall_on: Some(true), ..Facts::default() }
    }

    #[test]
    fn the_command_line_takes_only_detail() {
        assert_eq!(parse_cli(&[]), Ok(false));
        assert_eq!(parse_cli(&["--detail".into()]), Ok(true));
        assert!(parse_cli(&["--deatil".into()]).unwrap_err().contains("unknown option --deatil"));
    }

    #[test]
    fn a_calm_machine_exits_0_and_a_sick_one_exits_1_and_the_cli_says_what_it_did_not_look_at() {
        let (text, code) = report_text(&healthy(), false, false);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("Everything checked is fine."));
        assert!(text.contains(not_checked_note()), "from the command line the report says what it could not see");
        let sick = Facts { disk_free: 1 << 20, ..healthy() };
        let (text, code) = report_text(&sick, true, true);
        assert_eq!(code, 1, "{text}");
        assert!(!text.contains(not_checked_note()), "the shell sees everything, so no note");
    }

    #[test]
    fn without_the_running_system_nothing_is_said_about_the_cluster_the_vault_or_the_vpn() {
        let d = tmp("nolive");
        let settings = Mutex::new(SettingsStore::open(&d.join("settings.json")).unwrap());
        let f = gather(&Gather { dir: &d, user: "", settings: &settings, live: None });
        assert!(!f.cluster_installed && !f.node_ip_wanted && !f.vault_locked && f.vault_waiting.is_empty() && f.vpn.is_none());
        let found = doctor::diagnose(&f);
        for x in &found {
            assert!(!matches!(x.id, "cluster-stopped" | "cluster-failed" | "vault-locked"), "{}", x.text);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_probe_answers_are_put_into_the_facts_and_the_audit_line_is_read_through_the_closure() {
        let d = tmp("slow");
        let settings = Mutex::new(SettingsStore::open(&d.join("settings.json")).unwrap());
        let audit: AuditRecent = Arc::new(|n| {
            assert_eq!(n, AUDIT_LOOKBACK);
            vec!["2026-01-01 admin devcluster.exposure.unexpected NodePort 30300".into()]
        });
        let inp = slow_input(&d, &settings, audit);
        assert!(inp.servers.is_empty() && inp.dev_port.is_none());
        let out = slow_probe(inp);
        assert_eq!(out.exposure.as_deref(), Some("NodePort 30300"));
        let mut f = healthy();
        out.apply(&mut f);
        assert_eq!(f.exposure_unexpected.as_deref(), Some("NodePort 30300"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cli_report_runs_the_whole_chain_with_a_fake_probe() {
        let d = tmp("chain");
        let settings = Mutex::new(SettingsStore::open(&d.join("settings.json")).unwrap());
        let slow: SlowProbe = Arc::new(|_| SlowOut { models_unreachable: vec!["lemonade".into()], ..SlowOut::default() });
        let (text, _) = cli_report(&Gather { dir: &d, user: "", settings: &settings, live: None }, Arc::new(|_| Vec::new()), true, &slow);
        assert!(text.contains(not_checked_note()));
        assert!(text.to_lowercase().contains("lemonade") || text.contains("model"), "{text}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
