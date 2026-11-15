//! Chat tools for the TLS link between the domain listener and a pod (`cloudlisten_podtls.rs`; F-CLOUD-49): `pod_tls_status` (free, shows
//! which secrets exist and how long the certificates last, never a key), `pod_tls_create` (makes a CA for a route and both certificates in
//! the tenant vault) and `pod_tls_rotate` (new certificates, or a new CA with an overlap, or drop the old CA). Both changes ask every time,
//! are always hard asks (`agent::hard_ask`: they decide who may talk to a pod) and are for administrators. Nothing a tool returns holds a
//! certificate or a key; the vault is the signed-in person's own (`cloudlisten::podtls::tenant_vault`).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::cloudlisten::podtls::{self, PodVault, Rotate, LEAF_DAYS};
use crate::settingstools::{Entry, Env};

pub const TOOLS: [&str; 3] = ["pod_tls_status", "pod_tls_create", "pod_tls_rotate"];

pub const PROMPT_RULE: &str = "\nPOD TLS (administrators): pod_tls_status (free) shows, for the TLS link between the domain listener and the pod of a route, which secrets are in the tenant vault and how long the certificates last; pod_tls_create makes the certificate authority and the two certificates for a route name; pod_tls_rotate gives new certificates (leaves), a new authority with the old one still trusted (ca) or stops trusting the old one (drop-old). Creating and rotating ask every time; they never show a certificate or a key.";

/// How a tool gets the vault (the real one is the signed-in person's; tests pass their own).
pub type VaultFor = Arc<dyn Fn() -> Result<Box<dyn PodVault>, String> + Send + Sync>;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let (dir, user) = (env.dir.clone(), env.user.clone());
    entries_with(env, Arc::new(move || Ok(Box::new(podtls::tenant_vault(&dir, &user)?) as Box<dyn PodVault>)))
}

fn name_of(a: &Value) -> Result<String, String> {
    let n = a.get("name").and_then(Value::as_str).ok_or("name is the route's pod link name")?;
    if !crate::cloudstorage::plain_name(n) {
        return Err("name is letters, digits, - and _".into());
    }
    Ok(n.to_string())
}

fn days_of(a: &Value) -> Result<u32, String> {
    match a.get("leaf_days") {
        None => Ok(LEAF_DAYS),
        Some(v) => v.as_u64().and_then(|d| u32::try_from(d).ok()).filter(|d| (1..=825).contains(d)).ok_or_else(|| "leaf_days is a number from 1 to 825".to_string()),
    }
}

pub fn entries_with(env: &Env, vault: VaultFor) -> Vec<Entry> {
    let name_schema = json!({"type": "string", "description": "the pod link name of the route (letters, digits, - and _)"});
    let (v1, v2, v3) = (vault.clone(), vault.clone(), vault);
    let (e2, e3) = (env.clone(), env.clone());
    vec![
        Entry::read("Cloud", "pod_tls_status", "Show the TLS link between the domain listener and the pod of a route: which secrets are in the tenant vault and how long the certificates last. Without a name it lists every link found. Never shows a key.", json!({"type": "object", "properties": {"name": name_schema.clone()}}), move |a| {
            let name = match a.get("name") {
                Some(_) => Some(name_of(a)?),
                None => None,
            };
            podtls::status(&*v1()?, name.as_deref(), now())
        })
        .admin(),
        Entry::change("Cloud", "pod_tls_create", "Make the TLS link between the domain listener and the pod of a route: a certificate authority for the route and one certificate each for the listener and the pod, stored in the tenant vault. Refuses when the link exists (use pod_tls_rotate). The person approves a card first.", json!({"type": "object", "properties": {"name": name_schema.clone(), "leaf_days": {"type": "integer", "description": "how long the certificates last, 1 to 825 (default 90)"}}, "required": ["name"]}), |a| format!("Make the pod TLS link {} (new certificate authority and certificates in the tenant vault)", a["name"].as_str().unwrap_or("?")), move |a| {
            let (name, days) = (name_of(a)?, days_of(a)?);
            let v = v2()?;
            if v.names()?.contains(&podtls::SecretNames::of(&name).ca) {
                return Err(format!("the pod link {name} exists already; pod_tls_rotate renews it"));
            }
            let lines = podtls::provision(&*v, &name, now(), days)?;
            e2.audit("cloud.pod_tls.create", &name);
            Ok(lines.join("\n"))
        })
        .admin(),
        Entry::change("Cloud", "pod_tls_rotate", "Renew the TLS link of a route: what is leaves (new listener and pod certificates, same authority), ca (a new authority, the old one still trusted until drop-old) or drop-old (trust only the current authority). The person approves a card first.", json!({"type": "object", "properties": {"name": name_schema, "what": {"type": "string", "enum": ["leaves", "ca", "drop-old"]}, "leaf_days": {"type": "integer"}}, "required": ["name", "what"]}), |a| format!("Rotate the pod TLS link {}: {}", a["name"].as_str().unwrap_or("?"), a["what"].as_str().unwrap_or("?")), move |a| {
            let (name, days) = (name_of(a)?, days_of(a)?);
            let what = match a.get("what").and_then(Value::as_str) {
                Some("leaves") => Rotate::Leaves,
                Some("ca") => Rotate::CaOverlap,
                Some("drop-old") => Rotate::DropOldCa,
                _ => return Err("what is leaves, ca or drop-old".into()),
            };
            let v = v3()?;
            if !v.names()?.contains(&podtls::SecretNames::of(&name).ca) {
                return Err(format!("there is no pod link {name}; pod_tls_create makes it"));
            }
            let lines = podtls::rotate(&*v, &*v, &name, what, now(), days)?;
            e3.audit("cloud.pod_tls.rotate", &format!("{name} {}", a["what"].as_str().unwrap_or("?")));
            Ok(lines.join("\n"))
        })
        .admin(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::cloudlisten::podtls::SecretWriter;
    use crate::cloudlisten::vault::SecretReader;
    use crate::settingstools_tests::{assert_policy, rig, tools};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Mem(Arc<Mutex<HashMap<String, Vec<u8>>>>);
    impl SecretReader for Mem {
        fn read(&self, n: &str) -> Result<Vec<u8>, String> {
            self.0.lock().unwrap().get(n).cloned().ok_or_else(|| "404".to_string())
        }
    }
    impl SecretWriter for Mem {
        fn write(&self, n: &str, v: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().insert(n.into(), v.to_vec());
            Ok(())
        }
    }
    impl PodVault for Mem {
        fn names(&self) -> Result<Vec<String>, String> {
            Ok(self.0.lock().unwrap().keys().cloned().collect())
        }
    }

    #[test]
    fn the_tools_make_and_renew_a_link_ask_every_time_and_show_no_key() {
        let r = rig("podtls", true);
        let m = Mem::default();
        let m2 = m.clone();
        let t = tools(&r, entries_with(&r.env, Arc::new(move || Ok(Box::new(m2.clone()) as Box<dyn PodVault>))));
        assert_policy(&t, &["pod_tls_status"], &["pod_tls_create", "pod_tls_rotate"]);
        assert!(t.call("pod_tls_status", &json!({})).unwrap().contains("No pod link"));
        assert!(t.call("pod_tls_rotate", &json!({"name": "app", "what": "leaves"})).unwrap_err().contains("no pod link"));
        let made = t.call("pod_tls_create", &json!({"name": "app", "leaf_days": 30})).unwrap();
        assert!(made.contains("app.pod.iyakki.internal"), "{made}");
        assert!(t.call("pod_tls_create", &json!({"name": "app"})).unwrap_err().contains("exists already"), "a second create does not replace the authority");
        let before = m.read("app-pod-client-chain").unwrap();
        t.call("pod_tls_rotate", &json!({"name": "app", "what": "leaves"})).unwrap();
        assert_ne!(before, m.read("app-pod-client-chain").unwrap());
        t.call("pod_tls_rotate", &json!({"name": "app", "what": "ca"})).unwrap();
        assert_eq!(String::from_utf8(m.read("app-pod-ca").unwrap()).unwrap().matches("BEGIN CERTIFICATE").count(), 2);
        t.call("pod_tls_rotate", &json!({"name": "app", "what": "drop-old"})).unwrap();
        assert_eq!(String::from_utf8(m.read("app-pod-ca").unwrap()).unwrap().matches("BEGIN CERTIFICATE").count(), 1);
        let s = t.call("pod_tls_status", &json!({})).unwrap();
        assert!(s.contains("app: CA valid for") && s.contains("listener certificate valid for"), "{s}");
        assert!(!s.contains("PRIVATE") && !s.contains("BEGIN"), "no key or certificate text: {s}");
        for bad in [json!({"name": "../x", "what": "leaves"}), json!({"name": "app", "what": "sideways"}), json!({"name": "app", "what": "leaves", "leaf_days": 0})] {
            assert!(t.call("pod_tls_rotate", &bad).is_err(), "{bad}");
        }
        assert!(r.audits.lock().unwrap().iter().any(|a| a == "cloud.pod_tls.create app") && r.audits.lock().unwrap().iter().any(|a| a == "cloud.pod_tls.rotate app drop-old"));
        // not an administrator: none of the three is offered
        let p = rig("podtls2", false);
        let t2 = tools(&p, entries_with(&p.env, Arc::new(|| Err("locked".into()))));
        assert!(t2.defs().iter().all(|d| !d.spec.name.starts_with("pod_tls")));
        // a locked vault is said plainly
        let t3 = tools(&r, entries_with(&r.env, Arc::new(|| Err("the tenant vault is locked".to_string()))));
        assert!(t3.call("pod_tls_status", &json!({})).unwrap_err().contains("locked"));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}
