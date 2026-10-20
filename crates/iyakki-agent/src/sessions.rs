//! Chat sessions with memory.
//!
//! Each session has an id, an owner, and its full transcript on disk. Every completed exchange is
//! also indexed in the knowledge base under the private namespace `session/<id>`, so when a long
//! conversation outgrows the model's context window, earlier turns can be *recalled by relevance*
//! instead of being forgotten (see [`crate::context`]). Only the owner can read a session or recall
//! from its namespace.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::chat::{Message, Role};
use crate::kb::{Hit, KnowledgeBase};

pub const MAX_SESSIONS_PER_USER: usize = 200;
pub const MAX_MESSAGES: usize = 2000;
/// A chat opens showing the last three days, at most this many messages; older ones load as the person scrolls up.
pub const WINDOW_DAYS: u64 = 3;
pub const WINDOW_MESSAGES: usize = 1000;
/// An old chat still opens with at least its last few messages.
pub const WINDOW_MIN: usize = 10;
const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub owner: String,
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
    pub updated: u64,
    pub messages: Vec<Message>,
    /// When each message was added (milliseconds since the epoch), in step with `messages`.
    #[serde(default)]
    pub stamps: Vec<u64>,
    /// A few lines about the part of this chat that is no longer sent to the model word for word (rolling, rebuilt as the
    /// chat grows).
    #[serde(default)]
    pub summary: String,
}

impl Session {
    fn stamp(&self, i: usize) -> u64 {
        self.stamps.get(i).copied().unwrap_or(self.updated)
    }

    /// Index of the first message to show when the chat opens: the last `days` days, at most `max` messages, but never
    /// fewer than the last `WINDOW_MIN`.
    pub fn window_start(&self, now_ms: u64, days: u64, max: usize) -> usize {
        let n = self.messages.len();
        let cutoff = now_ms.saturating_sub(days * DAY_MS);
        let by_time = (0..n).find(|&i| self.stamp(i) >= cutoff).unwrap_or(n);
        let start = by_time.max(n.saturating_sub(max));
        if n - start < WINDOW_MIN { n.saturating_sub(WINDOW_MIN) } else { start }
    }

    /// Where the next older stretch begins when the person scrolls up from `shown_from`: another `days` days back,
    /// but at least 20 and at most 500 messages at a time.
    pub fn earlier_start(&self, shown_from: usize, days: u64) -> usize {
        if shown_from == 0 {
            return 0;
        }
        let cutoff = self.stamp(shown_from).saturating_sub(days * DAY_MS);
        let by_time = (0..shown_from).find(|&i| self.stamp(i) >= cutoff).unwrap_or(shown_from);
        by_time.min(shown_from.saturating_sub(20)).max(shown_from.saturating_sub(500))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub updated: u64,
    pub messages: usize,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn valid_session_id(id: &str) -> bool {
    id.len() == 12 && id.starts_with("s_") && id[2..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

pub fn namespace(id: &str) -> String {
    format!("session/{id}")
}

pub struct SessionStore {
    dir: PathBuf,
    kb: Arc<Mutex<KnowledgeBase>>,
}

impl SessionStore {
    pub fn new(dir: &Path, kb: Arc<Mutex<KnowledgeBase>>) -> SessionStore {
        SessionStore { dir: dir.into(), kb }
    }

    fn user_dir(&self, user: &str) -> Result<PathBuf, String> {
        if iyakki_sec::policy::valid_username(user) { Ok(self.dir.join(user)) } else { Err("invalid user".into()) }
    }

    pub fn create(&self, user: &str) -> Result<Session, String> {
        self.user_dir(user)?;
        let mut r = [0u8; 5];
        iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
        let id = format!("s_{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
        Ok(Session { id, owner: user.into(), title: "New chat".into(), created: now(), updated: now(), messages: vec![], stamps: vec![], summary: String::new() })
    }

    /// Save (creating the file on first save). The title follows the first user message.
    pub fn save(&self, s: &mut Session) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if !valid_session_id(&s.id) {
            return Err("invalid session id".into());
        }
        let d = self.user_dir(&s.owner)?;
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        if s.messages.len() > MAX_MESSAGES {
            return Err(format!("this chat is full (limit {MAX_MESSAGES} messages); start a new one"));
        }
        if s.title == "New chat" {
            if let Some(m) = s.messages.iter().find(|m| m.role == Role::User) {
                let t: String = m.content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(48).collect();
                if !t.is_empty() {
                    s.title = t;
                }
            }
        }
        // new messages are stamped now (older chats were stamped when they were loaded)
        let t = now();
        s.stamps.truncate(s.messages.len());
        while s.stamps.len() < s.messages.len() {
            s.stamps.push(t);
        }
        s.updated = t;
        let path = d.join(format!("{}.json", s.id));
        if !path.exists() && self.list(&s.owner).len() >= MAX_SESSIONS_PER_USER {
            return Err(format!("too many chats (limit {MAX_SESSIONS_PER_USER}); delete some first"));
        }
        let tmp = d.join(format!("{}.tmp", s.id));
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&serde_json::to_vec(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn list(&self, user: &str) -> Vec<SessionMeta> {
        let Ok(d) = self.user_dir(user) else { return vec![] };
        let mut v: Vec<SessionMeta> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice::<Session>(&std::fs::read(e.path()).ok()?).ok())
            .filter(|s| s.owner == user && valid_session_id(&s.id))
            .map(|s| SessionMeta { id: s.id, title: s.title, updated: s.updated, messages: s.messages.len() })
            .collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        v
    }

    /// Load a session; a file that is not owned by `user` is treated as nonexistent.
    pub fn load(&self, user: &str, id: &str) -> Option<Session> {
        if !valid_session_id(id) {
            return None;
        }
        let s: Session = serde_json::from_slice(&std::fs::read(self.user_dir(user).ok()?.join(format!("{id}.json"))).ok()?).ok()?;
        let mut s = s;
        // a chat saved before messages were stamped: its old messages count as written at its last update
        while s.stamps.len() < s.messages.len() {
            s.stamps.push(s.updated);
        }
        (s.owner == user && s.id == id).then_some(s)
    }

    pub fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        if self.load(user, id).is_none() {
            return Err("no such chat".into());
        }
        std::fs::remove_file(self.user_dir(user)?.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
        let _ = self.kb.lock().unwrap().drop_namespace(&namespace(id));
        Ok(())
    }

    /// Index one finished exchange so it can be recalled later.
    pub fn remember_turn(&self, s: &Session, question: &str, answer: &str) {
        if answer.trim().is_empty() || !valid_session_id(&s.id) {
            return;
        }
        let text = format!("You asked: {question}\nAssistant answered: {answer}");
        let mut kb = self.kb.lock().unwrap();
        let _ = kb.add_document(&namespace(&s.id), &s.title, &format!("earlier in this chat"), &text, Default::default());
    }

    /// Recall relevant earlier turns of `user`'s own session `id` (nothing for anyone else's).
    pub fn recall(&self, user: &str, id: &str, query: &str, k: usize) -> Vec<Hit> {
        if self.load(user, id).is_none() || query.trim().is_empty() || k == 0 {
            return vec![];
        }
        self.kb.lock().unwrap().search_weighted(&[(namespace(id).as_str(), 1.0)], query, k)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_chat_opens_on_its_last_three_days_and_up_to_a_thousand_messages_and_older_ones_load_in_steps() {
        let day = 86_400_000u64;
        let now = 100 * day;
        let mk = |stamps: Vec<u64>| Session { id: "s_0123456789".into(), owner: "a".into(), title: "t".into(), created: 0, updated: now, messages: stamps.iter().map(|_| Message::user("m")).collect(), stamps, summary: String::new() };
        // 100 messages a day for ten days
        let s = mk((0..1000u64).map(|i| now - 10 * day + i * day / 100).collect());
        let start = s.window_start(now, 3, 1000);
        assert_eq!(start, 700, "three days of 100 a day");
        assert_eq!(s.window_start(now, 3, 150), 850, "capped at the newest 150");
        // scrolling up brings the three days before, at most 500 at a time
        let e = s.earlier_start(start, 3);
        assert_eq!(e, 400);
        assert_eq!(s.earlier_start(e, 3), 100);
        assert_eq!(s.earlier_start(100, 3), 0);
        assert_eq!(s.earlier_start(0, 3), 0);
        // a chat idle for a month still opens with its last messages
        let old = mk((0..40u64).map(|i| i * 1000).collect());
        assert_eq!(old.window_start(now, 3, 1000), 30);
        // short chats show everything; a sparse chat steps back at least 20
        assert_eq!(mk(vec![now - 1, now]).window_start(now, 3, 1000), 0);
        let sparse = mk((0..100u64).map(|i| now - (100 - i) * day / 100 * 50).collect());
        assert!(sparse.earlier_start(80, 3) <= 60);
        assert_eq!(mk(vec![]).window_start(now, 3, 1000), 0);
    }

    #[test]
    fn saving_stamps_new_messages_and_loading_stamps_old_chats_with_their_last_update() {
        let d = std::env::temp_dir().join(format!("iyakki-stamps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let kb = Arc::new(Mutex::new(KnowledgeBase::open(&d.join("kb"), Arc::new(crate::embed::HashEmbed::default()))));
        let st = SessionStore::new(&d.join("s"), kb);
        let mut s = st.create("amy").unwrap();
        s.messages.push(Message::user("first"));
        st.save(&mut s).unwrap();
        assert_eq!(s.stamps.len(), 1);
        let t0 = s.stamps[0];
        s.messages.push(Message::assistant("second".to_string()));
        st.save(&mut s).unwrap();
        assert_eq!(s.stamps.len(), 2);
        assert_eq!(st.load("amy", &s.id).unwrap().stamps, s.stamps, "the stamps are persisted");
        assert_eq!(s.stamps[0], t0, "an old message keeps its stamp");
        // a file written before stamps existed
        let path = d.join("s/amy").join(format!("{}.json", s.id));
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("stamps");
        v["updated"] = serde_json::json!(12345);
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(st.load("amy", &s.id).unwrap().stamps, vec![12345, 12345]);
    }

    use super::*;
    use crate::embed::HashEmbed;

    fn store(tag: &str) -> SessionStore {
        let d = std::env::temp_dir().join(format!("iyakki-sessions-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        SessionStore::new(&d, Arc::new(Mutex::new(KnowledgeBase::in_memory(Arc::new(HashEmbed::default())))))
    }

    #[test]
    fn create_save_list_load_delete() {
        let st = store("crud");
        let mut s = st.create("amy").unwrap();
        assert!(valid_session_id(&s.id) && s.title == "New chat");
        assert!(st.list("amy").is_empty(), "nothing is stored until the first save");
        s.messages.push(Message::user("  How do I   rotate the deploy key?  "));
        s.messages.push(Message::assistant("Use the vault CLI."));
        st.save(&mut s).unwrap();
        assert_eq!(s.title, "How do I rotate the deploy key?", "title follows the first user message");
        let l = st.list("amy");
        assert_eq!((l.len(), l[0].messages, l[0].title.as_str()), (1, 2, "How do I rotate the deploy key?"));
        let back = st.load("amy", &s.id).unwrap();
        assert_eq!(back.messages, s.messages);
        // tool-call messages round-trip too
        let mut t = st.create("amy").unwrap();
        let call = crate::chat::ToolCall { id: "c1".into(), name: "read_file".into(), args: serde_json::json!({"path": "/x"}) };
        t.messages = vec![Message::user("read x"), Message::assistant_calls("", vec![call.clone()]), Message::tool_result(&call, "data"), Message::assistant("done")];
        st.save(&mut t).unwrap();
        assert_eq!(st.load("amy", &t.id).unwrap().messages, t.messages);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(st.dir.join("amy").join(format!("{}.json", s.id))).unwrap().permissions().mode() & 0o777, 0o600);
        st.delete("amy", &s.id).unwrap();
        assert!(st.load("amy", &s.id).is_none() && st.delete("amy", &s.id).is_err());
        assert_eq!(st.list("amy").len(), 1);
    }

    #[test]
    fn sessions_are_private_and_ids_are_validated() {
        let st = store("private");
        let mut s = st.create("amy").unwrap();
        s.messages.push(Message::user("my secret plan"));
        st.save(&mut s).unwrap();
        assert!(st.load("bob", &s.id).is_none() && st.list("bob").is_empty());
        // copying amy's file into bob's folder doesn't make it his
        std::fs::create_dir_all(st.dir.join("bob")).unwrap();
        std::fs::copy(st.dir.join("amy").join(format!("{}.json", s.id)), st.dir.join("bob").join(format!("{}.json", s.id))).unwrap();
        assert!(st.load("bob", &s.id).is_none(), "owner field must match");
        assert!(st.list("bob").is_empty());
        assert!(st.delete("bob", &s.id).is_err());
        for bad in ["../../x", "s_ZZZZZZZZZZ", "s_short", "", "s_0123456789ab/../x", "S_0123456789"] {
            assert!(st.load("amy", bad).is_none(), "{bad}");
        }
        assert!(st.create("../x").is_err());
        let mut forged = s.clone();
        forged.id = "../../etc/x".into();
        assert!(st.save(&mut forged).is_err());
    }

    #[test]
    fn memory_recalls_old_turns_for_the_owner_only() {
        let st = store("memory");
        let mut s = st.create("amy").unwrap();
        s.messages.push(Message::user("hello"));
        st.save(&mut s).unwrap();
        st.remember_turn(&s, "Where is the deploy key stored?", "It lives in the vault under prod/deploy.");
        st.remember_turn(&s, "What is for lunch?", "Sandwiches and soup.");
        let hits = st.recall("amy", &s.id, "which vault holds the deploy key", 2);
        assert!(hits[0].text.contains("vault under prod/deploy"), "{hits:?}");
        assert!(hits.iter().all(|h| h.ns == namespace(&s.id)));
        assert!(st.recall("bob", &s.id, "deploy key", 2).is_empty(), "another user recalls nothing from amy's chat");
        assert!(st.recall("amy", "s_00000000ff", "deploy key", 2).is_empty());
        // deleting the chat deletes its memory
        st.delete("amy", &s.id).unwrap();
        assert!(st.kb.lock().unwrap().stats(&namespace(&s.id)).is_none());
        assert!(st.recall("amy", &s.id, "deploy key", 2).is_empty());
        // blank answers aren't indexed; the same exchange twice is stored once
        let mut t = st.create("amy").unwrap();
        t.messages.push(Message::user("x"));
        st.save(&mut t).unwrap();
        st.remember_turn(&t, "q", "   ");
        st.remember_turn(&t, "q1", "a1");
        st.remember_turn(&t, "q1", "a1");
        assert_eq!(st.kb.lock().unwrap().stats(&namespace(&t.id)).unwrap().docs, 1);
    }

    #[test]
    fn limits_protect_the_disk() {
        let st = store("limits");
        let mut s = st.create("amy").unwrap();
        s.messages = (0..MAX_MESSAGES + 1).map(|i| Message::user(format!("m{i}"))).collect();
        assert!(st.save(&mut s).unwrap_err().contains("full"));
    }
}
