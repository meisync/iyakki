//! Re-running an install over a volume that exists already.
//!
//! A PersistentVolumeClaim is immutable once created (only its requested size may grow, and only when the StorageClass allows it).
//! The cluster fills in the default class at creation, so a manifest that does not name a class would, on the second `kubectl apply`,
//! try to clear it and be refused. Every installer therefore passes its manifest through [`guard`] before applying it:
//!
//! * a claim that does not exist yet is created with the StorageClass written out (the one the manifest names, else the cluster's
//!   default, else `local-path`);
//! * a claim that exists is left out of the manifest, so the apply never touches it; a bigger size in the manifest is applied as a
//!   resize when the class allows expanding, otherwise it is refused with a plain line and the volume is kept as it is.
//!
//! Nothing here reads a Secret or a value; the lines it returns go into the job log.

/// What a `kubectl` call answered.
#[derive(Clone, Debug, Default)]
pub struct KOut {
    pub ok: bool,
    pub stdout: String,
    /// Standard output and error together (for messages).
    pub text: String,
}

/// Runs `kubectl` with these arguments (a fake in tests).
pub type Kube<'a> = &'a dyn Fn(&[&str]) -> KOut;

/// The manifest to apply and what was decided about the volumes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guarded {
    /// The manifest without the claims that exist already (empty when nothing is left to apply).
    pub manifest: String,
    /// Plain lines for the job log.
    pub notes: Vec<String>,
}

fn split_docs(manifest: &str) -> Vec<String> {
    let mut docs: Vec<String> = vec![String::new()];
    for line in manifest.lines() {
        if line.trim_end() == "---" {
            docs.push(String::new());
        } else {
            let d = docs.last_mut().unwrap_or_else(|| unreachable!());
            d.push_str(line);
            d.push('\n');
        }
    }
    docs.into_iter().filter(|d| !d.trim().is_empty()).collect()
}

fn kind_of(doc: &str) -> &str {
    doc.lines().find_map(|l| l.strip_prefix("kind:")).map(str::trim).unwrap_or("")
}

/// A scalar under `metadata:` (two-space indent).
fn meta(doc: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for l in doc.lines() {
        if l == "metadata:" {
            inside = true;
        } else if inside {
            if !l.starts_with(' ') && !l.trim().is_empty() {
                return None;
            }
            if let Some(v) = l.strip_prefix(&format!("  {key}:")) {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

fn wanted_size(doc: &str) -> Option<String> {
    doc.lines().find_map(|l| l.trim().strip_prefix("storage:")).map(|v| v.trim().trim_matches('"').to_string())
}

fn has_class(doc: &str) -> bool {
    doc.lines().any(|l| l.trim_start().starts_with("storageClassName:"))
}

/// `5Gi` -> bytes (binary and decimal suffixes, plain numbers).
pub fn quantity(q: &str) -> Option<f64> {
    let q = q.trim();
    let split = q.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(q.len());
    let n: f64 = q[..split].parse().ok()?;
    let mul = match &q[split..] {
        "" => 1.0,
        "Ki" => 1024.0,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        "K" | "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        _ => return None,
    };
    Some(n * mul)
}

fn first_line(t: &str) -> String {
    t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// The StorageClass a new claim gets when the manifest names none: the cluster's default, else `local-path`.
fn default_class(kube: Kube) -> Option<String> {
    let o = kube(&["get", "storageclass", "-o", "json"]);
    if !o.ok {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&o.stdout).ok()?;
    let items = v["items"].as_array()?;
    let name = |i: &serde_json::Value| i["metadata"]["name"].as_str().map(str::to_string);
    let is_default = |i: &serde_json::Value| ["storageclass.kubernetes.io/is-default-class", "storageclass.beta.kubernetes.io/is-default-class"].iter().any(|k| i["metadata"]["annotations"][*k].as_str() == Some("true"));
    items.iter().find(|i| is_default(i)).and_then(name).or_else(|| items.iter().filter_map(name).find(|n| n == "local-path"))
}

fn with_class(doc: &str, class: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for l in doc.lines() {
        out.push_str(l);
        out.push('\n');
        if !done && l == "spec:" {
            out.push_str(&format!("  storageClassName: {class}\n"));
            done = true;
        }
    }
    out
}

fn args<'a>(base: &[&'a str], ns: &'a Option<String>) -> Vec<&'a str> {
    let mut v: Vec<&str> = base.to_vec();
    if let Some(n) = ns {
        v.push("-n");
        v.push(n);
    }
    v
}

/// Decide about every claim of `manifest` (see the module's words). `default_ns` is the namespace the apply will use when an object
/// names none.
pub fn guard(manifest: &str, default_ns: Option<&str>, kube: Kube) -> Result<Guarded, String> {
    let mut g = Guarded::default();
    let mut keep: Vec<String> = Vec::new();
    let mut class: Option<Option<String>> = None;
    for doc in split_docs(manifest) {
        if kind_of(&doc) != "PersistentVolumeClaim" {
            keep.push(doc);
            continue;
        }
        let name = meta(&doc, "name").ok_or("a PersistentVolumeClaim in the manifest has no name")?;
        let ns = meta(&doc, "namespace").or_else(|| default_ns.map(str::to_string));
        let got = kube(&args(&["get", "pvc", &name, "-o", "json"], &ns));
        if !got.ok {
            let t = got.text.to_lowercase();
            if !(t.contains("notfound") || t.contains("not found")) {
                return Err(format!("could not check the volume {name}: {}", first_line(&got.text)));
            }
            // new: write the class out so a later apply never clears it
            let doc = if has_class(&doc) {
                doc
            } else {
                let c = class.get_or_insert_with(|| default_class(kube)).clone();
                match c {
                    Some(c) => {
                        g.notes.push(format!("the volume {name} is new: it is created on the StorageClass {c}"));
                        with_class(&doc, &c)
                    }
                    None => {
                        g.notes.push(format!("the volume {name} is new (the cluster names no default StorageClass, so it uses the cluster's own choice)"));
                        doc
                    }
                }
            };
            keep.push(doc);
            continue;
        }
        // exists: never part of the apply
        let v: serde_json::Value = serde_json::from_str(&got.stdout).unwrap_or_default();
        let have = v["spec"]["resources"]["requests"]["storage"].as_str().unwrap_or("").to_string();
        let have_class = v["spec"]["storageClassName"].as_str().unwrap_or("").to_string();
        let kept = format!("the volume {name} exists already ({}{}): kept as it is, your data stays", if have.is_empty() { "size unknown" } else { &have }, if have_class.is_empty() { String::new() } else { format!(", StorageClass {have_class}") });
        match (wanted_size(&doc).and_then(|w| quantity(&w).map(|q| (w, q))), quantity(&have)) {
            (Some((w, wq)), Some(hq)) if wq > hq => {
                let sc = kube(&["get", "storageclass", &have_class, "-o", "json"]);
                let can = sc.ok && serde_json::from_str::<serde_json::Value>(&sc.stdout).is_ok_and(|s| s["allowVolumeExpansion"].as_bool() == Some(true));
                if can {
                    let patch = format!("{{\"spec\":{{\"resources\":{{\"requests\":{{\"storage\":\"{w}\"}}}}}}}}");
                    let p = kube(&args(&["patch", "pvc", &name, "--type", "merge", "-p", &patch], &ns));
                    if p.ok {
                        g.notes.push(format!("the volume {name} was asked to grow from {have} to {w}: resized (the StorageClass {have_class} allows it)"));
                    } else {
                        g.notes.push(format!("the volume {name} was asked to grow from {have} to {w} but the cluster refused: {}. {kept}", first_line(&p.text)));
                    }
                } else {
                    g.notes.push(format!("the volume {name} was asked to grow from {have} to {w}, but its StorageClass {have_class} does not allow growing a volume, so it stays {have}"));
                }
            }
            (Some((w, wq)), Some(hq)) if wq < hq => g.notes.push(format!("{kept}; the manifest asks for {w}, which is smaller than {have}, and a volume cannot shrink")),
            _ => g.notes.push(kept),
        }
    }
    g.manifest = if keep.is_empty() { String::new() } else { format!("{}\n", keep.iter().map(|d| d.trim_end().to_string()).collect::<Vec<_>>().join("\n---\n")) };
    Ok(g)
}

/// Guard `manifest`, apply what is left with `apply`, and return the notes followed by what the apply printed. When only
/// claims were in the manifest, nothing is applied.
pub fn guarded_apply(manifest: &str, default_ns: Option<&str>, kube: Kube, apply: &dyn Fn(&str) -> Result<String, String>) -> Result<String, String> {
    let g = guard(manifest, default_ns, kube)?;
    let mut out = g.notes.join("\n");
    if !g.manifest.trim().is_empty() {
        let printed = apply(&g.manifest).map_err(|e| explain_apply_error(&e))?;
        if !out.is_empty() && !printed.is_empty() {
            out.push('\n');
        }
        out.push_str(&printed);
    }
    Ok(out)
}

/// kubectl's refusal to change a volume, in plain words (the original text follows); any other text is returned as it is.
pub fn explain_apply_error(text: &str) -> String {
    if !(text.contains("PersistentVolumeClaim") && text.contains("immutable")) {
        return text.to_string();
    }
    let name = text.split("PersistentVolumeClaim \"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("a volume");
    format!("kubectl refused to change the existing volume {name} (a PersistentVolumeClaim). A volume's StorageClass and settings cannot be changed after it is created; only its size can grow. IYAKKI keeps an existing volume as it is and leaves it out of a repeated install, so this means the volume was made by something else with other settings: your data was not touched. Original message: {}", text.trim())
}

#[cfg(test)]
#[path = "pvcguard_tests.rs"]
mod tests;
