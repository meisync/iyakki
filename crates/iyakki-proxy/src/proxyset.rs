//! The browser proxy setting on disk (F-BROX-22): one text, `host:port`, in `<data>/browser-proxy.json`. The shell reads it when it builds
//! the browser's network; the chat tools (`iyakki-agent::browserproxy`) write it. No sign-in is kept (D-BROXY-AUTH).

use std::path::{Path, PathBuf};

use crate::h2client::Proxy;
use crate::https::parse_proxy;

fn file(data: &Path) -> PathBuf {
    data.join("browser-proxy.json")
}

/// The proxy text that is set ("" when there is none or the file is unreadable).
pub fn text(data: &Path) -> String {
    std::fs::read_to_string(file(data)).ok().and_then(|t| t.trim().strip_prefix("{\"proxy\":\"")?.strip_suffix("\"}").map(str::to_string)).filter(|t| t.len() <= 300 && !t.contains(['"', '\\'])).unwrap_or_default()
}

/// The proxy the browser's network goes through, or None (also None when the saved text is no longer valid).
pub fn load(data: &Path) -> Option<Proxy> {
    let t = text(data);
    (!t.is_empty()).then(|| parse_proxy(&t).ok()).flatten().filter(|p| p.creds.is_none())
}

/// Save a proxy text (checked first; no sign-in; empty clears). Returns the text now kept.
pub fn save(data: &Path, proxy: &str) -> Result<String, String> {
    let t = proxy.trim();
    if t.is_empty() {
        return match std::fs::remove_file(file(data)) {
            Ok(()) => Ok(String::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.to_string()),
        };
    }
    let p = parse_proxy(t)?;
    if p.creds.is_some() {
        return Err("a proxy that needs a name and password is not supported here yet; use host:port only".into());
    }
    let kept = format!("{}:{}", p.host, p.port);
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    let tmp = data.join("browser-proxy.json.tmp");
    std::fs::write(&tmp, format!("{{\"proxy\":\"{kept}\"}}")).and_then(|_| std::fs::rename(&tmp, file(data))).map_err(|e| e.to_string())?;
    Ok(kept)
}

