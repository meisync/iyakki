//! The real port-forward door (F-K8S-26): `kubectl port-forward` children behind the shared bookkeeping of `iyakki_core_api::portforward::Forwards`.
//! One list per process ([`LocalPortForwards::shared`]): the K8S tile (through the guarded `Os`), the chat tool and `dev forward` all see and stop the
//! same forwards. All of them stop when the process ends or the set is dropped.

use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use iyakki_core_api::portforward::{Forwards, Info, PortForwards, Proc, Request, Spawn, READY_SECS};

struct Real(Child);

impl Proc for Real {
    fn alive(&mut self) -> bool {
        iyakki_core_api::child_status(&mut self.0).is_running()
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `program` (`kubectl`) with the arguments; its standard output is read line by line on a thread.
fn spawn_of(program: String) -> Spawn {
    Box::new(move |args: &[String]| {
        let mut child = crate::starters::kubectl_spawn_lines_with(&program, args).map_err(|e| format!("kubectl could not be started ({e}): install kubectl"))?;
        let out = child.stdout.take().ok_or("kubectl has no output")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Ok((Box::new(Real(child)) as Box<dyn Proc>, rx))
    })
}

pub struct LocalPortForwards {
    inner: Forwards,
}

impl LocalPortForwards {
    /// The real `kubectl` of this computer.
    pub fn new() -> LocalPortForwards {
        LocalPortForwards::with_program("kubectl", Duration::from_secs(READY_SECS))
    }

    /// Another program in place of `kubectl`, and how long a start waits for it (tests).
    pub fn with_program(program: &str, ready: Duration) -> LocalPortForwards {
        LocalPortForwards { inner: Forwards::with_spawn(spawn_of(program.to_string()), ready) }
    }

    /// The one list of this process.
    pub fn shared() -> Arc<LocalPortForwards> {
        static SHARED: OnceLock<Arc<LocalPortForwards>> = OnceLock::new();
        SHARED.get_or_init(|| Arc::new(LocalPortForwards::new())).clone()
    }
}

impl Default for LocalPortForwards {
    fn default() -> LocalPortForwards {
        LocalPortForwards::new()
    }
}

impl PortForwards for LocalPortForwards {
    fn start(&self, req: &Request) -> Result<Info, String> {
        self.inner.start(req)
    }
    fn list(&self) -> Vec<Info> {
        self.inner.list()
    }
    fn stop(&self, id: u32) -> bool {
        self.inner.stop(id)
    }
    fn stop_all(&self) -> usize {
        self.inner.stop_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::portforward::Target;
    use std::os::unix::fs::PermissionsExt;

    fn script(tag: &str, body: &str) -> String {
        let d = std::env::temp_dir().join(format!("iyakki-pf-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("fake");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    fn req() -> Request {
        Request { kubeconfig: String::new(), context: String::new(), namespace: "shop".into(), kind: Target::Service, name: "web".into(), remote: 80, local: None }
    }

    #[test]
    fn a_child_that_prints_the_forwarding_line_is_listed_and_stopping_it_ends_the_process() {
        let _serial = crate::starters::spawn_lock();
        let f = LocalPortForwards::with_program(&script("run", "echo \"Forwarding from 127.0.0.1:41234 -> 80\"; exec sleep 300"), Duration::from_secs(20));
        let i = f.start(&req()).unwrap();
        assert_eq!((i.id, i.local), (1, 41234));
        assert_eq!(f.list().len(), 1);
        assert!(f.stop(1));
        assert!(f.list().is_empty() && !f.stop(1));
    }

    #[test]
    fn a_child_that_ends_without_the_line_and_a_missing_program_are_errors_and_the_set_stops_what_runs_when_dropped() {
        let _serial = crate::starters::spawn_lock();
        let f = LocalPortForwards::with_program(&script("end", "echo error: no pod; exit 1"), Duration::from_secs(20));
        assert!(f.start(&req()).unwrap_err().contains("ended before"));
        let f = LocalPortForwards::with_program("/nonexistent/kubectl", Duration::from_secs(1));
        assert!(f.start(&req()).unwrap_err().contains("install kubectl"));
        let f = LocalPortForwards::with_program(&script("drop", "echo \"Forwarding from 127.0.0.1:41235 -> 80\"; exec sleep 300"), Duration::from_secs(20));
        f.start(&req()).unwrap();
        assert_eq!(f.stop_all(), 1);
        assert!(f.list().is_empty());
    }

    #[test]
    fn the_shared_list_is_one_for_every_caller() {
        assert!(Arc::ptr_eq(&LocalPortForwards::shared(), &LocalPortForwards::shared()));
    }
}
