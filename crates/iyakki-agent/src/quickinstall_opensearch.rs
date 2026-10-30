//! The Quick install item "OpenSearch (search engine, open source)": OpenSearch (Apache-2.0) as one pod in the managed cluster, the
//! open-source choice next to Elasticsearch (which is source-available, not open source).
//!
//! * The image is the project's (`opensearchproject/opensearch` on Docker Hub), pinned by tag AND digest in [`IMAGE`]. The cluster
//!   pulls it when the person clicks Install; IYAKKI does not ship it.
//! * Namespace `iyakki-data`, a ClusterIP Service `opensearch` (port 9200), a 5 GB claim, 1.5 Gi memory limit, a readiness probe on
//!   the cluster health. Nothing is published outside the cluster.
//! * One node, the security plugin switched off (no sign-in, no TLS: the Service is reachable inside the cluster only, exactly as the
//!   Elasticsearch item), and `node.store.allow_mmap=false` so the pod starts without raising `vm.max_map_count` on the host.
//! * DASH gets the shared source [`SOURCE_ID`] of kind `elasticsearch` (OpenSearch answers the same `_search`, `_cat` and `_mapping`
//!   calls) at the stable address [`DASH_URL`]. [`remove_source`] takes it away again.
//!
//! NOT tried against a live cluster in this repository's tests; the manifest is checked as text.

use super::*;

pub use super::NAMESPACE as NS;
/// Tag and digest of the image: the OCI index of `opensearchproject/opensearch:3.9.0` (amd64 and arm64), read from Docker Hub's registry
/// API on 2026-10-27. Change the tag and the digest here only, and verify the digest against the registry.
pub const IMAGE: &str = "docker.io/opensearchproject/opensearch:3.9.0@sha256:3b55f6b6249cfdb92b52ee59adc8a266203ba9ef9f5d82544fad6f1af2f3609e";
/// The id of the shared DASH source this item registers.
pub const SOURCE_ID: &str = "opensearch-cluster";
/// How DASH and the AI reach it (a Service of the cluster, looked up at every connect).
pub const DASH_URL: &str = "cluster://iyakki-data/opensearch:9200";

pub const ABOUT: &str = "A search engine you can use freely (OpenSearch, Apache-2.0): one single-node pod in the cluster (namespace iyakki-data, ClusterIP only, port 9200, 5 GB volume, about 1.5 GB of memory), no sign-in because nothing outside the cluster can reach it. DASH gets it as the source \"Cluster OpenSearch\", through its Elasticsearch connector (the calls are the same)";
pub const LICENCE: &str = "OpenSearch is Apache-2.0 licensed (checked in the LICENSE.txt of the pinned release 3.9.0). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";

/// A volume claim, one unprivileged Deployment and a ClusterIP Service. Non-root (1000), no capabilities, no privilege escalation, no
/// service account token, no hostPath.
pub fn manifest() -> String {
    format!(
        "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: opensearch-data
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
  name: opensearch
  namespace: {ns}
  labels:
    app.kubernetes.io/name: opensearch
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: opensearch
  template:
    metadata:
      labels:
        app.kubernetes.io/name: opensearch
        app.kubernetes.io/managed-by: iyakki
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: opensearch
          image: {img}
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [ALL]
          env:
            - name: discovery.type
              value: single-node
            - name: DISABLE_SECURITY_PLUGIN
              value: \"true\"
            - name: DISABLE_INSTALL_DEMO_CONFIG
              value: \"true\"
            - name: node.store.allow_mmap
              value: \"false\"
            - name: OPENSEARCH_JAVA_OPTS
              value: -Xms512m -Xmx512m
          ports:
            - containerPort: 9200
          resources:
            requests:
              cpu: 100m
              memory: 1Gi
            limits:
              memory: 1536Mi
          readinessProbe:
            httpGet:
              path: /_cluster/health?local=true
              port: 9200
            initialDelaySeconds: 30
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /usr/share/opensearch/data
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: opensearch-data
---
apiVersion: v1
kind: Service
metadata:
  name: opensearch
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: opensearch
  ports:
    - port: 9200
      targetPort: 9200
",
        ns = NS,
        img = IMAGE
    )
}

/// The shared DASH source: the Elasticsearch connector at the stable cluster address (single node, no sign-in).
pub fn source() -> crate::dash::connector::SourceConfig {
    let mut values = std::collections::BTreeMap::new();
    values.insert("url".to_string(), DASH_URL.to_string());
    crate::dash::connector::SourceConfig { id: SOURCE_ID.into(), kind: "elasticsearch".into(), name: "Cluster OpenSearch".into(), values, ..Default::default() }
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
    env.say(&format!("[0/5] OpenSearch runs in k3s: checking the cluster first (licence: {})", Item::OpenSearch.licence()));
    wait_for_cluster(env, wait, poll)?;
    env.say(&format!("[1/5] k3s is Ready; making the namespace {NS}"));
    let out = env.kubectl_apply("data-namespace", &data_namespace_manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(4) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say("[2/5] applying the OpenSearch objects (unprivileged, ClusterIP only, 5 GB volume)");
    let out = env.kubectl_apply("opensearch", &manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say("[3/5] waiting for the pod (the first start pulls the OpenSearch image, about 1 GB)");
    env.rollout(NS, "opensearch", Duration::from_secs(900))?;
    let in_cluster = format!("http://opensearch.{NS}.svc:9200");
    let detail = match env.register_dash_source(source()) {
        Ok(()) => {
            env.say(&format!("[4/5] added to DASH as the source \"Cluster OpenSearch\" ({DASH_URL}: IYAKKI looks the Service up each time, nothing listens on localhost)"));
            format!("{in_cluster} inside the cluster; DASH source at {DASH_URL}")
        }
        Err(e) => {
            env.say(&format!("[4/5] could not add it to DASH ({e}); add an Elasticsearch source with {DASH_URL} yourself"));
            format!("{in_cluster} inside the cluster")
        }
    };
    env.mark(Item::OpenSearch, &detail)?;
    env.say(&format!("[5/5] done: {detail}"));
    Ok(())
}
