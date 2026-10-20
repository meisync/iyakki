//! Bugs on the feature board: a bug is a row whose id starts with `B-`, with the evidence captured when it was found.
//!
//! Evidence lives in `<workspace>/.iyakki/bugs/<B-id>/`: the logs of every container (current and previous), the pod description, the
//! events, the pod status (env values redacted), the manifest in effect, the Dockerfile, the image, the failing check's output and a
//! summary. Credentials are redacted before anything is written and every file is capped (head and tail kept, with a note). An index
//! maps a failure **signature** (normalised reason + pod name pattern + image) to its bug id, so the same failure is reported once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Most bytes of one evidence file.
pub const MAX_FILE: usize = 200_000;
/// Most lines of a container log (the request's `lines`, capped here).
pub const MAX_LINES: usize = 2000;
pub const DEFAULT_LINES: usize = 300;

pub fn bugs_dir(root: &Path) -> PathBuf {
    root.join(".iyakki/bugs")
}

/// Credentials in the text replaced by a placeholder (keys, tokens, `password: …`, private keys, `user:pw@host`).
pub fn redact(text: &str) -> String {
    let mut t = text.to_string();
    // a few passes: the detector reports every credential of the text, but a replaced text can reveal another pattern
    for _ in 0..3 {
        match crate::credguard::detect(&t) {
            Some(f) => t = f.redacted,
            None => break,
        }
    }
    // `DB_PASSWORD=value` and `api_token: value` anywhere in a line (names the detector does not know): the value goes
    const KEYS: [&str; 8] = ["password", "passwd", "secret", "token", "apikey", "api_key", "credential", "private_key"];
    t.lines()
        .map(|l| {
            let lower = l.to_lowercase();
            for (i, c) in l.char_indices() {
                if c != '=' && c != ':' {
                    continue;
                }
                let before = &lower[..i];
                let word = before.rsplit(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'))).next().unwrap_or("");
                let rest = l[i + 1..].trim();
                if KEYS.iter().any(|k| word.contains(k)) && !rest.is_empty() && !rest.starts_with("[redacted]") && !rest.starts_with("[credential") {
                    return format!("{} [redacted]", &l[..=i]);
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text cut to `max` bytes: the head and the tail are kept, with a note of how much was cut from the middle.
pub fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - half;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!("{}\n[... {} bytes cut from the middle ...]\n{}", &text[..head_end], tail_start - head_end, &text[tail_start..])
}

/// Write one evidence file: redacted, then capped. Returns the file name for the summary.
pub fn write_evidence(dir: &Path, name: &str, text: &str) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    std::fs::write(dir.join(&safe), cap(&redact(text), MAX_FILE))?;
    Ok(safe)
}

/// Pod `env` values are never kept: the names stay, the values go.
pub fn redact_pod_json(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if let Some(Value::Array(env)) = o.get_mut("env") {
                for e in env.iter_mut() {
                    if let Some(m) = e.as_object_mut() {
                        if m.contains_key("value") {
                            m.insert("value".into(), json!("[redacted]"));
                        }
                    }
                }
            }
            for x in o.values_mut() {
                redact_pod_json(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact_pod_json),
        _ => {}
    }
}

/// A pod name without its generated tail: `web-7d9f8-abcde` gives `web`.
pub fn pod_pattern(pod: &str) -> String {
    let parts: Vec<&str> = pod.split('-').collect();
    if parts.len() >= 3 { parts[..parts.len() - 2].join("-") } else { pod.to_string() }
}

/// The failure signature: reasons (lower case, digits removed), the pod name pattern and the image, so the same failure repeated is one bug.
pub fn signature(reasons: &[String], pod: &str, image: &str) -> String {
    let mut r: Vec<String> = reasons.iter().map(|x| crate::boardtrack::normalise(&x.to_lowercase())).filter(|x| !x.is_empty()).collect();
    r.sort();
    r.dedup();
    format!("{}|{}|{}", r.join("+"), pod_pattern(pod), image.split(['@', ':']).next().unwrap_or(image))
}

/// Pod or container reasons that make a failure worth a bug.
pub const BAD_REASONS: [&str; 8] = ["CrashLoopBackOff", "CreateContainerConfigError", "ImagePullBackOff", "ErrImagePull", "CreateContainerError", "OOMKilled", "RunContainerError", "InvalidImageName"];

/// The bad reasons named in a text (a `deploy_status` output, pod listings).
pub fn bad_reasons_in(text: &str) -> Vec<String> {
    BAD_REASONS.iter().filter(|r| text.contains(*r)).map(|r| r.to_string()).collect()
}

/// signature → (bug id, linked feature, created by the harness, times seen)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub items: BTreeMap<String, (String, String, bool, u32)>,
}

impl Index {
    pub fn load(root: &Path) -> Index {
        let v: Value = std::fs::read_to_string(bugs_dir(root).join("index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let items = v.as_object().map(|o| o.iter().filter_map(|(k, x)| Some((k.clone(), (x.get("id")?.as_str()?.to_string(), x.get("feature").and_then(Value::as_str).unwrap_or("").to_string(), x.get("auto").and_then(Value::as_bool).unwrap_or(false), x.get("seen").and_then(Value::as_u64).unwrap_or(1) as u32)))).collect()).unwrap_or_default();
        Index { items }
    }

    pub fn save(&self, root: &Path) {
        let d = bugs_dir(root);
        let _ = std::fs::create_dir_all(&d);
        let v: serde_json::Map<String, Value> = self.items.iter().map(|(k, (id, f, a, n))| (k.clone(), json!({"id": id, "feature": f, "auto": a, "seen": n}))).collect();
        let _ = std::fs::write(d.join("index.json"), Value::Object(v).to_string());
    }

    pub fn find(&self, sig: &str) -> Option<&(String, String, bool, u32)> {
        self.items.get(sig)
    }

    /// Bug ids linked to a feature row.
    pub fn for_feature(&self, feature: &str) -> Vec<String> {
        self.items.values().filter(|v| v.1 == feature).map(|v| v.0.clone()).collect()
    }
}

/// What a capture found.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    pub files: Vec<String>,
    pub reasons: Vec<String>,
    pub pods: Vec<String>,
    pub images: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_redacted_and_files_are_capped_with_head_and_tail() {
        let r = redact("start\nDB_PASSWORD: hunter2hunter2X9\ntoken ghp_abcdefghijklmnopqrstuvwxyz0123456789\nend");
        assert!(!r.contains("hunter2") && !r.contains("ghp_") && r.contains("start") && r.contains("end"), "{r}");
        let long = format!("HEAD{}TAIL", "x".repeat(500_000));
        let c = cap(&long, 1000);
        assert!(c.starts_with("HEAD") && c.ends_with("TAIL") && c.contains("bytes cut from the middle") && c.len() < 1100);
        assert_eq!(cap("short", 1000), "short");
        let mut v: Value = serde_json::from_str(r#"{"spec": {"containers": [{"name": "app", "env": [{"name": "DB_PASSWORD", "value": "s3cret"}, {"name": "X", "valueFrom": {"secretKeyRef": {"name": "s", "key": "k"}}}]}]}}"#).unwrap();
        redact_pod_json(&mut v);
        let t = v.to_string();
        assert!(!t.contains("s3cret") && t.contains("DB_PASSWORD") && t.contains("secretKeyRef"), "{t}");
        let d = std::env::temp_dir().join(format!("iyakki-bugs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let name = write_evidence(&d, "web-1/app.log", "line one\npassword: hunter2hunter2X9\n").unwrap();
        assert_eq!(name, "web-1_app.log");
        assert!(!std::fs::read_to_string(d.join(&name)).unwrap().contains("hunter2"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_same_failure_has_one_signature_and_different_failures_do_not() {
        let a = signature(&["CrashLoopBackOff".into()], "web-7d9f8-abcde", "localhost/web:abc123");
        let b = signature(&["CrashLoopBackOff".into()], "web-5c4d2-zzzzz", "localhost/web:def456");
        assert_eq!(a, b, "another replica set and another tag are the same failure");
        assert_ne!(a, signature(&["ImagePullBackOff".into()], "web-7d9f8-abcde", "localhost/web:abc123"));
        assert_ne!(a, signature(&["CrashLoopBackOff".into()], "api-7d9f8-abcde", "localhost/web:abc123"));
        assert_eq!(pod_pattern("web-7d9f8-abcde"), "web");
        assert_eq!(bad_reasons_in("Pod web-1: CreateContainerConfigError\nPod web-2: ImagePullBackOff"), vec!["CreateContainerConfigError".to_string(), "ImagePullBackOff".to_string()]);
    }

    #[test]
    fn the_index_remembers_signatures_and_the_bugs_of_a_feature() {
        let d = std::env::temp_dir().join(format!("iyakki-bugidx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut i = Index::load(&d);
        assert!(i.items.is_empty());
        i.items.insert("sig".into(), ("B-APP-01".into(), "F-APP-02".into(), true, 1));
        i.save(&d);
        let j = Index::load(&d);
        assert_eq!(j.find("sig").map(|x| x.0.as_str()), Some("B-APP-01"));
        assert_eq!(j.for_feature("F-APP-02"), vec!["B-APP-01".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
