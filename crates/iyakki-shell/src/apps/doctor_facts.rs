//! DOCTOR: reading the real machine into [`Facts`], and the pieces of the support bundle that only the shell can reach. Everything
//! here is read-only. The cheap reads run on the screen's thread when DOCTOR opens or the person checks again (a few small files,
//! the cached cluster health, one statvfs); the two slow ones (a connection to each local model server, the audit log) run in
//! [`slow_probe`] on a thread of their own.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::doctor::{self, FailedService, Facts};
use iyakki_agent::enginelog;

use super::AppCx;
use crate::accounts::AuditLog;

/// How many audit entries are looked at for the exposure line.
const AUDIT_LOOKBACK: usize = 400;

/// What the slow reads need (owned: it moves to a thread).
pub struct SlowIn {
    /// (provider id, `host:port`) of every enabled model server on this computer or the local network.
    pub servers: Vec<(String, String)>,
    pub audit: Arc<AuditLog>,
    /// Developer mode's port when it is on (asked on this computer only).
    pub dev_port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlowOut {
    pub models_unreachable: Vec<String>,
    pub exposure: Option<String>,
    pub net: Option<doctor::NetProbe>,
    pub dev_port_answers: Option<bool>,
}

pub type SlowProbe = Arc<dyn Fn(SlowIn) -> SlowOut + Send + Sync>;

/// The real slow reads.
pub(super) fn slow_probe(inp: SlowIn) -> SlowOut {
    let models_unreachable = inp.servers.iter().filter(|(_, hp)| !doctor::answers(hp, Duration::from_millis(600))).map(|(id, _)| id.clone()).collect();
    let route = doctor::has_default_route();
    // one name lookup (the time server's name), bounded; nothing is sent to it
    let dns = route && doctor::resolves(iyakki_agent::ntp::DEFAULT_SERVERS[0], Duration::from_secs(2));
    let dev_port_answers = inp.dev_port.map(|p| doctor::answers(&format!("127.0.0.1:{p}"), Duration::from_millis(600)));
    SlowOut { models_unreachable, exposure: doctor::exposure_from_audit(&inp.audit.recent(AUDIT_LOOKBACK)), net: Some(doctor::NetProbe { route, dns }), dev_port_answers }
}

pub(super) fn slow_input(cx: &AppCx) -> SlowIn {
    let servers = cx
        .services
        .settings
        .lock()
        .map(|s| s.get().providers.iter().filter(|p| p.enabled).filter_map(|p| p.base_url.as_deref().and_then(doctor::local_endpoint).map(|hp| (p.id.clone(), hp))).collect())
        .unwrap_or_default();
    let dev_port = iyakki_agent::devmode::DevMode::open(cx.services.dir()).is_on().then(|| iyakki_agent::devmode::DevMode::open(cx.services.dir()).state().port());
    SlowIn { servers, audit: cx.audit.clone(), dev_port }
}

fn uptime() -> u64 {
    doctor::uptime_secs()
}

/// The cheap reads. `slow` fields are left empty.
pub(super) fn gather(cx: &AppCx) -> Facts {
    use iyakki_agent::devmode::DevMode;
    let svc = &cx.services;
    let now = iyakki_agent::clock::now_unix();
    let h = svc.k3sdev.health();
    let st = svc.k3sdev.installed();
    let node_ip_wanted = st.as_ref().is_some_and(|s| !s.node_ip_disabled && s.node_ip == iyakki_agent::k3sdev::NODE_IP);
    let node_ip_present = doctor::has_address(iyakki_agent::k3sdev::NODE_IP);
    // developer mode and the firewall's opinion of its port
    let d = DevMode::open(svc.dir());
    let dev = d.state();
    let dev_on = d.is_on();
    let blocked = iyakki_fw::service::block_log().recent();
    let dev_firewall_hint = iyakki_fw::devrules::hint(&iyakki_fw::service::Handle::Global.lock(), &blocked, dev.port(), dev_on);
    // the vault and what waits for it
    let user = cx.session.user.as_str();
    let vault_waiting: Vec<String> = iyakki_agent::vaultpending::list(svc.dir()).into_iter().filter(|p| p.user == user).map(|p| p.what).collect();
    let vault_locked = !svc.user_vault_unlocked(user);
    // the web engine's kept messages
    let engine = enginelog::health(&enginelog::read_in(&enginelog::dir()), now - doctor::ENGINE_WINDOW_SECS);
    // sound
    let sound_cards = doctor::sound_card_count();
    let sound_chosen = svc.sound_settings().output;
    let sound_chosen_present = sound_chosen.is_empty() || {
        let outs = doctor::sound_outputs();
        iyakki_agent::audioout::choose(&outs, &sound_chosen).chosen
    };
    // the disk that holds the data
    let (disk_total, disk_free) = doctor::disk_space(svc.dir()).or_else(|| doctor::disk_space(Path::new("/"))).unwrap_or((0, 0));
    // time
    let network_time_on = svc.settings.lock().map(|s| !s.get().time.no_ntp).unwrap_or(true);
    let failed_services = iyakki_agent::startup::global()
        .snapshot()
        .into_iter()
        .filter_map(|r| match r.state {
            iyakki_agent::startup::StartupState::Failed(reason) => Some(FailedService { id: r.id.into(), name: r.name.into(), reason, open: r.goes.item() }),
            _ => None,
        })
        .collect();
    let (mem_total, mem_available, swap_total, swap_free) = doctor::parse_meminfo(&doctor::read_meminfo());
    let cameras = Some(doctor::camera_count());
    let firewall_on = Some(iyakki_fw::service::Handle::Global.lock().config().enabled);
    let update_note = iyakki_agent::devmode::update::interrupted_text(svc.dir());
    let crash_note = doctor::unseen_unclean_day(svc.dir());
    let vpn = {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let m = iyakki_agent::vpn_mgr::Manager::shared(svc, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)));
        m.summary().map(|s| s.light)
    };
    Facts {
        mem_total,
        mem_available,
        swap_total,
        swap_free,
        cameras,
        firewall_on,
        update_note,
        crash_note,
        vpn,
        now,
        uptime_secs: uptime(),
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
        clock_synced: iyakki_agent::ntp::synced(),
        network_time_on,
        clock_ahead_secs: iyakki_agent::ntp::status().pending_back.map(|s| s.max(0.0) as u64),
        exposure_unexpected: None,
        models_unreachable: Vec::new(),
        net: None,
        dev_port_answers: None,
        failed_services,
    }
}

/// What the support bundle collects from this machine (before redaction; `diagshare::build` redacts).
pub(super) fn bundle_inputs(cx: &AppCx, found: &[doctor::Finding]) -> iyakki_agent::diagshare::Inputs {
    let release = iyakki_agent::relinfo::ReleaseInfo::load(Path::new(iyakki_agent::relinfo::RELEASE_FILE));
    let mut version = vec![iyakki_agent::relinfo::about_line(release.as_ref())];
    if let Some(r) = &release {
        version.extend(r.rows().into_iter().map(|(k, v)| format!("{k}: {v}")));
    }
    let services = iyakki_agent::startup::global().report_lines();
    let hardware = doctor::hardware_text();
    let mut switches = Vec::new();
    if iyakki_agent::devmode::is_on(cx.services.dir()) {
        switches.push("developer mode".to_string());
    }
    if let Ok(s) = cx.services.settings.lock() {
        let t = &s.get().time;
        if !t.no_ntp {
            switches.push("network time".into());
        }
        if t.zone_auto {
            switches.push("time zone from the network".into());
        }
    }
    let boot = doctor::boot_log();
    let boot_tail: Vec<&str> = boot.lines().collect();
    iyakki_agent::diagshare::Inputs {
        version,
        services,
        hardware,
        doctor: doctor::report_lines(found, true),
        engine_log: enginelog::tail_text(&enginelog::dir(), 2000),
        boot_log: boot_tail[boot_tail.len().saturating_sub(200)..].join("\n"),
        switches,
        checkup: iyakki_agent::checkup::last_report(),
    }
}
