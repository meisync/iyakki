//! The AI IDE's deploy flow: build the image, load it into the local cluster, apply the manifest, check the rollout. Each step
//! shows the real command (and, for the apply, the manifest check and `kubectl diff`), waits for the person's approval (or
//! "approve all steps for this deploy"), runs on a worker thread through the same tools the assistant uses
//! (`iyakki_agent::k8stools`), and streams its output into the panel. A restart is a one-step flow.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::k8stools::{self, K8sTools};
use iyakki_agent::wsexec::Sink;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Pending,
    /// Shown to the person, waiting for the answer.
    Asking,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub tool: &'static str,
    pub title: String,
    pub state: St,
    /// One line: what the approval says.
    pub summary: String,
    /// The real commands, manifest check and diff.
    pub detail: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub image: String,
    pub dir: String,
    /// A manifest file of the workspace; empty: a starter Deployment and Service for `port`.
    pub manifest: String,
    pub port: u16,
    pub approve_all: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Approve,
    ApproveAll,
    Cancel,
}

#[derive(Default)]
pub struct Shared {
    pub steps: Vec<Step>,
    pub finished: bool,
    pub cancelled: bool,
    pub summary: String,
}

pub struct Flow {
    pub title: String,
    pub shared: Arc<Mutex<Shared>>,
    tx: Sender<Answer>,
    /// The live output of the running step.
    pub live: Sink,
}

fn set(sh: &Arc<Mutex<Shared>>, i: usize, f: impl FnOnce(&mut Step)) {
    let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.steps.get_mut(i) {
        f(s);
    }
}

fn wait(rx: &Receiver<Answer>, all: &mut bool) -> Answer {
    if *all {
        return Answer::Approve;
    }
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Answer::ApproveAll) => {
                *all = true;
                return Answer::Approve;
            }
            Ok(a) => return a,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Answer::Cancel,
        }
    }
}

/// `name` for the Deployment: the image's last path part without the tag.
pub fn app_name(image: &str) -> String {
    let last = image.rsplit('/').next().unwrap_or(image);
    let n: String = last.split(':').next().unwrap_or(last).chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() { "app".into() } else { n.chars().take(40).collect() }
}

impl Flow {
    /// Build → load → apply → verify.
    pub fn deploy(tools: Arc<K8sTools>, plan: Plan) -> Flow {
        let name = app_name(&plan.image);
        let titles = [("deploy_build", format!("Build {}", plan.image)), ("deploy_load", "Get the image to the cluster".to_string()), ("k8s_apply", format!("Apply {} to namespace {} on {}", if plan.manifest.is_empty() { "a starter Deployment and Service".to_string() } else { plan.manifest.clone() }, tools.ns, tools.cluster_label())), ("deploy_status", "Check the rollout".to_string())];
        let steps: Vec<Step> = titles.iter().map(|(t, title)| Step { tool: t, title: title.clone(), state: St::Pending, summary: String::new(), detail: String::new(), output: String::new() }).collect();
        let shared = Arc::new(Mutex::new(Shared { steps, ..Default::default() }));
        let (tx, rx) = channel();
        let live: Sink = Arc::new(Mutex::new(String::new()));
        let (sh, lv, tl) = (shared.clone(), live.clone(), tools.clone());
        let title = format!("Deploy {} to {} on {}", plan.image, tools.ns, tools.cluster_label());
        std::thread::spawn(move || {
            let mut all = plan.approve_all;
            let (mut built, mut cluster_ref) = (String::new(), String::new());
            for i in 0..4 {
                // the arguments of this step, from what the earlier ones produced
                let args: Value = match i {
                    0 => json!({"dir": plan.dir, "image": plan.image}),
                    1 => json!({"image": built}),
                    2 => {
                        if plan.manifest.is_empty() {
                            let path = format!("deploy/{name}.yaml");
                            let full = tl.root.join(&path);
                            if !full.exists() {
                                let image = if cluster_ref.is_empty() { built.clone() } else { cluster_ref.clone() };
                                let _ = std::fs::create_dir_all(tl.root.join("deploy"));
                                let _ = std::fs::write(&full, k8stools::starter_manifest(&name, &image, plan.port));
                            }
                            json!({"manifest_path": path})
                        } else {
                            json!({"manifest_path": plan.manifest})
                        }
                    }
                    _ => json!({"name": name}),
                };
                let tool = ["deploy_build", "deploy_load", "k8s_apply", "deploy_status"][i];
                let summary = tl.describe(tool, &args);
                let detail = tl.preview(tool, &args).unwrap_or_default();
                set(&sh, i, |s| {
                    s.summary = summary;
                    s.detail = detail;
                    s.state = if tool == "deploy_status" { St::Running } else { St::Asking };
                });
                if tool != "deploy_status" && wait(&rx, &mut all) == Answer::Cancel {
                    let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
                    for s in g.steps.iter_mut().skip(i) {
                        s.state = St::Skipped;
                    }
                    g.cancelled = true;
                    g.finished = true;
                    g.summary = "Cancelled: nothing more was run.".into();
                    return;
                }
                set(&sh, i, |s| s.state = St::Running);
                lv.lock().unwrap_or_else(|e| e.into_inner()).clear();
                tl.set_live(Some(lv.clone()));
                let r = tl.call(tool, &args);
                tl.set_live(None);
                let live_text = lv.lock().unwrap_or_else(|e| e.into_inner()).clone();
                match r {
                    Ok(out) => {
                        set(&sh, i, |s| {
                            s.state = St::Done;
                            s.output = if live_text.trim().is_empty() { out.clone() } else { format!("{}\n{out}", tail(&live_text, 30)) };
                        });
                        if tool == "deploy_build" {
                            built = tl.built_ref().unwrap_or_default();
                        }
                        if tool == "deploy_load" {
                            cluster_ref = tl.cluster_ref(&built).unwrap_or_else(|| built.clone());
                        }
                    }
                    Err(e) => {
                        let mut g = sh.lock().unwrap_or_else(|x| x.into_inner());
                        g.steps[i].state = St::Failed;
                        g.steps[i].output = e;
                        for s in g.steps.iter_mut().skip(i + 1) {
                            s.state = St::Skipped;
                        }
                        g.finished = true;
                        g.summary = format!("Stopped at \"{}\".", g.steps[i].title);
                        return;
                    }
                }
            }
            let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
            g.finished = true;
            g.summary = format!("Deployed {} to namespace {}.", plan.image, tl.ns);
        });
        Flow { title, shared, tx, live }
    }

    /// A rollout restart: one step, approved like the others.
    pub fn restart(tools: Arc<K8sTools>, kind: &str, name: &str) -> Flow {
        let args = json!({"name": name, "kind": kind});
        let step = Step { tool: "k8s_rollout_restart", title: format!("Restart {kind} {name}"), state: St::Asking, summary: tools.describe("k8s_rollout_restart", &args), detail: format!("kubectl rollout restart {kind}/{name} -n {}  (context {})", tools.ns, tools.profile.context), output: String::new() };
        let shared = Arc::new(Mutex::new(Shared { steps: vec![step], ..Default::default() }));
        let (tx, rx) = channel();
        let sh = shared.clone();
        std::thread::spawn(move || {
            let mut all = false;
            if wait(&rx, &mut all) == Answer::Cancel {
                let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
                g.steps[0].state = St::Skipped;
                g.cancelled = true;
                g.finished = true;
                g.summary = "Cancelled.".into();
                return;
            }
            set(&sh, 0, |s| s.state = St::Running);
            let r = tools.call("k8s_rollout_restart", &args);
            let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
            match r {
                Ok(o) => {
                    g.steps[0].state = St::Done;
                    g.steps[0].output = o;
                    g.summary = "Restart started: watch the pods.".into();
                }
                Err(e) => {
                    g.steps[0].state = St::Failed;
                    g.steps[0].output = e;
                    g.summary = "The restart failed.".into();
                }
            }
            g.finished = true;
        });
        Flow { title: format!("Restart {name}"), shared, tx, live: Arc::new(Mutex::new(String::new())) }
    }

    /// Set up the registry (`iyakki_agent::regsetup`): one step. Its detail is the exact plan (objects, files, commands, what needs
    /// root), computed on the worker (it reads the cluster), then the person approves.
    pub fn registry(tools: Arc<K8sTools>, args: Value) -> Flow {
        let kind = args["option"].as_str().unwrap_or("").to_string();
        Flow::single(tools, "registry_setup", args, format!("Set up the registry (option {kind})"), "Set up the registry", "Registry set up: the choice is saved in project.json and deploy uses it.")
    }

    /// One approved step for any cluster tool: the plan (the tool's preview, read on the worker) is shown, nothing runs before the answer.
    pub fn single(tools: Arc<K8sTools>, tool: &'static str, args: Value, step_title: String, title: &str, done: &str) -> Flow {
        let done = done.to_string();
        let step = Step { tool, title: step_title, state: St::Pending, summary: "Reading the cluster to prepare the plan…".into(), detail: String::new(), output: String::new() };
        let shared = Arc::new(Mutex::new(Shared { steps: vec![step], ..Default::default() }));
        let (tx, rx) = channel();
        let (sh, lv, tl) = (shared.clone(), Arc::new(Mutex::new(String::new())), tools.clone());
        let live = lv.clone();
        let title = format!("{title} on {}", tools.cluster_label());
        std::thread::spawn(move || {
            let summary = tl.describe(tool, &args);
            let detail = tl.preview(tool, &args).unwrap_or_default();
            if detail.starts_with("this would be refused") {
                let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
                g.steps[0].state = St::Failed;
                g.steps[0].output = detail;
                g.finished = true;
                g.summary = "Refused: nothing was changed.".into();
                return;
            }
            set(&sh, 0, |s| {
                s.summary = summary;
                s.detail = detail;
                s.state = St::Asking;
            });
            let mut all = false;
            if wait(&rx, &mut all) == Answer::Cancel {
                let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
                g.steps[0].state = St::Skipped;
                g.cancelled = true;
                g.finished = true;
                g.summary = "Cancelled: nothing was changed.".into();
                return;
            }
            set(&sh, 0, |s| s.state = St::Running);
            let r = tl.call(tool, &args);
            let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
            match r {
                Ok(o) => {
                    g.steps[0].state = St::Done;
                    g.steps[0].output = o;
                    g.summary = done.clone();
                }
                Err(e) => {
                    g.steps[0].state = St::Failed;
                    g.steps[0].output = e;
                    g.summary = "It failed: see the output.".into();
                }
            }
            g.finished = true;
        });
        Flow { title, shared, tx, live }
    }

    pub fn answer(&self, a: Answer) {
        let _ = self.tx.send(a);
    }

    pub fn steps(&self) -> Vec<Step> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).steps.clone()
    }

    pub fn finished(&self) -> bool {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).finished
    }

    pub fn summary(&self) -> String {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).summary.clone()
    }

    /// The step waiting for an answer.
    pub fn asking(&self) -> Option<usize> {
        self.steps().iter().position(|s| s.state == St::Asking)
    }

    pub fn live_text(&self) -> String {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn tail(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().collect();
    v[v.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::devcluster::{ClusterProfile, Source};
    use iyakki_agent::wsexec::FakeRunner;

    fn tools(tag: &str) -> (Arc<K8sTools>, Arc<FakeRunner>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("iyakki-deploy-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/Dockerfile"), "FROM scratch\n").unwrap();
        let r = Arc::new(FakeRunner::new(&["kubectl", "podman", "k3s"]));
        let p = ClusterProfile { name: "k3s".into(), kubeconfig: "/tmp/k.yaml".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::K3s, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
        (Arc::new(K8sTools::new(r.clone(), p, "shop-dev", "Shop", &root, Arc::new(|_, _| {}))), r, root)
    }

    fn until(f: impl Fn() -> bool) {
        for _ in 0..600 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    fn answers(r: &FakeRunner) {
        r.answer("podman build", "STEP 1/1\nSuccessfully tagged\n");
        r.answer("podman image exists", "");
        r.answer("podman save", "");
        r.answer("k3s ctr images import", "imported");
        r.answer("diff -f", "");
        r.answer("get namespace shop-dev", "namespace/shop-dev");
        r.answer("apply -f", "deployment.apps/web created\nservice/web created\n");
        r.answer("rollout status", "deployment \"web\" successfully rolled out\n");
        r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        r.answer("config current-context", "default\n");
        r.answer("get deployments", r#"{"items":[]}"#);
        r.answer("get configmaps", "");
        r.answer("get secrets", "");
        r.answer("get nodes", "{}");
    }

    #[test]
    fn the_flow_titles_name_the_cluster_the_deploy_goes_to() {
        let (t, r, _root) = tools("names-cluster");
        answers(&r);
        let mut t = Arc::try_unwrap(t).ok().expect("the only owner");
        let f = Flow::deploy(Arc::new(K8sTools::new(t.run.clone(), t.profile.clone(), "shop-dev", "Shop", &t.root, Arc::new(|_, _| {}))), Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 8080, approve_all: false });
        until(|| f.asking() == Some(0));
        assert_eq!(f.title, "Deploy web to shop-dev on cluster \"k3s\"", "the resolved profile's name when the registry has one cluster");
        assert!(f.steps()[2].title.ends_with("to namespace shop-dev on cluster \"k3s\""), "{}", f.steps()[2].title);
        t.cluster_phrase = "cluster \"gpu-box\" (remote)".into();
        let f = Flow::deploy(Arc::new(t), Plan { image: "web".into(), dir: "app".into(), manifest: "deploy/app.yaml".into(), port: 8080, approve_all: false });
        assert_eq!(f.title, "Deploy web to shop-dev on cluster \"gpu-box\" (remote)");
        assert_eq!(f.steps()[2].title, "Apply deploy/app.yaml to namespace shop-dev on cluster \"gpu-box\" (remote)");
    }

    #[test]
    fn each_step_waits_for_approval_shows_the_real_command_and_runs_in_order() {
        let (t, r, root) = tools("steps");
        answers(&r);
        let f = Flow::deploy(t, Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 8080, approve_all: false });
        until(|| f.asking() == Some(0));
        let s = f.steps();
        assert!(s[0].summary.contains("podman build") && s[0].detail.contains("podman build -t localhost/web:"), "{:?}", s[0]);
        assert!(r.lines().iter().all(|l| !l.contains("build")), "nothing ran before the approval: {:?}", r.lines());
        f.answer(Answer::Approve);
        until(|| f.asking() == Some(1));
        assert_eq!(f.steps()[0].state, St::Done);
        assert!(f.steps()[1].detail.contains("k3s ctr images import"), "{}", f.steps()[1].detail);
        f.answer(Answer::Approve);
        until(|| f.asking() == Some(2));
        let apply = f.steps()[2].clone();
        assert!(apply.detail.contains("manifest check") && apply.detail.contains("namespace shop-dev"), "{}", apply.detail);
        assert!(root.join("deploy/web.yaml").exists(), "the starter manifest is saved in the workspace");
        let saved = std::fs::read_to_string(root.join("deploy/web.yaml")).unwrap();
        assert!(saved.contains("image: localhost/web:") && saved.contains("containerPort: 8080"), "{saved}");
        f.answer(Answer::Approve);
        until(|| f.finished());
        let s = f.steps();
        assert!(s.iter().all(|x| x.state == St::Done), "{s:?}");
        assert!(f.summary().contains("Deployed web to namespace shop-dev"));
        let lines = r.lines();
        let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
        assert!(pos("podman build") < pos("ctr images import") && pos("ctr images import") < pos(" apply -f ") && pos(" apply -f ") < pos("rollout status"));
        assert!(!lines.iter().any(|l| l.contains("sudo") || l.starts_with("k3s server")));
    }

    #[test]
    fn approve_all_runs_every_step_and_cancel_stops_without_running_the_rest() {
        let (t, r, _root) = tools("all");
        answers(&r);
        let f = Flow::deploy(t, Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 80, approve_all: true });
        until(|| f.finished());
        assert!(f.steps().iter().all(|s| s.state == St::Done), "{:?}", f.steps());
        // approve-all chosen at the second step
        let (t, r2, _root) = tools("allmid");
        answers(&r2);
        let f = Flow::deploy(t, Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 80, approve_all: false });
        until(|| f.asking() == Some(0));
        f.answer(Answer::Approve);
        until(|| f.asking() == Some(1));
        f.answer(Answer::ApproveAll);
        until(|| f.finished());
        assert!(f.steps().iter().all(|s| s.state == St::Done));
        // cancel at the load step
        let (t, r3, _root) = tools("cancel");
        answers(&r3);
        let f = Flow::deploy(t, Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 80, approve_all: false });
        until(|| f.asking() == Some(0));
        f.answer(Answer::Approve);
        until(|| f.asking() == Some(1));
        f.answer(Answer::Cancel);
        until(|| f.finished());
        let s = f.steps();
        assert_eq!((s[0].state, s[1].state, s[2].state, s[3].state), (St::Done, St::Skipped, St::Skipped, St::Skipped));
        assert!(!r3.lines().iter().any(|l| l.contains("ctr images import") || l.contains(" apply ")), "{:?}", r3.lines());
        let _ = r;
    }

    #[test]
    fn a_failing_step_stops_the_flow_with_its_error_and_the_rest_is_skipped() {
        let (t, r, _root) = tools("fail");
        r.fail("podman build", 125, "error: Dockerfile line 3: unknown instruction");
        let f = Flow::deploy(t, Plan { image: "web".into(), dir: "app".into(), manifest: String::new(), port: 80, approve_all: true });
        until(|| f.finished());
        let s = f.steps();
        assert_eq!(s[0].state, St::Failed);
        assert!(s[0].output.contains("unknown instruction"), "{}", s[0].output);
        assert!(s[1..].iter().all(|x| x.state == St::Skipped));
        assert!(f.summary().contains("Stopped at"));
    }

    #[test]
    fn a_restart_is_one_approved_step() {
        let (t, r, _root) = tools("restart");
        r.answer("rollout restart deployment/web", "deployment.apps/web restarted\n");
        let f = Flow::restart(t, "deployment", "web");
        until(|| f.asking() == Some(0));
        assert!(f.steps()[0].detail.contains("kubectl rollout restart deployment/web -n shop-dev"));
        assert!(r.calls().is_empty());
        f.answer(Answer::Approve);
        until(|| f.finished());
        assert_eq!(f.steps()[0].state, St::Done);
        assert!(r.lines().iter().any(|l| l.ends_with("rollout restart deployment/web -n shop-dev")));
    }

    #[test]
    fn app_names_come_from_the_image() {
        assert_eq!(app_name("web"), "web");
        assert_eq!(app_name("localhost/My_App:1.2"), "my-app");
        assert_eq!(app_name("team/api-server"), "api-server");
        assert_eq!(app_name(":::"), "app");
    }
}
