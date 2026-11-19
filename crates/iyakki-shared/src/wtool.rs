//! The data of a WebAssembly tool package: manifest, capabilities, budget, the payload format and the checks of both (moved out of
//! `iyakki_agent::wtool`, which keeps the sandbox that runs a tool; F-SPLIT-03).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the host puts the JSON arguments in guest memory.
pub const IN_PTR: u32 = 0x1_0000;
/// Where the guest leaves its answer.
pub const OUT_PTR: u32 = 0x2_0000;
/// Largest input and output, in bytes.
pub const MAX_IO: usize = 0x1_0000;
pub const MAX_MODULE_BYTES: usize = crate::pkg::WASM_MODULE_MAX;
pub const MAX_TOOLS_PER_SCOPE: usize = 50;
pub const MAX_SCHEMA_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(default = "d_fuel")]
    pub fuel: u64,
    #[serde(default = "d_mem")]
    pub memory_mb: u32,
    #[serde(default = "d_secs")]
    pub seconds: u32,
}

fn d_fuel() -> u64 {
    10_000_000
}
fn d_mem() -> u32 {
    8
}
fn d_secs() -> u32 {
    5
}

impl Default for Budget {
    fn default() -> Self {
        Budget { fuel: d_fuel(), memory_mb: d_mem(), seconds: d_secs() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolManifest {
    pub name: String,
    pub version: String,
    pub license: String,
    pub description: String,
    /// JSON schema of the arguments (an object schema).
    pub schema: Value,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub budget: Budget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cap {
    Read(String),
    Write(String),
    Net(String),
}

impl Cap {
    pub fn parse(s: &str) -> Result<Cap, String> {
        let bad = || format!("capability {s:?}: use read:<absolute path>, write:<absolute path> or net:<host>");
        let (kind, arg) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "read" | "write" => {
                let probe = arg.replace("{home}", "/home/x");
                if !probe.starts_with('/') || probe.contains("{") || iyakki_vfs::normalize(&probe).is_err() || probe.split('/').any(|p| p == "..") {
                    return Err(bad());
                }
                Ok(if kind == "read" { Cap::Read(arg.into()) } else { Cap::Write(arg.into()) })
            }
            "net" => {
                let ok = !arg.is_empty() && arg.len() <= 253 && !arg.starts_with(['.', '-']) && arg.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
                if ok { Ok(Cap::Net(arg.to_ascii_lowercase())) } else { Err(bad()) }
            }
            _ => Err(bad()),
        }
    }

    /// Plain words for the approval prompt, with `{home}` filled in.
    pub fn describe(&self, user: &str) -> String {
        let home = format!("/home/{user}");
        match self {
            Cap::Read(p) => format!("read files in {}", p.replace("{home}", &home)),
            Cap::Write(p) => format!("write files in {}", p.replace("{home}", &home)),
            Cap::Net(h) => format!("connect to {h}"),
        }
    }
}

/// A tool package after its hash and signature were checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPackage {
    pub manifest: ToolManifest,
    pub caps: Vec<Cap>,
    pub module: Vec<u8>,
}

impl ToolPackage {
    pub fn capability_lines(&self, user: &str) -> Vec<String> {
        self.caps.iter().map(|c| c.describe(user)).collect()
    }

    pub fn has_read(&self) -> bool {
        self.caps.iter().any(|c| matches!(c, Cap::Read(_)))
    }

    /// One line for approval prompts and lists.
    pub fn access_text(&self, user: &str) -> String {
        if self.caps.is_empty() {
            "no access to files or the network".into()
        } else {
            format!("it may {}", self.capability_lines(user).join(", "))
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

/// Whether the bytes look like a module: binary (`\0asm`) or WAT text.
fn looks_like_module(m: &[u8]) -> bool {
    m.starts_with(b"\0asm") || std::str::from_utf8(m).is_ok_and(|t| t.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with(";;")).is_some_and(|l| l.starts_with("(module")))
}

/// Check a manifest. Returns the parsed capabilities.
pub fn validate(m: &ToolManifest) -> Result<Vec<Cap>, String> {
    if !crate::skills::valid_skill_name(&m.name) {
        return Err("name: lowercase letters, digits and `-`, starting with a letter or digit, at most 48 characters".into());
    }
    if !crate::pkg::valid_version(&m.version) {
        return Err("version: digits separated by dots, like 1.2.0".into());
    }
    if m.license.is_empty() || m.license.len() > 40 || !m.license.chars().all(|c| c.is_ascii_alphanumeric() || "-.+ ".contains(c)) {
        return Err("license: an SPDX name such as MIT or Apache-2.0".into());
    }
    if m.description.trim().is_empty() || m.description.chars().count() > 300 {
        return Err("description is required and must be at most 300 characters".into());
    }
    if m.schema.get("type").and_then(Value::as_str) != Some("object") || m.schema.to_string().len() > MAX_SCHEMA_BYTES {
        return Err(format!("schema: a JSON schema with \"type\": \"object\", at most {} KiB", MAX_SCHEMA_BYTES / 1024));
    }
    if m.capabilities.len() > 8 {
        return Err("at most 8 capabilities".into());
    }
    let caps = m.capabilities.iter().map(|c| Cap::parse(c)).collect::<Result<Vec<_>, _>>()?;
    let b = &m.budget;
    if !(1..=2_000_000_000).contains(&b.fuel) || !(1..=256).contains(&b.memory_mb) || !(1..=60).contains(&b.seconds) {
        return Err("budget: fuel 1..2000000000, memory_mb 1..256, seconds 1..60".into());
    }
    Ok(caps)
}

/// The package payload for a manifest and module.
pub fn make_payload(manifest_json: &str, module: &[u8]) -> Result<(ToolManifest, String), String> {
    let m: ToolManifest = serde_json::from_str(manifest_json).map_err(|e| format!("manifest: {e}"))?;
    validate(&m)?;
    if module.is_empty() || module.len() > MAX_MODULE_BYTES {
        return Err(format!("the module must be 1 byte to {} KiB", MAX_MODULE_BYTES / 1024));
    }
    if !looks_like_module(module) {
        return Err("the module is neither a WebAssembly binary nor WAT text".into());
    }
    let payload = format!("{}\n{}\n", serde_json::to_string(&m).map_err(|e| e.to_string())?, hex(module));
    Ok((m, payload))
}

/// Read a payload back (the package hash was already checked).
pub fn parse_payload(payload: &str) -> Result<ToolPackage, String> {
    let mut lines = payload.lines();
    let (Some(head), Some(body), None) = (lines.next(), lines.next(), lines.next()) else {
        return Err("a tool package holds a manifest line and a module line".into());
    };
    let manifest: ToolManifest = serde_json::from_str(head).map_err(|e| format!("manifest: {e}"))?;
    let caps = validate(&manifest)?;
    let module = unhex(body).filter(|m| !m.is_empty() && m.len() <= MAX_MODULE_BYTES && looks_like_module(m)).ok_or("the module in the package is not valid")?;
    Ok(ToolPackage { manifest, caps, module })
}

/// A small, honest subset of JSON schema: the arguments are an object, required keys are present and declared
/// keys have the declared primitive type. The model is told the full schema; the guest still validates its input.
pub fn check_args(schema: &Value, args: &Value) -> Result<(), String> {
    let obj = args.as_object().ok_or("the arguments must be a JSON object")?;
    for r in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !obj.contains_key(r) {
            return Err(format!("missing argument `{r}`"));
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (k, v) in obj {
            let Some(t) = props.get(k).and_then(|p| p.get("type")).and_then(Value::as_str) else { continue };
            let ok = match t {
                "string" => v.is_string(),
                "integer" => v.is_i64() || v.is_u64(),
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => true,
            };
            if !ok {
                return Err(format!("argument `{k}` must be a {t}"));
            }
        }
    }
    Ok(())
}

