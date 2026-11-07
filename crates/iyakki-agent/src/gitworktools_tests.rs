//! The assistant's git and workspace tools: a clone (with or without submodules) and an AI IDE workspace each ask every time, say plainly
//! what they do, refuse what the account may not do, and never pass a secret through the chat. The git program is a fake script or a
//! local repository; nothing here touches a network or a real cluster.

use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{App, Policy, Role};
use serde_json::{json, Value};

use crate::agent::{hard_ask, start_with, Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::assign::tests_support::NoTools;
use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
use crate::gitjobs::Ctx;
use crate::gitkeys::{GitKeys, MemStore, Session, Who};
use crate::gitworktools::*;

const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

struct Rig {
    base: PathBuf,
    env_dir: PathBuf,
    repos: PathBuf,
    tools: GitWorkTools,
    env: Arc<GitWorkEnv>,
    policy: Policy,
    audit: Arc<Mutex<Vec<String>>>,
}

fn fake_git(dir: &Path) -> PathBuf {
    let log = dir.join("calls.log");
    let p = dir.join("fakegit");
    std::fs::write(
        &p,
        format!(
            r#"#!/bin/sh
echo "ARGV: $*" >> '{log}'
case "$*" in *failme*) echo "fatal: repository not found" >&2; exit 128;; esac
case "$1" in
  clone)
    for a; do last=$a; done
    mkdir -p "$last/docs" && echo hi > "$last/README" && echo "Deploy with k3s: run the installer" > "$last/docs/DEPLOY.md"
    case "$*" in *--recurse-submodules*) printf '[submodule "a"]\n\tpath = a\n\turl = https://git.example.org/ops/a.git\n[submodule "b"]\n\tpath = b\n\turl = ../b.git\n' > "$last/.gitmodules";; esac
    echo "Receiving objects: 100% (3/3), done." >&2 ;;
esac
exit 0
"#,
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn policy_for(base: &Path) -> Policy {
    let mut p = Role::Admin.template();
    p.fs_read = vec![base.to_string_lossy().into_owned()];
    p.fs_write = vec![base.to_string_lossy().into_owned()];
    p
}

fn rig_with(tag: &str, edit: impl FnOnce(&mut Policy, &Path), allow_local: bool) -> Rig {
    build(tag, edit, allow_local, false)
}

fn build(tag: &str, edit: impl FnOnce(&mut Policy, &Path), allow_local: bool, real_git: bool) -> Rig {
    let base = std::fs::canonicalize(crate::testdir::short("gw", tag)).unwrap();
    let home = base.join("home");
    let repos = home.join("Repositories");
    let env_dir = base.join("svc");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&env_dir).unwrap();
    let mut policy = policy_for(&base);
    edit(&mut policy, &base);
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let audit_fn: crate::gitkeys::AuditFn = Arc::new(move |a, b| a2.lock().unwrap().push(format!("{a} {b}")));
    let jobs = Arc::new(crate::jobs::Jobs::new(&base.join("jobs")));
    let git = if real_git { crate::ssh::find_program("git", &std::env::var("PATH").unwrap()).unwrap() } else { fake_git(&base) };
    let (b2, p2, af2) = (base.clone(), policy.clone(), audit_fn.clone());
    let policy_copy = policy.clone();
    let make_ctx: Arc<dyn Fn(&Path, &str) -> Result<Ctx, String> + Send + Sync> = Arc::new(move |root, name| {
        let keys = Arc::new(GitKeys::new(&b2.join("ssh"), Arc::new(MemStore::default()), af2.clone(), Arc::new(Session::default())));
        let (pol, user) = (p2.clone(), "alice".to_string());
        Ok(Ctx {
            keys,
            tools: crate::gitjobs::Tools { git: git.clone(), ssh: Err("no ssh in tests".into()), allow_local },
            owner: "alice".into(),
            workspace: name.to_string(),
            root: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            ws_keys: Default::default(),
            who: Who::Assistant,
            run_dir: b2.join("run"),
            probe: Arc::new(|_, _| Err("no network in tests".into())),
            scope: Arc::new(move |p: &Path| crate::scope::check_write(&pol, &user, p)),
        })
    });
    let env = GitWorkEnv {
        user: "alice".into(),
        policy,
        audit: audit_fn,
        dir: env_dir.clone(),
        home,
        repos_root: repos.clone(),
        extra_write: None,
        make_ctx,
        tick: Arc::new(move |b| b.tick(&jobs)),
        runner: Arc::new(crate::wsexec::FakeRunner::new(&[])),
        wait: Duration::from_secs(30),
    };
    let env = Arc::new(env);
    Rig { base, env_dir, repos, tools: GitWorkTools::with_env(Arc::new(NoTools), env.clone()), env, policy: policy_copy, audit }
}

fn rig(tag: &str) -> Rig {
    rig_with(tag, |_, _| {}, false)
}

fn clone_args() -> Value {
    json!({"url": "https://git.example.org/ops/infra.git", "submodules": true})
}

fn audit_text(r: &Rig) -> String {
    r.audit.lock().unwrap().join("\n")
}

fn git_log(r: &Rig) -> String {
    std::fs::read_to_string(r.base.join("calls.log")).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ risk and roles

#[test]
fn the_clone_and_the_workspace_add_always_ask_and_the_lists_are_free() {
    let r = rig("risk");
    let defs = r.tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing")).risk;
    for t in WRITE_TOOLS {
        assert_eq!(risk(t), Risk::Ask, "{t}");
        assert!(hard_ask(t, &json!({})).is_some(), "{t} asks even in Auto mode");
        assert!(!r.tools.approved_always(t), "{t}: Allow for this session does not remember it");
        assert!(r.tools.grant_blocker(t, &json!({})).is_some(), "{t}");
        assert!(!r.tools.pre_approved(t), "{t}");
        assert_eq!(crate::toolblock::group_of(t), crate::toolblock::Group::Workspace);
    }
    for t in READ_TOOLS {
        assert_eq!(risk(t), Risk::Safe, "{t}");
        assert!(hard_ask(t, &json!({})).is_none() && r.tools.grant_blocker(t, &json!({})).is_none(), "{t}");
        assert!(r.tools.preview(t, &json!({})).is_none() && r.tools.refusal(t, &json!({})).is_none());
    }
    assert!(is_gitwork_tool("git_clone_repo") && !is_gitwork_tool("git_service_status"));
    assert_eq!(crate::runs::approval_risk("git_clone_repo"), "medium");
    // the request in the chat reaches the group that holds the tools
    assert!(crate::toolblock::intent_groups("create a workspace from this repo, clone submodules").contains(&crate::toolblock::Group::Workspace));
    // submodules are off unless asked for
    let s = defs.iter().find(|d| d.spec.name == "git_clone_repo").unwrap();
    assert_eq!(s.spec.schema["required"], json!(["url"]));
    assert!(PROMPT_RULE.contains("submodules") && PROMPT_RULE.contains("never ask for or send a token"));
}

#[test]
fn an_account_without_the_git_tile_or_the_ide_is_not_offered_the_tools_and_is_refused_if_they_are_called() {
    let r = rig_with("role", |p, _| { p.apps.remove(&App::Git); p.apps.remove(&App::Ide); }, false);
    assert!(!offered(&r.policy));
    let names: Vec<String> = r.tools.defs().iter().map(|d| d.spec.name.clone()).collect();
    assert!(!names.iter().any(|n| is_gitwork_tool(n)), "{names:?}");
    assert!(r.tools.refusal("git_clone_repo", &clone_args()).unwrap().contains("may not use Git"));
    assert!(r.tools.refusal("ide_workspace_add", &json!({"path": "x"})).unwrap().contains("may not use the AI IDE"));
    assert!(r.tools.call("git_clones_list", &json!({})).unwrap_err().contains("may not use Git"));
    assert!(r.tools.call("git_clone_repo", &clone_args()).is_err());
    assert!(!r.repos.exists(), "nothing was made");
    // Git without the IDE: the clone tools only
    let r = rig_with("role2", |p, _| { p.apps.remove(&App::Ide); }, false);
    let names: Vec<String> = r.tools.defs().iter().map(|d| d.spec.name.clone()).collect();
    assert!(names.contains(&"git_clone_repo".to_string()) && !names.contains(&"ide_workspace_add".to_string()), "{names:?}");
}

// ------------------------------------------------------------------------------------------------ the cards

#[test]
fn the_clone_card_says_what_from_which_host_into_which_folder_with_or_without_submodules() {
    let r = rig("card");
    let with = r.tools.preview("git_clone_repo", &clone_args()).unwrap();
    for w in ["Clone ops/infra from git.example.org", "with submodules", "https://git.example.org/ops/infra.git", &format!("{}/infra", r.repos.display()), "ALSO cloned", ".gitmodules", "over https only", "Signs in with: nothing (no saved Git connection for this host", "nothing on the host changes"] {
        assert!(with.contains(w), "missing `{w}` in\n{with}");
    }
    let without = r.tools.preview("git_clone_repo", &json!({"url": "https://git.example.org/ops/infra.git", "branch": "dev", "depth": 1})).unwrap();
    assert!(without.contains("without submodules") && without.contains("not cloned") && !without.contains("ALSO cloned"), "{without}");
    assert!(without.contains("Branch: dev; history: only the last 1 commit(s)"), "{without}");
    assert_eq!(r.tools.describe("git_clone_repo", &clone_args()), with.lines().next().unwrap());
    assert!(r.tools.describe("git_clone_repo", &clone_args()).starts_with("Clone ops/infra from git.example.org into your Repositories folder, with submodules"));
    // nothing was started or made by looking
    assert!(!r.repos.exists() && git_log(&r).is_empty());
    // a folder name that is taken is not reused: the card shows the one that will be used
    std::fs::create_dir_all(r.repos.join("infra/x")).unwrap();
    std::fs::write(r.repos.join("infra/x/f"), "x").unwrap();
    assert!(r.tools.preview("git_clone_repo", &clone_args()).unwrap().contains(&format!("{}/infra-2", r.repos.display())));
}

#[test]
fn the_workspace_card_names_the_folder_and_the_name() {
    let r = rig("wscard");
    std::fs::create_dir_all(r.repos.join("infra")).unwrap();
    let c = r.tools.preview("ide_workspace_add", &json!({"path": "infra", "name": "Infra"})).unwrap();
    assert!(c.starts_with("Add the folder ") && c.contains(&format!("{}/infra", r.repos.display())) && c.contains("“Infra”") && c.contains("nothing in the folder changes"), "{c}");
    assert!(r.tools.describe("ide_workspace_add", &json!({"path": "infra"})).contains("“infra”"));
    assert!(crate::workspace::Registry::load(&r.env_dir, "alice").items.is_empty(), "looking changes nothing");
}

// ------------------------------------------------------------------------------------------------ refusals

#[test]
fn forbidden_addresses_and_secrets_are_refused_without_being_echoed() {
    let r = rig("refuse");
    let bad = |a: Value| r.tools.refusal("git_clone_repo", &a).unwrap_or_else(|| panic!("{a} was not refused"));
    assert!(bad(json!({"url": "http://git.example.org/a/b.git"})).contains("not encrypted"));
    assert!(bad(json!({"url": "ssh://git@git.example.org/a/b.git"})).contains("Git tile"));
    assert!(bad(json!({"url": "git@git.example.org:a/b.git"})).contains("Git tile"));
    assert!(!bad(json!({"url": "file:///etc"})).is_empty(), "file:// is not an address");
    assert!(!bad(json!({"url": "/etc"})).is_empty());
    assert!(!bad(json!({"url": "ext::sh -c touch% /tmp/x"})).is_empty());
    assert!(!bad(json!({"url": "--upload-pack=x"})).is_empty());
    assert!(bad(json!({"url": "https://u:pw@git.example.org/a/b.git"})).contains("cannot be part of an address"));
    assert!(bad(json!({"url": "https://git.example.org/a/b.git", "branch": "-x"})).contains("not a branch name"));
    assert!(bad(json!({"url": "https://git.example.org/a/b.git", "depth": 0})).contains("depth"));
    assert!(bad(json!({"url": "https://git.example.org/a/b.git", "connection": "Nope"})).contains("no Git connection called Nope"));
    // a credential in any field
    let e = bad(json!({"url": format!("https://git.example.org/a/b.git?x={TOKEN}")}));
    assert!(e.contains("no `?` or `#`") && !e.contains(TOKEN), "{e}");
    for a in [json!({"url": "https://git.example.org/a/b.git", "branch": TOKEN}), json!({"url": "https://git.example.org/a/b.git", "connection": TOKEN})] {
        let e = bad(a);
        assert!(e.contains("Never send secrets") && !e.contains(TOKEN), "{e}");
    }
    let e = r.tools.refusal("ide_workspace_add", &json!({"path": "x", "name": TOKEN})).unwrap();
    assert!(e.contains("Never send secrets") && !e.contains(TOKEN));
    assert!(r.tools.call("git_clone_repo", &json!({"url": "https://git.example.org/a/b.git", "branch": TOKEN})).is_err());
    assert!(git_log(&r).is_empty() && !audit_text(&r).contains(TOKEN));
    // a host the account may not reach
    let r = rig_with("host", |p, _| p.net_hosts = vec!["other.example.org".into()], false);
    assert!(r.tools.refusal("git_clone_repo", &clone_args()).unwrap().contains("may not reach git.example.org"));
    // a Repositories folder the account may not write in
    let r = rig_with("nowrite", |p, base| p.fs_write = vec![base.join("elsewhere").to_string_lossy().into_owned()], false);
    assert!(r.tools.refusal("git_clone_repo", &clone_args()).unwrap().contains("may not write in the Repositories folder"));
}

#[test]
fn a_workspace_is_only_added_inside_the_accounts_own_folders() {
    let r = rig("scope");
    std::fs::create_dir_all(r.repos.join("infra")).unwrap();
    let outside = r.base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.repos.join("link")).unwrap();
    let no = |p: &str| r.tools.refusal("ide_workspace_add", &json!({"path": p})).unwrap_or_else(|| panic!("{p} was not refused"));
    assert!(no("/etc").contains("outside the folders this account may use"));
    assert!(no(&outside.to_string_lossy()).contains("outside the folders"));
    assert!(no("link").contains("outside the folders"), "a link out of the Repositories folder is followed first");
    assert!(no("../../..").contains("outside the folders") || no("../../..").contains("there is no folder"));
    assert!(no(&r.repos.to_string_lossy()).contains("outside the folders"), "the Repositories folder itself is not one product");
    assert!(no(&r.base.join("home").to_string_lossy()).contains("outside the folders"), "nor is the home folder");
    assert!(no("missing").contains("there is no folder"));
    assert!(r.tools.refusal("ide_workspace_add", &json!({})).unwrap().contains("give the folder"));
    assert!(r.tools.call("ide_workspace_add", &json!({"path": "/etc"})).is_err());
    assert!(crate::workspace::Registry::load(&r.env_dir, "alice").items.is_empty());
    // where the account may read but not write, nothing is added
    let r = rig_with("scope2", |p, base| p.fs_write = vec![base.join("elsewhere").to_string_lossy().into_owned()], false);
    std::fs::create_dir_all(r.repos.join("infra")).unwrap();
    assert!(r.tools.refusal("ide_workspace_add", &json!({"path": "infra"})).unwrap().contains("outside the folders"));
    // inside is fine
    let r = rig("scope3");
    std::fs::create_dir_all(r.repos.join("infra")).unwrap();
    assert!(r.tools.refusal("ide_workspace_add", &json!({"path": "infra"})).is_none());
    assert!(r.tools.refusal("ide_workspace_add", &json!({"path": r.repos.join("infra").to_string_lossy()})).is_none());
}

// ------------------------------------------------------------------------------------------------ doing it

#[test]
fn a_clone_runs_the_tiles_job_with_the_submodule_flags_and_lists_what_it_found() {
    let r = rig("clone");
    let out = r.tools.call("git_clone_repo", &clone_args()).unwrap();
    assert!(out.starts_with("Cloned ops/infra with submodules into ") && out.contains("Its 2 submodule(s) from .gitmodules were cloned too") && out.contains("ide_workspace_add"), "{out}");
    let log = git_log(&r);
    assert!(log.contains("clone --progress --recurse-submodules -- https://git.example.org/ops/infra.git infra"), "{log}");
    assert!(r.repos.join("infra/docs/DEPLOY.md").is_file());
    let cfg = crate::workspace::ProjectConfig::load(&r.repos).unwrap();
    assert_eq!(cfg.repos.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["infra"], "recorded like the tile does");
    let a = audit_text(&r);
    assert!(a.contains("assistant.git.clone ops/infra from git.example.org into Repositories/infra with submodules"), "{a}");
    assert!(!a.contains("https://git.example.org"), "the audit names the repository, not an address with anything in it: {a}");
    // without the flag nothing about submodules is asked of git, and the result says what was left out
    let out = r.tools.call("git_clone_repo", &json!({"url": "https://git.example.org/ops/other.git"})).unwrap();
    assert!(!out.contains("submodule"), "{out}");
    let last = git_log(&r).lines().last().unwrap().to_string();
    assert!(!last.contains("recurse"), "{last}");
    // a failed clone says why, leaves nothing, and is shown in the list
    let e = r.tools.call("git_clone_repo", &json!({"url": "https://git.example.org/ops/failme.git"})).unwrap_err();
    assert!(e.contains("failed") && e.contains("repository not found"), "{e}");
    assert!(!r.repos.join("failme").exists());
    let list = r.tools.call("git_clones_list", &json!({})).unwrap();
    assert!(list.contains("failed (fatal: repository not found)") && list.contains("finished"), "{list}");
}

#[test]
fn clones_and_workspaces_are_listed_and_a_workspace_is_added_once() {
    let r = rig("list");
    assert!(r.tools.call("git_clones_list", &json!({})).unwrap().starts_with("No clones in "));
    assert_eq!(r.tools.call("ide_workspace_list", &json!({})).unwrap(), "No AI IDE workspaces yet.");
    r.tools.call("git_clone_repo", &clone_args()).unwrap();
    // the clone is a folder with .git for the list (the fake git makes none)
    std::fs::create_dir_all(r.repos.join("infra/.git")).unwrap();
    let list = r.tools.call("git_clones_list", &json!({})).unwrap();
    assert!(list.contains("infra · ") && list.contains("2 submodule(s)") && list.contains("not an AI IDE workspace yet"), "{list}");
    let out = r.tools.call("ide_workspace_add", &json!({"path": "infra", "name": "Infra"})).unwrap();
    assert!(out.starts_with("Registered ") && out.contains("“Infra”"), "{out}");
    assert!(audit_text(&r).contains("ide.workspace.add"));
    let ws = r.tools.call("ide_workspace_list", &json!({})).unwrap();
    assert!(ws.starts_with("Infra · ") && ws.contains("/infra"), "{ws}");
    assert!(r.tools.call("git_clones_list", &json!({})).unwrap().contains("AI IDE workspace “Infra”"));
    // once: the same folder again, or the same name, is refused with the reason
    let again = r.tools.refusal("ide_workspace_add", &json!({"path": "infra"})).unwrap();
    assert!(again.contains("already the workspace") && again.contains("nothing to do"), "{again}");
    std::fs::create_dir_all(r.repos.join("two")).unwrap();
    assert!(r.tools.refusal("ide_workspace_add", &json!({"path": "two", "name": "infra"})).unwrap().contains("already a workspace named"));
    assert!(r.tools.call("ide_workspace_add", &json!({"path": "infra"})).is_err());
    assert_eq!(crate::workspace::Registry::load(&r.env_dir, "alice").items.len(), 1);
}

// ------------------------------------------------------------------------------------------------ the whole request

struct Script {
    turns: Mutex<VecDeque<ModelTurn>>,
}

impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("plain chat unused".into())
    }
    fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.turns.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
    }
}

fn turn(name: &str, args: Value) -> ModelTurn {
    ModelTurn { text: String::new(), calls: vec![ToolCall { id: format!("c-{name}"), name: name.into(), args }] }
}

/// The tools of the rest of the chat: `read_file` reads a real file, and the cluster is a flag.
struct Rest {
    ran: Mutex<Vec<String>>,
    cluster_up: Mutex<bool>,
}

impl Tools for Rest {
    fn defs(&self) -> Vec<ToolDef> {
        let d = |n: &str, risk| ToolDef { spec: ToolSpec { name: n.into(), description: n.into(), schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}) }, risk, untrusted: false };
        vec![d("read_file", Risk::Safe), d("cluster_status", Risk::Safe), d("cluster_start", Risk::Ask)]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(name.to_string());
        match name {
            "read_file" => std::fs::read_to_string(args["path"].as_str().unwrap_or("")).map_err(|e| e.to_string()),
            "cluster_status" => Ok(if *self.cluster_up.lock().unwrap() { "The cluster is running." } else { "The cluster is not running." }.into()),
            "cluster_start" => {
                *self.cluster_up.lock().unwrap() = true;
                Ok("The cluster is starting.".into())
            }
            _ => Err(format!("no tool {name}")),
        }
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

fn drive(h: &RunHandle, mut verdict: impl FnMut(&str) -> Verdict) -> Vec<RunEvent> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(e) = h.events.recv_timeout(Duration::from_millis(50)) {
            if let RunEvent::Approval { id, name, .. } = &e {
                let v = verdict(name);
                h.decide(id, v);
            }
            let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
            out.push(e);
            if done {
                break;
            }
        }
    }
    out
}

fn approvals(e: &[RunEvent]) -> Vec<String> {
    e.iter().filter_map(|x| if let RunEvent::Approval { name, .. } = x { Some(name.clone()) } else { None }).collect()
}

fn results(e: &[RunEvent]) -> Vec<(String, bool, String)> {
    e.iter().filter_map(|x| if let RunEvent::ToolResult { name, ok, text, .. } = x { Some((name.clone(), *ok, text.clone())) } else { None }).collect()
}

fn the_request(r: &Rig, rest: Arc<Rest>, script: Vec<ModelTurn>, verdict: impl FnMut(&str) -> Verdict) -> Vec<RunEvent> {
    let inner: Arc<dyn Tools> = rest;
    let tools: Arc<dyn Tools> = Arc::new(GitWorkTools::with_env(inner, r.env.clone()));
    let backend = Arc::new(Script { turns: Mutex::new(script.into()) });
    let ask = "create workspace in ai ide from this repo, clone submodules, look at the deployment documents and start k3s";
    let h = start_with(backend, tools, vec![Message::system("SYS"), Message::user(ask)], RunLimits::default(), crate::assistrun::Assist::standard(crate::toolblock::ReminderCtx { agent_on: true, browser_policy: "off".into(), engine_installed: false, screen_available: false, loaded: None }));
    drive(&h, verdict)
}

#[test]
fn the_whole_request_clone_with_submodules_workspace_documents_and_k3s_asks_in_order_and_runs_nothing_early() {
    let r = rig("e2e");
    let rest = Arc::new(Rest { ran: Mutex::new(vec![]), cluster_up: Mutex::new(false) });
    let infra = r.repos.join("infra");
    let (rest2, infra2, env_dir) = (rest.clone(), infra.clone(), r.env_dir.clone());
    let doc = infra.join("docs/DEPLOY.md").to_string_lossy().into_owned();
    let script = vec![
        turn("git_clone_repo", clone_args()),
        turn("ide_workspace_add", json!({"path": "infra", "name": "Infra"})),
        turn("read_file", json!({"path": doc})),
        turn("cluster_status", json!({})),
        turn("cluster_start", json!({})),
        ModelTurn { text: "Done: cloned, registered, the deployment document says to run the installer, and the cluster is starting.".into(), calls: vec![] },
    ];
    // at each question, nothing the question is about has happened yet
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    let ev = the_request(&r, rest.clone(), script, move |name| {
        let state = match name {
            "git_clone_repo" => format!("folder={}", infra2.exists()),
            "ide_workspace_add" => format!("workspaces={} ran={:?}", crate::workspace::Registry::load(&env_dir, "alice").items.len(), rest2.ran.lock().unwrap()),
            "cluster_start" => format!("up={} ran={:?}", *rest2.cluster_up.lock().unwrap(), rest2.ran.lock().unwrap()),
            other => panic!("unexpected question {other}"),
        };
        s2.lock().unwrap().push(format!("{name}: {state}"));
        Verdict::Approve
    });
    assert_eq!(approvals(&ev), ["git_clone_repo", "ide_workspace_add", "cluster_start"], "{ev:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0], "git_clone_repo: folder=false", "nothing cloned before the yes");
    assert_eq!(seen[1], "ide_workspace_add: workspaces=0 ran=[]", "nothing registered before the yes");
    assert_eq!(seen[2], "cluster_start: up=false ran=[\"read_file\", \"cluster_status\"]", "the cluster is not started before the yes; the free steps ran without asking");
    let res = results(&ev);
    assert_eq!(res.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["git_clone_repo", "ide_workspace_add", "read_file", "cluster_status", "cluster_start"]);
    assert!(res.iter().all(|x| x.1), "{res:?}");
    assert!(res[0].2.contains("Its 2 submodule(s)") && res[1].2.contains("“Infra”") && res[2].2.contains("Deploy with k3s") && res[3].2.contains("not running"), "{res:?}");
    assert!(*rest.cluster_up.lock().unwrap());
    assert!(infra.join("docs/DEPLOY.md").is_file());
    assert!(git_log(&r).contains("--recurse-submodules"));
    assert_eq!(crate::workspace::Registry::load(&r.env_dir, "alice").items[0].name, "Infra");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Final { text } if text.starts_with("Done:"))));
    let a = audit_text(&r);
    assert!(a.contains("assistant.git.clone") && a.contains("ide.workspace.add") && !a.contains(TOKEN));
}

#[test]
fn a_denied_clone_changes_nothing_and_the_later_steps_are_not_taken() {
    let r = rig("denied");
    let rest = Arc::new(Rest { ran: Mutex::new(vec![]), cluster_up: Mutex::new(false) });
    let script = vec![turn("git_clone_repo", clone_args()), ModelTurn { text: "You declined the clone, so I stopped.".into(), calls: vec![] }];
    let ev = the_request(&r, rest.clone(), script, |_| Verdict::Deny);
    assert_eq!(approvals(&ev), ["git_clone_repo"]);
    assert!(results(&ev)[0].2.contains("denied"), "{:?}", results(&ev));
    assert!(!r.repos.exists(), "no folder was made");
    assert!(git_log(&r).is_empty(), "git never ran");
    assert!(audit_text(&r).is_empty(), "nothing was audited as done: {}", audit_text(&r));
    assert!(crate::workspace::Registry::load(&r.env_dir, "alice").items.is_empty());
    assert!(rest.ran.lock().unwrap().is_empty() && !*rest.cluster_up.lock().unwrap());
    assert!(crate::workspace::Registry::load(&r.env_dir, "alice").items.is_empty());
}

// ------------------------------------------------------------------------------------------------ with the real git

fn have_git() -> bool {
    std::process::Command::new("git").arg("--version").output().is_ok()
}

fn git(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always"]).args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn the_real_git_clones_a_repository_with_its_submodule_only_when_asked() {
    if !have_git() {
        eprintln!("skipped: no git");
        return;
    }
    let r = build("real", |_, _| {}, true, true);
    let sub = r.base.join("sub-work");
    std::fs::create_dir_all(&sub).unwrap();
    git(&sub, &["init", "-q"]);
    std::fs::write(sub.join("inner.txt"), "inner\n").unwrap();
    git(&sub, &["add", "."]);
    git(&sub, &["commit", "-q", "-m", "inner"]);
    let main = r.base.join("main-work");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("README.md"), "main\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "main"]);
    git(&main, &["submodule", "add", "-q", sub.to_str().unwrap(), "deps/sub"]);
    git(&main, &["commit", "-q", "-m", "with a submodule"]);
    let url = main.to_string_lossy().into_owned();
    let plain = r.tools.call("git_clone_repo", &json!({"url": url})).unwrap();
    assert!(plain.contains("1 submodule(s) in .gitmodules that were not cloned") || plain.contains("submodule"), "{plain}");
    assert!(r.repos.join("main-work/README.md").is_file());
    assert!(!r.repos.join("main-work/deps/sub/inner.txt").exists(), "off by default: the submodule folder stays empty");
    let full = r.tools.call("git_clone_repo", &json!({"url": url, "submodules": true})).unwrap();
    assert!(full.contains("Its 1 submodule(s)"), "{full}");
    assert!(r.repos.join("main-work-2/deps/sub/inner.txt").is_file(), "{full}");
}
