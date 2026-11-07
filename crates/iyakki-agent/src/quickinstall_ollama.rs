//! The Quick install item "Ollama": a model server, now one pod in the managed cluster (like Lemonade) instead of a program on the host.
//!
//! * The image is the vendor's (`ollama/ollama` on Docker Hub, MIT), pulled by the cluster when the person clicks Install; IYAKKI ships
//!   nothing. It is pinned by tag in [`TAG`]; [`DIGEST`] stays empty until the first real pull has been checked against the registry (the
//!   job logs the digest the cluster resolved). A digest is never invented here.
//! * Namespace `iyakki-ai` (shared with Lemonade), a volume claim [`CLAIM`] (30 GB, kept when the item is removed), a ClusterIP Service,
//!   and the API published on this machine's loopback only ([`LOCAL_PORT`], so a host Ollama on 11434 and the GPU Ollama on 11435 never
//!   clash). The pod is unprivileged (an init container hands the volume to user 10001 and exits), has no capabilities, no service account
//!   token, a readiness probe, and CPU and memory limits from [`super::resources::OLLAMA`].
//! * The GPU is given the way Lemonade's is: the GPU steps patch the Deployment with the resource limit of the device plugin
//!   (`gpu_workloads::ollama_patch`); nothing privileged is added here.
//! * **Marker names are kept.** The item id is still `ollama` and its marker `quick-install/ollama.done`; the marker's text says where it
//!   runs: it starts with [`CLUSTER_WORD`] for the cluster. An older marker ("version 0.5.1") and the CLI catalogue record mean the HOST
//!   install, which keeps working untouched ([`Where`]).
//! * **Moving a host install** is a choice of the person, on a review card: the host models folder is copied into the claim only after
//!   they agree and there is room ([`decide`]); nothing is deleted from the host; the host server is stopped only after the cluster one
//!   answers (the shell does that on the request `ollama-host-stop`). When the copy would be too large to do safely in one step, the card
//!   offers "Start fresh in the cluster, keep host models where they are" instead ([`Mode::Fresh`]).

use super::*;

pub use super::LEMONADE_NAMESPACE as NS;
pub const NAME: &str = "ollama";
pub const CLAIM: &str = "ollama-data";
pub const STORAGE_GB: u64 = 30;
pub const REPO: &str = "docker.io/ollama/ollama";
/// The pinned tag (the same release the GPU step's Ollama uses).
pub const TAG: &str = "0.40.6";
/// Filled by a person after the first real pull has been checked against the registry; empty until then.
pub const DIGEST: &str = "";
/// The port inside the pod.
pub const CONTAINER_PORT: u16 = 11434;
/// The port on this machine's loopback.
pub const LOCAL_PORT: u16 = 11436;
/// The marker text of an install in the cluster starts with this.
pub const CLUSTER_WORD: &str = "in the cluster";
/// What the Quick install page says about an install on the host.
pub const HOST_WORD: &str = "Ollama runs on this machine's host";
/// The request files of the job to the shell.
pub const REQ_CONNECT: &str = "ollama";
pub const REQ_STOP_HOST: &str = "ollama-host-stop";
/// The titles of the two review cards (the shell tells them apart by the title, like the browser add-on's).
pub const MOVE_TITLE: &str = "Move Ollama into the cluster?";
pub const FRESH_TITLE: &str = "Start Ollama fresh in the cluster?";
/// The most the host models folder may be to copy in one step (no resume): bigger ones are offered "start fresh".
pub const MAX_COPY_BYTES: u64 = 20 << 30;
/// Room to keep free on the data partition after the copy (the copy exists twice for a while).
pub const KEEP_FREE_BYTES: u64 = 2 << 30;

pub const ABOUT: &str = "Run language models on this machine: Ollama as one pod in the cluster (namespace iyakki-ai, ClusterIP, published on this machine only at 127.0.0.1:11436, a 30 GB volume for the models, up to 16 GB of memory), added to Models as ollama-local. Models are pulled later from Settings \u{2192} Models. An Ollama installed earlier on this machine's host keeps working; this page offers to move it into the cluster";
pub const LICENCE: &str = "MIT (the Ollama image, pulled by the cluster from Docker Hub at your click, not shipped); each model you pull has its own licence, outside IYAKKI";

/// The image reference: the tag, with the digest when one has been recorded.
pub fn image() -> String {
    if DIGEST.is_empty() { format!("{REPO}:{TAG}") } else { format!("{REPO}:{TAG}@{DIGEST}") }
}

/// Where this machine's Ollama runs, from the marker text and the CLI catalogue record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    /// In the cluster; the marker's detail.
    Cluster(String),
    /// On the host: the version of the program.
    Host(String),
    Nowhere,
}

/// Does this presence detail or marker detail say the cluster?
pub fn is_cluster_detail(d: &str) -> bool {
    d.contains(CLUSTER_WORD)
}

pub fn where_runs(data: &Path, host_version: Option<String>) -> Where {
    match marked(data, Item::Ollama) {
        Some(d) if is_cluster_detail(&d) => Where::Cluster(d),
        _ => host_version.map_or(Where::Nowhere, Where::Host),
    }
}

/// The marker text of a finished install.
pub fn marker_detail(models: usize, how: &str) -> String {
    format!("{CLUSTER_WORD}: http://127.0.0.1:{LOCAL_PORT} on this machine, {NAME}.{NS}.svc:{CONTAINER_PORT} inside the cluster; {models} model(s){how}")
}

/// The presence line. `probe` asks a loopback port for Ollama's version.
pub fn presence(data: &Path, host_version: Option<String>, elsewhere: Option<String>, probe: &dyn Fn(u16) -> Option<String>) -> Presence {
    match where_runs(data, host_version) {
        Where::Cluster(d) => match probe(LOCAL_PORT) {
            Some(v) => Presence::Installed(format!("{d} · answering (version {v})")),
            None => Presence::Earlier(d),
        },
        Where::Host(v) => Presence::Installed(format!("version {v} · {HOST_WORD}")),
        Where::Nowhere => elsewhere.map_or(Presence::Missing, Presence::Elsewhere),
    }
}

// ------------------------------------------------------------------------------------------------ the manifest

/// The namespace, the volume claim, one Deployment and a ClusterIP Service.
pub fn manifest() -> String {
    let r = super::resources::OLLAMA;
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {claim}
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: {gb}Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  namespace: {ns}
  labels:
    app.kubernetes.io/name: {name}
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {name}
        app.kubernetes.io/managed-by: iyakki
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        runAsGroup: 10001
        seccompProfile:
          type: RuntimeDefault
      initContainers:
        - name: own-the-volume
          image: {img}
          command: [sh, -c, \"mkdir -p /data/models /data/home && chown -R 10001:10001 /data\"]
          securityContext:
            runAsNonRoot: false
            runAsUser: 0
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
              add: [CHOWN, DAC_OVERRIDE, FOWNER]
          volumeMounts:
            - name: data
              mountPath: /data
      containers:
        - name: {name}
          image: {img}
          env:
            - name: OLLAMA_HOST
              value: 0.0.0.0:{cport}
            - name: OLLAMA_MODELS
              value: /data/models
            - name: HOME
              value: /data/home
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
          ports:
            - containerPort: {cport}
              hostPort: {lport}
              hostIP: 127.0.0.1
          resources:
            requests:
              cpu: {creq}
              memory: {mreq}
            limits:
              cpu: \"{clim}\"
              memory: {mlim}
          readinessProbe:
            httpGet:
              path: /
              port: {cport}
            initialDelaySeconds: 5
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /data
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: {claim}
---
apiVersion: v1
kind: Service
metadata:
  name: {name}
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: {name}
  ports:
    - port: {cport}
      targetPort: {cport}
",
        ns = NS,
        claim = CLAIM,
        gb = STORAGE_GB,
        name = NAME,
        img = image(),
        cport = CONTAINER_PORT,
        lport = LOCAL_PORT,
        creq = r.cpu_req,
        clim = r.cpu_lim,
        mreq = r.mem_req,
        mlim = r.mem_lim,
    )
}

// ------------------------------------------------------------------------------------------------ the choice

/// What the person chose for an Ollama on the host; the Store leaves it for the job as a request file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Copy the host models into the claim, then (after the cluster answers) stop the host server.
    Move,
    /// A new, empty install in the cluster; the host models stay where they are; the host server stops after the cluster answers.
    Fresh,
}

fn mode_file(data: &Path) -> PathBuf {
    state_dir(data).join("ollama-mode")
}

pub fn write_mode(data: &Path, m: Mode) -> Result<(), String> {
    std::fs::create_dir_all(state_dir(data)).map_err(|e| e.to_string())?;
    std::fs::write(mode_file(data), if m == Mode::Move { "move" } else { "fresh" }).map_err(|e| e.to_string())
}

/// The waiting choice, taken (removed) at once.
pub fn take_mode(data: &Path) -> Option<Mode> {
    let p = mode_file(data);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    match t.trim() {
        "move" => Some(Mode::Move),
        "fresh" => Some(Mode::Fresh),
        _ => None,
    }
}

/// The host models folder of IYAKKI's own host install.
pub fn host_models_dir(data: &Path) -> PathBuf {
    data.join("ollama").join("models")
}

/// What the machine says about the host install (a fake in tests).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub models_dir: PathBuf,
    pub models_bytes: u64,
    /// Free bytes on the data partition; `None` when unknown.
    pub free_bytes: Option<u64>,
}

/// Read the host install's models folder and the free room of the data partition.
pub fn real_host_info(data: &Path) -> HostInfo {
    let dir = host_models_dir(data);
    HostInfo { models_bytes: super::uninstall::dir_bytes(&dir), free_bytes: crate::sharedsite::statvfs_free(data), models_dir: dir }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The copy is safe in one step.
    Copy,
    /// Too large to copy in one step (no resume): offer "start fresh".
    TooLarge,
    /// The data partition has no room for a second copy.
    NoRoom,
}

/// May the host models be copied into the claim now? The folder must fit the claim and the one-step limit, and the data partition must
/// keep [`KEEP_FREE_BYTES`] free after the copy.
pub fn decide(h: &HostInfo) -> Decision {
    let claim = STORAGE_GB << 30;
    if h.models_bytes > MAX_COPY_BYTES.min(claim / 10 * 8) {
        return Decision::TooLarge;
    }
    match h.free_bytes {
        Some(f) if f < h.models_bytes.saturating_add(KEEP_FREE_BYTES) => Decision::NoRoom,
        _ => Decision::Copy,
    }
}

fn gb(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", b >> 20) }
}

/// The review card's lines for a host install.
pub fn review_lines(mode: Mode, h: &HostInfo, d: Decision) -> Vec<String> {
    let r = super::resources::OLLAMA;
    let mut v = vec![
        format!("{HOST_WORD}. This puts a new Ollama into the cluster: one pod in the namespace {NS}, published on this machine only at http://127.0.0.1:{LOCAL_PORT}, with a {STORAGE_GB} GB volume for the models."),
        format!("It may use up to {} of memory and {}; it asks for {} and {}.", super::resources::mem_words(r.mem_lim), super::resources::cpu_words(r.cpu_lim), super::resources::mem_words(r.mem_req), super::resources::cpu_words(r.cpu_req)),
        format!("The cluster downloads the Ollama image from Docker Hub when you confirm (about 1.5 GB). Ollama is MIT licensed; IYAKKI does not ship it. {}", LICENCE),
    ];
    match mode {
        Mode::Move => {
            v.push(format!("Your models: the folder {} holds {}. After you confirm, they are copied into the volume of the new pod (a copy; nothing is deleted from the host).", h.models_dir.display(), gb(h.models_bytes)));
            match d {
                Decision::Copy => v.push(format!("There is room: {} free on the data partition.", h.free_bytes.map_or("enough".to_string(), gb))),
                Decision::TooLarge => v.push(format!("This is too large to copy safely in one step (the limit is {}). Close this card and choose \"Start fresh in the cluster, keep host models where they are\" instead.", gb(MAX_COPY_BYTES))),
                Decision::NoRoom => v.push(format!("There is not enough room for a second copy ({} free, {} needed plus {} to spare). Free some space, or choose \"Start fresh in the cluster, keep host models where they are\".", h.free_bytes.map_or("unknown".to_string(), gb), gb(h.models_bytes), gb(KEEP_FREE_BYTES))),
            }
        }
        Mode::Fresh => {
            v.push(format!("Your models stay where they are: the folder {} ({}) is not touched and not copied. The new Ollama starts empty; pull models later from Settings \u{2192} Models.", h.models_dir.display(), gb(h.models_bytes)));
        }
    }
    v.push("The Ollama on the host keeps running until the new one answers; only then is it stopped. The program stays installed and nothing is deleted. Models then points at the cluster one (ollama-local).".into());
    v
}

// ------------------------------------------------------------------------------------------------ the job

pub(crate) fn run(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    let mode = env.ollama_mode();
    let host = env.ollama_host();
    // decided before anything is applied: a refused copy changes nothing
    if mode == Some(Mode::Move) {
        match decide(&host) {
            Decision::Copy => {}
            Decision::TooLarge => return Err(format!("The host models ({}) are too large to copy safely in one step, so nothing was changed. Choose \"Start fresh in the cluster, keep host models where they are\" instead.", gb(host.models_bytes))),
            Decision::NoRoom => return Err(format!("There is not enough room on the data partition for a second copy of the host models ({} needed), so nothing was changed. Free some space, or choose \"Start fresh in the cluster, keep host models where they are\".", gb(host.models_bytes))),
        }
    }
    env.say(&format!("[0/7] Ollama runs in k3s: checking the cluster first (licence: {})", Item::Ollama.licence()));
    wait_for_cluster(env, wait, poll)?;
    let out = env.kubectl_apply(NAME, &manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say(&format!("[1/7] waiting for the pod (the first start pulls {}, about 1.5 GB)", image()));
    env.rollout(NS, NAME, Duration::from_secs(1200))?;
    let image_id = env.image_id(NS, NAME).unwrap_or_default();
    if !image_id.is_empty() {
        env.say(&format!("  image resolved by the cluster: {image_id} (to pin it, put this digest in the item's digest field after checking it against the registry)"));
    }
    let mut how = String::new();
    if mode == Some(Mode::Move) {
        env.say(&format!("[2/7] copying the host models ({}) into the volume; the host folder is not changed", gb(host.models_bytes)));
        let r = env.ollama_copy(&host.models_dir)?;
        env.say(&format!("  {r}"));
        how = "; the host models were copied in (the host folder is kept)".into();
    } else if mode == Some(Mode::Fresh) {
        env.say("[2/7] starting fresh: the host models stay where they are");
        how = "; started fresh (the host models folder is kept)".into();
    } else {
        env.say("[2/7] a new install: no models yet");
    }
    env.say("[3/7] waiting for the Ollama of the cluster to answer");
    let mut waited = Duration::ZERO;
    let models = loop {
        match env.ollama_models(true) {
            Ok(m) => break m,
            Err(e) => {
                if waited >= wait.min(Duration::from_secs(600)) {
                    return Err(format!("Ollama in the cluster did not answer on 127.0.0.1:{LOCAL_PORT} ({e}); the host Ollama was left as it is"));
                }
                env.sleep(poll);
                waited += poll;
            }
        }
    };
    if mode == Some(Mode::Move) {
        // every model the host lists must be in the cluster now (when the host answers, otherwise the copy's own check stands)
        if let Ok(on_host) = env.ollama_models(false) {
            let missing: Vec<&String> = on_host.iter().filter(|m| !models.contains(m)).collect();
            if !missing.is_empty() {
                return Err(format!("The copy is incomplete: {} of the host's models are not in the cluster ({}). The host Ollama was left running and nothing was deleted; press Move again or choose Start fresh.", missing.len(), missing.iter().take(3).map(|m| m.as_str()).collect::<Vec<_>>().join(", ")));
            }
        }
    }
    env.say(&format!("[4/7] it answers; {} model(s) in the cluster", models.len()));
    env.request_after(REQ_CONNECT)?;
    if mode.is_some() {
        env.request_after(REQ_STOP_HOST)?;
        env.say("[5/7] asked IYAKKI to point Models (ollama-local) at it and then stop the host Ollama");
    } else {
        env.say("[5/7] asked IYAKKI to add it to Settings \u{2192} Models as ollama-local");
    }
    let detail = marker_detail(models.len(), &how);
    env.mark(Item::Ollama, &detail)?;
    env.say(&format!("[6/7] done: {detail}"));
    env.say("[7/7] models are pulled from Settings \u{2192} Models and kept in the volume ollama-data (30 GB claimed); removing Ollama keeps them unless you also delete the data");
    Ok(())
}

/// Where Ollama runs and what it uses, for the chat tool and the status line.
pub fn status_line(w: &Where) -> String {
    let r = super::resources::OLLAMA;
    match w {
        Where::Cluster(d) => format!("Ollama runs in the cluster ({d}). Pod: deployment {NAME} in {NS}; {}", r.text()),
        Where::Host(v) => format!("{HOST_WORD} (version {v}, models in the data folder). It can be moved into the cluster from Store \u{2192} Setup \u{2192} Ollama."),
        Where::Nowhere => "Ollama is not installed.".into(),
    }
}
