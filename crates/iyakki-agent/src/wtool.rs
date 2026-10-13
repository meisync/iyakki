//! WASM tool packages: a signed, reviewed `.iyp` package of type `tool` carries a WebAssembly module and a
//! manifest, and the agent offers it to the model as the named tool `pkg_<name>` (the model APIs do not allow a
//! dot in tool names).
//!
//! The payload of the package is two lines: the manifest as compact JSON, then the module in hexadecimal.
//!
//! ```json
//! {"name":"word-count","version":"1.0.0","license":"MIT","description":"Count the words in a text",
//!  "schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},
//!  "capabilities":["read:{home}/notes"],
//!  "budget":{"fuel":5000000,"memory_mb":8,"seconds":5}}
//! ```
//!
//! * `capabilities` (default none) are `read:<path>`, `write:<path>` and `net:<host>`; `{home}` stands for the
//!   caller's home folder. A tool with none runs with no files and no network at all: the host functions it
//!   would need refuse every call. Declared ones are issued as sandbox tokens in the order written
//!   (`iyakki.token_get(0)` is the first), only if the caller's own policy allows them too, and the whole call
//!   is shown to the person for approval, capabilities included, every time.
//! * `budget` is a request, never more than the caller's policy allows: `fuel` (instructions), `memory_mb` and
//!   `seconds` of real time. Going over any of them stops the guest with a message saying which.
//!
//! Guest ABI: the module exports `memory` (at least three 64 KiB pages) and `run(input_len: i32) -> i32`. The
//! host writes the call's JSON arguments at [`IN_PTR`] before the call; `run` leaves its answer at [`OUT_PTR`] and
//! returns its length, or a negative number for an error (the sandbox's `errno` codes are shown by name).
//! The module may be a binary `.wasm` or, for development, WAT text. Files a tool may read are copied into the
//! sandbox's memory-only file system before the call; files it wrote under a `write:` scope are copied out
//! afterwards (each path checked again against the caller's policy). The guest never sees the real disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::Upstream;
use iyakki_sec::policy::Policy;
use iyakki_sec::{Capability, CapabilityStore};
use iyakki_vfs::{DiffKind, MemFs};
use iyakki_wasm::{errno, NetBackend, Sandbox, SandboxConfig, WasmError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the host puts the JSON arguments in guest memory.
pub const IN_PTR: u32 = 0x1_0000;
/// Where the guest leaves its answer.
pub const OUT_PTR: u32 = 0x2_0000;
/// Largest input and output, in bytes.
pub const MAX_IO: usize = 0x1_0000;
pub const MAX_MODULE_BYTES: usize = 400 * 1024;
pub const MAX_TOOLS_PER_SCOPE: usize = 50;
const MAX_SCHEMA_BYTES: usize = 8 * 1024;
const STAGE_FILES: usize = 200;
const STAGE_BYTES: usize = 8 << 20;
const WRITE_BACK_FILES: usize = 64;

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

struct Gateway(Arc<dyn Upstream>);

impl NetBackend for Gateway {
    fn fetch(&self, method: &str, url: &str, body: &[u8]) -> Result<Vec<u8>, i32> {
        let r = self.0.request(method, url, &[], body).map_err(|_| errno::NET)?;
        Ok(iyakki_proxy::sanitize(&r.body.into_iter().take(256 * 1024).collect::<Vec<u8>>()))
    }
}

fn errno_name(code: i32) -> &'static str {
    match code {
        errno::BAD_MEMORY => "bad memory address",
        errno::DENIED => "denied: the tool did not declare that capability (or the path/host is outside it)",
        errno::NOT_FOUND => "not found",
        errno::IO => "input/output error",
        errno::BUF_TOO_SMALL => "buffer too small",
        errno::BAD_INDEX => "no such capability index",
        errno::NET => "network request failed",
        _ => "tool error",
    }
}

/// Copy the files under `real` (never following links) into the memory file system.
fn stage(real: &Path, fs: &mut MemFs, files: &mut usize, bytes: &mut usize, depth: u8) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(real).map_err(|e| format!("{}: {e}", real.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_file() {
        *files += 1;
        *bytes += meta.len() as usize;
        if *files > STAGE_FILES || *bytes > STAGE_BYTES {
            return Err(format!("the folder the tool may read is too large (over {STAGE_FILES} files or {} MiB)", STAGE_BYTES >> 20));
        }
        let data = std::fs::read(real).map_err(|e| format!("{}: {e}", real.display()))?;
        fs.write(&real.to_string_lossy(), &data).map_err(|e| e.to_string())?;
    } else if meta.is_dir() && depth < 6 {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(real).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            stage(&p, fs, files, bytes, depth + 1)?;
        }
    }
    Ok(())
}

/// Run the tool for `user` with `args`. Everything the guest can reach is decided here: the capabilities it
/// declared, narrowed by the caller's policy, and the budget it asked for, capped by the policy.
pub fn run(tool: &ToolPackage, user: &str, policy: &Policy, args: &Value, upstream: Arc<dyn Upstream>) -> Result<String, String> {
    let name = &tool.manifest.name;
    check_args(&tool.manifest.schema, args).map_err(|e| format!("{name}: {e}"))?;
    let input = serde_json::to_vec(args).map_err(|e| e.to_string())?;
    if input.len() > MAX_IO {
        return Err(format!("{name}: the arguments are larger than {} KiB", MAX_IO / 1024));
    }
    let home = format!("/home/{user}");
    let store = CapabilityStore::new();
    let (mut tokens, mut vfs) = (Vec::new(), MemFs::with_quota((policy.vfs_quota_mb.clamp(1, 64) as usize) << 20));
    let (mut write_scopes, mut need_net) = (Vec::<String>::new(), false);
    let (mut files, mut bytes) = (0usize, 0usize);
    for c in &tool.caps {
        let issue = |cap: Capability| store.issue(cap).map_err(|e| e.to_string());
        match c {
            Cap::Read(p) | Cap::Write(p) => {
                let writing = matches!(c, Cap::Write(_));
                let real = crate::scope::resolve(Path::new("/"), &p.replace("{home}", &home)).map_err(|e| format!("{name}: {e}"))?;
                let allowed = if writing { crate::scope::check_write(policy, user, &real) } else { crate::scope::check_read(policy, user, &real) };
                allowed.map_err(|e| format!("{name} declares `{}` but your account does not allow it ({e})", if writing { "write" } else { "read" }))?;
                let s = real.to_string_lossy().to_string();
                if writing {
                    write_scopes.push(s.clone());
                    tokens.push(issue(Capability::FsWrite(s))?);
                } else {
                    if real.exists() {
                        stage(&real, &mut vfs, &mut files, &mut bytes, 0).map_err(|e| format!("{name}: {e}"))?;
                    }
                    tokens.push(issue(Capability::FsRead(s))?);
                }
            }
            Cap::Net(h) => {
                if !policy.allows_host(h) {
                    return Err(format!("{name} declares net:{h} but your account may not reach {h}"));
                }
                need_net = true;
                tokens.push(issue(Capability::NetConnect(h.clone()))?);
            }
        }
    }
    let base = vfs.clone();
    let vfs = Arc::new(Mutex::new(vfs));

    let b = &tool.manifest.budget;
    let fuel = b.fuel.min(policy.max_fuel.max(1));
    let mem_mb = b.memory_mb.min(policy.max_memory_mb.max(1));
    let mut sandbox = Sandbox::new(SandboxConfig { fuel, max_memory_bytes: (mem_mb as usize) << 20 }).map_err(|e| format!("{name}: {e}"))?.with_wall_time(Duration::from_secs(b.seconds as u64));
    if need_net {
        sandbox = sandbox.with_net(Arc::new(Gateway(upstream)));
    }
    let mut session = sandbox.session(&tool.module, vfs.clone(), store, tokens).map_err(|e| format!("{name}: {e}"))?;
    session.write_memory(IN_PTR, &input).map_err(|_| format!("{name}: the module must export `memory` of at least 3 pages (192 KiB) and a memory limit that allows it"))?;
    let out = match session.call("run", &[input.len() as i32]) {
        Ok(o) => o,
        Err(WasmError::OutOfFuel) => return Err(format!("{name} was stopped: it used up its CPU budget of {fuel} instructions")),
        Err(WasmError::Timeout) => return Err(format!("{name} was stopped: it ran longer than its time budget of {} s", b.seconds)),
        Err(WasmError::NoEntry(_)) => return Err(format!("{name} is not a valid tool: it must export `run(i32) -> i32`")),
        Err(e) => return Err(format!("{name} failed: {e} (memory limit {mem_mb} MiB)")),
    };
    let log = String::from_utf8_lossy(&out.log).trim().to_string();
    let code = out.result.unwrap_or(-1);
    if code < 0 {
        let more = if log.is_empty() { String::new() } else { format!(": {}", log.chars().take(300).collect::<String>()) };
        return Err(format!("{name} reported an error ({code}, {}){more}", errno_name(code)));
    }
    if code as usize > MAX_IO {
        return Err(format!("{name} returned {code} bytes; the limit is {}", MAX_IO));
    }
    let bytes = session.read_memory(OUT_PTR, code as usize).map_err(|_| format!("{name} returned an output outside its memory"))?;
    let mut text = String::from_utf8_lossy(&bytes).to_string();

    // files the guest wrote under a `write:` scope are copied out, each path checked again
    let after = vfs.lock().unwrap().clone();
    let mut written = BTreeSet::new();
    for (path, kind) in after.diff(&base) {
        if kind == DiffKind::Removed || !write_scopes.iter().any(|s| Capability::FsWrite(s.clone()).permits(&iyakki_sec::Access::FsWrite(&path))) {
            continue;
        }
        if written.len() >= WRITE_BACK_FILES {
            return Err(format!("{name} wrote more than {WRITE_BACK_FILES} files"));
        }
        let target = crate::scope::resolve(Path::new("/"), &path)?;
        crate::scope::check_write(policy, user, &target)?;
        if !write_scopes.iter().any(|s| target.starts_with(s)) {
            return Err(format!("{name} tried to write outside its scope: {path}"));
        }
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink() || m.is_dir()) {
            return Err(format!("{name} tried to write over {}", target.display()));
        }
        std::fs::write(&target, after.read(&path).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", target.display()))?;
        written.insert(target.display().to_string());
    }
    if !written.is_empty() {
        text.push_str(&format!("\n[wrote {}]", written.into_iter().collect::<Vec<_>>().join(", ")));
    }
    Ok(text)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;
    use iyakki_sec::policy::Role;

    /// Echoes the arguments back: copies the input to the output.
    pub const ECHO_WAT: &str = include_str!("../../../tests/fixtures/tool_echo.wat");
    /// Tries to write /home/alice/pwned using token #0 and returns the host's answer (0 or an errno).
    pub const WRITER_WAT: &str = include_str!("../../../tests/fixtures/tool_write.wat");
    /// Spins forever.
    pub const SPIN_WAT: &str = include_str!("../../../tests/fixtures/tool_spin.wat");
    /// Reads /home/alice/in.txt with token #0 and returns it as its output.
    pub const READER_WAT: &str = include_str!("../../../tests/fixtures/tool_read.wat");
    /// Fetches https://example.com/ with token #0.
    pub const FETCH_WAT: &str = include_str!("../../../tests/fixtures/tool_fetch.wat");

    /// Fill in a fixture's `{FILE}` and `{LEN}`.
    pub fn wat(src: &str, file: &Path) -> String {
        let f = file.display().to_string();
        src.replace("{LEN}", &f.len().to_string()).replace("{FILE}", &f)
    }

    pub fn manifest(name: &str, caps: &[&str], budget: Value) -> String {
        serde_json::json!({
            "name": name, "version": "1.0.0", "license": "MIT", "description": "test tool",
            "schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
            "capabilities": caps, "budget": budget
        })
        .to_string()
    }

    pub fn package(name: &str, caps: &[&str], wat: &str) -> ToolPackage {
        let (_, payload) = make_payload(&manifest(name, caps, serde_json::json!({})), wat.as_bytes()).unwrap();
        parse_payload(&payload).unwrap()
    }

    struct Net;
    impl Upstream for Net {
        fn request(&self, m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
            Ok(UpstreamResponse { status: 200, body: format!("{m} {url}").into_bytes() })
        }
    }

    pub fn sandbox_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-wtool-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("alice")).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    fn policy(home: &Path) -> Policy {
        let mut p = Role::User.template();
        p.fs_read = vec![home.display().to_string()];
        p.fs_write = vec![home.display().to_string()];
        p.net_hosts = vec!["example.com".into()];
        p
    }

    #[test]
    fn manifests_are_checked_and_the_payload_round_trips() {
        let (m, payload) = make_payload(&manifest("word-count", &["read:{home}/notes", "net:Example.com"], serde_json::json!({"fuel": 1000})), ECHO_WAT.as_bytes()).unwrap();
        assert_eq!(m.budget, Budget { fuel: 1000, memory_mb: 8, seconds: 5 });
        let t = parse_payload(&payload).unwrap();
        assert_eq!(t.manifest, m);
        assert_eq!(t.caps, vec![Cap::Read("{home}/notes".into()), Cap::Net("example.com".into())]);
        assert_eq!(t.module, ECHO_WAT.as_bytes());
        assert_eq!(t.access_text("alice"), "it may read files in /home/alice/notes, connect to example.com");
        assert_eq!(package("plain", &[], ECHO_WAT).access_text("alice"), "no access to files or the network");
        for (bad, why) in [
            (manifest("Bad Name", &[], serde_json::json!({})), "name:"),
            (manifest("ok", &["read:relative"], serde_json::json!({})), "capability"),
            (manifest("ok", &["read:/a/../etc"], serde_json::json!({})), "capability"),
            (manifest("ok", &["exec:ls"], serde_json::json!({})), "capability"),
            (manifest("ok", &["net:*"], serde_json::json!({})), "capability"),
            (manifest("ok", &[], serde_json::json!({"seconds": 0})), "budget"),
            (manifest("ok", &[], serde_json::json!({"fuel": 99999999999u64})), "budget"),
            (manifest("ok", &[], serde_json::json!({"gpu": 1})), "unknown field"),
            ("{}".to_string(), "manifest"),
        ] {
            let e = make_payload(&bad, ECHO_WAT.as_bytes()).unwrap_err();
            assert!(e.contains(why), "{bad} -> {e}");
        }
        assert!(make_payload(&manifest("ok", &[], serde_json::json!({})), b"hello").is_err(), "not a module");
        assert!(make_payload(&manifest("ok", &[], serde_json::json!({})), b"").is_err());
        let mut noschema: Value = serde_json::from_str(&manifest("ok", &[], serde_json::json!({}))).unwrap();
        noschema["schema"] = serde_json::json!({"type": "string"});
        assert!(make_payload(&noschema.to_string(), ECHO_WAT.as_bytes()).unwrap_err().contains("schema"));
        assert!(parse_payload("one line").is_err() && parse_payload("{}\nzz").is_err());
    }

    #[test]
    fn arguments_are_checked_against_the_schema_before_anything_runs() {
        let s = serde_json::json!({"type": "object", "properties": {"n": {"type": "integer"}, "t": {"type": "string"}}, "required": ["t"]});
        assert!(check_args(&s, &serde_json::json!({"t": "x", "n": 3})).is_ok());
        assert!(check_args(&s, &serde_json::json!({"n": 3})).unwrap_err().contains("missing argument `t`"));
        assert!(check_args(&s, &serde_json::json!({"t": 5})).unwrap_err().contains("must be a string"));
        assert!(check_args(&s, &serde_json::json!([1])).is_err());
    }

    #[test]
    fn a_tool_without_capabilities_runs_and_returns_its_output() {
        let d = sandbox_dir("echo");
        let t = package("echo", &[], ECHO_WAT);
        let out = run(&t, "alice", &policy(&d.join("alice")), &serde_json::json!({"text": "héllo"}), Arc::new(Net)).unwrap();
        assert_eq!(out, r#"{"text":"héllo"}"#);
        assert!(run(&t, "alice", &policy(&d.join("alice")), &serde_json::json!({}), Arc::new(Net)).unwrap_err().contains("missing argument"));
    }

    #[test]
    fn an_undeclared_capability_is_refused_by_the_host_and_nothing_is_written() {
        let d = sandbox_dir("undeclared");
        let home = d.join("alice");
        let writer = wat(WRITER_WAT, &home.join("pwned"));
        // the guest tries to write inside the user's own (writable) home without having declared write
        let t = package("sneaky", &[], &writer);
        let e = run(&t, "alice", &policy(&home), &serde_json::json!({"text": "x"}), Arc::new(Net)).unwrap_err();
        assert!(e.contains("-2") && e.contains("did not declare"), "{e}");
        assert!(!home.join("pwned").exists());
        // declaring a different scope does not help either
        std::fs::create_dir_all(home.join("other")).unwrap();
        let t = package("sneaky", &[&format!("write:{}", home.join("other").display())], &writer);
        let e = run(&t, "alice", &policy(&home), &serde_json::json!({"text": "x"}), Arc::new(Net)).unwrap_err();
        assert!(e.contains("-2"), "{e}");
        assert!(!home.join("pwned").exists());
        // no network without net:, even with a policy that allows the host
        let t = package("fetchy", &[], FETCH_WAT);
        let e = run(&t, "alice", &policy(&home), &serde_json::json!({"text": "x"}), Arc::new(Net)).unwrap_err();
        assert!(e.contains("-2"), "{e}");
    }

    #[test]
    fn declared_capabilities_work_but_only_inside_what_the_callers_policy_allows() {
        let d = sandbox_dir("declared");
        let home = d.join("alice");
        std::fs::write(home.join("in.txt"), "secret notes").unwrap();
        let args = serde_json::json!({"text": "x"});
        // read: the file is copied into the sandbox, the guest returns it
        let r = package("reader", &[&format!("read:{}", home.display())], &wat(READER_WAT, &home.join("in.txt")));
        assert_eq!(run(&r, "alice", &policy(&home), &args, Arc::new(Net)).unwrap(), "secret notes");
        // the same declaration for a user whose policy cannot read there is refused before anything runs
        let mut nope = policy(&home);
        nope.fs_read = vec![d.join("bob").display().to_string()];
        let e = run(&r, "alice", &nope, &args, Arc::new(Net)).unwrap_err();
        assert!(e.contains("does not allow it"), "{e}");
        // write: written files are copied out afterwards
        let w = package("writer", &[&format!("write:{}", home.display())], &wat(WRITER_WAT, &home.join("pwned")));
        let out = run(&w, "alice", &policy(&home), &args, Arc::new(Net)).unwrap();
        assert!(out.contains("pwned"), "{out}");
        assert_eq!(std::fs::read_to_string(home.join("pwned")).unwrap(), "ok");
        // ... unless the policy has no write scope there
        std::fs::remove_file(home.join("pwned")).unwrap();
        let mut ro = policy(&home);
        ro.fs_write.clear();
        assert!(run(&w, "alice", &ro, &args, Arc::new(Net)).unwrap_err().contains("does not allow it"));
        assert!(!home.join("pwned").exists());
        // net: reaches the gateway for the declared host, and a host the policy forbids is refused
        let f = package("fetchy", &["net:example.com"], FETCH_WAT);
        assert_eq!(run(&f, "alice", &policy(&home), &args, Arc::new(Net)).unwrap(), "GET https://example.com/");
        let mut nonet = policy(&home);
        nonet.net_hosts.clear();
        assert!(run(&f, "alice", &nonet, &args, Arc::new(Net)).unwrap_err().contains("may not reach example.com"));
    }

    #[test]
    fn an_over_budget_guest_is_stopped_with_a_clear_error() {
        let d = sandbox_dir("budget");
        let home = d.join("alice");
        let args = serde_json::json!({"text": "x"});
        let spin = |budget: Value| {
            let (_, payload) = make_payload(&manifest("spin", &[], budget), SPIN_WAT.as_bytes()).unwrap();
            parse_payload(&payload).unwrap()
        };
        // CPU
        let e = run(&spin(serde_json::json!({"fuel": 50_000})), "alice", &policy(&home), &args, Arc::new(Net)).unwrap_err();
        assert!(e.contains("CPU budget of 50000"), "{e}");
        // the policy caps what a package may ask for
        let mut small = policy(&home);
        small.max_fuel = 10_000;
        let e = run(&spin(serde_json::json!({"fuel": 1_000_000_000})), "alice", &small, &args, Arc::new(Net)).unwrap_err();
        assert!(e.contains("CPU budget of 10000"), "{e}");
        // wall time
        let t = std::time::Instant::now();
        let mut roomy = policy(&home);
        roomy.max_fuel = u64::MAX / 2;
        let e = run(&spin(serde_json::json!({"fuel": 2_000_000_000u64, "seconds": 1})), "alice", &roomy, &args, Arc::new(Net)).unwrap_err();
        assert!(e.contains("time budget of 1 s"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(10));
        // memory: a module that needs more than its budget cannot even start
        let big = r#"(module (memory (export "memory") 40) (func (export "run") (param i32) (result i32) i32.const 0))"#;
        let (_, payload) = make_payload(&manifest("big", &[], serde_json::json!({"memory_mb": 1})), big.as_bytes()).unwrap();
        let e = run(&parse_payload(&payload).unwrap(), "alice", &policy(&home), &args, Arc::new(Net)).unwrap_err();
        assert!(e.contains("big"), "{e}");
        // a module without the entry point
        let (_, payload) = make_payload(&manifest("noentry", &[], serde_json::json!({})), br#"(module (memory (export "memory") 3))"#).unwrap();
        assert!(run(&parse_payload(&payload).unwrap(), "alice", &policy(&home), &args, Arc::new(Net)).unwrap_err().contains("export `run"));
    }
}
