//! The class of break "a facet was forgotten in the real wiring" (KI-devmode-off-2026-11-17): every trait of the agent API
//! that has default (fake) answers must be overridden by the real implementation, method by method; otherwise production
//! silently answers "unavailable". This reads the sources, so a new trait or method is covered the day it is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn fn_name(line: &str) -> Option<String> {
    let t = line.strip_prefix("    ")?;
    if t.starts_with(' ') {
        return None;
    }
    let t = t.strip_prefix("pub ").unwrap_or(t);
    let t = t.strip_prefix("fn ")?;
    Some(t.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?.to_string())
}

/// trait name -> methods that have a default body.
fn default_methods(src: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut m = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("pub trait ") {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            cur = Some(name.clone());
            m.entry(name).or_insert_with(BTreeSet::new);
            continue;
        }
        if line == "}" {
            cur = None;
            continue;
        }
        if let (Some(t), Some(f)) = (&cur, fn_name(line)) {
            // a default body opens a block on the signature line; a required method ends in `;`
            if !line.trim_end().ends_with(';') {
                m.get_mut(t).unwrap().insert(f);
            }
        }
    }
    m
}

/// trait name -> methods the impl blocks (for any type that is not a fake or a test double) override.
fn overridden(src: &str, out: &mut BTreeMap<String, BTreeSet<String>>) {
    let mut cur: Option<String> = None;
    for line in src.lines() {
        if line.starts_with("impl") && line.trim_end().ends_with('{') {
            cur = None;
            if let Some((a, b)) = line.split_once(" for ") {
                let tr = a.trim_start_matches("impl").trim_start();
                let tr = tr.strip_prefix('<').map_or(tr, |r| r.split_once("> ").map_or(r, |x| x.1));
                let tr: String = tr.rsplit("::").next().unwrap_or(tr).chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let ty = b.trim();
                if !ty.starts_with("Fake") && !ty.starts_with("No") && !ty.contains("Test") {
                    cur = Some(tr.clone());
                    out.entry(tr).or_default();
                }
            }
            continue;
        }
        if line == "}" {
            cur = None;
            continue;
        }
        if let (Some(t), Some(f)) = (&cur, fn_name(line)) {
            out.get_mut(t).unwrap().insert(f);
        }
    }
}

/// Methods that are deliberately left at their default in production (nothing to do there, or the answer is the same).
/// Each line: Trait::method, with the reason. Keep this list short; it may only shrink.
const ALLOWED: &[&str] = &[];

#[test]
fn every_default_answer_of_an_agent_api_trait_is_overridden_by_the_real_implementation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut api = Vec::new();
    rs_files(&root.join("iyakki-agent-api/src"), &mut api);
    let mut defaults: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &api {
        if f.file_name().is_some_and(|n| n == "generated.rs") || f.to_string_lossy().ends_with("_tests.rs") {
            continue;
        }
        for (k, v) in default_methods(&std::fs::read_to_string(f).unwrap()) {
            defaults.entry(k).or_default().extend(v);
        }
    }
    let mut real = BTreeMap::new();
    let mut agent = Vec::new();
    rs_files(&root.join("iyakki-agent/src"), &mut agent);
    for f in &agent {
        overridden(&std::fs::read_to_string(f).unwrap(), &mut real);
    }
    let mut missing = Vec::new();
    let mut checked = 0;
    for (tr, ms) in &defaults {
        let Some(done) = real.get(tr) else { continue }; // a trait nothing real implements is a different problem (see below)
        for m in ms {
            checked += 1;
            let key = format!("{tr}::{m}");
            if !done.contains(m) && !ALLOWED.contains(&key.as_str()) {
                missing.push(key);
            }
        }
    }
    assert!(checked > 300, "the scan found only {checked} defaulted methods: the parser no longer reads the sources");
    assert!(missing.is_empty(), "defaults that production silently falls back to:\n{}", missing.join("\n"));
}

#[test]
fn every_agent_api_trait_with_defaults_has_a_real_implementation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut api = Vec::new();
    rs_files(&root.join("iyakki-agent-api/src"), &mut api);
    let mut defaults: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &api {
        if f.file_name().is_some_and(|n| n == "generated.rs") || f.to_string_lossy().ends_with("_tests.rs") {
            continue;
        }
        for (k, v) in default_methods(&std::fs::read_to_string(f).unwrap()) {
            defaults.entry(k).or_default().extend(v);
        }
    }
    let mut real = BTreeMap::new();
    let mut all = Vec::new();
    for c in ["iyakki-agent/src", "iyakki-agent-api/src", "iyakki-shared/src", "iyakki-shell/src", "iyakki-apps/src", "iyakki-core/src", "iyakki-core-local/src"] {
        rs_files(&root.join(c), &mut all);
    }
    for f in &all {
        overridden(&std::fs::read_to_string(f).unwrap(), &mut real);
    }
    let orphans: Vec<_> = defaults.iter().filter(|(t, ms)| !ms.is_empty() && !real.contains_key(*t)).map(|(t, _)| t.clone()).collect();
    assert!(orphans.is_empty(), "traits whose defaults are the only answer in production: {orphans:?}");
}

/// The handle the program installs must be the real one, and the real one must really serve developer mode: switch it on in a
/// temporary data folder with an administrator policy (the Settings page's own call chain), see it on, switch it off.
#[test]
fn the_real_handle_is_real_and_developer_mode_switches_on_through_it() {
    use iyakki_agent_api::AgentFacets;
    use iyakki_sec::policy::Role;
    assert!(!iyakki_agent_api::FakeAgentApi::default().is_real(), "the fake must say it is not real");
    let api = iyakki_agent::AgentApiImpl;
    assert!(api.is_real());
    let dir = std::env::temp_dir().join(format!("iyakki-devfix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dm = iyakki_agent_api::AgentFacetsS6B::host_probe(&api).devmode(&dir);
    assert!(!dm.is_on());
    let st = dm.enable(&Role::Admin.template(), "root", 2).expect("developer mode must switch on, not answer unavailable");
    assert!(dm.is_on(), "{st:?}");
    dm.disable(&Role::Admin.template(), "root").unwrap();
    assert!(!dm.is_on());
    let _ = std::fs::remove_dir_all(&dir);
}
