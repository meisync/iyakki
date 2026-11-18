//! The phone relay: while a vendor sign-in waits for a code that the browser shows (Claude's `code#state`, a hundred characters or
//! more), the person can paste it on the phone that signed in instead of typing it on this device. The device serves one tiny page,
//! `/login/<one-time token>`, over HTTPS on its own addresses; what is submitted is handed to the sign-in dialog, which forwards it to
//! the vendor program exactly as if typed.
//!
//! This file is the whole policy and none of the networking: [`handle`] answers one request (the listener of `iyakki-remote` calls it,
//! on the web front end's port or on a listener of its own for the length of the login), so every rule is tested without a socket.
//!
//! * The token is 128 random bits, valid for one submission at a time (the dialog re-arms it when the vendor rejects the code), and
//!   dies with the dialog or after [`MAX_LIFE`].
//! * Only a text body up to [`MAX_BODY`] bytes is read; at most [`PER_MINUTE`] requests per address per minute.
//! * With no sign-in waiting the answer is always the same 404, whatever the path.
//! * Nothing is logged but "login relay used"; the code never reaches a log, the audit or an error.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The longest a relay lives (the dialog's own limit).
pub const MAX_LIFE: Duration = Duration::from_secs(15 * 60);
/// A code (or a pasted redirect address) is at most this many bytes.
pub const MAX_BODY: usize = 8 * 1024;
pub const PER_MINUTE: usize = 20;

struct Active {
    token: String,
    until: Instant,
    /// A code has been submitted and not yet taken (or taken and not re-armed).
    used: bool,
    submitted: Option<String>,
    label: String,
    hits: Vec<(String, Instant)>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Active>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start waiting for a code: a new random token (any earlier relay ends). `label` names the program on the page ("Claude Code").
pub fn open(label: &str) -> String {
    let mut b = [0u8; 16];
    if iyakki_sec::random_fill(&mut b).is_err() {
        // no randomness, no relay: a guessable token would be worse than none
        close();
        return String::new();
    }
    let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
    *lock() = Some(Active { token: token.clone(), until: Instant::now() + MAX_LIFE, used: false, submitted: None, label: label.chars().filter(|c| c.is_alphanumeric() || *c == ' ').take(40).collect(), hits: vec![] });
    token
}

/// The sign-in ended or the dialog closed.
pub fn close() {
    *lock() = None;
}

/// Is a relay open (and not expired)?
pub fn waiting() -> bool {
    lock().as_ref().is_some_and(|a| Instant::now() < a.until)
}

/// The code the phone sent, once (the dialog calls this on every heartbeat).
pub fn take_submitted() -> Option<String> {
    lock().as_mut().and_then(|a| a.submitted.take())
}

/// The vendor rejected the code: the same page accepts another.
pub fn rearm() {
    if let Some(a) = lock().as_mut() {
        a.used = false;
    }
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub type Reply = (u16, &'static str, Vec<u8>);

fn page(label: &str, note: &str) -> Reply {
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Send the sign-in code</title>
<style>body{{font:18px/1.45 system-ui,sans-serif;margin:0;padding:20px;background:#10142a;color:#e7ebf7}}h1{{font-size:24px;margin:0 0 8px}}p{{color:#a5afca}}textarea{{width:100%;box-sizing:border-box;height:180px;font:20px/1.3 ui-monospace,monospace;padding:12px;border-radius:12px;border:2px solid #4b5a8c;background:#0b0e1c;color:#fff;margin:12px 0}}button{{font:bold 22px system-ui;padding:16px 32px;border:0;border-radius:14px;background:#6ee0a0;color:#06210f;width:100%}}#n{{color:#ffc45e}}</style></head><body>
<h1>Send the sign-in code to your device</h1><p>{note}1. On this phone, sign in to {label} and copy the code the page shows (it has a copy button).<br>2. Paste it below and press Send. Nothing has to be typed.</p>
<form method="post"><textarea name="code" autocomplete="off" autocapitalize="off" spellcheck="false" placeholder="Paste the code here" id="c" oninput="document.getElementById('n').textContent=this.value.trim().length+' characters'"></textarea><div id="n"></div><button type="submit">Send</button></form>
<p>This page works once, only while the sign-in on your device is waiting. If your browser warned about the certificate, that is because the device made its own; it is safe to continue on your own network.</p></body></html>"#,
        note = note,
        label = label
    );
    (200, "text/html; charset=utf-8", html.into_bytes())
}

fn plain(code: u16, text: &str) -> Reply {
    (code, "text/html; charset=utf-8", format!("<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><body style=\"font:20px system-ui;padding:24px;background:#10142a;color:#e7ebf7\">{text}</body>").into_bytes())
}

/// Answer one request, or `None` when the path is not ours (the caller then answers as it always does). `peer` is the address of the
/// other end, for the rate limit.
pub fn handle(method: &str, path: &str, content_type: Option<&str>, body: &[u8], peer: &str) -> Option<Reply> {
    let token = path.strip_prefix("/login/")?;
    let token = token.split(['?', '#']).next().unwrap_or("");
    let mut g = lock();
    let not_valid = || plain(404, "This sign-in link is not valid any more. Start the sign-in again on your device.");
    let Some(a) = g.as_mut() else { return Some(not_valid()) };
    let now = Instant::now();
    a.hits.retain(|(_, t)| now.duration_since(*t) < Duration::from_secs(60));
    if a.hits.iter().filter(|(p, _)| p == peer).count() >= PER_MINUTE {
        return Some(plain(429, "Too many tries. Wait a minute."));
    }
    a.hits.push((peer.to_string(), now));
    if now >= a.until || !same(token, &a.token) {
        return Some(not_valid());
    }
    match method {
        "GET" if !a.used => Some(page(&a.label, "")),
        "GET" => Some(plain(410, "The code was sent. Look at your device.")),
        "POST" if a.used => Some(plain(410, "A code was already sent. Look at your device.")),
        "POST" => {
            if body.len() > MAX_BODY {
                return Some(plain(413, "That is too long to be a sign-in code."));
            }
            if content_type.is_some_and(|c| !c.starts_with("application/x-www-form-urlencoded") && !c.starts_with("text/plain")) {
                return Some(plain(415, "Send the code from the page."));
            }
            let text = String::from_utf8_lossy(body);
            let raw = if content_type.is_some_and(|c| c.starts_with("text/plain")) { text.into_owned() } else { form_value(&text, "code") };
            if raw.trim().is_empty() {
                return Some(page(&a.label, "<b id=\"n\">Nothing was pasted. </b>"));
            }
            a.used = true;
            a.submitted = Some(raw);
            log::info!("login relay used");
            Some(plain(200, "<h1>Sent</h1>The code is on your device now. You can close this page and look at the device."))
        }
        _ => Some((405, "text/plain", b"method not allowed".to_vec())),
    }
}

/// The value of `name` in an `application/x-www-form-urlencoded` body.
fn form_value(body: &str, name: &str) -> String {
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return decode(v);
            }
        }
    }
    String::new()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() => {
                let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(h(b[i + 1]) << 4 | h(b[i + 2]));
                i += 3;
                continue;
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ------------------------------------------------------------------------------------------------ the listener hook

/// Where the relay page is served: the web front end's own port when it runs, else a listener started for the length of the login
/// (by `iyakki-core`, which owns the certificate). The shell asks; nothing here opens a socket.
pub trait Listener: Send + Sync {
    /// The port and whether it speaks HTTPS; starts a listener when none is running.
    fn start(&self) -> Option<(u16, bool)>;
    /// The login ended: stop a listener started for it.
    fn stop(&self);
}

static LISTENER: Mutex<Option<std::sync::Arc<dyn Listener>>> = Mutex::new(None);
static WEB: Mutex<Option<(u16, bool)>> = Mutex::new(None);

pub fn set_listener(l: std::sync::Arc<dyn Listener>) {
    *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
}

/// The browser front end is running on this port: the relay page is served there.
pub fn note_web_front_end(port: u16, tls: bool) {
    *WEB.lock().unwrap_or_else(|e| e.into_inner()) = Some((port, tls));
}

/// The browser front end was closed (Settings -> Remote access switched off): the relay page is no longer served there.
pub fn clear_web_front_end() {
    *WEB.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `(port, https)` where the page can be reached, or `None` when this device cannot serve it.
pub fn port() -> Option<(u16, bool)> {
    if let Some(w) = *WEB.lock().unwrap_or_else(|e| e.into_inner()) {
        return Some(w);
    }
    let l = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    l.and_then(|l| l.start())
}

/// End the relay and any listener started for it.
pub fn finish() {
    close();
    if let Some(l) = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        l.stop();
    }
}

/// May a request from this address be answered? Only the machine's own networks: private IPv4 ranges, link-local, and the IPv6
/// equivalents. Loopback and public addresses are refused, so the page is never reachable from beyond the local network.
pub fn lan_peer(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => v.is_private() || v.is_link_local(),
        std::net::IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || v.to_ipv4_mapped().is_some_and(|m| m.is_private() || m.is_link_local())
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
