//! The chat tools for the optional browser package (F-BROW-40): `browser_package_status`, `browser_package_install` and
//! `browser_package_remove`. The logic is in [`crate::browserpkg`] (signed description, size and checksum checks, switch last); this file
//! only reaches the store the person pinned and asks first. Install and remove ALWAYS ask (the install card says size, permissions and
//! what it replaces); the package is fetched only from a store source the person configured with a pinned key, never from a built-in
//! address (the official store address is empty until the owner publishes it).

use std::path::Path;
use std::sync::Arc;

use iyakki_sec::policy::{App, Policy};
use iyakki_shared::store_catalog::{Config, Fetch, HttpFetch, SourceKind};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::browserpkg::{self, Source, MAX_SIZE};
use crate::chat::ToolSpec;
use crate::services::Services;

pub fn is_package_tool(name: &str) -> bool {
    matches!(name, "browser_package_status" | "browser_package_install" | "browser_package_remove")
}

fn offered(p: &Policy) -> bool {
    p.apps.contains(&App::Store) && p.apps.contains(&App::Browser)
}

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !offered(p) {
        return vec![];
    }
    let obj = |props: Value| json!({"type": "object", "properties": props, "required": []});
    let tool = |name: &str, desc: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false };
    vec![
        tool("browser_package_status", "Whether the full web engine (the optional browser package, not in the base image) is installed on this computer, which version, and what it may do. With `check` true it also reads the card from the store the person pinned (version, size, what it replaces); that needs the network and a store source with a pinned key (Settings, Store). Read-only.", obj(json!({"check": {"type": "boolean"}})), Risk::Safe),
        tool("browser_package_install", "Install or update the full web engine from the store the person pinned, after the person approves the card (size, the permissions network and sound only, what it replaces). The signed description, size and checksum are checked before anything is kept; a failed install leaves the old version. This changes the system and ALWAYS asks.", obj(json!({})), Risk::Ask),
        tool("browser_package_remove", "Remove the optional web engine package (the image's own engine, if any, stays). The person's browser profile is not touched. ALWAYS asks.", obj(json!({})), Risk::Ask),
    ]
}

pub fn describe(name: &str) -> Option<String> {
    match name {
        "browser_package_status" => Some("look at the optional browser package".into()),
        "browser_package_install" => Some("install or update the full web engine from your store: about 90 MB, it may reach the network and play sound and nothing else, it is listed in the task manager and can be removed".into()),
        "browser_package_remove" => Some("remove the optional full web engine package".into()),
        _ => None,
    }
}

/// The package source of a store the person pinned: the base address and the pinned key of the first enabled, pinned web source.
pub fn pinned_source(cfg: &Config) -> Option<(String, String)> {
    cfg.sources.iter().find(|s| s.enabled && s.kind == SourceKind::Url && !s.pin.trim().is_empty() && s.location.starts_with("https://")).map(|s| (crate::store::compmanifest::base_of(&s.location), s.pin.trim().to_string()))
}

/// The package files of a store: `<base>/browser/<arch>/manifest.json` and `.../webview-helper`.
pub struct StoreSource {
    pub fetch: Arc<dyn Fetch>,
    pub base: String,
    pub arch: String,
}

impl Source for StoreSource {
    fn manifest(&self) -> Result<Value, String> {
        let b = self.fetch.get(&format!("{}/browser/{}/manifest.json", self.base, self.arch), 64 << 10)?;
        serde_json::from_slice(&b).map_err(|_| "the store's package description is not readable".to_string())
    }

    fn download(&self, dest: &Path) -> Result<(), String> {
        let b = self.fetch.get(&format!("{}/browser/{}/webview-helper", self.base, self.arch), MAX_SIZE as usize)?;
        std::fs::write(dest, b).map_err(|e| e.to_string())
    }
}

/// The package files of a store read through a streaming transport (the install job has no `Services`): the description is small and read in
/// full; the program is written to its file as it arrives and never beyond [`MAX_SIZE`] (the caller checks size and checksum after).
pub struct StreamSource {
    pub streamer: Arc<dyn iyakki_proxy::Streamer>,
    pub base: String,
    pub arch: String,
}

impl StreamSource {
    fn open(&self, file: &str) -> Result<Box<dyn std::io::Read + Send>, String> {
        let url = format!("{}/browser/{}/{file}", self.base, self.arch);
        let h = self.streamer.open(&url, &[]).map_err(|e| format!("could not reach the store: {e}"))?;
        if h.status != 200 {
            return Err(format!("the store answered {} for {file}", h.status));
        }
        Ok(h.body)
    }
}

impl Source for StreamSource {
    fn manifest(&self) -> Result<Value, String> {
        use std::io::Read;
        let mut b = Vec::new();
        self.open("manifest.json")?.take(64 << 10).read_to_end(&mut b).map_err(|e| e.to_string())?;
        serde_json::from_slice(&b).map_err(|_| "the store's package description is not readable".to_string())
    }

    fn download(&self, dest: &Path) -> Result<(), String> {
        use std::io::Read;
        let mut f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut self.open("webview-helper")?.take(MAX_SIZE + 1), &mut f).map_err(|e| format!("the download was cut: {e}"))?;
        Ok(())
    }
}

/// The install of the Store's job: the first enabled store source with a pinned key (never a built-in address), read through `streamer`.
pub fn install_with_streamer(data: &Path, streamer: Arc<dyn iyakki_proxy::Streamer>) -> Result<String, String> {
    let cfg = Config::load(&data.join("store"));
    let (base, pin) = pinned_source(&cfg).ok_or_else(|| "no store with a pinned key is set up (Settings, Store): the browser package is fetched only from one the person trusts".to_string())?;
    let arch = std::env::consts::ARCH;
    let src = StreamSource { streamer, base, arch: arch.into() };
    browserpkg::install(data, &src, &pin, arch, true)
}

/// What the Store card says about the package the pinned store offers (its version and size, what it replaces), read through `fetch`: the
/// card's own lines, or the plain reason the store could not be read. A network read: callers run it off the screen's thread.
pub fn card_lines_with(data: &Path, fetch: Arc<dyn Fetch>) -> Result<Vec<String>, String> {
    let cfg = Config::load(&data.join("store"));
    let (base, pin) = pinned_source(&cfg).ok_or_else(|| "no store with a pinned key is set up (Settings, Store)".to_string())?;
    let arch = std::env::consts::ARCH;
    browserpkg::card(data, &StoreSource { fetch, base, arch: arch.into() }, &pin, arch).map(|c| c.lines)
}

/// [`card_lines_with`] over the person's transport (the Store page's card).
pub fn card_lines(data: &Path, upstream: Arc<dyn iyakki_proxy::Upstream>) -> Result<Vec<String>, String> {
    card_lines_with(data, Arc::new(HttpFetch(upstream)))
}

/// The tool, against any source (the real one comes from [`call`]).
pub fn run(name: &str, args: &Value, data: &Path, src: Option<(&dyn Source, &str)>, arch: &str) -> Result<String, String> {
    let no_store = || "no store with a pinned key is set up (Settings, Store): the browser package is fetched only from one the person trusts".to_string();
    match name {
        "browser_package_status" => {
            let mut out = match browserpkg::task_entry(data, false) {
                Some(t) => format!("The full web engine {} is installed; it may use: {}.", t.version.unwrap_or_default(), t.permissions.join(", ")),
                None => "The full web engine package is not installed.".to_string(),
            };
            if args.get("check").and_then(Value::as_bool) == Some(true) {
                match src {
                    Some((s, pin)) => match browserpkg::card(data, s, pin, arch) {
                        Ok(c) => out.push_str(&format!("\n{}", c.lines.join(" "))),
                        Err(e) => out.push_str(&format!("\nThe store could not be read: {e}")),
                    },
                    None => out.push_str(&format!("\n{}", no_store())),
                }
            }
            Ok(out)
        }
        "browser_package_install" => {
            let (s, pin) = src.ok_or_else(no_store)?;
            // the tool only runs after the person approved, so `approved` is the person's own answer
            let v = browserpkg::install(data, s, pin, arch, true)?;
            Ok(format!("The full web engine {v} is installed. A browser tab opened from now on can use it."))
        }
        "browser_package_remove" => {
            if browserpkg::installed_version(data).is_none() {
                return Ok("The package was not installed; nothing to remove.".into());
            }
            browserpkg::remove(data)?;
            Ok("The optional web engine package is removed.".into())
        }
        _ => Err(format!("unknown tool `{name}`")),
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, _user: &str) -> Result<String, String> {
    if !offered(p) {
        return Err("your account may not install or remove the browser package".into());
    }
    let cfg = Config::load(&svc.dir().join("store"));
    let arch = std::env::consts::ARCH;
    let real = pinned_source(&cfg).map(|(base, pin)| (StoreSource { fetch: Arc::new(HttpFetch(svc.quick_upstream())), base, arch: arch.into() }, pin));
    run(name, args, svc.dir(), real.as_ref().map(|(s, pin)| (s as &dyn Source, pin.as_str())), arch)
}

#[cfg(test)]
#[path = "browserpkg_tools_tests.rs"]
mod tests;
