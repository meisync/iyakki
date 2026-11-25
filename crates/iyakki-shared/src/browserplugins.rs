//! Browser plug-ins (F-BROW-90, docs/development/BROWSER-PLUGINS.md): small user scripts, user style sheets and a User-Agent string
//! the person installs for the full web engine. WPE WebKit has no WebExtensions API, so these are IYAKKI's own: a plug-in is a record
//! in a per-person file, the list that is switched on goes to the engine's helper as two hooks of the browser extension snapshot
//! (`content.script`, `user.agent`, see `iyakki_webview::ext`), and the extension host leaves a hook out unless the broker holds its
//! capability. There are none until the person adds one, and every change (add, switch on or off, remove) goes through a chat tool that
//! asks first. A script or style must name the sites it runs on (never every site); a User-Agent applies to every request and is one at a
//! time. The page side (the helper's `WebKitUserContentManager` and `webkit_settings_set_user_agent`) is C and is built with the next
//! helper rebuild; until then the engine ignores the hooks and the tools say so.
//!
//! Limits are the wire's: 16 plug-ins, 32 KiB a body, 256 KiB all together, 8 sites each.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use iyakki_webview::ext::{self, hook, ContentScript, Hook, Pat};
use serde::{Deserialize, Serialize};

use crate::browserext::BrowserExtension;

pub const PLUGINS_ID: &str = "browser-plugins";
pub const MAX_PLUGINS: usize = 16;
pub const MAX_BODY_ALL: usize = 256 * 1024;
pub const MAX_SITES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Script,
    Style,
    UserAgent,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Script => "script",
            Kind::Style => "style",
            Kind::UserAgent => "user agent",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "script" => Some(Kind::Script),
            "style" => Some(Kind::Style),
            "user_agent" | "useragent" | "ua" => Some(Kind::UserAgent),
            _ => None,
        }
    }
    /// The capability the engine's hook needs.
    pub fn capability(self) -> &'static str {
        match self {
            Kind::Script | Kind::Style => "web.content.script",
            Kind::UserAgent => "web.user.agent",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Sites a script or style runs on: `example.com` (that host) or `*.example.com` (it and its subdomains). Empty for a User-Agent.
    #[serde(default)]
    pub sites: Vec<String>,
    /// The script, the style sheet or the User-Agent string.
    pub body: String,
    /// Scripts and styles: at the end of the document instead of the start.
    #[serde(default)]
    pub at_end: bool,
    #[serde(default)]
    pub enabled: bool,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plugin({} {} {:?}, {} bytes)", self.id, self.kind.word(), self.name, self.body.len())
    }
}

/// A site entry as a wire pattern: `example.com` or `*.example.com`; anything wider or odd is refused.
pub fn site_pattern(s: &str) -> Result<Pat, String> {
    let s = s.trim().to_ascii_lowercase();
    let (suffix, host) = match s.strip_prefix("*.") {
        Some(h) => (true, h),
        None => (false, s.as_str()),
    };
    let ok_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.';
    let labels: Vec<&str> = host.split('.').collect();
    if host.is_empty() || !host.chars().all(ok_char) || labels.iter().any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-')) {
        return Err(format!("{s:?} is not a site: write example.com or *.example.com"));
    }
    if suffix && labels.len() < 2 {
        return Err(format!("{s:?} would match a whole top-level domain; name a site, for example *.example.com"));
    }
    if !suffix && labels.len() < 2 && host != "localhost" {
        return Err(format!("{s:?} is not a site: write example.com or *.example.com"));
    }
    Ok(Pat { scheme: 0, suffix, host: host.to_string(), port: 0, segs: Vec::new() })
}

/// The refusals, in plain words. Run on every add and again when a file is read.
pub fn check(p: &Plugin) -> Result<(), String> {
    let name = p.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_control()) {
        return Err("Give the plug-in a name of 1 to 64 letters.".into());
    }
    match p.kind {
        Kind::Script | Kind::Style => {
            if p.sites.is_empty() {
                return Err("A script or style must name the sites it runs on (for example example.com or *.example.com); it never runs on every site.".into());
            }
            if p.sites.len() > MAX_SITES {
                return Err(format!("At most {MAX_SITES} sites for one plug-in."));
            }
            for s in &p.sites {
                site_pattern(s)?;
            }
            if p.body.trim().is_empty() {
                return Err("The plug-in has no text.".into());
            }
            if p.body.len() > ext::MAX_BODY {
                return Err(format!("The text is longer than {} KiB.", ext::MAX_BODY / 1024));
            }
            if p.body.contains('\0') {
                return Err("The text has a NUL byte.".into());
            }
        }
        Kind::UserAgent => {
            if !p.sites.is_empty() {
                return Err("A User-Agent applies to every site the browser opens; leave the sites out.".into());
            }
            let b = p.body.trim();
            if b.len() < 8 || b.len() > 300 || !b.chars().all(|c| (' '..='~').contains(&c)) {
                return Err("A User-Agent is 8 to 300 printable ASCII characters on one line.".into());
            }
        }
    }
    Ok(())
}

/// One person's plug-ins in a file next to their other browser settings.
pub struct PluginStore {
    path: PathBuf,
    items: RwLock<Vec<Plugin>>,
}

impl PluginStore {
    /// Opens the file (missing or damaged: no plug-ins, the damaged file is left alone until the next change).
    pub fn open(path: &Path) -> Arc<PluginStore> {
        let items: Vec<Plugin> = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Plugin>>(&t).ok())
            .map(|v| v.into_iter().filter(|p| check(p).is_ok()).take(MAX_PLUGINS).collect())
            .unwrap_or_default();
        Arc::new(PluginStore { path: path.to_path_buf(), items: RwLock::new(items) })
    }

    pub fn list(&self) -> Vec<Plugin> {
        self.items.read().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn find(&self, which: &str) -> Option<Plugin> {
        let v = self.list();
        v.iter().find(|p| p.id == which).or_else(|| v.iter().find(|p| p.name.eq_ignore_ascii_case(which.trim()))).cloned()
    }

    fn save(&self, v: &[Plugin]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&self.path, text.as_bytes(), 0o600).map_err(|e| format!("could not save the plug-ins: {e}"))
    }

    fn change(&self, f: impl FnOnce(&mut Vec<Plugin>) -> Result<Plugin, String>) -> Result<Plugin, String> {
        let mut w = self.items.write().map_err(|_| "plug-ins are busy".to_string())?;
        let mut next = w.clone();
        let out = f(&mut next)?;
        let total: usize = next.iter().map(|p| p.body.len()).sum();
        if total > MAX_BODY_ALL {
            return Err(format!("The plug-ins together may hold {} KiB of text.", MAX_BODY_ALL / 1024));
        }
        self.save(&next)?;
        *w = next;
        Ok(out)
    }

    /// Adds a plug-in (off until switched on by `set_enabled`); the id is made here.
    pub fn add(&self, mut p: Plugin) -> Result<Plugin, String> {
        p.name = p.name.trim().to_string();
        p.sites = p.sites.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
        check(&p)?;
        p.enabled = false;
        self.change(|v| {
            if v.len() >= MAX_PLUGINS {
                return Err(format!("At most {MAX_PLUGINS} plug-ins; remove one first."));
            }
            if v.iter().any(|x| x.name.eq_ignore_ascii_case(&p.name)) {
                return Err(format!("There is already a plug-in called {}.", p.name));
            }
            let n = v.iter().filter_map(|x| x.id.strip_prefix("pl").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0) + 1;
            p.id = format!("pl{n}");
            v.push(p.clone());
            Ok(p.clone())
        })
    }

    /// Switch one on or off. Only one User-Agent is on at a time: switching one on switches the other off.
    pub fn set_enabled(&self, id: &str, on: bool) -> Result<Plugin, String> {
        self.change(|v| {
            let kind = v.iter().find(|p| p.id == id).map(|p| p.kind).ok_or_else(|| format!("There is no plug-in {id}."))?;
            if on && kind == Kind::UserAgent {
                for p in v.iter_mut().filter(|p| p.kind == Kind::UserAgent) {
                    p.enabled = false;
                }
            }
            let p = v.iter_mut().find(|p| p.id == id).ok_or_else(|| "gone".to_string())?;
            p.enabled = on;
            Ok(p.clone())
        })
    }

    pub fn remove(&self, id: &str) -> Result<Plugin, String> {
        self.change(|v| {
            let i = v.iter().position(|p| p.id == id).ok_or_else(|| format!("There is no plug-in {id}."))?;
            Ok(v.remove(i))
        })
    }
}

/// What the engine is given now: the enabled plug-ins of the signed-in person (set at sign-in and after each change).
static ACTIVE: RwLock<Vec<Plugin>> = RwLock::new(Vec::new());

/// Make these the plug-ins the full engine gets, and tell the extension host something changed.
pub fn activate(list: Vec<Plugin>) {
    let on: Vec<Plugin> = list.into_iter().filter(|p| p.enabled && check(p).is_ok()).collect();
    if let Ok(mut w) = ACTIVE.write() {
        if *w == on {
            return;
        }
        *w = on;
    }
    crate::browserext::host().changed();
}

pub fn active() -> Vec<Plugin> {
    ACTIVE.read().map(|v| v.clone()).unwrap_or_default()
}

/// The hooks of a list of plug-ins (enabled ones only).
pub fn hooks_for(list: &[Plugin]) -> Vec<Hook> {
    let mut scripts = Vec::new();
    let mut ua = None;
    for p in list.iter().filter(|p| p.enabled) {
        match p.kind {
            Kind::Script | Kind::Style => {
                let domains: Result<Vec<Pat>, String> = p.sites.iter().map(|s| site_pattern(s)).collect();
                if let Ok(domains) = domains {
                    scripts.push(ContentScript { style: p.kind == Kind::Style, at_end: p.at_end, domains, body: p.body.clone() });
                }
            }
            Kind::UserAgent => ua = Some(p.body.trim().to_string()),
        }
    }
    let mut out = Vec::new();
    if !scripts.is_empty() {
        out.push(Hook { kind: hook::CONTENT_SCRIPT, payload: ext::encode_content_scripts(&scripts) });
    }
    if let Some(ua) = ua {
        out.push(Hook { kind: hook::USER_AGENT, payload: ext::encode_user_agent(&ua) });
    }
    out
}

/// The plug-ins as one built-in browser extension: it asks only for the capabilities of the plug-ins that are on.
pub struct PluginsExtension;

impl BrowserExtension for PluginsExtension {
    fn id(&self) -> &str {
        PLUGINS_ID
    }
    fn version(&self) -> &str {
        "1"
    }
    fn capabilities(&self) -> Vec<String> {
        let mut caps: Vec<String> = active().iter().map(|p| p.kind.capability().to_string()).collect();
        caps.sort();
        caps.dedup();
        caps
    }
    fn hooks(&self) -> Vec<Hook> {
        hooks_for(&active())
    }
}

#[cfg(test)]
#[path = "browserplugins_tests.rs"]
mod tests;
