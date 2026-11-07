//! "Build it here (k3s)": the browser's audio and video add-on built on this machine, from the FFmpeg project's own source, in an isolated
//! k3s Job (docs/development/AV-ADDON-K3S-BUILD.md).
//!
//! **IYAKKI ships and hosts no FFmpeg.** The image carries only the version map and the build recipe of the `iyakki-media-bridge` repository
//! (`/usr/share/iyakki/av-addon/bridge/`, copied there at image build time by `dist/addons/ffmpeg-libs/sync-bridge.sh`, never fetched). At the
//! administrator's click, after the consent text ([`consent_text`]) was read, this module:
//!
//! 1. checks the gate (k3s answers) and finds the map entry for the image's own IYAKKI version ([`lookup`]; none = a message pointing to the
//!    mirror and file routes);
//! 2. checks the bundled recipe against its recorded hash ([`Recipe::load`]) and hands it to the Job as a ConfigMap;
//! 3. runs the Job ([`manifests`]): no privilege, no service-account token, one host folder (the output), CPU, memory and disk limits, a time
//!    limit, egress only to DNS, HTTP and HTTPS ports;
//! 4. surfaces a checksum failure with the file name and both hashes ([`explain_failure`]);
//! 5. copies the package into `<data>/local/packages` ([`copy_into_packages`], which refuses any path outside the folder). **It never
//!    installs it**: the person presses Install and goes through the existing review and confirm step (`avaddon_install.rs`);
//! 6. records a stamp (map entry + the image's GStreamer version) with the installed add-on. When either changes after an image update
//!    ([`stamp_state`]) the engine starts without the add-on and the administrator gets the inbox note "Browser audio and video needs a rebuild"
//!    ([`notify_if_stale`]). Nothing is rebuilt on its own.
//!
//! Everything that touches the cluster goes through a `kubectl` closure and `sleep` is a closure too, so the tests use fakes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::avaddon;

/// The image's copy of the bridge repository's map and recipe.
pub const BRIDGE_DIR: &str = "/usr/share/iyakki/av-addon/bridge";
/// The IYAKKI version of this program (= of the image it runs in).
pub const IMAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The bundled map, for tests and checks of the repository's own copy.
pub const BUNDLED_MAP: &str = include_str!("../../../dist/addons/ffmpeg-libs/bridge/versions.map");
/// The namespace of the build (removed with everything in it when the build ends).
pub const NAMESPACE: &str = "iyakki-avbuild";
pub const JOB_NAME: &str = "av-addon-build";
/// The Job's time limit.
pub const TIME_LIMIT_SECS: u64 = 3600;
/// The title of the Store's confirm step for this route (the Store recognises the route by it).
pub const CONFIRM_TITLE: &str = "Browser audio and video: build it here (k3s), read, then confirm";
/// The inbox note after an image update.
pub const REBUILD_TITLE: &str = "Browser audio and video needs a rebuild";
/// What the person is told when the image's version has no map entry.
pub const NO_ENTRY_TEXT: &str = "This IYAKKI version has no build recipe in the version map (its sources may no longer be available), so it cannot be built here. Use the mirror route (an address your organisation provides) or put a package file in /local/packages.";
/// What the person is told when k3s is not there.
pub const NO_K3S_TEXT: &str = "Building here needs k3s, which is not installed or not running. Install and start k3s in Store -> Setup, or use the file route (put ffmpeg-libs-<version>-linux-musl-<arch>.tar.gz in /local/packages) or the mirror route.";
/// What the Job leaves, for the person's information.
pub const AFTER_BUILD_TEXT: &str = "The package was built and placed in /local/packages. Nothing is installed yet: press Install on this card to read the review (version, licence, configure line) and confirm.";

// ------------------------------------------------------------------------------------------------ the version map

/// One line of `versions.map`: `IYAKKI_VERSION FFMPEG_VERSION SHA256 BUILDER_IMAGE SONAMES`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    /// May end in `.*` (any version with that prefix).
    pub pattern: String,
    pub ffmpeg: String,
    pub sha256: String,
    /// `registry/repository@sha256:<digest>`: a builder image is always pinned by digest.
    pub builder: String,
    pub sonames: Vec<String>,
}

fn plain(s: &str, extra: &[char]) -> bool {
    !s.is_empty() && s.len() <= 200 && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Parses one map line (five fields; every field checked, so nothing odd reaches the Job's command line).
pub fn parse_line(line: &str) -> Result<MapEntry, String> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() != 5 {
        return Err(format!("a version map line has 5 fields, this one has {}", f.len()));
    }
    let (pattern, ffmpeg, sha, builder, so) = (f[0], f[1], f[2], f[3], f[4]);
    if !plain(pattern, &['.', '-', '*', '_']) || pattern.contains('*') && !pattern.ends_with(".*") || pattern.matches('*').count() > 1 {
        return Err(format!("the IYAKKI version `{pattern}` is not usable (letters, digits, . - _ and an optional trailing .*)"));
    }
    if !plain(ffmpeg, &['.', '-']) {
        return Err(format!("the FFmpeg version `{ffmpeg}` is not usable"));
    }
    if !hex64(sha) {
        return Err("the FFmpeg source SHA-256 must be 64 lower-case hexadecimal characters".into());
    }
    let (image, digest) = builder.split_once("@sha256:").ok_or("the builder image must be pinned by digest (name@sha256:...)")?;
    if !hex64(digest) || !plain(image, &['.', '-', '_', '/', ':']) || image.starts_with('/') || image.contains("..") {
        return Err("the builder image reference is not usable (name@sha256:<64 hex>)".into());
    }
    let sonames: Vec<String> = so.split(',').map(str::to_string).collect();
    if sonames.is_empty() || !sonames.iter().all(|s| s.starts_with("lib") && s.contains(".so.") && plain(s, &['.', '_', '-', '+'])) {
        return Err("the sonames must be library names like libavcodec.so.60, comma separated".into());
    }
    Ok(MapEntry { pattern: pattern.into(), ffmpeg: ffmpeg.into(), sha256: sha.into(), builder: builder.into(), sonames })
}

impl MapEntry {
    /// The entry as one normalised line (what the stamp records).
    pub fn line(&self) -> String {
        format!("{} {} {} {} {}", self.pattern, self.ffmpeg, self.sha256, self.builder, self.sonames.join(","))
    }
    /// The registry host of the builder image (`docker.io` when the reference names none).
    pub fn registry(&self) -> String {
        let image = self.builder.split('@').next().unwrap_or("");
        match image.split_once('/') {
            Some((h, _)) if h.contains('.') || h.contains(':') || h == "localhost" => h.to_string(),
            _ => "docker.io".to_string(),
        }
    }
    /// Where the source comes from.
    pub fn source_url(&self) -> String {
        format!("https://ffmpeg.org/releases/ffmpeg-{}.tar.xz", self.ffmpeg)
    }
}

/// The same rule as the repository's `tools/map-lookup.sh`: a pattern ending in `.*` matches any version with that prefix, else the version must be
/// equal.
pub fn pattern_matches(pattern: &str, version: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => version.starts_with(&format!("{prefix}.")) && version.len() > prefix.len() + 1,
        None => pattern == version,
    }
}

/// The first entry of `map_text` that matches `version`; comments and blank lines are skipped. No match: [`NO_ENTRY_TEXT`]. A damaged line before
/// the match is an error (the map is not trusted half-read).
pub fn lookup(map_text: &str, version: &str) -> Result<MapEntry, String> {
    for l in map_text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let e = parse_line(l)?;
        if pattern_matches(&e.pattern, version) {
            return Ok(e);
        }
    }
    Err(NO_ENTRY_TEXT.to_string())
}

// ------------------------------------------------------------------------------------------------ the recipe

/// The bundled recipe: the files the Job runs, checked against the recorded hash.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub build_sh: String,
    pub configure_args: String,
    pub av_check: String,
    pub hash: String,
}

/// SHA-256 over `build.sh`, `configure-args.txt`, `files/iyakki-av-check` (in this order, concatenated), as `sync-bridge.sh` computes it.
pub fn recipe_hash(build_sh: &[u8], configure_args: &[u8], av_check: &[u8]) -> String {
    let mut all = build_sh.to_vec();
    all.extend_from_slice(configure_args);
    all.extend_from_slice(av_check);
    crate::clis::sha256_hex(&all)
}

impl Recipe {
    /// Reads `<bridge>/recipe/...` and `<bridge>/RECIPE.SHA256`; a changed file is refused with both hashes.
    pub fn load(bridge: &Path) -> Result<Recipe, String> {
        let rd = |p: PathBuf| std::fs::read(&p).map_err(|e| format!("the build recipe is not complete: {}: {e}", p.display()));
        let (b, c, a) = (rd(bridge.join("recipe/build.sh"))?, rd(bridge.join("recipe/configure-args.txt"))?, rd(bridge.join("recipe/files/iyakki-av-check"))?);
        let want = std::fs::read_to_string(bridge.join("RECIPE.SHA256")).map_err(|e| format!("the recipe hash is missing: {e}"))?.trim().to_string();
        let got = recipe_hash(&b, &c, &a);
        if got != want {
            return Err(format!("the bundled build recipe does not match its recorded hash (expected {want}, got {got}); nothing was started"));
        }
        let text = |v: Vec<u8>| String::from_utf8(v).map_err(|_| "the build recipe is not text".to_string());
        let r = Recipe { build_sh: text(b)?, configure_args: text(c)?, av_check: text(a)?, hash: got };
        // the LGPL-only line must be intact whatever the hash says
        for must in ["--disable-gpl", "--disable-nonfree"] {
            if !r.configure_args.lines().any(|l| l.trim() == must) {
                return Err(format!("the recipe's configure arguments lack {must}"));
            }
        }
        for bad in ["--enable-gpl", "--enable-nonfree", "--enable-libx264", "--enable-libx265", "--enable-libfdk-aac", "--enable-version3"] {
            if r.configure_args.lines().any(|l| l.trim() == bad) {
                return Err(format!("the recipe's configure arguments contain {bad}: not an LGPL-only build"));
            }
        }
        Ok(r)
    }
}

// ------------------------------------------------------------------------------------------------ the consent text

/// What the person reads BEFORE anything starts (the Store shows it and a button; nothing runs until the button).
pub fn consent_text(e: &MapEntry, machine: &str) -> Vec<String> {
    let l = |s: &str| s.to_string();
    vec![
        l("Build the browser's audio and video add-on on this machine (k3s)."),
        format!("IYAKKI {IMAGE_VERSION} builds FFmpeg {} from the FFmpeg project's own source, so that the BROWSER tile can play H.264 video and AAC/MP3 sound. IYAKKI itself ships and hosts no FFmpeg.", e.ffmpeg),
        String::new(),
        l("What is downloaded, and from where (by the build container, when you press the button below):"),
        format!("  - the FFmpeg {} source, about 11 MB, from ffmpeg.org ({}), checked against SHA-256 {} before it is used", e.ffmpeg, e.source_url(), e.sha256),
        format!("  - the builder image, about 4 MB, from its registry {} (pinned by digest: {})", e.registry(), e.builder),
        l("  - the compiler and tools inside the builder container, about 200 MB, from the Alpine package mirror that container uses (Alpine's own default mirror list)"),
        l("Total download: about 215 MB (an estimate; it has not been measured on every machine)."),
        l("Time: about 10 to 30 minutes, depending on the processor. Disk: up to 4 GB while it builds (removed afterwards); the finished package is about 2 MB."),
        l("The build runs in an isolated, unprivileged k3s Job: no access to your files except one output folder, no cluster credentials, limited CPU and memory, a time limit of one hour, network only for web and name lookups."),
        format!("Libraries built: {} for {machine}, as shared libraries; decoders h264, aac, aac_latm, mp3 only; no encoders.", e.sonames.join(", ")),
        l("Licence: LGPL-2.1 or later. FFmpeg is configured WITHOUT --enable-gpl, --enable-nonfree and --enable-version3; no x264, x265 or fdk-aac. The licence texts and the written source offer are inside the package."),
        l("Patents: H.264 and AAC are formats covered by patents in some countries. Decoding them is your own choice and responsibility."),
        l("After the build the package is placed in /local/packages. Nothing is installed by this step: you then press Install and read the usual review and confirm it."),
    ]
}

// ------------------------------------------------------------------------------------------------ the Job

/// Where the build puts its package, below the data folder.
pub fn scratch_dir(data: &Path) -> PathBuf {
    data.join("addons").join(".build")
}

pub fn out_dir(data: &Path) -> PathBuf {
    scratch_dir(data).join("out")
}

/// The Job's manifests as one `List` (JSON, which `kubectl apply` reads): namespace, network policy, recipe ConfigMap and the Job.
pub fn manifests(e: &MapEntry, recipe: &Recipe, out_host_dir: &Path) -> Value {
    let labels = json!({"app.kubernetes.io/managed-by": "iyakki", "iyakki.dev/job": "av-addon-build"});
    // The container: copy the recipe from the ConfigMap into a writable folder in the layout build.sh expects, check its hash again, run it.
    let script = "set -eu; mkdir -p /recipe/files; cp /recipe-src/build.sh /recipe-src/configure-args.txt /recipe/; cp /recipe-src/iyakki-av-check /recipe/files/; \
                  got=$(cat /recipe/build.sh /recipe/configure-args.txt /recipe/files/iyakki-av-check | sha256sum | cut -d' ' -f1); \
                  [ \"$got\" = \"$1\" ] || { echo \"recipe hash differs: expected $1, got $got\" >&2; exit 6; }; shift; exec sh /recipe/build.sh \"$@\"";
    json!({
        "apiVersion": "v1",
        "kind": "List",
        "items": [
            {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE, "labels": {
                "app.kubernetes.io/managed-by": "iyakki",
                "pod-security.kubernetes.io/enforce": "baseline"}}},
            {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy", "metadata": {"name": "build-egress", "namespace": NAMESPACE, "labels": labels},
             "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"], "ingress": [],
                      "egress": [{"ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}, {"protocol": "TCP", "port": 80}, {"protocol": "TCP", "port": 443}]}]}},
            {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "build-recipe", "namespace": NAMESPACE, "labels": labels},
             "data": {"build.sh": recipe.build_sh, "configure-args.txt": recipe.configure_args, "iyakki-av-check": recipe.av_check}},
            {"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": JOB_NAME, "namespace": NAMESPACE, "labels": labels},
             "spec": {
                 "backoffLimit": 0,
                 "activeDeadlineSeconds": TIME_LIMIT_SECS,
                 "ttlSecondsAfterFinished": 600,
                 "template": {
                     "metadata": {"labels": labels},
                     "spec": {
                         "restartPolicy": "Never",
                         "automountServiceAccountToken": false,
                         "enableServiceLinks": false,
                         "hostNetwork": false, "hostPID": false, "hostIPC": false,
                         "securityContext": {"seccompProfile": {"type": "RuntimeDefault"}},
                         "containers": [{
                             "name": "build",
                             "image": e.builder,
                             "imagePullPolicy": "IfNotPresent",
                             "command": ["sh", "-c", script, "iyakki-av-build", recipe.hash, e.ffmpeg, e.sha256, e.sonames.join(",")],
                             "env": [{"name": "WORK", "value": "/work"}, {"name": "OUT", "value": "/out"}],
                             // root inside the container is needed by `apk add`; everything else is taken away (no privilege, no new privileges)
                             "securityContext": {"privileged": false, "allowPrivilegeEscalation": false,
                                                 "capabilities": {"drop": ["ALL"], "add": ["CHOWN", "DAC_OVERRIDE", "FOWNER", "SETUID", "SETGID"]}},
                             "resources": {"requests": {"cpu": "1", "memory": "1Gi", "ephemeral-storage": "1Gi"},
                                           "limits": {"cpu": "4", "memory": "4Gi", "ephemeral-storage": "4Gi"}},
                             "volumeMounts": [
                                 {"name": "recipe", "mountPath": "/recipe-src", "readOnly": true},
                                 {"name": "work", "mountPath": "/work"},
                                 {"name": "out", "mountPath": "/out"}],
                         }],
                         "volumes": [
                             {"name": "recipe", "configMap": {"name": "build-recipe"}},
                             {"name": "work", "emptyDir": {"sizeLimit": "4Gi"}},
                             {"name": "out", "hostPath": {"path": out_host_dir.to_string_lossy(), "type": "Directory"}}],
                     }}}}
        ]
    })
}

/// What a failed build's log says. A checksum failure names the file and both hashes.
pub fn explain_failure(log: &str) -> String {
    if let Some((file, want, got)) = checksum_failure(log) {
        return format!("Checksum failure: {file} does not match the version map. Expected SHA-256 {want}, got {got}. The build stopped before using it; nothing was built or installed. The download may have been altered or the source may have changed: use the mirror or file route, or try again later.");
    }
    if let Some(l) = log.lines().find(|l| l.starts_with("recipe hash differs")) {
        return format!("The recipe inside the build container does not match the bundled one ({l}); nothing was built.");
    }
    let tail: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(4)..].join(" | ");
    if tail.is_empty() {
        "The build failed and left no output.".to_string()
    } else {
        format!("The build failed: {tail}")
    }
}

/// `checksum of <file> differs: expected <sha>, got <sha>` as `recipe/build.sh` prints it.
pub fn checksum_failure(log: &str) -> Option<(String, String, String)> {
    let l = log.lines().find_map(|l| l.trim().strip_prefix("checksum of "))?;
    let (file, rest) = l.split_once(" differs: expected ")?;
    let (want, got) = rest.split_once(", got ")?;
    Some((file.trim().to_string(), want.trim().to_string(), got.trim().to_string()))
}

// ------------------------------------------------------------------------------------------------ the output

/// A file name that is one plain component: no separator, no `..`, no NUL.
fn plain_name(n: &str) -> bool {
    !n.is_empty() && n != "." && n != ".." && !n.contains(['/', '\\', '\0'])
}

/// Copies `src` (a plain file inside `from_dir`) to `<data>/local/packages/<name>`. Refuses a name with a separator, a source that is a link
/// or resolves outside `from_dir`, and a name that is not an add-on package of this processor. The copy is written beside the target and
/// renamed. Returns the final path.
pub fn copy_into_packages(data: &Path, from_dir: &Path, name: &str, arch: &str) -> Result<PathBuf, String> {
    if !plain_name(name) || !name.starts_with("ffmpeg-libs-") || !name.ends_with(&format!("-linux-musl-{arch}.tar.gz")) {
        return Err(format!("`{name}` is not a package name this step copies"));
    }
    let from = std::fs::canonicalize(from_dir).map_err(|e| format!("the output folder cannot be read: {e}"))?;
    let src = from.join(name);
    let m = std::fs::symlink_metadata(&src).map_err(|e| format!("the package {name} was not found in the output folder: {e}"))?;
    if !m.file_type().is_file() {
        return Err(format!("{name} in the output folder is not a plain file"));
    }
    if std::fs::canonicalize(&src).map_err(|e| e.to_string())?.parent() != Some(from.as_path()) {
        return Err(format!("{name} resolves outside the output folder"));
    }
    let dir = crate::avaddon_install::local_dir(data);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(name);
    if dest.parent() != Some(dir.as_path()) {
        return Err("the target is outside /local/packages".into());
    }
    if std::fs::symlink_metadata(&dest).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(format!("{} exists and is not a plain file", dest.display()));
    }
    let tmp = dir.join(format!(".{name}.part"));
    std::fs::copy(&src, &tmp).map_err(|e| format!("could not copy the package: {e}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o640));
    }
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(dest)
}

/// The package the recipe wrote in `out`, and its checked SHA-256 (the `.sha256` file beside it must agree with the file).
pub fn find_package(out: &Path, arch: &str) -> Result<String, String> {
    let tail = format!("-linux-musl-{arch}.tar.gz");
    let mut found: Vec<String> = std::fs::read_dir(out)
        .map_err(|e| format!("the output folder cannot be read: {e}"))?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.starts_with("ffmpeg-libs-") && n.ends_with(&tail) && e.file_type().is_ok_and(|t| t.is_file())).then_some(n)
        })
        .collect();
    found.sort();
    let name = found.pop().ok_or("the build finished but left no package")?;
    let bytes = std::fs::read(out.join(&name)).map_err(|e| e.to_string())?;
    let got = crate::clis::sha256_hex(&bytes);
    let recorded = std::fs::read_to_string(out.join(format!("{name}.sha256"))).map_err(|_| format!("{name}.sha256 is missing"))?;
    let want = recorded.split_whitespace().next().unwrap_or("");
    if want != got {
        return Err(format!("Checksum failure: {name} does not match its .sha256 (expected {want}, got {got}); it was not copied"));
    }
    Ok(name)
}

// ------------------------------------------------------------------------------------------------ the run

/// What `kubectl` answered: success and its text.
pub type Kubectl<'a> = &'a dyn Fn(&[&str]) -> (bool, String);

/// The k3s cluster answers and its node is Ready.
pub fn k3s_ready(kubectl: Kubectl) -> bool {
    let (ok, out) = kubectl(&["get", "nodes", "-o", "jsonpath={.items[*].status.conditions[?(@.type=='Ready')].status}"]);
    ok && out.contains("True")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
}

/// `succeeded,failed` counters of the Job.
pub fn job_state(kubectl: Kubectl) -> JobState {
    let (ok, out) = kubectl(&["-n", NAMESPACE, "get", "job", JOB_NAME, "-o", "jsonpath={.status.succeeded},{.status.failed}"]);
    if !ok {
        return JobState::Running;
    }
    let (s, f) = out.trim().split_once(',').unwrap_or((out.trim(), ""));
    if s.trim().parse::<u32>().unwrap_or(0) > 0 {
        JobState::Succeeded
    } else if f.trim().parse::<u32>().unwrap_or(0) > 0 {
        JobState::Failed
    } else {
        JobState::Running
    }
}

pub struct RunCx<'a> {
    pub data: &'a Path,
    pub bridge: &'a Path,
    pub map_text: &'a str,
    pub version: &'a str,
    pub arch: &'a str,
    pub kubectl: Kubectl<'a>,
    pub sleep: &'a dyn Fn(Duration),
    pub say: &'a dyn Fn(&str),
    /// How often the Job is asked about (a test passes a handful).
    pub max_polls: u32,
}

/// The whole build: gate, map entry, recipe, Job, copy. Cleans up in every case. Returns a plain sentence for the log.
pub fn build(cx: &RunCx) -> Result<String, String> {
    (cx.say)("[1/5] checking k3s and the version map");
    if !k3s_ready(cx.kubectl) {
        return Err(NO_K3S_TEXT.into());
    }
    let entry = lookup(cx.map_text, cx.version)?;
    check_image_matches(&entry)?;
    let recipe = Recipe::load(cx.bridge)?;
    let scratch = scratch_dir(cx.data);
    let out = out_dir(cx.data);
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&out).map_err(|e| format!("could not make the output folder: {e}"))?;
    let result = run_job(cx, &entry, &recipe, &scratch, &out);
    // the pod, the namespace and every partial output go in all cases
    let _ = (cx.kubectl)(&["delete", "namespace", NAMESPACE, "--ignore-not-found", "--wait=false"]);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// The sonames the map names must be the ones the image's bridge plugin needs (a map entry for other libraries would build an add-on the engine refuses).
fn check_image_matches(e: &MapEntry) -> Result<(), String> {
    let abi = std::fs::read_to_string(avaddon::ABI_FILE).unwrap_or_default();
    check_sonames(e, &avaddon::required_sonames(&abi))
}

pub fn check_sonames(e: &MapEntry, image: &[String]) -> Result<(), String> {
    let mut a = e.sonames.clone();
    let mut b = image.to_vec();
    a.sort();
    b.sort();
    if a != b {
        return Err(format!("the version map builds {} but this image's browser engine needs {}; it was not started", a.join(", "), b.join(", ")));
    }
    Ok(())
}

fn run_job(cx: &RunCx, e: &MapEntry, recipe: &Recipe, scratch: &Path, out: &Path) -> Result<String, String> {
    let manifest = manifests(e, recipe, out);
    let file = scratch.join("job.json");
    std::fs::write(&file, serde_json::to_vec(&manifest).map_err(|x| x.to_string())?).map_err(|x| format!("could not write the Job: {x}"))?;
    (cx.say)(&format!("[2/5] starting the isolated build Job (FFmpeg {}, builder {})", e.ffmpeg, e.builder));
    let (ok, text) = (cx.kubectl)(&["apply", "-f", &file.display().to_string()]);
    if !ok {
        return Err(format!("k3s refused the build Job: {}", text.trim()));
    }
    (cx.say)("[3/5] building (download, checksum, configure, compile); this takes a while");
    let mut state = JobState::Running;
    for _ in 0..cx.max_polls {
        state = job_state(cx.kubectl);
        if state != JobState::Running {
            break;
        }
        (cx.sleep)(Duration::from_secs(5));
    }
    let (_, log) = (cx.kubectl)(&["-n", NAMESPACE, "logs", &format!("job/{JOB_NAME}"), "--tail=200"]);
    match state {
        JobState::Succeeded => {}
        JobState::Failed => return Err(explain_failure(&log)),
        JobState::Running => return Err(format!("The build did not finish within the time limit ({} minutes) and was stopped. {}", TIME_LIMIT_SECS / 60, explain_failure(&log))),
    }
    (cx.say)("[4/5] checking the package");
    let name = find_package(out, cx.arch)?;
    (cx.say)("[5/5] copying it into /local/packages");
    let dest = copy_into_packages(cx.data, out, &name, cx.arch)?;
    save_built_here(cx.data, &name, e)?;
    Ok(format!("{AFTER_BUILD_TEXT} ({})", dest.display()))
}

// ------------------------------------------------------------------------------------------------ the request marker

fn request_path(data: &Path) -> PathBuf {
    data.join("addons").join(".build-request")
}

/// The Store writes this when the administrator pressed the button after the consent text; the install job reads it once.
pub fn write_request(data: &Path) -> Result<(), String> {
    let p = request_path(data);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, b"build-here\n").map_err(|e| e.to_string())
}

/// True once per [`write_request`].
pub fn take_request(data: &Path) -> bool {
    std::fs::remove_file(request_path(data)).is_ok()
}

// ------------------------------------------------------------------------------------------------ the stamp

fn built_here_path(data: &Path) -> PathBuf {
    data.join("addons").join(".built-here")
}

/// The image's GStreamer version from the library's file name (`libgstreamer-1.0.so.0.2406.0` is 1.24.6).
pub fn gst_from_soname(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix("libgstreamer-1.0.so.0.")?;
    let (minor_micro, _) = rest.split_once('.')?;
    let n: u32 = minor_micro.parse().ok()?;
    Some(format!("1.{}.{}", n / 100, n % 100))
}

/// The GStreamer version of this image, read from the library link in `dirs`.
pub fn image_gst_version(dirs: &[&Path]) -> Option<String> {
    dirs.iter().find_map(|d| {
        let real = std::fs::canonicalize(d.join("libgstreamer-1.0.so.0")).ok()?;
        gst_from_soname(&real.file_name()?.to_string_lossy())
    })
}

fn current_gst() -> String {
    image_gst_version(&[Path::new("/usr/lib"), Path::new("/lib")]).unwrap_or_default()
}

fn stamp_text(entry_line: &str, gst: &str) -> String {
    format!("entry {entry_line}\ngstreamer {gst}\n")
}

/// Remembers that this package was built here, with the map entry and the GStreamer version it was built for.
pub fn save_built_here(data: &Path, package: &str, e: &MapEntry) -> Result<(), String> {
    save_built_here_with(data, package, e, &current_gst())
}

pub fn save_built_here_with(data: &Path, package: &str, e: &MapEntry, gst: &str) -> Result<(), String> {
    let p = built_here_path(data);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|x| x.to_string())?;
    std::fs::write(&p, format!("package {package}\n{}", stamp_text(&e.line(), gst))).map_err(|x| x.to_string())
}

/// Called after an install of the archive `package` (a file name): when that is the package built here, the stamp goes into the installed
/// add-on folder as `BUILD-STAMP`.
pub fn stamp_after_install(data: &Path, package: &str, addon_dir: &Path) {
    let Ok(t) = std::fs::read_to_string(built_here_path(data)) else { return };
    let Some((first, rest)) = t.split_once('\n') else { return };
    if first.strip_prefix("package ") == Some(package) {
        let _ = std::fs::write(addon_dir.join("BUILD-STAMP"), rest);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StampState {
    /// No stamp: the add-on came from a file, a mirror or an installed ffmpeg; nothing to compare.
    NotBuiltHere,
    Current,
    /// The map entry or the image's GStreamer changed since the build: why.
    Stale(String),
}

/// Compares the installed add-on's stamp with the current map entry (for `version`, from `map_text`) and GStreamer version.
pub fn stamp_state_with(addon_dir: &Path, map_text: &str, version: &str, gst: &str) -> StampState {
    let Ok(t) = std::fs::read_to_string(addon_dir.join("BUILD-STAMP")) else { return StampState::NotBuiltHere };
    let field = |k: &str| t.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(' '))).unwrap_or("").trim().to_string();
    let (was_entry, was_gst) = (field("entry"), field("gstreamer"));
    let now = match lookup(map_text, version) {
        Ok(e) => e.line(),
        Err(_) => return StampState::Stale(format!("this IYAKKI version has no entry in the version map any more (the add-on was built from: {was_entry})")),
    };
    if was_entry != now {
        return StampState::Stale("the version map entry for this IYAKKI version changed since the add-on was built".into());
    }
    if was_gst != gst {
        return StampState::Stale(format!("the image's GStreamer changed from {} to {}", if was_gst.is_empty() { "unknown" } else { &was_gst }, if gst.is_empty() { "unknown" } else { gst }));
    }
    StampState::Current
}

/// [`stamp_state_with`] for this image.
pub fn stamp_state(data: &Path) -> StampState {
    let map = std::fs::read_to_string(Path::new(BRIDGE_DIR).join("versions.map")).unwrap_or_default();
    stamp_state_with(&avaddon::addon_dir(data), &map, IMAGE_VERSION, &current_gst())
}

/// The engine starts without the add-on while the stamp is stale.
pub fn needs_rebuild(data: &Path) -> bool {
    matches!(stamp_state(data), StampState::Stale(_))
}

/// The inbox note (title, body) for a stale stamp.
pub fn rebuild_note(why: &str) -> (String, String) {
    (REBUILD_TITLE.to_string(), format!("{why}. The browser starts without H.264 and AAC until you rebuild it: Store -> Setup -> Browser audio and video -> Build it here (k3s). Nothing is rebuilt on its own."))
}

/// At an administrator's sign-in: the inbox note when the add-on built here no longer matches this image (not again while an unread one is there).
pub fn notify_if_stale(svc: &crate::services::Services, user: &str) {
    if let StampState::Stale(why) = stamp_state(svc.dir()) {
        let (t, b) = rebuild_note(&why);
        let _ = svc.notify_once(user, crate::inbox::Kind::Warning, &t, &b, Some(crate::inbox::Link::new("store", avaddon::INSTALL_ITEM)));
    }
}
