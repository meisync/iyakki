//! The Quick install item "Matrix server (chat)": Tuwunel (Apache-2.0), a small Matrix homeserver, as one pod in the managed cluster, so a
//! person can have a chat server of their own and use it with IYAKKI (notifications, flows) without an account anywhere else.
//!
//! * The image is the vendor's (`ghcr.io/matrix-construct/tuwunel`), pinned by tag AND digest in [`IMAGE`] (the digest is the OCI index of
//!   the release, read from the registry, never invented). The cluster pulls it when the person clicks Install; IYAKKI does not ship it.
//! * Namespace [`NS`] of its own, a ClusterIP Service `matrix` (client port 8008 only), a 5 GB claim, a 512 Mi memory limit, a readiness
//!   probe. Federation is OFF (the server talks to no other Matrix server and publishes nothing), the federation port is not exposed.
//! * Registration is not open. Tuwunel only accepts a registration with the token [`SECRET`] holds (open registration needs a flag this
//!   manifest never sets). The token is generated in the job, goes into the Secret through a 0600 file, is read by the pod only through
//!   `secretKeyRef`, and then moves into the person's vault as [`VAULT_NAME`] (the same handoff as the databases). It is never in a
//!   manifest, a log, a job line, an audit line or a test. The first account registered becomes the server's administrator.
//! * Nothing here creates the IYAKKI account: the person registers one (with any Matrix client, using the token), then Settings,
//!   Notifications, Matrix account takes the address [`HOMESERVER`] and that account's sign-in.

use super::*;

pub const NS: &str = "iyakki-matrix";
/// Tag and digest of the image: the OCI index of release v1.9.4 (amd64, arm64 and two amd64 variants), read from ghcr.io on 2026-10-26.
/// Change the tag and the digest here only, and verify the digest against the registry.
pub const IMAGE: &str = "ghcr.io/matrix-construct/tuwunel:v1.9.4@sha256:0e86c6164d6c9f5b60291938eb61ad8395021c37fe58d47a963ea7edddbedaec";
/// The name every account of this server ends with (`@name:iyakki.local`). It cannot be changed after the first start.
pub const SERVER_NAME: &str = "iyakki.local";
/// How IYAKKI reaches it (a Service of the cluster, looked up at every connect).
pub const HOMESERVER: &str = "cluster://iyakki-matrix/matrix:8008";
/// The Kubernetes Secret with the registration token (keys `TUWUNEL_REGISTRATION_TOKEN` and `token`).
pub const SECRET: &str = "matrix-registration";
/// The vault entry that holds the same token for the person.
pub const VAULT_NAME: &str = "svc.matrix.registration-token";
/// The request file the job leaves for the shell's vault handoff.
pub const REQUEST: &str = "matrix-vault";

pub const ABOUT: &str = "A chat server of your own (Tuwunel, a Matrix homeserver): one pod in the cluster (namespace iyakki-matrix, ClusterIP only, 5 GB volume, about 512 MB of memory). Registration is closed to anyone without a token that is generated for you and kept in your vault, and the server talks to no other server (federation off). Register an account with any Matrix client, then Settings, Notifications, Matrix account connects IYAKKI to it so flows and the assistant can message you";
pub const LICENCE: &str = "Tuwunel is Apache-2.0 licensed (checked in the LICENSE file of the official repository matrix-construct/tuwunel and in the image label org.opencontainers.image.licenses of v1.9.4). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from ghcr.io when you click Install, IYAKKI does not ship it";

pub fn namespace_manifest() -> String {
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {NS}
  labels:
    app.kubernetes.io/managed-by: iyakki
"
    )
}

/// A volume claim, one unprivileged Deployment and a ClusterIP Service. The token is NOT here: the pod reads it from the Secret by
/// reference. Non-root, no capabilities, no privilege escalation, no service account token, no hostPath. The root file system stays
/// writable and the user id is chosen here, not by the image (not tried under k3s).
pub fn manifest() -> String {
    format!(
        "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: matrix-data
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 5Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: matrix
  namespace: {ns}
  labels:
    app.kubernetes.io/name: matrix
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: matrix
  template:
    metadata:
      labels:
        app.kubernetes.io/name: matrix
        app.kubernetes.io/managed-by: iyakki
    spec:
      automountServiceAccountToken: false
      terminationGracePeriodSeconds: 1800
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        runAsGroup: 10001
        fsGroup: 10001
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: matrix
          image: {img}
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
          env:
            - name: TUWUNEL_SERVER_NAME
              value: \"{server}\"
            - name: TUWUNEL_DATABASE_PATH
              value: /var/lib/tuwunel
            - name: TUWUNEL_ADDRESS
              value: \"0.0.0.0\"
            - name: TUWUNEL_PORT
              value: \"8008\"
            - name: TUWUNEL_ALLOW_REGISTRATION
              value: \"true\"
            - name: TUWUNEL_REGISTRATION_TOKEN
              valueFrom:
                secretKeyRef:
                  name: {secret}
                  key: TUWUNEL_REGISTRATION_TOKEN
            - name: TUWUNEL_ALLOW_FEDERATION
              value: \"false\"
            - name: TUWUNEL_TRUSTED_SERVERS
              value: \"[]\"
            - name: TUWUNEL_MAX_REQUEST_SIZE
              value: \"10000000\"
            - name: TUWUNEL_DB_CACHE_CAPACITY_MB
              value: \"64\"
            - name: TUWUNEL_DB_POOL_MAX_WORKERS
              value: \"64\"
          ports:
            - containerPort: 8008
          resources:
            requests:
              cpu: 50m
              memory: 128Mi
            limits:
              memory: 512Mi
          startupProbe:
            httpGet:
              path: /_matrix/client/versions
              port: 8008
            periodSeconds: 10
            failureThreshold: 180
          readinessProbe:
            httpGet:
              path: /_matrix/client/versions
              port: 8008
            periodSeconds: 15
          volumeMounts:
            - name: data
              mountPath: /var/lib/tuwunel
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: matrix-data
---
apiVersion: v1
kind: Service
metadata:
  name: matrix
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: matrix
  ports:
    - name: client
      port: 8008
      targetPort: 8008
",
        ns = NS,
        img = IMAGE,
        secret = SECRET,
        server = SERVER_NAME
    )
}

/// The shell's half of the token handoff: read the token from the cluster's Secret (a closure), put it in the vault (a closure). Retries
/// because the vault may be locked for a moment. The token is never in the text that comes back.
pub fn handoff(read: &dyn Fn() -> Result<zeroize::Zeroizing<String>, String>, store: &dyn Fn(&str) -> Result<(), String>, sleep: &dyn Fn(Duration), tries: u32, wait: Duration) -> Result<String, String> {
    let mut last = String::from("not tried");
    for n in 0..tries.max(1) {
        if n > 0 {
            sleep(wait);
        }
        let t = match read() {
            Ok(t) if !t.trim().is_empty() => t,
            Ok(_) => {
                last = "the Secret holds no token".into();
                continue;
            }
            Err(e) => {
                last = format!("could not read the token from the cluster: {e}");
                continue;
            }
        };
        match store(t.trim()) {
            Ok(()) => return Ok(format!("the Matrix registration token is in your vault as {VAULT_NAME}; register an account with it, then add the account in Settings, Notifications, Matrix account (server {HOMESERVER})")),
            Err(e) => last = format!("could not put the token in your vault: {e}"),
        }
    }
    Err(format!("{last}. The server works; unlock your vault (sign in again) and press Install on the Matrix server again, or read the token with kubectl -n {NS} get secret {SECRET} and save it as {VAULT_NAME}"))
}

pub(crate) fn run(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    env.say(&format!("[0/7] the Matrix server runs in k3s: checking the cluster first (licence: {})", Item::Matrix.licence()));
    wait_for_cluster(env, wait, poll)?;
    env.say(&format!("[1/7] k3s is Ready; making the namespace {NS}"));
    let out = env.kubectl_apply("matrix-namespace", &namespace_manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(4) {
        env.say(&format!("  {}", l.trim()));
    }
    match env.matrix_secret()? {
        true => env.say(&format!("[2/7] made the registration token and the Secret {SECRET} (the token is not shown; it goes into your vault as {VAULT_NAME})")),
        false => env.say(&format!("[2/7] the Secret {SECRET} is there already; its token is kept")),
    }
    env.say("[3/7] applying the Matrix objects (unprivileged, ClusterIP only, registration needs the token, federation off, 5 GB volume)");
    let out = env.kubectl_apply("matrix", &manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say("[4/7] waiting for the pod (the first start pulls the Tuwunel image, about 35 MB)");
    env.rollout(NS, "matrix", Duration::from_secs(900))?;
    env.say("[5/7] the Matrix server is running");
    let detail = format!("http://matrix.{NS}.svc:8008 inside the cluster; IYAKKI address {HOMESERVER}; accounts end with :{SERVER_NAME}");
    env.say(&format!("[6/7] to use it with IYAKKI: register an account with any Matrix client (server {HOMESERVER} is for IYAKKI; a client on another computer needs its own way in), using the registration token from your vault, then Settings, Notifications, Matrix account"));
    env.request_after(REQUEST)?;
    env.mark(Item::Matrix, &detail)?;
    env.say(&format!("[7/7] done: {detail}. IYAKKI is putting the registration token into your vault as {VAULT_NAME}"));
    Ok(())
}
