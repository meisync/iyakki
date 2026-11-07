//! The Quick install items "SeaweedFS (object storage)" and "RustFS (object storage, Rust)": an S3-compatible object store as one pod in
//! the managed cluster, for DASH exports, workspaces and backups of your own.
//!
//! * Both are Apache-2.0 (the licence text of each upstream repository must be re-read by a person before a release: this file was
//!   written offline). The images are the vendors' (Docker Hub), pulled by the cluster when the person clicks Install; IYAKKI ships
//!   nothing. The tags are pinned in [`SEAWEEDFS`] and [`RUSTFS`]; the digest field is empty until the first real install resolves it
//!   (the job reads the pulled image's `imageID` and writes it in its log and in the item's record). A digest is never invented here.
//! * Namespace [`NS`] (one for both, so a person can run both side by side), a ClusterIP Service with the S3 port only, a 20 GB claim,
//!   a 1 Gi memory limit, a readiness probe. Non-root, no capabilities, no privilege escalation, no service account token, no hostPath.
//! * The access key and the secret key are generated in the job, written to the Secret `<name>-s3` through 0600 files and read by the
//!   pod only by reference (`secretKeyRef`, or a read-only mounted file for SeaweedFS' identity file). They are never in a manifest, a
//!   log, a job line, an audit line, a chat result or a test. [`finish`] then moves both into the person's vault
//!   (`svc.<name>.access-key`, `svc.<name>.secret-key`) and saves the typed record [`Connection`] as the vault entry `s3.<name>`, which
//!   holds the endpoint, region, bucket and the NAMES of the two key entries: never a key.
//! * Removing keeps the data unless the person asks to delete it ([`remove`]).

use super::*;

/// Where the object stores go.
pub const NS: &str = "iyakki-objects";
/// The bucket made on the first start where the store has a command for it (SeaweedFS); RustFS has none in its image, so the first
/// client makes it (`aws s3 mb`).
pub const BUCKET: &str = "iyakki";
/// The region the clients are told to sign with (these stores accept any region name).
pub const REGION: &str = "us-east-1";

/// What differs between the two stores.
#[derive(Debug)]
pub struct Engine {
    pub item: Item,
    /// Deployment, Service, claim prefix, label and vault name part.
    pub name: &'static str,
    pub title: &'static str,
    pub repo: &'static str,
    pub tag: &'static str,
    /// Filled by a person after the first real pull has been checked against the registry; empty until then (see the module text).
    pub digest: &'static str,
    pub uid: u32,
    pub port: u16,
    pub data_dir: &'static str,
    pub storage: &'static str,
    pub mem_request: &'static str,
    pub mem_limit: &'static str,
    /// The file the shell leaves for the vault handoff.
    pub request: &'static str,
}

pub const SEAWEEDFS: Engine = Engine {
    item: Item::SeaweedFs,
    name: "seaweedfs",
    title: "SeaweedFS",
    repo: "docker.io/chrislusf/seaweedfs",
    // read from a third-party deployment template while offline (2026-11-06): confirm the tag exists on Docker Hub
    tag: "4.02",
    digest: "",
    uid: 10001,
    port: 8333,
    data_dir: "/data",
    storage: "20Gi",
    mem_request: "256Mi",
    mem_limit: "1Gi",
    request: "seaweedfs-vault",
};

pub const RUSTFS: Engine = Engine {
    item: Item::RustFs,
    name: "rustfs",
    title: "RustFS",
    repo: "docker.io/rustfs/rustfs",
    // an alpha tag seen in the project's README mirrors while offline (2026-11-06): confirm the tag exists, prefer a stable one
    tag: "1.0.0-alpha.76",
    digest: "",
    uid: 10001,
    port: 9000,
    data_dir: "/data",
    storage: "20Gi",
    mem_request: "256Mi",
    mem_limit: "1Gi",
    request: "rustfs-vault",
};

pub const ENGINES: [&Engine; 2] = [&SEAWEEDFS, &RUSTFS];

pub fn engine_of(item: Item) -> Option<&'static Engine> {
    ENGINES.into_iter().find(|e| e.item == item)
}

/// An engine by its name or its Quick install id (`seaweedfs`, `rustfs`).
pub fn engine_named(s: &str) -> Option<&'static Engine> {
    let s = s.trim().to_ascii_lowercase();
    ENGINES.into_iter().find(|e| e.name == s)
}

pub const SEAWEEDFS_ABOUT: &str = "S3-compatible object storage you can rely on (SeaweedFS, mature): one pod in the cluster (namespace iyakki-objects, ClusterIP only, S3 port 8333, 20 GB volume, about 1 GB of memory), with an access key and a secret key generated for you and kept in your vault, and a bucket named iyakki made on the first start. Pick this one, or RustFS, or both";
pub const RUSTFS_ABOUT: &str = "S3-compatible object storage written in Rust (RustFS, young: its releases are still alpha, so use it for trying and for data you can lose): one pod in the cluster (namespace iyakki-objects, ClusterIP only, S3 port 9000, 20 GB volume, about 1 GB of memory), with an access key and a secret key generated for you and kept in your vault. The first client makes the bucket. Pick this one, or SeaweedFS, or both";
pub const SEAWEEDFS_LICENCE: &str = "SeaweedFS is Apache-2.0 licensed. It runs as its own pod and IYAKKI only talks to it over the S3 protocol; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";
pub const RUSTFS_LICENCE: &str = "RustFS is Apache-2.0 licensed (the name RustFS is its owner's trademark). It runs as its own pod and IYAKKI only talks to it over the S3 protocol; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";

impl Engine {
    /// The image reference: the tag, with the digest when one has been recorded.
    pub fn image(&self) -> String {
        if self.digest.is_empty() { format!("{}:{}", self.repo, self.tag) } else { format!("{}:{}@{}", self.repo, self.tag, self.digest) }
    }
    /// The Kubernetes Secret with `access-key`, `secret-key` (and the engine's own files).
    pub fn secret(&self) -> String {
        format!("{}-s3", self.name)
    }
    pub fn claim(&self) -> String {
        format!("{}-data", self.name)
    }
    pub fn access_vault(&self) -> String {
        format!("svc.{}.access-key", self.name)
    }
    pub fn secret_vault(&self) -> String {
        format!("svc.{}.secret-key", self.name)
    }
    /// The vault entry of the typed connection record.
    pub fn record_name(&self) -> String {
        format!("s3.{}", self.name)
    }
    /// The address inside the cluster (pods use it; this machine does not).
    pub fn endpoint(&self) -> String {
        format!("http://{}.{NS}.svc:{}", self.name, self.port)
    }
    pub fn about(&self) -> &'static str {
        if self.item == Item::SeaweedFs { SEAWEEDFS_ABOUT } else { RUSTFS_ABOUT }
    }
    pub fn licence(&self) -> &'static str {
        if self.item == Item::SeaweedFs { SEAWEEDFS_LICENCE } else { RUSTFS_LICENCE }
    }

    /// The container's arguments (and the mounted identity file for SeaweedFS).
    fn args(&self) -> Vec<String> {
        if self.item == Item::SeaweedFs {
            ["server", "-dir=/data", "-ip.bind=0.0.0.0", "-master.volumeSizeLimitMB=1024", "-volume.max=0", "-filer", "-s3", "-s3.port=8333", "-s3.config=/etc/iyakki-s3/s3.json"].iter().map(|s| s.to_string()).collect()
        } else {
            vec!["/data".into()]
        }
    }

    /// The Secret's entries for these keys: what each store reads.
    pub fn secret_values(&self, access: &str, secret: &str) -> Vec<(String, String)> {
        let mut v = vec![("access-key".to_string(), access.to_string()), ("secret-key".to_string(), secret.to_string())];
        if self.item == Item::SeaweedFs {
            let doc = serde_json::json!({"identities": [{"name": "iyakki", "credentials": [{"accessKey": access, "secretKey": secret}], "actions": ["Admin", "Read", "Write", "List", "Tagging"]}]});
            v.push(("s3.json".into(), doc.to_string()));
        }
        v
    }
}

pub fn namespace_manifest() -> String {
    format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NS}\n  labels:\n    app.kubernetes.io/managed-by: iyakki\n")
}

/// A volume claim, one unprivileged Deployment and a ClusterIP Service. The keys are NOT here: the pod reads them from the Secret by
/// reference. The root file system stays writable because neither image has been tried read-only under k3s.
pub fn manifest(e: &Engine) -> String {
    let (name, ns) = (e.name, NS);
    let args = e.args().iter().map(|a| format!("            - \"{a}\"\n")).collect::<String>();
    let env = if e.item == Item::RustFs {
        format!(
            "          env:
            - name: RUSTFS_ACCESS_KEY
              valueFrom:
                secretKeyRef:
                  name: {secret}
                  key: access-key
            - name: RUSTFS_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: {secret}
                  key: secret-key
            - name: RUSTFS_VOLUMES
              value: /data
            - name: RUSTFS_ADDRESS
              value: \":{port}\"
            - name: RUSTFS_CONSOLE_ENABLE
              value: \"false\"
",
            secret = e.secret(),
            port = e.port
        )
    } else {
        String::new()
    };
    let (mount, volume) = if e.item == Item::SeaweedFs {
        (
            "            - name: s3conf
              mountPath: /etc/iyakki-s3
              readOnly: true
",
            format!(
                "        - name: s3conf
          secret:
            secretName: {}
            defaultMode: 0440
            items:
              - key: s3.json
                path: s3.json
",
                e.secret()
            ),
        )
    } else {
        ("", String::new())
    };
    format!(
        "apiVersion: v1
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
      storage: {storage}
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
        runAsUser: {uid}
        runAsGroup: {uid}
        fsGroup: {uid}
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: {name}
          image: {img}
          args:
{args}          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
{env}          ports:
            - containerPort: {port}
          resources:
            requests:
              cpu: 100m
              memory: {mreq}
            limits:
              cpu: \"1\"
              memory: {mlim}
          readinessProbe:
            tcpSocket:
              port: {port}
            initialDelaySeconds: 10
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: {data}
{mount}      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: {claim}
{volume}---
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
    - name: s3
      port: {port}
      targetPort: {port}
",
        claim = e.claim(),
        storage = e.storage,
        uid = e.uid,
        img = e.image(),
        port = e.port,
        mreq = e.mem_request,
        mlim = e.mem_limit,
        data = e.data_dir,
    )
}

// ------------------------------------------------------------------------------------------------ the keys

/// A random text from `alphabet`, `len` characters (rejection sampling, no bias). Nothing is printed.
fn random_text(alphabet: &[u8], len: usize) -> Result<zeroize::Zeroizing<String>, String> {
    let limit = 256 - (256 % alphabet.len());
    let mut out = zeroize::Zeroizing::new(String::new());
    while out.len() < len {
        let mut b = zeroize::Zeroizing::new([0u8; 64]);
        iyakki_sec::random_fill(&mut b[..]).map_err(|e| format!("no random numbers for the keys: {e}"))?;
        for x in b.iter() {
            if (*x as usize) < limit && out.len() < len {
                out.push(alphabet[*x as usize % alphabet.len()] as char);
            }
        }
    }
    Ok(out)
}

/// A new access key (20 capitals and digits, like AWS') and secret key (40 letters and digits).
pub fn new_keys() -> Result<(zeroize::Zeroizing<String>, zeroize::Zeroizing<String>), String> {
    let a = random_text(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789", 20)?;
    let s = random_text(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", 40)?;
    Ok((a, s))
}

// ------------------------------------------------------------------------------------------------ the typed record

/// The typed connection record kept in the person's vault as `s3.<name>`. It holds NAMES of vault entries, never a key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub engine: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub path_style: bool,
    pub access_key_vault: String,
    pub secret_key_vault: String,
}

impl Connection {
    pub fn of(e: &Engine) -> Connection {
        Connection { engine: e.name.into(), endpoint: e.endpoint(), region: REGION.into(), bucket: BUCKET.into(), path_style: true, access_key_vault: e.access_vault(), secret_key_vault: e.secret_vault() }
    }
    /// A record is valid when it names an http(s) endpoint, a bucket and two vault entries (and holds no key text of its own).
    pub fn check(&self) -> Result<(), String> {
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) || self.endpoint.contains('@') {
            return Err("the endpoint must be an http(s) address without a sign-in in it".into());
        }
        let name_ok = |n: &str| !n.is_empty() && n.len() <= 80 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !name_ok(&self.bucket) || !name_ok(&self.access_key_vault) || !name_ok(&self.secret_key_vault) || self.region.trim().is_empty() {
            return Err("bucket, region and the two key names must be plain names".into());
        }
        Ok(())
    }
    pub fn to_text(&self) -> Result<String, String> {
        self.check()?;
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn from_text(t: &str) -> Result<Connection, String> {
        let c: Connection = serde_json::from_str(t).map_err(|e| format!("not an object storage record: {e}"))?;
        c.check()?;
        Ok(c)
    }
}

/// The shell's half of the handoff: read both keys from the cluster's Secret (a closure) and put them into the vault (a closure), then
/// save the record. Retries because the vault may be locked for a moment. Keys are never in the text that comes back.
pub fn finish(e: &Engine, read: &dyn Fn(&str) -> Result<zeroize::Zeroizing<String>, String>, store: &dyn Fn(&str, &str) -> Result<(), String>, sleep: &dyn Fn(Duration), tries: u32, wait: Duration) -> Result<String, String> {
    let mut last = String::from("not tried");
    for n in 0..tries.max(1) {
        if n > 0 {
            sleep(wait);
        }
        let step = (|| -> Result<(), String> {
            for (key, vault) in [("access-key", e.access_vault()), ("secret-key", e.secret_vault())] {
                let v = read(key).map_err(|x| format!("could not read the {key} from the cluster: {x}"))?;
                if v.trim().is_empty() {
                    return Err(format!("the Secret holds no {key}"));
                }
                store(&vault, v.trim()).map_err(|x| format!("could not put the {key} in your vault: {x}"))?;
            }
            store(&e.record_name(), &Connection::of(e).to_text()?).map_err(|x| format!("could not save the connection record: {x}"))
        })();
        match step {
            Ok(()) => return Ok(format!("the {} keys are in your vault as {} and {}; the connection record is {} (endpoint {}, bucket {BUCKET})", e.title, e.access_vault(), e.secret_vault(), e.record_name(), e.endpoint())),
            Err(x) => last = x,
        }
    }
    Err(format!("{last}. {} works; unlock your vault (sign in again) and press Install on {} again, or read the keys with kubectl -n {NS} get secret {} and save them as {} and {}", e.title, e.title, e.secret(), e.access_vault(), e.secret_vault()))
}

/// The real hooks of this machine for [`finish`]: kubectl and the person's vault.
pub fn finish_real(svc: &crate::services::Services, user: &str, e: &Engine) -> Result<String, String> {
    let read = |key: &str| -> Result<zeroize::Zeroizing<String>, String> {
        let out = svc.k3sdev.kubectl(&["-n", NS, "get", "secret", &e.secret(), "-o", &format!("go-template={{{{index .data \"{key}\" | base64decode}}}}")]);
        let v = zeroize::Zeroizing::new(out.stdout.clone());
        if out.code == Some(0) { Ok(v) } else { Err(out.stderr.trim().lines().last().unwrap_or("kubectl failed").to_string()) }
    };
    let store = |name: &str, v: &str| svc.user_set(user, name, v).map_err(|x| x.to_string());
    finish(e, &read, &store, &|d| std::thread::sleep(d), 40, Duration::from_secs(3))
}

// ------------------------------------------------------------------------------------------------ the job

pub(crate) fn run(e: &Engine, env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    env.say(&format!("[0/7] {} runs in k3s: checking the cluster first (licence: {})", e.title, e.item.licence()));
    wait_for_cluster(env, wait, poll)?;
    env.say(&format!("[1/7] k3s is Ready; making the namespace {NS}"));
    let out = env.kubectl_apply("objects-namespace", &namespace_manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(4) {
        env.say(&format!("  {}", l.trim()));
    }
    match env.objstore_secret(e)? {
        true => env.say(&format!("[2/7] made the access key, the secret key and the Secret {} (the keys are not shown; they go into your vault as {} and {})", e.secret(), e.access_vault(), e.secret_vault())),
        false => env.say(&format!("[2/7] the Secret {} is there already; its keys are kept", e.secret())),
    }
    env.say(&format!("[3/7] applying the {} objects (unprivileged, ClusterIP only, {} volume)", e.title, e.storage));
    let out = env.kubectl_apply(e.name, &manifest(e))?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say(&format!("[4/7] waiting for the pod (the first start pulls the {} image)", e.title));
    env.rollout(NS, e.name, Duration::from_secs(900))?;
    env.say(&format!("[5/7] {} is running", e.title));
    let image_id = env.image_id(NS, e.name).unwrap_or_default();
    if !image_id.is_empty() {
        env.say(&format!("  image resolved by the cluster: {image_id} (to pin it, put this digest in the item's digest field after checking it against the registry)"));
    }
    if e.item == Item::SeaweedFs {
        let script = format!("echo 's3.bucket.create -name {BUCKET}' | weed shell -master=localhost:9333 2>&1");
        let mut made = false;
        for n in 0..6 {
            if n > 0 {
                env.sleep(Duration::from_secs(5));
            }
            if let Ok(o) = env.pod_exec(NS, e.name, e.name, &script, "") {
                made = !o.to_lowercase().contains("error") || o.to_lowercase().contains("exist");
                if made {
                    break;
                }
            }
        }
        if made {
            env.say(&format!("[6/7] the bucket {BUCKET} is there"));
        } else {
            env.say(&format!("[6/7] could not make the bucket {BUCKET} yet; make it with any S3 client (aws s3 mb s3://{BUCKET})"));
        }
    } else {
        env.say(&format!("[6/7] RustFS has no bucket command in its image: make the bucket with any S3 client (aws s3 mb s3://{BUCKET})"));
    }
    env.request_after(e.request)?;
    let detail = format!("{} inside the cluster (S3, path style, region {REGION}, bucket {BUCKET}){}", e.endpoint(), if image_id.is_empty() { String::new() } else { format!("; image {image_id}") });
    env.mark(e.item, &detail)?;
    env.say(&format!("[7/7] done: {detail}. IYAKKI is putting the keys into your vault as {} and {}", e.access_vault(), e.secret_vault()));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ remove

/// What [`remove`] needs from the machine (fakes in tests).
pub trait RemoveEnv {
    /// `kubectl <args>`: its output, or the plain reason it failed.
    fn kubectl(&mut self, args: &[&str]) -> Result<String, String>;
    /// Take the item's marker away.
    fn unmark(&mut self, item: Item);
    /// Delete one vault entry of the person (not an error when it is not there).
    fn vault_delete(&mut self, name: &str) -> Result<(), String>;
}

/// Remove an object store. The Deployment and the Service always go; the volume (the data), the Secret and the person's vault entries
/// go only with `delete_data`, so a later install finds the data and the same keys. Returns one plain line.
pub fn remove(e: &Engine, env: &mut dyn RemoveEnv, delete_data: bool) -> Result<String, String> {
    for kind in ["deployment", "service"] {
        env.kubectl(&["-n", NS, "delete", kind, e.name, "--ignore-not-found"])?;
    }
    if delete_data {
        env.kubectl(&["-n", NS, "delete", "pvc", &e.claim(), "--ignore-not-found"])?;
        env.kubectl(&["-n", NS, "delete", "secret", &e.secret(), "--ignore-not-found"])?;
        for n in [e.access_vault(), e.secret_vault(), e.record_name()] {
            env.vault_delete(&n)?;
        }
    }
    env.unmark(e.item);
    Ok(if delete_data {
        format!("{} removed with its data, its keys and its connection record", e.title)
    } else {
        format!("{} removed; the data ({}) and the keys ({}, and your vault entries) are kept for a later install", e.title, e.claim(), e.secret())
    })
}
