//! The port-forward door on the fake machine: the real bookkeeping ([`Forwards`]: checks, limit, list, stop) over a scripted `kubectl` that answers
//! "Forwarding from 127.0.0.1:4000N" (N is the start's number), a record of the argument arrays it was started with, and a way to end a process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::portforward::{scripted_spawn, Forwards, Info, PortForwards, Request};

pub struct FakePortForwards {
    inner: Forwards,
    seen: Arc<Mutex<Vec<Vec<String>>>>,
    flags: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl Default for FakePortForwards {
    fn default() -> FakePortForwards {
        FakePortForwards::with_lines(vec!["Forwarding from 127.0.0.1:{n} -> 80"])
    }
}

impl FakePortForwards {
    /// The scripted kubectl prints these lines at once (`{n}` is 40000 plus the start's number); none makes every start end before it is ready.
    pub fn with_lines(lines: Vec<&'static str>) -> FakePortForwards {
        let (spawn, seen, flags) = scripted_spawn(lines);
        FakePortForwards { inner: Forwards::with_spawn(spawn, Duration::from_millis(200)), seen, flags }
    }
    /// The argument arrays of every process started, in order.
    pub fn spawned(&self) -> Vec<Vec<String>> {
        self.seen.lock().unwrap().clone()
    }
    /// Whether the process of the n-th start (0 based) was ended (killed, or [`end`](Self::end)ed).
    pub fn ended(&self, n: usize) -> bool {
        self.flags.lock().unwrap().get(n).is_some_and(|f| f.load(Ordering::SeqCst))
    }
    /// The n-th process dies by itself (the pod went away).
    pub fn end(&self, n: usize) {
        if let Some(f) = self.flags.lock().unwrap().get(n) {
            f.store(true, Ordering::SeqCst);
        }
    }
}

impl PortForwards for FakePortForwards {
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

    fn req(name: &str) -> Request {
        Request { kubeconfig: String::new(), context: "dev".into(), namespace: "shop".into(), kind: Target::Service, name: name.into(), remote: 80, local: None }
    }

    #[test]
    fn the_fake_forwards_for_real_lists_stops_and_sees_a_process_end() {
        let f = FakePortForwards::default();
        let a = f.start(&req("web")).unwrap();
        let b = f.start(&req("api")).unwrap();
        assert_eq!((a.local, b.local), (40000, 40001));
        assert_eq!(f.spawned()[0][..2], ["--context=dev", "port-forward"]);
        f.end(0);
        assert_eq!(f.list().iter().map(|i| i.id).collect::<Vec<_>>(), [2], "an ended process leaves the list");
        assert!(f.stop(2) && f.ended(1));
        assert!(f.start(&req("Bad Name")).is_err());
        assert_eq!(f.spawned().len(), 2, "a refused request starts nothing");
    }

    #[test]
    fn a_scripted_kubectl_that_prints_nothing_gives_an_error_and_no_forward() {
        let f = FakePortForwards::with_lines(vec![]);
        assert!(f.start(&req("web")).unwrap_err().contains("ended before"));
        assert!(f.list().is_empty() && f.ended(0));
    }
}
