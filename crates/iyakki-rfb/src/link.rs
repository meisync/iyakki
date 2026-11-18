//! A session on a Unix socket, in a reader thread. The owner polls [`Link::with`] from its own thread.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::Session;

struct Shared {
    session: Session,
    writer: Option<Box<dyn Write + Send>>,
    ended: Option<String>,
}

pub struct Link {
    shared: Arc<Mutex<Shared>>,
    stream: Option<UnixStream>,
}

fn lock(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn flush(sh: &mut Shared) {
    let out = sh.session.take_output();
    if out.is_empty() {
        return;
    }
    if let Some(w) = sh.writer.as_mut() {
        if w.write_all(&out).and_then(|_| w.flush()).is_err() && sh.ended.is_none() {
            sh.ended = Some("the connection to the display server was lost".into());
        }
    }
}

impl Link {
    /// Run a session over any reader and writer (the reader is read in a thread until it ends).
    pub fn start<R: Read + Send + 'static, W: Write + Send + 'static>(mut reader: R, writer: W) -> Link {
        let shared = Arc::new(Mutex::new(Shared { session: Session::new(), writer: Some(Box::new(writer)), ended: None }));
        let sh = shared.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        lock(&sh).ended.get_or_insert_with(|| "the display server closed the connection".into());
                        return;
                    }
                    Ok(n) => {
                        let mut g = lock(&sh);
                        if let Err(e) = g.session.feed(&buf[..n]) {
                            g.ended.get_or_insert_with(|| e.to_string());
                            flush(&mut g);
                            return;
                        }
                        flush(&mut g);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        lock(&sh).ended.get_or_insert_with(|| format!("the connection to the display server failed: {e}"));
                        return;
                    }
                }
            }
        });
        Link { shared, stream: None }
    }

    /// Connect to the display server's socket, trying again until `wait` has passed (it starts a moment after the sandbox does).
    pub fn connect_unix(path: &Path, wait: Duration) -> Result<Link, String> {
        let t0 = Instant::now();
        let s = loop {
            match UnixStream::connect(path) {
                Ok(s) => break s,
                Err(e) if t0.elapsed() >= wait => return Err(format!("cannot connect to the display server: {e}")),
                Err(_) => std::thread::sleep(Duration::from_millis(150)),
            }
        };
        let r = s.try_clone().map_err(|e| e.to_string())?;
        let w = s.try_clone().map_err(|e| e.to_string())?;
        let mut l = Link::start(r, w);
        l.stream = Some(s);
        Ok(l)
    }

    /// Use the session (read the picture, take events, send input); what it queued is written afterwards.
    pub fn with<T>(&self, f: impl FnOnce(&mut Session) -> T) -> T {
        let mut g = lock(&self.shared);
        let r = f(&mut g.session);
        flush(&mut g);
        r
    }

    /// Why the link ended, when it did.
    pub fn ended(&self) -> Option<String> {
        lock(&self.shared).ended.clone()
    }

    pub fn close(&mut self) {
        if let Some(s) = self.stream.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let mut g = lock(&self.shared);
        g.writer = None;
        g.ended.get_or_insert_with(|| "closed".into());
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

