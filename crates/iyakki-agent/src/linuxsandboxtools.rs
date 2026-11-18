//! The assistant's tools for making and deleting Linux sandboxes (F-LINAPP-13): `linux_sandbox_create` starts the detached job that downloads a
//! base and makes a sandbox (the same job as `linux new` and the Linux apps page), `linux_sandbox_delete` removes one with everything in it.
//! Both ASK every time and never ride on a session grant. The same checks as the command line: the role may use Linux apps, a sandbox with
//! its network on needs an account allowed every host, and the account must be allowed to reach the distribution servers.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::linuxapps;
pub use iyakki_shared::linuxsandboxtools::{PROMPT_RULE};

pub const TOOLS: [&str; 2] = ["linux_sandbox_create", "linux_sandbox_delete"];


pub trait Backend: Send + Sync {
    /// The role may use Linux apps at all.
    fn allowed(&self) -> bool;
    /// The account may reach every host (a sandbox with its network on can).
    fn every_host(&self) -> bool;
    fn may_reach(&self, host: &str) -> bool;
    /// The names of the person's sandboxes.
    fn names(&self) -> Vec<String>;
    /// Size in MB, for the delete card.
    fn size_mb(&self, name: &str) -> u64;
    /// Start the detached job with these arguments (`create <name> <release> [--net on]`); the sentence for the person.
    fn start_job(&self, label: &str, args: &[String]) -> Result<String, String>;
    /// Delete (the typed name must equal the sandbox's); MB freed.
    fn delete(&self, name: &str, typed: &str) -> Result<u64, String>;
    fn audit(&self, action: &str, detail: &str);
}

pub struct LinuxSandboxTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

enum Plan {
    Create { name: String, release: String, net: bool },
    Delete { name: String, typed: String },
}

impl LinuxSandboxTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> LinuxSandboxTools {
        LinuxSandboxTools { inner, be }
    }

    fn plan(&self, name: &str, a: &Value) -> Result<Plan, String> {
        if !self.be.allowed() {
            return Err("your role may not use Linux apps".into());
        }
        let sname = a["name"].as_str().ok_or("give the sandbox name (name)")?.trim().to_string();
        if !linuxapps::valid_name(&sname) {
            return Err("a sandbox name is lower-case letters, digits and - (up to 30)".into());
        }
        if name == TOOLS[0] {
            let release = a["release"].as_str().ok_or("give the release (release): debian-12, debian-13, ubuntu-22.04 or ubuntu-24.04")?.to_string();
            if crate::linuxapps_dist::release(&release).is_none() {
                return Err(format!("`{release}` is not a release that can be made here (debian-12, debian-13, ubuntu-22.04, ubuntu-24.04)"));
            }
            if self.be.names().contains(&sname) {
                return Err(format!("a sandbox called {sname} already exists"));
            }
            let net = a["network"].as_bool().unwrap_or(false);
            if net && !self.be.every_host() {
                return Err("a sandbox with its network on can reach any server; only an account allowed every host may make one. Make it without the network".into());
            }
            if !self.be.may_reach("registry-1.docker.io") || !self.be.may_reach("cdimage.ubuntu.com") {
                return Err("your account may not reach the distribution servers (cdimage.ubuntu.com, registry-1.docker.io)".into());
            }
            Ok(Plan::Create { name: sname, release, net })
        } else {
            if !self.be.names().contains(&sname) {
                return Err(format!("there is no sandbox called {sname}"));
            }
            let typed = a["confirm_name"].as_str().unwrap_or("").to_string();
            if typed != sname {
                return Err(format!("repeat the sandbox name ({sname}) in confirm_name to delete it; nothing was deleted"));
            }
            Ok(Plan::Delete { name: sname, typed })
        }
    }
}

impl Tools for LinuxSandboxTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !TOOLS.contains(&d.spec.name.as_str()));
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[0].into(), description: "Make a Linux sandbox (Debian or Ubuntu) as a background job that downloads the base. Asks the person first.".into(), schema: json!({"type": "object", "properties": {"name": {"type": "string"}, "release": {"type": "string", "enum": ["debian-12", "debian-13", "ubuntu-22.04", "ubuntu-24.04"]}, "network": {"type": "boolean", "description": "Network on (any server reachable); default off."}}, "required": ["name", "release"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[1].into(), description: "Delete a Linux sandbox and everything in it. Asks the person first; cannot be undone.".into(), schema: json!({"type": "object", "properties": {"name": {"type": "string"}, "confirm_name": {"type": "string", "description": "The same name again."}}, "required": ["name", "confirm_name"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if !TOOLS.contains(&name) {
            return self.inner.call(name, args);
        }
        match self.plan(name, args)? {
            Plan::Create { name, release, net } => {
                let mut a: Vec<String> = vec!["create".into(), name.clone(), release.clone()];
                if net {
                    a.extend(["--net".into(), "on".into()]);
                }
                self.be.audit("linuxapps.create", &format!("{name} {release}{}", if net { " net" } else { "" }));
                self.be.start_job(&format!("Linux apps: create {name}"), &a)
            }
            Plan::Delete { name, typed } => {
                let freed = self.be.delete(&name, &typed)?;
                self.be.audit("linuxapps.delete", &format!("{name} {} MB freed", freed));
                Ok(format!("{name} was deleted ({freed} MB freed)"))
            }
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "linux_sandbox_create" => format!("Make the Linux sandbox “{}” ({})", args["name"].as_str().unwrap_or("?"), args["release"].as_str().unwrap_or("?")),
            "linux_sandbox_delete" => format!("DELETE the Linux sandbox “{}” and everything in it", args["name"].as_str().unwrap_or("?")),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if !TOOLS.contains(&name) {
            return self.inner.preview(name, args);
        }
        Some(match self.plan(name, args).ok()? {
            Plan::Create { name, release, net } => format!("Make the sandbox {name} from {release}. It downloads the base from the distribution's servers (about 120 MB unpacked) as a background job. Network: {}.", if net { "ON: it can reach any server" } else { "off" }),
            Plan::Delete { name, .. } => format!("Delete the sandbox {name}: its files and its settings ({} MB) are removed and cannot be brought back. Folders it had mounted from your home are not touched.", self.be.size_mb(&name)),
        })
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if TOOLS.contains(&name) { self.plan(name, args).err() } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if TOOLS.contains(&name) { Some("making and deleting sandboxes ask every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn correct_claims(&self, answer: &str) -> Option<String> {
        self.inner.correct_claims(answer)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: `LinuxApps` of the account and the jobs engine of the services.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl RealBackend {
    fn manager(&self) -> Result<linuxapps::LinuxApps, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        Ok(linuxapps::LinuxApps::new(self.svc.dir(), &self.user, &exe, Arc::new(crate::wsexec::System)))
    }
}

impl Backend for RealBackend {
    fn allowed(&self) -> bool {
        self.policy.cli == iyakki_sec::policy::CliLevel::Full
    }
    fn every_host(&self) -> bool {
        self.policy.net_hosts.iter().any(|h| h == "*")
    }
    fn may_reach(&self, host: &str) -> bool {
        self.policy.allows_host(host)
    }
    fn names(&self) -> Vec<String> {
        self.manager().map(|m| m.list().into_iter().map(|s| s.name).collect()).unwrap_or_default()
    }
    fn size_mb(&self, name: &str) -> u64 {
        self.manager().map(|m| m.storage(name) >> 20).unwrap_or(0)
    }
    fn start_job(&self, label: &str, args: &[String]) -> Result<String, String> {
        let l = crate::quickinstall::Launch::this_program(self.svc.dir())?;
        let spec = linuxapps::job_spec(&l, &self.user, label, args);
        let job = self.svc.jobs.start(&self.user, &spec, &|_: &std::path::Path| Ok(()))?;
        Ok(format!("started: {} (job {}). Its progress is beside the clock in the top bar; the log is in the installer list.", job.label, job.id))
    }
    fn delete(&self, name: &str, typed: &str) -> Result<u64, String> {
        self.manager()?.delete(name, typed).map(|b| b >> 20)
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
#[path = "linuxsandboxtools_tests.rs"]
mod tests;
