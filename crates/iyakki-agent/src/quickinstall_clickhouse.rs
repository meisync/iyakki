//! The Quick install item "ClickHouse (analytics database)": ClickHouse (Apache-2.0) as one pod in the managed cluster for DASH.
//!
//! * The image is the vendor's (`clickhouse/clickhouse-server` on Docker Hub), pinned by tag AND digest in [`IMAGE`]. The cluster pulls
//!   it when the person clicks Install; IYAKKI does not ship it.
//! * Namespace `iyakki-data`, a ClusterIP Service `clickhouse` (HTTP 8123 and native 9000), a 5 GB claim, 1 Gi memory limit, a readiness
//!   probe on `/ping`. Nothing is published outside the cluster.
//! * The password of the `default` user is generated in the job, goes into the Secret [`SECRET`] through a 0600 file and is read by the
//!   pod only through `secretKeyRef`. It is never in a manifest, a log, a job line, an audit line or a test. The shell then moves it into
//!   the person's vault as [`VAULT_NAME`] (the same handoff as the graph database; [`handoff`] names it).
//! * DASH gets the shared source [`SOURCE_ID`] at the stable address [`DASH_URL`] with the password as a vault reference.
//!   [`remove_source`] takes it away again (used when the item is removed).

use super::*;

/// Where the ClickHouse objects go (shared with Elasticsearch and the graph database).
pub use super::NAMESPACE as NS;
/// Tag and digest of the image. The digest is the OCI index of `clickhouse/clickhouse-server:26.8.9.10` (amd64 and arm64), read from
/// Docker Hub's registry API on 2026-10-26. Change the tag and the digest here only, and verify the digest against the registry.
pub const IMAGE: &str = "docker.io/clickhouse/clickhouse-server:26.8.9.10@sha256:85ddbad63bd6779d4b3c64af17a0720c85e9dc6e5fa636f80bfdbb6096bdc5ab";
/// The id of the shared DASH source this item registers.
pub const SOURCE_ID: &str = "clickhouse-cluster";
/// How DASH and the AI reach it (a Service of the cluster, looked up at every connect).
pub const DASH_URL: &str = "cluster://iyakki-data/clickhouse:8123";
/// The Kubernetes Secret with the password (keys `CLICKHOUSE_PASSWORD` and `password`).
pub const SECRET: &str = "clickhouse-auth";
/// The vault entry that holds the same password for DASH.
pub const VAULT_NAME: &str = "svc.clickhouse";
/// The user the password belongs to.
pub const USER: &str = "default";
/// The request file the job leaves for the shell's vault handoff.
pub const REQUEST: &str = "clickhouse-vault";

pub const ABOUT: &str = "An analytics database for DASH and the assistant (ClickHouse): one pod in the cluster (namespace iyakki-data, ClusterIP only, HTTP 8123 and native 9000, 5 GB volume, about 1 GB of memory), with a password generated for you and kept in your vault. It is added to DASH as the source \"Cluster ClickHouse\"; a ClickHouse source you added by hand at localhost then offers \"Use cluster address\"";
pub const LICENCE: &str = "ClickHouse is Apache-2.0 licensed. It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";

/// What the vault handoff says about this service.
pub fn handoff() -> VaultNames {
    VaultNames { what: "ClickHouse", vault_name: VAULT_NAME, source: "Cluster ClickHouse", secret: SECRET }
}

/// A volume claim, one unprivileged Deployment and a ClusterIP Service. The password is NOT here: the pod reads it from the Secret
/// by reference. Non-root (101), no capabilities, no privilege escalation, no service account token, no hostPath. The root file system
/// stays writable because the image's entrypoint writes the user's configuration at start (not tried under k3s).
pub fn manifest() -> String {
    format!(
        "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: clickhouse-data
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
  name: clickhouse
  namespace: {ns}
  labels:
    app.kubernetes.io/name: clickhouse
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: clickhouse
  template:
    metadata:
      labels:
        app.kubernetes.io/name: clickhouse
        app.kubernetes.io/managed-by: iyakki
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 101
        runAsGroup: 101
        fsGroup: 101
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: clickhouse
          image: {img}
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
          env:
            - name: CLICKHOUSE_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: {secret}
                  key: CLICKHOUSE_PASSWORD
          ports:
            - containerPort: 8123
            - containerPort: 9000
          resources:
            requests:
              cpu: 100m
              memory: 512Mi
            limits:
              cpu: \"1\"
              memory: 1Gi
          readinessProbe:
            httpGet:
              path: /ping
              port: 8123
            initialDelaySeconds: 10
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /var/lib/clickhouse
            - name: logs
              mountPath: /var/log/clickhouse-server
            - name: users
              mountPath: /etc/clickhouse-server/users.d
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: clickhouse-data
        - name: logs
          emptyDir: {{}}
        - name: users
          emptyDir: {{}}
---
apiVersion: v1
kind: Service
metadata:
  name: clickhouse
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: clickhouse
  ports:
    - name: http
      port: 8123
      targetPort: 8123
    - name: native
      port: 9000
      targetPort: 9000
",
        ns = NS,
        img = IMAGE,
        secret = SECRET
    )
}

/// The shared DASH source: SQL over the HTTP port at the stable cluster address, basic sign-in as [`USER`] with the password taken
/// from the vault entry [`VAULT_NAME`] (a reference, never the value).
pub fn source() -> crate::dash::connector::SourceConfig {
    let mut values = std::collections::BTreeMap::new();
    values.insert("url".to_string(), DASH_URL.to_string());
    values.insert("auth".to_string(), "basic".to_string());
    values.insert("username".to_string(), USER.to_string());
    values.insert("database".to_string(), "default".to_string());
    let mut secrets = std::collections::BTreeMap::new();
    secrets.insert("password".to_string(), format!("uvault:{VAULT_NAME}"));
    crate::dash::connector::SourceConfig { id: SOURCE_ID.into(), kind: "clickhouse".into(), name: "Cluster ClickHouse".into(), values, secrets }
}

/// Take the shared source away (the item was removed): from the account's file and from the shared one. Returns how many were removed.
pub fn remove_source(data: &std::path::Path, user: &str) -> usize {
    let mut n = 0;
    for who in [user, "system"] {
        if let Ok(true) = crate::dash::store::Sources::open(data, who).remove(SOURCE_ID) {
            n += 1;
        }
        if user == "system" {
            break;
        }
    }
    n
}

pub(crate) fn run(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    env.say(&format!("[0/7] ClickHouse runs in k3s: checking the cluster first (licence: {})", Item::ClickHouse.licence()));
    wait_for_cluster(env, wait, poll)?;
    env.say(&format!("[1/7] k3s is Ready; making the namespace {NS}"));
    let out = env.kubectl_apply("data-namespace", &data_namespace_manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(4) {
        env.say(&format!("  {}", l.trim()));
    }
    match env.clickhouse_secret()? {
        true => env.say(&format!("[2/7] made the password and the Secret {SECRET} (the password is not shown; it goes into your vault as {VAULT_NAME})")),
        false => env.say(&format!("[2/7] the Secret {SECRET} is there already; its password is kept")),
    }
    env.say("[3/7] applying the ClickHouse objects (unprivileged, ClusterIP only, 5 GB volume)");
    let out = env.kubectl_apply("clickhouse", &manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say("[4/7] waiting for the pod (the first start pulls the ClickHouse image, about 220 MB)");
    env.rollout(NS, "clickhouse", Duration::from_secs(900))?;
    env.say("[5/7] ClickHouse is running");
    let in_cluster = format!("http://clickhouse.{NS}.svc:8123");
    let detail = match env.register_dash_source(source()) {
        Ok(()) => {
            env.say(&format!("[6/7] added to DASH as the source \"Cluster ClickHouse\" ({DASH_URL}; every query is sent read-only)"));
            format!("{in_cluster} inside the cluster; DASH source at {DASH_URL}")
        }
        Err(e) => {
            env.say(&format!("[6/7] could not add it to DASH ({e}); add a ClickHouse source with {DASH_URL} yourself"));
            format!("{in_cluster} inside the cluster")
        }
    };
    env.request_after(REQUEST)?;
    env.mark(Item::ClickHouse, &detail)?;
    env.say(&format!("[7/7] done: {detail}. IYAKKI is putting the password into your vault as {VAULT_NAME}"));
    Ok(())
}
