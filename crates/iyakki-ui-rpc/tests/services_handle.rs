//! `RemoteServicesHandle` as a `ServicesHandle` (F-SPLIT-03 part 1, D-SPLIT-03-HANDLE): the generated files are fresh, the client and the core's
//! dispatcher agree on every method that crosses the socket, and a lost core never panics the UI and never answers a made-up success.
//! The real socket against the real services is `iyakki-compose/tests/remote_services.rs`.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::ServicesHandle;
use iyakki_ui_client::client::Connection;
use iyakki_ui_rpc::{RemoteServicesHandle, Rpc};

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The handle over a connection whose other end is already closed: every answer is the quiet value.
fn lost() -> Arc<dyn ServicesHandle> {
    Arc::new(RemoteServicesHandle::new(Rpc::new(Connection::new(Box::new(Cursor::new(Vec::new())), Box::new(std::io::sink())))))
}

/// The four generators write nothing under `--check` and fail when the committed file differs from what the trait gives now.
#[test]
fn the_generated_files_are_what_the_generators_write_now() {
    let py = std::process::Command::new("python3").arg("--version").output();
    if py.is_err() {
        eprintln!("python3 is not here: the freshness of the generated files is checked on the hosts that have it");
        return;
    }
    for script in ["gen_rpc.py", "gen_services.py", "gen_unserved.py", "gen_flows_jobs.py"] {
        let out = std::process::Command::new("python3").arg("-I").arg(repo().join("dist/split").join(script)).arg("--check").output().unwrap();
        assert!(out.status.success(), "{script} --check: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

fn names_between(text: &str, start: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find(start) {
        rest = &rest[i + start.len()..];
        if let Some(j) = rest.find('"') {
            out.insert(rest[..j].to_string());
        }
    }
    out
}

/// A method the client sends is one the dispatcher answers, and the other way round (apart from the three the client sends under another name
/// because the closure form becomes "read the whole value, write the whole value").
#[test]
fn every_call_the_client_sends_has_an_arm_in_the_dispatcher_and_the_reverse() {
    let t = std::fs::read_to_string(repo().join("crates/iyakki-ui-rpc/src/gen_services.rs")).unwrap();
    let (client, server) = t.split_once("pub(crate) fn serve_services").unwrap();
    let sent = names_between(client, "\"ServicesHandle\", 0, \"");
    let mut answered = BTreeSet::new();
    for l in server.lines() {
        let l = l.trim_start();
        if l.starts_with('"') && l.contains("=>") {
            answered.insert(l[1..l[1..].find('"').unwrap() + 1].to_string());
        }
    }
    let k_only: BTreeSet<String> = ["set_sound_settings", "set_system_sounds"].iter().map(|s| s.to_string()).collect();
    let sent_by_kept: BTreeSet<String> = {
        // the hand-written k_* methods in handle.rs send these names
        let h = std::fs::read_to_string(repo().join("crates/iyakki-ui-rpc/src/handle.rs")).unwrap();
        names_between(&h, "\"ServicesHandle\", 0, \"")
    };
    let mut all_sent = sent.clone();
    all_sent.extend(sent_by_kept);
    // `rule` sends its name through a variable: those two are the prompt rules
    all_sent.insert("cloud_stream_prompt_rule".into());
    all_sent.insert("fleet_prompt_rule".into());
    assert!(sent.len() > 100, "the generated client sends {} methods, the plain ones and the handle accessors of generated traits", sent.len());
    for n in &all_sent {
        assert!(answered.contains(n), "the client sends {n} but the dispatcher has no arm for it");
    }
    for n in &answered {
        assert!(all_sent.contains(n) || k_only.contains(n) || n == "other", "the dispatcher answers {n} but no client sends it");
    }
}

/// Nothing is served by a lost core, and the answer says so: no panic, no invented success, no secret, no open vault.
#[test]
fn a_lost_core_answers_the_quiet_value_of_every_kind_of_method() {
    let h = lost();
    assert!(!h.is_real(), "a lost core is not the real one (KI-1010)");
    // plain data: reads are empty or false, writes are errors that carry the reason
    assert!(!h.show_all_ai());
    assert!(h.system_slots().is_empty() && h.approvals("ann").is_empty() && h.pull_status().is_empty());
    assert!(h.set_show_all_ai(true).unwrap_err().contains("closed"));
    assert!(h.on_login("ann", "pw", false).user_vault != iyakki_shared::services::VaultState::Unlocked, "a failed sign-in call opens nothing");
    assert_eq!(h.web_access_port(), 0);
    // handle accessors of generated traits: remote objects whose calls answer quiet
    assert!(h.inbox().list("ann").is_empty() && h.calendar().events("ann").is_empty());
    assert!(h.mcp().list().is_empty());
    assert!(h.contract_local().settings().is_none());
    // kept answers: nothing was read, nothing is invented
    assert_eq!(h.dir(), Path::new(""));
    assert_eq!(h.cloud_stream_prompt_rule(), "");
    assert!(!h.secrets().system_unlocked(), "the plain secrets never cross: an empty, locked one");
    assert!(h.update_sound(|s| s.muted = true).is_err(), "a closure that changes a value writes the whole value back, and a lost core refuses it");
    // not served: the quiet object of the trait, never a success
    assert!(h.ai().provider_check("x").is_err(), "a served accessor answers a failed call with the quiet value of its answer");
    assert!(h.cue_out().ready().is_err());
    assert!(h.wake_spawn(None).is_err());
    assert!(h.agent_vault_token("ann", "x").is_none());
    assert!(h.streamer().is_none());
    h.set_streamer(Arc::new(NoStream));
}

/// The accessors that became served objects in part 2 (X3) answer quiet over a lost core: an empty list, an error, a number.
#[test]
fn the_accessors_added_in_part_two_are_quiet_over_a_lost_core() {
    let h = lost();
    assert!(h.sessions().list("ann").is_empty() && h.sessions().create("ann").is_err());
    assert!(h.skills().list("ann").is_empty() && h.runs().list("ann").is_empty() && h.jobs().list("ann").is_empty());
    assert_eq!(h.radio().levels(), [0.0; 28]);
    assert!(!h.radio().is_active());
    let dev = h.k3sdev();
    assert!(std::ptr::eq(dev.layout(), dev.layout()), "a borrowed answer is read once and kept for the life of the object");
    let dom = h.domains_facet("ann", Arc::new(|_, _| {}));
    assert!(!dom.status().enabled);
    assert!(dom.set_listener(Default::default()).is_err());
}

struct NoStream;
impl iyakki_proxy::Streamer for NoStream {
    fn open(&self, _: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        Err("none".into())
    }
}
