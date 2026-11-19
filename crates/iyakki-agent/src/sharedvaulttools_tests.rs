//! The chat tools for the shared vault's provider: reading is free, changing asks every time, a token is never taken.

use crate::agent::{hard_ask, Risk, Tools};
use crate::sharedvault::{Provider, Setting};
use crate::sharedvaulttools::*;
use serde_json::json;
use std::sync::Arc;

fn rig(tag: &str) -> (SharedVaultTools, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-svt-{tag}-{}", crate::cloudkeys::hex(&crate::cloudkeys::random::<6>().unwrap())));
    std::fs::create_dir_all(&d).unwrap();
    (SharedVaultTools::new(Arc::new(crate::assign::tests_support::NoTools), d.clone(), "alice"), d)
}

fn ext_args() -> serde_json::Value {
    json!({"provider": "external", "address": "https://kms.example.invalid", "path": "iyakki/acme", "token_secret": "shared-vault-token"})
}

#[test]
fn reading_shows_no_secrets_changing_asks_every_time_and_the_setting_is_saved() {
    let (t, d) = rig("a");
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|x| x.spec.name == n).map(|x| x.risk);
    assert!(matches!(risk(READ_TOOL), Some(Risk::Safe)) && matches!(risk(WRITE_TOOL), Some(Risk::Ask)));
    // both always ask: the general rule for any tool whose name has "vault" in it (the read tool shows no secrets, but still asks)
    assert!(hard_ask(WRITE_TOOL, &json!({})).is_some() && hard_ask(READ_TOOL, &json!({})).is_some());
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL) && t.grant_blocker(WRITE_TOOL, &json!({})).is_some());
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("built-in") && t.call(READ_TOOL, &json!({})).unwrap().contains("not externally reviewed"));
    let card = t.preview(WRITE_TOOL, &ext_args()).unwrap();
    assert!(card.contains("shared-vault-token") && card.contains("never falls back"));
    assert!(t.refusal(WRITE_TOOL, &ext_args()).is_none());
    t.call(WRITE_TOOL, &ext_args()).unwrap();
    assert_eq!(Setting::load(&d).unwrap().provider, Provider::External);
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("external"));
    t.call(WRITE_TOOL, &json!({"provider": "builtin"})).unwrap();
    assert_eq!(Setting::load(&d).unwrap().provider, Provider::Builtin);
}

#[test]
fn a_token_is_refused_without_being_echoed_and_nothing_is_saved() {
    let (t, d) = rig("b");
    let mut a = ext_args();
    a["token_secret"] = json!("hvs.PLANTED-token-4c9e1a");
    let e = t.refusal(WRITE_TOOL, &a).unwrap();
    assert!(!e.contains("PLANTED"), "{e}");
    assert!(!t.call(WRITE_TOOL, &a).unwrap_err().contains("PLANTED"));
    assert!(!t.describe(WRITE_TOOL, &a).contains("PLANTED"));
    assert!(!crate::sharedvault::Setting::path(&d).exists());
    assert!(t.call(WRITE_TOOL, &json!({"provider": "external"})).is_err());
    assert!(t.call(WRITE_TOOL, &json!({"provider": "kms"})).is_err());
}
