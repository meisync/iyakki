//! `iyakki dev follow ADDRESS[:PORT] --fingerprint FP [--seconds N] [--max N] [--retries N] [--json]`: the program client of the web service's event
//! stream (F-WEBSVC-14). It signs in with this computer's developer key (the `api:tiles` scope is needed on the service) and prints each
//! event as it arrives instead of asking again and again. It reaches only the service the person names, with its pinned fingerprint.

use std::time::Duration;

use super::cli::Args;
use super::client::{split_host, Home};
use super::webclient::{event_line, ApiClient};

pub const USAGE: &str = "  follow ADDRESS[:PORT] --fingerprint FP [--seconds N] [--max N] [--retries N] [--json]   print the web service's events as they happen (changes, notifications, run and flow progress); scope api:tiles on the service";

pub(super) fn run(cmd: &str, args: &Args, home: &Home, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    if cmd != "follow" {
        return None;
    }
    Some(follow(args, home, out))
}

fn follow(args: &Args, home: &Home, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let usage = "usage: iyakki dev follow ADDRESS[:PORT] --fingerprint FP [--seconds N] [--max N] [--retries N] [--json]";
    let (host, port) = split_host(args.pos.get(1).ok_or(usage)?, 8443)?;
    let fp = args.get("--fingerprint").ok_or("the service's certificate fingerprint is needed (`iyakki serve` prints it): --fingerprint FP")?;
    let seconds: u64 = args.get("--seconds").map(|v| v.parse().map_err(|_| "--seconds needs a number".to_string())).transpose()?.unwrap_or(60).clamp(1, 86_400);
    let max: usize = args.get("--max").map(|v| v.parse().map_err(|_| "--max needs a number".to_string())).transpose()?.unwrap_or(1000).clamp(1, 100_000);
    let json = args.has("--json");
    let api = ApiClient::new(home, &host, port, fp)?;
    let retries: u32 = args.get("--retries").map(|v| v.parse().map_err(|_| "--retries needs a number".to_string())).transpose()?.unwrap_or(5).min(100);
    // a dropped stream is opened again (signing in again) up to --retries times; the retry notices go to the same output
    let sink = std::cell::RefCell::new(&mut *out);
    let n = api.follow_reconnecting(
        Duration::from_secs(seconds),
        max,
        retries,
        Duration::from_secs(30),
        &mut |v| {
            (sink.borrow_mut())(&if json { v.to_string() } else { event_line(v) });
            true
        },
        &mut |n, why| (sink.borrow_mut())(&if json { serde_json::json!({"type": "reconnecting", "attempt": n, "why": why}).to_string() } else { format!("reconnecting (attempt {n}): {why}") }),
    );
    let n = n?;
    if !json {
        out(&format!("{n} event(s)."));
    }
    Ok(())
}
