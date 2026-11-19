//! The real inputs of the `app.permissions` watch (F-SDK-217), read from the files of the machine: the approvals of the store (what each
//! app holds and until when), the store's revocation list, the tenant or company policy files and the people's roles and overrides (whose
//! file rules say which folders an app may read). Everything here is a pure read of text the caller hands over, so the tests need no
//! machine; `facts_from_dir` is the one place that reads the files.

use crate::appevent_perm::PermFacts;
use iyakki_core_api::appcaps::Approval;
use iyakki_shared::store_revocation::Revocations;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A small stable hash (FNV-1a, 64 bit): the epoch changes when any policy text changes, and nothing else needs it.
pub fn fnv(texts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in texts {
        for b in t.bytes().chain(std::iter::once(0)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// `(person, resolved policy)` for each account in the text of `users.json` (a broken file or entry gives nothing).
pub fn user_policies(users_json: &str) -> BTreeMap<String, iyakki_sec::policy::Policy> {
    #[derive(serde::Deserialize)]
    struct Rec {
        name: String,
        role: iyakki_sec::policy::Role,
        #[serde(default)]
        overrides: iyakki_sec::policy::PolicyOverride,
    }
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        users: Vec<serde_json::Value>,
    }
    let Ok(f) = serde_json::from_str::<File>(users_json) else { return BTreeMap::new() };
    f.users.into_iter().filter_map(|v| serde_json::from_value::<Rec>(v).ok()).map(|r| (r.name, iyakki_sec::policy::Policy::resolve(r.role, &r.overrides))).collect()
}

/// The folders (the granted `files.read` parameters) that the person's file policy still lets an app read. A grant for "any folder"
/// (no parameter) is not a folder name and is left out.
pub fn readable_folders(a: &Approval, policy: Option<&iyakki_sec::policy::Policy>, user: &str) -> BTreeSet<String> {
    a.grants
        .iter()
        .filter(|g| g.name == "files.read" && a.holds("files.read", g.param.as_deref()))
        .filter_map(|g| g.param.clone())
        .filter(|f| policy.is_some_and(|p| p.allows_read(user, f)))
        .collect()
}

/// Everything one look needs, from the approvals map, the revocation list and the policy texts.
pub fn facts_from(approvals: &BTreeMap<String, Approval>, revs: &Revocations, policy_texts: &[&str], users_json: &str, now: i64) -> PermFacts {
    let policies = user_policies(users_json);
    let mut f = PermFacts { now, policy_epoch: fnv(&[&policy_texts.join("\u{1}"), &users_epoch(users_json)]), ..Default::default() };
    for a in approvals.values() {
        let key = (a.who.clone(), a.app.clone());
        let held: BTreeSet<String> = a.grants.iter().filter(|g| a.holds_at(&g.name, g.param.as_deref(), now)).map(|g| g.name.clone()).collect();
        f.effective.entry(key.clone()).or_default().extend(held);
        for (cap, end) in a.ends() {
            f.expiries.insert((a.who.clone(), a.app.clone(), cap), end);
        }
        let folders = readable_folders(a, policies.get(&a.who), &a.who);
        if a.grants.iter().any(|g| g.name == "files.read" && g.param.is_some()) {
            f.folders.entry(key).or_default().extend(folders);
        }
        if ["app", "tile"].iter().any(|k| revs.package_revoked(k, &a.app, &a.app_version, &a.app_sha256).is_some()) {
            f.store_revoked.insert(a.app.clone());
        }
    }
    f
}

/// What of `users.json` is policy: the roles and overrides, never the hashes (a password change is not a policy change).
fn users_epoch(users_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(users_json) else { return String::new() };
    let Some(list) = v.get("users").and_then(|u| u.as_array()) else { return String::new() };
    list.iter().map(|u| format!("{}|{}|{}", u.get("name").and_then(|n| n.as_str()).unwrap_or(""), u.get("role").map(|r| r.to_string()).unwrap_or_default(), u.get("overrides").map(|o| o.to_string()).unwrap_or_default())).collect::<Vec<_>>().join("\n")
}

/// Read the files of the data folder and make one look.
pub fn facts_from_dir(dir: &Path, now: i64) -> PermFacts {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let approvals = iyakki_shared::store_approvals::read_map(&dir.join("store").join("approvals.json"));
    let revs = Revocations::load(&Revocations::path(&dir.join("store")));
    let cfg = dir.join("cloud").join("config");
    let (a, b) = (read(cfg.join("tenant-config.json")), read(cfg.join("tenant-config.local.json")));
    facts_from(&approvals, &revs, &[&a, &b], &read(dir.join("users.json")), now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::appcaps::Grant;

    fn approval(app: &str, who: &str, grants: &[(&str, Option<&str>)], ends: &[(&str, i64)]) -> Approval {
        let mut extra = serde_json::Map::new();
        if !ends.is_empty() {
            extra.insert("ends".into(), serde_json::Value::Object(ends.iter().map(|(k, v)| (k.to_string(), (*v).into())).collect()));
        }
        Approval { who: who.into(), app: app.into(), app_version: "1.0".into(), app_sha256: "abc".into(), at: 0, grants: grants.iter().map(|(n, p)| Grant { name: n.to_string(), param: p.map(str::to_string) }).collect(), revoked: vec![], extra }
    }

    const USERS: &str = r#"{"schema":1,"users":[{"name":"pat","hash":"x","role":"user","overrides":{}}]}"#;

    #[test]
    fn a_grant_with_an_end_is_held_until_its_time_and_the_look_names_the_end() {
        let book = BTreeMap::from([("i1".to_string(), approval("notes", "pat", &[("events.power", None), ("camera.view", None)], &[("camera.view", 1000)]))]);
        let before = facts_from(&book, &Revocations::default(), &[], USERS, 999);
        let after = facts_from(&book, &Revocations::default(), &[], USERS, 1000);
        let key = ("pat".to_string(), "notes".to_string());
        assert!(before.effective[&key].contains("camera.view"));
        assert!(!after.effective[&key].contains("camera.view") && after.effective[&key].contains("events.power"));
        assert_eq!(after.expiries[&("pat".into(), "notes".into(), "camera.view".into())], 1000);
    }

    #[test]
    fn the_policy_epoch_follows_policy_text_and_roles_but_not_a_password() {
        let book = BTreeMap::new();
        let e = |texts: &[&str], users: &str| facts_from(&book, &Revocations::default(), texts, users, 0).policy_epoch;
        assert_eq!(e(&["a"], USERS), e(&["a"], &USERS.replace("\"x\"", "\"other-hash\"")));
        assert_ne!(e(&["a"], USERS), e(&["b"], USERS));
        assert_ne!(e(&["a"], USERS), e(&["a"], &USERS.replace("\"user\"", "\"admin\"")));
        assert_eq!(e(&["a"], "not json"), e(&["a"], "{}"));
    }

    #[test]
    fn a_folder_leaves_the_list_when_the_persons_file_rule_narrows() {
        let a = approval("notes", "pat", &[("files.read", Some("/home/pat/Documents")), ("files.read", Some("/etc/secret")), ("files.read", None)], &[]);
        let book = BTreeMap::from([("i1".to_string(), a)]);
        let wide = facts_from(&book, &Revocations::default(), &[], USERS, 0);
        let key = ("pat".to_string(), "notes".to_string());
        assert_eq!(wide.folders[&key], BTreeSet::from(["/home/pat/Documents".to_string()]), "the folder outside the rule is never readable");
        let narrow = USERS.replace("\"overrides\":{}", "\"overrides\":{\"fs_read\":[\"/home/pat/Pictures\"]}");
        let n = facts_from(&book, &Revocations::default(), &[], &narrow, 0);
        assert!(n.folders[&key].is_empty());
        let mut w = crate::appevent_perm::PermWatch::default();
        w.observe(wide);
        let got = w.observe(n);
        assert!(got.iter().any(|t| t.4 == "policy" && t.2 == ["folder:/home/pat/Documents"] && t.3.is_empty()), "{got:?}");
    }

    #[test]
    fn an_app_on_the_revocation_list_is_marked_and_files_are_read_from_a_folder() {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-permsrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("store")).unwrap();
        std::fs::create_dir_all(d.join("cloud").join("config")).unwrap();
        let book = BTreeMap::from([("i1".to_string(), approval("notes", "pat", &[("events.power", None)], &[]))]);
        std::fs::write(d.join("store").join("approvals.json"), serde_json::to_string(&book).unwrap()).unwrap();
        std::fs::write(d.join("users.json"), USERS).unwrap();
        std::fs::write(d.join("cloud").join("config").join("tenant-config.json"), "one").unwrap();
        let root = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let roots = iyakki_shared::store_trust::Roots::with("r", &root);
        let mut revs = Revocations::default();
        revs.accept(&revs.sign_for_tests(&root, 1, 100, vec![], vec![], vec![("notes".into(), None)]), &roots, 100).unwrap();
        let marked = facts_from(&book, &revs, &[], USERS, 5);
        assert!(marked.store_revoked.contains("notes"));
        let f1 = facts_from_dir(&d, 5);
        assert!(f1.effective.contains_key(&("pat".to_string(), "notes".to_string())) && f1.store_revoked.is_empty());
        std::fs::write(d.join("cloud").join("config").join("tenant-config.json"), "two").unwrap();
        assert_ne!(f1.policy_epoch, facts_from_dir(&d, 5).policy_epoch);
        let _ = std::fs::remove_dir_all(&d);
    }
}
