//! A flow `approval` node as a tenant approval request.
//!
//! The node waits in the engine (the run is `blocked`, no worker is held). Here it becomes a request of kind `flow.approval` in the
//! tenant's approval router (`tenantconfig::approvals`): the approvers are the holders of the approver role, then the team's admins
//! without the requester, then the tenant admins (escalation); nobody approves their own request. The decision arrives by
//! `POST /api/v1/tenant/approvals/{id}/approve|deny` from any session of any eligible approver, at any time before the request
//! expires. A poll ([`Waits::poll`]) carries a decision to the node, or applies the node's `on_timeout` (approve, reject or fail;
//! default reject) when the node's own timeout or the request's expiry comes first, and the notification hub (`notifyhub`) announces the
//! request to the approvers and sends the reminders (it reads the approvals file), so this file only tells the approvers and the
//! requester when a request expired.
//!
//! Exactly once: [`resolve_once`] takes a claim file (`O_EXCL`, so it holds across processes on the volume) named after the run and
//! node before it tells the engine; the desktop path (the INBOX approve/reject) and this one both go through it. The claim holds the
//! outcome, so a crash between the claim and the engine is finished at the next poll.

use super::notify::{say, NotifySink, Severity};
use crate::cloudroles::Roles;
use crate::tenantconfig::{api::Env, approvals};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnTimeout {
    Approve,
    Reject,
    Fail,
}

impl OnTimeout {
    /// The node's `on_timeout`; anything else, and empty, is reject (default deny).
    pub fn parse(s: &str) -> OnTimeout {
        match s {
            "approve" => OnTimeout::Approve,
            "fail" => OnTimeout::Fail,
            _ => OnTimeout::Reject,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub approval_id: String,
    pub tenant: String,
    pub user: String,
    pub team: String,
    pub flow: String,
    pub run: String,
    pub node: String,
    pub message: String,
    pub created: u64,
    /// The node's own timeout (seconds since the epoch, 0: none).
    pub deadline: u64,
    pub on_timeout: OnTimeout,
    /// `waiting`, `claimed` (the outcome is chosen, the engine not yet told) or `resolved`.
    pub state: String,
    /// `approved by x`, `rejected by x`, `timeout: reject` ...
    pub outcome: String,
}

/// What the engine side offers: tell a waiting node the verdict, or fail it.
pub trait Resolver: Send + Sync {
    fn decide(&self, user: &str, run: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String>;
    fn fail(&self, user: &str, run: &str, node: &str, reason: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Applied,
    AlreadyResolved,
}

fn plain(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(30).collect()
}

/// The router's item name of a waiting node.
pub fn item_of(run: &str, node: &str) -> String {
    format!("{}_{}", plain(run), plain(node))
}

fn claims(dir: &Path) -> PathBuf {
    dir.join("claims")
}

/// Take the right to resolve (run, node). True for exactly one caller.
fn claim(dir: &Path, run: &str, node: &str, outcome: &str) -> bool {
    use std::io::Write;
    let d = claims(dir);
    let _ = std::fs::create_dir_all(&d);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(d.join(item_of(run, node))) {
        Ok(mut f) => {
            let _ = f.write_all(outcome.as_bytes());
            true
        }
        Err(_) => false,
    }
}

pub struct Waits {
    dir: PathBuf,
    lock: std::sync::Mutex<()>,
}

impl Waits {
    pub fn new(flows_dir: &Path) -> Waits {
        Waits { dir: flows_dir.to_path_buf(), lock: std::sync::Mutex::new(()) }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("waits.json")
    }

    fn load(&self) -> Vec<Wait> {
        super::load_json(&self.file())
    }

    fn save(&self, mut v: Vec<Wait>, now: u64) {
        v.retain(|w| w.state != "resolved" || w.created + 7 * 86_400 > now);
        let _ = super::save_json(&self.file(), &v);
    }

    pub fn waiting(&self) -> Vec<Wait> {
        self.load().into_iter().filter(|w| w.state != "resolved").collect()
    }

    /// The node started waiting: file the request and tell the approvers. `Err` means nobody can approve: the caller rejects the node.
    #[allow(clippy::too_many_arguments)]
    pub fn file_wait(&self, env: &Env, roles: &Roles, tenant: &str, team: &str, user: &str, flow: &str, run: &str, node: &str, message: &str, timeout_s: Option<u64>, on_timeout: &str, now: u64) -> Result<Wait, String> {
        let item = item_of(run, node);
        if approvals::route(roles, team, user, false, &[&item, "flow.approval"], false).approvers.is_empty() {
            return Err("nobody is eligible to approve this step; it is rejected".into());
        }
        let p = approvals::request(env, roles, "flow.approval", team, &item, user, false, false, now)?;
        let w = Wait { approval_id: p.id.clone(), tenant: tenant.into(), user: user.into(), team: team.into(), flow: flow.into(), run: run.into(), node: node.into(), message: message.chars().take(300).collect(), created: now, deadline: timeout_s.map_or(0, |t| now + t), on_timeout: OnTimeout::parse(on_timeout), state: "waiting".into(), outcome: String::new() };
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        if !all.iter().any(|x| x.approval_id == w.approval_id) {
            all.push(w.clone());
        }
        self.save(all, now);
        Ok(w)
    }

    /// One verdict for (run, node), from whichever path got there first.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_once(&self, user: &str, run: &str, node: &str, approved: bool, by: &str, note: &str, resolver: &dyn Resolver, now: u64) -> Resolution {
        let outcome = format!("{} by {by}", if approved { "approved" } else { "rejected" });
        if !claim(&self.dir, run, node, &outcome) {
            return Resolution::AlreadyResolved;
        }
        self.mark(run, node, "claimed", &outcome, now);
        self.apply(user, run, node, approved, by, note, resolver, now);
        Resolution::Applied
    }

    fn mark(&self, run: &str, node: &str, state: &str, outcome: &str, now: u64) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        for w in all.iter_mut().filter(|w| w.run == run && w.node == node && w.state != "resolved") {
            w.state = state.into();
            w.outcome = outcome.into();
        }
        self.save(all, now);
    }

    #[allow(clippy::too_many_arguments)]
    fn apply(&self, user: &str, run: &str, node: &str, approved: bool, by: &str, note: &str, resolver: &dyn Resolver, now: u64) {
        let r = if by == "timeout:fail" { resolver.fail(user, run, node, note) } else { resolver.decide(user, run, node, approved, by, note) };
        // an engine that no longer has the node waiting (the run was cancelled, a restart finished it) counts as done
        if r.is_ok() || r.as_ref().err().is_some_and(|e| e.contains("not waiting") || e.contains("no such")) {
            self.mark_state(run, node, "resolved", now);
        }
    }

    fn mark_state(&self, run: &str, node: &str, state: &str, now: u64) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        for w in all.iter_mut().filter(|w| w.run == run && w.node == node) {
            w.state = state.into();
        }
        self.save(all, now);
    }

    /// Carry decisions and timeouts to the nodes. Returns one line per thing that happened.
    #[allow(clippy::too_many_arguments)]
    pub fn poll(&self, env: &Env, roles: &Roles, sink: &dyn NotifySink, resolver: &dyn Resolver, now: u64) -> Vec<String> {
        let mut lines = Vec::new();
        for w in self.waiting() {
            if w.state == "claimed" {
                // finish what a crash left half done
                let approved = w.outcome.starts_with("approved") || w.outcome.starts_with("timeout: approve");
                let by = w.outcome.rsplit(' ').next().unwrap_or("").to_string();
                self.apply(&w.user, &w.run, &w.node, approved, if w.outcome.starts_with("timeout: fail") { "timeout:fail" } else { &by }, &w.outcome, resolver, now);
                continue;
            }
            let st = approvals::state_of(env, &w.approval_id);
            match st {
                Some((s, by, _, _)) if s == "approved" || s == "used" || s == "denied" => {
                    let approved = s != "denied";
                    let by = by.unwrap_or_default();
                    if self.resolve_once(&w.user, &w.run, &w.node, approved, &by, "", resolver, now) == Resolution::Applied {
                        lines.push(format!("{}/{}: {} by {by}", w.run, w.node, if approved { "approved" } else { "rejected" }));
                    }
                }
                other => {
                    let expires = other.as_ref().map_or(0, |(_, _, _, e)| *e);
                    let timed_out = (w.deadline > 0 && now >= w.deadline) || now >= expires;
                    if timed_out {
                        let (approved, tag, by) = match w.on_timeout {
                            OnTimeout::Approve => (true, "timeout: approve", "timeout"),
                            OnTimeout::Reject => (false, "timeout: reject", "timeout"),
                            OnTimeout::Fail => (false, "timeout: fail", "timeout:fail"),
                        };
                        if claim(&self.dir, &w.run, &w.node, tag) {
                            approvals::close(env, &w.approval_id, "timeout", now);
                            self.mark(&w.run, &w.node, "claimed", tag, now);
                            self.apply(&w.user, &w.run, &w.node, approved, by, "the approval timed out", resolver, now);
                            let mut to = approvals::approvers_of(env, roles, &w.approval_id);
                            to.push(w.user.clone());
                            say(sink, &w.tenant, &w.team, to, vec![], Severity::High, &format!("Approval expired: {}", w.flow), &format!("Nobody answered in time; the step was handled as `{}`.", tag.trim_start_matches("timeout: ")), Some(("flows", &w.run)), &format!("approval-expired:{}", w.approval_id), &format!("flow:{}:{}", w.flow, w.run));
                            lines.push(format!("{}/{}: {tag}", w.run, w.node));
                        }
                        continue;
                    }
                }
            }
        }
        lines
    }
}
