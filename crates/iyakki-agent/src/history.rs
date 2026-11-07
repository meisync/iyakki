//! Workspace history: what earlier sessions (and earlier steps of an Autopilot run) did, kept in the in-house knowledge base
//! so that it can be *retrieved* instead of being resent with every request.
//!
//! One namespace per person and workspace (`workspace/<person>/<workspace>`, stored by [`crate::kb`] as
//! `workspace__<person>__<workspace>.afkb`). Chunks are short (turn answers, capped tool-result digests, board decisions,
//! errors and rolling summaries) and tagged with kind, row id, session id and time. Credentials are redacted with
//! [`crate::credguard`] before anything is indexed, the oldest low-value chunks are pruned past a cap, and the whole
//! history can be forgotten per workspace or per session.

use std::sync::{Arc, Mutex};

use crate::kb::KnowledgeBase;

/// Longest text of one chunk (characters).
pub const MAX_CHUNK: usize = 1200;
/// Most documents kept per workspace before the oldest low-value ones are pruned.
pub const MAX_DOCS: usize = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Only what happened in the current session.
    Session,
    /// Everything in this workspace, from every session.
    Workspace,
}

impl Scope {
    pub fn parse(s: &str) -> Scope {
        if s.trim().eq_ignore_ascii_case("session") { Scope::Session } else { Scope::Workspace }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub id: String,
    /// turn, tool, summary, decision or error.
    pub kind: String,
    pub row: String,
    pub session: String,
    pub ts: i64,
    pub text: String,
}

#[derive(Clone)]
pub struct History {
    kb: Arc<Mutex<KnowledgeBase>>,
    ns: String,
    session: String,
}

fn slug(s: &str) -> String {
    let t: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let t = t.trim_matches('-').chars().take(40).collect::<String>();
    if t.is_empty() { "x".into() } else { t }
}

fn parse_tag(title: &str) -> (String, String, String, i64) {
    let mut p = title.split('|');
    let (k, r, s, t) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or("0"));
    (k.into(), r.into(), s.into(), t.parse().unwrap_or(0))
}

impl History {
    /// `workspace` is the workspace's name or folder; `session` the chat's id (empty when there is none).
    pub fn new(kb: Arc<Mutex<KnowledgeBase>>, user: &str, workspace: &str, session: Option<&str>) -> History {
        History { kb, ns: format!("workspace/{}/{}", slug(user), slug(workspace)), session: session.map(slug).unwrap_or_default() }
    }

    pub fn namespace(&self) -> &str {
        &self.ns
    }

    /// Index one piece of history. Credentials are redacted first; the text is cut to [`MAX_CHUNK`] characters.
    pub fn record(&self, kind: &str, row: &str, text: &str) {
        let redacted = crate::credguard::detect(text).map_or_else(|| text.to_string(), |f| f.redacted);
        let body: String = redacted.trim().chars().take(MAX_CHUNK).collect();
        if body.is_empty() {
            return;
        }
        let tag = format!("{}|{}|{}|{}", slug(kind), row.replace('|', "/"), self.session, crate::clock::now_unix());
        let mut kb = self.kb.lock().unwrap_or_else(|e| e.into_inner());
        let _ = kb.add_document(&self.ns, &tag, "workspace history", &format!("{kind} {row}\n{body}"), Default::default());
        self.prune(&mut kb);
    }

    fn prune(&self, kb: &mut KnowledgeBase) {
        let mut docs = kb.docs(&self.ns);
        if docs.len() <= MAX_DOCS {
            return;
        }
        // oldest first; tool digests, errors and turns go before decisions and summaries
        docs.sort_by_key(|d| (matches!(parse_tag(&d.title).0.as_str(), "summary" | "decision"), d.added));
        for d in docs.iter().take(docs.len() - MAX_DOCS + 50) {
            let _ = kb.remove_document(&self.ns, &d.id);
        }
    }

    /// The `k` most relevant snippets for `query`, newest information not preferred over relevance.
    pub fn search(&self, query: &str, scope: Scope, kind: Option<&str>, k: usize) -> Vec<Snippet> {
        if query.trim().is_empty() || k == 0 {
            return vec![];
        }
        let hits = self.kb.lock().unwrap_or_else(|e| e.into_inner()).search_weighted(&[(self.ns.as_str(), 1.0)], query, k * 4 + 8);
        let mut out = Vec::new();
        for h in hits {
            let (kd, row, session, ts) = parse_tag(&h.title);
            if scope == Scope::Session && session != self.session {
                continue;
            }
            if kind.is_some_and(|w| !w.is_empty() && w != kd) {
                continue;
            }
            let text = h.text.split_once('\n').map_or(h.text.clone(), |x| x.1.to_string());
            out.push(Snippet { id: format!("{}#{}", h.doc_id, h.chunk_idx), kind: kd, row, session, ts, text });
            if out.len() >= k {
                break;
            }
        }
        out
    }

    /// The newest chunk of a kind in this workspace (for example the last `summary` of an earlier session).
    pub fn latest(&self, kind: &str) -> Option<Snippet> {
        let kb = self.kb.lock().unwrap_or_else(|e| e.into_inner());
        let doc = kb.docs(&self.ns).into_iter().filter(|d| parse_tag(&d.title).0 == kind).max_by_key(|d| (parse_tag(&d.title).3, d.added))?;
        let hit = kb.search_weighted(&[(self.ns.as_str(), 1.0)], kind, 400).into_iter().find(|h| h.doc_id == doc.id)?;
        let (kd, row, session, ts) = parse_tag(&hit.title);
        Some(Snippet { id: format!("{}#{}", hit.doc_id, hit.chunk_idx), kind: kd, row, session, ts, text: hit.text.split_once('\n').map_or(hit.text.clone(), |x| x.1.to_string()) })
    }

    pub fn len(&self) -> usize {
        self.kb.lock().unwrap_or_else(|e| e.into_inner()).docs(&self.ns).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget everything of this workspace (the IDE's "forget history").
    pub fn forget_workspace(&self) {
        let _ = self.kb.lock().unwrap_or_else(|e| e.into_inner()).drop_namespace(&self.ns);
    }

    /// Forget what one session did (when its chat is deleted).
    pub fn forget_session(&self, session: &str) {
        let s = slug(session);
        let mut kb = self.kb.lock().unwrap_or_else(|e| e.into_inner());
        for d in kb.docs(&self.ns) {
            if parse_tag(&d.title).2 == s {
                let _ = kb.remove_document(&self.ns, &d.id);
            }
        }
    }

    /// Forget what one chat did in every workspace of this person (the chat does not know its workspace any more).
    pub fn forget_session_everywhere(kb: &Arc<Mutex<KnowledgeBase>>, user: &str, session: &str) -> usize {
        let prefix = format!("workspace/{}/", slug(user));
        let s = slug(session);
        let mut kb = kb.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for ns in kb.namespaces().into_iter().filter(|n| n.starts_with(&prefix)) {
            for d in kb.docs(&ns) {
                if parse_tag(&d.title).2 == s && kb.remove_document(&ns, &d.id).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    /// Snippets as text for a prompt, within about `max_chars` characters, each with its id (so the model can cite it).
    pub fn render(snips: &[Snippet], max_chars: usize) -> String {
        let mut s = String::new();
        for x in snips {
            let line = format!("- [{}] ({}{}) {}\n", x.id, x.kind, if x.row.is_empty() { String::new() } else { format!(" {}", x.row) }, x.text.replace('\n', " "));
            if s.chars().count() + line.chars().count() > max_chars {
                break;
            }
            s.push_str(&line);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> Arc<Mutex<KnowledgeBase>> {
        Arc::new(Mutex::new(KnowledgeBase::in_memory(crate::kb::default_embedder())))
    }

    #[test]
    fn a_fact_from_step_two_is_found_at_step_sixteen_and_scopes_separate_sessions() {
        let store = kb();
        let h1 = History::new(store.clone(), "alice", "Web Shop", Some("s_one"));
        h1.record("tool", "F-APP-01", "k8s_status: the registry is at 192.168.1.68:30500 and the namespace is test2-dev");
        for i in 0..14 {
            h1.record("tool", &format!("F-APP-0{}", i % 7), &format!("step {i}: wrote file{i}.js and ran npm install, nothing special"));
        }
        let hit = h1.search("which registry address did we find", Scope::Session, None, 3);
        assert!(hit.iter().any(|s| s.text.contains("192.168.1.68:30500") && s.row == "F-APP-01" && s.kind == "tool"), "{hit:?}");
        // another session of the same workspace sees it with workspace scope only
        let h2 = History::new(store.clone(), "alice", "Web Shop", Some("s_two"));
        assert!(h2.search("registry address", Scope::Session, None, 3).iter().all(|s| !s.text.contains("192.168.1.68")), "session scope is this session only");
        assert!(h2.search("registry address", Scope::Workspace, None, 3).iter().any(|s| s.text.contains("192.168.1.68:30500") && s.session == "s-one"));
        // kind filter
        assert!(h2.search("registry address", Scope::Workspace, Some("summary"), 3).is_empty());
        // another person, or another workspace, sees nothing
        assert!(History::new(store.clone(), "bob", "Web Shop", None).search("registry address", Scope::Workspace, None, 3).is_empty());
        assert!(History::new(store, "alice", "Other", None).search("registry address", Scope::Workspace, None, 3).is_empty());
    }

    #[test]
    fn a_new_session_recalls_the_summary_of_the_earlier_run() {
        let store = kb();
        let old = History::new(store.clone(), "alice", "shop", Some("s_old"));
        old.record("summary", "", "Done: F-APP-01 write server.js, F-APP-02 Dockerfile. Blocked: none. Files changed: server.js, Dockerfile. Last error: none.");
        let fresh = History::new(store, "alice", "shop", Some("s_new"));
        let s = fresh.latest("summary").expect("the earlier summary");
        assert!(s.text.contains("Files changed: server.js, Dockerfile") && s.session == "s-old", "{s:?}");
        assert!(fresh.search("what was done earlier with the Dockerfile", Scope::Workspace, None, 2).iter().any(|x| x.kind == "summary"));
    }

    #[test]
    fn credentials_are_never_indexed_and_chunks_are_capped_and_forgetting_works() {
        let store = kb();
        let h = History::new(store.clone(), "alice", "shop", Some("s_a"));
        h.record("tool", "", "ran the deploy with password: hunter2hunter2X9 and the token ghp_abcdefghijklmnopqrstuvwxyz0123456789 set");
        h.record("tool", "", &format!("long output {}", "x ".repeat(2000)));
        let all = h.search("password token deploy", Scope::Workspace, None, 5);
        let joined: String = all.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" ");
        assert!(!joined.contains("hunter2") && !joined.contains("ghp_"), "{joined}");
        assert!(h.search("long output", Scope::Workspace, None, 5).iter().all(|s| s.text.chars().count() <= MAX_CHUNK + 20));
        // a summary stays inside the cap the autopilot uses (see autopilot::SUMMARY_CAP) and renders within a budget
        let snips = h.search("long output deploy", Scope::Workspace, None, 5);
        assert!(History::render(&snips, 500).chars().count() <= 500);
        h.record("decision", "F-APP-01", "chose port 3000");
        h.forget_session("s_a");
        assert!(h.is_empty(), "everything of that session is gone");
        h.record("decision", "", "again");
        h.forget_workspace();
        assert!(h.is_empty());
    }

    #[test]
    fn deleting_a_chat_forgets_its_history_in_every_workspace_of_that_person_only() {
        let store = kb();
        for ws in ["shop", "blog"] {
            History::new(store.clone(), "alice", ws, Some("s_gone")).record("answer", "r1", "the registry is at port 5000");
            History::new(store.clone(), "alice", ws, Some("s_keep")).record("answer", "r2", "the cache uses redis");
        }
        History::new(store.clone(), "bob", "shop", Some("s_gone")).record("answer", "r3", "bob's note");
        assert_eq!(History::forget_session_everywhere(&store, "alice", "s_gone"), 2);
        assert_eq!(History::new(store.clone(), "alice", "shop", None).len(), 1);
        assert_eq!(History::new(store.clone(), "alice", "blog", None).len(), 1);
        assert_eq!(History::new(store, "bob", "shop", None).len(), 1, "another person's history stays");
    }

    #[test]
    fn pruning_keeps_decisions_and_summaries_longest() {
        let store = kb();
        let h = History::new(store, "alice", "shop", Some("s"));
        h.record("summary", "", "the one summary that must survive pruning");
        for i in 0..(MAX_DOCS + 30) {
            h.record("tool", "", &format!("digest number {i} of some tool output"));
        }
        assert!(h.len() <= MAX_DOCS + 1, "{}", h.len());
        assert!(h.latest("summary").is_some(), "the summary survived");
    }
}
