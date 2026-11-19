//! The pending-work facet over the real ledger (F-SPLIT-03): the chat and the voice assistant ask for a waker and a strip line and never name
//! the ledger, the feed or the jobs. See `iyakki_agent_api::pending`.

use std::sync::Arc;

use iyakki_agent_api::pending::{AgentPending, PendingWaker, TaskList};

use crate::pending::{Feed, Ledger, Source, Waker};
use crate::services::Services;

pub struct RealPending {
    svc: Arc<Services>,
}

impl Services {
    /// The pending-work facet over these services (`AppCx::pendingapi`).
    pub fn pending_facet(self: &Arc<Self>) -> Arc<dyn AgentPending> {
        Arc::new(RealPending { svc: self.clone() })
    }
}

impl RealPending {
    fn ledger(&self, user: &str) -> Ledger {
        Ledger::open(&self.svc.dir().join("pending"), user)
    }
}

struct RealWaker(Waker);

impl PendingWaker for RealWaker {
    fn ready(&self) -> bool {
        self.0.ready()
    }
    fn take(&self) -> Option<iyakki_shared::pending::Wake> {
        self.0.take()
    }
}

impl AgentPending for RealPending {
    fn pending_waker(&self, user: &str, workspace: &str, tasks: Option<TaskList>) -> Option<Box<dyn PendingWaker>> {
        let mut feed = Feed::new(user, workspace);
        feed.jobs = Some(self.svc.jobs.clone());
        if let Some(m) = tasks {
            feed.tasks = Some(Arc::new(move || m.list()));
        }
        Some(Box::new(RealWaker(Waker::start(self.ledger(user), feed))))
    }

    fn wrap_pending_tools(&self, inner: Arc<dyn iyakki_shared::agent::Tools>, user: &str, workspace: &str, tasks: Option<TaskList>, policy: iyakki_sec::policy::Policy, cwd: &std::path::Path, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_shared::agent::Tools> {
        let mut feed = Feed::new(user, workspace);
        feed.jobs = Some(self.svc.jobs.clone());
        if let Some(m) = tasks {
            feed.tasks = Some(Arc::new(move || m.list()));
        }
        Arc::new(crate::pendingtools::PendingTools::new(inner, self.ledger(user), feed, policy, cwd).with_audit(audit))
    }

    fn pending_settings(&self, user: &str) -> iyakki_shared::pending::Settings {
        self.ledger(user).settings()
    }

    fn pending_set_settings(&self, user: &str, s: iyakki_shared::pending::Settings) {
        self.ledger(user).set_settings(s);
    }

    fn pending_wakes_left(&self, user: &str) -> u32 {
        self.ledger(user).wakes_left()
    }

    fn pending_strip_line(&self, user: &str, workspace: &str, now_ms: u64) -> Option<String> {
        let mut tickets = self.ledger(user).list(workspace);
        tickets.retain(|t| t.source != Source::Job);
        crate::pendingplus::strip_text(&tickets, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_facet_gives_a_waker_that_starts_quiet_and_a_strip_line_that_is_empty_with_nothing_pending() {
        let d = std::env::temp_dir().join(format!("iyakki-pendfacet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        struct Dead;
        impl iyakki_proxy::Upstream for Dead {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        let f = svc.pending_facet();
        let w = f.pending_waker("amy", "", None).expect("the real core has a ledger");
        assert!(!w.ready());
        assert_eq!(f.pending_strip_line("amy", "", 1), None);
        let mut st = f.pending_settings("amy");
        st.auto_continue = !st.auto_continue;
        f.pending_set_settings("amy", st.clone());
        assert_eq!(f.pending_settings("amy"), st, "the Waiting settings are kept");
        let _ = std::fs::remove_dir_all(d);
    }
}
