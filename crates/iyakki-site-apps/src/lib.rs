//! Sites installed as app tiles (F-BROW-100) and the plain message for protected video (F-BROW-101). A site app is a small record:
//! a name and icon (from the page's web app manifest, else its favicon, else the host), a start address, an origin allow-list (any
//! other host opens in the Browser tile), its own profile folder (cookies and storage never shared with the Browser tile) and the
//! capabilities it holds. Removal deletes the profile folder ("Remove and forget sign-ins"). Audit lines name the origin only.
//!
//! This crate decides and stores; it fetches nothing. The page's manifest text and favicon address are handed in by the browser tile
//! (the real engine) or by a test. Sites that need H.264 or AAC are offered the audio and video add-on card (`browser_av`); nothing here
//! downloads it. Sites that need a content decryption module (Widevine and the like) are told plainly that this computer has none: IYAKKI
//! ships no such module, offers no downloader for one and does not emulate one (owner decision 2026-11-18). The person is never led around it.

use std::path::{Path, PathBuf};

use iyakki_shared::avaddon::Addon;
use serde::{Deserialize, Serialize};

/// The most disk one site app's profile may use before the tile asks the person to clear it.
pub const PROFILE_CAP_BYTES: u64 = 512 * 1024 * 1024;
/// The most site apps one person keeps.
pub const MAX_APPS: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteApp {
    /// Folder-safe id: the host and a short hash of the start address.
    pub id: String,
    pub name: String,
    pub start_url: String,
    /// Address of the icon (manifest icon or favicon); empty means the tile draws the first letter.
    #[serde(default)]
    pub icon_url: String,
    /// Origins the tile may load (scheme, host, port); the start origin is always first.
    pub origins: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub apps: Vec<SiteApp>,
}

/// The name a site app's engine profile is made under: the person (hashed, so a long or odd account name cannot make two people share a
/// folder), the site and a fixed word. It never equals a person's own name for the Browser tile's profile.
pub fn profile_user(user: &str, id: &str) -> String {
    let mut h: u32 = 2166136261;
    for b in user.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16777619);
    }
    format!("site-{id}-{h:08x}")
}

/// Where a site app's own cookies and storage live: beside the Browser tile's profile under `web-profile`, under its own name.
pub fn profile_dir(data: &Path, user: &str, id: &str) -> PathBuf {
    data.join("web-profile").join(iyakki_shared::webengine::safe_name(&profile_user(user, id)))
}

/// `scheme://host[:port]` in lower case, for https (and http only on localhost); `None` for anything else.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let auth = rest.split(['/', '?', '#']).next()?;
    if auth.contains('@') || auth.is_empty() {
        return None;
    }
    let auth = auth.to_ascii_lowercase();
    let host = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()).map(|(h, _)| h).unwrap_or(&auth);
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')) {
        return None;
    }
    let local = host == "localhost";
    // the default port is left out, as the engine writes an origin
    let port = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()).map(|(_, p)| p);
    let shown = match (scheme.as_str(), port) {
        ("https", Some("443")) | ("http", Some("80")) => host.to_string(),
        _ => auth.clone(),
    };
    match scheme.as_str() {
        "https" => Some(format!("https://{shown}")),
        "http" if local => Some(format!("http://{shown}")),
        _ => None,
    }
}

pub fn host_of(url: &str) -> String {
    let o = origin_of(url).unwrap_or_default();
    let h = o.split_once("://").map(|x| x.1).unwrap_or("");
    h.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(h).to_string()
}

/// Join an address found in a page or manifest to the address of the document that named it.
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.contains("://") {
        return Some(href.to_string());
    }
    let origin = origin_of(base)?;
    if let Some(r) = href.strip_prefix("//") {
        return Some(format!("{}://{}", origin.split("://").next()?, r));
    }
    if href.starts_with('/') {
        return Some(format!("{origin}{href}"));
    }
    let path = base.split(['?', '#']).next()?.strip_prefix(origin.as_str())?;
    let dir = path.rfind('/').map(|i| &path[..=i]).unwrap_or("/");
    Some(format!("{origin}{dir}{href}"))
}

/// What the page told about itself.
#[derive(Clone, Debug, Default)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
    /// Text of the page's web app manifest, if the engine found and read one.
    pub manifest: Option<String>,
    /// The manifest's address (icon and start addresses are relative to it); the page address when unknown.
    pub manifest_url: Option<String>,
    pub favicon: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub start_url: Option<String>,
    pub icon: Option<String>,
}

#[derive(Deserialize)]
struct RawIcon {
    src: Option<String>,
    sizes: Option<String>,
}
#[derive(Deserialize)]
struct RawManifest {
    name: Option<String>,
    short_name: Option<String>,
    start_url: Option<String>,
    #[serde(default)]
    icons: Vec<RawIcon>,
}

/// Read a web app manifest: the name (short name when `name` is missing), the start address and the best icon (the largest square
/// size up to 512, else the first). Addresses are made absolute against `base`. Bad JSON gives `None`.
pub fn parse_manifest(text: &str, base: &str) -> Option<Manifest> {
    let raw: RawManifest = serde_json::from_str(text.get(..200_000).unwrap_or(text)).ok()?;
    let name = clean_name(raw.name.as_deref().or(raw.short_name.as_deref()).unwrap_or(""));
    let size = |i: &RawIcon| -> u32 { i.sizes.as_deref().unwrap_or("").split_whitespace().filter_map(|s| s.split_once('x')).filter(|(w, h)| w == h).filter_map(|(w, _)| w.parse::<u32>().ok()).filter(|w| *w <= 512).max().unwrap_or(0) };
    let best = raw.icons.iter().filter(|i| i.src.as_deref().is_some_and(|s| !s.trim().is_empty())).max_by_key(|i| size(i));
    Some(Manifest { name, start_url: raw.start_url.as_deref().and_then(|s| resolve(base, s)), icon: best.and_then(|i| resolve(base, i.src.as_deref()?)) })
}

/// A name for a tile: control characters out, at most 40 characters.
pub fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(40).collect()
}

fn slug(host: &str, url: &str) -> String {
    let mut h: u32 = 2166136261;
    for b in url.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16777619);
    }
    let host: String = host.chars().take(40).map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    format!("{host}-{h:08x}")
}

/// Make the record for the page being shown. The name is the person's (when given), else the manifest's, else the page title, else the
/// host; the start address is the manifest's when it stays on the same origin, else the page's own address.
pub fn from_page(page: &PageInfo, person_name: Option<&str>, extra_origins: &[String]) -> Result<SiteApp, String> {
    let page_origin = origin_of(&page.url).ok_or("only an https page can be installed as an app")?;
    let m = page.manifest.as_deref().and_then(|t| parse_manifest(t, page.manifest_url.as_deref().unwrap_or(&page.url)));
    let host = host_of(&page.url);
    let start = m.as_ref().and_then(|m| m.start_url.clone()).filter(|s| origin_of(s).as_deref() == Some(page_origin.as_str())).unwrap_or_else(|| page.url.split('#').next().unwrap_or("").to_string());
    let name = [person_name.map(clean_name), m.as_ref().map(|m| m.name.clone()), Some(clean_name(&page.title)), Some(host.clone())].into_iter().flatten().find(|n| !n.is_empty()).unwrap_or_default();
    let icon = m.as_ref().and_then(|m| m.icon.clone()).or_else(|| page.favicon.as_deref().and_then(|f| resolve(&page.url, f))).unwrap_or_default();
    let mut origins = vec![page_origin];
    for o in extra_origins {
        let o = origin_of(o).ok_or_else(|| format!("{o} is not an https origin"))?;
        if !origins.contains(&o) {
            origins.push(o);
        }
    }
    Ok(SiteApp { id: slug(&host, &start), name, start_url: start, icon_url: icon, origins })
}

impl Store {
    /// Add (or replace the one with the same id). The person's approval happens before this is called.
    pub fn add(&mut self, a: SiteApp) -> Result<(), String> {
        if let Some(x) = self.apps.iter_mut().find(|x| x.id == a.id) {
            *x = a;
            return Ok(());
        }
        if self.apps.len() >= MAX_APPS {
            return Err(format!("{MAX_APPS} site apps are the most; remove one first"));
        }
        self.apps.push(a);
        Ok(())
    }

    pub fn find(&self, name_or_id: &str) -> Option<&SiteApp> {
        let n = name_or_id.trim().to_ascii_lowercase();
        self.apps.iter().find(|a| a.id == n || a.name.to_ascii_lowercase() == n)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let n = clean_name(name);
        if n.is_empty() {
            return Err("the name is empty".into());
        }
        self.apps.iter_mut().find(|a| a.id == id).map(|a| a.name = n).ok_or_else(|| "no such site app".into())
    }

    /// Remove the record and delete its profile folder (cookies, storage, sign-ins). Whether a folder was deleted.
    pub fn remove(&mut self, data: &Path, user: &str, id: &str) -> Result<bool, String> {
        let before = self.apps.len();
        self.apps.retain(|a| a.id != id);
        if self.apps.len() == before {
            return Err("no such site app".into());
        }
        let p = profile_dir(data, user, id);
        Ok(p.exists() && std::fs::remove_dir_all(&p).is_ok())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Loads in the site app's own window.
    InTile,
    /// Another host: opens in the Browser tile.
    BrowserTile,
}

impl SiteApp {
    pub fn route(&self, url: &str) -> Route {
        match origin_of(url) {
            Some(o) if self.origins.contains(&o) => Route::InTile,
            _ => Route::BrowserTile,
        }
    }

    /// What the tile lists as held (shown in the tile's info card and by `site_apps_list`).
    pub fn capabilities(&self) -> Vec<String> {
        vec![format!("network to {}", self.origins.join(", ")), "media playback".into(), "sound".into(), "camera and microphone only when the site asks and the person approves each use".into()]
    }

    /// The audit text: the origin only, never a path, a title or anything typed.
    pub fn audit(&self, what: &str) -> String {
        format!("Site app {what}: {}", self.origins[0])
    }
}

/// Bytes the profile folder uses (a missing folder is 0).
pub fn profile_size(p: &Path) -> u64 {
    let mut n = 0;
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => n += profile_size(&e.path()),
            Ok(m) => n += m.len(),
            Err(_) => {}
        }
    }
    n
}

/// The warning for a profile that has grown past [`PROFILE_CAP_BYTES`]: the person clears it (the chat tool `site_app_remove` forgets
/// the sign-ins too); nothing is deleted for them. `None` while it is under the cap.
pub fn size_note(bytes: u64) -> Option<String> {
    (bytes > PROFILE_CAP_BYTES).then(|| format!("its own cookies and storage use {} MB, over the {} MB this app is meant to keep; clearing it signs the person out of the site", bytes / (1024 * 1024), PROFILE_CAP_BYTES / (1024 * 1024)))
}

/// Hosts whose video needs H.264 or AAC (the add-on). YouTube is not here: it plays VP9 and Opus without it.
const NEEDS_CODECS: [&str; 8] = ["tiktok.com", "amazon.com", "twitch.tv", "vimeo.com", "instagram.com", "facebook.com", "dailymotion.com", "bilibili.com"];

pub fn host_in(host: &str, list: &[&str]) -> bool {
    let h = host.to_ascii_lowercase();
    list.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

/// The card a site app's tile offers when its site usually needs H.264 or AAC and the add-on is not ready: the same text as the browser's
/// bar, with Install leading to Store -> Setup (the person confirms there). `None` when nothing is needed.
pub fn codec_card(app: &SiteApp, addon: &Addon) -> Option<&'static str> {
    if matches!(addon, Addon::Ready) || !host_in(&host_of(&app.start_url), &NEEDS_CODECS) {
        return None;
    }
    Some(iyakki_shared::avaddon::bar_text(addon))
}

/// Sites whose video is sealed by a content decryption module.
const PROTECTED: [&str; 9] = ["netflix.com", "primevideo.com", "disneyplus.com", "hulu.com", "max.com", "hbomax.com", "peacocktv.com", "paramountplus.com", "tv.apple.com"];

/// The plain message shown instead of a black player. It says what is missing and that IYAKKI does not provide it; it names no workaround.
pub const PROTECTED_MESSAGE: &str = "This site protects its video with a content decryption module (such as Widevine). This computer does not have one and IYAKKI does not include or download one, so the video cannot play here. Everything else on the site works, and sites without that protection (YouTube, for example) play normally.";

/// Is this site known to seal its video? (The first check, before the page asks.)
pub fn protected_site(host: &str) -> bool {
    host_in(host, &PROTECTED)
}

/// The page asked for a media key system (`requestMediaKeySystemAccess`): the helper passes the name; any DRM key system means the
/// message is shown. `org.w3.clearkey` needs no module and is not DRM in this sense.
pub fn key_system_needs_module(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    ["com.widevine", "com.microsoft.playready", "com.apple.fps", "com.adobe.primetime", "com.huawei.wiseplay"].iter().any(|k| n.starts_with(k))
}

/// The message for this page, from what is known: the helper's key-system request wins; the site list is the fallback.
pub fn protection_message(host: &str, requested_key_system: Option<&str>) -> Option<&'static str> {
    match requested_key_system {
        Some(k) if key_system_needs_module(k) => Some(PROTECTED_MESSAGE),
        Some(_) => None,
        None if protected_site(host) => Some(PROTECTED_MESSAGE),
        None => None,
    }
}

#[cfg(test)]
mod tests;
