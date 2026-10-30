//! Sharing the cluster dashboard: who is seeded, who can open the `kube` source, what a recipient can and cannot do. Fake cluster
//! answers (see `kube_tests.rs`), accounts and homes in a temporary folder, no network.

use super::connector::fake::*;
use super::connector::*;
use super::kube_tests::FakeApi;
use super::model::*;
use super::seed::{seed_defaults, DASHBOARD_ID};
use super::share::Role as ShareRole;
use super::share::*;
use super::store::{Dashboards, Sources};
use super::tools::{Env, People};
use crate::wsexec::FakeRunner;
use iyakki_sec::policy::Role;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Ppl {
    names: Mutex<Vec<String>>,
    root: PathBuf,
}

impl People for Ppl {
    fn exists(&self, n: &str) -> bool {
        self.names.lock().unwrap().iter().any(|x| x == n)
    }
    fn home(&self, n: &str, _: &Path) -> PathBuf {
        let h = self.root.join("home").join(n);
        let _ = std::fs::create_dir_all(&h);
        h
    }
}

struct World {
    data: PathBuf,
    people: Arc<Ppl>,
    api: Arc<FakeApi>,
}

fn world(tag: &str) -> World {
    let root = std::env::temp_dir().join(format!("iyakki-dash-share-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let names = ["amy", "bob", "carol", "gus"].iter().map(|s| s.to_string()).collect();
    World { data, people: Arc::new(Ppl { names: Mutex::new(names), root }), api: FakeApi::full() }
}

type Audit = Arc<Mutex<Vec<String>>>;

fn env_of(w: &World, user: &str, role: Role) -> (Env, Audit) {
    let audit: Audit = Arc::new(Mutex::new(vec![]));
    let a = audit.clone();
    let mut d = deps(&FakeUp::new(), &[], Arc::new(FakeRunner::new(&[])));
    d.cluster = w.api.clone();
    let e = Env {
        user: user.into(),
        policy: role.template(),
        data_dir: w.data.clone(),
        home: w.people.home(user, &w.data),
        deps: d,
        now_ms: Arc::new(|| 1_792_065_600_000),
        vault_names: Arc::new(Vec::new),
        vault_set: Arc::new(|_, _| Ok(())),
        audit: Arc::new(move |k, d| a.lock().unwrap().push(format!("{k} {d}"))),
        cache: crate::dash::multi::FetchCache::new(),
        people: w.people.clone(),
    };
    (e, audit)
}

/// amy (administrator) with her seeded dashboard.
fn amy(w: &World) -> (Env, Audit) {
    let (e, a) = env_of(w, "amy", Role::Admin);
    assert_eq!(seed_defaults(&w.data, &e.home, "amy", e.is_admin()).unwrap().len(), 2);
    (e, a)
}

/// Viewers.
fn names(v: &[&str]) -> Vec<Grant> {
    v.iter().map(|s| Grant::viewer(s)).collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const SID: &str = "share:amy/k3s-local";

// ------------------------------------------------------------------------------------------------------ seeding and the gate

#[test]
fn only_administrators_are_seeded_and_a_later_promotion_seeds_then() {
    let w = world("seed");
    let (bob, _) = env_of(&w, "bob", Role::User);
    for role in [Role::User, Role::Operator, Role::Guest] {
        let (e, _) = env_of(&w, "bob", role);
        assert!(seed_defaults(&w.data, &e.home, "bob", e.is_admin()).unwrap().is_empty());
    }
    assert!(bob.dashboards().list().is_empty() && Sources::open(&w.data, "bob").list().is_empty(), "nothing was created for a non-administrator");
    assert!(!w.data.join("dash").join("seeded-bob.json").exists(), "and nothing was marked");
    let (admin_bob, _) = env_of(&w, "bob", Role::Admin);
    assert_eq!(seed_defaults(&w.data, &admin_bob.home, "bob", admin_bob.is_admin()).unwrap().len(), 2);
    assert!(admin_bob.dashboards().load(DASHBOARD_ID).is_ok());
    // deleted stays deleted, edits are kept
    admin_bob.dashboards().delete(DASHBOARD_ID).unwrap();
    assert!(seed_defaults(&w.data, &admin_bob.home, "bob", true).unwrap().is_empty());
    assert!(admin_bob.dashboards().find(DASHBOARD_ID).is_none());
}

#[test]
fn a_non_administrator_cannot_open_the_cluster_source_but_an_administrator_can() {
    let w = world("gate");
    let (bob, _) = env_of(&w, "bob", Role::User);
    // bob makes a kube source himself (the form offers the kind): it saves, but it will not connect
    bob.sources().save(super::seed::default_source()).unwrap();
    let e = bob.source("kube-local").err().unwrap();
    assert!(e.message.contains("open to administrators"), "{e}");
    assert!(w.api.asked.lock().unwrap().is_empty());
    let (op, _) = env_of(&w, "bob", Role::Operator);
    assert!(op.source("kube-local").is_err(), "an operator is not an administrator");
    let (admin, _) = env_of(&w, "bob", Role::Admin);
    assert!(admin.source("kube-local").is_ok());
    // a source kept in the shared system file does not open the door either
    let sys = Sources::open(&w.data, "system");
    sys.save(SourceConfig { id: "kube-sys".into(), ..super::seed::default_source() }).unwrap();
    assert!(bob.source("kube-sys").is_err());
    assert!(admin.source("kube-sys").is_ok());
}

// ------------------------------------------------------------------------------------------------------ sharing

#[test]
fn a_shared_dashboard_is_listed_opened_and_answers_only_its_own_views() {
    let w = world("flow");
    let (amy, audit) = amy(&w);
    let (bob, _) = env_of(&w, "bob", Role::User);
    assert!(bob.list_dashboards().is_empty(), "private until shared");
    assert!(bob.load_dashboard(SID).is_err());
    let ch = amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    assert_eq!(ch, ShareChange { added: names(&["bob"]), changed: vec![], removed: vec![] });
    assert_eq!(amy.shared_with(DASHBOARD_ID), names(&["bob"]));
    assert_eq!(audit.lock().unwrap().as_slice(), ["dash.shared k3s-local: bob (viewer)"]);
    // bob sees it, titled with the owner, and gets it with shared source ids
    let l = bob.list_dashboards();
    assert_eq!((l.len(), l[0].id.as_str(), l[0].title.as_str(), l[0].panels), (1, SID, "Local cluster (k3s) (shared by amy)", 20));
    let d = bob.load_dashboard(SID).unwrap();
    assert_eq!(d.id, SID);
    assert!(d.panels.iter().all(|p| p.source == "share:amy/k3s-local/kube-local") && d.default_source == "share:amy/k3s-local/kube-local");
    // he can read what the dashboard shows
    let cancel = Cancel::new();
    let (cfg, src) = bob.source(&d.panels[0].source).unwrap();
    assert!(cfg.name.contains("shared by amy") && cfg.kind == "kube");
    let f = src.query(&QueryRequest::new("nodes", range()), &cancel).unwrap();
    assert_eq!(f.cell(0, 0), "iyakki-dev");
    // ... and nothing else: a view the dashboard does not show, a write, a made-up query
    for q in ["node_states", "pods_by_phase ", "delete pods", "get secrets", "../x", ""] {
        let r = src.query(&QueryRequest::new(q, range()), &cancel);
        if q == "pods_by_phase " {
            assert!(r.is_ok(), "this one the dashboard shows");
        } else {
            assert!(r.is_err(), "{q:?} must be refused");
        }
    }
    let browsed: Vec<String> = src.browse(&[], &cancel).unwrap().into_iter().map(|n| n.name).collect();
    assert!(browsed.contains(&"nodes".to_string()) && !browsed.contains(&"node_states".to_string()), "{browsed:?}");
    // his own sources never hold the owner's
    assert!(bob.sources().list().is_empty());
    let bundle: serde_json::Value = serde_json::from_str(&Dashboards::export_bundle(&d, &bob.sources().list())).unwrap();
    assert!(bundle["sources"].as_array().unwrap().is_empty(), "no source settings travel with an export");
}

#[test]
fn someone_it_is_not_shared_with_gets_nothing_and_unsharing_takes_effect_at_once() {
    let w = world("unshare");
    let (amy, audit) = amy(&w);
    let (bob, _) = env_of(&w, "bob", Role::User);
    let (carol, _) = env_of(&w, "carol", Role::User);
    amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    // carol is not on the list: not listed, not opened, and a source id typed by hand does not work
    assert!(carol.list_dashboards().is_empty() && carol.load_dashboard(SID).is_err());
    assert!(carol.source("share:amy/k3s-local/kube-local").is_err());
    // bob can use it; then amy unshares
    assert!(bob.source("share:amy/k3s-local/kube-local").is_ok());
    let ch = amy.set_shares(DASHBOARD_ID, &[]).unwrap();
    assert_eq!(ch.removed, strs(&["bob"]));
    assert!(bob.list_dashboards().is_empty() && bob.load_dashboard(SID).is_err());
    assert!(bob.source("share:amy/k3s-local/kube-local").is_err());
    assert_eq!(audit.lock().unwrap().as_slice(), ["dash.shared k3s-local: bob (viewer)", "dash.unshared k3s-local: bob"]);
    // changing the list audits only the difference, by account name
    amy.set_shares(DASHBOARD_ID, &names(&["bob", "carol"])).unwrap();
    amy.set_shares(DASHBOARD_ID, &names(&["carol", "gus"])).unwrap();
    let a = audit.lock().unwrap().clone();
    assert_eq!(&a[2..], ["dash.shared k3s-local: bob (viewer), carol (viewer)", "dash.shared k3s-local: gus (viewer)", "dash.unshared k3s-local: bob"]);
    assert!(a.iter().all(|l| !l.contains("kube-local") && !l.contains("http")), "names and the dashboard id only");
    // the same list again changes nothing and audits nothing
    amy.set_shares(DASHBOARD_ID, &names(&["carol", "gus"])).unwrap();
    assert_eq!(audit.lock().unwrap().len(), a.len());
}

#[test]
fn a_recipient_cannot_edit_re_share_delete_or_export() {
    let w = world("readonly");
    let (amy, _) = amy(&w);
    let (bob, audit) = env_of(&w, "bob", Role::User);
    amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    let mut d = bob.load_dashboard(SID).unwrap();
    d.title = "mine now".into();
    assert!(bob.dashboards().save(&d).is_err(), "a shared id is not a dashboard id: nothing is written");
    d.id = "copy".into();
    assert!(bob.dashboards().list().is_empty());
    assert!(bob.set_shares(SID, &names(&["carol"])).is_err());
    assert!(bob.set_shares(DASHBOARD_ID, &names(&["carol"])).is_err(), "he has no dashboard of that id");
    assert!(bob.delete_dashboard(SID).is_err());
    assert!(amy.dashboards().load(DASHBOARD_ID).unwrap().title == "Local cluster (k3s)", "the owner's file is untouched");
    assert_eq!(amy.shared_with(DASHBOARD_ID), names(&["bob"]));
    // a dashboard of his own, even a copy of the shared one, cannot be shared by a non-administrator, and cannot reach the cluster
    let mut mine = bob.load_dashboard(SID).unwrap();
    mine.id = "copy".into();
    for p in &mut mine.panels {
        p.source = "kube-local".into();
    }
    mine.default_source = "kube-local".into();
    bob.dashboards().save(&mine).unwrap();
    bob.sources().save(super::seed::default_source()).unwrap();
    assert!(bob.set_shares("copy", &names(&["carol"])).unwrap_err().contains("only an administrator"));
    assert!(bob.source("kube-local").is_err());
    assert!(audit.lock().unwrap().is_empty());
    // a copy that uses the shared source id keeps working only as long as the share does
    mine.default_source = "share:amy/k3s-local/kube-local".into();
    amy.set_shares(DASHBOARD_ID, &[]).unwrap();
    assert!(bob.source(&mine.default_source).is_err());
}

#[test]
fn a_deleted_account_is_left_out_and_cannot_be_chosen() {
    let w = world("gone");
    let (amy, _) = amy(&w);
    let (bob, _) = env_of(&w, "bob", Role::User);
    assert!(amy.set_shares(DASHBOARD_ID, &names(&["zed"])).unwrap_err().contains("no account `zed`"));
    assert!(amy.shared_with(DASHBOARD_ID).is_empty(), "a refused list changes nothing");
    amy.set_shares(DASHBOARD_ID, &names(&["bob", "carol"])).unwrap();
    // carol's account is removed: she is no longer shown as a viewer
    w.people.names.lock().unwrap().retain(|n| n != "carol");
    assert_eq!(amy.shared_with(DASHBOARD_ID), names(&["bob"]));
    // the owner's account is removed: what they shared is gone for everyone
    assert_eq!(bob.list_dashboards().len(), 1);
    w.people.names.lock().unwrap().retain(|n| n != "amy");
    assert!(bob.list_dashboards().is_empty() && bob.load_dashboard(SID).is_err() && bob.source("share:amy/k3s-local/kube-local").is_err());
}

#[test]
fn odd_names_and_ids_are_refused_or_ignored() {
    let w = world("odd");
    let (amy, _) = amy(&w);
    for bad in ["Bob", "../etc", "bob/x", "a b", "", "-x", "bóß", &"a".repeat(40)] {
        let r = amy.set_shares(DASHBOARD_ID, &names(&["bob", bad]));
        if bad.trim().is_empty() {
            assert!(r.is_ok(), "blank entries are skipped");
        } else {
            assert!(r.is_err(), "{bad:?}");
        }
    }
    assert!(amy.set_shares(DASHBOARD_ID, &names(&["amy"])).unwrap_err().contains("already yours"));
    let many: Vec<Grant> = (0..25).map(|i| Grant::viewer(&format!("u{i}"))).collect();
    {
        let mut n = w.people.names.lock().unwrap();
        n.extend(many.iter().map(|g| g.name.clone()));
    }
    assert!(amy.set_shares(DASHBOARD_ID, &many).unwrap_err().contains("at most 20"));
    // duplicates collapse
    let c = amy.set_shares(DASHBOARD_ID, &names(&["bob", " bob ", "bob"])).unwrap();
    assert!(c.added.is_empty() && c.changed.is_empty() && c.removed.is_empty(), "bob was already there once");
    assert_eq!(amy.shared_with(DASHBOARD_ID), names(&["bob"]));
    // dashboards that are not mine, not there, or not shareable
    assert!(amy.set_shares("nope", &names(&["bob"])).is_err());
    assert!(amy.set_shares("../x", &names(&["bob"])).is_err());
    // ids
    for bad in ["share:", "share:amy", "share:../x/y", "share:amy/..", "share:amy/a b", "share:amy/a/b/c", "share:AMY/x", "share:amy//x", "shared:amy/x", ""] {
        assert!(parse_share_id(bad).is_none(), "{bad:?}");
    }
    assert_eq!(parse_share_id(SID), Some(("amy".into(), "k3s-local".into())));
    assert!(parse_share_source_id("share:amy/k3s-local").is_none());
    assert_eq!(parse_share_source_id("share:amy/k3s-local/kube-local"), Some(("amy".into(), "k3s-local".into(), "kube-local".into())));
    // a damaged or hand-edited file widens nothing and never panics
    let f = w.data.join("dash").join("shares.json");
    for body in ["", "{", "[]", "null", r#"{"shares":5}"#, r#"{"shares":[{"owner":"../x","dashboard":"d","with":["bob"]},{"owner":"amy","dashboard":"../d","with":["bob"]},{"owner":"amy","dashboard":"k3s-local","with":["Bob","bob","","a/b","amy"]}]}"#] {
        std::fs::write(&f, body).unwrap();
        let s = Shares::open(&w.data);
        let w1 = s.with_of("amy", "k3s-local");
        assert!(w1.iter().all(|x| x.name == "bob" && x.role == ShareRole::Viewer), "{body}: {w1:?}");
        assert!(s.for_viewer("bob").iter().all(|(o, d)| o == "amy" && d == "k3s-local"));
    }
    assert!(Shares::open(&w.data).for_viewer("../x").is_empty());
}

#[test]
fn only_dashboards_on_the_local_cluster_can_be_shared_and_a_later_change_ends_it() {
    let w = world("kinds");
    let (amy, _) = amy(&w);
    let (bob, _) = env_of(&w, "bob", Role::User);
    let mut prom = SourceConfig { id: "prom".into(), kind: "prometheus".into(), name: "P".into(), ..Default::default() };
    prom.values.insert("url".into(), "http://prom.internal:9090".into());
    amy.sources().save(prom).unwrap();
    let mut d = Dashboard::new("metrics", "Metrics");
    d.panels = vec![Panel::new("a", "A", "prom", Visual::Stat, "up", Grid::new(0, 0, 6, 4))];
    amy.dashboards().save(&d).unwrap();
    assert!(amy.set_shares("metrics", &names(&["bob"])).unwrap_err().contains("nothing but the local cluster"));
    // a dashboard with no source at all (text only) is harmless to share
    let mut t = Dashboard::new("notes", "Notes");
    t.panels = vec![Panel::new("t", "T", "", Visual::Text, "", Grid::new(0, 0, 6, 4))];
    amy.dashboards().save(&t).unwrap();
    assert!(amy.set_shares("notes", &names(&["bob"])).is_ok());
    // the cluster dashboard is shared, then the owner adds a panel on another source: the share stops working until she removes it
    amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    assert!(bob.load_dashboard(SID).is_ok());
    let mut cd = amy.dashboards().load(DASHBOARD_ID).unwrap();
    cd.add_panel(Panel::new("x", "X", "prom", Visual::Stat, "up", Grid::new(0, 0, 6, 4))).unwrap();
    amy.dashboards().save(&cd).unwrap();
    assert!(bob.load_dashboard(SID).is_err());
    assert!(bob.list_dashboards().iter().all(|e| e.id != SID));
    // a source of the owner that is not the cluster cannot be reached through the share id either
    assert!(bob.source("share:amy/k3s-local/prom").is_err());
    assert!(bob.source("share:amy/metrics/prom").is_err(), "not shared");
}

#[test]
fn deleting_a_dashboard_ends_its_shares_for_good() {
    let w = world("delete");
    let (amy, audit) = amy(&w);
    let (bob, _) = env_of(&w, "bob", Role::User);
    amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    assert!(amy.delete_dashboard(DASHBOARD_ID).unwrap());
    assert!(audit.lock().unwrap().last().unwrap().starts_with("dash.unshared k3s-local: bob"));
    assert!(bob.list_dashboards().is_empty());
    // a new dashboard with the same id is not shared again
    seed_defaults(&w.data, &amy.home, "amy", true).unwrap();
    let t = super::templates::find("k3s-local").unwrap();
    amy.dashboards().save(&t.instantiate("kube-local", DASHBOARD_ID)).unwrap();
    assert!(bob.load_dashboard(SID).is_err() && amy.shared_with(DASHBOARD_ID).is_empty());
}

#[test]
fn a_guest_without_the_dashboard_app_gets_nothing_even_when_named() {
    let w = world("guest");
    let (amy, _) = amy(&w);
    let (gus, _) = env_of(&w, "gus", Role::Guest);
    amy.set_shares(DASHBOARD_ID, &names(&["gus"])).unwrap();
    assert!(gus.list_dashboards().is_empty());
    assert!(gus.source("share:amy/k3s-local/kube-local").is_err());
}

#[test]
fn the_confirmation_says_in_plain_words_what_each_role_can_do() {
    let t = share_warning("Local cluster (k3s)", &names(&["bob", "carol"]), &[]);
    for must in ["Share “Local cluster (k3s)” with bob (viewer), carol (viewer)", "live state of this machine's k3s cluster", "pod", "event messages", "open ports", "Secrets, logs or a shell", "share, unshare, rename, delete or export", "source settings", "at any time"] {
        assert!(t.contains(must), "{must}: {t}");
    }
    assert!(!t.contains("Operators can also change"), "no operator, no operator text: {t}");
    let o = share_warning("X", &[Grant::operator("bob"), Grant::viewer("carol")], &[]);
    assert!(o.contains("bob (operator), carol (viewer)") && o.contains("Operators can also change this dashboard") && o.contains("previous version is kept as a backup") && o.contains("views it already shows"), "{o}");
    let up = share_warning("X", &[Grant::operator("bob")], &names(&["bob"]));
    assert!(up.starts_with("Make bob an operator instead of a viewer?") && !up.contains("Share “X”"), "{up}");
    let down = share_warning("X", &names(&["bob"]), &[Grant::operator("bob")]);
    assert!(down.starts_with("Make bob a viewer instead of an operator?") && down.contains("can no longer change"), "{down}");
    let u = share_warning("X", &names(&["carol"]), &names(&["bob", "carol"]));
    assert!(u.starts_with("Stop sharing with bob?") && !u.contains("Share “X”"), "{u}");
    assert!(share_warning("X", &[], &[]).starts_with("Nothing changes"));
}

// ------------------------------------------------------------------------------------------------------ operators

/// amy shares the cluster dashboard with bob as an operator and carol as a viewer.
fn with_roles(tag: &str) -> (World, Env, Audit, Env, Env) {
    let w = world(tag);
    let (amy, audit) = amy(&w);
    amy.set_shares(DASHBOARD_ID, &[Grant::operator("bob"), Grant::viewer("carol")]).unwrap();
    let (bob, _) = env_of(&w, "bob", Role::User);
    let (carol, _) = env_of(&w, "carol", Role::User);
    (w, amy, audit, bob, carol)
}

#[test]
fn the_dialog_text_parses_roles_and_refuses_the_odd() {
    let g = parse_grants("bob:operator, carol, dave : viewer; gus operator").unwrap();
    assert_eq!(g, vec![Grant::operator("bob"), Grant::viewer("carol"), Grant::viewer("dave"), Grant::operator("gus")]);
    assert_eq!(grants_input(&g), "bob:operator, carol, dave, gus:operator");
    assert_eq!(grants_text(&g[..2]), "bob (operator), carol (viewer)");
    assert!(parse_grants("").unwrap().is_empty() && parse_grants(" , ;").unwrap().is_empty());
    for bad in ["bob:admin", "bob:operator:x", "bob operator extra", "bob, bob", "bob:operator, bob:viewer"] {
        assert!(parse_grants(bad).is_err(), "{bad}");
    }
    // the same name with two roles never reaches the file
    let w = world("twice");
    let (amy, _) = amy(&w);
    assert!(amy.set_shares(DASHBOARD_ID, &[Grant::viewer("bob"), Grant::operator("bob")]).unwrap_err().contains("twice"));
}

#[test]
fn an_operator_changes_panels_time_and_refresh_and_it_lands_in_the_owners_dashboard() {
    let (w, amy, audit, bob, carol) = with_roles("op-edit");
    assert_eq!(bob.my_role(SID), Some(ShareRole::Operator));
    assert_eq!(carol.my_role(SID), Some(ShareRole::Viewer));
    assert_eq!(amy.shared_with(DASHBOARD_ID), vec![Grant::operator("bob"), Grant::viewer("carol")]);
    let mut d = bob.load_dashboard(SID).unwrap();
    d.remove_panel("events");
    d.panel_mut("nodes").unwrap().title = "Machines".into();
    let g = d.panel("nodes").unwrap().grid;
    d.place("nodes", Grid::new(g.x, g.y, g.w, g.h + 1));
    d.refresh_s = 10;
    d.time = TimeSpec { from: "now-6h".into(), to: "now".into() };
    // a panel on a view the dashboard already shows is fine, with the shared source id
    let mut p = Panel::new("extra", "Pods again", "share:amy/k3s-local/kube-local", Visual::Donut, "pods_by_phase", Grid::new(0, 0, 6, 5));
    p.id = "extra".into();
    d.add_panel(p).unwrap();
    bob.save_dashboard(&d).unwrap();
    // amy's file has the changes, with HER source ids and her title, id and variables
    let mine = amy.dashboards().load(DASHBOARD_ID).unwrap();
    assert!(mine.panel("events").is_none() && mine.panel("extra").is_some());
    assert_eq!(mine.panel("nodes").unwrap().title, "Machines");
    assert_eq!((mine.refresh_s, mine.time.from.as_str()), (10, "now-6h"));
    assert!(mine.panels.iter().all(|p| p.source == "kube-local") && mine.id == DASHBOARD_ID && mine.title == "Local cluster (k3s)");
    // her previous version is kept next to it
    let bak = std::fs::read_to_string(amy.dashboards().dir().join("k3s-local.json.bak")).unwrap();
    assert!(bak.contains("\"events\"") && bak.contains("\"refresh_s\": 30"));
    // the audit names the account and the role
    assert!(audit.lock().unwrap().iter().all(|l| !l.starts_with("dash.edited_shared")), "amy's own env did not log it");
    // and carol, a viewer, sees the new version at once
    assert!(carol.load_dashboard(SID).unwrap().panel("extra").is_some());
    let _ = w;
}

#[test]
fn an_operator_still_cannot_share_unshare_rename_delete_export_or_widen() {
    let (_w, amy, _a, bob, _carol) = with_roles("op-limits");
    let d = bob.load_dashboard(SID).unwrap();
    assert!(bob.set_shares(SID, &names(&["gus"])).is_err());
    assert!(bob.set_shares(SID, &[]).is_err());
    assert!(bob.delete_dashboard(SID).is_err());
    assert_eq!(amy.shared_with(DASHBOARD_ID).len(), 2, "nobody was added or removed");
    assert!(amy.dashboards().load(DASHBOARD_ID).is_ok());
    // rename is refused at save
    let mut r = d.clone();
    r.title = "Mine".into();
    assert!(bob.save_dashboard(&r).unwrap_err().contains("cannot rename"));
    // another source, a source of a different dashboard, a view the dashboard does not show, a made-up query: all refused, nothing written
    let before = std::fs::read(amy.dashboards().dir().join("k3s-local.json")).unwrap();
    for (source, query) in [("kube-local", "nodes"), ("prom", "nodes"), ("share:amy/other/kube-local", "nodes"), ("share:carol/k3s-local/kube-local", "nodes"), ("share:amy/k3s-local/kube-local", "node_states"), ("share:amy/k3s-local/kube-local", "delete pods"), ("share:amy/k3s-local/kube-local", ""), ("share:amy/k3s-local/kube-local", "secrets")] {
        let mut x = d.clone();
        x.add_panel(Panel::new("bad", "Bad", source, Visual::Table, query, Grid::new(0, 0, 6, 4))).unwrap();
        assert!(bob.save_dashboard(&x).is_err(), "{source} / {query}");
        let mut y = d.clone();
        y.panels[0].queries.push(QueryDef { source: source.into(), query: PanelQuery { text: query.into(), ..Default::default() } });
        assert!(bob.save_dashboard(&y).is_err(), "query B: {source} / {query}");
    }
    // the dashboard's variables, description and default source are the owner's: a changed copy saves without touching them
    let mut z = d.clone();
    z.description = "mine".into();
    z.default_source = "prom".into();
    z.variables.clear();
    bob.save_dashboard(&z).unwrap();
    let after = amy.dashboards().load(DASHBOARD_ID).unwrap();
    assert_eq!(after.default_source, "kube-local");
    assert!(!after.description.contains("mine"));
    let _ = before;
    // export and rename of the open copy are not saved anywhere; his own dashboards are untouched
    assert!(bob.dashboards().list().is_empty());
    let bundle: serde_json::Value = serde_json::from_str(&Dashboards::export_bundle(&d, &bob.sources().list())).unwrap();
    assert!(bundle["sources"].as_array().unwrap().is_empty(), "he has no source settings to export");
}

#[test]
fn a_viewer_cannot_save_and_a_stranger_cannot_either() {
    let (w, amy, _a, _bob, carol) = with_roles("viewer-save");
    let mut d = carol.load_dashboard(SID).unwrap();
    d.remove_panel("events");
    assert!(carol.save_dashboard(&d).unwrap_err().contains("viewer"));
    assert!(amy.dashboards().load(DASHBOARD_ID).unwrap().panel("events").is_some());
    let (gus, _) = env_of(&w, "gus", Role::User);
    assert!(gus.save_dashboard(&d).unwrap_err().contains("not shared with you"));
    assert!(gus.my_role(SID).is_none());
}

#[test]
fn roles_can_be_changed_and_removed_later_and_take_effect_at_once() {
    let (_w, amy, audit, bob, carol) = with_roles("roles");
    let mut d = bob.load_dashboard(SID).unwrap();
    d.remove_panel("events");
    // amy makes bob a viewer: his next save is refused
    let ch = amy.set_shares(DASHBOARD_ID, &[Grant::viewer("bob"), Grant::viewer("carol")]).unwrap();
    assert_eq!((ch.added.len(), ch.changed.clone(), ch.removed.len()), (0, vec![Grant::viewer("bob")], 0));
    assert!(bob.save_dashboard(&d).unwrap_err().contains("viewer"));
    // and carol an operator: her save works
    amy.set_shares(DASHBOARD_ID, &[Grant::viewer("bob"), Grant::operator("carol")]).unwrap();
    let mut c = carol.load_dashboard(SID).unwrap();
    c.refresh_s = 60;
    carol.save_dashboard(&c).unwrap();
    assert_eq!(amy.dashboards().load(DASHBOARD_ID).unwrap().refresh_s, 60);
    // removing her ends it
    amy.set_shares(DASHBOARD_ID, &names(&["bob"])).unwrap();
    assert!(carol.save_dashboard(&c).is_err());
    let a = audit.lock().unwrap().clone();
    assert!(a.contains(&"dash.shared k3s-local: bob (operator), carol (viewer)".to_string()), "{a:?}");
    assert!(a.contains(&"dash.share_role k3s-local: bob (viewer)".to_string()) && a.contains(&"dash.share_role k3s-local: carol (operator)".to_string()), "{a:?}");
    assert!(a.contains(&"dash.unshared k3s-local: carol".to_string()));
    assert!(a.iter().all(|l| !l.contains("kube-local") && !l.contains("http")), "names and roles only");
}

#[test]
fn an_owner_who_edits_at_the_same_time_is_never_overwritten_silently() {
    let (_w, amy, _a, bob, _c) = with_roles("conflict");
    // bob opens it; amy then saves a change of her own; bob's save is refused with a clear message and amy's file is exactly as she left it
    let mut d = bob.load_dashboard(SID).unwrap();
    d.refresh_s = 10;
    let mut mine = amy.dashboards().load(DASHBOARD_ID).unwrap();
    mine.refresh_s = 120;
    amy.dashboards().save(&mine).unwrap();
    let e = bob.save_dashboard(&d).unwrap_err();
    assert!(e.contains("amy changed this dashboard after you opened it") && e.contains("nothing was saved"), "{e}");
    assert_eq!(amy.dashboards().load(DASHBOARD_ID).unwrap().refresh_s, 120);
    // he opens it again and redoes his change
    let mut d = bob.load_dashboard(SID).unwrap();
    d.refresh_s = 10;
    bob.save_dashboard(&d).unwrap();
    assert_eq!(amy.dashboards().load(DASHBOARD_ID).unwrap().refresh_s, 10);
    // a second save from the same copy works (his own save is the new base); one that was never opened here is refused
    d.refresh_s = 20;
    bob.save_dashboard(&d).unwrap();
    // an operator who never opened it cannot save over it
    let (w2, amy2, _a2, bob2, _c2) = with_roles("conflict-unopened");
    let mut never = amy2.dashboards().load(DASHBOARD_ID).unwrap();
    never.id = SID.into();
    assert!(bob2.save_dashboard(&never).unwrap_err().contains("after you opened it"));
    let _ = w2;
    // amy saving over bob's change keeps bob's version as the backup
    let mut mine = amy.dashboards().load(DASHBOARD_ID).unwrap();
    mine.refresh_s = 300;
    amy.dashboards().save(&mine).unwrap();
    let bak = std::fs::read_to_string(amy.dashboards().dir().join("k3s-local.json.bak")).unwrap();
    assert!(bak.contains("\"refresh_s\": 20"));
    // deleting her dashboard removes the backup too
    amy.delete_dashboard(DASHBOARD_ID).unwrap();
    assert!(!amy.dashboards().dir().join("k3s-local.json.bak").exists());
}

#[test]
fn a_file_from_before_roles_means_viewers_and_a_damaged_role_list_widens_nothing() {
    let w = world("compat");
    let (_amy, _) = amy(&w);
    let f = w.data.join("dash").join("shares.json");
    std::fs::write(&f, r#"{"schema":1,"shares":[{"owner":"amy","dashboard":"k3s-local","with":["bob","carol"]}]}"#).unwrap();
    let s = Shares::open(&w.data);
    assert_eq!(s.with_of("amy", "k3s-local"), names(&["bob", "carol"]));
    assert_eq!(s.role_of("amy", "k3s-local", "bob"), Some(ShareRole::Viewer));
    // an operator who is not in `with`, a duplicate, a bad name, a role list that is not a list: nobody becomes an operator by accident
    for body in [
        r#"{"shares":[{"owner":"amy","dashboard":"k3s-local","with":["bob"],"operators":["carol","../x","bob","bob"]}]}"#,
        r#"{"shares":[{"owner":"amy","dashboard":"k3s-local","with":["bob"],"operators":"bob"}]}"#,
        r#"{"shares":[{"owner":"amy","dashboard":"k3s-local","with":["bob"],"operators":[1,null,{"a":1}]}]}"#,
    ] {
        std::fs::write(&f, body).unwrap();
        let g = Shares::open(&w.data).with_of("amy", "k3s-local");
        assert!(g.iter().all(|x| x.name == "bob"), "{body}: {g:?}");
        assert!(Shares::open(&w.data).role_of("amy", "k3s-local", "carol").is_none(), "{body}");
        assert!(Shares::open(&w.data).role_of("amy", "k3s-local", "bob").is_some());
    }
    // the first body keeps bob an operator and nothing else
    std::fs::write(&f, r#"{"shares":[{"owner":"amy","dashboard":"k3s-local","with":["bob"],"operators":["carol","bob"]}]}"#).unwrap();
    assert_eq!(Shares::open(&w.data).with_of("amy", "k3s-local"), vec![Grant::operator("bob")]);
    // an old reader's view of a new file: everybody is in `with`
    let s = Shares::open(&w.data);
    s.set("amy", "k3s-local", &[Grant::operator("bob"), Grant::viewer("carol")]).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(raw["shares"][0]["with"], serde_json::json!(["bob", "carol"]));
    assert_eq!(raw["shares"][0]["operators"], serde_json::json!(["bob"]));
}

#[test]
fn an_operator_sees_only_the_views_the_dashboard_shows_when_running_panels() {
    let (_w, _amy, _a, bob, _c) = with_roles("op-views");
    let d = bob.load_dashboard(SID).unwrap();
    let (_, src) = bob.source(&d.panels[0].source).unwrap();
    let cancel = Cancel::new();
    assert!(src.query(&QueryRequest::new("nodes", range()), &cancel).is_ok());
    for q in ["node_states", "delete pods", "get secrets"] {
        assert!(src.query(&QueryRequest::new(q, range()), &cancel).is_err(), "{q}");
    }
}

#[test]
fn an_owner_is_asked_before_a_save_replaces_an_operators_change_unless_a_run_is_unattended() {
    let (_w, amy, _a, bob, _c) = with_roles("overwrite");
    // amy opens her dashboard; bob (an operator) saves a change; amy's own save would replace it, so she is asked
    let mine = amy.load_dashboard(DASHBOARD_ID).unwrap();
    assert!(amy.overwrite_warning(&mine).is_none(), "nothing changed yet");
    let mut b = bob.load_dashboard(SID).unwrap();
    b.refresh_s = 90;
    bob.save_dashboard(&b).unwrap();
    let w = amy.overwrite_warning(&mine).expect("an operator saved after amy opened it");
    assert!(w.contains("replaces their change"), "{w}");
    // an unattended run (Autopilot) goes through without asking; the previous version is still kept as a backup
    crate::dash::share::set_unattended(true);
    assert!(amy.overwrite_warning(&mine).is_none());
    crate::dash::share::set_unattended(false);
    // once she has saved (or opened it again) she is not asked again
    amy.dashboards().save(&mine).unwrap();
    amy.note_saved(DASHBOARD_ID);
    assert!(amy.overwrite_warning(&mine).is_none());
    // a dashboard nobody is shared on never asks
    amy.set_shares(DASHBOARD_ID, &[]).unwrap();
    assert!(amy.overwrite_warning(&mine).is_none());
}
