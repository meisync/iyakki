//! The vault service of a cloud deployment (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3a): where the shared (tenant, namespace,
//! common) and personal vaults live when a launch is given no remote vault.
//!
//! [`manifests`] writes the objects of the in-cluster service: its own namespace, a volume, one hardened pod, a ClusterIP Service and a
//! NetworkPolicy that lets only the tenants' namespaces in and lets nothing out. The service stores ciphertext (keys are wrapped per
//! member, so the service never holds a key that opens a vault).
//!
//! The program that answers is `iyakki vault serve` (`cloudvaultd.rs`, [`SERVICE_PROGRAM_BUILT`]); a launch no longer refuses these objects
//! for want of it. Tried on a loopback socket only: the objects have not been applied to a real cluster.

use crate::cloudtenant::{dns_label, dns_subdomain};

/// Whether `iyakki vault serve` exists (`cloudvaultd.rs`). A launch that would run it refuses while this is false.
pub const SERVICE_PROGRAM_BUILT: bool = true;

pub const NAMESPACE: &str = "iyakki-vault";
pub const PORT: u16 = 8445;

/// What the tenants' pods are told (`IYAKKI_VAULT_URL`).
pub fn endpoint() -> String {
    format!("https://vault.{NAMESPACE}.svc:{PORT}")
}

/// A vault address: https only (the keys are wrapped, but the traffic still has a name and a size), a host name, optional port, no path.
pub fn check_url(u: &str) -> Result<(), String> {
    let rest = u.strip_prefix("https://").ok_or_else(|| format!("vault address \"{u}\" must start with https://"))?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, Some(p)),
        _ => (rest, None),
    };
    if !dns_subdomain(host) || host.len() > 253 {
        return Err(format!("vault address \"{u}\": \"{host}\" is not a host name"));
    }
    if port.is_some_and(|p| p.parse::<u32>().map_or(true, |n| n == 0 || n > 65535)) {
        return Err(format!("vault address \"{u}\": the port is not valid"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultService {
    pub image: String,
    pub storage: String,
    pub storage_class: Option<String>,
}

impl Default for VaultService {
    fn default() -> VaultService {
        VaultService { image: format!("iyakki:{}", env!("CARGO_PKG_VERSION")), storage: "1Gi".into(), storage_class: None }
    }
}

pub fn manifests(v: &VaultService) -> Result<String, String> {
    if v.image.is_empty() || !v.image.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b':' | b'@')) || v.image.starts_with(['-', '/', ':']) {
        return Err(format!("image \"{}\" is not an image reference", v.image));
    }
    let digits = v.storage.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || !matches!(&v.storage[digits.len()..], "Mi" | "Gi" | "Ti") {
        return Err(format!("storage \"{}\" is not a quantity (1Gi)", v.storage));
    }
    if let Some(c) = &v.storage_class {
        if !dns_subdomain(c) {
            return Err(format!("storage class \"{c}\" is not a name"));
        }
    }
    let _ = dns_label; // names below are fixed
    let ns = NAMESPACE;
    let labels = "app.kubernetes.io/name: iyakki-vault\n    iyakki.dev/role: vault";
    let sc = v.storage_class.as_ref().map(|s| format!("  storageClassName: {s}\n")).unwrap_or_default();
    Ok(format!(
        "# The vault service of this cluster (iyakki cloud launch). Runs `iyakki vault serve`.\n\
         apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {ns}\n  labels:\n    iyakki.dev/role: vault\n---\n\
         apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: vault-data\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  accessModes: [\"ReadWriteOnce\"]\n{sc}  resources:\n    requests:\n      storage: {st}\n---\n\
         apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: vault\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  replicas: 1          # one writer for the volume\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n      app.kubernetes.io/name: iyakki-vault\n  template:\n    metadata:\n      labels:\n        {labels}\n    spec:\n      automountServiceAccountToken: false\n      securityContext:\n        runAsNonRoot: true\n        runAsUser: 65532\n        runAsGroup: 65532\n        fsGroup: 65532\n        seccompProfile:\n          type: RuntimeDefault\n      containers:\n        - name: vault\n          image: {img}\n          imagePullPolicy: IfNotPresent\n          args: [\"vault\", \"serve\", \"--listen\", \"0.0.0.0:{port}\", \"--data\", \"/var/lib/iyakki-vault\"]\n          ports:\n            - {{ name: vault, containerPort: {port} }}\n          securityContext:\n            allowPrivilegeEscalation: false\n            readOnlyRootFilesystem: true\n            capabilities:\n              drop: [\"ALL\"]\n          readinessProbe:\n            tcpSocket: {{ port: vault }}\n            periodSeconds: 10\n          resources:\n            requests: {{ cpu: 50m, memory: 64Mi }}\n            limits: {{ cpu: \"1\", memory: 256Mi }}\n          volumeMounts:\n            - {{ name: data, mountPath: /var/lib/iyakki-vault }}\n            - {{ name: tmp, mountPath: /tmp }}\n      volumes:\n        - name: data\n          persistentVolumeClaim: {{ claimName: vault-data }}\n        - name: tmp\n          emptyDir: {{}}\n---\n\
         apiVersion: v1\nkind: Service\nmetadata:\n  name: vault\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  type: ClusterIP\n  selector:\n    app.kubernetes.io/name: iyakki-vault\n  ports:\n    - {{ name: vault, port: {port}, targetPort: vault }}\n---\n\
         apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: vault-only-from-tenants\n  namespace: {ns}\nspec:\n  podSelector: {{}}\n  policyTypes: [Ingress, Egress]\n  ingress:\n    - from:\n        - namespaceSelector:\n            matchExpressions:\n              - {{ key: iyakki.dev/tenant, operator: Exists }}\n      ports:\n        - {{ protocol: TCP, port: {port} }}\n  egress: []\n---\n",
        img = v.image,
        st = v.storage,
        port = PORT,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifests_make_a_namespace_a_volume_one_hardened_pod_and_a_policy_that_lets_only_tenants_in() {
        let y = manifests(&VaultService::default()).unwrap();
        for want in ["kind: Namespace", "name: iyakki-vault", "kind: PersistentVolumeClaim", "kind: Deployment", "kind: Service", "kind: NetworkPolicy", "runAsNonRoot: true", "readOnlyRootFilesystem: true", "drop: [\"ALL\"]", "automountServiceAccountToken: false", "key: iyakki.dev/tenant, operator: Exists", "egress: []", "policyTypes: [Ingress, Egress]"] {
            assert!(y.contains(want), "{want}\n{y}");
        }
        assert!(!y.contains("kind: Ingress"), "no way in from outside the cluster");
        assert!(!y.contains("hostNetwork") && !y.contains("privileged: true"));
    }

    #[test]
    fn values_that_could_add_a_line_of_yaml_are_refused() {
        for bad in ["a\nb", "x y", "", "-x", "a;b"] {
            assert!(manifests(&VaultService { image: bad.into(), ..Default::default() }).is_err(), "{bad:?}");
        }
        assert!(manifests(&VaultService { storage: "1Gi\nx".into(), ..Default::default() }).is_err());
        assert!(manifests(&VaultService { storage: "lots".into(), ..Default::default() }).is_err());
        assert!(manifests(&VaultService { storage_class: Some("a b".into()), ..Default::default() }).is_err());
    }

    #[test]
    fn a_vault_address_is_https_with_a_host_and_an_optional_port() {
        assert!(check_url(&endpoint()).is_ok());
        assert!(check_url("https://vault.example.org").is_ok() && check_url("https://vault.example.org:8445").is_ok());
        for bad in ["http://vault.example.org", "vault.example.org", "https://", "https://a b.example.org", "https://v.example.org:0", "https://v.example.org:99999", "https://v.example.org/x", "https://v.example.org\nx"] {
            assert!(check_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_tenant_with_a_vault_address_tells_its_pod_and_the_program_exists() {
        let mut c = crate::cloudtenant::TenantConfig::example("acme");
        c.vault_url = Some(endpoint());
        let y = crate::cloudtenant::manifests(&c).unwrap();
        assert!(y.contains("IYAKKI_VAULT_URL") && y.contains("vault.iyakki-vault.svc"), "{y}");
        c.vault_url = Some("http://plain".into());
        assert!(crate::cloudtenant::manifests(&c).is_err());
        assert!(SERVICE_PROGRAM_BUILT, "the program is `iyakki vault serve`, in cloudvaultd.rs");
    }
}
