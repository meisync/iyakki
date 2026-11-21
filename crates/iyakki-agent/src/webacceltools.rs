//! The chat tools for graphics acceleration of the browser (F-BROX-25, `webaccel.rs`): `browser_accel_get` reads (free): which parts are
//! on for this person and, with a site, for that site, and why each off part is off. `browser_accel_set` asks each time: `scope` `me` (this
//! person, every site), `site` (one site of this person) or `computer` (an administrator's override for every person). Turning it off is
//! always possible; turning it on only lifts the person's own switch (an administrator's off stays until an administrator lifts it).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
pub use iyakki_shared::webaccel::{Policy, Verdict};
use iyakki_shared::webaccel::{self as acc, Plan};

pub const READ_TOOL: &str = "browser_accel_get";
pub const WRITE_TOOL: &str = "browser_accel_set";

pub fn is_accel_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn policy(&self) -> Policy;
    fn save(&self, p: &Policy, what: &str) -> Result<(), String>;
    fn plan(&self, user: &str, host: &str) -> Plan;
}

pub struct AccelTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
    user: String,
    admin: bool,
}

enum Scope {
    Me,
    Site(String),
    Computer,
}

impl AccelTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>, user: &str, admin: bool) -> AccelTools {
        AccelTools { inner, backend, user: user.to_string(), admin }
    }

    fn parse(&self, args: &Value) -> Result<(Scope, bool), String> {
        let on = args.get("on").and_then(Value::as_bool).ok_or("`on` is true or false")?;
        let scope = match args.get("scope").and_then(Value::as_str).unwrap_or("me").trim() {
            "me" => Scope::Me,
            "site" => Scope::Site(acc::clean_site(args.get("site").and_then(Value::as_str).unwrap_or("")).ok_or("`site` is a host name such as video.example.org")?),
            "computer" if self.admin => Scope::Computer,
            "computer" => return Err("only an administrator changes this for the whole computer".into()),
            _ => return Err("scope is `me`, `site` or `computer`".into()),
        };
        Ok((scope, on))
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let (scope, on) = self.parse(args)?;
        let state = if on { "on" } else { "off" };
        Ok(match scope {
            Scope::Me => format!("Turn graphics acceleration {state} for you, on every site. It covers compositing, rasterisation, hardware video decoding and WebGL; where no driver is usable the processor draws anyway. Applies the next time the browser engine starts."),
            Scope::Site(s) => format!("Turn graphics acceleration {state} for {s} (and its subdomains), for you. Applies the next time that page is opened in the full browser."),
            Scope::Computer => format!("Turn graphics acceleration {state} for everyone on this computer (administrator). {} Applies the next time the browser engine starts.", if on { "People who turned it off themselves keep it off." } else { "Pages draw on the processor: slower, and it is the way out if a card misbehaves." }),
        })
    }

    fn get_text(&self, args: &Value) -> String {
        let host = args.get("site").and_then(Value::as_str).and_then(acc::clean_site).unwrap_or_default();
        let plan = self.backend.plan(&self.user, &host);
        let mut t = format!("Graphics acceleration of the browser{}:", if host.is_empty() { String::new() } else { format!(" for {host}") });
        for l in plan.lines() {
            t.push_str(&format!("\n- {l}"));
        }
        let p = self.backend.policy();
        if p.admin_off {
            t.push_str("\nAn administrator turned it off for this computer.");
        }
        if let Some(me) = p.people.get(&self.user) {
            if me.off {
                t.push_str("\nYou turned it off.");
            }
            if !me.sites_off.is_empty() {
                t.push_str(&format!("\nOff for these sites of yours: {}.", me.sites_off.join(", ")));
            }
        }
        t
    }
}

impl Tools for AccelTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_accel_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show which parts of the browser's graphics acceleration (compositing, rasterisation, video decoding, WebGL) are on for the person, optionally for one site, and why each off part is off.".into(), schema: json!({"type": "object", "properties": {"site": {"type": "string"}}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: WRITE_TOOL.into(),
                description: "Turn the browser's graphics acceleration on or off: for the person (scope me), for one site (scope site, with site), or for the whole computer (scope computer, administrators only). The person approves a card.".into(),
                schema: json!({"type": "object", "properties": {"scope": {"type": "string", "enum": ["me", "site", "computer"]}, "site": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["on"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.get_text(args)),
            WRITE_TOOL => {
                let (scope, on) = self.parse(args)?;
                let mut p = self.backend.policy();
                let what = match scope {
                    Scope::Me => {
                        p.set_person_off(&self.user, !on);
                        format!("Browser graphics acceleration {} for the person (chosen in chat)", if on { "on" } else { "off" })
                    }
                    Scope::Site(s) => {
                        p.set_site_off(&self.user, &s, !on)?;
                        format!("Browser graphics acceleration {} for {s} (chosen in chat)", if on { "on" } else { "off" })
                    }
                    Scope::Computer => {
                        p.set_admin_off(!on);
                        format!("Browser graphics acceleration {} for the computer (chosen in chat)", if on { "on" } else { "off" })
                    }
                };
                self.backend.save(&p, &what)?;
                Ok(format!("Done. It applies the next time the browser engine starts.\n{}", self.get_text(&json!({}))))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            WRITE_TOOL => match self.card(args) {
                Ok(c) => c.split(". ").next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            READ_TOOL => "Show the browser's graphics acceleration".into(),
            _ => self.inner.describe(name, args),
        }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }
    fn pre_approved(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("changing the browser's graphics acceleration asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
}

/// The real backend: `web-accel.json` in the data folder and the machine under `/`; every change is audited.
pub struct RealBackend {
    pub data: std::path::PathBuf,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn policy(&self) -> Policy {
        acc::load(&self.data.join("web-profile")).0
    }
    fn save(&self, p: &Policy, what: &str) -> Result<(), String> {
        acc::save(&self.data.join("web-profile"), p)?;
        (self.audit)("settings.browser", what);
        Ok(())
    }
    fn plan(&self, user: &str, host: &str) -> Plan {
        let software = crate::webgraphics::status(&self.data).software;
        crate::webaccel::plan_for(&self.data, std::path::Path::new("/"), user, host, software)
    }
}

#[cfg(test)]
#[path = "webacceltools_tests.rs"]
mod tests;
