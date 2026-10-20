//! The private store as a platform service (`private-store`): a Deployment of `iyakki store-server` with a PVC, a Secret (the admin
//! token and the seed of the catalog key, generated on this device and kept in the person's vault), a Service and an optional
//! Ingress. The image is built here from the static `iyakki-store-server` (found, built or chosen: `platform::localbin`) and goes to
//! the in-cluster registry (never Docker Hub).
//! See docs/STORE.md, section 13, and docs/PLATFORM-SERVICES.md.

use crate::platform::*;
use crate::platform::localbin;

pub const ID: &str = "private-store";
pub const VERSIONS: [&str; 1] = ["1.0.0"];
pub const PORT: u16 = 8080;
pub const IMAGE: &str = "localhost/iyakki-store-server";
/// The Dockerfile IYAKKI writes next to the binary it copies: nothing is pulled, so it builds with no network. No `COPY --chmod`: the
/// classic docker builder (no BuildKit) refuses it, so the copy is made executable before the build instead.
pub const DEVICE_DOCKERFILE: &str = "FROM scratch\nCOPY iyakki /iyakki\nUSER 10001:10001\nEXPOSE 8080\nENTRYPOINT [\"/iyakki\"]\n";

/// The program the image holds: a small static server of its own (`crates/iyakki-store-server`); an older cache's full static `iyakki`
/// also works because it takes the same `store-server ...` arguments.
pub const LOCAL: localbin::LocalBinary = localbin::LocalBinary { file: "iyakki-store-server", package: "iyakki-store-server", sub: "store-server", legacy: &["iyakki"], docs: "docs/STORE.md, section 13" };

pub struct PrivateStore;

fn labels(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: private-store\n{pad}app.kubernetes.io/managed-by: iyakki\n{pad}app.kubernetes.io/version: \"{}\"\n{pad}{LABEL_SERVICE}: {ID}\n{pad}{LABEL_INSTANCE}: {}\n", c.version, c.instance)
}

fn selector(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: private-store\n{pad}{LABEL_INSTANCE}: {}\n", c.instance)
}

fn exposure(o: &Options) -> Exposure {
    Exposure::parse(o.get("exposure")).unwrap_or(Exposure::NodePort)
}

/// True when the ELF file at `path` is a static executable (no program interpreter, no needed library): the headers are read, the file is
/// never run. See [`localbin::read_elf`].
pub fn is_static_elf(path: &std::path::Path) -> bool {
    localbin::read_elf(path).is_ok_and(|i| i.is_static)
}

/// The binary to put in the image for `arch` (`amd64` / `arm64`): found by the order in [`localbin`]. `Err` says what is missing.
pub fn binary_for_image(pl: &Platform, places: &localbin::Places, arch: &str) -> Result<std::path::PathBuf, String> {
    let want = localbin::Arch::parse(arch).unwrap_or_else(localbin::Arch::host);
    let d = localbin::discover(pl.run.as_ref(), &LOCAL, places, want);
    match d.found {
        Some(f) => Ok(f.path),
        None => Err(d.problem(&LOCAL, &places.cache)),
    }
}

impl ServiceDef for PrivateStore {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "Private store".into()
    }
    fn description(&self) -> String {
        "Your own IYAKKI app store in the cluster: publish skills, tools and MCP servers to it, and every IYAKKI that adds it (with the key pinned) can install them. One small container built from IYAKKI itself, a PVC, signed catalog.".into()
    }
    fn kind(&self) -> &'static str {
        "store"
    }
    fn versions(&self) -> Vec<String> {
        VERSIONS.iter().map(|s| s.to_string()).collect()
    }
    fn images(&self, version: &str) -> Vec<ImageRef> {
        // built here for the cluster's architecture from a static file of that architecture (checked in apply, before anything changes)
        vec![ImageRef::new(&format!("{IMAGE}:{version}"), &["amd64", "arm64"])]
    }
    fn options(&self) -> Vec<OptionDef> {
        vec![
            OptionDef::new("public_url", "Public URL", "The address people will type (for example https://store.example.lan). Empty: worked out from the exposure. TLS is the ingress's job.", OptKind::Text, ""),
            OptionDef::new("publishers", "Publishers", "Comma-separated ids to create a publish token for now (for example alice,bob). Tokens go to your vault; hand each to its owner yourself.", OptKind::Text, ""),
            OptionDef::new("read_only", "Read-only", "Serve the catalog but accept no publishing.", OptKind::Bool, "false"),
            OptionDef::new("require_trusted", "Only trusted keys", "Refuse packages signed by a key listed in nobody's trusted-keys.txt (needs trusted keys in the data volume).", OptKind::Bool, "false"),
        ]
    }
    fn check_options(&self, o: &Options) -> Result<(), String> {
        let u = o.get("public_url");
        if !u.is_empty() && !(u.starts_with("https://") || u.starts_with("http://")) {
            return Err("Public URL: starts with https:// or http://".into());
        }
        if u.len() > 200 || u.chars().any(|c| c.is_control() || c == ' ') {
            return Err("Public URL: not an address".into());
        }
        for p in publisher_ids(o)? {
            let _ = p;
        }
        if exposure(o) == Exposure::LoadBalancer && u.is_empty() {
            return Err("Public URL: a LoadBalancer's address is not known before it is assigned; give the address people will use".into());
        }
        Ok(())
    }
    fn always_mirror(&self) -> bool {
        true
    }
    fn local_binary(&self) -> Option<localbin::LocalBinary> {
        Some(LOCAL)
    }
    fn prepare_images(&self, pl: &Platform, version: &str, arch: &str, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.prepare_with(pl, &localbin::Places::real(&pl.dir, &LOCAL), version, arch, progress)
    }
    fn push_builtin(&self, pl: &Platform, http: &dyn crate::ocipush::RegistryHttp, version: &str, arch: &str, registry: &str, repo_tag: &str, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.push_with(pl, &localbin::Places::real(&pl.dir, &LOCAL), http, version, arch, registry, repo_tag, progress)
    }
    fn build(&self, c: &BuildCtx) -> Result<Built, String> {
        let ex = exposure(&c.opts);
        let image = c.images.first().ok_or("no image")?;
        let public = c.opts.get("public_url").trim_end_matches('/').to_string();
        let base = if !public.is_empty() {
            public
        } else {
            match ex {
                Exposure::NodePort => format!("http://{}:{}", c.host, c.http_port),
                Exposure::Ingress => format!("http://{}", c.opts.get("ingress_host")),
                _ => format!("http://private-store.{}.svc.cluster.local:{PORT}", c.namespace),
            }
        };
        let secret = "private-store";
        let mut args = vec!["store-server".to_string(), "--data".into(), "/data".into(), "--listen".into(), format!("0.0.0.0:{PORT}"), "--publish-token-file".into(), "/secrets/admin-token".into(), "--catalog-seed-file".into(), "/secrets/catalog-seed".into(), "--name".into(), "Private store".into(), "--id".into(), format!("private-{}", c.namespace.trim_start_matches("iyakki-").chars().take(20).collect::<String>().trim_end_matches('-'))];
        if c.opts.flag("read_only") {
            args.push("--read-only".into());
        }
        if c.opts.flag("require_trusted") {
            args.push("--require-trusted".into());
        }
        let args_yaml: String = args.iter().map(|a| format!("        - \"{a}\"\n")).collect();
        let mut m = String::new();
        m.push_str(&format!("apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: private-store-data\n  labels:\n{}spec:\n  accessModes:\n  - ReadWriteOnce\n", labels(c, 4)));
        if let Some(sc) = &c.storage_class {
            m.push_str(&format!("  storageClassName: {sc}\n"));
        }
        m.push_str(&format!("  resources:\n    requests:\n      storage: {}\n---\n", c.opts.get("storage_size")));
        m.push_str(&format!(
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: private-store\n  labels:\n{l4}spec:\n  replicas: 1\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n{s6}  template:\n    metadata:\n      labels:\n{l8}    spec:\n      automountServiceAccountToken: false\n      securityContext:\n        runAsNonRoot: true\n        runAsUser: 10001\n        runAsGroup: 10001\n        fsGroup: 10001\n        seccompProfile:\n          type: RuntimeDefault\n      containers:\n      - name: private-store\n        image: {image}\n        imagePullPolicy: IfNotPresent\n        args:\n{args_yaml}        ports:\n        - name: http\n          containerPort: {PORT}\n        startupProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          periodSeconds: 3\n          failureThreshold: 30\n        readinessProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          periodSeconds: 10\n          failureThreshold: 3\n        livenessProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          initialDelaySeconds: 10\n          periodSeconds: 20\n          failureThreshold: 6\n        resources:\n          requests:\n            cpu: 50m\n            memory: 64Mi\n          limits:\n            cpu: 500m\n            memory: 256Mi\n        securityContext:\n          allowPrivilegeEscalation: false\n          readOnlyRootFilesystem: true\n          runAsNonRoot: true\n          capabilities:\n            drop:\n            - ALL\n        volumeMounts:\n        - name: data\n          mountPath: /data\n        - name: secrets\n          mountPath: /secrets\n          readOnly: true\n      volumes:\n      - name: data\n        persistentVolumeClaim:\n          claimName: private-store-data\n      - name: secrets\n        secret:\n          secretName: {secret}\n          defaultMode: 0440\n---\n",
            l4 = labels(c, 4),
            s6 = selector(c, 6),
            l8 = labels(c, 8)
        ));
        let svc_type = match ex {
            Exposure::NodePort => "NodePort",
            Exposure::LoadBalancer => "LoadBalancer",
            _ => "ClusterIP",
        };
        m.push_str(&format!("apiVersion: v1\nkind: Service\nmetadata:\n  name: private-store\n  labels:\n{}spec:\n  type: {svc_type}\n  selector:\n{}  ports:\n  - name: http\n    port: {PORT}\n    targetPort: http\n", labels(c, 4), selector(c, 4)));
        if ex == Exposure::NodePort && c.http_port != 0 {
            m.push_str(&format!("    nodePort: {}\n", c.http_port));
        }
        m.push_str("---\n");
        if ex == Exposure::Ingress {
            m.push_str(&format!("apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: private-store\n  labels:\n{}spec:\n  rules:\n  - host: {}\n    http:\n      paths:\n      - path: /\n        pathType: Prefix\n        backend:\n          service:\n            name: private-store\n            port:\n              number: {PORT}\n---\n", labels(c, 4), c.opts.get("ingress_host")));
        }
        let manifest = m.trim_end_matches("---\n").to_string();
        let urls = vec![UrlInfo { label: "Private store".into(), url: base.clone(), kind: "web".into() }, UrlInfo { label: "Catalog".into(), url: format!("{base}/catalog.json"), kind: "catalog".into() }, UrlInfo { label: "Publish".into(), url: format!("{base}/v1/publish"), kind: "api".into() }];
        let mut notes = vec!["The server runs as user 10001 with a read-only root file system; packages and the catalog live on the PVC. The catalog key never leaves the pod: the pod derives it from a seed in the Secret `private-store`, which is also in your vault.".to_string(), "The server speaks plain HTTP: put TLS in front (an Ingress with a certificate, or a sidecar) before anything but a trusted network reaches it; IYAKKI refuses to send a publish token over http to a public address.".into()];
        if ex == Exposure::ClusterIP {
            notes.push(format!("ClusterIP: reach it with kubectl port-forward svc/private-store {PORT}:{PORT} -n {}.", c.namespace));
        }
        Ok(Built {
            manifest,
            secrets: vec![SecretSpec {
                name: secret.into(),
                entries: vec![
                    SecretEntry { key: "admin-token".into(), vault: format!("{}.admin-token", c.vault_prefix), label: "Private store admin token".into(), kind: SecretKind::Token, reveal: true },
                    SecretEntry { key: "catalog-seed".into(), vault: format!("{}.catalog-seed", c.vault_prefix), label: "Private store catalog key seed".into(), kind: SecretKind::Token, reveal: false },
                ],
            }],
            urls,
            notes,
        })
    }
    fn workloads(&self) -> Vec<Workload> {
        vec![Workload { name: "private-store".into(), container: "private-store".into(), image: 0 }]
    }
    fn health_path(&self) -> Option<String> {
        Some("/healthz".into())
    }
    fn how_to_use(&self, urls: &[UrlInfo], _o: &Options) -> String {
        let web = urls.iter().find(|u| u.kind == "web").map(|u| u.url.as_str()).unwrap_or("");
        format!("Open STORE, Settings, Private store and choose Add this store (address {web}): compare the key fingerprint shown with the one the store prints, then confirm. Publish with `iyakki pkg publish --server {web} --token-env STORE_TOKEN file.iyp` or the assistant's store_publish tool (the token is a vault entry named store-token-<catalog id>). The admin token is in your vault as shown after the deploy; use it only to add publishers and withdraw packages.")
    }
    fn backup_hint(&self) -> String {
        "Back up the PVC (packages, publishers.json, state.json, audit.log). The catalog key is derived from the seed in the Secret and your vault: keep that safe, because a new seed means a new key and every device must add the store again.".into()
    }
    fn exposes(&self, o: &Options) -> Vec<String> {
        let e = exposure(o);
        let mut v = vec![format!("{}: {}", e.word(), e.explain()), "Read access to the catalog and the packages is open to anyone who can reach it; publishing needs a token".into(), "Plain HTTP: terminate TLS in the ingress before a network you do not trust can reach it".into()];
        if e == Exposure::NodePort {
            v.push("A NodePort is open on every node of the cluster".into());
        }
        v
    }
    fn post_deploy(&self, p: &mut PostCtx<'_>) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        for id in publisher_ids(&p.opts)? {
            let name = format!("{}.publisher-{id}", p.vault_prefix);
            if p.p.vault.has(&name) {
                lines.push(format!("kept the existing token of publisher {id}"));
                p.added_vault.push(name);
                continue;
            }
            let o = p.kc.kubectl(&["exec", "deploy/private-store", "-n", &p.namespace, "-c", &p.workload.container, "--", "/iyakki", "store-server", "publisher-add", "--data", "/data", "--catalog-seed-file", "/secrets/catalog-seed", "--pub-id", &id], 30, 60);
            let said = o.stdout.clone();
            let tok = said.split_whitespace().find(|w| w.starts_with("iyk_")).map(str::to_string);
            match (o.ok(), tok) {
                (true, Some(t)) => {
                    let t = zeroize::Zeroizing::new(t);
                    p.p.vault.set(&name, &t)?;
                    p.added_vault.push(name.clone());
                    lines.push(format!("created a publish token for {id} (vault entry uvault:{name}); hand it to them yourself"));
                }
                _ if o.combined().contains("already exists") => lines.push(format!("publisher {id} already exists (its token cannot be shown again)")),
                _ => return Err(format!("creating publisher {id} failed: {}", o.combined().lines().next().unwrap_or("").trim())),
            }
        }
        Ok(lines)
    }
}

/// [`ServiceDef::prepare_images`] with the places to look given (tests).
impl PrivateStore {
    /// Build the one-file image in memory (the same content as [`DEVICE_DOCKERFILE`]: the static program at `/iyakki`, user 10001,
    /// port 8080) and push it with the registry API: no podman or docker. `places` says where the program is looked for.
    #[allow(clippy::too_many_arguments)]
    pub fn push_with(&self, pl: &Platform, places: &localbin::Places, http: &dyn crate::ocipush::RegistryHttp, version: &str, arch: &str, registry: &str, repo_tag: &str, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        let _ = version;
        let bin = binary_for_image(pl, places, arch)?;
        let bytes = std::fs::read(&bin).map_err(|e| format!("reading {}: {e}", bin.display()))?;
        let want = localbin::Arch::parse(arch).unwrap_or_else(localbin::Arch::host);
        progress(&format!("packing {} ({} bytes, {}) into a one-file image", bin.display(), bytes.len(), want.uname()));
        let img = crate::ocipush::build_image(&crate::ocipush::ImageSpec { file: "iyakki", contents: &bytes, arch: want.node(), user: "10001:10001", args: vec![], port: PORT })?;
        let (repo, tag) = repo_tag.rsplit_once(':').ok_or_else(|| format!("`{repo_tag}` is not repository:tag"))?;
        let digest = crate::ocipush::push(http, registry, repo, tag, &img, &mut |l| progress(l)).map_err(|e| format!("pushing the store server image failed: {e}"))?;
        progress(&format!("pushed {registry}/{repo_tag} ({digest})"));
        Ok(())
    }

    pub fn prepare_with(&self, pl: &Platform, places: &localbin::Places, version: &str, arch: &str, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        let engine = ["podman", "docker"].into_iter().find(|e| pl.run.has(e)).ok_or("building the image needs podman or docker on this machine")?;
        let bin = binary_for_image(pl, places, arch)?;
        let dir = pl.dir.join("store-server").join(format!("build-{version}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::copy(&bin, dir.join("iyakki")).map_err(|e| format!("copying {}: {e}", bin.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("iyakki"), std::fs::Permissions::from_mode(0o555)).map_err(|e| format!("{}: {e}", dir.join("iyakki").display()))?;
        }
        std::fs::write(dir.join("Dockerfile"), DEVICE_DOCKERFILE).map_err(|e| e.to_string())?;
        progress(&format!("building the store server image from {}", bin.display()));
        let want = localbin::Arch::parse(arch).unwrap_or_else(localbin::Arch::host);
        let argv: Vec<String> = [engine, "build", "-t", &format!("{IMAGE}:{version}"), "--platform", &format!("linux/{}", want.node()), "."].iter().map(|s| s.to_string()).collect();
        let o = pl.run.run(&dir, &argv, &[], std::time::Duration::from_secs(600), 64 * 1024, pl.live.clone());
        if !o.ok() {
            return Err(format!("building the image failed: {}", o.combined().lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim()));
        }
        Ok(())
    }
}

/// The publisher ids of the `publishers` option, checked.
pub fn publisher_ids(o: &Options) -> Result<Vec<String>, String> {
    let v: Vec<String> = o.get("publishers").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if v.len() > 20 {
        return Err("Publishers: at most 20 at deploy time (add more later through the API)".into());
    }
    for id in &v {
        if !crate::store::manifest::valid_publisher_id(id) || id == "admin" {
            return Err(format!("Publishers: `{id}` is not a publisher id (2 to 32 lowercase letters, digits and dashes, and not admin)"));
        }
    }
    Ok(v)
}

/// What the Private store card shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusInfo {
    /// The deployed instance, if any (`id`, catalog address, state word).
    pub instance: Option<(String, String)>,
    /// The catalog source already added for it (source id, name).
    pub added: Option<(String, String)>,
    /// The source `publish` goes to by default.
    pub default_target: Option<String>,
}

pub fn status_info(data_dir: &std::path::Path, user: &str, store: &crate::store::Store) -> StatusInfo {
    let inst = crate::platform::installed::load(data_dir, user).into_iter().find(|i| i.service == ID);
    let cfg = store.config();
    let catalog = inst.as_ref().and_then(|i| i.urls.iter().find(|u| u.kind == "catalog").map(|u| u.url.clone()));
    let added = catalog.as_ref().and_then(|u| cfg.sources.iter().find(|s| &s.location == u)).map(|s| (s.id.clone(), s.name.clone()));
    StatusInfo { instance: inst.map(|i| (i.id.clone(), catalog.unwrap_or_default())), added, default_target: (!cfg.publish_target.is_empty()).then(|| cfg.publish_target.clone()) }
}

pub fn status_text(info: &StatusInfo) -> String {
    let mut t = match &info.instance {
        None => "No private store is deployed from this device (STORE, Settings, Private store, Deploy to k3s, or the assistant's platform_deploy with service private-store).".to_string(),
        Some((id, url)) => format!("Private store {id} is deployed; catalog {url}."),
    };
    match &info.added {
        Some((id, name)) => t.push_str(&format!(" It is added as the catalog {name} ({id}), key pinned.")),
        None if info.instance.is_some() => t.push_str(" It is not added to this device's STORE yet (Settings, Private store, Add this store)."),
        None => {}
    }
    if let Some(d) = &info.default_target {
        t.push_str(&format!(" Publishing goes to {d} by default."));
    }
    t
}
