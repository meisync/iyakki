//! Data imports through the web service on a cloud machine (F-WEBSVC-08, option A of docs/development/WEBSVC-DATA-IMPORT-DECISION.md).
//!
//! `POST /api/v1/data/call` with `data_create`, `data_append` or `data_export` does not write anything. It stores the request as a
//! pending import (in memory only, the rows are never written to disk before an approval), answers 202 with the import's id and a
//! plain summary (tool, file name, counts, never the data), and the person approves on the web page's approvals list
//! (`POST /api/v1/approvals/data/{id}`). The approval must come from a DIFFERENT sign-in than the one that asked: a stolen script token
//! cannot approve its own import. A request expires after five minutes, runs once, and the requester reads the outcome once with
//! `GET /api/v1/data/imports/{id}`.

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const WRITE_TOOLS: [&str; 3] = ["data_create", "data_append", "data_export"];
pub const LIFE: Duration = Duration::from_secs(300);
pub const MAX_PENDING: usize = 5;

pub fn is_write_tool(t: &str) -> bool {
    WRITE_TOOLS.contains(&t)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Running,
    Done(Result<String, String>),
    Denied,
}

struct Import {
    id: String,
    user: String,
    /// A fingerprint of the sign-in that asked (never the token itself).
    asker: u64,
    tool: String,
    args: Value,
    summary: String,
    state: State,
    at: Instant,
}

pub struct View {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub age_secs: u64,
}

#[derive(Default)]
pub struct DataImports {
    list: Mutex<Vec<Import>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A fingerprint of a bearer token so two sign-ins can be told apart without keeping the token.
pub fn fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// What the person is asked to approve, in words. Counts and names only: never a cell.
pub fn summarize(tool: &str, args: &Value) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").chars().filter(|c| !c.is_control()).take(80).collect::<String>();
    let rows = args.get("rows").and_then(Value::as_array).map(|r| r.len());
    match tool {
        "data_create" => {
            let (name, cols, rows) = (s("name"), args.get("columns").and_then(Value::as_array).map(|c| c.len()), rows);
            let (Some(cols), Some(rows)) = (cols, rows) else { return Err("data_create needs name, columns and rows".into()) };
            if name.is_empty() {
                return Err("data_create needs a file name".into());
            }
            let over = if args.get("overwrite").and_then(Value::as_bool) == Some(true) { ", replacing a file of that name" } else { "" };
            Ok(format!("create the table file {name} with {cols} columns and {rows} rows{over}"))
        }
        "data_append" => {
            let Some(rows) = rows else { return Err("data_append needs rows".into()) };
            let target = if !s("path").is_empty() { s("path") } else if !s("handle").is_empty() { format!("the table {}", s("handle")) } else { return Err("data_append needs a path or a handle".into()) };
            Ok(format!("append {rows} rows to {target}"))
        }
        "data_export" => {
            if s("handle").is_empty() || s("path").is_empty() {
                return Err("data_export needs a handle and a path".into());
            }
            Ok(format!("write the table {} to the file {}", s("handle"), s("path")))
        }
        _ => Err("not a data import tool".into()),
    }
}

impl DataImports {
    fn sweep(l: &mut Vec<Import>) {
        l.retain(|i| i.at.elapsed() < LIFE);
    }

    /// Store a request. Returns (id, summary).
    pub fn submit(&self, user: &str, asker: u64, tool: &str, args: Value) -> Result<(String, String), String> {
        let summary = summarize(tool, &args)?;
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        if l.iter().filter(|i| i.user == user).count() >= MAX_PENDING {
            return Err("too many imports are waiting; answer or wait for them first".into());
        }
        let mut b = [0u8; 8];
        let _ = iyakki_sec::random_fill(&mut b);
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        l.push(Import { id: id.clone(), user: user.into(), asker, tool: tool.into(), args, summary: summary.clone(), state: State::Pending, at: Instant::now() });
        Ok((id, summary))
    }

    /// The person's waiting imports, oldest first.
    pub fn waiting(&self, user: &str) -> Vec<View> {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        l.iter().filter(|i| i.user == user && i.state == State::Pending).map(|i| View { id: i.id.clone(), tool: i.tool.clone(), summary: i.summary.clone(), age_secs: i.at.elapsed().as_secs() }).collect()
    }

    /// The person's answer from another sign-in. On approve returns the tool and its arguments to run, once.
    pub fn decide(&self, user: &str, decider: u64, id: &str, approve: bool) -> Result<Option<(String, Value)>, &'static str> {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        let Some(i) = l.iter_mut().find(|i| i.user == user && i.id == id) else { return Err("no such waiting import") };
        if i.state != State::Pending {
            return Err("already answered");
        }
        if i.asker == decider {
            return Err("the sign-in that asked cannot approve its own import");
        }
        if !approve {
            i.state = State::Denied;
            i.args = Value::Null;
            return Ok(None);
        }
        i.state = State::Running;
        Ok(Some((i.tool.clone(), std::mem::take(&mut i.args))))
    }

    pub fn finish(&self, user: &str, id: &str, result: Result<String, String>) {
        let mut l = lock(&self.list);
        if let Some(i) = l.iter_mut().find(|i| i.user == user && i.id == id) {
            i.state = State::Done(result);
            i.at = Instant::now();
        }
    }

    /// The requester's poll. A finished or refused import is read once and then forgotten.
    pub fn poll(&self, user: &str, id: &str) -> Option<Value> {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        let p = l.iter().position(|i| i.user == user && i.id == id)?;
        let v = match &l[p].state {
            State::Pending => return Some(json!({"id": id, "status": "pending", "summary": l[p].summary})),
            State::Running => return Some(json!({"id": id, "status": "running"})),
            State::Denied => json!({"id": id, "status": "denied"}),
            State::Done(Ok(r)) => json!({"id": id, "status": "done", "result": r}),
            State::Done(Err(e)) => json!({"id": id, "status": "failed", "error": e}),
        };
        l.remove(p);
        Some(v)
    }

    #[cfg(test)]
    pub fn age_all(&self, by: Duration) {
        for i in lock(&self.list).iter_mut() {
            i.at -= by;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_names_counts_and_never_cells() {
        let a = json!({"name": "sales.csv", "columns": ["a", "b"], "rows": [["secret-1", 2], ["secret-3", 4]]});
        let s = summarize("data_create", &a).unwrap();
        assert_eq!(s, "create the table file sales.csv with 2 columns and 2 rows");
        assert!(!s.contains("secret"));
        assert!(summarize("data_append", &json!({"rows": []})).is_err());
        assert!(summarize("data_query", &json!({})).is_err());
    }

    #[test]
    fn the_asking_sign_in_cannot_approve_and_an_answer_runs_once() {
        let d = DataImports::default();
        let (id, _) = d.submit("ann", 1, "data_append", json!({"path": "t.csv", "rows": [[1]]})).unwrap();
        assert_eq!(d.poll("ann", &id).unwrap()["status"], "pending");
        assert!(d.poll("bob", &id).is_none(), "another person never sees it");
        assert!(d.decide("bob", 2, &id, true).is_err());
        assert_eq!(d.decide("ann", 1, &id, true), Err("the sign-in that asked cannot approve its own import"));
        let run = d.decide("ann", 2, &id, true).unwrap().unwrap();
        assert_eq!(run.0, "data_append");
        assert_eq!(d.decide("ann", 2, &id, true), Err("already answered"));
        assert_eq!(d.poll("ann", &id).unwrap()["status"], "running");
        d.finish("ann", &id, Ok("done".into()));
        assert_eq!(d.poll("ann", &id).unwrap()["result"], "done");
        assert!(d.poll("ann", &id).is_none(), "read once");
    }

    #[test]
    fn a_refused_import_drops_the_data_and_an_old_one_expires() {
        let d = DataImports::default();
        let (id, _) = d.submit("ann", 1, "data_export", json!({"handle": "d1", "path": "o.csv"})).unwrap();
        assert_eq!(d.decide("ann", 2, &id, false), Ok(None));
        assert_eq!(d.poll("ann", &id).unwrap()["status"], "denied");
        let (id2, _) = d.submit("ann", 1, "data_export", json!({"handle": "d1", "path": "o.csv"})).unwrap();
        d.age_all(LIFE + Duration::from_secs(1));
        assert!(d.waiting("ann").is_empty());
        assert!(d.poll("ann", &id2).is_none());
    }

    #[test]
    fn at_most_five_wait_per_person() {
        let d = DataImports::default();
        for _ in 0..MAX_PENDING {
            d.submit("ann", 1, "data_append", json!({"path": "t.csv", "rows": []})).unwrap();
        }
        assert!(d.submit("ann", 1, "data_append", json!({"path": "t.csv", "rows": []})).is_err());
        assert!(d.submit("bob", 1, "data_append", json!({"path": "t.csv", "rows": []})).is_ok());
    }
}
