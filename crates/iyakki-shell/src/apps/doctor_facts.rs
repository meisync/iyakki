//! DOCTOR in the shell: the reading of the real machine lives in `iyakki_agent::doctor_gather` (shared with `iyakki doctor`); this file
//! only hands it what the running shell holds ([`Live`]: the dev cluster, the vault, the VPN) and builds the pieces of the support
//! bundle that only the shell can reach. Everything here is read-only.

use std::path::Path;
use std::sync::Arc;

use iyakki_agent::doctor::{self, Facts};
use iyakki_agent::doctor_gather::{self as gather_mod, Gather, Live};
use iyakki_agent::enginelog;

use super::AppCx;

pub use gather_mod::{slow_probe, SlowIn, SlowOut, SlowProbe};

pub(super) fn slow_input(cx: &AppCx) -> SlowIn {
    let audit = cx.audit.clone();
    gather_mod::slow_input(cx.services.dir(), &cx.services.settings, Arc::new(move |n| audit.recent(n)))
}

/// The cheap reads. `slow` fields are left empty.
pub(super) fn gather(cx: &AppCx) -> Facts {
    let svc = &cx.services;
    let user = cx.session.user.as_str();
    let vpn_light = {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let m = iyakki_agent::vpn_mgr::Manager::shared(svc, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)));
        m.summary().map(|s| s.light)
    };
    let live = Live { k3sdev: &svc.k3sdev, vault_locked: !svc.user_vault_unlocked(user), vpn_light };
    gather_mod::gather(&Gather { dir: svc.dir(), user, settings: &svc.settings, live: Some(live) })
}

/// The compat report as plain lines (versions, peers for administrators, deprecated names seen): the support bundle and Settings show it.
pub(super) fn compat_lines(cx: &AppCx) -> Vec<String> {
    let admin = cx.session.policy.manage_settings;
    let dm = iyakki_agent::devmode::DevMode::open(cx.services.dir());
    let peers = if admin { iyakki_agent::compatreport::peers(cx.services.dir(), &dm.keys(), dm.now()) } else { vec![] };
    let mut l = iyakki_agent::compatreport::report(peers).lines(admin);
    l.extend(iyakki_agent::twophase::status_lines(cx.services.dir()));
    l
}

/// What the support bundle collects from this machine (before redaction; `diagshare::build` redacts).
pub(super) fn bundle_inputs(cx: &AppCx, found: &[doctor::Finding]) -> iyakki_agent::diagshare::Inputs {
    let release = iyakki_agent::relinfo::ReleaseInfo::load(Path::new(iyakki_agent::relinfo::RELEASE_FILE));
    let mut version = vec![iyakki_agent::relinfo::about_line(release.as_ref())];
    if let Some(r) = &release {
        version.extend(r.rows().into_iter().map(|(k, v)| format!("{k}: {v}")));
    }
    let services = iyakki_agent::startup::global().report_lines();
    let hardware = format!("{}\nBrowser graphics: {}", doctor::hardware_text(), iyakki_agent::webgraphics::status(cx.services.dir()).line);
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
        compat: compat_lines(cx),
    }
}
