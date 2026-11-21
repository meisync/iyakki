//! The accept loop. A bound listener is handed in (a test keeps its port; the program binds the address it was given).

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::http::{serve_connection, Relay};

/// Connections served at once; further ones are closed at once.
pub const MAX_CONNECTIONS: usize = 128;
/// How long a silent connection may wait for the next byte (also bounds the TLS handshake).
pub const IDLE: Duration = Duration::from_secs(20);

pub struct Running {
    pub stop: Arc<AtomicBool>,
    pub thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Stop accepting and wait for the loop to end (connections in flight finish on their own).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Serve HTTPS on `listener` until `stop` is set.
pub fn start(listener: TcpListener, tls: Arc<ServerConfig>, relay: Arc<Relay>, max_conns: usize) -> Result<Running, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let thread = std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((sock, addr)) => {
                    if active.load(Ordering::SeqCst) >= max_conns {
                        (relay.log)("connection refused: too many at once");
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let (tls, relay, active) = (tls.clone(), relay.clone(), active.clone());
                    let peer = addr.ip().to_string();
                    std::thread::spawn(move || {
                        handle(sock, tls, &relay, &peer);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    });
    Ok(Running { stop, thread: Some(thread) })
}

fn handle(sock: TcpStream, tls: Arc<ServerConfig>, relay: &Relay, peer: &str) {
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(IDLE));
    let _ = sock.set_write_timeout(Some(IDLE));
    let _ = sock.set_nodelay(true);
    let Ok(conn) = ServerConnection::new(tls) else { return };
    let mut s = StreamOwned::new(conn, sock);
    serve_connection(&mut s, relay, peer);
    s.conn.send_close_notify();
    let _ = s.flush();
}
