//! `forward` scope: TCP port forwarding from the computer to services listening on the device's OWN loopback (`iyakki dev forward
//! 8080:13305`), carried over the same signed, pinned HTTPS requests as everything else. The device only ever connects to
//! `127.0.0.1`: never to another machine, never to the developer-mode port itself. A forward is a session (listed in Settings, ended
//! by Disconnect, revoke, a scope change or switching developer mode off); connections are bounded in number, idle time and buffer.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::scopes::Scope;
use super::sessions;
use super::{DevKey, DevMode};

pub const MAX_CONNS_PER_FORWARD: usize = 16;
pub const MAX_CONNS_PER_COMPUTER: usize = 64;
pub const IDLE_SECS: u64 = 300;
pub const MAX_CHUNK: usize = 256 * 1024;
const BUF_CAP: usize = 1 << 20;
pub const MAX_WAIT_MS: u64 = 20_000;

struct Conn {
    stream: TcpStream,
    buf: Mutex<(VecDeque<u8>, bool)>,
    cv: Condvar,
    last: Mutex<Instant>,
    closed: AtomicBool,
}

struct Forward {
    computer: String,
    port: u16,
    guard: sessions::Guard,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    next: AtomicU64,
}

fn registry() -> std::sync::MutexGuard<'static, HashMap<(PathBuf, u64), Arc<Forward>>> {
    static R: OnceLock<Mutex<HashMap<(PathBuf, u64), Arc<Forward>>>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn find(dm: &DevMode, fid: u64, computer: &str) -> Result<Arc<Forward>, String> {
    let f = registry().get(&(dm.data_dir().to_path_buf(), fid)).cloned().filter(|f| f.computer == computer);
    let f = f.ok_or("no such forward (it ended)")?;
    if f.guard.ended() {
        close_all(&f);
        registry().remove(&(dm.data_dir().to_path_buf(), fid));
        return Err("this forward was ended".into());
    }
    Ok(f)
}

fn close_all(f: &Forward) {
    for c in f.conns.lock().unwrap_or_else(|e| e.into_inner()).values() {
        c.closed.store(true, Ordering::SeqCst);
        let _ = c.stream.shutdown(std::net::Shutdown::Both);
        c.cv.notify_all();
    }
}

/// Start a forward to `port` on the device's loopback. `own_port` (the developer-mode listener) is refused.
pub fn start(dm: &DevMode, key: &DevKey, port: u16, own_port: u16) -> Result<u64, String> {
    if port == 0 {
        return Err("a port is 1 to 65535".into());
    }
    if port == own_port {
        return Err("that is the developer-mode port itself".into());
    }
    let guard = sessions::open(dm.data_dir(), &key.name, &key.account, Scope::Forward, &format!("forward {port}"), dm.now())?;
    let id = guard.id();
    let f = Arc::new(Forward { computer: key.name.clone(), port, guard, conns: Mutex::new(HashMap::new()), next: AtomicU64::new(1) });
    registry().insert((dm.data_dir().to_path_buf(), id), f.clone());
    // an ended forward (revoke, scope taken away, Disconnect, switch-off) closes its connections at once, not at the next request
    let key = (dm.data_dir().to_path_buf(), id);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        if f.guard.ended() {
            close_all(&f);
            registry().remove(&key);
            return;
        }
        if !registry().contains_key(&key) {
            return;
        }
    });
    Ok(id)
}

pub fn stop(dm: &DevMode, fid: u64, computer: &str) -> Result<(), String> {
    let f = find(dm, fid, computer)?;
    close_all(&f);
    registry().remove(&(dm.data_dir().to_path_buf(), fid));
    Ok(())
}

/// Open one connection of a forward to 127.0.0.1:port. A service that is not there is an error the computer sees at once.
pub fn connect(dm: &DevMode, fid: u64, computer: &str) -> Result<u64, String> {
    let f = find(dm, fid, computer)?;
    {
        let mut c = f.conns.lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|_, x| !x.closed.load(Ordering::SeqCst));
        if c.len() >= MAX_CONNS_PER_FORWARD {
            return Err(format!("{MAX_CONNS_PER_FORWARD} connections are open already on this forward"));
        }
    }
    let total: usize = registry().values().filter(|x| x.computer == computer).map(|x| x.conns.lock().unwrap_or_else(|e| e.into_inner()).len()).sum();
    if total >= MAX_CONNS_PER_COMPUTER {
        return Err(format!("{MAX_CONNS_PER_COMPUTER} forwarded connections are open for this computer"));
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, f.port));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("nothing answers on port {} of the device ({e})", f.port))?;
    let _ = stream.set_nodelay(true);
    let reader = stream.try_clone().map_err(|e| e.to_string())?;
    let c = Arc::new(Conn { stream, buf: Mutex::new((VecDeque::new(), false)), cv: Condvar::new(), last: Mutex::new(Instant::now()), closed: AtomicBool::new(false) });
    let id = f.next.fetch_add(1, Ordering::SeqCst);
    f.conns.lock().unwrap_or_else(|e| e.into_inner()).insert(id, c.clone());
    std::thread::spawn(move || pump(reader, c));
    Ok(id)
}

/// Device side reading: bytes from the service wait in a bounded buffer until the computer collects them.
fn pump(mut r: TcpStream, c: Arc<Conn>) {
    let mut chunk = [0u8; 16 * 1024];
    loop {
        {
            let mut g = c.buf.lock().unwrap_or_else(|e| e.into_inner());
            while g.0.len() >= BUF_CAP && !c.closed.load(Ordering::SeqCst) {
                g = c.cv.wait_timeout(g, Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        if c.closed.load(Ordering::SeqCst) {
            break;
        }
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut g = c.buf.lock().unwrap_or_else(|e| e.into_inner());
                g.0.extend(&chunk[..n]);
                c.cv.notify_all();
            }
        }
    }
    c.buf.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
    c.cv.notify_all();
}

fn conn(f: &Forward, cid: u64) -> Result<Arc<Conn>, String> {
    let c = f.conns.lock().unwrap_or_else(|e| e.into_inner()).get(&cid).cloned().ok_or("no such connection")?;
    if c.last.lock().unwrap_or_else(|e| e.into_inner()).elapsed() > Duration::from_secs(IDLE_SECS) {
        c.closed.store(true, Ordering::SeqCst);
        let _ = c.stream.shutdown(std::net::Shutdown::Both);
        return Err("this connection was idle for too long and was closed".into());
    }
    *c.last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    Ok(c)
}

pub fn send(dm: &DevMode, fid: u64, computer: &str, cid: u64, data: &[u8]) -> Result<(), String> {
    if data.len() > MAX_CHUNK {
        return Err(format!("at most {} KiB per request", MAX_CHUNK / 1024));
    }
    let f = find(dm, fid, computer)?;
    let c = conn(&f, cid)?;
    (&c.stream).write_all(data).map_err(|e| format!("the service closed the connection ({e})"))
}

/// Bytes from the service, waiting up to `wait_ms` for some. `Ok(None)` once the service closed and everything was collected.
pub fn recv(dm: &DevMode, fid: u64, computer: &str, cid: u64, wait_ms: u64) -> Result<Option<Vec<u8>>, String> {
    let f = find(dm, fid, computer)?;
    let c = conn(&f, cid)?;
    let deadline = Instant::now() + Duration::from_millis(wait_ms.min(MAX_WAIT_MS));
    let mut g = c.buf.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if !g.0.is_empty() {
            let n = g.0.len().min(MAX_CHUNK);
            let out: Vec<u8> = g.0.drain(..n).collect();
            c.cv.notify_all();
            return Ok(Some(out));
        }
        if g.1 || c.closed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Some(Vec::new()));
        }
        g = c.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
    }
}

pub fn close(dm: &DevMode, fid: u64, computer: &str, cid: u64) -> Result<(), String> {
    let f = find(dm, fid, computer)?;
    if let Some(c) = f.conns.lock().unwrap_or_else(|e| e.into_inner()).remove(&cid) {
        c.closed.store(true, Ordering::SeqCst);
        let _ = c.stream.shutdown(std::net::Shutdown::Both);
        c.cv.notify_all();
    }
    Ok(())
}
