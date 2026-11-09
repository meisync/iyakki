//! A tile project on the developer's disk: the manifest (`iyakki-app.json` with a `tile` section), the tile code (`.wasm` or `.wat`) and
//! the optional emulator settings (`iyakki-emulator.json`). Also the review text the device's install card would show.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Settings for the fake services, from `iyakki-emulator.json` next to the manifest. Every part is optional.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// `settings.get` answers.
    pub settings: Vec<(String, Value)>,
    /// `vault.get` answers by entry name. A name that is not listed gets a made-up value.
    pub secrets: Vec<(String, String)>,
    /// Canned `net.fetch` answers by exact URL: status and body.
    pub net: Vec<(String, u16, String)>,
    /// Calls the emulated person says no to: `vault.get`, `net` or `net.*`.
    pub deny: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("iyakki-emulator.json is not valid JSON: {e}"))?;
        let mut c = Config::default();
        if let Some(o) = v.get("settings").and_then(Value::as_object) {
            c.settings = o.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        if let Some(o) = v.get("secrets").and_then(Value::as_object) {
            c.secrets = o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect();
        }
        if let Some(o) = v.get("net").and_then(Value::as_object) {
            for (url, r) in o {
                let status = r.get("status").and_then(Value::as_u64).unwrap_or(200).min(599) as u16;
                let body = match r.get("body") {
                    Some(Value::String(s)) => s.clone(),
                    Some(b) => b.to_string(),
                    None => String::new(),
                };
                c.net.push((url.clone(), status, body));
            }
        }
        if let Some(a) = v.get("deny").and_then(Value::as_array) {
            c.deny = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        }
        Ok(c)
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    /// What was named on the command line (reload reads it again).
    pub source: PathBuf,
    pub dir: PathBuf,
    pub name: String,
    pub version: String,
    pub abi: u32,
    /// Capabilities the manifest declares: `domain`, `domain.*` or `domain.method`.
    pub caps: Vec<String>,
    pub code: Vec<u8>,
    pub code_path: PathBuf,
    pub config: Config,
}

/// Whether `c` is a well-formed capability: `domain`, `domain.*` or `domain.method`, lower case.
pub fn valid_cap(c: &str) -> bool {
    let word = |w: &str| w.len() <= 32 && w.chars().next().is_some_and(|f| f.is_ascii_lowercase()) && w.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
    match c.split_once('.') {
        None => word(c),
        Some((d, m)) => word(d) && (m == "*" || word(m)),
    }
}

impl Project {
    /// Load a tile folder, or a `.wasm`/`.wat` file with its manifest beside it. A `.iyp` package is explained, not run.
    pub fn load(path: &Path) -> Result<Project, String> {
        if path.extension().is_some_and(|e| e == "iyp") {
            return Err(iyp_note(path));
        }
        let (dir, file) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else if path.is_file() {
            (path.parent().map(Path::to_path_buf).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from(".")), Some(path.to_path_buf()))
        } else {
            return Err(format!("{}: no such tile folder or file", path.display()));
        };
        let mtext = std::fs::read_to_string(dir.join("iyakki-app.json")).map_err(|_| format!("{}: no iyakki-app.json here; a tile needs one with a \"tile\" section ({{\"tile\": {{\"abi\": 1, \"name\": \"...\", \"caps\": []}}}})", dir.display()))?;
        let m: Value = serde_json::from_str(&mtext).map_err(|e| format!("iyakki-app.json is not valid JSON: {e}"))?;
        let t = m.get("tile").and_then(Value::as_object).ok_or("iyakki-app.json has no \"tile\" section: this is not a WASM tile (web-bundle tiles run in a browser; see docs/SDK-EMULATOR.md)")?;
        let folder_name = std::fs::canonicalize(&dir).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "tile".into());
        let name = t.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(&folder_name).to_string();
        let abi = t.get("abi").and_then(Value::as_u64).unwrap_or(1) as u32;
        let version = m.get("version").and_then(Value::as_str).unwrap_or("0.1.0").to_string();
        let mut caps = Vec::new();
        if let Some(a) = t.get("caps") {
            for c in a.as_array().ok_or("tile.caps must be a list of strings")? {
                let c = c.as_str().ok_or("tile.caps must be a list of strings")?;
                if !valid_cap(c) {
                    return Err(format!("tile.caps: {c:?} is not a capability (use domain, domain.* or domain.method in lower case)"));
                }
                caps.push(c.to_string());
            }
        }
        let code_path = match file {
            Some(f) => f,
            None => {
                let named = t.get("file").and_then(Value::as_str);
                let mut found = named.map(|n| dir.join(n));
                if found.is_none() {
                    for n in ["tile.wasm", "tile.wat"] {
                        if dir.join(n).is_file() {
                            found = Some(dir.join(n));
                            break;
                        }
                    }
                }
                found.ok_or_else(|| format!("{}: no tile.wasm or tile.wat (or name the file in tile.file)", dir.display()))?
            }
        };
        if let Some(n) = t.get("file").and_then(Value::as_str) {
            if n.contains("..") || Path::new(n).is_absolute() {
                return Err("tile.file must be a path inside the tile folder".into());
            }
        }
        let code = std::fs::read(&code_path).map_err(|e| format!("{}: {e}", code_path.display()))?;
        let config = match std::fs::read_to_string(dir.join("iyakki-emulator.json")) {
            Ok(t) => Config::parse(&t)?,
            Err(_) => Config::default(),
        };
        Ok(Project { source: path.to_path_buf(), dir, name, version, abi, caps, code, code_path, config })
    }

    /// The files whose change reloads the emulator.
    pub fn watched(&self) -> Vec<PathBuf> {
        vec![self.code_path.clone(), self.dir.join("iyakki-app.json"), self.dir.join("iyakki-emulator.json")]
    }

    pub fn manifest(&self) -> iyakki_tile_host::TileManifest {
        let mut m = iyakki_tile_host::TileManifest::new(&self.name, &[]);
        m.abi = self.abi;
        m.caps = self.caps.clone();
        m
    }

    /// What the device's install card would say about this tile, in the same layout as the store review summary (first line, then
    /// `permissions:` with the words, then `warning:` lines).
    pub fn review_text(&self) -> String {
        let mut s = format!("tile {} {} (new)\n", self.name, self.version);
        if self.caps.is_empty() {
            s.push_str("permissions: none\n");
        } else {
            s.push_str(&format!("permissions: {}\n", self.caps.iter().map(|c| cap_words(c)).collect::<Vec<_>>().join("; ")));
        }
        for c in &self.caps {
            if cap_sensitive(c) {
                s.push_str(&format!("warning: {c} asks you again each time it is used, whatever your autonomy setting\n"));
            }
            if !known_domain(c.split('.').next().unwrap_or("")) {
                s.push_str(&format!("warning: {c} is not a service this emulator knows; calls to it fail here\n"));
            }
        }
        s
    }
}

/// Domains the emulator has fake services for.
pub const DOMAINS: [&str; 10] = ["clock", "settings", "storage", "files", "vault", "net", "notify", "audio", "camera", "chat"];

pub fn known_domain(d: &str) -> bool {
    DOMAINS.contains(&d)
}

/// Sensitive: the person is asked again on the device (vault, network, camera, microphone, writing files).
pub fn cap_sensitive(c: &str) -> bool {
    let (d, m) = c.split_once('.').unwrap_or((c, "*"));
    matches!(d, "vault" | "net" | "camera") || (d == "audio" && matches!(m, "*" | "record")) || (d == "files" && matches!(m, "*" | "write"))
}

/// A capability in plain words. The wording for `storage`, `files` and `notify` is the install card's own for the same permission.
pub fn cap_words(c: &str) -> String {
    let (d, m) = c.split_once('.').unwrap_or((c, "*"));
    let base = match d {
        "clock" => "read the time",
        "settings" => "read the settings that apps may see",
        "storage" => "keep its own saved data on this device (at most 256 KB, only this app can read it)",
        "files" => "open and save files you pick in the file dialog (it never sees other files)",
        "vault" => "use a vault entry (you are asked each time)",
        "net" => "connect to the internet (you are asked each time)",
        "notify" => "leave notes in your inbox",
        "audio" => "play sound or use the microphone",
        "camera" => "use the camera (you are asked each time)",
        "chat" => "ask the assistant questions, each one only after you allow it",
        _ => return format!("call the service {c}"),
    };
    if m == "*" {
        base.to_string()
    } else {
        format!("{base} [only {m}]")
    }
}

/// `.iyp` packages hold a signed text container; a WASM tile has no defined payload in it yet, so the emulator explains instead of guessing.
fn iyp_note(path: &Path) -> String {
    let head = std::fs::read_to_string(path).ok().map(|t| t.lines().take_while(|l| *l != "---").take(8).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    format!("{} is a packed .iyp. The emulator runs an unpacked tile folder (iyakki-app.json plus tile.wasm or tile.wat): keep the folder you pack from and emulate that.{}", path.display(), if head.is_empty() { String::new() } else { format!("\nPackage header:\n{head}") })
}
