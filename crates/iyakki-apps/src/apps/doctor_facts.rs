//! DOCTOR in the shell: the reading of the real machine lives in `iyakki_agent::doctor_gather` (shared with `iyakki doctor`); this file
//! only hands it what the running shell holds ([`Live`]: the dev cluster, the vault, the VPN) and builds the pieces of the support
//! bundle that only the shell can reach. Everything here is read-only.

use std::path::Path;
use std::sync::Arc;

use iyakki_shared::doctor::{self, Facts};
pub use iyakki_shared::doctor_gather::{SlowIn, SlowOut};
use iyakki_shared::enginelog;

use super::AppCx;

pub type SlowProbe = Arc<dyn Fn(SlowIn) -> SlowOut + Send + Sync>;

/// The slow reads of the DOCTOR tile (a thread of their own).
pub fn slow_probe(inp: SlowIn) -> SlowOut {
    crate::agent().helpers_b4().doctor_slow_probe(inp)
}

pub(super) fn slow_input(cx: &AppCx) -> SlowIn {
    let audit = cx.audit.clone();
    cx.b5().doctor_slow_input(Arc::new(move |n| audit.recent(n)))
}

/// The cheap reads. `slow` fields are left empty.
pub(super) fn gather(cx: &AppCx) -> Facts {
    let audit = cx.audit.clone();
    cx.b5().doctor_gather(&cx.session.user, Arc::new(move |u, a, d| audit.record(u, a, d)))
}

/// The compat report as plain lines (versions, peers for administrators, deprecated names seen): the support bundle and Settings show it.
pub(super) fn compat_lines(cx: &AppCx) -> Vec<String> {
    let admin = cx.session.policy.manage_settings;
    let mut l = cx.agent.devmode().compat_lines(cx.data_dir(), admin);
    l.extend(iyakki_shared::twophase::status_lines(cx.data_dir()));
    l
}

/// What the support bundle collects from this machine (before redaction; `diagshare::build` redacts).
pub(super) fn bundle_inputs(cx: &AppCx, found: &[doctor::Finding]) -> iyakki_shared::diagshare::Inputs {
    let release = iyakki_shared::relinfo::ReleaseInfo::load(Path::new(iyakki_shared::relinfo::RELEASE_FILE));
    let mut version = vec![iyakki_shared::relinfo::about_line(release.as_ref())];
    if let Some(r) = &release {
        version.extend(r.rows().into_iter().map(|(k, v)| format!("{k}: {v}")));
    }
    let services = iyakki_shared::startup::global().report_lines();
    let hardware = format!("{}\nBrowser graphics: {}", doctor::hardware_text(), cx.agent.web_graphics().status(cx.data_dir()).line);
    let mut switches = Vec::new();
    if cx.agent.devmode().is_on(cx.data_dir()) {
        switches.push("developer mode".to_string());
    }
    if let Ok(s) = cx.settings().lock() {
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
    iyakki_shared::diagshare::Inputs {
        version,
        services,
        hardware,
        doctor: doctor::report_lines(found, true),
        engine_log: enginelog::tail_text(&enginelog::dir(), 2000),
        boot_log: boot_tail[boot_tail.len().saturating_sub(200)..].join("\n"),
        switches,
        checkup: cx.agent.checkup_last_report(),
        compat: compat_lines(cx),
    }
}
