//! The assistant's tool that turns an archived sandbox root into an image app in the dev registry (F-LINAPP-16): `linuxapp_image`.
//! It reads the person's own `.tar.gz` (made inside the sandbox), rewrites it to one clean layer (`ocipush_root`), pushes it to the dev
//! cluster's registry and writes the image app file that Linux apps -> Image apps lists. It ASKS every time (the card says what is in
//! the archive, what is dropped and what the image runs), administrators only. Nothing here was run on a cluster (KI-781).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::imagebuild::Built;
use crate::linuxpod as lp;
use crate::ocipush::RegistryHttp;
use crate::ocipush_root::{build_image_from_root, layer_from_root_tar, RootImageSpec};
pub use iyakki_shared::linuxappimagetools::{PROMPT_RULE, TOOL};



pub trait Backend: Send + Sync {
    fn is_admin(&self) -> bool;
    /// The archive `path` names (inside the person's own home), or why not.
    fn archive(&self, path: &str) -> Result<Vec<u8>, String>;
    fn dev_cluster_running(&self) -> bool;
    /// `localhost:30500`: where the dev registry listens.
    fn registry(&self) -> String;
    fn http(&self) -> Arc<dyn RegistryHttp>;
    /// `amd64` or `arm64`.
    fn arch(&self) -> String;
    /// Write the image app file `<id>.json`.
    fn write_app(&self, id: &str, text: &str) -> Result<(), String>;
    fn audit(&self, action: &str, detail: &str);
}

struct Plan {
    tar: Vec<u8>,
    id: String,
    title: String,
    port: u16,
    licence: String,
    tag: String,
    cmd: Vec<String>,
}

pub struct LinuxAppImageTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

fn id_of(name: &str) -> String {
    name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').chars().take(40).collect()
}

impl LinuxAppImageTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> LinuxAppImageTools {
        LinuxAppImageTools { inner, be }
    }

    fn plan(&self, a: &Value) -> Result<Plan, String> {
        if !self.be.is_admin() {
            return Err("only an administrator account may make an image from a sandbox".into());
        }
        if !self.be.dev_cluster_running() {
            return Err("the dev cluster is not running: start it in Settings -> Developer first".into());
        }
        let archive = self.be.archive(a["archive"].as_str().ok_or("give the archive path (archive)")?)?;
        let tar = lp::unpack_archive(&archive)?;
        let title = a["name"].as_str().ok_or("give the app name (name)")?.trim().to_string();
        let id = id_of(&title);
        if id.is_empty() {
            return Err("the app name has no letters or digits".into());
        }
        let licence = a["licence"].as_str().map(str::trim).filter(|l| !l.is_empty()).ok_or("an image app needs its licence (for example MIT): ask the person, do not guess")?.to_string();
        let port = a["port"].as_u64().filter(|p| (1024..=65535).contains(p)).ok_or("give the port the app listens on (1024 to 65535: the app runs unprivileged)")? as u16;
        let tag = a["tag"].as_str().unwrap_or("1").to_string();
        if tag.is_empty() || tag.len() > 40 || !tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
            return Err("the tag is letters, digits, dot, dash and underscore (up to 40)".into());
        }
        let cmd: Vec<String> = a["command"].as_array().ok_or("give the command that starts the app (command: a list, the first item a full path)")?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        if cmd.is_empty() || !cmd[0].starts_with('/') || cmd.iter().any(|c| c.contains('\0') || c.len() > 500) || cmd.len() > 32 {
            return Err("the command is a list whose first item is a full path (such as /usr/bin/myapp), at most 32 items".into());
        }
        Ok(Plan { tar, id, title, port, licence, tag, cmd })
    }
}

impl Tools for LinuxAppImageTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != TOOL);
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Turn the archived root of a sandbox (a .tar.gz in the person's home) into an image app in the dev registry. Asks the person first; administrators only.".into(), schema: json!({"type": "object", "properties": {"archive": {"type": "string"}, "name": {"type": "string"}, "port": {"type": "integer"}, "licence": {"type": "string"}, "command": {"type": "array", "items": {"type": "string"}}, "tag": {"type": "string"}}, "required": ["archive", "name", "port", "licence", "command"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name != TOOL {
            return self.inner.call(name, args);
        }
        let p = self.plan(args)?;
        let spec = RootImageSpec { arch: &self.be.arch(), user: "10001:10001", entrypoint: vec![], cmd: p.cmd.clone(), env: vec![], workdir: "/", port: Some(p.port) };
        let (img, rep) = build_image_from_root(&p.tar, &spec, lp::MAX_ROOT)?;
        let registry = self.be.registry();
        self.be.audit("linuxapp.image", &format!("{} {} file(s)", p.id, rep.files));
        let http = self.be.http();
        let digest = crate::ocipush::push(&*http, &registry, &p.id, &p.tag, &img, &mut |_| {}).map_err(|e| format!("pushing the image to the dev registry failed: {e}"))?;
        let built = Built { image: format!("{registry}/{}:{}@{digest}", p.id, p.tag), digest };
        let text = crate::imagebuild::image_app_json(&p.id, &p.title, &built, p.port, &p.licence)?;
        self.be.write_app(&p.id, &text)?;
        self.be.audit("linuxapp.image.done", &format!("{} {}", p.id, built.digest));
        Ok(format!("Made {}: {} It is now in Linux apps -> Image apps; Run starts it.", p.title, built.image))
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == TOOL { format!("Make the image app “{}” from the sandbox archive {}", args["name"].as_str().unwrap_or("?"), args["archive"].as_str().unwrap_or("?")) } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name != TOOL {
            return self.inner.preview(name, args);
        }
        let p = self.plan(args).ok()?;
        let (_, rep) = layer_from_root_tar(&p.tar, lp::MAX_ROOT).ok()?;
        let mut l = vec![
            format!("Turn the sandbox root into the image app “{}”: {} files, {} folders, {} links, {} KB unpacked.", p.title, rep.files, rep.dirs, rep.links, rep.bytes.div_ceil(1024)),
            format!("It runs {} as user 10001, listens on port {}, licence {}. Pushed to the dev registry {} as {}:{}.", p.cmd.join(" "), p.port, p.licence, self.be.registry(), p.id, p.tag),
        ];
        if rep.setid_cleared > 0 {
            l.push(format!("{} file(s) lose their set-user-id or set-group-id bit.", rep.setid_cleared));
        }
        if !rep.dropped.is_empty() {
            l.push(format!("Left out (devices, pipes, sockets): {}", rep.dropped.iter().take(8).cloned().collect::<Vec<_>>().join(", ")));
        }
        Some(l.join("\n"))
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL { self.plan(args).err() } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        name != TOOL && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL { Some("an image of a sandbox root is made from files the assistant has not seen, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
    fn correct_claims(&self, answer: &str) -> Option<String> {
        self.inner.correct_claims(answer)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: the person's home (the same archive reader as the pod tools), the dev registry over plain HTTP, the image apps folder.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl RealBackend {
    fn pod(&self) -> crate::linuxpodtools::RealBackend {
        crate::linuxpodtools::RealBackend { svc: self.svc.clone(), policy: self.policy.clone(), user: self.user.clone(), audit: self.audit.clone() }
    }
}

impl Backend for RealBackend {
    fn is_admin(&self) -> bool {
        self.policy.manage_settings
    }
    fn archive(&self, path: &str) -> Result<Vec<u8>, String> {
        crate::linuxpodtools::Backend::archive(&self.pod(), path)
    }
    fn dev_cluster_running(&self) -> bool {
        self.svc.k3sdev.health().state == "running"
    }
    fn registry(&self) -> String {
        "localhost:30500".into()
    }
    fn http(&self) -> Arc<dyn RegistryHttp> {
        Arc::new(crate::ocipush::TcpHttp::default())
    }
    fn arch(&self) -> String {
        if cfg!(target_arch = "aarch64") { "arm64".into() } else { "amd64".into() }
    }
    fn write_app(&self, id: &str, text: &str) -> Result<(), String> {
        let dir = self.svc.dir().join("image-apps");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&dir.join(format!("{id}.json")), text.as_bytes(), 0o600)
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
#[path = "linuxappimagetools_tests.rs"]
mod tests;
