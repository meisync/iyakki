//! Tests of what a task gets besides its tree: reference to the main workspace, namespace, shared folders, close question.

// the boundary scan reads a file's code up to its first #[cfg(test)]: this whole file is test code (it runs real `git` in temp folders)
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::taskenv::*;
use crate::tasks::{Status, Task};

fn tmp(tag: &str) -> PathBuf {
    // outside every Cargo project and git work tree: a lookup that walks up the parents must find nothing of the repository (the gate's TMPDIR is in it)
    crate::testdir::outside_project("iyakki-tenv", tag)
}

fn g(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

#[derive(Default)]
struct Fake {
    log: Mutex<Vec<String>>,
    fail_create: bool,
}
impl TaskCluster for Fake {
    fn create_namespace(&self, n: &str) -> Result<(), String> {
        if self.fail_create {
            return Err("cluster is off".into());
        }
        self.log.lock().unwrap().push(format!("create {n}"));
        Ok(())
    }
    fn delete_namespace(&self, n: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("delete {n}"));
        Ok(())
    }
    fn apply(&self, n: &str, m: &str) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("apply {n} {}", m.len()));
        Ok("applied".into())
    }
}

struct Empty;
impl Tools for Empty {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("inner {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

#[test]
fn the_main_workspace_can_be_read_but_not_left_or_written() {
    let main = tmp("ref");
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(main.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(main.join("bin.dat"), [0u8, 1, 2]).unwrap();
    std::fs::create_dir_all(main.join(".git")).unwrap();
    std::fs::write(main.join(".git/config"), "secret").unwrap();
    assert_eq!(reference_read(&main, "src/a.rs").unwrap(), "fn a() {}\n");
    assert!(reference_read(&main, "../x").is_err());
    assert!(reference_read(&main, "/etc/hosts").is_err());
    assert!(reference_read(&main, ".git/config").is_err());
    assert!(reference_read(&main, "bin.dat").unwrap_err().contains("not text"));
    assert!(reference_read(&main, "src").is_err());
    assert_eq!(reference_list(&main, "").unwrap(), vec!["bin.dat".to_string(), "src/".to_string()]);
    #[cfg(unix)]
    {
        let out = tmp("ref-out");
        std::fs::write(out.join("s.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(&out, main.join("link")).unwrap();
        assert!(reference_read(&main, "link/s.txt").unwrap_err().contains("outside"));
    }
    // the tools only read: there is no write tool among them
    let t = ReferenceTools::new(Arc::new(Empty), &main, "ns", None);
    let names: Vec<String> = t.defs().iter().map(|d| d.spec.name.clone()).collect();
    assert_eq!(names, vec!["main_read", "main_list"]);
    assert!(t.defs().iter().all(|d| d.risk == Risk::Safe));
    assert_eq!(t.call("main_read", &json!({"path":"src/a.rs"})).unwrap(), "fn a() {}\n");
    assert!(t.call("task_deploy", &json!({"manifest":"x"})).unwrap_err().contains("inner"));
}

#[test]
fn a_big_file_is_cut_on_a_character_boundary() {
    let main = tmp("big");
    std::fs::write(main.join("big.txt"), "é".repeat(REFERENCE_MAX_BYTES)).unwrap();
    let t = reference_read(&main, "big.txt").unwrap();
    assert!(t.ends_with("bytes]") && t.len() < REFERENCE_MAX_BYTES + 100);
}

#[test]
fn a_task_deploys_only_into_its_own_namespace_and_asks_every_time() {
    let main = tmp("dep");
    let fake = Arc::new(Fake::default());
    let t = ReferenceTools::new(Arc::new(Empty), &main, "app-dev-t1", Some(fake.clone()));
    let d = t.defs().into_iter().find(|d| d.spec.name == "task_deploy").unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(!t.approved_always("task_deploy"));
    assert!(t.grant_blocker("task_deploy", &json!({})).is_some());
    let ok = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\n  namespace: app-dev-t1\n";
    assert!(t.refusal("task_deploy", &json!({"manifest": ok})).is_none());
    assert_eq!(t.call("task_deploy", &json!({"manifest": ok})).unwrap(), "applied");
    for bad in ["kind: ClusterRole\n", "kind: Namespace\nmetadata:\n  name: x\n", "kind: Pod\nmetadata:\n  namespace: other\n", "  "] {
        assert!(t.refusal("task_deploy", &json!({"manifest": bad})).is_some(), "{bad}");
        assert!(t.call("task_deploy", &json!({"manifest": bad})).is_err());
    }
    assert_eq!(fake.log.lock().unwrap().len(), 1, "only the good manifest reached the cluster");
}

#[test]
fn the_namespace_is_created_with_the_task_and_deleted_after_and_a_failure_is_said() {
    let task = Task { id: "t1".into(), namespace: "app-dev-t1".into(), ..Default::default() };
    let fake = Fake::default();
    assert!(provision(&fake, &task).unwrap().contains("app-dev-t1"));
    teardown(&fake, &task).unwrap();
    assert_eq!(*fake.log.lock().unwrap(), vec!["create app-dev-t1", "delete app-dev-t1"]);
    assert!(provision(&Fake { fail_create: true, ..Default::default() }, &task).unwrap_err().contains("off"));
    assert!(provision(&fake, &Task::default()).is_err());
    teardown(&fake, &Task::default()).unwrap();
}

#[test]
fn a_manager_creates_the_namespace_on_spawn_and_deletes_it_on_discard() {
    let root = tmp("mgr");
    g(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    std::fs::write(root.join(".gitignore"), "node_modules\n.iyakki/\n").unwrap();
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::fs::write(root.join("FEATURES.md"), "# b\n").unwrap();
    g(&root, &["add", "-A"]);
    g(&root, &["commit", "-q", "-m", "i"]);
    let m = crate::tasks::Manager::open(&root, &root.join("FEATURES.md"));
    m.set_concurrency(0);
    let fake = Arc::new(Fake::default());
    m.set_hooks(crate::tasks::Hooks { cluster: Some(fake.clone()), ..Default::default() });
    let id = m.spawn(crate::tasks::NewTask { title: "t".into(), goal: "g".into(), ..Default::default() }).unwrap();
    let t = m.get(&id).unwrap();
    assert!(fake.log.lock().unwrap()[0].starts_with("create "));
    assert!(t.note.contains("node_modules"), "{}", t.note);
    assert!(t.tree.join("node_modules").exists());
    m.discard(&id).unwrap();
    assert!(fake.log.lock().unwrap().iter().any(|l| l.starts_with("delete ")));
}

#[test]
fn dependency_folders_are_linked_only_when_git_ignores_them_and_the_tree_lacks_them() {
    let main = tmp("share-main");
    let tree = tmp("share-tree");
    std::fs::write(main.join(".gitignore"), "node_modules\ntarget/\n").unwrap();
    std::fs::create_dir_all(main.join("node_modules")).unwrap();
    std::fs::create_dir_all(main.join("target")).unwrap();
    std::fs::create_dir_all(main.join("vendor")).unwrap(); // not ignored: stays out
    std::fs::create_dir_all(tree.join("target")).unwrap(); // already there
    let got = share_dependency_folders(&main, &tree);
    assert_eq!(got, vec!["node_modules".to_string()]);
    assert!(tree.join("node_modules").exists() && !tree.join("vendor").exists());
}

#[test]
fn closing_asks_only_when_a_task_is_working() {
    let t = |s: Status, name: &str| Task { title: name.into(), status: s, ..Default::default() };
    assert!(close_question(&[]).is_none());
    assert!(close_question(&[t(Status::Done, "a"), t(Status::Merged, "b")]).is_none());
    let q = close_question(&[t(Status::Running, "Fix login"), t(Status::Done, "x")]).unwrap();
    assert!(q.contains("Fix login") && q.contains("1 task(s)") && q.contains("Close anyway?"));
}

#[test]
fn uncommitted_changes_of_the_main_workspace_are_listed_and_brought_on_request() {
    let main = tmp("unc-main");
    let tree = tmp("unc-tree");
    g(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    std::fs::write(main.join("gone.txt"), "x\n").unwrap();
    g(&main, &["add", "-A"]);
    g(&main, &["commit", "-q", "-m", "i"]);
    std::fs::write(main.join("a.txt"), "two\n").unwrap();
    std::fs::create_dir_all(main.join("sub")).unwrap();
    std::fs::write(main.join("sub/new.txt"), "n\n").unwrap();
    std::fs::remove_file(main.join("gone.txt")).unwrap();
    std::fs::write(tree.join("gone.txt"), "x\n").unwrap();
    let mut files = uncommitted_in_main(&main);
    files.sort();
    assert_eq!(files, vec!["a.txt", "gone.txt", "sub/new.txt"]);
    assert_eq!(bring_uncommitted(&main, &tree, &files).unwrap(), 3);
    assert_eq!(std::fs::read_to_string(tree.join("a.txt")).unwrap(), "two\n");
    assert!(tree.join("sub/new.txt").exists() && !tree.join("gone.txt").exists());
    assert!(bring_uncommitted(&main, &tree, &["../x".to_string()]).is_err());
    assert!(uncommitted_in_main(&tmp("unc-nogit")).is_empty());
}

#[test]
fn a_task_uses_its_own_model_else_the_chats() {
    let t = Task { model: " qwen3 ".into(), ..Default::default() };
    assert_eq!(effective_model(&t, "chat-model"), "qwen3");
    assert_eq!(effective_model(&Task::default(), "chat-model"), "chat-model");
}
