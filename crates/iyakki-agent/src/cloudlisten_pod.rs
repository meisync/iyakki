//! Targets that are a pod or service of a cluster, reached from a local listener (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3c).
//!
//! Two [`ServiceDialer`]s:
//! * [`ClusterDns`]: a direct connection to `NAME.NAMESPACE.svc.cluster.local:PORT`, for a machine that is inside the cluster network (or
//!   has the cluster DNS and routes). The name is resolved in a helper thread so a dead resolver cannot hold a request past the timeout.
//! * [`PortForward`]: `kubectl port-forward` to `svc/NAME` on a free local port, kept while it lives and started again when it has died.
//!   The process is started through a [`Spawn`] function, so tests use a fake. Start-up (the "Forwarding from" line) is waited for at most
//!   `ready_timeout`; a process that does not say it is ready is killed. All forwards are killed when the dialer is dropped.
//!
//! `run_kubectl` of `cloudcontrol.rs` waits for the command to end, which a port-forward never does, so this module starts `kubectl` itself
//! with the same `--kubeconfig`/`--context` flags.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::ServiceDialer;
use crate::cloudtenant::{dns_label, dns_subdomain};

fn check(name: &str, port: u16) -> io::Result<()> {
    if !dns_label(name) || port == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("service \"{name}\" port {port} is not valid")));
    }
    Ok(())
}

fn connect_any(addrs: &[SocketAddr], timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the name has no address");
    for a in addrs {
        match TcpStream::connect_timeout(a, timeout) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// A direct connection to the Service's cluster DNS name.
pub struct ClusterDns {
    namespace: String,
    domain: String,
    /// Replaces the resolver (tests): name and port -> addresses.
    resolve: Arc<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>,
}

impl ClusterDns {
    /// `domain` is normally `svc.cluster.local`.
    pub fn new(namespace: &str, domain: &str) -> Result<ClusterDns, String> {
        if !dns_label(namespace) || !dns_subdomain(domain) {
            return Err(format!("namespace \"{namespace}\" or domain \"{domain}\" is not a name"));
        }
        Ok(ClusterDns { namespace: namespace.into(), domain: domain.into(), resolve: Arc::new(|n, p| (n, p).to_socket_addrs().map(|i| i.collect())) })
    }

    pub fn with_resolver(mut self, r: Box<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>) -> ClusterDns {
        self.resolve = Arc::from(r);
        self
    }

    pub fn fqdn(&self, name: &str) -> String {
        format!("{name}.{}.{}", self.namespace, self.domain)
    }
}

impl ServiceDialer for ClusterDns {
    fn dial(&self, name: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
        check(name, port)?;
        let fqdn = self.fqdn(name);
        // the resolver may block longer than the timeout: ask in a thread of its own and stop waiting (the thread ends when it ends)
        let (tx, rx) = mpsc::channel();
        let (asked, resolver) = (fqdn.clone(), self.resolve.clone());
        let started = Instant::now();
        std::thread::Builder::new().name("cloud-svc-dns".into()).spawn(move || {
            let _ = tx.send(resolver(&asked, port));
        })?;
        let addrs = rx.recv_timeout(timeout).map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{fqdn} did not resolve within {timeout:?}")))??;
        connect_any(&addrs, timeout.saturating_sub(started.elapsed()).max(Duration::from_millis(50)))
    }
}

/// A started process, as far as the forwarder needs it.
pub trait Proc: Send {
    fn alive(&mut self) -> bool;
    fn kill(&mut self);
}

/// Start `kubectl` with these arguments; returns the process and the lines it prints on standard output.
pub type Spawn = Box<dyn Fn(&[String]) -> Result<(Box<dyn Proc>, Receiver<String>), String> + Send + Sync>;

struct Real(Child);

impl Proc for Real {
    fn alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The real `kubectl`. `base` are the leading flags (`--kubeconfig=...`, `--context=...`).
pub fn real_spawn(base: Vec<String>) -> Spawn {
    Box::new(move |args: &[String]| {
        let mut child = Command::new("kubectl")
            .args(&base)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("kubectl could not be started ({e}): install kubectl"))?;
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

struct Fwd {
    proc: Box<dyn Proc>,
    local: u16,
}

/// `kubectl port-forward` to a Service.
pub struct PortForward {
    namespace: String,
    spawn: Spawn,
    ready_timeout: Duration,
    live: Mutex<HashMap<(String, u16), Fwd>>,
}

impl PortForward {
    pub fn new(namespace: &str, kubeconfig: Option<&str>, context: Option<&str>) -> Result<PortForward, String> {
        let mut base = Vec::new();
        for (flag, v) in [("--kubeconfig", kubeconfig), ("--context", context)] {
            if let Some(v) = v {
                if v.is_empty() || v.starts_with('-') || v.chars().any(char::is_control) {
                    return Err(format!("{flag} value is not usable"));
                }
                base.push(format!("{flag}={v}"));
            }
        }
        PortForward::with_spawn(namespace, real_spawn(base), Duration::from_secs(10))
    }

    pub fn with_spawn(namespace: &str, spawn: Spawn, ready_timeout: Duration) -> Result<PortForward, String> {
        if !dns_label(namespace) {
            return Err(format!("namespace \"{namespace}\" is not a name"));
        }
        Ok(PortForward { namespace: namespace.into(), spawn, ready_timeout, live: Mutex::new(HashMap::new()) })
    }

    /// The arguments of the command for a service (shown by tests and by dry runs).
    pub fn command(&self, name: &str, port: u16) -> Vec<String> {
        vec!["port-forward".into(), "--address=127.0.0.1".into(), format!("--namespace={}", self.namespace), format!("svc/{name}"), format!(":{port}")]
    }

    /// The local port of a running forward, started if needed.
    fn local_port(&self, name: &str, port: u16) -> Result<u16, String> {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let key = (name.to_string(), port);
        if let Some(f) = live.get_mut(&key) {
            if f.proc.alive() {
                return Ok(f.local);
            }
            live.remove(&key);
        }
        let (mut proc, lines) = (self.spawn)(&self.command(name, port))?;
        let until = Instant::now() + self.ready_timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match lines.recv_timeout(left) {
                Ok(l) => {
                    if let Some(p) = forwarding_port(&l) {
                        live.insert(key, Fwd { proc, local: p });
                        return Ok(p);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    proc.kill();
                    return Err(format!("port-forward to svc/{name}:{port} was not ready within {:?}; stopped", self.ready_timeout));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    proc.kill();
                    return Err(format!("port-forward to svc/{name}:{port} ended before it was ready (is the service there, and does it have a pod?)"));
                }
            }
        }
    }

    /// Number of forwards running (tests).
    pub fn running(&self) -> usize {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// `Forwarding from 127.0.0.1:41234 -> 80` gives 41234.
pub fn forwarding_port(line: &str) -> Option<u16> {
    let rest = line.strip_prefix("Forwarding from 127.0.0.1:")?;
    rest.split_whitespace().next()?.parse().ok().filter(|p| *p != 0)
}

impl ServiceDialer for PortForward {
    fn dial(&self, name: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
        check(name, port)?;
        let local = self.local_port(name, port).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let a: SocketAddr = ([127, 0, 0, 1], local).into();
        match connect_any(&[a], timeout) {
            Ok(s) => Ok(s),
            Err(e) => {
                // the forward may have died between the check and now; drop it so the next request starts a new one
                if let Some(mut f) = self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(&(name.to_string(), port)) {
                    f.proc.kill();
                }
                Err(e)
            }
        }
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        for (_, mut f) in self.live.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            f.proc.kill();
        }
    }
}
