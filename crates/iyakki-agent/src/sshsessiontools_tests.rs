//! The assistant's SSH login tools: the list is free, the login asks every time and is never covered by a session grant, nothing is
//! asked that the tile would refuse, a host key is never trusted by a tool, and a request reaches the tile once.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{hard_ask, Risk, Tools};
use crate::ssh::{Connection, Env, HostKey};
use crate::sshsessiontools::*;

struct Rig {
    tools: SshSessionTools,
    env: Env,
    dir: std::path::PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

fn rig(tag: &str, full: bool, reach: bool) -> Rig {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let dir = std::path::Path::new(&base).join(format!("sst-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    // a stand-in `ssh` so a client is found
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\necho \"OpenSSH_9.6p1 Ubuntu-3, OpenSSL 3.0.13\" >&2\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let search = bin.display().to_string();
    let mk = {
        let (d, s) = (dir.clone(), search.clone());
        move || Env::new(&d.join("ssh-data"), "casey", &s, Arc::new(|_| Err("no vault here".into())))
    };
    let env = mk();
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let tools = SshSessionTools::new(Arc::new(crate::assign::tests_support::NoTools), Arc::new(mk), &dir, "casey", full, Arc::new(move |h| reach || h != "blocked.example.invalid"), Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}"))));
    Rig { tools, env, dir, log }
}

fn save(r: &Rig, name: &str, host: &str, trusted: bool) {
    r.env.connections().save(Connection { name: name.into(), host: host.into(), port: 22, user: "deploy".into(), key: "ssh.deploy".into(), command: String::new() }, None).unwrap();
    if trusted {
        r.env.known_hosts().trust(host, 22, &[HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: "AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H".into() }]).unwrap();
    }
}

#[test]
fn the_login_is_a_hard_ask_tool_and_the_list_is_free() {
    let r = rig("defs", true, true);
    let defs = r.tools.defs();
    let list = defs.iter().find(|d| d.spec.name == LIST_TOOL).unwrap();
    let login = defs.iter().find(|d| d.spec.name == LOGIN_TOOL).unwrap();
    assert_eq!(list.risk, Risk::Safe);
    assert_eq!(login.risk, Risk::Ask);
    assert!(hard_ask(LOGIN_TOOL, &json!({"connection": "box"})).is_some(), "asks every time, in Auto mode too");
    assert!(hard_ask(LIST_TOOL, &json!({})).is_none());
    assert!(r.tools.grant_blocker(LOGIN_TOOL, &json!({"connection": "box"})).is_some(), "no session grant covers it");
    assert!(!r.tools.pre_approved(LOGIN_TOOL) && !r.tools.approved_always(LOGIN_TOOL));
    assert!(login.spec.description.contains("cannot type in it"), "the model is told it cannot drive the session");
}

#[test]
fn the_list_shows_names_hosts_and_trust_but_no_secret() {
    let r = rig("list", true, true);
    assert!(r.tools.call(LIST_TOOL, &json!({})).unwrap().contains("No saved SSH connections"));
    save(&r, "box", "10.0.0.5", true);
    save(&r, "new", "10.0.0.6", false);
    let t = r.tools.call(LIST_TOOL, &json!({})).unwrap();
    assert!(t.contains("box · deploy@10.0.0.5:22 · key ssh.deploy · host key trusted"), "{t}");
    assert!(t.contains("new") && t.contains("NOT trusted yet"), "{t}");
    assert!(!t.contains("AAAAC3"), "no key material");
}

#[test]
fn a_login_is_checked_before_the_person_is_asked_and_queued_for_the_tile_once_approved() {
    let r = rig("login", true, true);
    save(&r, "box", "10.0.0.5", true);
    let args = json!({"connection": "box"});
    let card = r.tools.preview(LOGIN_TOOL, &args).unwrap();
    assert!(card.contains("“box”") && card.contains("deploy@10.0.0.5") && card.contains("cannot type in it") && card.contains("no password"), "{card}");
    assert!(r.tools.describe(LOGIN_TOOL, &args).starts_with("Open an interactive SSH login to “box”"));
    assert!(r.tools.refusal(LOGIN_TOOL, &args).is_none());
    assert!(!pending(&r.dir, "casey"), "nothing is queued before the call");
    let out = r.tools.call(LOGIN_TOOL, &args).unwrap();
    assert!(out.contains("cannot see or type"), "{out}");
    assert!(pending(&r.dir, "casey") && !pending(&r.dir, "someone-else"));
    r.tools.call(LOGIN_TOOL, &args).unwrap();
    assert_eq!(take_login(&r.dir, "casey").as_deref(), Some("box"));
    assert_eq!(take_login(&r.dir, "casey"), None, "asked twice, queued once");
    assert!(r.log.lock().unwrap().iter().any(|l| l == "ssh.login.request box"));
}

#[test]
fn what_the_tile_would_refuse_is_refused_here_with_the_reason() {
    let r = rig("refuse", true, true);
    save(&r, "fresh", "10.0.0.6", false);
    let host = r.tools.refusal(LOGIN_TOOL, &json!({"connection": "fresh"})).unwrap();
    assert!(host.contains("not trusted") && host.contains("press T") && host.contains("cannot trust"), "{host}");
    assert!(r.tools.call(LOGIN_TOOL, &json!({"connection": "fresh"})).is_err() && !pending(&r.dir, "casey"));
    assert!(r.tools.refusal(LOGIN_TOOL, &json!({"connection": "nope"})).unwrap().contains("no saved connection called nope"));
    assert!(r.tools.refusal(LOGIN_TOOL, &json!({})).unwrap().contains("name the connection"));
    // an account that may not reach the host
    let r2 = rig("reach", true, false);
    save(&r2, "bl", "blocked.example.invalid", true);
    assert!(r2.tools.refusal(LOGIN_TOOL, &json!({"connection": "bl"})).unwrap().contains("may not reach"));
    // an account without a full terminal
    let r3 = rig("term", false, true);
    save(&r3, "box", "10.0.0.5", true);
    assert!(r3.tools.refusal(LOGIN_TOOL, &json!({"connection": "box"})).unwrap().contains("full terminal"));
}
