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
    manage: bool,
    stored: Mutex<Vec<StoredServer>>,
    file: Vec<ServerSpec>,
    audit: Mutex<Vec<String>>,
}

impl Fake {
    fn new(manage: bool) -> Arc<Fake> {
        let spec = |n: &str| ServerSpec { name: n.into(), command: "/usr/bin/from-file".into(), args: vec![], env: Default::default() };
        Arc::new(Fake { manage, stored: Mutex::new(vec![StoredServer { name: "sample".into(), command: "/bin/true".into(), args: vec![], sample: true }]), file: vec![spec("fromfile")], audit: Mutex::new(vec![]) })
    }
}

impl Backend for Fake {
    fn can_manage(&self) -> bool {
        self.manage
    }
    fn servers(&self) -> Vec<(ServerSpec, &'static str)> {
        let stored = self.stored.lock().unwrap().clone();
        let mut policy = iyakki_sec::policy::Role::Admin.template();
        policy.mcp_servers = vec!["*".into()];
        mcp::visible_servers(&self.file, &stored, &policy).into_iter().map(|s| { let o = mcp::origin_of(&stored, &s.name); (s, o) }).collect()
    }
    fn add(&self, s: StoredServer) -> Result<(), String> {
        self.stored.lock().unwrap().push(s);
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.stored.lock().unwrap().retain(|s| s.name != name);
        Ok(())
    }
    fn audit(&self, a: &str, s: &str) {
        self.audit.lock().unwrap().push(format!("{a} {s}"));
    }
}

fn tools(f: &Arc<Fake>) -> McpAdminTools {
    McpAdminTools::new(Arc::new(Inner), f.clone())
}

#[test]
fn the_list_is_free_and_adding_and_removing_ask_every_time_with_no_standing_grant() {
    let f = Fake::new(true);
    let t = tools(&f);
    let d = t.defs();
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).unwrap().risk;
    assert_eq!((risk("mcp_servers"), risk("mcp_server_add"), risk("mcp_server_remove")), (Risk::Safe, Risk::Ask, Risk::Ask));
    for n in ["mcp_server_add", "mcp_server_remove"] {
        assert!(t.grant_blocker(n, &json!({})).is_some() && !t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert_eq!(t.call("something_else", &json!({})).unwrap_err(), "inner got something_else");
    let said = t.call("mcp_servers", &json!({})).unwrap();
    assert!(said.contains("fromfile (mcp.json)") && said.contains("sample (sample)"), "{said}");
    assert!(d.iter().find(|x| x.spec.name == "mcp_servers").unwrap().untrusted, "names and commands come from files");
}

#[test]
fn a_server_is_added_after_the_checks_and_removed_only_when_it_was_added_here() {
    let f = Fake::new(true);
    let t = tools(&f);
    let a = json!({"name": "notes", "command": "/usr/bin/mcp-notes", "args": ["--db", "a.db"]});
    let card = t.preview("mcp_server_add", &a).unwrap();
    assert!(card.contains("/usr/bin/mcp-notes") && card.contains("--db a.db") && card.contains("with your rights"), "{card}");
    assert!(t.describe("mcp_server_add", &a).contains("starts the program /usr/bin/mcp-notes"));
    assert!(f.stored.lock().unwrap().iter().all(|s| s.name != "notes"), "the card changes nothing");
    assert!(t.call("mcp_server_add", &a).unwrap().contains("mcp__notes__TOOL"));
    assert_eq!(f.stored.lock().unwrap().iter().find(|s| s.name == "notes").unwrap().args, ["--db", "a.db"]);
    assert_eq!(f.audit.lock().unwrap().as_slice(), ["mcp.add notes"]);
    // refusals before anything is asked
    for (bad, want) in [(json!({"command": "/x"}), "name"), (json!({"name": "bad name!", "command": "/x"}), "letters"), (json!({"name": "ok"}), "command"), (json!({"name": "ok", "command": ""}), "command"), (json!({"name": "ok", "command": "/x", "args": "a b"}), "list of text"), (json!({"name": "ok", "command": "/x", "args": [1]}), "text"), (json!({"name": "notes", "command": "/x"}), "exists already"), (json!({"name": "fromfile", "command": "/x"}), "exists already")] {
        let e = t.refusal("mcp_server_add", &bad).unwrap_or_else(|| panic!("{bad}"));
        assert!(e.contains(want), "{bad}: {e}");
        assert!(t.call("mcp_server_add", &bad).is_err());
    }
    // removal: the one added here goes, the administrator's file entry stays, an unknown one is said
    assert!(t.refusal("mcp_server_remove", &json!({"name": "fromfile"})).unwrap().contains("mcp.json"));
    assert!(t.call("mcp_server_remove", &json!({"name": "fromfile"})).is_err());
    assert!(t.call("mcp_server_remove", &json!({"name": "nobody"})).unwrap_err().contains("no server called"));
    assert!(t.call("mcp_server_remove", &json!({"name": "notes"})).unwrap().contains("Removed"));
    assert!(f.stored.lock().unwrap().iter().all(|s| s.name != "notes"));
    assert_eq!(f.audit.lock().unwrap().as_slice(), ["mcp.add notes", "mcp.remove notes"]);
}

#[test]
fn an_account_that_may_not_manage_servers_can_list_them_and_nothing_else() {
    let f = Fake::new(false);
    let t = tools(&f);
    assert!(t.call("mcp_servers", &json!({})).is_ok());
    let a = json!({"name": "notes", "command": "/usr/bin/mcp-notes"});
    assert!(t.refusal("mcp_server_add", &a).unwrap().contains("only an administrator"));
    assert!(t.call("mcp_server_add", &a).is_err() && t.call("mcp_server_remove", &json!({"name": "sample"})).is_err());
    assert_eq!(f.stored.lock().unwrap().len(), 1);
    assert!(f.audit.lock().unwrap().is_empty());
}

#[test]
fn the_real_backend_uses_the_services_store_the_policy_and_the_same_filter_as_the_tile() {
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let dir = std::env::temp_dir().join(format!("iyakki-mcpadmin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = crate::services::Services::open(&dir, Arc::new(Off), Arc::new(Off)).unwrap();
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let mut policy = iyakki_sec::policy::Role::Admin.template();
    policy.mcp_servers = vec!["notes".into(), "fromfile".into()];
    let spec = ServerSpec { name: "fromfile".into(), command: "/usr/bin/x".into(), args: vec![], env: Default::default() };
    let be = Arc::new(RealBackend { svc: svc.clone(), policy, user: "admin".into(), file_servers: vec![spec], audit: Arc::new(move |a, s| a2.lock().unwrap().push(format!("{a} {s}"))) });
    let t = McpAdminTools::new(Arc::new(Inner), be);
    t.call("mcp_server_add", &json!({"name": "notes", "command": "/usr/bin/mcp-notes"})).unwrap();
    t.call("mcp_server_add", &json!({"name": "hidden", "command": "/usr/bin/h"})).unwrap();
    assert!(svc.mcp.list().iter().any(|s| s.name == "notes"), "kept in the account's store, the file the tile reads");
    let said = t.call("mcp_servers", &json!({})).unwrap();
    assert!(said.contains("notes (added here)") && said.contains("fromfile (mcp.json)") && !said.contains("hidden"), "the policy hides what the account may not use: {said}");
    assert_eq!(audit.lock().unwrap().as_slice(), ["mcp.add notes", "mcp.add hidden"]);
    t.call("mcp_server_remove", &json!({"name": "notes"})).unwrap();
    assert!(svc.mcp.list().iter().all(|s| s.name != "notes"));
    assert!(PROMPT_RULE.contains("mcp_server_add") && PROMPT_RULE.contains("mcp_server_remove") && PROMPT_RULE.contains("mcp_servers"));
}
