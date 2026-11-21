//! The lead assistant's tools for the team's shared build store (docs/development/BUILD-SHARING.md): `builds_status` (read-only) shows the
//! newest build per machine type the store holds, from its signed list; `builds_store_set` (asks) remembers which store, which update public
//! key and which read-token file to use; `builds_fetch` (hard ask, never a session grant) downloads the newest build of one machine type into
//! a folder, checking every file against the signed list. Nothing here installs anything on a device (`iyakki dev update` does, with its own
//! approval), and publishing stays a script run by the person who holds the update key: the assistant never sees that key or the tokens.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::store::buildshare::{self as bs, Build, List};
pub use iyakki_shared::buildtools::{PROMPT_RULE};

pub const STATUS_TOOL: &str = "builds_status";
pub const SET_TOOL: &str = "builds_store_set";
pub const FETCH_TOOL: &str = "builds_fetch";
pub const INSTALL_TOOL: &str = "builds_install";

pub fn is_builds_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL || n == FETCH_TOOL || n == INSTALL_TOOL
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cfg {
    /// Base address, for example `http://192.168.1.5:30580`.
    pub url: String,
    /// The update public key (64 hex characters) the list must be signed by.
    pub key: String,
    /// A file holding the read token (one line); empty when the store needs none.
    #[serde(default)]
    pub token_file: String,
}

pub trait Backend: Send + Sync {
    fn config(&self) -> Option<Cfg>;
    fn set_config(&self, c: &Cfg) -> Result<(), String>;
    /// The store's list, checked against the pinned key (network).
    fn list(&self, c: &Cfg) -> Result<List, String>;
    /// Download and check one build into `dir`; the sentence to report.
    fn fetch(&self, c: &Cfg, b: &Build, dir: &Path) -> Result<String, String>;
    fn out_root(&self) -> PathBuf;
    /// This device's own machine type (its image variant and CPU family, `pc-aarch64`), when it can say. Used when a tool is not given one.
    fn machine_type(&self) -> Option<String> {
        None
    }
    /// The CPU family of this device (`x86_64`, `aarch64`), when it can say. An install of a build for another family is refused.
    fn cpu(&self) -> Option<String> {
        None
    }
    /// The SHA-256 of every layer this device holds (`None`: a single-root device, or not known). Decides which layered build fits.
    fn held_layers(&self) -> Option<std::collections::BTreeSet<String>> {
        None
    }
    /// What installing the fetched build in `dir` would change, in plain lines (for the card). Nothing is written.
    fn install_preview(&self, _dir: &Path) -> Result<Vec<String>, String> {
        Err("this machine cannot install a system update".into())
    }
    /// Install the fetched build in `dir` through the device's own update path. Does not restart.
    fn install(&self, _dir: &Path, _who: &str) -> Result<String, String> {
        Err("this machine cannot install a system update".into())
    }
}

fn token_headers(c: &Cfg) -> Result<Vec<(String, String)>, String> {
    if c.token_file.is_empty() {
        return Ok(vec![]);
    }
    let t = std::fs::read_to_string(&c.token_file).map_err(|_| "the read-token file cannot be read".to_string())?;
    Ok(vec![("Authorization".into(), format!("Bearer {}", t.trim()))])
}

/// Get one blob into `dest` (through a `.part` file), refusing a size or SHA-256 that differs from the signed list.
pub fn download_file(st: &dyn iyakki_proxy::Streamer, base: &str, h: &[(String, String)], sha: &str, size: u64, dest: &Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let r = st.open(&format!("{base}/v1/blobs/{sha}"), h)?;
    match r.status {
        200 => {}
        401 | 403 => return Err("the store refused the read token".into()),
        404 => return Err("a file is gone: the build was replaced while fetching; ask again for the newest".into()),
        s => return Err(format!("the store answered {s} for a file")),
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let (mut hasher, mut n, mut buf, mut body) = (Sha256::new(), 0u64, vec![0u8; 1 << 16], r.body);
    loop {
        let k = body.read(&mut buf).map_err(|e| format!("download cut: {e}"))?;
        if k == 0 {
            break;
        }
        n += k as u64;
        if n > size {
            let _ = std::fs::remove_file(&part);
            return Err("a file is larger than the signed list says: refused".into());
        }
        hasher.update(&buf[..k]);
        f.write_all(&buf[..k]).map_err(|e| e.to_string())?;
    }
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if n != size || got != sha {
        let _ = std::fs::remove_file(&part);
        return Err("a downloaded file does not match the signed list: refused".into());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
}

impl RealBackend {
    fn cfg_path(&self) -> PathBuf {
        self.svc.dir().join("build-store.json")
    }
}

impl Backend for RealBackend {
    fn config(&self) -> Option<Cfg> {
        std::fs::read(self.cfg_path()).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }
    fn set_config(&self, c: &Cfg) -> Result<(), String> {
        let p = self.cfg_path();
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(c).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| e.to_string())
    }
    fn list(&self, c: &Cfg) -> Result<List, String> {
        let r = self.svc.quick_upstream().request("GET", &format!("{}{}", c.url, bs::PATH), &token_headers(c)?, &[]).map_err(|e| format!("could not reach the build store: {e}"))?;
        match r.status {
            200 => bs::verify(&r.body, &c.key),
            401 | 403 => Err("the store refused the read token".into()),
            404 => Err("the store has no builds yet".into()),
            s => Err(format!("the store answered {s}")),
        }
    }
    fn fetch(&self, c: &Cfg, b: &Build, dir: &Path) -> Result<String, String> {
        let st = iyakki_proxy::https::download_streamer();
        let h = token_headers(c)?;
        for f in &b.files {
            let dest = dir.join(&f.path);
            if dest.metadata().is_ok_and(|m| m.len() == f.size) && crate::devmode::files::sha256_file(&dest).is_ok_and(|s| s == f.sha256) {
                continue;
            }
            download_file(st.as_ref(), &c.url, &h, &f.sha256, f.size, &dest)?;
        }
        bs::verify_dir(dir, b)?;
        Ok(format!("Fetched build {} ({} files, {}) into {} and checked every file against the signed list and the pack's own check.", b.build_id, b.files.len(), crate::store::components::human(b.total), dir.display()))
    }
    fn out_root(&self) -> PathBuf {
        self.svc.dir().join("local").join("builds")
    }
    fn machine_type(&self) -> Option<String> {
        let rel = iyakki_shared::relinfo::ReleaseInfo::load(Path::new("/etc/iyakki-release"))?;
        bs::machine_type_for(&rel.variant, std::env::consts::ARCH)
    }
    fn cpu(&self) -> Option<String> {
        Some(std::env::consts::ARCH.to_string())
    }
    fn held_layers(&self) -> Option<std::collections::BTreeSet<String>> {
        let c = crate::devmode::centre::Centre::real(self.svc.dir());
        let vol = c.boot_read().ok()?;
        crate::devmode::update_layers::device_set(self.svc.dir(), &vol.root).map(|s| s.layers.iter().map(|l| l.sha256.clone()).collect())
    }
    fn install_preview(&self, dir: &Path) -> Result<Vec<String>, String> {
        let c = crate::devmode::centre::Centre::real(self.svc.dir());
        let f = crate::devmode::centre::read_pack(dir, c.key())?;
        Ok(c.preview(&f)?.lines())
    }
    fn install(&self, dir: &Path, who: &str) -> Result<String, String> {
        let c = crate::devmode::centre::Centre::real(self.svc.dir());
        let f = crate::devmode::centre::read_pack(dir, c.key())?;
        // the person's yes to the card (which listed every line that needs one) is the confirmation; a pack whose signature is WRONG is never installed
        c.install(&f, true, who)
    }
}

pub struct BuildTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn sarg<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

impl BuildTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> BuildTools {
        BuildTools { inner, backend }
    }

    fn need_cfg(&self) -> Result<Cfg, String> {
        self.backend.config().ok_or_else(|| "No build store is set. Ask the person for its address and the update public key, then use builds_store_set.".to_string())
    }

    fn pick(&self, a: &Value) -> Result<(Cfg, String, String), String> {
        // without a machine type the device's own is used (its image variant and CPU family)
        let own = self.backend.machine_type();
        let v = if sarg(a, "machine_type").is_empty() { own.as_deref().unwrap_or("") } else { sarg(a, "machine_type") };
        let ch = if sarg(a, "channel").is_empty() { "dev" } else { sarg(a, "channel") };
        if !bs::valid_variant(v) || !bs::valid_channel(ch) {
            return Err("say the machine type (for example pc-x86_64 or pc-aarch64) and, if not dev, the channel".into());
        }
        Ok((self.need_cfg()?, v.to_string(), ch.to_string()))
    }

    fn status(&self) -> Result<String, String> {
        let c = self.need_cfg()?;
        let l = self.backend.list(&c)?;
        if l.builds.is_empty() {
            return Ok("The store holds no builds yet.".into());
        }
        let mut out = vec![format!("{} holds {} build(s); only the newest per machine type and channel is kept:", c.url, l.builds.len())];
        for b in &l.builds {
            out.push(format!("- {} / {}: {} (version {}, commit {}, {}, {} files)", b.variant, b.channel, b.build_id, b.version, b.commit.chars().take(12).collect::<String>(), crate::store::components::human(b.total), b.files.len()));
        }
        Ok(out.join("\n"))
    }

    fn set(&self, a: &Value) -> Result<String, String> {
        let c = Cfg { url: sarg(a, "url").trim_end_matches('/').to_string(), key: sarg(a, "update_public_key").to_ascii_lowercase(), token_file: sarg(a, "read_token_file").to_string() };
        self.set_check(&c)?;
        self.backend.set_config(&c)?;
        Ok(format!("Build store set to {}. The list will be accepted only when it is signed by key {}.", c.url, c.key))
    }

    fn set_check(&self, c: &Cfg) -> Result<(), String> {
        if !(c.url.starts_with("http://") || c.url.starts_with("https://")) {
            return Err("the store address starts with http:// or https://".into());
        }
        if c.key.len() != 64 || !c.key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("the update public key is 64 hex characters".into());
        }
        Ok(())
    }

    fn target(&self, c_ch: &(Cfg, String, String)) -> PathBuf {
        self.backend.out_root().join(format!("{}-{}", c_ch.1, c_ch.2))
    }

    fn card(&self, a: &Value) -> Result<String, String> {
        let p = self.pick(a)?;
        let l = self.backend.list(&p.0)?;
        let b = self.chosen(&l, &p)?;
        Ok(format!("Download build {} for {} (channel {}): {} files, {}, version {}.\nInto {}. Every file is checked against the store's signed list. Nothing is installed.", b.build_id, p.1, p.2, b.files.len(), crate::store::components::human(b.total), b.version, self.target(&p).display()))
    }

    fn chosen<'a>(&self, l: &'a List, p: &(Cfg, String, String)) -> Result<&'a Build, String> {
        bs::choose_for_device(l, &p.1, &p.2, "latest", self.backend.held_layers().as_ref())
    }

    /// Fetch the newest build that fits this device, check it, and install it through the device's own update path. Does not restart.
    fn install(&self, a: &Value) -> Result<String, String> {
        let p = self.pick(a)?;
        if let Some(cpu) = self.backend.cpu() {
            if !bs::fits_cpu(&p.1, &cpu) {
                return Err(format!("{} is a build for another kind of processor than this device's ({cpu}); it was not fetched or installed. Use builds_fetch to only download it.", p.1));
            }
        }
        let l = self.backend.list(&p.0)?;
        let b = self.chosen(&l, &p)?;
        let dir = self.target(&p);
        let done = self.backend.fetch(&p.0, b, &dir)?;
        let said = self.backend.install(&dir, "assistant")?;
        Ok(format!("{done}\n{said}"))
    }

    fn fetch(&self, a: &Value) -> Result<String, String> {
        let p = self.pick(a)?;
        let l = self.backend.list(&p.0)?;
        let b = self.chosen(&l, &p)?;
        let dir = self.target(&p);
        let done = self.backend.fetch(&p.0, b, &dir)?;
        Ok(format!("{done}\nTo install on a paired device the person runs: iyakki dev update {} --device NAME (then iyakki dev reboot). Nothing was installed.", dir.display()))
    }
}

impl Tools for BuildTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_builds_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Show the newest build per machine type in the team's shared build store, from its signed list. Read-only; downloads nothing.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Remember which shared build store to use: its address, the update public key (64 hex characters) its list must be signed by, and optionally the path of a file holding the read token. Always asks. Never put a token in the call, only the file's path.".into(), schema: json!({"type": "object", "properties": {"url": {"type": "string"}, "update_public_key": {"type": "string"}, "read_token_file": {"type": "string"}}, "required": ["url", "update_public_key"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: FETCH_TOOL.into(), description: "Download the newest build of one machine type (for example pc-x86_64) from the shared build store into a folder, checking every file against the signed list. Always asks; installs nothing.".into(), schema: json!({"type": "object", "properties": {"machine_type": {"type": "string"}, "channel": {"type": "string", "description": "dev (default)"}}, "required": []}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: INSTALL_TOOL.into(), description: "Download the newest build of one machine type that fits this device from the shared build store, check every file against the signed list, and install it through this device's own update path (the boot files are replaced, the previous version is kept, nothing restarts). A layered build is chosen only when this device holds its base layers. Always asks, every time; the card lists what changes and every point that needs a yes.".into(), schema: json!({"type": "object", "properties": {"machine_type": {"type": "string"}, "channel": {"type": "string", "description": "dev (default)"}}, "required": []}) }, risk: Risk::Ask, untrusted: false });
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => self.status(),
            SET_TOOL => self.set(args),
            FETCH_TOOL => self.fetch(args),
            INSTALL_TOOL => self.install(args),
            _ => self.inner.call(name, args),
        }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "Show the shared builds".into(),
            SET_TOOL => format!("Use {} as the shared build store", sarg(args, "url")),
            FETCH_TOOL => format!("Download the newest {} build", sarg(args, "machine_type")),
            INSTALL_TOOL => format!("Install the newest {} build from the shared store", sarg(args, "machine_type")),
            _ => self.inner.describe(name, args),
        }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            STATUS_TOOL => None,
            SET_TOOL => Some(format!("Use the build store at {}.\nAccept only lists signed by key {}.\nRead token file: {}", sarg(args, "url"), sarg(args, "update_public_key"), if sarg(args, "read_token_file").is_empty() { "none" } else { sarg(args, "read_token_file") })),
            FETCH_TOOL => Some(self.card(args).unwrap_or_else(|e| format!("Cannot download: {e}"))),
            INSTALL_TOOL => Some(match self.card(args) {
                Ok(c) => format!("{}\nThen it is INSTALLED on this device through the device's own update path: the boot files are replaced, the previous version is kept, nothing restarts by itself. If the build was fetched before, the lines below say what would change.\n{}", c.replace("Nothing is installed.", ""), self.pick(args).ok().and_then(|p| self.backend.install_preview(&self.target(&p)).ok()).map(|v| v.join("\n")).unwrap_or_else(|| "(the exact list of changes is shown after the download is checked; the install stops there if anything needs a yes the card did not show)".into())),
                Err(e) => format!("Cannot install: {e}"),
            }),
            _ => self.inner.preview(name, args),
        }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            STATUS_TOOL => None,
            SET_TOOL => self.set_check(&Cfg { url: sarg(args, "url").to_string(), key: sarg(args, "update_public_key").to_ascii_lowercase(), token_file: String::new() }).err(),
            FETCH_TOOL | INSTALL_TOOL => self.pick(args).err(),
            _ => self.inner.refusal(name, args),
        }
    }
    fn pre_approved(&self, name: &str) -> bool {
        !is_builds_tool(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        name == STATUS_TOOL || (!is_builds_tool(name) && self.inner.approved_always(name))
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == FETCH_TOOL || name == SET_TOOL || name == INSTALL_TOOL { Some("it uses the network and the disk for a shared build, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}
