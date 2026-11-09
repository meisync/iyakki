//! The manifest linter that runs before anything is applied to a cluster (docs/K3S-DEV.md, "Safety").
//!
//! It reads block-style YAML (and JSON) line by line, without a YAML library, and
//! * **rejects**: privileged containers, `hostPath`, `hostNetwork`/`hostPID`/`hostIPC`, `allowPrivilegeEscalation: true`, added
//!   capabilities, `runAsUser: 0`, cluster-scoped kinds (Namespace, ClusterRole, CustomResourceDefinition …), Secrets with
//!   literal values, any namespace other than the workspace's, and YAML it cannot read reliably (anchors, aliases, tabs, flow
//!   style spread over lines);
//! * **rewrites**: `imagePullPolicy: IfNotPresent` for images that were loaded into the local cluster (the default policy for a
//!   `:latest` or untagged image is `Always`, which tries to pull an image that only exists on this node and ends in
//!   `ImagePullBackOff`), saying so;
//! * **warns**: `:latest` images that will be pulled, images from registries it does not know, missing resource limits, a
//!   container that may run as root.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lint {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// What was changed for the person (the pull policy), shown with the approval.
    pub notes: Vec<String>,
    /// The manifest to apply: the input with the rewrites.
    pub text: String,
    /// (kind, name) of every object.
    pub objects: Vec<(String, String)>,
    pub images: Vec<String>,
}

impl Lint {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn report(&self) -> String {
        let mut t = String::new();
        for e in &self.errors {
            t.push_str(&format!("ERROR: {e}\n"));
        }
        for w in &self.warnings {
            t.push_str(&format!("warning: {w}\n"));
        }
        for n in &self.notes {
            t.push_str(&format!("changed: {n}\n"));
        }
        t.trim_end().to_string()
    }
}

pub const CLUSTER_SCOPED: &[&str] = &[
    "Namespace", "ClusterRole", "ClusterRoleBinding", "CustomResourceDefinition", "PersistentVolume", "StorageClass", "Node", "PriorityClass", "MutatingWebhookConfiguration", "ValidatingWebhookConfiguration", "APIService", "PodSecurityPolicy", "IngressClass", "RuntimeClass", "CSIDriver", "CSINode", "VolumeAttachment", "ValidatingAdmissionPolicy", "ValidatingAdmissionPolicyBinding", "CertificateSigningRequest", "ComponentStatus", "PodSecurityPolicyTemplate",
];

const WORKLOADS: &[&str] = &["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob", "Pod", "ReplicaSet"];
const KNOWN_REGISTRIES: &[&str] = &["docker.io", "ghcr.io", "quay.io", "registry.k8s.io", "gcr.io", "k8s.gcr.io", "mcr.microsoft.com", "public.ecr.aws", "registry.gitlab.com", "localhost"];

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

/// Drop a trailing comment (a `#` at the start or after a space, outside quotes).
fn strip_comment(l: &str) -> &str {
    let (mut q, mut prev) = (None::<char>, ' ');
    for (i, c) in l.char_indices() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (None, '"' | '\'') => q = Some(c),
            (None, '#') if prev == ' ' || prev == '\t' || i == 0 => return &l[..i],
            _ => {}
        }
        prev = c;
    }
    l
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) { &s[1..s.len() - 1] } else { s }
}

/// A line split into (content column, key, value) when it is `key: value` or `- key: value`.
fn key_value(l: &str) -> Option<(usize, String, String)> {
    let body = strip_comment(l);
    let ind = indent_of(body);
    let mut t = &body[ind..];
    let mut col = ind;
    while let Some(r) = t.strip_prefix("- ") {
        let skipped = t.len() - r.len();
        col += skipped + (r.len() - r.trim_start().len());
        t = r.trim_start();
    }
    let (k, v) = t.split_once(':')?;
    // a colon inside a value or a URL is not a key separator: the key has no spaces
    let k = unquote(k.trim());
    if k.is_empty() || k.contains(' ') {
        return None;
    }
    if !(v.is_empty() || v.starts_with(' ')) {
        return None;
    }
    Some((col, k.to_string(), unquote(v.trim()).to_string()))
}

fn is_dash(l: &str) -> bool {
    strip_comment(l).trim_start().starts_with("- ")
}

fn content_col(l: &str) -> usize {
    key_value(l).map_or_else(|| indent_of(l), |x| x.0)
}

fn registry_of(image: &str) -> String {
    let first = image.split('/').next().unwrap_or("");
    if image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost") { first.split(':').next().unwrap_or(first).to_string() } else { "docker.io".to_string() }
}

fn tag_of(image: &str) -> Option<&str> {
    let last = image.rsplit('/').next().unwrap_or(image);
    if image.contains('@') {
        return Some("digest");
    }
    last.split_once(':').map(|x| x.1)
}

/// Check one manifest (one or more `---` separated documents) for `namespace`. `is_local(image)` says whether an image was
/// loaded into the local cluster (so it must not be pulled).
pub fn lint(text: &str, namespace: &str, is_local: &dyn Fn(&str) -> bool) -> Lint {
    let mut out = Lint::default();
    if text.contains('\t') {
        out.errors.push("the manifest contains tab characters; YAML needs spaces".into());
    }
    let docs = split_docs(text);
    let mut rewritten: Vec<String> = Vec::new();
    for (n, doc) in docs.iter().enumerate() {
        let (new_doc, l) = lint_doc(doc, namespace, is_local, n + 1);
        out.errors.extend(l.errors);
        out.warnings.extend(l.warnings);
        out.notes.extend(l.notes);
        out.objects.extend(l.objects);
        out.images.extend(l.images);
        rewritten.push(new_doc);
    }
    if out.objects.is_empty() && out.errors.is_empty() {
        out.errors.push("the manifest has no objects (each needs `apiVersion`, `kind` and `metadata.name`)".into());
    }
    out.text = rewritten.join("---\n");
    out
}

fn split_docs(text: &str) -> Vec<String> {
    let mut docs = vec![String::new()];
    for l in text.lines() {
        if l.trim_end() == "---" || l.starts_with("--- ") {
            docs.push(String::new());
        } else {
            let d = docs.last_mut().expect("never empty");
            d.push_str(l);
            d.push('\n');
        }
    }
    docs.into_iter().filter(|d| d.lines().any(|l| !strip_comment(l).trim().is_empty())).collect()
}

fn lint_doc(doc: &str, namespace: &str, is_local: &dyn Fn(&str) -> bool, nth: usize) -> (String, Lint) {
    let mut l = Lint::default();
    let lines: Vec<String> = doc.lines().map(str::to_string).collect();
    let (mut kind, mut name) = (String::new(), String::new());
    let (mut in_meta, mut caps_indent, mut skip_block): (bool, Option<usize>, Option<usize>) = (false, None, None);
    let (mut has_limits, mut has_nonroot, mut has_container) = (false, false, false);
    let mut image_lines: Vec<(usize, String)> = Vec::new();
    let mut literal_secret = false;
    let mut depth: i32 = 0;
    let mut unbalanced = false;
    let _ = nth;
    if doc.trim_start().starts_with('{') {
        l.errors.push("write the manifest as block-style YAML; JSON manifests are not checked".into());
    }
    for (i, raw) in lines.iter().enumerate() {
        let body = strip_comment(raw);
        if body.trim().is_empty() {
            continue;
        }
        let ind = indent_of(body);
        // the text of a block scalar (`|`, `>`) is data, not manifest keys
        if let Some(b) = skip_block {
            if ind > b {
                continue;
            }
            skip_block = None;
        }
        if body.contains(" &") && body.split_whitespace().any(|w| w.starts_with('&') && w.len() > 1) || body.split_whitespace().any(|w| (w.starts_with('*') && w.len() > 1) || w == "<<:") {
            l.errors.push(format!("line {}: YAML anchors, aliases and merge keys are not supported by the checker; write the values out", i + 1));
        }
        for c in body.chars() {
            match c {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        if depth != 0 {
            unbalanced = true;
            depth = 0;
        }
        // flow mappings on one line: look for the dangerous settings in the text itself
        if body.contains('{') {
            let flat: String = body.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect();
            for (pat, msg) in [("privileged:true", "privileged containers are not allowed"), ("hostNetwork:true", "hostNetwork: true is not allowed"), ("hostPID:true", "hostPID: true is not allowed"), ("hostIPC:true", "hostIPC: true is not allowed"), ("allowPrivilegeEscalation:true", "allowPrivilegeEscalation: true is not allowed"), ("runAsUser:0,", "runAsUser: 0 runs as root"), ("runAsUser:0}", "runAsUser: 0 runs as root"), ("hostPath:", "hostPath volumes are not allowed"), ("add:[\u{0}", "")] {
                if !msg.is_empty() && flat.contains(pat) {
                    l.errors.push(format!("line {}: {msg}", i + 1));
                }
            }
            if flat.contains("add:[") && !flat.contains("add:[]") {
                l.errors.push(format!("line {}: adding Linux capabilities is not allowed", i + 1));
            }
        }
        let Some((col, k, v)) = key_value(raw) else { continue };
        let v = v.trim_end_matches(',').to_string();
        if ind == 0 && !is_dash(raw) {
            in_meta = k == "metadata";
        }
        if matches!(v.as_str(), "|" | ">" | "|-" | ">-" | "|+" | ">+") {
            skip_block = Some(ind.max(col.saturating_sub(if is_dash(raw) { 2 } else { 0 })));
        }
        if let Some(ci) = caps_indent {
            if ind <= ci && k != "capabilities" {
                caps_indent = None;
            }
        }
        match k.as_str() {
            "kind" if ind == 0 => kind = v.clone(),
            "name" if in_meta && col == 2 => name = v.clone(),
            "namespace" if in_meta && col == 2 => {
                if v != namespace {
                    l.errors.push(format!("line {}: namespace \"{v}\" is not this workspace's namespace \"{namespace}\"; remove it from the manifest (the tools apply to \"{namespace}\" only)", i + 1));
                }
            }
            "privileged" if v.eq_ignore_ascii_case("true") => l.errors.push(format!("line {}: privileged containers are not allowed", i + 1)),
            "allowPrivilegeEscalation" if v.eq_ignore_ascii_case("true") => l.errors.push(format!("line {}: allowPrivilegeEscalation: true is not allowed", i + 1)),
            "hostNetwork" | "hostPID" | "hostIPC" if v.eq_ignore_ascii_case("true") => l.errors.push(format!("line {}: {k}: true is not allowed (it shares the node's {})", i + 1, &k[4..])),
            "hostPath" => l.errors.push(format!("line {}: hostPath volumes are not allowed (they mount the node's files); use a PersistentVolumeClaim or emptyDir", i + 1)),
            "hostPort" => l.warnings.push(format!("line {}: hostPort binds a port on the node; use a Service instead", i + 1)),
            "runAsUser" if v == "0" => l.errors.push(format!("line {}: runAsUser: 0 runs as root", i + 1)),
            "runAsNonRoot" => {
                if v.eq_ignore_ascii_case("true") {
                    has_nonroot = true;
                } else {
                    l.warnings.push(format!("line {}: runAsNonRoot is not true", i + 1));
                }
            }
            "capabilities" => caps_indent = Some(ind),
            "add" if caps_indent.is_some() => {
                let nonempty = !(v == "[]" || v.is_empty() && !lines.get(i + 1).is_some_and(|n| strip_comment(n).trim_start().starts_with('-')));
                if nonempty {
                    l.errors.push(format!("line {}: adding Linux capabilities is not allowed", i + 1));
                }
            }
            "limits" => has_limits = true,
            "image" => {
                has_container = true;
                if !v.is_empty() {
                    image_lines.push((i, v.clone()));
                }
            }
            "stringData" | "data" if kind == "Secret" && (ind == 0) => literal_secret = true,
            _ => {}
        }
    }
    if unbalanced {
        l.errors.push("brackets or braces are not closed on the same line they open; the checker only reads block-style YAML or JSON that is balanced".into());
    }
    if kind.is_empty() {
        l.errors.push("a document has no `kind`".into());
    } else {
        if CLUSTER_SCOPED.contains(&kind.as_str()) {
            l.errors.push(format!("{kind} is cluster-scoped; the tools only create things inside the workspace namespace \"{namespace}\" (the namespace itself is created for you on the first apply: never write a Namespace for it). Fix: remove this {kind} from the manifest and apply the rest; if it is really needed, tell the person to apply it by hand as a cluster admin step (kubectl apply -f <this file>). Do not ask for permissions: carry on with the namespaced objects"));
        }
        if kind == "Secret" && literal_secret {
            l.errors.push("a Secret with its values written in the manifest would pass through the model and the log; create it from your vault instead".into());
        }
        l.objects.push((kind.clone(), name.clone()));
    }
    // images: pull policy and tags
    let mut new_lines = lines.clone();
    let mut edits: Vec<(usize, String, bool)> = Vec::new();
    for (i, image) in &image_lines {
        l.images.push(image.clone());
        let local = is_local(image);
        let (has_policy, policy_line) = pull_policy_of(&lines, *i);
        let tag = tag_of(image);
        let latest = tag.is_none() || tag == Some("latest");
        if local {
            match &policy_line {
                Some((j, val)) if val == "IfNotPresent" || val == "Never" => {
                    let _ = j;
                }
                Some((j, val)) => {
                    edits.push((*j, image.clone(), true));
                    l.notes.push(format!("{image}: imagePullPolicy {val} changed to IfNotPresent. This image exists only on this machine's cluster node; the default for an untagged or :latest image is Always, which tries to pull it from a registry and ends in ImagePullBackOff."));
                }
                None => {
                    edits.push((*i, image.clone(), false));
                    l.notes.push(format!("{image}: added imagePullPolicy: IfNotPresent. This image was loaded into the local cluster and is not in any registry; without the policy an untagged or :latest image is pulled (policy Always) and fails with ImagePullBackOff."));
                }
            }
        } else {
            if latest && !has_policy {
                l.warnings.push(format!("{image}: no tag or :latest with no imagePullPolicy; pods may run an older copy of the image. Use a specific tag (the deploy flow tags builds with a content hash)."));
            }
            let reg = registry_of(image);
            if !KNOWN_REGISTRIES.contains(&reg.as_str()) {
                l.warnings.push(format!("{image}: the registry {reg} is not one the checker knows; make sure the cluster can pull from it"));
            }
        }
    }
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    for (idx, _image, replace) in edits {
        if replace {
            let line = &new_lines[idx];
            if let Some(p) = line.find("imagePullPolicy:") {
                new_lines[idx] = format!("{}imagePullPolicy: IfNotPresent", &line[..p]);
            }
        } else {
            let col = content_col(&new_lines[idx]);
            new_lines.insert(idx + 1, format!("{}imagePullPolicy: IfNotPresent", " ".repeat(col)));
        }
    }
    if WORKLOADS.contains(&kind.as_str()) && has_container {
        if !has_limits {
            l.warnings.push(format!("{kind} {name}: no resource limits. Add, per container, `resources: {{requests: {{cpu: 50m, memory: 64Mi}}, limits: {{cpu: 500m, memory: 256Mi}}}}` so one app cannot starve the node."));
        }
        if !has_nonroot {
            l.warnings.push(format!("{kind} {name}: no `securityContext: {{runAsNonRoot: true}}`; the container may run as root."));
        }
    }
    let mut text = new_lines.join("\n");
    text.push('\n');
    (text, l)
}

/// The `imagePullPolicy` of the container that owns the `image:` line at `i`: whether there is one, and its line and value.
fn pull_policy_of(lines: &[String], i: usize) -> (bool, Option<(usize, String)>) {
    let c = content_col(&lines[i]);
    let mut found: Option<(usize, String)> = None;
    // backwards up to the start of this list item
    let mut j = i;
    while j > 0 {
        j -= 1;
        if strip_comment(&lines[j]).trim().is_empty() {
            continue;
        }
        let cc = content_col(&lines[j]);
        if cc < c {
            break;
        }
        if cc == c {
            if let Some((_, k, v)) = key_value(&lines[j]) {
                if k == "imagePullPolicy" {
                    found = Some((j, v));
                }
            }
            if is_dash(&lines[j]) {
                break;
            }
        }
    }
    // forwards up to the next item or key of a parent
    for (j, line) in lines.iter().enumerate().skip(i + 1) {
        if strip_comment(line).trim().is_empty() {
            continue;
        }
        if indent_of(line) < c {
            break;
        }
        if content_col(line) == c && !is_dash(line) || (content_col(line) == c && indent_of(line) == c) {
            if let Some((_, k, v)) = key_value(line) {
                if k == "imagePullPolicy" {
                    found = Some((j, v));
                }
            }
        }
    }
    (found.is_some(), found)
}

/// Every image named in a manifest, for the caller to compare with what was built.
pub fn images_of(text: &str) -> BTreeSet<String> {
    lint(text, "x", &|_| false).images.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  labels:
    app: web
spec:
  replicas: 1
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
      - name: web
        image: localhost/web:abc123
        ports:
        - containerPort: 8080
        resources:
          limits:
            cpu: 500m
            memory: 256Mi
        securityContext:
          runAsNonRoot: true
---
apiVersion: v1
kind: Service
metadata:
  name: web
spec:
  selector:
    app: web
  ports:
  - port: 80
    targetPort: 8080
";

    fn local(i: &str) -> bool {
        i.starts_with("localhost/") || i == "myapp:latest"
    }

    fn one(text: &str) -> Lint {
        lint(text, "shop-dev", &local)
    }

    #[test]
    fn a_clean_manifest_passes_gets_the_pull_policy_and_lists_its_objects() {
        let l = one(GOOD);
        assert!(l.ok(), "{}", l.report());
        assert_eq!(l.objects, vec![("Deployment".into(), "web".into()), ("Service".into(), "web".into())]);
        assert_eq!(l.images, vec!["localhost/web:abc123".to_string()]);
        assert!(l.text.contains("        image: localhost/web:abc123\n        imagePullPolicy: IfNotPresent\n"), "{}", l.text);
        assert!(l.notes.iter().any(|n| n.contains("ImagePullBackOff")), "the change is explained: {:?}", l.notes);
        // linting the result changes nothing more
        let again = one(&l.text);
        assert!(again.ok() && again.notes.is_empty() && again.text == l.text, "{}", again.report());
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    }

    #[test]
    fn dangerous_settings_are_rejected_one_by_one() {
        let cases: &[(&str, &str)] = &[
            ("        securityContext:\n          privileged: true\n", "privileged"),
            ("      hostNetwork: true\n", "hostNetwork"),
            ("      hostPID: true\n", "hostPID"),
            ("      hostIPC: true\n", "hostIPC"),
            ("      volumes:\n      - name: d\n        hostPath:\n          path: /\n", "hostPath"),
            ("        securityContext:\n          capabilities:\n            add:\n            - NET_ADMIN\n", "capabilities"),
            ("        securityContext:\n          capabilities:\n            add: [\"SYS_ADMIN\"]\n", "capabilities"),
            ("        securityContext:\n          allowPrivilegeEscalation: true\n", "allowPrivilegeEscalation"),
            ("        securityContext:\n          runAsUser: 0\n", "runAsUser"),
            ("        securityContext:\n          {privileged: true}\n", "privileged"),
        ];
        for (extra, what) in cases {
            // put the extra lines at the end of the container / pod spec
            let text = GOOD.replacen("        ports:\n", &format!("{}        ports:\n", if extra.starts_with("      host") || extra.starts_with("      volumes") { "" } else { extra }), 1);
            let text = if extra.starts_with("      host") || extra.starts_with("      volumes") { text.replacen("      containers:\n", &format!("{extra}      containers:\n"), 1) } else { text };
            let l = one(&text);
            assert!(l.errors.iter().any(|e| e.contains(what)), "{what}: {}", l.report());
        }
        // an empty add list and drop are fine
        let ok = GOOD.replacen("        ports:\n", "        securityContext:\n          capabilities:\n            drop: [ALL]\n            add: []\n        ports:\n", 1);
        assert!(one(&ok).errors.is_empty(), "{}", one(&ok).report());
    }

    #[test]
    fn cluster_scoped_kinds_secrets_and_other_namespaces_are_rejected() {
        for kind in ["Namespace", "ClusterRole", "ClusterRoleBinding", "CustomResourceDefinition", "PersistentVolume", "StorageClass", "MutatingWebhookConfiguration"] {
            let l = one(&format!("apiVersion: v1\nkind: {kind}\nmetadata:\n  name: x\n"));
            assert!(l.errors.iter().any(|e| e.contains("cluster-scoped")), "{kind}: {}", l.report());
        }
        let l = one("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\n  namespace: kube-system\ndata:\n  a: b\n");
        assert!(l.errors.iter().any(|e| e.contains("kube-system") && e.contains("shop-dev")), "{}", l.report());
        let l = one("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\n  namespace: shop-dev\ndata:\n  a: b\n");
        assert!(l.ok(), "the workspace's own namespace may be written out: {}", l.report());
        let l = one("apiVersion: v1\nkind: Secret\nmetadata:\n  name: s\nstringData:\n  password: hunter2\n");
        assert!(l.errors.iter().any(|e| e.contains("vault")), "{}", l.report());
        // a ConfigMap whose text mentions the dangerous words is data, not settings
        let l = one("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\ndata:\n  script.sh: |\n    privileged: true\n    hostPath: /\n  other: ok\n");
        assert!(l.ok(), "{}", l.report());
    }

    #[test]
    fn unreadable_yaml_is_refused_rather_than_guessed() {
        assert!(!one("kind: Pod\nmetadata:\n\tname: x\n").ok());
        assert!(one("kind: Pod\nmetadata:\n  name: x\nspec:\n  containers: [{name: a,\n  image: b}]\n").errors.iter().any(|e| e.contains("brackets")));
        assert!(one("base: &b\n  privileged: true\nkind: Pod\nmetadata:\n  name: x\nspec:\n  x: *b\n").errors.iter().any(|e| e.contains("anchors")));
        assert!(one("").errors.iter().any(|e| e.contains("no objects")));
        assert!(one("# only a comment\n").errors.iter().any(|e| e.contains("no objects")));
    }

    #[test]
    fn local_images_get_if_not_present_and_an_always_policy_is_changed_with_a_reason() {
        // untagged / :latest local image with no policy
        let m = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: a\nspec:\n  template:\n    spec:\n      containers:\n      - image: myapp:latest\n        name: a\n      - name: side\n        image: nginx:1.27\n        imagePullPolicy: Always\n";
        let l = one(m);
        assert!(l.ok(), "{}", l.report());
        assert!(l.text.contains("      - image: myapp:latest\n        imagePullPolicy: IfNotPresent\n        name: a\n"), "{}", l.text);
        assert!(l.text.contains("        imagePullPolicy: Always\n"), "the remote image keeps its policy:\n{}", l.text);
        // an Always policy on a local image is replaced
        let m2 = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: p\nspec:\n  containers:\n  - name: a\n    image: localhost/web:1\n    imagePullPolicy: Always\n";
        let l = one(m2);
        assert!(l.text.contains("imagePullPolicy: IfNotPresent") && !l.text.contains("Always"), "{}", l.text);
        assert!(l.notes[0].contains("Always") && l.notes[0].contains("IfNotPresent"));
        // Never is respected
        let m3 = m2.replace("Always", "Never");
        let l = one(&m3);
        assert!(l.text == m3 || l.text.trim_end() == m3.trim_end(), "{}", l.text);
        // each container gets its own
        let m4 = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: p\nspec:\n  containers:\n  - name: a\n    image: localhost/a:1\n  - name: b\n    image: localhost/b:1\n    imagePullPolicy: IfNotPresent\n";
        let l = one(m4);
        assert_eq!(l.text.matches("imagePullPolicy").count(), 2, "{}", l.text);
        assert_eq!(l.notes.len(), 1);
    }

    #[test]
    fn warnings_cover_latest_unknown_registries_limits_and_root() {
        let m = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: a\nspec:\n  template:\n    spec:\n      containers:\n      - name: a\n        image: nginx\n      - name: b\n        image: reg.example.org/team/app:1.2\n";
        let l = one(m);
        assert!(l.ok());
        let w = l.warnings.join("\n");
        assert!(w.contains("nginx") && w.contains(":latest"), "{w}");
        assert!(w.contains("reg.example.org"), "{w}");
        assert!(w.contains("no resource limits") && w.contains("limits: {cpu: 500m"), "the suggestion is concrete: {w}");
        assert!(w.contains("runAsNonRoot"), "{w}");
        // a pinned digest or tag from a known registry is quiet about tags
        let q = one(&m.replace("image: nginx\n", "image: ghcr.io/x/y:1.0\n").replace("reg.example.org/team/app:1.2", "quay.io/a/b:2"));
        assert!(!q.warnings.iter().any(|w| w.contains("latest")), "{:?}", q.warnings);
    }
}
