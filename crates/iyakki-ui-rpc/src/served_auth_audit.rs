//! The audit that keeps the policy/session hole closed (D-SEC1-VOUCH): it walks every served method and fails when a method takes an
//! authority-carrying argument that is neither overwritten by the core (`served_auth::RULES`) nor on the reviewed allow-list
//! (`served_auth::ALLOWED`). Two views of the same surface are read:
//!
//! * the wire: every `"method" => { let (..): (types) = de(a)?; ..}` arm of the generated dispatchers (`gen_serve.rs`, `gen_services.rs`) and of
//!   the hand-written ones (`served_*.rs`): a decoded `Policy`, `Role`, `PolicyOverride`, `Session`, `Actor` or `Who` is an authority claim;
//! * the signatures: every method of the remote objects (`gen_remote.rs`, `gen_services.rs`, `served_*.rs`) with an argument NAMED like a
//!   right (`admin`, `admins`, `is_admin`, `can_*`, `approved`, `allowed`), whatever its type (a bool or a list of names crosses as data and
//!   would pass the first view).
//!
//! A new method that does either, with no rule, fails here with the method named. The audit is itself tested on synthetic source text, so it
//! cannot go quiet by accident.

use std::collections::BTreeMap;

use crate::served_auth::{Claim, Why, ALLOWED, RULES};

const AUTH_TYPES: &[&str] = &["Policy", "Role", "PolicyOverride", "Session", "Actor", "Who"];

fn is_auth_type(ty: &str) -> bool {
    ty.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| AUTH_TYPES.contains(&w))
}

fn is_auth_name(name: &str) -> bool {
    matches!(name, "admin" | "admins" | "is_admin" | "approved" | "allowed") || name.starts_with("can_")
}

fn split_top(s: &str) -> Vec<String> {
    let (mut out, mut depth, mut cur) = (Vec::new(), 0i32, String::new());
    let s = s.replace("->", "\u{1}");
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            out.push(cur.trim().replace('\u{1}', "->"));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().replace('\u{1}', "->"));
    }
    out
}

fn quoted(l: &str) -> Option<String> {
    let l = l.trim_start();
    let rest = l.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// What the dispatchers decode, as (trait, method, wire index, type).
fn wire_args(files: &[(&str, &str)]) -> Vec<(String, String, usize, String)> {
    let mut out = Vec::new();
    for (name, text) in files {
        let (mut tr, mut method) = (String::new(), String::new());
        let mut want_iface = false;
        for l in text.lines() {
            let t = l.trim_start();
            if let Some(rest) = t.split("fn serve_").nth(1) {
                let id: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if t.contains("t: &Arc<dyn ") || id == "services" {
                    tr = if id == "services" { "ServicesHandle".into() } else { id };
                }
            }
            if let Some(rest) = t.split("fn s_").nth(1) {
                let id: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if let Some((a, b)) = id.split_once('_') {
                    tr = a.to_string();
                    method = b.to_string();
                }
            }
            if want_iface {
                if let Some(q) = quoted(t) {
                    tr = q;
                    want_iface = false;
                }
            }
            if t.contains("fn iface(&self)") {
                want_iface = true;
                if let Some(q) = t.split("{ ").nth(1).and_then(quoted) {
                    tr = q;
                    want_iface = false;
                }
            }
            if t.starts_with('"') && t.contains("=>") {
                if let Some(m) = quoted(t) {
                    method = m;
                }
            }
            let tuple = t.split("): (").nth(1).and_then(|r| r.split(") = de(a)").next()).or_else(|| t.split("de::<(").nth(1).and_then(|r| r.split(")>(a)").next()));
            if let Some(types) = tuple {
                if tr.is_empty() || method.is_empty() {
                    continue;
                }
                for (i, ty) in split_top(types).into_iter().enumerate() {
                    out.push((tr.clone(), method.clone(), i, ty));
                }
            }
            let _ = name;
        }
    }
    out
}

/// What the remote objects declare: (trait, method) to the argument names in order (the receiver and a leading `s: &Remote..` of a `c_` function
/// left out).
fn signatures(files: &[(&str, &str)]) -> BTreeMap<(String, String), Vec<(String, String)>> {
    let mut out = BTreeMap::new();
    for (_, text) in files {
        let mut tr = String::new();
        let lines: Vec<&str> = text.lines().collect();
        for (li, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if let Some(rest) = t.strip_prefix("impl ") {
                if let Some((a, _)) = rest.split_once(" for ") {
                    tr = a.rsplit("::").next().unwrap_or(a).trim().to_string();
                }
            }
            let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
            let Some(rest) = t.strip_prefix("fn ") else { continue };
            // a call the proxy answers "not served" never reaches the core: its arguments are dropped (`let _ = (..);`)
            if lines.get(li + 1).is_some_and(|n| n.trim_start().starts_with("let _ = (")) {
                continue;
            }
            let Some((name, rest)) = rest.split_once('(') else { continue };
            let name = name.split('<').next().unwrap_or(name);
            let Some(args) = rest.rsplit_once(')').map(|x| x.0) else { continue };
            let mut params = split_top(args);
            let (key_trait, key_method, drop_first) = if let Some(c) = name.strip_prefix("c_") {
                match c.split_once('_') {
                    Some((a, b)) => (a.to_string(), b.to_string(), 1),
                    None => continue,
                }
            } else if tr.is_empty() {
                continue;
            } else {
                (tr.clone(), name.to_string(), 0)
            };
            if params.first().is_some_and(|p| p.contains("self")) {
                params.remove(0);
            }
            if drop_first == 1 && !params.is_empty() {
                params.remove(0);
            }
            let named: Vec<(String, String)> = params.into_iter().filter_map(|p| p.split_once(':').map(|(n, t)| (n.trim().trim_start_matches("mut ").trim_start_matches('_').to_string(), t.trim().to_string()))).collect();
            out.insert((key_trait, key_method), named);
        }
    }
    out
}

fn covered(tr: &str, method: &str, idx: usize) -> bool {
    if crate::served_login::ticketed(tr, method).is_some_and(|t| t.approved == idx) {
        return true; // `approved` is a core-issued ticket (served_login.rs)
    }
    RULES.iter().filter(|r| r.iface == tr && r.method == method).any(|r| {
        r.claims.iter().any(|c| match c {
            Claim::Policy(i) | Claim::Role(i) | Claim::Admin(i) | Claim::Admins(i) | Claim::Session(i) => *i == idx,
            Claim::ScopeFlags { shared, own, .. } => *shared == idx || *own == idx,
        })
    })
}

fn allowed(tr: &str, method: &str, name: &str) -> bool {
    ALLOWED.iter().any(|(t, m, n, _)| *t == tr && *m == method && *n == name)
}

/// Every authority-carrying argument that has no rule and no reviewed reason. Empty when the surface is closed.
pub(crate) fn violations(wire_files: &[(&str, &str)], sig_files: &[(&str, &str)]) -> Vec<String> {
    let sigs = signatures(sig_files);
    let mut bad = Vec::new();
    for (tr, method, idx, ty) in wire_args(wire_files) {
        if !is_auth_type(&ty) {
            continue;
        }
        let name = sigs.get(&(tr.clone(), method.clone())).and_then(|v| v.get(idx)).map(|(n, _)| n.clone()).unwrap_or_else(|| format!("arg{idx}"));
        if !covered(&tr, &method, idx) && !allowed(&tr, &method, &name) {
            bad.push(format!("{tr}::{method} decodes a `{ty}` as argument {idx} ({name}) from the wire: add a served_auth RULE (the core reads it from its accounts) or a reviewed ALLOWED entry"));
        }
    }
    for ((tr, method), params) in &sigs {
        for (idx, (name, ty)) in params.iter().enumerate() {
            if is_auth_name(name) && !covered(tr, method, idx) && !allowed(tr, method, name) && !ty.contains("dyn") {
                bad.push(format!("{tr}::{method} takes `{name}: {ty}`, a name that claims a right: add a served_auth RULE or a reviewed ALLOWED entry"));
            }
        }
    }
    bad
}

const WIRE: &[(&str, &str)] = &[
    ("gen_serve.rs", include_str!("gen_serve.rs")),
    ("gen_services.rs", include_str!("gen_services.rs")),
    ("served.rs", include_str!("served.rs")),
    ("served_apps.rs", include_str!("served_apps.rs")),
    ("served_git.rs", include_str!("served_git.rs")),
    ("served_hosts.rs", include_str!("served_hosts.rs")),
    ("served_jobs.rs", include_str!("served_jobs.rs")),
    ("served_person.rs", include_str!("served_person.rs")),
    ("served_policy.rs", include_str!("served_policy.rs")),
    ("served_sound.rs", include_str!("served_sound.rs")),
    ("served_state.rs", include_str!("served_state.rs")),
    ("served_store.rs", include_str!("served_store.rs")),
];

const SIGS: &[(&str, &str)] = &[
    ("gen_remote.rs", include_str!("gen_remote.rs")),
    ("gen_services.rs", include_str!("gen_services.rs")),
    ("served_apps.rs", include_str!("served_apps.rs")),
    ("served_hosts.rs", include_str!("served_hosts.rs")),
    ("served_policy.rs", include_str!("served_policy.rs")),
    ("served_person.rs", include_str!("served_person.rs")),
    ("served_sound.rs", include_str!("served_sound.rs")),
];

#[test]
fn every_served_method_with_an_authority_argument_has_a_rule_or_a_reviewed_reason() {
    let bad = violations(WIRE, SIGS);
    assert!(bad.is_empty(), "the UI socket would carry a right the core does not read for itself:\n{}", bad.join("\n"));
}

#[test]
fn the_audit_sees_a_new_method_that_takes_a_policy_a_role_a_session_or_an_admin_flag() {
    let wire = [("gen_serve.rs", "    pub(crate) fn serve_NewFacet(t: &Arc<dyn NewFacet>, reg: &crate::Registry<'_>, method: &str, a: Value) -> Result<Value, String> {\n        match method {\n            \"grant\" => {\n                let (x0, x1, ): (String, iyakki_sec::policy::Policy, ) = de(a)?;\n                ser(&t.grant(&x0, &x1))\n            }\n            \"promote\" => {\n                let (x0, x1, ): (iyakki_accounts::Session, iyakki_sec::policy::Role, ) = de(a)?;\n                ser(&t.promote(&x0, x1))\n            }\n            \"read\" => {\n                let (x0, ): (String, ) = de(a)?;\n                ser(&t.read(&x0))\n            }\n        }\n    }\n")];
    let sigs = [("gen_remote.rs", "    impl NewFacet for RemoteNewFacet {\n        fn grant(&self, user: &str, policy: &Policy) -> bool {\n        }\n        fn announce(&self, is_admin: bool, can_edit: bool) {\n        }\n        fn read(&self, user: &str) -> bool {\n        }\n    }\n")];
    let bad = violations(&wire, &sigs);
    assert_eq!(bad.len(), 5, "{bad:?}");
    assert!(bad.iter().any(|b| b.starts_with("NewFacet::grant") && b.contains("Policy")));
    assert!(bad.iter().filter(|b| b.starts_with("NewFacet::promote")).count() == 2);
    assert!(bad.iter().any(|b| b.contains("NewFacet::announce") && b.contains("is_admin")));
    assert!(bad.iter().any(|b| b.contains("NewFacet::announce") && b.contains("can_edit")));
    assert!(!bad.iter().any(|b| b.contains("NewFacet::read")));
}

#[test]
fn every_rule_and_every_reviewed_reason_still_names_a_served_method() {
    let sigs = signatures(SIGS);
    let wire = wire_args(WIRE);
    for r in RULES {
        assert!(wire.iter().any(|(t, m, ..)| *t == r.iface && *m == r.method) || sigs.contains_key(&(r.iface.to_string(), r.method.to_string())), "rule for {}::{} names no served method (renamed or removed?)", r.iface, r.method);
    }
    for (t, m, n, why) in ALLOWED {
        let params = sigs.get(&(t.to_string(), m.to_string())).unwrap_or_else(|| panic!("allow-list entry {t}::{m} names no method"));
        assert!(params.iter().any(|(pn, _)| pn == n), "allow-list entry {t}::{m}::{n} names no argument");
        match why {
            Why::Data(s) | Why::Open(s) => assert!(s.len() > 20, "{t}::{m}::{n}: a reason is a sentence"),
        }
    }
}

#[test]
fn a_rule_only_reaches_the_claims_of_its_own_method_and_every_claim_index_is_in_range_of_the_signature() {
    let sigs = signatures(SIGS);
    for r in RULES {
        let Some(params) = sigs.get(&(r.iface.to_string(), r.method.to_string())) else { continue };
        let max = r.claims.iter().flat_map(|c| match c {
            Claim::Policy(i) | Claim::Role(i) | Claim::Admin(i) | Claim::Admins(i) | Claim::Session(i) => vec![*i],
            Claim::ScopeFlags { scope, shared, own, .. } => vec![*scope, *shared, *own],
        }).chain(r.user).max().unwrap_or(0);
        assert!(max < params.len(), "{}::{}: a rule points at argument {max} but the method has {}", r.iface, r.method, params.len());
    }
}

#[test]
fn every_ticketed_call_names_its_approved_argument_and_its_wire_length_matches_the_signature() {
    let sigs = signatures(SIGS);
    for t in crate::served_login::TICKETED {
        let params = sigs.get(&(t.iface.to_string(), t.method.to_string())).unwrap_or_else(|| panic!("{}::{} is not a served method", t.iface, t.method));
        assert_eq!(params.get(t.approved).map(|p| p.0.as_str()), Some("approved"), "{}::{}: argument {} is not `approved`", t.iface, t.method, t.approved);
        let wire = params.iter().filter(|(_, ty)| !ty.contains("dyn")).count();
        assert_eq!(wire, t.wire_args, "{}::{}: the wire carries {wire} arguments", t.iface, t.method);
    }
}

#[test]
fn no_authority_argument_is_left_open_since_the_acting_person_is_bound() {
    assert!(!ALLOWED.iter().any(|(.., why)| matches!(why, Why::Open(_))), "an Open entry needs an owner decision in docs/development/ALPHA-8-DECISIONS.md");
}
