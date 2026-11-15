use super::*;
use std::sync::Mutex;

struct Inner;
impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("inner got {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

struct Fake {
    own: bool,
    shared: bool,
    skills: Mutex<Vec<Skill>>,
    audit: Mutex<Vec<String>>,
}

impl Fake {
    fn new(own: bool, shared: bool) -> Arc<Fake> {
        let sk = |scope: Scope, n: &str| Skill { scope, name: n.into(), description: format!("about {n}"), triggers: vec!["tea".into()], body: "Do the thing.".into() };
        Arc::new(Fake { own, shared, skills: Mutex::new(vec![sk(Scope::User("ann".into()), "mine-one"), sk(Scope::Shared, "team-one")]), audit: Mutex::new(vec![]) })
    }
}

impl Backend for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn can_own(&self) -> bool {
        self.own
    }
    fn can_shared(&self) -> bool {
        self.shared
    }
    fn list(&self) -> Vec<Skill> {
        self.skills.lock().unwrap().clone()
    }
    fn add(&self, scope: Scope, markdown: &str) -> Result<Skill, String> {
        let s = skills::parse(markdown, scope).map_err(|e| e.to_string())?;
        self.skills.lock().unwrap().push(s.clone());
        Ok(s)
    }
    fn remove(&self, scope: &Scope, name: &str) -> Result<(), String> {
        self.skills.lock().unwrap().retain(|s| !(s.name == name && &s.scope == scope));
        Ok(())
    }
    fn audit(&self, a: &str, s: &str) {
        self.audit.lock().unwrap().push(format!("{a} {s}"));
    }
}

const GOOD: &str = "---\nname: kettle\ndescription: how to make tea\ntriggers: tea, kettle\n---\nFill, boil, pour.\n";

fn tools(f: &Arc<Fake>) -> SkillsTools {
    SkillsTools::new(Arc::new(Inner), f.clone())
}

#[test]
fn the_list_is_free_and_adding_and_removing_ask_every_time_with_no_standing_grant() {
    let f = Fake::new(true, false);
    let t = tools(&f);
    let d = t.defs();
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).unwrap().risk;
    assert_eq!((risk("skills_list"), risk("skill_add"), risk("skill_remove")), (Risk::Safe, Risk::Ask, Risk::Ask));
    for n in ["skill_add", "skill_remove"] {
        assert!(t.grant_blocker(n, &json!({})).is_some() && !t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert!(d.iter().find(|x| x.spec.name == "skills_list").unwrap().untrusted, "skill texts come from files");
    let said = t.call("skills_list", &json!({})).unwrap();
    assert!(said.contains("mine-one (mine)") && said.contains("team-one (shared)") && said.contains("tea"), "{said}");
    assert_eq!(t.call("other", &json!({})).unwrap_err(), "inner got other");
}

#[test]
fn a_skill_is_added_after_the_text_is_checked_and_shown_and_removed_by_name() {
    let f = Fake::new(true, false);
    let t = tools(&f);
    let a = json!({"text": GOOD});
    let card = t.preview("skill_add", &a).unwrap();
    assert!(card.contains("you only") && card.contains("Name: kettle") && card.contains("Fill, boil, pour.") && card.contains("tea, kettle"), "{card}");
    assert!(t.describe("skill_add", &a).contains("called “kettle”"));
    assert!(f.skills.lock().unwrap().iter().all(|s| s.name != "kettle"), "the card changes nothing");
    assert!(t.call("skill_add", &a).unwrap().contains("Added the skill “kettle”"));
    assert_eq!(f.audit.lock().unwrap().as_slice(), ["skill.add kettle"]);
    // refusals before anything is asked
    for (bad, want) in [(json!({}), "markdown"), (json!({"text": "no front matter"}), ""), (json!({"text": GOOD}), "exists already"), (json!({"text": GOOD, "shared": true}), "manages the shared"), (json!({"text": "x".repeat(70_000)}), "64 KiB")] {
        let e = t.refusal("skill_add", &bad).unwrap_or_else(|| panic!("{}", bad.to_string().chars().take(60).collect::<String>()));
        assert!(e.contains(want), "{e}");
    }
    assert!(t.call("skill_remove", &json!({"name": "nobody"})).unwrap_err().contains("no mine skill called nobody"));
    assert!(t.call("skill_remove", &json!({"name": "team-one", "shared": true})).unwrap_err().contains("manages the shared"));
    assert!(t.call("skill_remove", &json!({"name": "kettle"})).unwrap().contains("Removed"));
    assert_eq!(f.audit.lock().unwrap().as_slice(), ["skill.add kettle", "skill.delete kettle"]);
}

#[test]
fn an_account_that_manages_shared_skills_may_change_them_and_one_that_may_not_keep_its_own_cannot_add() {
    let f = Fake::new(true, true);
    let t = tools(&f);
    let a = json!({"text": GOOD, "shared": true});
    assert!(t.preview("skill_add", &a).unwrap().contains("everyone"));
    t.call("skill_add", &a).unwrap();
    assert_eq!(f.audit.lock().unwrap().as_slice(), ["skill.shared.add kettle"]);
    assert!(t.call("skill_remove", &json!({"name": "kettle", "shared": true})).is_ok());
    let none = Fake::new(false, false);
    let t = tools(&none);
    assert!(t.refusal("skill_add", &json!({"text": GOOD})).unwrap().contains("may not keep skills"));
    assert!(t.call("skills_list", &json!({})).is_ok(), "reading is always free");
}

#[test]
fn the_real_backend_uses_the_skill_store_the_policy_and_the_audit_log() {
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let dir = std::env::temp_dir().join(format!("iyakki-skilltools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = crate::services::Services::open(&dir, Arc::new(Off), Arc::new(Off)).unwrap();
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let mut policy = iyakki_sec::policy::Role::User.template();
    policy.skills = true;
    policy.manage_skills = false;
    let be = Arc::new(RealBackend { svc: svc.clone(), policy, user: "ann".into(), audit: Arc::new(move |a, s| a2.lock().unwrap().push(format!("{a} {s}"))) });
    let t = SkillsTools::new(Arc::new(Inner), be);
    t.call("skill_add", &json!({"text": GOOD})).unwrap();
    assert!(svc.skills.list("ann").iter().any(|s| s.name == "kettle"), "kept in the account's skill store, the one the tile reads");
    assert!(t.call("skills_list", &json!({})).unwrap().contains("kettle (mine)"));
    assert!(t.call("skill_add", &json!({"text": GOOD, "shared": true})).is_err(), "a user may not add a shared one");
    t.call("skill_remove", &json!({"name": "kettle"})).unwrap();
    assert!(svc.skills.list("ann").iter().all(|s| s.name != "kettle"));
    assert_eq!(audit.lock().unwrap().as_slice(), ["skill.add kettle", "skill.delete kettle"]);
    assert!(PROMPT_RULE.contains("skill_add") && PROMPT_RULE.contains("skill_remove") && PROMPT_RULE.contains("skills_list"));
}
