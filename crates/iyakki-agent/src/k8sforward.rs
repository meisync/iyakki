//! Port-forwards the person asks for (F-K8S-24): `kubectl port-forward` from a free (or chosen) port of 127.0.0.1 to a pod, Service or
//! Deployment of the workspace namespace, kept while it runs, listed and stopped by number, all stopped when the set is dropped.
//!
//! Only 127.0.0.1 is ever bound (`--address=127.0.0.1`), so nothing on the network can reach a forward; local ports below 1024 are
//! refused; at most [`MAX_FORWARDS`] run at once. The process is started through the same [`Spawn`] function the domain listener uses
//! (`cloudlisten_pod.rs`), so tests use a fake. The chat tool `k8s_port_forward` (`k8stools.rs`) asks first.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cloudlisten::pod::{forwarding_port, real_spawn, Proc, Spawn};

pub const MAX_FORWARDS: usize = 8;
const READY_SECS: u64 = 10;

/// What a forward goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Pod,
    Service,
    Deployment,
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pod" | "pods" | "po" => Some(Target::Pod),
            "service" | "services" | "svc" => Some(Target::Service),
            "deployment" | "deployments" | "deploy" => Some(Target::Deployment),
            _ => None,
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Target::Pod => "pod",
            Target::Service => "svc",
            Target::Deployment => "deployment",
        }
    }
}

/// A forward that runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub id: u32,
    pub namespace: String,
    pub target: String,
    pub remote: u16,
    pub local: u16,
}

impl Info {
    pub fn line(&self) -> String {
        format!("#{} 127.0.0.1:{} -> {}/{}:{}", self.id, self.local, self.namespace, self.target, self.remote)
    }
}

struct Entry {
    info: Info,
    proc: Box<dyn Proc>,
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') && !s.starts_with(['-', '.'])
}

fn ns_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}

/// The argument array (without the leading `--kubeconfig`/`--context` flags), after every check. `local` 0 or None: kubectl picks.
pub fn command(namespace: &str, kind: Target, name: &str, remote: u16, local: Option<u16>) -> Result<Vec<String>, String> {
    if !ns_ok(namespace) {
        return Err(format!("`{namespace}` is not a namespace name"));
    }
    if !name_ok(name) {
        return Err(format!("`{name}` is not an object name"));
    }
    if remote == 0 {
        return Err("the port to reach on the object is 0".into());
    }
    let local = local.filter(|p| *p != 0);
    if let Some(p) = local {
        if p < 1024 {
            return Err(format!("local port {p} is a privileged port; choose 1024 or higher, or leave it out"));
        }
    }
    Ok(vec![
        "port-forward".into(),
        "--address=127.0.0.1".into(),
        format!("--namespace={namespace}"),
        format!("{}/{name}", kind.prefix()),
        format!("{}:{remote}", local.map(|p| p.to_string()).unwrap_or_default()),
    ])
}

/// The forwards of one tool set.
pub struct Forwards {
    spawn: Spawn,
    ready: Duration,
    live: Mutex<(u32, Vec<Entry>)>,
}

impl Forwards {
    /// The real `kubectl` with these leading flags.
    pub fn real(kubeconfig: &str, context: &str) -> Forwards {
        let mut base = Vec::new();
        for (flag, v) in [("--kubeconfig", kubeconfig), ("--context", context)] {
            if !v.is_empty() && !v.starts_with('-') && !v.chars().any(char::is_control) {
                base.push(format!("{flag}={v}"));
            }
        }
        Forwards::with_spawn(real_spawn(base), Duration::from_secs(READY_SECS))
    }

    pub fn with_spawn(spawn: Spawn, ready: Duration) -> Forwards {
        Forwards { spawn, ready, live: Mutex::new((0, Vec::new())) }
    }

    fn prune(v: &mut Vec<Entry>) {
        v.retain_mut(|e| e.proc.alive());
    }

    /// Start one; waits for the "Forwarding from" line for at most the ready time and stops the process when it does not come.
    pub fn start(&self, namespace: &str, kind: Target, name: &str, remote: u16, local: Option<u16>) -> Result<Info, String> {
        let args = command(namespace, kind, name, remote, local)?;
        let mut g = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::prune(&mut g.1);
        if g.1.len() >= MAX_FORWARDS {
            return Err(format!("{MAX_FORWARDS} forwards are running; stop one first"));
        }
        if let Some(p) = local.filter(|p| *p != 0) {
            if g.1.iter().any(|e| e.info.local == p) {
                return Err(format!("local port {p} is already used by a forward of this session"));
            }
        }
        let what = format!("{}/{name}:{remote}", kind.prefix());
        let (mut proc, lines) = (self.spawn)(&args)?;
        let until = Instant::now() + self.ready;
        loop {
            match lines.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(l) => {
                    if let Some(p) = forwarding_port(&l) {
                        g.0 += 1;
                        let info = Info { id: g.0, namespace: namespace.into(), target: format!("{}/{name}", kind.prefix()), remote, local: p };
                        g.1.push(Entry { info: info.clone(), proc });
                        return Ok(info);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    proc.kill();
                    return Err(format!("the forward to {what} was not ready within {:?}; stopped", self.ready));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    proc.kill();
                    return Err(format!("the forward to {what} ended before it was ready (is it there, does it have a running pod, is the port right, is the local port free?)"));
                }
            }
        }
    }

    /// The running ones (a forward that ended is dropped from the list).
    pub fn list(&self) -> Vec<Info> {
        let mut g = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::prune(&mut g.1);
        g.1.iter().map(|e| e.info.clone()).collect()
    }

    /// Stop one by number; false when there is none.
    pub fn stop(&self, id: u32) -> bool {
        let mut g = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match g.1.iter().position(|e| e.info.id == id) {
            Some(i) => {
                let mut e = g.1.remove(i);
                e.proc.kill();
                true
            }
            None => false,
        }
    }

    pub fn stop_all(&self) -> usize {
        let mut g = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = g.1.len();
        for mut e in g.1.drain(..) {
            e.proc.kill();
        }
        n
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Fake(Arc<AtomicBool>);
    impl Proc for Fake {
        fn alive(&mut self) -> bool {
            !self.0.load(Ordering::SeqCst)
        }
        fn kill(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A spawn that answers with `lines`, records the arguments, and hands out each process's "dead" flag.
    #[allow(clippy::type_complexity)]
    fn fake(lines: Vec<&'static str>) -> (Spawn, Arc<Mutex<Vec<Vec<String>>>>, Arc<Mutex<Vec<Arc<AtomicBool>>>>, Arc<AtomicUsize>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let flags = Arc::new(Mutex::new(Vec::new()));
        let n = Arc::new(AtomicUsize::new(0));
        let (s2, f2, n2) = (seen.clone(), flags.clone(), n.clone());
        let spawn: Spawn = Box::new(move |a: &[String]| {
            s2.lock().unwrap().push(a.to_vec());
            let i = n2.fetch_add(1, Ordering::SeqCst) as u16;
            let dead = Arc::new(AtomicBool::new(false));
            f2.lock().unwrap().push(dead.clone());
            let (tx, rx) = mpsc::channel();
            for l in &lines {
                let l = l.replace("{n}", &(40000 + i).to_string());
                let _ = tx.send(l);
            }
            // keep the sender alive for a process that is "running" (lines were all sent): drop it only when there were none
            if !lines.is_empty() {
                std::mem::forget(tx);
            }
            Ok((Box::new(Fake(dead)) as Box<dyn Proc>, rx))
        });
        (spawn, seen, flags, n)
    }

    #[test]
    fn the_argument_arrays_are_golden_and_bind_only_the_loopback() {
        assert_eq!(command("shop-dev", Target::Service, "web", 80, None).unwrap(), ["port-forward", "--address=127.0.0.1", "--namespace=shop-dev", "svc/web", ":80"]);
        assert_eq!(command("shop-dev", Target::Pod, "web-7d9-abc", 8080, Some(18080)).unwrap(), ["port-forward", "--address=127.0.0.1", "--namespace=shop-dev", "pod/web-7d9-abc", "18080:8080"]);
        assert_eq!(command("a", Target::Deployment, "api", 3000, Some(0)).unwrap()[4], ":3000");
    }

    #[test]
    fn names_ports_and_privileged_local_ports_are_refused_before_anything_starts() {
        for bad in ["", "-x", "a b", "A", "a;b", "../x", "a/b"] {
            assert!(command("ns", Target::Pod, bad, 80, None).is_err(), "{bad}");
        }
        assert!(command("Bad_NS", Target::Pod, "x", 80, None).is_err());
        assert!(command("ns", Target::Pod, "x", 0, None).is_err());
        assert!(command("ns", Target::Pod, "x", 80, Some(80)).unwrap_err().contains("privileged"));
        assert_eq!(Target::parse("SVC"), Some(Target::Service));
        assert_eq!(Target::parse("node"), None);
        let (spawn, seen, _, _) = fake(vec!["Forwarding from 127.0.0.1:{n} -> 80"]);
        let f = Forwards::with_spawn(spawn, Duration::from_secs(1));
        assert!(f.start("ns", Target::Pod, "bad name", 80, None).is_err());
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_forward_is_listed_stopped_and_all_are_stopped_on_drop() {
        let (spawn, seen, flags, _) = fake(vec!["Forwarding from 127.0.0.1:{n} -> 80", "Forwarding from [::1]:{n} -> 80"]);
        let f = Forwards::with_spawn(spawn, Duration::from_secs(1));
        let a = f.start("shop-dev", Target::Service, "web", 80, None).unwrap();
        let b = f.start("shop-dev", Target::Pod, "web-1", 8080, Some(18080)).unwrap();
        assert_eq!((a.id, a.local, b.id), (1, 40000, 2));
        assert_eq!(a.line(), "#1 127.0.0.1:40000 -> shop-dev/svc/web:80");
        assert_eq!(seen.lock().unwrap()[1][4], "18080:8080");
        assert_eq!(f.list().len(), 2);
        assert!(f.stop(1));
        assert!(!f.stop(1), "already gone");
        assert!(flags.lock().unwrap()[0].load(Ordering::SeqCst), "the process was killed");
        assert_eq!(f.list().iter().map(|i| i.id).collect::<Vec<_>>(), [2]);
        drop(f);
        assert!(flags.lock().unwrap()[1].load(Ordering::SeqCst), "dropping the set stops what runs");
    }

    #[test]
    fn a_forward_that_died_leaves_the_list_and_a_chosen_port_is_not_given_twice() {
        let (spawn, _, flags, _) = fake(vec!["Forwarding from 127.0.0.1:18080 -> 80"]);
        let f = Forwards::with_spawn(spawn, Duration::from_secs(1));
        f.start("ns", Target::Pod, "p", 80, Some(18080)).unwrap();
        assert!(f.start("ns", Target::Pod, "q", 80, Some(18080)).unwrap_err().contains("already used"));
        flags.lock().unwrap()[0].store(true, Ordering::SeqCst);
        assert!(f.list().is_empty());
        f.start("ns", Target::Pod, "q", 80, Some(18080)).unwrap();
    }

    #[test]
    fn a_forward_that_is_not_ready_or_ends_early_is_stopped_and_not_listed() {
        let (spawn, _, flags, _) = fake(vec!["error: unable to forward port"]);
        let f = Forwards::with_spawn(spawn, Duration::from_millis(100));
        assert!(f.start("ns", Target::Pod, "p", 80, None).unwrap_err().contains("not ready"));
        assert!(flags.lock().unwrap()[0].load(Ordering::SeqCst));
        let (spawn, _, flags, _) = fake(vec![]);
        let f = Forwards::with_spawn(spawn, Duration::from_secs(1));
        assert!(f.start("ns", Target::Pod, "p", 80, None).unwrap_err().contains("ended before"));
        assert!(flags.lock().unwrap()[0].load(Ordering::SeqCst));
        assert!(f.list().is_empty());
    }

    #[test]
    fn no_more_than_the_limit_run_at_once() {
        let (spawn, _, _, _) = fake(vec!["Forwarding from 127.0.0.1:{n} -> 80"]);
        let f = Forwards::with_spawn(spawn, Duration::from_secs(1));
        for i in 0..MAX_FORWARDS {
            f.start("ns", Target::Pod, &format!("p{i}"), 80, None).unwrap();
        }
        assert!(f.start("ns", Target::Pod, "extra", 80, None).unwrap_err().contains("stop one first"));
        assert_eq!(f.stop_all(), MAX_FORWARDS);
    }
}
