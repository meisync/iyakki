//! A cloud tenant as configuration: one tenant, one namespace, one pod (docs/development/CLOUD-MULTITENANT-PLAN.md, step 1).
//!
//! [`TenantConfig`] is the file a person (or the OS's Cloud page) fills in; [`manifests`] turns it into the Kubernetes objects of that
//! tenant: a namespace, a quota, the data volume, a pod that runs IYAKKI as the web desktop or as the limited cloud service, a service,
//! a network policy and, when there is a domain, an ingress (HTTP and WebSocket). Everything that reaches the YAML is checked first
//! (names, quantities, host names), so a value can never add a line of YAML. Nothing here starts a process or touches a cluster:
//! `cloudcli.rs` applies the text with `kubectl`.
//!
//! Passwords are never in the manifests: each member's first password is read from the Secret `iyakki-accounts` (key
//! `password-<member>`), made by the person with `kubectl create secret` (the command is printed by [`secret_command`]).

use serde::{Deserialize, Serialize};

/// What the pod runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// The whole desktop streamed to a browser (`iyakki serve --web`), the usual way to work in a tenant.
    #[default]
    Web,
    /// The limited web service (`iyakki serve --profile cloud`): sign-in and the allow-listed assistant, flows and data routes only.
    Cloud,
}

/// One person of the tenant. The first start makes the account if it is missing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub name: String,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "user".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfig {
    /// A DNS label: `acme`. The namespace is `iyakki-<tenant>`.
    pub tenant: String,
    /// The public name (`acme.example.org`): makes an ingress. Without it the tenant is reached with `port-forward`.
    #[serde(default)]
    pub domain: Option<String>,
    /// The Secret (type `kubernetes.io/tls`) with the domain's certificate. Needed for the cloud profile; with the web profile the
    /// ingress ends the TLS with it.
    #[serde(default)]
    pub tls_secret: Option<String>,
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default)]
    pub profile: Profile,
    #[serde(default = "default_storage")]
    pub storage: String,
    #[serde(default)]
    pub storage_class: Option<String>,
    #[serde(default = "default_cpu")]
    pub cpu_limit: String,
    #[serde(default = "default_memory")]
    pub memory_limit: String,
    /// Networks the cloud profile accepts connections from (the pod network and the ingress): a CIDR each.
    #[serde(default = "default_allow_from")]
    pub allow_from: Vec<String>,
    #[serde(default = "default_members")]
    pub members: Vec<Member>,
    /// Mount the operator's cluster-wide Secret `iyakki-common` (the common vault, `cvault:NAME`, read-only) into the pod.
    #[serde(default)]
    pub common_vault: bool,
    /// How the files of the tenant's people are kept apart (`cloudstorage.rs`): folders of the one volume (the only one built).
    #[serde(default)]
    pub isolation: crate::cloudstorage::Isolation,
    /// The vault service the tenant's shared and personal vaults are kept in (`cloudvault.rs`): the cluster's own, or a remote one.
    /// Without it the vaults stay on the tenant's volume.
    #[serde(default)]
    pub vault_url: Option<String>,
    /// Whether the people of the tenant may switch on their browser's microphone, camera and clipboard reading (`browsermedia.rs`).
    /// Default deny.
    #[serde(default)]
    pub media: crate::browsermedia::Allow,
}

fn default_image() -> String {
    format!("iyakki:{}", env!("CARGO_PKG_VERSION"))
}
fn default_storage() -> String {
    "2Gi".into()
}
fn default_cpu() -> String {
    "2".into()
}
fn default_memory() -> String {
    "1Gi".into()
}
fn default_allow_from() -> Vec<String> {
    vec!["10.0.0.0/8".into()]
}
fn default_members() -> Vec<Member> {
    vec![Member { name: "admin".into(), role: "admin".into() }]
}

impl TenantConfig {
    /// A configuration for `tenant` with every default (what `iyakki cloud example` prints).
    pub fn example(tenant: &str) -> TenantConfig {
        TenantConfig {
            tenant: tenant.into(),
            domain: None,
            tls_secret: None,
            image: default_image(),
            profile: Profile::Web,
            storage: default_storage(),
            storage_class: None,
            cpu_limit: default_cpu(),
            memory_limit: default_memory(),
            allow_from: default_allow_from(),
            members: default_members(),
            common_vault: false,
            isolation: Default::default(),
            vault_url: None,
            media: Default::default(),
        }
    }

    pub fn parse(text: &str) -> Result<TenantConfig, String> {
        let c: TenantConfig = serde_json::from_str(text).map_err(|e| format!("tenant configuration: {e}"))?;
        c.check()?;
        Ok(c)
    }

    pub fn namespace(&self) -> String {
        format!("iyakki-{}", self.tenant)
    }

    /// Refuse anything that is not a plain name, quantity or host name, and a configuration that could not work.
    pub fn check(&self) -> Result<(), String> {
        if !dns_label(&self.tenant) || self.tenant.len() > 40 {
            return Err(format!("tenant \"{}\": use lower-case letters, digits and - (at most 40, starting with a letter)", self.tenant));
        }
        self.isolation.check()?;
        if let Some(u) = &self.vault_url {
            crate::cloudvault::check_url(u)?;
        }
        if let Some(d) = &self.domain {
            if !host_name(d) {
                return Err(format!("domain \"{d}\" is not a host name (for example acme.example.org)"));
            }
        }
        if let Some(t) = &self.tls_secret {
            if !dns_subdomain(t) {
                return Err(format!("tls_secret \"{t}\" is not a Secret name"));
            }
        }
        if !image_ref(&self.image) {
            return Err(format!("image \"{}\" is not an image name (name:tag)", self.image));
        }
        if !quantity(&self.storage) || !quantity(&self.memory_limit) || !cpu_quantity(&self.cpu_limit) {
            return Err("storage, cpu_limit and memory_limit are Kubernetes quantities (2Gi, 1Gi, 2 or 500m)".into());
        }
        if let Some(c) = &self.storage_class {
            if !dns_subdomain(c) {
                return Err(format!("storage_class \"{c}\" is not a storage class name"));
            }
        }
        if self.members.is_empty() || self.members.len() > 50 {
            return Err("members: between 1 and 50 people".into());
        }
        let mut seen = std::collections::HashSet::new();
        for m in &self.members {
            if !dns_label(&m.name) || m.name.len() > 32 {
                return Err(format!("member \"{}\": lower-case letters, digits and - (at most 32, starting with a letter)", m.name));
            }
            if !["admin", "operator", "user", "guest"].contains(&m.role.as_str()) {
                return Err(format!("member \"{}\": role \"{}\" is not admin, operator, user or guest", m.name, m.role));
            }
            if !seen.insert(m.name.clone()) {
                return Err(format!("member \"{}\" is listed twice", m.name));
            }
        }
        if !self.members.iter().any(|m| m.role == "admin") {
            return Err("members: the tenant needs an administrator".into());
        }
        if self.profile == Profile::Cloud {
            if self.tls_secret.is_none() {
                return Err("the cloud profile serves HTTPS itself: name the certificate Secret in tls_secret".into());
            }
            if self.allow_from.is_empty() || self.allow_from.len() > 16 || !self.allow_from.iter().all(|c| cidr(c)) {
                return Err("allow_from: one to sixteen networks as CIDR (10.0.0.0/8); 0.0.0.0/0 is not accepted".into());
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------------------------ checks

pub(crate) fn dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_lowercase()
        && *b.last().unwrap_or(&b'-') != b'-'
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub(crate) fn dns_subdomain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(|p| !p.is_empty() && p.len() <= 63 && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') && !p.starts_with('-') && !p.ends_with('-'))
}

pub(crate) fn host_name(s: &str) -> bool {
    s.contains('.') && dns_subdomain(s)
}

fn image_ref(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.starts_with(['-', '/', ':']) && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b':' | b'@'))
}

fn quantity(s: &str) -> bool {
    let digits = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &s[digits.len()..];
    !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|c| c.is_ascii_digit()) && matches!(unit, "Ki" | "Mi" | "Gi" | "Ti" | "K" | "M" | "G" | "T")
}

fn cpu_quantity(s: &str) -> bool {
    let (n, unit) = match s.strip_suffix('m') {
        Some(n) => (n, true),
        None => (s, false),
    };
    !n.is_empty() && n.len() <= 6 && n.bytes().all(|c| c.is_ascii_digit() || (!unit && c == b'.')) && n.bytes().any(|c| c.is_ascii_digit())
}

fn cidr(s: &str) -> bool {
    let Some((ip, bits)) = s.split_once('/') else { return false };
    let Ok(ip) = ip.parse::<std::net::IpAddr>() else { return false };
    let Ok(bits) = bits.parse::<u8>() else { return false };
    bits >= 1 && bits <= if ip.is_ipv4() { 32 } else { 128 }
}

// ------------------------------------------------------------------------------------------------------------------ manifests

/// The command that makes the Secret with the members' first passwords (`<PASSWORD>` stands for each one; never printed with a value).
pub fn secret_command(c: &TenantConfig) -> String {
    let mut s = format!("kubectl -n {} create secret generic iyakki-accounts", c.namespace());
    for m in &c.members {
        s.push_str(&format!(" --from-literal=password-{}='<PASSWORD of {}>'", m.name, m.name));
    }
    s
}

/// The tenant's `service.json` (the cloud profile's configuration), checked like the service checks its own file.
pub fn service_json(c: &TenantConfig) -> Result<String, String> {
    let v = serde_json::json!({
        "profile": "cloud",
        "listen": "0.0.0.0:8444",
        "lan": true,
        "allow_from": c.allow_from,
        "tls": { "cert": "/etc/iyakki/tls/tls.crt", "key": "/etc/iyakki/tls/tls.key" },
        "token_store": "/var/lib/iyakki/service/tokens.json",
        "data_dir": "/var/lib/iyakki",
        "cors_origins": c.domain.iter().map(|d| format!("https://{d}")).collect::<Vec<_>>(),
        "enable_tools": [],
    });
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::webservice::config::ServiceConfig::parse(&text)?.check()?;
    Ok(text)
}

/// All the objects of the tenant as one multi-document YAML text.
pub fn manifests(c: &TenantConfig) -> Result<String, String> {
    c.check()?;
    let ns = c.namespace();
    let cloud = c.profile == Profile::Cloud;
    // the port the pod listens on, and whether the pod does its own TLS
    let (port, scheme) = if cloud { (8444, "HTTPS") } else if c.domain.is_some() { (8080, "HTTP") } else { (8443, "HTTPS") };
    let labels = format!("app.kubernetes.io/name: iyakki\n    app.kubernetes.io/instance: {}\n    iyakki.dev/tenant: {}", c.tenant, c.tenant);
    let mut y = String::new();
    y.push_str(&format!(
        "# IYAKKI tenant \"{t}\": generated by `iyakki cloud manifests`; apply with `iyakki cloud deploy` (or kubectl apply -f).\n\
         # Before the first start make the accounts' Secret:\n#   {secret}\n{common}\
         apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {ns}\n  labels:\n    iyakki.dev/tenant: {t}\n---\n",
        t = c.tenant,
        secret = secret_command(c),
        common = if c.common_vault { "# and, once per cluster, the operator's common vault Secret `iyakki-common` in this namespace (iyakki cloud common ...)\n" } else { "" }
    ));
    y.push_str(&format!(
        "apiVersion: v1\nkind: ResourceQuota\nmetadata:\n  name: tenant\n  namespace: {ns}\nspec:\n  hard:\n    pods: \"6\"\n    persistentvolumeclaims: \"2\"\n    requests.storage: {st}\n    limits.cpu: \"{cpu}\"\n    limits.memory: {mem}\n---\n",
        st = c.storage,
        cpu = c.cpu_limit.parse::<f64>().map(|n| format!("{}", (n * 3.0).max(3.0))).unwrap_or_else(|_| "6".into()),
        mem = mem_times(&c.memory_limit, 3),
    ));
    y.push_str(&format!(
        "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: iyakki-data\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  accessModes: [\"ReadWriteOnce\"]\n{sc}  resources:\n    requests:\n      storage: {st}\n---\n",
        sc = c.storage_class.as_ref().map(|s| format!("  storageClassName: {s}\n")).unwrap_or_default(),
        st = c.storage
    ));
    if cloud {
        let json = service_json(c)?;
        let body: String = json.lines().map(|l| format!("    {l}\n")).collect();
        y.push_str(&format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: iyakki-service\n  namespace: {ns}\ndata:\n  service.json: |\n{body}---\n"));
    }
    // the pod: one account per init container (the image has no shell), then the server
    let mut inits = String::new();
    for m in &c.members {
        inits.push_str(&format!(
            "        - name: account-{n}\n          image: {img}\n          imagePullPolicy: IfNotPresent\n          args: [\"user\", \"add\", \"{n}\", \"--role\", \"{r}\", \"--if-missing\", \"--password-file\", \"/secret/password-{n}\"]\n          securityContext: {{ allowPrivilegeEscalation: false, readOnlyRootFilesystem: true, capabilities: {{ drop: [\"ALL\"] }} }}\n          volumeMounts:\n            - {{ name: data, mountPath: /var/lib/iyakki }}\n            - {{ name: accounts, mountPath: /secret, readOnly: true }}\n",
            n = m.name,
            r = m.role,
            img = c.image
        ));
    }
    let (args, env) = if cloud {
        ("[\"serve\", \"--profile\", \"cloud\", \"--config\", \"/etc/iyakki/service.json\"]".to_string(), String::new())
    } else {
        ("[\"serve\"]".to_string(), format!("          env:\n            - {{ name: IYAKKI_WEB, value: \"{}\" }}\n            - {{ name: IYAKKI_LOG, value: info }}\n", if scheme == "HTTP" { "http:8080" } else { "1" }))
    };
    // the vault service of the tenant, when it has one: the pod finds it in the environment
    let env = match (&c.vault_url, cloud) {
        (Some(u), false) => format!("{env}            - {{ name: IYAKKI_VAULT_URL, value: \"{u}\" }}\n            - {{ name: IYAKKI_TENANT, value: \"{}\" }}\n", c.tenant),
        (Some(u), true) => format!("          env:\n            - {{ name: IYAKKI_VAULT_URL, value: \"{u}\" }}\n            - {{ name: IYAKKI_TENANT, value: \"{}\" }}\n", c.tenant),
        (None, _) => env,
    };
    // what the browser may supply to the tenant's sessions (the web server of the pod reads it)
    let media = if c.media == crate::browsermedia::Allow::Allow { "allow" } else { "deny" };
    let env = if env.is_empty() { format!("          env:\n            - {{ name: IYAKKI_BROWSER_MEDIA, value: {media} }}\n") } else { format!("{env}            - {{ name: IYAKKI_BROWSER_MEDIA, value: {media} }}\n") };
    let mut mounts = "            - { name: data, mountPath: /var/lib/iyakki }\n            - { name: tmp, mountPath: /tmp }\n".to_string();
    let mut vols = "        - name: data\n          persistentVolumeClaim: { claimName: iyakki-data }\n        - name: tmp\n          emptyDir: {}\n        - name: accounts\n          secret: { secretName: iyakki-accounts }\n".to_string();
    if cloud {
        mounts.push_str("            - { name: service, mountPath: /etc/iyakki/service.json, subPath: service.json, readOnly: true }\n            - { name: tls, mountPath: /etc/iyakki/tls, readOnly: true }\n");
        vols.push_str(&format!("        - name: service\n          configMap: {{ name: iyakki-service }}\n        - name: tls\n          secret: {{ secretName: {} }}\n", c.tls_secret.clone().unwrap_or_default()));
    }
    if c.common_vault {
        mounts.push_str("            - { name: common, mountPath: /etc/iyakki/common, readOnly: true }\n");
        vols.push_str("        - name: common\n          secret: { secretName: iyakki-common, defaultMode: 0400 }\n");
    }
    y.push_str(&format!(
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: iyakki\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  replicas: 1          # one writer for the data volume\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n      app.kubernetes.io/name: iyakki\n      app.kubernetes.io/instance: {t}\n  template:\n    metadata:\n      labels:\n        app.kubernetes.io/name: iyakki\n        app.kubernetes.io/instance: {t}\n        iyakki.dev/tenant: {t}\n    spec:\n      automountServiceAccountToken: false\n      terminationGracePeriodSeconds: 10\n      securityContext:\n        runAsNonRoot: true\n        runAsUser: 65532\n        runAsGroup: 65532\n        fsGroup: 65532\n        seccompProfile:\n          type: RuntimeDefault\n      initContainers:\n{inits}      containers:\n        - name: iyakki\n          image: {img}\n          imagePullPolicy: IfNotPresent\n          args: {args}\n{env}          ports:\n            - {{ name: web, containerPort: {port} }}\n          securityContext:\n            allowPrivilegeEscalation: false\n            readOnlyRootFilesystem: true\n            capabilities:\n              drop: [\"ALL\"]\n          readinessProbe:\n            httpGet: {{ path: /healthz, port: web, scheme: {scheme} }}\n            periodSeconds: 10\n          livenessProbe:\n            httpGet: {{ path: /healthz, port: web, scheme: {scheme} }}\n            initialDelaySeconds: 20\n            periodSeconds: 20\n          resources:\n            requests: {{ cpu: 100m, memory: 128Mi }}\n            limits: {{ cpu: \"{cpu}\", memory: {mem} }}\n          volumeMounts:\n{mounts}      volumes:\n{vols}---\n",
        t = c.tenant,
        img = c.image,
        cpu = c.cpu_limit,
        mem = c.memory_limit,
    ));
    y.push_str(&format!(
        "apiVersion: v1\nkind: Service\nmetadata:\n  name: iyakki\n  namespace: {ns}\n  labels:\n    {labels}\nspec:\n  selector:\n    app.kubernetes.io/name: iyakki\n    app.kubernetes.io/instance: {t}\n  ports:\n    - {{ name: web, port: {port}, targetPort: web }}\n---\n",
        t = c.tenant
    ));
    // other tenants' pods cannot reach this one: only this namespace and the cluster's ingress controllers may connect
    y.push_str(&format!(
        "apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: tenant-ingress\n  namespace: {ns}\nspec:\n  podSelector: {{}}\n  policyTypes: [\"Ingress\"]\n  ingress:\n    - from:\n        - podSelector: {{}}\n        - namespaceSelector:\n            matchExpressions:\n              - {{ key: kubernetes.io/metadata.name, operator: In, values: [\"kube-system\", \"ingress-nginx\", \"traefik\"] }}\n"
    ));
    if let Some(d) = &c.domain {
        let tls = c.tls_secret.as_ref().map(|s| format!("  tls:\n    - hosts: [\"{d}\"]\n      secretName: {s}\n")).unwrap_or_default();
        y.push_str(&format!(
            "---\napiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: iyakki\n  namespace: {ns}\n  annotations:\n    # the desktop stream and the API use WebSocket: the controllers pass the upgrade through; long reads are not cut\n    nginx.ingress.kubernetes.io/proxy-read-timeout: \"3600\"\n    nginx.ingress.kubernetes.io/proxy-send-timeout: \"3600\"\n    nginx.ingress.kubernetes.io/backend-protocol: \"{scheme}\"\n    traefik.ingress.kubernetes.io/service.serversscheme: \"{sch}\"\nspec:\n{tls}  rules:\n    - host: {d}\n      http:\n        paths:\n          - {{ path: /, pathType: Prefix, backend: {{ service: {{ name: iyakki, port: {{ name: web }} }} }} }}\n",
            sch = scheme.to_lowercase()
        ));
    }
    Ok(y)
}

/// `1Gi` times `n` (the namespace may hold a few pods at once: the pod and a restart).
fn mem_times(q: &str, n: u64) -> String {
    let digits = q.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &q[digits.len()..];
    format!("{}{unit}", digits.parse::<u64>().unwrap_or(1) * n)
}

/// What a person reads after `deploy`: how to reach the tenant.
pub fn reach_text(c: &TenantConfig) -> String {
    let ns = c.namespace();
    let port = if c.profile == Profile::Cloud { 8444 } else if c.domain.is_some() { 8080 } else { 8443 };
    match &c.domain {
        Some(d) => format!("Open https://{d} once its DNS name points at the cluster's ingress.\nOr, for a look right now:  kubectl -n {ns} port-forward svc/iyakki 8443:{port}  and open https://localhost:8443"),
        None => format!("No domain: kubectl -n {ns} port-forward svc/iyakki 8443:{port}  then open https://localhost:8443"),
    }
}

#[cfg(test)]
#[path = "cloudtenant_tests.rs"]
mod tests;
