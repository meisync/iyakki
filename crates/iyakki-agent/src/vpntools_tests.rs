//! The assistant's VPN tools: the list is free, every change asks every time, no key passes through the assistant, the file is read by
//! the tool through the file scope, and nothing is echoed. The system layer is a fake; every key is FAKE.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{hard_ask, Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::gitkeys::SecretStore;
use crate::vpn_mgr_tests::{conf, key, rig, Rig};
use crate::vpntools::*;

/// A stand-in for the base tools: `read_file` inside one folder only (the real one checks the person's file scope).
struct FileTools {
    root: std::path::PathBuf,
}

impl Tools for FileTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![ToolDef { spec: ToolSpec { name: "read_file".into(), description: "".into(), schema: json!({}) }, risk: Risk::Safe, untrusted: true }]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let p = std::path::Path::new(args["path"].as_str().unwrap_or(""));
        if name != "read_file" || !p.starts_with(&self.root) {
            return Err(format!("no tool {name} for that path"));
        }
        std::fs::read_to_string(p).map_err(|e| e.to_string())
    }
    fn describe(&self, name: &str, _args: &Value) -> String {
        name.to_string()
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        let p = std::path::Path::new(args["path"].as_str().unwrap_or(""));
        (name == "read_file" && !p.starts_with(&self.root)).then(|| "that path is outside the folders you may read".to_string())
    }
}

fn tools(r: &Rig) -> VpnTools {
    VpnTools::new(Arc::new(FileTools { root: r.dir.clone() }), r.mgr.clone())
}

fn write_conf(r: &Rig, name: &str, text: &str) -> String {
    let p = r.dir.join(name);
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn the_list_is_free_and_every_change_asks_every_time() {
    let r = rig("tools-ask");
    let t = tools(&r);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk("vpn_list"), Risk::Safe);
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(hard_ask(n, &json!({})).is_some(), "{n} must be on the hard-ask list");
        assert!(!t.pre_approved(n) && !t.approved_always(n));
        assert!(t.grant_blocker(n, &json!({})).is_some(), "Allow for this session must not remember {n}");
    }
    assert!(hard_ask("vpn_list", &json!({})).is_none());
    assert!(defs.iter().any(|d| d.spec.name == "read_file"), "the inner tools stay");
    assert!(t.call("vpn_list", &json!({})).unwrap().contains("No VPN profiles"));
}

#[test]
fn add_reads_the_file_itself_shows_a_card_and_keeps_the_keys_out_of_every_output() {
    let r = rig("tools-add");
    let t = tools(&r);
    let path = write_conf(&r, "office.conf", &conf("0.0.0.0/0", "vpn.example.com:51820"));
    let args = json!({"name": "Office", "path": path});
    let card = t.preview("vpn_add", &args).unwrap();
    assert!(card.contains("Office") && card.contains("starts disabled") && card.contains("wireguard") && card.contains("vpn.example.com:51820"), "{card}");
    assert!(t.refusal("vpn_add", &args).is_none());
    assert!(r.mgr.profiles().is_empty(), "a card writes nothing");
    let out = t.call("vpn_add", &args).unwrap();
    assert!(out.contains("Imported") && out.contains("disabled"));
    let listed = t.call("vpn_list", &json!({})).unwrap();
    for k in [key(0x11), key(0x33)] {
        assert!(![&card, &out, &listed, &t.describe("vpn_add", &args)].iter().any(|s| s.contains(&k)), "a key leaked");
    }
    assert!(listed.contains("Office") && listed.contains("disabled") && listed.contains("FULL"));
    assert!(r.mgr.is_enabled("v1") == false);
}

#[test]
fn add_refuses_key_text_paths_outside_the_scope_and_bad_files_without_echoing() {
    let r = rig("tools-refuse");
    let t = tools(&r);
    // the whole profile in an argument
    let text = conf("0.0.0.0/0", "vpn.example.com:51820");
    let e = t.refusal("vpn_add", &json!({"name": "x", "path": text})).unwrap();
    assert!(e.contains("Never send") && !e.contains(&key(0x11)));
    assert!(t.call("vpn_add", &json!({"name": "x", "path": text})).is_err());
    assert!(t.refusal("vpn_add", &json!({"name": "x", "path": "-----BEGIN PRIVATE KEY-----"})).is_some());
    assert!(t.refusal("vpn_add", &json!({"name": "x"})).unwrap().contains("path"));
    // outside the folders the person may read
    let e = t.refusal("vpn_add", &json!({"name": "x", "path": "/etc/shadow"})).unwrap();
    assert!(e.contains("outside"));
    assert!(t.call("vpn_add", &json!({"name": "x", "path": "/etc/shadow"})).is_err());
    // a file with a script directive: the reason names the directive, not the value
    let bad = conf("0.0.0.0/0", "vpn.example.com:51820").replace("[Peer]", "PostUp = /bin/sh -c evil\n[Peer]");
    let path = write_conf(&r, "bad.conf", &bad);
    let e = t.refusal("vpn_add", &json!({"name": "Bad", "path": path})).unwrap();
    assert!(e.contains("PostUp") && !e.contains("evil") && !e.contains(&key(0x11)));
    assert!(r.vault.names().unwrap().is_empty() && r.mgr.profiles().is_empty());
}

#[test]
fn enable_disable_and_delete_show_what_changes_and_do_it() {
    let r = rig("tools-life");
    let t = tools(&r);
    let path = write_conf(&r, "office.conf", &conf("0.0.0.0/0", "vpn.example.com:51820"));
    t.call("vpn_add", &json!({"name": "Office", "path": path})).unwrap();
    let enable = json!({"profile": "office"});
    let card = t.preview("vpn_enable", &enable).unwrap();
    assert!(card.contains("changes this device's routes") && card.contains("all traffic"), "{card}");
    let out = t.call("vpn_enable", &enable).unwrap();
    assert!(out.contains("Enabled"), "{out}");
    assert!(r.sys.st.lock().unwrap().links.contains("iyvpn0"));
    let dis = t.preview("vpn_disable", &json!({"profile": "v1"})).unwrap();
    assert!(dis.contains("removed") && dis.contains("kept"));
    t.call("vpn_disable", &json!({"profile": "v1"})).unwrap();
    assert!(r.sys.clean());
    let del = t.preview("vpn_delete", &json!({"profile": "Office"})).unwrap();
    assert!(del.starts_with("DELETE") && del.contains("cannot be undone"));
    assert!(t.call("vpn_delete", &json!({"profile": "Office"})).unwrap().contains("Deleted"));
    assert!(r.vault.names().unwrap().is_empty());
    // unknown profile, and the audit log has the actions without any key
    assert!(t.refusal("vpn_enable", &json!({"profile": "nope"})).unwrap().contains("no such"));
    assert!(t.refusal("vpn_enable", &json!({})).unwrap().contains("name the VPN profile"));
    let log = r.log.lock().unwrap().join("\n");
    assert!(log.contains("vpn.enable Office") && log.contains("vpn.delete Office") && !log.contains(&key(0x11)));
}

#[test]
fn describe_is_one_line_and_other_tools_pass_through() {
    let r = rig("tools-desc");
    let t = tools(&r);
    assert_eq!(t.describe("vpn_list", &json!({})), "List the VPN profiles");
    assert!(t.describe("vpn_enable", &json!({"profile": "nope"})).contains("invalid"));
    assert_eq!(t.describe("read_file", &json!({"path": "x"})), "read_file");
    assert!(is_vpn_tool("vpn_add") && !is_vpn_tool("ssh_tunnel_add"));
    assert!(PROMPT_RULE.contains("no VPN service"));
}
