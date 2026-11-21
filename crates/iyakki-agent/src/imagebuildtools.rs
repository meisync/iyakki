//! The assistant's tool for the on-device image builder (F-LINAPP-15): `image_build` packs a folder of the person's own home into a build job
//! of the dev cluster and writes the result as an image app. It ASKS every time (the card names the files, what is left out and the builder),
//! only an administrator may use it, and nothing builds until a builder image pinned by digest is configured (KI-851).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::imagebuild::{self as ib, BuildSpec};
pub use iyakki_shared::imagebuildtools::{builder_pinned, BUILDER_IMAGE, PROMPT_RULE};

pub const TOOL: &str = "image_build";
pub const STATUS_TOOL: &str = "image_builder_status";
pub const SET_TOOL: &str = "image_builder_set";


pub trait Backend: Send + Sync {
    fn is_admin(&self) -> bool;
    /// The folder `dir` names, inside the person's own home, or why not.
    fn folder(&self, dir: &str) -> Result<PathBuf, String>;
    /// The builder image pinned by digest, if one is set up.
    fn builder_image(&self) -> Option<String>;
    /// Set (`Some`) or clear (`None`) the builder image of this computer (already checked to be pinned by digest).
    fn set_builder_image(&self, image: Option<&str>) -> Result<(), String>;
    fn registry(&self) -> String;
    fn dev_cluster_running(&self) -> bool;
    /// Run the build (blocks) and write the image app file; returns the sentence for the person.
    fn run(&self, spec: &BuildSpec, ctx: &ib::Context, app_id: &str, title: &str, port: u16, licence: &str) -> Result<String, String>;
}

pub struct ImageBuildTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

fn id_of(name: &str) -> String {
    name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').chars().take(40).collect()
}

impl ImageBuildTools {
    /// The image to set (`None`: clear), after the checks: an administrator, and a reference pinned by a real digest.
    fn check_set(&self, a: &Value) -> Result<Option<String>, String> {
        if !self.be.is_admin() {
            return Err("only an administrator account may choose the builder image".into());
        }
        match a["image"].as_str().map(str::trim).filter(|i| !i.is_empty()) {
            None => Ok(None),
            Some(i) => {
                crate::imageapps::validate_image_ref(i).map_err(|e| format!("the builder image: {e}"))?;
                if !builder_pinned(i) {
                    return Err("the builder image has to be pinned by a real digest (name@sha256: and 64 hex digits): a tag can be moved to other contents".into());
                }
                Ok(Some(i.to_string()))
            }
        }
    }

    fn set(&self, a: &Value) -> Result<String, String> {
        let image = self.check_set(a)?;
        self.be.set_builder_image(image.as_deref())?;
        Ok(match image {
            Some(i) => format!("The image builder now uses {i}."),
            None => "The builder image is cleared: no images can be built here until one is set.".to_string(),
        })
    }

    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> ImageBuildTools {
        ImageBuildTools { inner, be }
    }

    fn plan(&self, a: &Value) -> Result<(BuildSpec, ib::Context, String, String, u16, String), String> {
        if !self.be.is_admin() {
            return Err("only an administrator account may build images".into());
        }
        let builder = self.be.builder_image().ok_or("no builder image is set up on this computer yet: an administrator has to pin one by digest before images can be built here")?;
        if !self.be.dev_cluster_running() {
            return Err("the dev cluster is not running: start it in Settings -> Developer first".into());
        }
        let dir = self.be.folder(a["dir"].as_str().ok_or("give the folder (dir)")?)?;
        let name = a["name"].as_str().ok_or("give the app name")?.trim().to_string();
        let id = id_of(&name);
        if id.is_empty() {
            return Err("the app name has no letters or digits".into());
        }
        let licence = a["licence"].as_str().map(str::trim).filter(|l| !l.is_empty()).ok_or("an image app needs its licence (for example MIT): ask the person, do not guess")?.to_string();
        let port = a["port"].as_u64().filter(|p| (1..=65535).contains(p)).ok_or("give the port the app listens on (1 to 65535; above 1023 since it runs unprivileged)")? as u16;
        if port < 1024 {
            return Err("the app runs unprivileged, so its port is 1024 or higher".into());
        }
        let dockerfile = a["dockerfile"].as_str().unwrap_or("Containerfile").to_string();
        let spec = BuildSpec { repo: id.clone(), tag: a["tag"].as_str().unwrap_or("1").to_string(), dockerfile, builder_image: builder, registry: self.be.registry() };
        ib::validate(&spec)?;
        let ctx = ib::package_context(&dir, &spec.dockerfile)?;
        Ok((spec, ctx, id, name, port, licence))
    }
}

impl Tools for ImageBuildTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != TOOL);
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say which builder image the on-device image builder uses (the image name and its digest) or that none is set up. Free.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Set the builder image of the on-device image builder, or clear it (give no image). Administrators only; it must be pinned by digest (name@sha256:<64 hex>) and the person has to give it: never invent one. Asks the person first.".into(), schema: json!({"type": "object", "properties": {"image": {"type": "string", "description": "name@sha256:<64 hex digits>; leave out to clear"}}}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Build a container image from a folder in the person's home (it needs a Containerfile or Dockerfile) in the dev cluster, and add it as an image app. Asks the person first; administrators only; takes minutes.".into(), schema: json!({"type": "object", "properties": {"dir": {"type": "string"}, "name": {"type": "string"}, "port": {"type": "integer"}, "licence": {"type": "string"}, "tag": {"type": "string"}, "dockerfile": {"type": "string"}}, "required": ["dir", "name", "port", "licence"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == STATUS_TOOL {
            return Ok(match self.be.builder_image() {
                Some(i) => format!("The image builder uses {i}."),
                None => "No builder image is set up on this computer: an administrator has to pin one by digest (image_builder_set) before images can be built here.".to_string(),
            });
        }
        if name == SET_TOOL {
            return self.set(args);
        }
        if name != TOOL {
            return self.inner.call(name, args);
        }
        let (spec, ctx, id, title, port, licence) = self.plan(args)?;
        self.be.run(&spec, &ctx, &id, &title, port, &licence)
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == SET_TOOL {
            return match args["image"].as_str().filter(|i| !i.trim().is_empty()) {
                Some(i) => format!("Use {i} as the image builder"),
                None => "Clear the image builder (no images can be built until one is set)".to_string(),
            };
        }
        if name == STATUS_TOOL {
            return "Show the image builder".into();
        }
        if name == TOOL { format!("Build the image app “{}” from {}", args["name"].as_str().unwrap_or("?"), args["dir"].as_str().unwrap_or("a folder")) } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name != TOOL {
            return self.inner.preview(name, args);
        }
        let (spec, ctx, _, title, port, licence) = self.plan(args).ok()?;
        let mut l = vec![format!("Build “{title}” in the dev cluster: {} file(s), {} KB, listens on port {port}, licence {licence}.", ctx.files, ctx.bytes.div_ceil(1024)), format!("Builder: {}", spec.builder_image), format!("Result: {}/{}:{} (pinned by the digest the build prints)", spec.registry, spec.repo, spec.tag), "The build downloads base images from the internet and runs the commands of your Containerfile; it stops after 15 minutes.".to_string()];
        if !ctx.skipped.is_empty() {
            l.push(format!("Left out: {}", ctx.skipped.iter().take(8).cloned().collect::<Vec<_>>().join(", ")));
        }
        Some(l.join("\n"))
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL {
            return self.check_set(args).err();
        }
        if name == TOOL { self.plan(args).err() } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        name != TOOL && name != SET_TOOL && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != TOOL && name != SET_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL {
            return Some("the builder image runs the commands of every build, so choosing it asks every time".into());
        }
        if name == TOOL { Some("a build pulls images from the internet and runs their commands, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the person's home, the dev cluster's `kubectl`, and the builder image from `<data>/image-builder.json`
/// (`{"image": "name@sha256:..."}`, written by an administrator; absent: no builds).
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

struct DevKube(Arc<crate::services::Services>);
impl crate::k8s::Kube for DevKube {
    fn run(&self, args: &[String]) -> crate::wsexec::Out {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        self.0.k3sdev.kubectl(&a)
    }
}

impl Backend for RealBackend {
    fn is_admin(&self) -> bool {
        self.policy.manage_settings
    }
    fn folder(&self, dir: &str) -> Result<PathBuf, String> {
        let home = crate::scope::home_root(&self.user, self.svc.dir());
        let p = std::path::Path::new(dir);
        let full = if p.is_absolute() { p.to_path_buf() } else { home.join(p) };
        if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err("the folder may not contain ..".into());
        }
        let real = std::fs::canonicalize(&full).map_err(|_| format!("{dir} is not a folder"))?;
        let home_real = std::fs::canonicalize(&home).unwrap_or(home);
        if !real.starts_with(&home_real) || !real.is_dir() {
            return Err("the folder has to be inside your own home folder".into());
        }
        Ok(real)
    }
    fn builder_image(&self) -> Option<String> {
        // the administrator's choice for this computer wins over the one this build ships (empty until the owner pins one)
        let own = std::fs::read(self.svc.dir().join("image-builder.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["image"].as_str().map(str::to_string)).filter(|i| builder_pinned(i));
        own.or_else(|| Some(BUILDER_IMAGE.to_string()).filter(|i| builder_pinned(i)))
    }
    fn set_builder_image(&self, image: Option<&str>) -> Result<(), String> {
        let f = self.svc.dir().join("image-builder.json");
        (self.audit)("image.builder.set", image.unwrap_or("cleared"));
        match image {
            Some(i) => iyakki_schema::write_atomic(&f, json!({"image": i}).to_string().as_bytes(), 0o600),
            None => match std::fs::remove_file(&f) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
        }
    }
    fn registry(&self) -> String {
        "localhost:30500".into()
    }
    fn dev_cluster_running(&self) -> bool {
        self.svc.k3sdev.health().state == "running"
    }
    fn run(&self, spec: &BuildSpec, ctx: &ib::Context, app_id: &str, title: &str, port: u16, licence: &str) -> Result<String, String> {
        (self.audit)("image.build", &format!("{app_id} {} file(s)", ctx.files));
        let id: String = format!("b{}", crate::clock::now_unix() % 1_000_000_000).chars().filter(char::is_ascii_alphanumeric).collect();
        let apply = self.svc.dir().join(format!("image-build-{id}.json"));
        let built = ib::build(&DevKube(self.svc.clone()), spec, ctx, &id, &apply, 90, &|| std::thread::sleep(std::time::Duration::from_secs(10)))?;
        let dir = self.svc.dir().join("image-apps");
        let text = ib::image_app_json(app_id, title, &built, port, licence)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&dir.join(format!("{app_id}.json")), text.as_bytes(), 0o600)?;
        (self.audit)("image.build.done", &format!("{app_id} {}", built.digest));
        Ok(format!("Built {title}: {} It is now in Linux apps -> Image apps; Run starts it.", built.image))
    }
}

#[cfg(test)]
#[path = "imagebuildtools_tests.rs"]
mod tests;
