//! The assistant's SSH tunnel tools: the list is free, every change asks every time, no key or password passes through the assistant,
//! and a new host key is never trusted by a tool. The client is a fake; nothing here touches a network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{hard_ask, Risk, Tools};
use crate::gitkeys::{GitKeys, MemStore, Session};
use crate::ssh::HostKey;
use crate::sshtunnel::Ports;
use crate::sshtunnel_mgr::{FakePorts, FakeRunner, Manager};
use crate::sshtunneltools::*;

struct Rig {
    tools: SshTunnelTools,
    mgr: Arc<Manager>,
    keys: Arc<GitKeys>,
    store: Arc<MemStore>,
    log: Arc<Mutex<Vec<String>>>,
    shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>>,
}

const TOKEN: &str = "tok_FAKE_planted_9f3a77";

fn rig(tag: &str) -> Rig {
    let dir = iyakki_sec_free_dir(tag);
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let audit: crate::gitkeys::AuditFn = Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}")));
    let keys = Arc::new(GitKeys::new(&dir, store.clone(), audit.clone(), Arc::new(Session::default())));
    keys.generate("work", "fake@example.invalid").unwrap();
    store.map.lock().unwrap().insert("git.token.example.invalid".into(), TOKEN.into());
    let shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>> = Arc::new(Mutex::new(HashMap::new()));
    let s2 = shown.clone();
    let probe: crate::gitjobs::Probe = Arc::new(move |h, p| s2.lock().unwrap().get(&(h.to_string(), p)).cloned().ok_or_else(|| format!("{h}:{p} sent no host key")));
    let ports: Arc<dyn Ports> = Arc::new(FakePorts::default());
    let mgr = Arc::new(Manager::new(&dir, keys.clone(), probe, Arc::new(FakeRunner::default()), ports, audit).with_pause(Arc::new(|_| {})));
    let tools = SshTunnelTools::new(Arc::new(crate::assign::tests_support::NoTools), mgr.clone());
    Rig { tools, mgr, keys, store, log, shown }
}

fn iyakki_sec_free_dir(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = std::path::Path::new(&base).join(format!("tt-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn add_args() -> serde_json::Value {
    json!({"name": "Wiki", "host": "bastion.example.invalid", "user": "alice", "key": "work", "kind": "local", "target_host": "wiki.example.invalid", "target_port": 8080, "local_port": 8123})
}

fn host_key() -> HostKey {
    HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: crate::story::base64_encode(b"fake-host-key-tools") }
}

impl Rig {
    /// Everything the assistant could see or the person's log could hold.
    fn assert_no_secret(&self, seen: &[String]) {
        let private = self.store.map.lock().unwrap().get("ssh.work").unwrap().clone();
        let mut all = seen.join("\n");
        all.push_str(&self.log.lock().unwrap().join("\n"));
        for line in private.lines().filter(|l| l.len() > 20) {
            assert!(!all.contains(line), "key material leaked");
        }
        assert!(!all.contains(TOKEN), "token leaked");
    }
}

#[test]
fn only_the_list_is_free_and_every_change_is_on_the_hard_ask_list() {
    let r = rig("risk");
    let defs = r.tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing")).risk;
    assert_eq!(risk(READ_TOOL), Risk::Safe);
    assert!(hard_ask(READ_TOOL, &json!({})).is_none(), "the read-only tool needs no card");
    for t in WRITE_TOOLS {
        assert_eq!(risk(t), Risk::Ask, "{t}");
        assert!(hard_ask(t, &json!({})).is_some(), "{t} asks even in Auto mode");
        assert!(!r.tools.approved_always(t), "{t}: Allow for this session does not remember it");
        assert!(r.tools.grant_blocker(t, &json!({})).is_some(), "{t}");
        assert!(!r.tools.pre_approved(t), "{t}");
    }
    assert!(r.tools.grant_blocker(READ_TOOL, &json!({})).is_none());
    assert_eq!(crate::toolblock::group_of("ssh_tunnel_add"), crate::toolblock::Group::System);
}

#[test]
fn the_card_says_in_words_what_will_change() {
    let r = rig("card");
    let c = r.tools.preview("ssh_tunnel_add", &add_args()).unwrap();
    for w in ["Wiki", "bastion.example.invalid:22", "user alice", "Jump hosts before it: none", "wiki.example.invalid:8080", "127.0.0.1:8123", "vault entry ssh.work", "starts disabled"] {
        assert!(c.contains(w), "{w} in {c}");
    }
    let withj = json!({"name": "Wiki", "host": "bastion.example.invalid", "user": "alice", "key": "work", "kind": "socks", "jump": ["bob@gate.example.invalid:2222"]});
    let c = r.tools.preview("ssh_tunnel_add", &withj).unwrap();
    assert!(c.contains("bob@gate.example.invalid:2222") && c.contains("SOCKS") && c.contains("a free port is chosen"), "{c}");
    // delete is stronger
    r.tools.call("ssh_tunnel_add", &add_args()).unwrap();
    let c = r.tools.preview("ssh_tunnel_delete", &json!({"tunnel": "Wiki"})).unwrap();
    assert!(c.contains("DELETE") && c.contains("known hosts") && c.contains("cannot be undone") && c.contains("stays in your vault"), "{c}");
    assert!(r.tools.describe("ssh_tunnel_delete", &json!({"tunnel": "t1"})).starts_with("DELETE"));
}

#[test]
fn refusals_are_plain_and_never_echo_what_was_sent() {
    let r = rig("refuse");
    let bad = |a: serde_json::Value| r.tools.refusal("ssh_tunnel_add", &a).unwrap();
    let mut a = add_args();
    a["host"] = json!("-oProxyCommand=id");
    assert!(bad(a).contains("not a host name"));
    let mut a = add_args();
    a["key"] = json!("ghost");
    let m = bad(a);
    assert!(m.contains("no key called ghost") && m.contains("Ask the person"), "{m}");
    // key text instead of a name: refused, and the text is not repeated
    let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAAFAKEFAKEFAKE\n-----END OPENSSH PRIVATE KEY-----";
    let mut a = add_args();
    a["key"] = json!(pem);
    let m = bad(a);
    assert!(m.contains("Never send keys") && !m.contains("FAKEFAKE"), "{m}");
    let mut a = add_args();
    a["name"] = json!(TOKEN.repeat(20));
    assert!(!bad(a).contains(TOKEN));
    assert!(r.tools.refusal("ssh_tunnel_enable", &json!({"tunnel": "nope"})).unwrap().contains("no such tunnel"));
    assert!(r.tools.refusal("ssh_tunnel_delete", &json!({})).unwrap().contains("name the tunnel"));
    let mut a = add_args();
    a["kind"] = json!("remote");
    assert!(bad(a).contains("`local`"));
    assert!(r.tools.refusal("ssh_tunnel_add", &add_args()).is_none());
    assert!(r.mgr.tunnels().is_empty(), "a refusal changes nothing");
}

#[test]
fn a_new_host_key_is_shown_but_never_trusted_by_a_tool() {
    let r = rig("hostkey");
    let k = host_key();
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    let out = r.tools.call("ssh_tunnel_add", &add_args()).unwrap();
    assert!(out.contains(&k.fingerprint()) && out.contains("must confirm") && out.contains("disabled"), "{out}");
    assert!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).is_empty(), "nothing was trusted");
    let e = r.tools.call("ssh_tunnel_enable", &json!({"tunnel": "Wiki"})).unwrap_err();
    assert!(e.contains("Not enabled") && e.contains(&k.fingerprint()), "{e}");
    assert!(!r.mgr.get("t1").unwrap().enabled);
    // the person trusts it (their own action), then the tool may enable
    r.mgr.trust("bastion.example.invalid", 22, &k.fingerprint()).unwrap();
    assert!(r.tools.call("ssh_tunnel_enable", &json!({"tunnel": "t1"})).unwrap().contains("Enabled"));
    assert!(r.mgr.get("t1").unwrap().enabled);
    // a key that changed is a refusal in words
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![HostKey { b64: crate::story::base64_encode(b"another"), ..k }]);
    assert!(r.tools.call("ssh_tunnel_enable", &json!({"tunnel": "t1"})).unwrap_err().contains("CHANGED"));
}

#[test]
fn add_edit_disable_and_delete_work_through_the_managers_own_checks() {
    let r = rig("flow");
    let k = host_key();
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    r.tools.call("ssh_tunnel_add", &add_args()).unwrap();
    let t = r.mgr.get("t1").unwrap();
    assert_eq!((t.name.as_str(), t.local_port, t.enabled, t.key.as_str()), ("Wiki", 8123, false, "work"));
    // a clash is the manager's refusal, passed on
    let mut dup = add_args();
    dup["name"] = json!("Other");
    assert!(r.tools.refusal("ssh_tunnel_add", &dup).is_none());
    assert!(r.tools.call("ssh_tunnel_add", &dup).unwrap_err().contains("already used by the tunnel Wiki"));
    // edit changes only what is given
    r.tools.call("ssh_tunnel_edit", &json!({"tunnel": "t1", "target_port": 9090})).unwrap();
    let t = r.mgr.get("t1").unwrap();
    assert_eq!((t.target_port, t.target_host.as_str(), t.user.as_str()), (9090, "wiki.example.invalid", "alice"));
    let c = r.tools.preview("ssh_tunnel_edit", &json!({"tunnel": "t1", "kind": "socks"})).unwrap();
    assert!(c.contains("SOCKS") && c.contains("Wiki"));
    r.mgr.trust("bastion.example.invalid", 22, &k.fingerprint()).unwrap();
    r.tools.call("ssh_tunnel_enable", &json!({"tunnel": "t1"})).unwrap();
    assert!(r.tools.call("ssh_tunnel_disable", &json!({"tunnel": "Wiki"})).unwrap().contains("Disabled"));
    assert!(!r.mgr.get("t1").unwrap().enabled);
    let listed = r.tools.call(READ_TOOL, &json!({})).unwrap();
    assert!(listed.contains("Wiki") && listed.contains("127.0.0.1:8123") && listed.contains("disabled") && listed.contains("key work"), "{listed}");
    assert!(r.tools.call("ssh_tunnel_delete", &json!({"tunnel": "t1"})).unwrap().contains("Deleted"));
    assert_eq!(r.tools.call(READ_TOOL, &json!({})).unwrap(), "No SSH tunnels yet.");
    assert!(r.keys.key("work").is_some(), "the person's own key stays");
}

#[test]
fn no_key_or_token_ever_reaches_a_result_the_card_or_the_log() {
    let r = rig("leak");
    let k = host_key();
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    let mut seen = vec![];
    for (n, a) in [("ssh_tunnel_add", add_args()), ("ssh_tunnel_edit", json!({"tunnel": "t1", "name": "Wiki two"}))] {
        seen.push(r.tools.describe(n, &a));
        seen.push(r.tools.preview(n, &a).unwrap());
        seen.push(r.tools.call(n, &a).unwrap());
    }
    r.mgr.trust("bastion.example.invalid", 22, &k.fingerprint()).unwrap();
    seen.push(r.tools.call(READ_TOOL, &json!({})).unwrap());
    seen.push(r.tools.call("ssh_tunnel_enable", &json!({"tunnel": "t1"})).unwrap());
    seen.push(r.tools.preview("ssh_tunnel_delete", &json!({"tunnel": "t1"})).unwrap());
    seen.push(r.tools.call("ssh_tunnel_delete", &json!({"tunnel": "t1"})).unwrap());
    seen.push(r.tools.refusal("ssh_tunnel_add", &json!({"name": "x", "host": "h", "user": "u", "key": TOKEN})).unwrap_or_default());
    r.assert_no_secret(&seen);
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("sshtunnel.save ")), "the audit lines carry names");
}
