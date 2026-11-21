//! `iyakki-relay`: the owner-run mailbox of docs/RELAY.md.

use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::Arc;

use iyakki_relay::http::Relay;
use iyakki_relay::store::{Limits, Store};
use iyakki_relay::{server, tls, SystemClock};

const USAGE: &str = "\
iyakki-relay: a mailbox for sealed IYAKKI envelopes (docs/RELAY.md)

USAGE:
  iyakki-relay --cert FILE --key FILE [--listen ADDR:PORT] [options]

  --cert FILE            the certificate (PEM, leaf first); required
  --key FILE             its private key (PEM, mode 600); required
  --listen ADDR:PORT     where to listen (default 0.0.0.0:8443)
  --max-wait-ms N        the longest an envelope waits (default 86400000 = 24 hours; a test relay may set it short)
  --mailbox-per-minute N envelopes one mailbox accepts per minute (default 120)
  --mailbox-depth N      envelopes one mailbox holds (default 256)
  --peer-per-minute N    requests one network address may make per minute (default 1200)
  --max-total-mib N      all queued bytes together (default 256)
  --max-mailboxes N      mailboxes held at once (default 10000)
  --max-connections N    connections at once (default 128)
  --quiet                log nothing but start and errors
  --version | --help
";

fn say(s: &str) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(s.as_bytes());
}

fn err(s: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "iyakki-relay: {s}");
}

fn run(args: &[String]) -> Result<(), String> {
    let mut cert = None;
    let mut key = None;
    let mut listen = "0.0.0.0:8443".to_string();
    let mut limits = Limits::default();
    let mut conns = server::MAX_CONNECTIONS;
    let mut quiet = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--quiet" {
            quiet = true;
            i += 1;
            continue;
        }
        let v = args.get(i + 1).ok_or_else(|| format!("{a} needs a value"))?.clone();
        let num = |name: &str| v.parse::<u64>().map_err(|_| format!("{name} needs a whole number"));
        match a {
            "--cert" => cert = Some(v.clone()),
            "--key" => key = Some(v.clone()),
            "--listen" => listen = v.clone(),
            "--max-wait-ms" => limits.max_wait_ms = num(a)?.clamp(1, iyakki_relay::store::MAX_TTL_MS),
            "--mailbox-per-minute" => limits.per_mailbox_per_minute = num(a)? as usize,
            "--mailbox-depth" => limits.mailbox_depth = num(a)? as usize,
            "--peer-per-minute" => limits.per_peer_per_minute = num(a)? as usize,
            "--max-total-mib" => limits.max_total_bytes = (num(a)? as usize).saturating_mul(1024 * 1024),
            "--max-mailboxes" => limits.max_mailboxes = num(a)? as usize,
            "--max-connections" => conns = (num(a)? as usize).max(1),
            _ => return Err(format!("unknown option {a} (see --help)")),
        }
        i += 2;
    }
    let cert = cert.ok_or("--cert is required")?;
    let key = key.ok_or("--key is required")?;
    let cfg = tls::config_from_files(&cert, &key)?;
    let listener = TcpListener::bind(&listen).map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let log: Box<dyn Fn(&str) + Send + Sync> = if quiet {
        Box::new(|_| {})
    } else {
        Box::new(|s| {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            eprintln!("{ms} {s}");
        })
    };
    let relay = Arc::new(Relay::new(Store::new(limits.clone()), Box::new(SystemClock), log));
    say(&format!("iyakki-relay {} listening on https://{addr} (envelopes wait at most {} ms)\n", env!("CARGO_PKG_VERSION"), limits.max_wait_ms));
    let running = server::start(listener, cfg, relay, conns)?;
    // Run until the process is stopped (kill, Ctrl-C): queues are in memory, nothing needs saving.
    if let Some(t) = running.thread {
        let _ = t.join();
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            say(USAGE);
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            say(&format!("iyakki-relay {}\n", env!("CARGO_PKG_VERSION")));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            err(&e);
            ExitCode::FAILURE
        }
    }
}
