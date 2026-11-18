use std::sync::Mutex;

use super::*;

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
    allowed: bool,
    every: bool,
    reach: bool,
    names: Mutex<Vec<String>>,
    jobs: Mutex<Vec<(String, Vec<String>)>>,
    audit: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn allowed(&self) -> bool {
        self.allowed
    }
    fn every_host(&self) -> bool {
        self.every
    }
    fn may_reach(&self, _: &str) -> bool {
        self.reach
    }
    fn names(&self) -> Vec<String> {
        self.names.lock().unwrap().clone()
    }
    fn size_mb(&self, _: &str) -> u64 {
        412
    }
    fn start_job(&self, label: &str, args: &[String]) -> Result<String, String> {
        self.jobs.lock().unwrap().push((label.into(), args.to_vec()));
        Ok("started: job 7".into())
    }
    fn delete(&self, name: &str, typed: &str) -> Result<u64, String> {
        if typed != name {
            return Err("type the name".into());
        }
        self.names.lock().unwrap().retain(|n| n != name);
        Ok(412)
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(allowed: bool, every: bool, reach: bool) -> (LinuxSandboxTools, Arc<Fake>) {
    let f = Arc::new(Fake { allowed, every, reach, names: Mutex::new(vec!["dev".into()]), jobs: Mutex::new(vec![]), audit: Mutex::new(vec![]) });
    (LinuxSandboxTools::new(Arc::new(Inner), f.clone()), f)
}

#[test]
fn both_tools_ask_every_time_never_ride_on_a_grant_and_the_cards_say_what_happens() {
    let (t, _) = rig(true, false, true);
    let defs = t.defs();
    assert_eq!(defs.iter().filter(|d| TOOLS.contains(&d.spec.name.as_str())).count(), 2);
    assert!(defs.iter().all(|d| d.risk == Risk::Ask));
    for n in TOOLS {
        assert!(!t.pre_approved(n) && !t.approved_always(n) && t.grant_blocker(n, &json!({})).is_some());
    }
    let c = t.preview("linux_sandbox_create", &json!({"name": "web", "release": "debian-12"})).unwrap();
    assert!(c.contains("web") && c.contains("debian-12") && c.contains("Network: off"), "{c}");
    let d = t.preview("linux_sandbox_delete", &json!({"name": "dev", "confirm_name": "dev"})).unwrap();
    assert!(d.contains("412 MB") && d.contains("cannot be brought back"), "{d}");
    assert!(crate::agent::hard_ask("linux_sandbox_create", &json!({})).is_some() && crate::agent::hard_ask("linux_sandbox_delete", &json!({})).is_some());
}

#[test]
fn creating_starts_the_same_job_as_the_command_line_and_checks_what_the_account_may_do() {
    let (t, f) = rig(true, false, true);
    assert_eq!(t.call("linux_sandbox_create", &json!({"name": "web", "release": "ubuntu-24.04"})).unwrap(), "started: job 7");
    let jobs = f.jobs.lock().unwrap();
    assert_eq!(jobs[0].0, "Linux apps: create web");
    assert_eq!(jobs[0].1, ["create", "web", "ubuntu-24.04"]);
    assert!(f.audit.lock().unwrap()[0].starts_with("linuxapps.create web ubuntu-24.04"));
    drop(jobs);
    // the network on needs an account allowed every host
    assert!(t.refusal("linux_sandbox_create", &json!({"name": "web2", "release": "debian-12", "network": true})).unwrap().contains("every host"));
    let (every, f2) = rig(true, true, true);
    every.call("linux_sandbox_create", &json!({"name": "web2", "release": "debian-12", "network": true})).unwrap();
    assert_eq!(f2.jobs.lock().unwrap()[0].1, ["create", "web2", "debian-12", "--net", "on"]);
    for (what, a) in [("bad name", json!({"name": "Web App", "release": "debian-12"})), ("unknown release", json!({"name": "web", "release": "arch"})), ("existing name", json!({"name": "dev", "release": "debian-12"})), ("no release", json!({"name": "web"}))] {
        assert!(t.refusal("linux_sandbox_create", &a).is_some(), "{what}");
    }
    let (no_servers, _) = rig(true, false, false);
    assert!(no_servers.refusal("linux_sandbox_create", &json!({"name": "web", "release": "debian-12"})).unwrap().contains("distribution servers"));
    let (denied, f3) = rig(false, true, true);
    assert!(denied.refusal("linux_sandbox_create", &json!({"name": "web", "release": "debian-12"})).unwrap().contains("role"));
    assert!(f3.jobs.lock().unwrap().is_empty());
}

#[test]
fn deleting_needs_the_name_again_and_an_existing_sandbox_and_is_audited() {
    let (t, f) = rig(true, false, true);
    assert!(t.refusal("linux_sandbox_delete", &json!({"name": "dev"})).unwrap().contains("confirm_name"));
    assert!(t.refusal("linux_sandbox_delete", &json!({"name": "dev", "confirm_name": "dve"})).unwrap().contains("nothing was deleted"));
    assert!(t.refusal("linux_sandbox_delete", &json!({"name": "ghost", "confirm_name": "ghost"})).unwrap().contains("no sandbox"));
    assert_eq!(f.names.lock().unwrap().len(), 1, "nothing was deleted by the refusals");
    assert_eq!(t.call("linux_sandbox_delete", &json!({"name": "dev", "confirm_name": "dev"})).unwrap(), "dev was deleted (412 MB freed)");
    assert!(f.names.lock().unwrap().is_empty());
    assert!(f.audit.lock().unwrap().iter().any(|a| a == "linuxapps.delete dev 412 MB freed"));
    assert!(t.call("other_tool", &json!({})).unwrap_err().contains("inner got other_tool"));
}
