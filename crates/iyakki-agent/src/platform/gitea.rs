//! The Gitea service (docs/PLATFORM-SERVICES.md, "Why Gitea"): a rootless Gitea Deployment (one replica, SQLite on a PVC), an HTTP and an
//! SSH Service, optional Ingress, a generated admin password in a Kubernetes Secret and the vault, an API token for IYAKKI created
//! inside the pod, `gitea dump` backups, upgrades along pinned versions; plus detection of a Gitea or Forgejo that already runs in the
//! cluster and "connect to existing" (URL plus a token the person types into the vault).

use super::*;

pub struct Gitea;

/// Known pinned versions of the rootless image, oldest first (the last is the default). The tags are public Docker Hub tags; pin
/// digests here once they are verified (`ImageRef::digest`).
pub const VERSIONS: [&str; 2] = ["1.23.8", "1.24.5"];
pub const HTTP_PORT: u16 = 3000;
pub const SSH_LISTEN: u16 = 2222;

fn labels(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: gitea\n{pad}app.kubernetes.io/managed-by: iyakki\n{pad}app.kubernetes.io/version: \"{}\"\n{pad}{LABEL_SERVICE}: gitea\n{pad}{LABEL_INSTANCE}: {}\n", c.version, c.instance)
}

fn selector(c: &BuildCtx, indent: usize) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}app.kubernetes.io/name: gitea\n{pad}{LABEL_INSTANCE}: {}\n", c.instance)
}

fn env(name: &str, value: &str) -> String {
    format!("        - name: {name}\n          value: \"{}\"\n", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Gitea {
    fn exposure(c: &BuildCtx) -> Exposure {
        Exposure::parse(c.opts.get("exposure")).unwrap_or(Exposure::NodePort)
    }

    /// The host people type: the Ingress host, the public host, or the node address.
    fn host(c: &BuildCtx) -> String {
        match Self::exposure(c) {
            Exposure::Ingress => c.opts.get("ingress_host").to_string(),
            Exposure::LoadBalancer => c.opts.get("public_host").to_string(),
            Exposure::NodePort if !c.opts.get("public_host").is_empty() => c.opts.get("public_host").to_string(),
            Exposure::NodePort => c.host.clone(),
            Exposure::ClusterIP => format!("gitea-http.{}.svc.cluster.local", c.namespace),
        }
    }

    /// `(root_url, ssh_host, ssh_port)`.
    fn addresses(c: &BuildCtx) -> (String, String, u16) {
        let host = Self::host(c);
        match Self::exposure(c) {
            Exposure::NodePort => (format!("http://{host}:{}/", c.http_port), host, c.ssh_port),
            Exposure::ClusterIP => (format!("http://{host}:{HTTP_PORT}/"), host, 22),
            Exposure::LoadBalancer => (format!("http://{host}:{HTTP_PORT}/"), host, 22),
            Exposure::Ingress => (format!("http://{host}/"), host, 22),
        }
    }
}

impl ServiceDef for Gitea {
    fn id(&self) -> &'static str {
        "gitea"
    }
    fn title(&self) -> String {
        "Gitea".into()
    }
    fn description(&self) -> String {
        "A light self-hosted git service (repositories, pull requests, issues, an API and optional Actions runners) in your cluster. MIT licence, one rootless container, SQLite on a PVC.".into()
    }
    fn kind(&self) -> &'static str {
        "git"
    }
    fn versions(&self) -> Vec<String> {
        VERSIONS.iter().map(|s| s.to_string()).collect()
    }
    fn images(&self, version: &str) -> Vec<ImageRef> {
        vec![ImageRef::new(&format!("docker.io/gitea/gitea:{version}-rootless"), &["amd64", "arm64"])]
    }
    fn options(&self) -> Vec<OptionDef> {
        vec![
            OptionDef::new("public_host", "Public host", "The host name or address people use (needed for LoadBalancer; for NodePort it replaces the node address). Gitea builds its clone URLs from it.", OptKind::Text, ""),
            OptionDef::new("http_node_port", "HTTP NodePort", "NodePort exposure only. Empty: the first free port from 30300.", OptKind::Port, ""),
            OptionDef::new("ssh_node_port", "SSH NodePort", "NodePort exposure only. Empty: the first free port from 30222.", OptKind::Port, ""),
        ]
    }
    fn check_options(&self, o: &Options) -> Result<(), String> {
        let host = o.get("public_host");
        if Exposure::parse(o.get("exposure"))? == Exposure::LoadBalancer && host.is_empty() {
            return Err("Public host: a LoadBalancer's address is not known before it is assigned; give the host name or IP people will use".into());
        }
        if !host.is_empty() && !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':' || c == '[' || c == ']') {
            return Err(format!("Public host: `{host}` is not a host name or address"));
        }
        Ok(())
    }
    fn build(&self, c: &BuildCtx) -> Result<Built, String> {
        let exposure = Self::exposure(c);
        let (root_url, ssh_host, ssh_port) = Self::addresses(c);
        let host = Self::host(c);
        let image = c.images.first().ok_or("no image")?;
        let admin = c.opts.get("admin_user");
        let secret = "gitea-admin";
        let mut m = String::new();
        // storage
        m.push_str(&format!("apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: gitea-data\n  labels:\n{}spec:\n  accessModes:\n  - ReadWriteOnce\n", labels(c, 4)));
        if let Some(sc) = &c.storage_class {
            m.push_str(&format!("  storageClassName: {sc}\n"));
        }
        m.push_str(&format!("  resources:\n    requests:\n      storage: {}\n---\n", c.opts.get("storage_size")));
        // deployment
        m.push_str(&format!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: gitea\n  labels:\n{}spec:\n  replicas: 1\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n{}  template:\n    metadata:\n      labels:\n{}    spec:\n      automountServiceAccountToken: false\n      securityContext:\n        runAsNonRoot: true\n        runAsUser: 1000\n        runAsGroup: 1000\n        fsGroup: 1000\n        seccompProfile:\n          type: RuntimeDefault\n      containers:\n      - name: gitea\n        image: {image}\n        imagePullPolicy: IfNotPresent\n        env:\n", labels(c, 4), selector(c, 6), labels(c, 8)));
        for (k, v) in [
            ("GITEA__server__DOMAIN", host.as_str()),
            ("GITEA__server__ROOT_URL", root_url.as_str()),
            ("GITEA__server__HTTP_PORT", &HTTP_PORT.to_string()),
            ("GITEA__server__SSH_DOMAIN", ssh_host.as_str()),
            ("GITEA__server__SSH_PORT", &ssh_port.to_string()),
            ("GITEA__server__SSH_LISTEN_PORT", &SSH_LISTEN.to_string()),
            ("GITEA__server__START_SSH_SERVER", "true"),
            ("GITEA__server__LFS_START_SERVER", "true"),
            ("GITEA__security__INSTALL_LOCK", "true"),
            ("GITEA__service__DISABLE_REGISTRATION", "true"),
            ("GITEA__service__REQUIRE_SIGNIN_VIEW", "true"),
            ("GITEA__repository__DEFAULT_PRIVATE", "private"),
            ("GITEA__database__DB_TYPE", "sqlite3"),
            ("GITEA__database__PATH", "/var/lib/gitea/data/gitea.db"),
            ("GITEA__webhook__ALLOWED_HOST_LIST", "external"),
            ("GITEA__log__MODE", "console"),
            ("GITEA_ADMIN_USER", admin),
            ("GITEA_ADMIN_EMAIL", &format!("{admin}@iyakki.local")),
        ] {
            m.push_str(&env(k, v));
        }
        m.push_str(&format!("        - name: GITEA_ADMIN_PASSWORD\n          valueFrom:\n            secretKeyRef:\n              name: {secret}\n              key: password\n"));
        m.push_str(&format!(
            "        ports:\n        - name: http\n          containerPort: {HTTP_PORT}\n        - name: ssh\n          containerPort: {SSH_LISTEN}\n        startupProbe:\n          httpGet:\n            path: /api/healthz\n            port: http\n          periodSeconds: 5\n          failureThreshold: 60\n        readinessProbe:\n          httpGet:\n            path: /api/healthz\n            port: http\n          periodSeconds: 10\n          failureThreshold: 3\n        livenessProbe:\n          httpGet:\n            path: /api/healthz\n            port: http\n          initialDelaySeconds: 60\n          periodSeconds: 20\n          failureThreshold: 6\n        resources:\n          requests:\n            cpu: 100m\n            memory: 256Mi\n          limits:\n            cpu: \"1\"\n            memory: 1Gi\n        securityContext:\n          allowPrivilegeEscalation: false\n          runAsNonRoot: true\n          capabilities:\n            drop:\n            - ALL\n        volumeMounts:\n        - name: data\n          mountPath: /var/lib/gitea\n          subPath: data\n        - name: data\n          mountPath: /etc/gitea\n          subPath: config\n        - name: tmp\n          mountPath: /tmp\n      volumes:\n      - name: data\n        persistentVolumeClaim:\n          claimName: gitea-data\n      - name: tmp\n        emptyDir: {{}}\n---\n"
        ));
        // services
        let svc_type = match exposure {
            Exposure::NodePort => "NodePort",
            Exposure::LoadBalancer => "LoadBalancer",
            _ => "ClusterIP",
        };
        let svc = |name: &str, port: u16, target: &str, node: u16| {
            let mut s = format!("apiVersion: v1\nkind: Service\nmetadata:\n  name: {name}\n  labels:\n{}spec:\n  type: {svc_type}\n  selector:\n{}  ports:\n  - name: {target}\n    port: {port}\n    targetPort: {target}\n", labels(c, 4), selector(c, 4));
            if exposure == Exposure::NodePort && node != 0 {
                s.push_str(&format!("    nodePort: {node}\n"));
            }
            s.push_str("---\n");
            s
        };
        m.push_str(&svc("gitea-http", HTTP_PORT, "http", c.http_port));
        m.push_str(&svc("gitea-ssh", 22, "ssh", c.ssh_port));
        if exposure == Exposure::Ingress {
            m.push_str(&format!("apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: gitea\n  labels:\n{}spec:\n  rules:\n  - host: {host}\n    http:\n      paths:\n      - path: /\n        pathType: Prefix\n        backend:\n          service:\n            name: gitea-http\n            port:\n              number: {HTTP_PORT}\n---\n", labels(c, 4)));
        }
        let manifest = m.trim_end_matches("---\n").to_string();
        let mut urls = vec![UrlInfo { label: "Gitea".into(), url: root_url.trim_end_matches('/').to_string(), kind: "web".into() }, UrlInfo { label: "API".into(), url: format!("{}api/v1", root_url), kind: "api".into() }];
        urls.push(UrlInfo { label: "Clone over HTTP".into(), url: format!("{root_url}<owner>/<repo>.git"), kind: "clone-http".into() });
        urls.push(UrlInfo { label: "Clone over SSH".into(), url: if ssh_port == 22 { format!("git@{ssh_host}:<owner>/<repo>.git") } else { format!("ssh://git@{ssh_host}:{ssh_port}/<owner>/<repo>.git") }, kind: "clone-ssh".into() });
        let mut notes = vec![format!("Gitea runs rootless (uid 1000) with SQLite on the PVC; registration is closed, sign-in is required, and the first administrator is {admin}. Postgres is a later option.")];
        if exposure == Exposure::ClusterIP {
            notes.push(format!("ClusterIP: reach it from your machine with kubectl port-forward svc/gitea-http {HTTP_PORT}:{HTTP_PORT} -n {}; the ROOT_URL then only works inside the cluster, so use NodePort or Ingress to connect a workspace.", c.namespace));
        }
        if exposure == Exposure::Ingress {
            notes.push("Ingress: the URL is http://; terminate TLS in your ingress controller and set the ROOT_URL accordingly if you use https. SSH stays a ClusterIP Service (an Ingress carries HTTP only).".into());
        }
        Ok(Built { manifest, secrets: vec![SecretSpec { name: secret.into(), entries: vec![SecretEntry { key: "password".into(), vault: format!("{}.admin-password", c.vault_prefix), label: format!("Gitea admin password for {admin}"), kind: SecretKind::Password, reveal: true }] }], urls, notes })
    }
    fn workloads(&self) -> Vec<Workload> {
        vec![Workload { name: "gitea".into(), container: "gitea".into(), image: 0 }]
    }
    fn health_path(&self) -> Option<String> {
        Some("/api/healthz".into())
    }
    fn how_to_use(&self, urls: &[UrlInfo], o: &Options) -> String {
        let web = urls.iter().find(|u| u.kind == "web").map(|u| u.url.as_str()).unwrap_or("");
        format!("Open {web} and sign in as {} (the password is in your vault as the entry shown, and in the one-time dialog). Registration is closed: create further users from the site administration page.\nTo put a workspace on it: IDE, K8S panel, Services, Connect workspace to Gitea. IYAKKI creates the repository (private by default) through the API with its own token, sets origin and pushes after you approve. Clone URLs are shown under the service's Details.", o.get("admin_user"))
    }
    fn backup_hint(&self) -> String {
        "Back up with `gitea dump` (repositories, database, LFS, configuration) into the volume, optionally copied to this machine; copy it somewhere off the cluster too. Restore is `gitea restore` or unzipping the dump into a fresh instance.".into()
    }
    fn backup(&self, _w: &Workload) -> Option<BackupPlan> {
        Some(BackupPlan { exec: vec!["sh".into(), "-c".into(), "mkdir -p /var/lib/gitea/backups && gitea dump -c /etc/gitea/app.ini -f \"$0\" --type zip --tempdir /tmp".into(), "{file}".into()], remote_file: "/var/lib/gitea/backups/{name}".into(), name: "gitea-dump-{stamp}.zip".into(), note: "gitea dump".into() })
    }
    fn exposes(&self, o: &Options) -> Vec<String> {
        let e = Exposure::parse(o.get("exposure")).unwrap_or(Exposure::NodePort);
        let mut v = vec![format!("{}: {}", e.word(), e.explain()), "Two Services: gitea-http (web, API, git over HTTP) and gitea-ssh (git over SSH)".into(), "Registration is closed and sign-in is required to view anything; webhooks may only call external (public) hosts".into()];
        if e == Exposure::NodePort {
            v.push("A NodePort is open on every node of the cluster: put a firewall or a NetworkPolicy in front if the network is not trusted".into());
        }
        v
    }
    fn detect_existing(&self, f: &ClusterFacts) -> Vec<Existing> {
        let mut out = Vec::new();
        for (ns, name, images) in &f.workloads {
            let img = images.iter().find(|i| {
                let l = i.to_lowercase();
                l.contains("gitea/gitea") || l.contains("forgejo/forgejo") || l.contains("/forgejo:") || l.contains("gitea:")
            });
            let by_name = name.starts_with("gitea") || name.starts_with("forgejo");
            if img.is_none() && !by_name {
                continue;
            }
            let image = img.cloned().unwrap_or_default();
            let product = if image.to_lowercase().contains("forgejo") || name.starts_with("forgejo") { "Forgejo" } else { "Gitea" };
            let hint = f.services.iter().filter(|s| &s.0 == ns && !s.3.is_empty()).find(|s| s.1.contains("http") || s.1.contains("web") || s.1 == *name).or_else(|| f.services.iter().find(|s| &s.0 == ns && !s.3.is_empty())).map(|s| format!("http://{}:{}", f.api_host, s.3[0])).unwrap_or_default();
            out.push(Existing { namespace: ns.clone(), name: name.clone(), image, url_hint: hint, product: product.into() });
        }
        out
    }
    fn post_deploy(&self, p: &mut PostCtx<'_>) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        let exec = |script: &str, arg0: &str| {
            let mut args: Vec<&str> = vec!["exec", "deploy/gitea", "-n", &p.namespace, "-c", &p.workload.container, "--", "sh", "-c", script];
            if !arg0.is_empty() {
                args.push(arg0);
            }
            p.kc.kubectl(&args, 30, 90)
        };
        // the first administrator; the password stays in the pod's environment (from the Secret), never in a command line here
        let o = exec("gitea admin user create --admin --username \"$GITEA_ADMIN_USER\" --password \"$GITEA_ADMIN_PASSWORD\" --email \"$GITEA_ADMIN_EMAIL\" --must-change-password=false", "");
        let said = o.combined().to_lowercase();
        if o.ok() {
            lines.push(format!("created the administrator {}", p.opts.get("admin_user")));
        } else if said.contains("already exist") || said.contains("already used") {
            lines.push(format!("the administrator {} already exists", p.opts.get("admin_user")));
        } else {
            return Err(format!("creating the administrator failed: {}", first_line(&o.combined())));
        }
        // an API token for IYAKKI, kept in the vault only
        let name = format!("{}.token", p.vault_prefix);
        if p.p.vault.has(&name) {
            lines.push(format!("kept the existing IYAKKI token (uvault:{name})"));
            p.added_vault.push(name);
            return Ok(lines);
        }
        let tname = format!("iyakki-{}", crate::clock::now_unix());
        let o = exec("gitea admin user generate-access-token --username \"$GITEA_ADMIN_USER\" --token-name \"$0\" --scopes \"write:repository,read:user\" --raw", &tname);
        if !o.ok() {
            return Err(format!("creating the API token failed: {}", first_line(&o.combined())));
        }
        let token = zeroize::Zeroizing::new(o.stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string());
        if token.len() < 20 || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("the token command answered, but not with a token".into());
        }
        p.p.vault.set(&name, &token)?;
        p.added_vault.push(name.clone());
        lines.push(format!("created an API token for IYAKKI (vault entry uvault:{name}); it is never shown"));
        Ok(lines)
    }
    fn network_policy_example(&self, ns: &str) -> Option<String> {
        Some(format!("apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: gitea-ingress\n  namespace: {ns}\nspec:\n  podSelector:\n    matchLabels:\n      app.kubernetes.io/name: gitea\n  policyTypes:\n  - Ingress\n  ingress:\n  - from:\n    - ipBlock:\n        cidr: 192.168.0.0/16   # your LAN, change it\n    ports:\n    - port: 3000\n    - port: 2222\n"))
    }
}

// ------------------------------------------------------------------------------------------------ connect to an existing one

/// Connect a Gitea or Forgejo that already runs (in this cluster or anywhere): the URL and a token the person typed. IYAKKI deploys and
/// removes nothing. The token goes to the vault only; the URL answers `/api/v1/version` and the token answers `/api/v1/user` first.
pub fn connect_existing(pl: &Platform, url: &str, token: &str, cluster: Option<&ClusterRef>, namespace: &str, approved: bool) -> Result<Installed, String> {
    if !approved {
        return Err("connecting was not approved".into());
    }
    let url = normalize_url(url)?;
    if token.trim().len() < 20 || token.chars().any(char::is_whitespace) {
        return Err("the token does not look like an access token (create one in the service under Settings, Applications, with the repository write scope)".into());
    }
    let v = pl.http.request("GET", &format!("{url}/api/v1/version"), None, None).map_err(|e| format!("{url} did not answer: {e}"))?;
    if v.status != 200 {
        return Err(format!("{url} answered {} to /api/v1/version: is this a Gitea or Forgejo address?", v.status));
    }
    let version = serde_json::from_str::<serde_json::Value>(&v.body).ok().and_then(|j| j["version"].as_str().map(str::to_string)).ok_or("the answer of /api/v1/version is not a Gitea or Forgejo one")?;
    let u = pl.http.request("GET", &format!("{url}/api/v1/user"), Some(token.trim()), None).map_err(|e| format!("could not check the token: {e}"))?;
    if u.status != 200 {
        return Err(format!("the service refused the token (HTTP {}); create a new one with the repository write scope", u.status));
    }
    let login = serde_json::from_str::<serde_json::Value>(&u.body).ok().and_then(|j| j["login"].as_str().map(str::to_string)).unwrap_or_default();
    let existing = installed::load(&pl.dir, &pl.user);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let id = format!("gitea-existing-{:06x}", h & 0xff_ffff);
    let _ = existing;
    let vault = format!("svc.{id}.token");
    pl.vault.set(&vault, token.trim())?;
    let now = crate::clock::now_unix();
    let rec = Installed {
        id: id.clone(),
        service: "gitea".into(),
        title: "Gitea (existing)".into(),
        version,
        kind: "git".into(),
        cluster: cluster.cloned(),
        namespace: namespace.into(),
        created_namespace: false,
        connected: true,
        options: Options::default(),
        urls: vec![UrlInfo { label: "Gitea".into(), url: url.clone(), kind: "web".into() }, UrlInfo { label: "API".into(), url: format!("{url}/api/v1"), kind: "api".into() }, UrlInfo { label: "Clone over HTTP".into(), url: format!("{url}/<owner>/<repo>.git"), kind: "clone-http".into() }],
        vault: vec![vault],
        installed_at: now,
        updated_at: now,
        workspaces: vec![],
        note: format!("connected as {login}"),
    };
    installed::upsert(&pl.dir, &pl.user, rec.clone())?;
    pl.audit("connect", &format!("gitea:{url} as {login} ok"));
    Ok(rec)
}

/// `http(s)://host[:port]` without a path, credentials or query.
pub fn normalize_url(url: &str) -> Result<String, String> {
    let u = url.trim().trim_end_matches('/');
    let (scheme, rest) = u.split_once("://").ok_or("the address must start with http:// or https://")?;
    if scheme != "http" && scheme != "https" {
        return Err("the address must start with http:// or https://".into());
    }
    if rest.is_empty() || rest.contains(['@', '?', '#', ' ']) || rest.contains('/') && rest.split_once('/').is_some_and(|x| !x.1.is_empty()) {
        return Err("give the address without a path, user name or query (such as http://192.168.1.10:3000)".into());
    }
    let host = rest.trim_end_matches('/');
    if !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']')) {
        return Err(format!("`{host}` is not a host name or address"));
    }
    Ok(format!("{scheme}://{host}"))
}
