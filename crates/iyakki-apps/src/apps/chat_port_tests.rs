//! The chat port: the real one answers from the shell's context and writes to its audit log; the fake keeps the same promises in memory.

use std::sync::{Arc, Mutex};

use iyakki_core_fake::FakeCore;
use iyakki_sec::policy::Role;

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn fake(tag: &str) -> FakeChatPort {
    let dir = temp_dir(tag);
    let store = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    FakeChatPort::new("ada", Role::Admin.template(), &dir, Arc::new(FakeCore::new()), SettingsRef::of_store(&store))
}

#[test]
fn the_real_port_answers_from_the_context_and_audits_for_the_signed_in_person() {
    let cx = cx_with_admin_store("chatport-real");
    let p = CxChat(&cx);
    assert_eq!(p.user(), "root");
    assert!(p.policy().manage_settings);
    assert_eq!(p.data_dir(), cx.data_dir());
    p.audit("chat.port.test", "one");
    let seen = p.audit_recent(5);
    assert!(seen.iter().any(|l| l.contains("chat.port.test") && l.contains("root")), "{seen:?}");
    assert!(p.now_secs() > 1_600_000_000);
}

#[test]
fn the_fake_records_audit_lines_newest_last_and_cuts_to_n() {
    let p = fake("chatport-fake-audit");
    p.audit("a.one", "x");
    p.audit("a.two", "y");
    (p.audit_sink())("a.three", "z");
    assert_eq!(p.audited().len(), 3);
    assert_eq!(p.audited()[0], ("ada".to_string(), "a.one".to_string(), "x".to_string()));
    let last = p.audit_recent(2);
    assert_eq!(last.len(), 2);
    assert!(last[0].contains("a.two") && last[1].contains("a.three"));
    assert!(p.audit_recent(0).is_empty());
}

#[test]
fn the_fake_clock_moves_only_when_told() {
    let p = fake("chatport-fake-clock");
    let t0 = p.now_secs();
    assert_eq!(p.now_secs(), t0, "no time passes by itself");
    p.advance(90);
    assert_eq!(p.now_secs(), t0 + 90);
}

#[test]
fn the_fake_hands_out_the_core_and_settings_it_was_given() {
    let p = fake("chatport-fake-core");
    assert!(Arc::ptr_eq(&p.core(), &p.core.clone()));
    assert_eq!(p.user(), "ada");
    assert!(p.policy().manage_settings);
}

struct NoTools;
impl iyakki_shared::agent::Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
        name.to_string()
    }
}

/// A chat tool file runs against the fake port alone: no `AppCx`, no services, no real audit log.
#[test]
fn a_chat_tool_file_runs_on_the_fake_port_without_a_shell_context() {
    use iyakki_shared::agent::Tools;
    use iyakki_shared::settingstools::SettingsTools;
    let core = Arc::new(FakeCore::new());
    core.monitors_fake().set_screens(&[("DEL-A0C5-1", "DELL U2720Q", 40)]);
    let dir = temp_dir("chatport-tools");
    let store = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    let port = FakeChatPort::new("ada", Role::Admin.template(), &dir, core.clone(), SettingsRef::of_store(&store));
    let env = iyakki_agent::settingstools::Env { dir: dir.clone(), user: "ada".into(), admin: true, settings: port.settings(), audit: Arc::new(|_, _| {}) };
    let t = SettingsTools::new(Arc::new(NoTools), true, crate::apps::chat_monitor::entries(&port, &env));
    let said = t.call("monitor_brightness", &serde_json::json!({})).unwrap();
    assert!(said.contains("DEL-A0C5-1") && said.contains("40%"), "{said}");
    t.call("monitor_choose", &serde_json::json!({"id": "DEL-A0C5-1"})).unwrap();
    t.call("monitor_brightness_set", &serde_json::json!({"percent": 25})).unwrap();
    assert_eq!(core.monitors_fake().percent_of("DEL-A0C5-1"), Some(25));
}

/// A saved-chat store in memory, so the chat's session code runs on the fake port.
struct MemSessions {
    all: Mutex<Vec<iyakki_shared::sessions::Session>>,
    clock_ms: u64,
}

impl iyakki_agent_api::SessionApi for MemSessions {
    fn create(&self, user: &str) -> Result<iyakki_shared::sessions::Session, String> {
        let mut all = self.all.lock().unwrap();
        let s = iyakki_shared::sessions::Session { id: format!("s{}", all.len() + 1), owner: user.into(), title: String::new(), created: self.clock_ms, updated: self.clock_ms, messages: vec![], stamps: vec![], summary: String::new() };
        all.push(s.clone());
        Ok(s)
    }
    fn list(&self, user: &str) -> Vec<iyakki_shared::sessions::SessionMeta> {
        self.all.lock().unwrap().iter().filter(|s| s.owner == user).map(|s| iyakki_shared::sessions::SessionMeta { id: s.id.clone(), title: s.title.clone(), updated: s.updated, messages: s.messages.len() }).collect()
    }
    fn load(&self, user: &str, id: &str) -> Option<iyakki_shared::sessions::Session> {
        self.all.lock().unwrap().iter().find(|s| s.owner == user && s.id == id).cloned()
    }
    fn save(&self, s: &mut iyakki_shared::sessions::Session) -> Result<(), String> {
        let mut all = self.all.lock().unwrap();
        match all.iter_mut().find(|x| x.id == s.id) {
            Some(x) => *x = s.clone(),
            None => all.push(s.clone()),
        }
        Ok(())
    }
    fn remember_turn(&self, _s: &iyakki_shared::sessions::Session, _q: &str, _a: &str) {}
}

#[test]
fn the_fake_port_serves_the_persons_chats_and_their_working_folders() {
    let p = fake("chatport-sessions").with_sessions(Arc::new(MemSessions { all: Mutex::new(vec![]), clock_ms: 5 }));
    let sessions = p.sessions();
    let mut s = sessions.create(p.user()).unwrap();
    s.title = "first".into();
    sessions.save(&mut s).unwrap();
    assert_eq!(sessions.list(p.user()).len(), 1);
    assert!(sessions.list("someone-else").is_empty(), "another person's chats are not listed");
    let dir = p.session_workdir(&s.id).unwrap();
    assert_eq!(p.session_files(&s.id), 0);
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    assert_eq!(p.session_files(&s.id), 1);
}

#[test]
fn the_real_port_reads_the_same_chat_store_the_context_holds() {
    let cx = cx_with_admin_store("chatport-real-sessions");
    let p = CxChat(&cx);
    let mut made = p.sessions().create(p.user()).unwrap();
    made.messages.push(iyakki_shared::chat::Message::user("hello".to_string()));
    p.sessions().save(&mut made).unwrap();
    assert_eq!(p.sessions().list("root"), cx.sessions().list("root"), "the same store, not a copy");
    assert_eq!(cx.sessions().load("root", &made.id).map(|s| s.id), Some(made.id.clone()));
    assert!(p.session_workdir(&made.id).is_ok());
    assert_eq!(p.voice_settings(), cx.voice_settings());
}

#[test]
#[should_panic(expected = "give it a vault")]
fn a_fake_port_never_falls_back_to_the_real_vault() {
    let p = fake("chatport-no-vault");
    let _ = p.vault();
}

#[test]
fn a_shared_port_moves_to_another_thread_and_audits_for_the_same_person() {
    let cx = cx_with_admin_store("chatport-shared");
    let shared = CxChat(&cx).shared();
    let t = std::thread::spawn({
        let p = shared.clone();
        move || {
            p.audit("chat.port.shared", "thread");
            (p.user().to_string(), p.data_root().to_path_buf())
        }
    });
    let (user, dir) = t.join().unwrap();
    assert_eq!((user.as_str(), dir.as_path()), ("root", cx.data_dir()));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("chat.port.shared") && l.contains("root")));
}

#[test]
fn a_shared_fake_shares_its_audit_list_and_clock_with_the_test() {
    let p = fake("chatport-shared-fake");
    let shared = p.shared();
    shared.audit("a.from.run", "x");
    assert_eq!(p.audited().len(), 1, "what a spawned run audits shows in the test's own handle");
    p.advance(30);
    assert_eq!(shared.now_secs(), p.now_secs());
}

#[test]
fn the_store_view_of_a_port_gives_the_person_folder_policy_and_audit_and_a_fake_agent_must_be_given() {
    let p = fake("chatport-userdata");
    let u: &dyn UserData = &p;
    assert_eq!(u.acting_user(), "ada");
    assert_eq!(u.data_root(), p.dir.as_path());
    assert!(u.acting_policy().manage_settings);
    u.record_audit("a.store", "y");
    assert_eq!(p.audited()[0].1, "a.store");
    let missing = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.agent()));
    assert!(missing.is_err(), "a fake never falls back to the real facade");
}
