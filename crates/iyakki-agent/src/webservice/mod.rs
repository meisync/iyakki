//! The web service: a headless way to run a small, chosen part of IYAKKI behind HTTPS (`iyakki serve --profile agent-api`).
//! Plan and decisions: `docs/development/MINIMAL-WEBSERVICE.md`; user text: `docs/USER-GUIDE.md`.
//!
//! One route table ([`routes::ROUTES`]) says which routes exist, for which profile, who may call them and whether they are
//! audited; the dispatcher and the OpenAPI document ([`openapi`]) are both generated from it. Handlers see only the traits in
//! [`deps`] (accounts and audit from `iyakki-core-api`, the run store, a chat backend, a tool source) and never touch a
//! kernel interface. Everything not in the table answers 404.
//!
//! Nothing here runs without being started: [`server::start`] listens (loopback by default, HTTPS only), and the
//! supervisor wires the real accounts and services ([`real`]).

pub mod auth;
pub mod config;
pub mod deps;
mod files;
mod handlers;
pub mod openapi;
pub mod real;
pub mod routes;
pub mod runs;
pub mod server;
pub mod tls;
pub mod tools;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "v1";

/// A named set of routes and tools switched on at start. One binary, many profiles; choosing one never adds hardware access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Profile {
    /// Sign-in, chat runs, run history, approvals, models, the read-only web and data tools.
    AgentApi,
    /// The limited cloud deployment: `agent-api` plus flows (read) and the data tools directly. No local hardware of any kind.
    Cloud,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::AgentApi, Profile::Cloud];

    pub fn name(self) -> &'static str {
        match self {
            Profile::AgentApi => "agent-api",
            Profile::Cloud => "cloud",
        }
    }

    pub fn parse(s: &str) -> Result<Profile, String> {
        Profile::ALL.into_iter().find(|p| p.name() == s.trim()).ok_or_else(|| {
            let known: Vec<&str> = Profile::ALL.iter().map(|p| p.name()).collect();
            format!("unknown profile \"{s}\" (known: {}; the other profiles of the plan are not built yet)", known.join(", "))
        })
    }

    /// The tools a run may use in this profile, by name. Everything else is not offered to the model and refused if called.
    /// Read tools only: no file writes, no commands, no devices, no shell, no vault, no media, no radio, no camera.
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Profile::AgentApi => &["now", "web_search", "web_fetch", "web_research", "web_feed", "web_links", "weather", "stock_quote", "search_documents", "data_open", "data_describe", "data_query"],
            Profile::Cloud => &["now", "web_search", "web_fetch", "web_research", "web_feed", "web_links", "weather", "stock_quote", "search_documents", "data_open", "data_describe", "data_query", "flow_list", "flow_get", "flow_runs", "flow_run_status"],
        }
    }

    /// High-risk tools an administrator may switch on by name (`enable_tools`). Each still asks for approval on every call.
    /// None of them touches hardware. Empty today: the first slice offers no high-risk tool.
    pub fn high_risk_enableable(self) -> &'static [&'static str] {
        &[]
    }

    /// The cloud profile is cloud mode forced on (docs/CLOUD-MODE.md): nothing that needs local hardware, whatever the machine has.
    pub fn is_cloud(self) -> bool {
        self == Profile::Cloud
    }

    /// The apps whose permission a person needs for this profile's routes (all of them work with no local hardware).
    pub fn apps(self) -> &'static [iyakki_sec::policy::App] {
        use iyakki_sec::policy::App;
        match self {
            Profile::AgentApi => &[App::Chat],
            Profile::Cloud => &[App::Chat, App::Flows, App::Data],
        }
    }
}

/// The hardware an assistant tool needs, if any: the cloud profile never offers these, whatever the machine has.
pub fn tool_hardware(name: &str) -> Option<crate::cloudmode::Need> {
    use crate::cloudmode::Need;
    let n = name;
    if n.starts_with("radio_") || n.starts_with("media_") || n.ends_with("_media") || n.starts_with("speak") || n.starts_with("tales") || n.starts_with("voice") || n == "show_image" {
        Some(Need::Audio)
    } else if n.starts_with("camera") {
        Some(Need::Camera)
    } else {
        None
    }
}

/// Opens the service's port to the networks the administrator named, through the firewall (`lan` only). The supervisor
/// supplies the real one (it owns the firewall engine); tests supply a recorder.
pub trait PortOpener: Send + Sync {
    fn open(&self, port: u16, allow_from: &[String]) -> Result<(), String>;
}

/// The supervised foreground service: certificate, firewall rule (only on a LAN opt-in), startup registry, listen, wait for
/// `stop`, shut down gracefully. Returns when stopped. The caller has checked the configuration.
pub fn run(chk: &config::Checked, deps: std::sync::Arc<deps::Deps>, opener: &dyn PortOpener, stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let reg = crate::startup::global();
    reg.starting("service", "starting");
    let started = (|| {
        let identity = if chk.cfg.tls.cert.is_empty() {
            let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
            let names: Vec<String> = [host].into_iter().filter(|h| !h.is_empty()).collect();
            tls::self_signed(&chk.cfg.tls_dir(), &names)?
        } else {
            tls::from_files(std::path::Path::new(&chk.cfg.tls.cert), std::path::Path::new(&chk.cfg.tls.key))?
        };
        if identity.self_signed {
            log::warn!("service: made a self-signed certificate; pin this fingerprint in clients: SHA-256 {}", identity.fingerprint);
        }
        let svc = server::start(chk, deps, identity)?;
        if !chk.addr.ip().is_loopback() {
            if let Err(e) = opener.open(svc.local_addr().port(), &chk.cfg.allow_from) {
                svc.shutdown(std::time::Duration::from_secs(0));
                return Err(format!("the firewall rule for the service could not be added ({e}); not listening"));
            }
        }
        Ok(svc)
    })();
    let svc = match started {
        Ok(s) => s,
        Err(e) => {
            reg.failed("service", &e);
            return Err(e);
        }
    };
    reg.ready("service");
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    svc.shutdown(std::time::Duration::from_secs(10));
    reg.stopped("service");
    Ok(())
}
