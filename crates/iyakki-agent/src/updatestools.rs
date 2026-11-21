//! The lead assistant's tools for the update check (F-UPD-04): `updates_status` reads (no card): the version that runs, how often the
//! device looks, when it last looked, what is waiting and whether the link is metered. `updates_check_now` asks every time (it
//! contacts the update sources: the store sources and, when an administrator set one, the system update list) and only reports: it
//! never downloads or installs anything. Applying an app update is `store_update` (which asks too); a system update stays a person's
//! click on Settings -> System update.
//!
//! * Only an administrator may ask for a check (the same rule as the Settings page); the card says which addresses will be asked.
//! * A check is refused while "How often" is off, the same as the page.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::store::updatecheck::{CheckSettings, Kind, Mode, Report};
pub use iyakki_shared::updatestools::{PROMPT_RULE};


pub const READ_TOOL: &str = "updates_status";
pub const WRITE_TOOL: &str = "updates_check_now";
/// How often the device looks (the same choice as Settings -> System update and Store -> Settings, devmode key `store.daily_check`).
pub const SETTINGS_READ_TOOL: &str = "updates_settings_status";
pub const SETTINGS_WRITE_TOOL: &str = "updates_settings_set";
/// Open a signed system update file (a .tgz with its .sig next to it) from a stick or a folder: the signature is checked first, the update
/// is unpacked where Settings -> System update lists it, and nothing is installed (F-UPD-01).
pub const BUNDLE_TOOL: &str = "updates_bundle_open";
/// Fetch a signed system update file from an https address (an update channel or a private address the person names), check its signature
/// with the release key before unpacking, and list it like an opened file (F-UPD-01). Nothing is installed.
pub const FETCH_TOOL: &str = "updates_bundle_fetch";

pub fn is_updates_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL || n == SETTINGS_READ_TOOL || n == SETTINGS_WRITE_TOOL || n == BUNDLE_TOOL || n == FETCH_TOOL
}

fn parse_mode(args: &Value) -> Result<Mode, String> {
    match args.get("how_often").and_then(Value::as_str).unwrap_or("").trim().to_lowercase().as_str() {
        "daily" | "every day" => Ok(Mode::Daily),
        "on_request" | "manual" | "only on request" => Ok(Mode::Manual),
        "off" => Ok(Mode::Off),
        other => Err(format!("`{other}` is not a choice; use daily, on_request or off")),
    }
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// The running version and the variant.
    fn running(&self) -> (String, String);
    fn settings(&self) -> CheckSettings;
    /// The answer of the last check, if there was one.
    fn report(&self) -> Option<Report>;
    /// `None` when the machine cannot tell.
    fn metered(&self) -> Option<bool>;
    /// May this person ask for a check (administrators)?
    fn may_check(&self) -> bool;
    /// Look now. Slow (network).
    fn check(&self) -> Report;
    fn now(&self) -> i64;
    /// Keep how often the device looks (administrators; the real one also writes the audit line).
    fn save_mode(&self, mode: Mode) -> Result<(), String>;
    /// The compat report in plain lines (F-CORE-19): which versions of the core and its interfaces meet here, the peers (names for
    /// administrators only) and the deprecated names. A backend without one says nothing.
    fn compat_lines(&self) -> Vec<String> {
        Vec::new()
    }
    /// Open the signed system update file at `path` for the update centre (administrators; see `sysupdate::open_for_centre`). A backend
    /// that cannot says so.
    fn open_bundle(&self, _path: &str) -> Result<String, String> {
        Err("opening a system update file is not available here".into())
    }
    /// Fetch the signed system update at the https address `url` for the update centre (administrators; see `sysupdate::fetch_for_centre`).
    fn fetch_bundle(&self, _url: &str) -> Result<String, String> {
        Err("fetching a system update is not available here".into())
    }
}

pub struct UpdatesTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn ago(secs: i64) -> String {
    match secs {
        i64::MIN..=89 => "just now".into(),
        90..=5399 => format!("{} minutes ago", secs / 60),
        5400..=129_599 => format!("{} hours ago", (secs + 1800) / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// The lines of a report: one per update with the new permissions named, then the sources that could not be reached.
fn report_lines(r: &Report) -> Vec<String> {
    let mut out = vec![];
    for u in &r.updates {
        let what = match u.kind {
            Kind::System => format!("System: {} {} -> {} (install it from Settings -> System update; the assistant cannot)", u.title, u.from, u.to),
            Kind::App => format!("App {}: {} {} -> {} from {}", u.id, u.title, u.from, u.to, u.source),
        };
        let what = if let Some(v) = u.needs_newer_iyakki() { format!("{what}. NEEDS A NEWER IYAKKI ({v} or newer): update IYAKKI first") } else { what };
        out.push(if u.needs_review() { format!("{what}. ASKS FOR NEW PERMISSIONS: {}", u.new_permissions.join(", ")) } else { what });
    }
    for (src, e) in &r.errors {
        out.push(format!("Could not reach {src}: {e}"));
    }
    out
}

impl UpdatesTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> UpdatesTools {
        UpdatesTools { inner, backend }
    }

    fn status(&self) -> String {
        let b = &self.backend;
        let (version, variant) = b.running();
        let cfg = b.settings();
        let mut out = vec![format!("IYAKKI {version}{}.", if variant.is_empty() { String::new() } else { format!(" ({variant})") })];
        out.push(format!("Checking for updates: {}.{}", cfg.mode.label(), if cfg.system_url.is_empty() { " No system update list is set; only apps in the store sources are checked." } else { " The system update list is checked too." }));
        out.push(match b.metered() {
            Some(true) => format!("The connection is metered{}.", if cfg.metered_ok { "; an administrator allowed the daily check on it" } else { ", so the daily check waits" }),
            Some(false) => "The connection is not metered.".into(),
            None => "This computer cannot tell whether the connection is metered.".into(),
        });
        match b.report() {
            None => out.push("Not checked yet (updates_check_now asks the person).".into()),
            Some(r) => {
                out.push(format!("Last checked {}: {}.", ago(b.now() - r.checked_at), match r.count() { 0 => "everything is up to date".to_string(), n => format!("{n} update(s) waiting; nothing was downloaded or installed") }));
                out.extend(report_lines(&r));
            }
        }
        let compat = b.compat_lines();
        if !compat.is_empty() {
            out.push("Compatibility:".into());
            out.extend(compat.into_iter().map(|l| format!("  {l}")));
        }
        out.join("\n")
    }

    fn settings_status(&self) -> String {
        let cfg = self.backend.settings();
        let mut out = vec![format!("Checking for updates: {}.", cfg.mode.label())];
        out.push(match cfg.mode {
            Mode::Daily => "Once a day while an administrator is signed in (and once at sign-in when the last look is older than a day), the device asks only the stores and update list address that were added here; the built-in catalog is never contacted. It reports and raises one notice; nothing is installed.".into(),
            Mode::Manual => "Only when the person presses Check now (or updates_check_now is approved).".into(),
            Mode::Off => "Nothing is asked, not even on request.".into(),
        });
        out.push(if cfg.system_url.is_empty() { "No system update list address is set.".into() } else { "A system update list address is set (Settings -> System update shows it).".into() });
        out.join("\n")
    }

    fn settings_card(&self, args: &Value) -> Result<String, String> {
        if !self.backend.may_check() {
            return Err("only an administrator can change how often updates are looked for".into());
        }
        let mode = parse_mode(args)?;
        let cur = self.backend.settings().mode;
        if mode == cur {
            return Err(format!("it is already set to {}", mode.label()));
        }
        Ok(format!("Change how often this device looks for updates: {} -> {}. {}", cur.label(), mode.label(), match mode {
            Mode::Daily => "It will contact the stores and the update list address that were added here, once a day. Nothing is downloaded or installed.",
            Mode::Manual => "It will look only when the person asks.",
            Mode::Off => "It will never look, not even on request.",
        }))
    }

    fn bundle_card(&self, args: &Value) -> Result<String, String> {
        if !self.backend.may_check() {
            return Err("only an administrator can open a system update file".into());
        }
        let path = args.get("path").and_then(Value::as_str).unwrap_or("").trim();
        if path.is_empty() || path.len() > 400 || path.contains('\0') {
            return Err("give the path of the update file (a .tgz with its .sig file next to it)".into());
        }
        if !path.ends_with(".tgz") && !path.ends_with(".tar.gz") {
            return Err("a system update file is a .tgz".into());
        }
        Ok(format!("Open the system update file {path}: its signature file ({path}.sig) is checked against the IYAKKI release key before anything is unpacked, then the update is listed in Settings -> System update. Nothing is installed or restarted."))
    }

    fn fetch_card(&self, args: &Value) -> Result<String, String> {
        if !self.backend.may_check() {
            return Err("only an administrator can fetch a system update".into());
        }
        let url = args.get("url").and_then(Value::as_str).unwrap_or("").trim();
        if url.is_empty() || url.len() > 400 || url.contains(['\0', ' ', '\n', '?', '#']) {
            return Err("give the https address of the update file (a .tgz; its signature is the same address with .sig added)".into());
        }
        if !url.starts_with("https://") {
            return Err("a system update is fetched over https only".into());
        }
        if !url.ends_with(".tgz") && !url.ends_with(".tar.gz") {
            return Err("a system update file is a .tgz".into());
        }
        let host = url.trim_start_matches("https://").split('/').next().unwrap_or("");
        Ok(format!("Download the system update {url} (and {url}.sig) from {host}: the signature is checked against the IYAKKI release key before anything is unpacked, then the update is listed in Settings -> System update and the downloaded files are removed. Nothing is installed or restarted."))
    }

    fn card(&self) -> Result<String, String> {
        if !self.backend.may_check() {
            return Err("only an administrator can ask for an update check".into());
        }
        let cfg = self.backend.settings();
        if cfg.mode == Mode::Off {
            return Err("checking for updates is off (Settings -> System update, How often); the person turns it on first".into());
        }
        Ok(format!(
            "Look for updates now. This device contacts every store source{}. It only reports what is newer; nothing is downloaded or installed.",
            if cfg.system_url.is_empty() { String::new() } else { format!(" and the system update list ({})", cfg.system_url) }
        ))
    }
}

impl Tools for UpdatesTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_updates_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show the installed IYAKKI version, how often and when updates were last looked for, what is waiting (apps and the system), whether the connection is metered, and the compatibility report (versions of the core and its interfaces, paired computers for administrators, deprecated names). Reads the last answer; contacts nothing.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: WRITE_TOOL.into(), description: "Look for updates now: contacts the store sources and the system update list the administrator set, and reports what is newer. Never downloads or installs. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SETTINGS_READ_TOOL.into(), description: "Show how often the device looks for updates (daily, only on request, off) and what it asks. Reads settings; contacts nothing.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SETTINGS_WRITE_TOOL.into(), description: "Change how often the device looks for updates: daily, on_request or off. Daily contacts only the stores and update list address the person added. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"how_often": {"type": "string", "enum": ["daily", "on_request", "off"]}}, "required": ["how_often"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: BUNDLE_TOOL.into(), description: "Open a signed system update file (a .tgz with its .sig file next to it, for example from a USB stick) for Settings -> System update: the signature is checked with the IYAKKI release key before anything is unpacked; an update for another edition, too old a base or not newer is refused with the reason. Never installs or restarts. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"path": {"type": "string", "description": "The path of the .tgz; its signature is the file with .sig added."}}, "required": ["path"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: FETCH_TOOL.into(), description: "Download a signed system update from an https address (a .tgz; the signature file is the same address with .sig added) for Settings -> System update: the signature is checked with the IYAKKI release key before anything is unpacked; an update for another edition, too old a base or not newer is refused with the reason. Never installs or restarts. The person approves a card that names the address first.".into(), schema: json!({"type": "object", "properties": {"url": {"type": "string", "description": "The https address of the .tgz."}}, "required": ["url"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.status()),
            SETTINGS_READ_TOOL => Ok(self.settings_status()),
            SETTINGS_WRITE_TOOL => {
                self.settings_card(args)?;
                let mode = parse_mode(args)?;
                self.backend.save_mode(mode)?;
                Ok(format!("Checking for updates is now: {}.", mode.label()))
            }
            BUNDLE_TOOL => {
                self.bundle_card(args)?;
                self.backend.open_bundle(args.get("path").and_then(Value::as_str).unwrap_or("").trim())
            }
            FETCH_TOOL => {
                self.fetch_card(args)?;
                self.backend.fetch_bundle(args.get("url").and_then(Value::as_str).unwrap_or("").trim())
            }
            WRITE_TOOL => {
                self.card()?;
                let r = self.backend.check();
                let mut out = vec![match (r.count(), r.errors.len()) {
                    (0, 0) => "Everything is up to date.".to_string(),
                    (0, e) => format!("Nothing new found, but {e} source(s) could not be reached."),
                    (n, 0) => format!("{n} update(s) available. Nothing was downloaded or installed."),
                    (n, e) => format!("{n} update(s) available; {e} source(s) could not be reached. Nothing was downloaded or installed."),
                }];
                out.extend(report_lines(&r));
                Ok(out.join("\n"))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            WRITE_TOOL => "Look for updates now".into(),
            READ_TOOL => "Show the update status".into(),
            SETTINGS_READ_TOOL => "Show how often updates are looked for".into(),
            SETTINGS_WRITE_TOOL => "Change how often updates are looked for".into(),
            BUNDLE_TOOL => format!("Open the system update file {}", args.get("path").and_then(Value::as_str).unwrap_or("")),
            FETCH_TOOL => format!("Download the system update {}", args.get("url").and_then(Value::as_str).unwrap_or("")),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card().ok(),
            SETTINGS_WRITE_TOOL => self.settings_card(args).ok(),
            BUNDLE_TOOL => self.bundle_card(args).ok(),
            FETCH_TOOL => self.fetch_card(args).ok(),
            READ_TOOL | SETTINGS_READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card().err(),
            SETTINGS_WRITE_TOOL => self.settings_card(args).err(),
            BUNDLE_TOOL => self.bundle_card(args).err(),
            FETCH_TOOL => self.fetch_card(args).err(),
            READ_TOOL | SETTINGS_READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_updates_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && name != SETTINGS_WRITE_TOOL && name != BUNDLE_TOOL && name != FETCH_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL || name == SETTINGS_WRITE_TOOL { Some("looking for updates contacts other computers and the schedule is a privacy choice, so it asks every time".into()) } else if name == BUNDLE_TOOL || name == FETCH_TOOL { Some("opening or downloading a system update unpacks software that is meant to be installed on this computer, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the release file, the settings, the kept answer and the store sources.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn running(&self) -> (String, String) {
        let rel = crate::relinfo::ReleaseInfo::parse(&std::fs::read_to_string(crate::relinfo::RELEASE_FILE).unwrap_or_default());
        (if rel.version.is_empty() { crate::relinfo::VERSION.to_string() } else { rel.version }, rel.variant)
    }
    fn settings(&self) -> CheckSettings {
        self.svc.settings.lock().map(|s| s.get().update_check.clone()).unwrap_or_default()
    }
    fn report(&self) -> Option<Report> {
        Report::load(self.svc.dir())
    }
    fn metered(&self) -> Option<bool> {
        crate::meteredlink::metered_now()
    }
    fn may_check(&self) -> bool {
        self.policy.manage_settings
    }
    fn check(&self) -> Report {
        (self.audit)("system.update.check", "now (assistant)");
        let (running, variant) = self.running();
        let store = crate::store::Store::for_services(&self.svc);
        let fetch = crate::store::catalog::HttpFetch(self.svc.quick_upstream());
        crate::store::updatecheck::run_and_keep(&store, &self.svc, &self.policy, &self.user, &fetch, &self.settings(), &running, &variant)
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
    fn compat_lines(&self) -> Vec<String> {
        let admin = self.policy.manage_settings;
        let dm = crate::devmode::DevMode::open(self.svc.dir());
        let mut l = crate::compatreport::report(if admin { crate::compatreport::peers(self.svc.dir(), &dm.keys(), dm.now()) } else { vec![] }).lines(admin);
        l.extend(crate::twophase::status_lines(self.svc.dir()));
        l
    }
    fn open_bundle(&self, path: &str) -> Result<String, String> {
        if !self.policy.manage_settings {
            return Err("only an administrator can open a system update file".into());
        }
        let (running, variant) = self.running();
        let centre = crate::devmode::centre::Centre::real(self.svc.dir());
        let s = crate::sysupdate::open_for_centre(self.svc.dir(), std::path::Path::new(path), centre.key(), &running, &variant)?;
        (self.audit)("system.update.bundle", &format!("opened {} (assistant)", s.release.version));
        Ok(format!("The signature is good. IYAKKI {} ({}) is unpacked and listed in Settings -> System update; nothing was installed.", s.release.version, s.release.variant))
    }
    fn fetch_bundle(&self, url: &str) -> Result<String, String> {
        if !self.policy.manage_settings {
            return Err("only an administrator can fetch a system update".into());
        }
        let st = self.svc.streamer().ok_or("this computer has no download connection set up")?;
        let (running, variant) = self.running();
        let centre = crate::devmode::centre::Centre::real(self.svc.dir());
        let host = url.trim_start_matches("https://").split('/').next().unwrap_or("").to_string();
        // the person's apps may hear that the download ended, with the fixed name only (F-SDK-207)
        let told = |ok: bool| crate::appevent_more::transfer_finished(&*self.svc.app_events, &self.user, "download", "system update", ok);
        let s = crate::sysupdate::fetch_for_centre(self.svc.dir(), &*st, url, centre.key(), &running, &variant).inspect_err(|_| {
            (self.audit)("system.update.fetch", &format!("refused from {host} (assistant)"));
            told(false);
        })?;
        told(true);
        (self.audit)("system.update.fetch", &format!("opened {} from {host} (assistant)", s.release.version));
        Ok(format!("The signature is good. IYAKKI {} ({}) was downloaded from {host}, is unpacked and listed in Settings -> System update; nothing was installed.", s.release.version, s.release.variant))
    }
    fn save_mode(&self, mode: Mode) -> Result<(), String> {
        (self.audit)("settings.update_check", &format!("mode={mode:?} (assistant)"));
        self.svc.settings.lock().map_err(|_| "the settings are busy; try again".to_string())?.update(|s| s.update_check.mode = mode).map(|_| ()).map_err(|e| e.to_string())
    }
}
