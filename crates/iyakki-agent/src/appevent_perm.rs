//! Pulled sources of `app.permissions` (F-SDK-217): what the pushed ones (F-SDK-215) cannot see because nobody calls them: a company
//! policy that changed, a folder the file policy no longer lets an app read, a grant with an end time that has passed and an app the
//! store's revocation list withdrew. Each look compares what an app effectively holds with the look before; when it changed and one of
//! these causes explains it the app is told once, with the cause as the word `by` (`policy`, `expiry` or `store`). A change with no cause
//! here (the person or an administrator took it away) is told by its own push and never twice.

use crate::appevent_sources::permissions_told;
use iyakki_core_api::appevents::AppEvents;
use std::collections::{BTreeMap, BTreeSet};

type AppKey = (String, String);

/// One look at what each app holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermFacts {
    /// `(person, app)` to the capability names the app holds after every policy was applied.
    pub effective: BTreeMap<AppKey, BTreeSet<String>>,
    /// `(person, app)` to the folders the file policy lets the app read.
    pub folders: BTreeMap<AppKey, BTreeSet<String>>,
    /// Changes whenever a tenant or company policy was replaced.
    pub policy_epoch: u64,
    /// Grants with an end: `(person, app, capability)` to the Unix time they end.
    pub expiries: BTreeMap<(String, String, String), i64>,
    /// Apps the store's revocation list withdrew.
    pub store_revoked: BTreeSet<String>,
    pub now: i64,
}

#[derive(Default)]
pub struct PermWatch {
    last: Option<PermFacts>,
}

/// A change to tell: `(person, app, was, now, by)`.
pub type Told = (String, String, Vec<String>, Vec<String>, &'static str);

impl PermWatch {
    pub fn observe(&mut self, now: PermFacts) -> Vec<Told> {
        let mut out = Vec::new();
        if let Some(prev) = &self.last {
            for ((user, app), held) in &now.effective {
                let Some(was) = prev.effective.get(&(user.clone(), app.clone())) else { continue };
                if was == held {
                    continue;
                }
                let lost: Vec<&String> = was.difference(held).collect();
                let by = if now.store_revoked.contains(app) && !prev.store_revoked.contains(app) {
                    Some("store")
                } else if !lost.is_empty() && lost.iter().all(|c| prev.expiries.get(&(user.clone(), app.clone(), (*c).clone())).is_some_and(|e| *e <= now.now)) {
                    Some("expiry")
                } else if now.policy_epoch != prev.policy_epoch {
                    Some("policy")
                } else {
                    None
                };
                if let Some(by) = by {
                    out.push((user.clone(), app.clone(), was.iter().cloned().collect(), held.iter().cloned().collect(), by));
                }
            }
            for (key, now_f) in &now.folders {
                let Some(was_f) = prev.folders.get(key) else { continue };
                if was_f != now_f {
                    // a folder is a name like any other capability here: `folder:Documents`
                    let tag = |s: &BTreeSet<String>| s.iter().map(|f| format!("folder:{f}")).collect::<Vec<_>>();
                    out.push((key.0.clone(), key.1.clone(), tag(was_f), tag(now_f), "policy"));
                }
            }
        }
        self.last = Some(now);
        out
    }
}

/// Tell the engine what a look found.
pub fn tell(hub: &dyn AppEvents, found: Vec<Told>) {
    for (user, app, was, now, by) in found {
        permissions_told(hub, &user, &app, &was, &now, by, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(held: &[&str]) -> PermFacts {
        PermFacts { effective: BTreeMap::from([(("pat".to_string(), "notes".to_string()), held.iter().map(|s| s.to_string()).collect())]), ..Default::default() }
    }

    #[test]
    fn a_company_policy_that_took_something_away_is_told_once_by_policy() {
        let mut w = PermWatch::default();
        assert!(w.observe(facts(&["camera.view", "events.power"])).is_empty());
        let mut f = facts(&["events.power"]);
        f.policy_epoch = 1;
        let got = w.observe(f.clone());
        assert_eq!(got, [("pat".to_string(), "notes".to_string(), vec!["camera.view".to_string(), "events.power".to_string()], vec!["events.power".to_string()], "policy")]);
        assert!(w.observe(f).is_empty());
    }

    #[test]
    fn a_change_nothing_here_explains_is_left_to_its_own_push() {
        let mut w = PermWatch::default();
        w.observe(facts(&["a", "b"]));
        assert!(w.observe(facts(&["a"])).is_empty());
    }

    #[test]
    fn a_grant_that_ended_is_told_by_expiry_only_when_its_time_has_come() {
        let mut w = PermWatch::default();
        let mut f = facts(&["a", "b"]);
        f.expiries.insert(("pat".into(), "notes".into(), "b".into()), 1000);
        f.now = 900;
        w.observe(f.clone());
        let mut g = facts(&["a"]);
        g.expiries = f.expiries.clone();
        g.now = 900;
        assert!(w.observe(g.clone()).is_empty(), "taken away before its end: not an expiry");
        let mut w = PermWatch::default();
        f.now = 900;
        w.observe(f);
        g.now = 1001;
        let got = w.observe(g);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].4, "expiry");
    }

    #[test]
    fn a_withdrawn_app_is_told_by_store_even_when_a_policy_changed_too() {
        let mut w = PermWatch::default();
        w.observe(facts(&["a"]));
        let mut f = facts(&[]);
        f.store_revoked.insert("notes".into());
        f.policy_epoch = 4;
        assert_eq!(w.observe(f)[0].4, "store");
    }

    #[test]
    fn a_folder_the_policy_no_longer_lets_the_app_read_is_told_by_name() {
        let mut w = PermWatch::default();
        let key = ("pat".to_string(), "notes".to_string());
        let mut f = PermFacts::default();
        f.folders.insert(key.clone(), ["Documents".to_string(), "Pictures".to_string()].into());
        w.observe(f.clone());
        f.folders.insert(key, ["Documents".to_string()].into());
        let got = w.observe(f);
        assert_eq!(got[0].2, ["folder:Documents", "folder:Pictures"]);
        assert_eq!(got[0].3, ["folder:Documents"]);
        assert_eq!(got[0].4, "policy");
    }

    #[test]
    fn the_engine_is_told_through_the_one_helper_and_remembers_the_cause() {
        use iyakki_proxy::{Upstream, UpstreamResponse};
        struct Dead;
        impl Upstream for Dead {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-evperm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let s = crate::services::Services::open(&d.join("svc"), std::sync::Arc::new(Dead), std::sync::Arc::new(Dead)).unwrap();
        tell(&*s.app_events, vec![("pat".into(), "sample".into(), vec!["a".into(), "b".into()], vec!["a".into()], "policy")]);
        let (_, last) = s.app_events.permissions("pat", "sample");
        assert!(last.is_some_and(|l| l.contains("policy")));
    }
}
