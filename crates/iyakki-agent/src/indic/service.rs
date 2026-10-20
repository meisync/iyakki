//! The Indic voice server as a platform service (`indic-tts`): one Deployment of `dist/tts-indic` (numpy + onnxruntime, no model in
//! the image), a PVC that holds the model folders, a Service and an optional Ingress. Built on this machine from the Dockerfile in
//! `dist/tts-indic/` (the build fetches a Python base image and two wheels once; nothing else leaves the machine) and pushed to the
//! in-cluster registry, never Docker Hub. Models are copied into the volume afterwards (`kubectl cp`, shown in the how-to), or the
//! pod is pointed at a PVC that already holds them.

use crate::platform::*;

pub const ID: &str = "indic-tts";
pub const VERSIONS: [&str; 1] = ["1.0.0"];
pub const PORT: u16 = 8189;
pub const IMAGE: &str = "localhost/iyakki-tts-indic";
pub const DOCKERFILE: &str = include_str!("../../../../dist/tts-indic/Dockerfile");
pub const SERVER_PY: &str = include_str!("../../../../dist/tts-indic/server.py");

pub struct IndicTts;

fn labels(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: indic-tts\n{pad}app.kubernetes.io/managed-by: iyakki\n{pad}app.kubernetes.io/version: \"{}\"\n{pad}{LABEL_SERVICE}: {ID}\n{pad}{LABEL_INSTANCE}: {}\n", c.version, c.instance)
}

fn selector(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: indic-tts\n{pad}{LABEL_INSTANCE}: {}\n", c.instance)
}

fn exposure(o: &Options) -> Exposure {
    Exposure::parse(o.get("exposure")).unwrap_or(Exposure::ClusterIP)
}

impl ServiceDef for IndicTts {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "Indic voices".into()
    }
    fn description(&self) -> String {
        "Neural text-to-speech for Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi and Indian English (AI4Bharat Indic-TTS as ONNX) in your cluster: an OpenAI-compatible speech server that IYAKKI's reader, on any device, can use. Holds no model in the image; the models sit on a volume. CPU only.".into()
    }
    fn kind(&self) -> &'static str {
        "voice"
    }
    fn versions(&self) -> Vec<String> {
        VERSIONS.iter().map(|s| s.to_string()).collect()
    }
    fn images(&self, version: &str) -> Vec<ImageRef> {
        vec![ImageRef::new(&format!("{IMAGE}:{version}"), &["amd64", "arm64"])]
    }
    fn options(&self) -> Vec<OptionDef> {
        vec![OptionDef::new("threads", "CPU threads", "Threads the neural networks use per request (1 to 8). A request is one piece of a story; two threads read a Tamil sentence in about a third of its spoken length on a modern core.", OptKind::Text, "2")]
    }
    fn check_options(&self, o: &Options) -> Result<(), String> {
        let t = o.get("threads");
        if !t.is_empty() && !t.parse::<u8>().is_ok_and(|n| (1..=8).contains(&n)) {
            return Err("CPU threads: a number from 1 to 8".into());
        }
        if exposure(o) == Exposure::Ingress && o.get("ingress_host").is_empty() {
            return Err("Ingress host: give the host name".into());
        }
        Ok(())
    }
    fn always_mirror(&self) -> bool {
        true
    }
    fn prepare_images(&self, pl: &Platform, version: &str, arch: &str, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        let engine = ["podman", "docker"].into_iter().find(|e| pl.run.has(e)).ok_or("building the image needs podman or docker on this machine")?;
        let dir = pl.dir.join("tts-indic").join(format!("build-{version}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(dir.join("Dockerfile"), DOCKERFILE).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("server.py"), SERVER_PY).map_err(|e| e.to_string())?;
        progress("building the Indic voice server image (fetches the Python base image and two wheels once)");
        let want = localbin::Arch::parse(arch).unwrap_or_else(localbin::Arch::host);
        let argv: Vec<String> = [engine, "build", "-t", &format!("{IMAGE}:{version}"), "--platform", &format!("linux/{}", want.node()), "."].iter().map(|s| s.to_string()).collect();
        let o = pl.run.run(&dir, &argv, &[], std::time::Duration::from_secs(1200), 64 * 1024, pl.live.clone());
        if !o.ok() {
            return Err(format!("building the image failed: {}", o.combined().lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim()));
        }
        Ok(())
    }
    fn build(&self, c: &BuildCtx) -> Result<Built, String> {
        let ex = exposure(&c.opts);
        let image = c.images.first().ok_or("no image")?;
        let threads = if c.opts.get("threads").is_empty() { "2".to_string() } else { c.opts.get("threads").to_string() };
        let base = match ex {
            Exposure::NodePort => format!("http://{}:{}", c.host, c.http_port),
            Exposure::Ingress => format!("http://{}", c.opts.get("ingress_host")),
            _ => format!("http://indic-tts.{}.svc.cluster.local:{PORT}", c.namespace),
        };
        let mut m = String::new();
        m.push_str(&format!("apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: indic-tts-models\n  labels:\n{}spec:\n  accessModes:\n  - ReadWriteOnce\n", labels(c, 4)));
        if let Some(sc) = &c.storage_class {
            m.push_str(&format!("  storageClassName: {sc}\n"));
        }
        m.push_str(&format!("  resources:\n    requests:\n      storage: {}\n---\n", c.opts.get("storage_size")));
        m.push_str(&format!(
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: indic-tts\n  labels:\n{l4}spec:\n  replicas: 1\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n{s6}  template:\n    metadata:\n      labels:\n{l8}    spec:\n      automountServiceAccountToken: false\n      securityContext:\n        runAsNonRoot: true\n        runAsUser: 10001\n        runAsGroup: 10001\n        fsGroup: 10001\n        seccompProfile:\n          type: RuntimeDefault\n      containers:\n      - name: indic-tts\n        image: {image}\n        imagePullPolicy: IfNotPresent\n        env:\n        - name: IYAKKI_TTS_THREADS\n          value: \"{threads}\"\n        ports:\n        - name: http\n          containerPort: {PORT}\n        startupProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          periodSeconds: 3\n          failureThreshold: 30\n        readinessProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          periodSeconds: 10\n          failureThreshold: 3\n        livenessProbe:\n          httpGet:\n            path: /healthz\n            port: http\n          initialDelaySeconds: 10\n          periodSeconds: 20\n          failureThreshold: 6\n        resources:\n          requests:\n            cpu: 500m\n            memory: 1Gi\n          limits:\n            cpu: \"4\"\n            memory: 2Gi\n        securityContext:\n          allowPrivilegeEscalation: false\n          readOnlyRootFilesystem: true\n          runAsNonRoot: true\n          capabilities:\n            drop:\n            - ALL\n        volumeMounts:\n        - name: models\n          mountPath: /models\n        - name: tmp\n          mountPath: /tmp\n      volumes:\n      - name: models\n        persistentVolumeClaim:\n          claimName: indic-tts-models\n      - name: tmp\n        emptyDir:\n          medium: Memory\n          sizeLimit: 64Mi\n---\n",
            l4 = labels(c, 4),
            s6 = selector(c, 6),
            l8 = labels(c, 8)
        ));
        let svc_type = match ex {
            Exposure::NodePort => "NodePort",
            Exposure::LoadBalancer => "LoadBalancer",
            _ => "ClusterIP",
        };
        m.push_str(&format!("apiVersion: v1\nkind: Service\nmetadata:\n  name: indic-tts\n  labels:\n{}spec:\n  type: {svc_type}\n  selector:\n{}  ports:\n  - name: http\n    port: {PORT}\n    targetPort: http\n", labels(c, 4), selector(c, 4)));
        if ex == Exposure::NodePort && c.http_port != 0 {
            m.push_str(&format!("    nodePort: {}\n", c.http_port));
        }
        m.push_str("---\n");
        if ex == Exposure::Ingress {
            m.push_str(&format!("apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: indic-tts\n  labels:\n{}spec:\n  rules:\n  - host: {}\n    http:\n      paths:\n      - path: /\n        pathType: Prefix\n        backend:\n          service:\n            name: indic-tts\n            port:\n              number: {PORT}\n---\n", labels(c, 4), c.opts.get("ingress_host")));
        }
        let manifest = m.trim_end_matches("---\n").to_string();
        let urls = vec![UrlInfo { label: "Voice server".into(), url: base.clone(), kind: "api".into() }, UrlInfo { label: "Voices".into(), url: format!("{base}/v1/voices"), kind: "api".into() }];
        let mut notes = vec![
            "The pod runs as user 10001 with a read-only root file system. The image holds no model: copy the model folders into the volume (see how to use). Until a language is there, the server answers \"model not installed\" for it and IYAKKI reads with its next engine.".to_string(),
            "The server speaks plain HTTP and has no sign-in: anyone who can reach it can have text read aloud (nothing is stored or logged but the length of each request). Keep it on ClusterIP or the local network, or put an authenticating ingress in front.".into(),
        ];
        if ex == Exposure::ClusterIP {
            notes.push(format!("ClusterIP: reach it with kubectl port-forward svc/indic-tts {PORT}:{PORT} -n {}.", c.namespace));
        }
        Ok(Built { manifest, secrets: vec![], urls, notes })
    }
    fn workloads(&self) -> Vec<Workload> {
        vec![Workload { name: "indic-tts".into(), container: "indic-tts".into(), image: 0 }]
    }
    fn health_path(&self) -> Option<String> {
        Some("/healthz".into())
    }
    fn how_to_use(&self, urls: &[UrlInfo], _o: &Options) -> String {
        let api = urls.iter().find(|u| u.kind == "api").map(|u| u.url.as_str()).unwrap_or("");
        format!("Copy a language's model folder into the volume, for example: kubectl cp <data folder>/tts/indic/ta <namespace>/<pod>:/models/ta (the folder holds fastpitch.onnx, hifigan.onnx and vocab.json; Settings, Indic voices shows where yours are). Then in Settings, Indic voices, set the server address to {api} and choose Use Indic voices for reading. The server is OpenAI-compatible: POST {api}/v1/audio/speech with input, voice (ta-female, ta-male, hi-female, ...) and speed.")
    }
    fn backup_hint(&self) -> String {
        "The volume only holds model files that can be copied again from the device that installed them; no backup is needed.".into()
    }
    fn exposes(&self, o: &Options) -> Vec<String> {
        let e = exposure(o);
        let mut v = vec![format!("{}: {}", e.word(), e.explain()), "No sign-in: anyone who can reach the server can have text read aloud".into(), "Plain HTTP: the text of what is read crosses the network unencrypted unless TLS is put in front".into()];
        if e == Exposure::NodePort {
            v.push("A NodePort is open on every node of the cluster".into());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn ctx(opts: &[(&str, &str)]) -> BuildCtx {
        let mut o = Options::default();
        for (k, v) in [("storage_size", "5Gi"), ("exposure", "ClusterIP"), ("namespace", "iyakki-indic-tts"), ("threads", "2")].iter().chain(opts.iter()) {
            o.set(k, v);
        }
        BuildCtx { namespace: "iyakki-indic-tts".into(), opts: o, version: "1.0.0".into(), images: vec!["localhost:5000/iyakki-platform/iyakki-tts-indic:1.0.0".into()], storage_class: None, host: "10.0.0.5".into(), http_port: 30189, ssh_port: 0, vault_prefix: "svc.indic-tts".into(), instance: "indic-tts-iyakki-indic-tts".into() }
    }

    #[test]
    fn the_service_is_registered_and_its_manifest_passes_the_linter_with_no_warnings() {
        let def = crate::platform::spec(ID).expect("registered");
        assert_eq!((def.kind(), def.versions()), ("voice", vec!["1.0.0".to_string()]));
        assert!(def.always_mirror() && def.health_path().as_deref() == Some("/healthz"));
        for opts in [vec![], vec![("exposure", "NodePort")], vec![("exposure", "Ingress"), ("ingress_host", "tts.example.lan")], vec![("threads", "4")]] {
            let b = def.build(&ctx(&opts)).unwrap();
            let l = crate::deploylint::lint(&b.manifest, "iyakki-indic-tts", &|_| false);
            assert!(l.ok() && l.warnings.is_empty(), "{opts:?}: {}", l.report());
            let m = &b.manifest;
            for must in ["runAsNonRoot: true", "runAsUser: 10001", "readOnlyRootFilesystem: true", "allowPrivilegeEscalation: false", "automountServiceAccountToken: false", "- ALL", "claimName: indic-tts-models", "mountPath: /models", "path: /healthz"] {
                assert!(m.contains(must), "{must}");
            }
            assert!(!m.contains("privileged") && !m.contains("hostNetwork") && !m.contains("hostPath"));
            assert!(b.secrets.is_empty(), "no secret: the server has no sign-in");
            assert!(b.notes.iter().any(|n| n.contains("no sign-in") || n.contains("Keep it")));
        }
        let b = def.build(&ctx(&[("exposure", "NodePort")])).unwrap();
        assert!(b.manifest.contains("nodePort: 30189") && b.urls[0].url == "http://10.0.0.5:30189");
        let c = def.build(&ctx(&[])).unwrap();
        assert!(c.urls[0].url == "http://indic-tts.iyakki-indic-tts.svc.cluster.local:8189" && c.notes.iter().any(|n| n.contains("port-forward")));
        assert!(def.exposes(&ctx(&[("exposure", "NodePort")]).opts).iter().any(|e| e.contains("No sign-in")));
    }

    #[test]
    fn options_are_checked() {
        let mut o = Options::default();
        o.set("threads", "0");
        assert!(IndicTts.check_options(&o).is_err());
        o.set("threads", "9");
        assert!(IndicTts.check_options(&o).is_err());
        o.set("threads", "x");
        assert!(IndicTts.check_options(&o).is_err());
        o.set("threads", "3");
        o.set("exposure", "Ingress");
        assert!(IndicTts.check_options(&o).is_err(), "an ingress needs a host");
        o.set("ingress_host", "tts.example.lan");
        assert!(IndicTts.check_options(&o).is_ok());
    }

    #[test]
    fn the_dockerfile_is_safe_and_the_image_builds_from_what_is_embedded() {
        assert!(DOCKERFILE.contains("USER 10001:10001") && DOCKERFILE.contains("EXPOSE 8189") && DOCKERFILE.contains("python:3.12-slim"));
        assert!(!DOCKERFILE.contains("ADD http") && !DOCKERFILE.contains("curl "), "nothing fetched but the base image and two pinned wheels");
        assert!(DOCKERFILE.contains("numpy==") && DOCKERFILE.contains("onnxruntime=="), "pinned");
        assert!(!DOCKERFILE.lines().any(|l| l.trim_start().starts_with("COPY") && !l.contains("server.py")), "only the server is copied in: no model in the image");
        let d = std::env::temp_dir().join(format!("iyakki-indic-svc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = Arc::new(crate::wsexec::FakeRunner::new(&["podman"]));
        run.answer("podman build", "ok");
        let pl = crate::platform::Platform {
            run: run.clone(),
            dir: d.clone(),
            user: "root".into(),
            vault: Arc::new(crate::platform::MemVault::default()),
            audit: Arc::new(|_, _| {}),
            env: crate::devcluster::Env::real(),
            tmp: d.clone(),
            http: Arc::new(crate::platform::connect::FakeHttp::default()),
            gen: Arc::new(|n| "x".repeat(n)),
            live: None,
        };
        IndicTts.prepare_images(&pl, "1.0.0", "amd64", &mut |_| {}).unwrap();
        assert!(run.lines().iter().any(|l| l.starts_with("podman build -t localhost/iyakki-tts-indic:1.0.0") && l.contains("--platform linux/amd64")), "{:?}", run.lines());
        assert_eq!(std::fs::read_to_string(d.join("tts-indic/build-1.0.0/Dockerfile")).unwrap(), DOCKERFILE);
        assert!(std::fs::read_to_string(d.join("tts-indic/build-1.0.0/server.py")).unwrap().contains("/v1/audio/speech"));
        let none = crate::platform::Platform { run: Arc::new(crate::wsexec::FakeRunner::new(&[])), ..pl };
        assert!(IndicTts.prepare_images(&none, "1.0.0", "amd64", &mut |_| {}).unwrap_err().contains("podman or docker"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
