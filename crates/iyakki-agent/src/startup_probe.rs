//! The cheap poll behind the startup registry ([`crate::startup`]): for the services that cannot push their own progress it reads
//! what is already there (a few small files, a lock, a cached health snapshot) and turns it into states.
//!
//! [`Facts`] is everything the rules need; [`apply`] is pure (tests bring their own facts), [`gather`] reads the real machine. The
//! caller polls about once a second while something is still starting and about every five seconds after that. Nothing here starts
//! or waits: no process is run, no network is touched, no lock is waited for.
//!
//! Services that push (the local models while one loads, the flows scheduler at sign-in) are not touched here.

use std::sync::Arc;

use crate::startup::{Registry, StartupState};

/// What the clock gate of the cluster waits for, and how long it waits when nobody answers ([`crate::k3sdev::CLOCK_WAIT_LIMIT`]).
const CLOCK_LIMIT_SECS: u64 = 120;
/// After this long without a default route the network is reported as missing (the cluster keeps waiting for it).
const NET_GRACE_SECS: u64 = 90;
/// The network drivers load a few seconds after the sign-in screen; when this much time has passed and no network device (Ethernet or Wi-Fi) is
/// there at all, nothing is coming: the network is reported as not available at once, and the clock does not wait for it.
const NO_NIC_GRACE_SECS: u64 = 20;
/// A sound card shows up late sometimes (USB); after this long "none" is the answer.
const SOUND_WAIT_SECS: u64 = 30;
/// Autostart fires within seconds of the start; later, a stopped cluster was stopped on purpose.
const AUTOSTART_WAIT_SECS: u64 = 45;
/// A start that takes longer than this says so.
const SLOW_SECS: u64 = 120;

/// The cluster as `K3sDev::health()` shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct K3sFact {
    pub installed: bool,
    pub autostart: bool,
    /// `not installed`, `stopped`, `starting`, `running`, `stopping`, `failed`.
    pub state: String,
    /// The cluster's own wait note ("waiting for the clock to be set...").
    pub note: String,
    pub failed_reason: String,
    pub node_ready: Option<bool>,
}

/// Everything the rules read.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub uptime_secs: u64,
    /// The clock floor has not been applied yet.
    pub clock_floor_pending: bool,
    /// A time server answered.
    pub clock_synced: bool,
    /// An IPv4 default route is up.
    pub route: bool,
    /// The machine has no network device at all (no Ethernet or Wi-Fi in /sys/class/net with hardware behind it).
    pub no_nic: bool,
    pub k3s: K3sFact,
    /// The firewall is wanted but not in force.
    pub firewall_alert: bool,
    /// Sound cards the kernel lists.
    pub sound_cards: usize,
    /// The browser's engine is installed on this image.
    pub web_engine: bool,
    /// Folder shares being served now.
    pub shares_running: usize,
    /// USB touch screens, and whether each had its one re-attach ([`crate::touchprobe`]).
    pub touch: crate::touchprobe::TouchFact,
}

fn trim_note(t: &str) -> String {
    t.trim().trim_end_matches(['\u{2026}', '.']).trim().to_string()
}

/// Turn the facts into states (only for the polled services).
pub fn apply(reg: &Registry, f: &Facts) {
    // nothing to wait for: no network device, and the drivers had their time
    let no_net = !f.route && f.no_nic && f.uptime_secs >= NO_NIC_GRACE_SECS;
    // the clock: the same gate the cluster uses
    if f.clock_floor_pending {
        reg.starting("clock", "setting the clock");
    } else if f.clock_synced {
        reg.ready("clock");
    } else if f.uptime_secs < CLOCK_LIMIT_SECS && !no_net {
        // without a network no time server can answer: say what is really missing
        reg.starting("clock", if f.route { "waiting for a time server" } else { "waiting for the network" });
    } else {
        // offline: the saved time is good enough, which is what the cluster decides too
        reg.ready("clock");
    }
    // the network
    if f.route {
        reg.ready("network");
    } else if no_net {
        reg.disabled("network");
    } else if f.uptime_secs < NET_GRACE_SECS {
        reg.starting("network", "waiting for a connection (cable or Wi-Fi)");
    } else if f.k3s.installed && f.k3s.autostart {
        reg.failed("network", "no network yet: the cluster is waiting for one (connect a cable or Wi-Fi)");
    } else {
        reg.stopped("network");
    }
    // the firewall
    if f.firewall_alert {
        reg.failed("firewall", "the firewall is wanted but not in force");
    } else {
        reg.ready("firewall");
    }
    // sound
    if f.sound_cards > 0 {
        reg.ready("sound");
    } else if f.uptime_secs < SOUND_WAIT_SECS {
        reg.starting("sound", "looking for a sound card");
    } else {
        reg.disabled("sound");
    }
    // the touch screen: none is nothing to wait for; one is checked once, in software, when the boot has settled
    if f.touch.panels == 0 || f.touch.attached {
        if f.touch.panels == 0 {
            reg.disabled("touch");
        } else {
            reg.ready("touch");
        }
    } else {
        reg.starting("touch", "checking the touch screen");
    }
    // the dev cluster
    let k = &f.k3s;
    if !k.installed || k.state == "not installed" {
        reg.disabled("k3s");
    } else {
        match k.state.as_str() {
            "failed" => {
                reg.failed("k3s", if k.failed_reason.is_empty() { "k3s stopped with an error" } else { &k.failed_reason });
            }
            "starting" => {
                let note = trim_note(&k.note);
                let step = if !note.is_empty() {
                    note
                } else if reg.age("k3s") > SLOW_SECS && matches!(reg.state("k3s"), Some(StartupState::Starting { .. })) {
                    "starting k3s (taking longer than usual)".to_string()
                } else {
                    "starting k3s (about 20 s)".to_string()
                };
                reg.starting("k3s", &step);
            }
            "running" => {
                if k.node_ready == Some(false) {
                    reg.starting("k3s", "waiting for the node to be ready");
                } else {
                    reg.ready("k3s");
                }
            }
            "stopping" => {
                reg.starting("k3s", "stopping k3s");
            }
            // stopped
            _ => {
                if k.autostart && f.uptime_secs < AUTOSTART_WAIT_SECS && matches!(reg.state("k3s"), Some(StartupState::Pending | StartupState::Disabled)) {
                    // autostart has not fired yet: pending (the dependency text says for what it waits)
                    reg.report("k3s", StartupState::Pending);
                } else {
                    reg.stopped("k3s");
                }
            }
        }
    }
    // the web engine is started on demand: ready means it is there
    if f.web_engine {
        reg.ready("web");
    } else {
        reg.disabled("web");
    }
    // shared folders: serving, or none set up
    if f.shares_running > 0 {
        reg.ready("shares");
    } else {
        reg.disabled("shares");
    }
}

/// Read the machine and apply.
/// `firewall_alert`: the firewall is wanted but not in force (the shell asks `iyakki_fw::service::alert()`; this crate does not
/// depend on the firewall).
pub fn poll(svc: &Arc<crate::services::Services>, reg: &Registry, firewall_alert: bool) {
    let f = gather(svc, firewall_alert);
    crate::touchprobe::step(f.uptime_secs);
    apply(reg, &f);
}

pub fn gather(svc: &Arc<crate::services::Services>, firewall_alert: bool) -> Facts {
    let h = svc.k3sdev.health();
    Facts {
        uptime_secs: std::fs::read_to_string("/proc/uptime").ok().and_then(|t| t.split('.').next().and_then(|s| s.parse().ok())).unwrap_or(u64::MAX / 2),
        clock_floor_pending: crate::clockfloor::pending(),
        clock_synced: crate::ntp::synced(),
        route: crate::k3sdev::has_default_route(&std::fs::read_to_string(crate::k3sdev::ROUTE_FILE).unwrap_or_default()),
        no_nic: !std::fs::read_dir("/sys/class/net").map(|d| d.flatten().any(|e| e.path().join("device").exists())).unwrap_or(true),
        k3s: K3sFact { installed: h.installed, autostart: h.autostart, state: h.state.clone(), note: h.note.clone(), failed_reason: h.failed_reason.clone(), node_ready: h.node_ready },
        firewall_alert,
        sound_cards: std::fs::read_to_string("/proc/asound/cards").map(|t| t.lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).count()).unwrap_or(0),
        web_engine: crate::webengine::offered(),
        shares_running: crate::sharedfolder::running_total(),
        touch: crate::touchprobe::state(),
    }
}

/// The Retry button: start the service again. Returns the line to show.
pub fn retry(svc: &Arc<crate::services::Services>, id: &str, user: &str) -> Result<String, String> {
    let reg = crate::startup::global();
    match id {
        "k3s" => {
            reg.starting("k3s", "starting k3s (about 20 s)");
            svc.k3sdev.start(user).map(|_| "Starting the cluster.".to_string()).inspect_err(|e| {
                reg.failed("k3s", e);
            })
        }
        "models" => {
            reg.starting("models", "loading the model");
            svc.warm_up_chat();
            Ok("Loading the model.".to_string())
        }
        _ => Err("nothing to start again for this one".to_string()),
    }
}
