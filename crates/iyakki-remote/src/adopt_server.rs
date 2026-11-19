//! The listener of the first-run adopt mode (`iyakki_agent::cloudadopt`): HTTPS on one port (8443) while a cloud node has no administrator.
//! It prints the one-time code on the console, announces `iyakki-<id>.local` with the code in the TXT record, serves the pairing page, makes
//! the first administrator, and then closes and calls `on_adopted` (the normal web front end starts there). Local networks only.

use std::io::{Read, Write};

/// One line on the console that never panics: a closed or hung-up console is not a reason to stop the sign-in page.
fn console_line(text: &str) {
    let mut o = std::io::stdout();
    let _ = o.write_all(format!("{text}\n").as_bytes());
    let _ = o.flush();
}
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iyakki_agent::cloudadopt::{code_from, handle, Adopt};
use iyakki_agent::devname::{self, mdns};
use iyakki_agent::httpmini::{parse, Parse};

pub const PORT: u16 = 8443;

/// Are there accounts? A missing or unreadable file means none.
pub fn admin_exists(users: &Path) -> bool {
    std::fs::read_to_string(users).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["users"].as_array().map(|a| !a.is_empty())).unwrap_or(false)
}

fn random8() -> [u8; 8] {
    let mut b = [0u8; 8];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // no random source: refuse rather than guess (the caller treats this as a failure to start)
        return [0; 8];
    }
    b
}

/// Start adopt mode in its own thread. `data_dir` holds `tls/`, `users.json` and `device-id`. Returns at once.
pub fn start(data_dir: PathBuf, port: u16, on_adopted: Box<dyn FnOnce() + Send>) -> Result<(), String> {
    let users = data_dir.join("users.json");
    if admin_exists(&users) {
        return Err("an administrator exists".into());
    }
    let rnd = random8();
    if rnd == [0; 8] {
        return Err("no random numbers (/dev/urandom)".into());
    }
    let label = devname::label(&data_dir);
    let names = vec![label.clone(), format!("{label}.local")];
    let id = crate::tls::identity(&data_dir.join("tls"), &names)?;
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("adopt: cannot listen on {port}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    // wall clock may be wrong early in boot; the expiry only needs a steady clock
    let t0 = Instant::now();
    let mut st = Adopt::new(code_from(&rnd), 0);
    let console = st.console_text(&label, 0);
    // `println!` panics when the console answers with an I/O error (seen on a laptop right after the splash ended: "failed printing to
    // stdout", which stops the shell); these lines are for a person at the screen or the serial line, so a failed write is not an error
    console_line(&console);
    console_line(&format!("IYAKKI-ADOPT certificate SHA-256 {}", id.fingerprint));
    log::info!("{console}");
    let svc = mdns::responder::Svc { kind: mdns::WEB_SERVICE.into(), port, txt: st.txt(&label) };
    let announcer = mdns::net::Announcer::start(&label, vec![svc]);
    if let Err(e) = &announcer {
        log::warn!("adopt: the name {label}.local is not announced: {e} (use the node's address)");
    }
    let cfg = id.config.clone();
    std::thread::Builder::new()
        .name("iyakki-adopt".into())
        .spawn(move || {
            let mut announcer = announcer.ok();
            let mut last_check = Instant::now();
            let mut adopted = false;
            while !adopted {
                if last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    if admin_exists(&users) {
                        break;
                    }
                }
                match listener.accept() {
                    Ok((s, peer)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                        let Ok(conn) = rustls::ServerConnection::new(cfg.clone()) else { continue };
                        let mut tls = rustls::StreamOwned::new(conn, s);
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 2048];
                        let req = loop {
                            match parse(&buf) {
                                Parse::Done(r) => break Some(r),
                                Parse::Bad(..) => break None,
                                Parse::Incomplete => {}
                            }
                            match tls.read(&mut chunk) {
                                Ok(0) | Err(_) => break None,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let Some(req) = req else { continue };
                        let exists = admin_exists(&users);
                        let uf = users.clone();
                        let (reply, done) = handle(&mut st, &req.method, &req.path, &String::from_utf8_lossy(&req.body), peer.ip(), t0.elapsed().as_secs(), exists, &mut |name, pw| {
                            let mut store = iyakki_shell::accounts::UserStore::open(&uf).map_err(|e| e.to_string())?;
                            store.create_first_admin(name, pw).map_err(|e| e.to_string())
                        });
                        let head = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'\r\nConnection: close\r\n\r\n", reply.status, if reply.status == 200 { "OK" } else { "Refused" }, reply.content_type, reply.body.len());
                        let _ = tls.write_all(head.as_bytes());
                        let _ = tls.write_all(reply.body.as_bytes());
                        tls.conn.send_close_notify();
                        let _ = tls.flush();
                        adopted = done;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
            drop(listener);
            if let Some(a) = announcer.as_mut() {
                a.stop();
            }
            console_line("IYAKKI-ADOPT done: the administrator exists; the sign-in page starts");
            on_adopted();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_exists_reads_the_users_file() {
        let d = std::env::temp_dir().join(format!("iyakki-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("users.json");
        assert!(!admin_exists(&f));
        std::fs::write(&f, "{\"users\":[]}").unwrap();
        assert!(!admin_exists(&f));
        std::fs::write(&f, "{\"users\":[{\"name\":\"a\",\"hash\":\"h\",\"role\":\"admin\"}]}").unwrap();
        assert!(admin_exists(&f));
        let _ = std::fs::remove_dir_all(&d);
    }
}
