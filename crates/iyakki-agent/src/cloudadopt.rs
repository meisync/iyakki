//! First-run "adopt" mode of a headless cloud node (docs/CLOUD-IMAGES.md, "Adopting a node"). A node with no account and no screen prints a
//! one-time pairing code on its console and puts it in its mDNS TXT record; the owner opens `https://iyakki-<label>.local:8443/`, types the
//! code and makes the first administrator. This file is the pure part: the code, its limits and the page handler. The listener is
//! `iyakki_remote::adopt_server`.
//!
//! Limits (all tested): the code expires after 15 minutes; 5 wrong tries in a minute lock the page for a minute; 20 wrong tries burn the
//! code (a restart makes a new one); only peers on a local network are answered (loopback, private, link-local, unique-local); it works
//! only while no account exists, and never after the first administrator was made.
//! The TXT record is readable by everyone on the local network, so the code protects against the wider internet and against a late
//! visitor, not against someone already on the same network segment during the 15 minutes (a design choice of the owner, noted in the docs).

use std::net::IpAddr;

pub const EXPIRES_SECS: u64 = 15 * 60;
pub const WINDOW_SECS: u64 = 60;
pub const WINDOW_TRIES: usize = 5;
pub const BURN_AFTER: u32 = 20;
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// A code like `K7QD-4MXA` from eight random bytes (5 bits of each are used; 40 bits).
pub fn code_from(rand: &[u8; 8]) -> String {
    let c: String = rand.iter().map(|b| ALPHABET[(*b & 31) as usize] as char).collect();
    format!("{}-{}", &c[..4], &c[4..])
}

/// Is the address on a local network? (Loopback, 10/8, 172.16/12, 192.168/16, 169.254/16, fc00::/7, fe80::/10.)
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return is_local(IpAddr::V4(m));
            }
            let s = v.segments()[0];
            v.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refuse {
    NotLocal,
    NotNeeded,
    Expired,
    Burnt,
    Locked(u64),
    Wrong,
}

impl Refuse {
    pub fn text(&self) -> String {
        match self {
            Refuse::NotLocal => "This page answers only on the local network.".into(),
            Refuse::NotNeeded => "This node already has an administrator.".into(),
            Refuse::Expired => "The code has expired. Restart the node to get a new one.".into(),
            Refuse::Burnt => "Too many wrong codes. Restart the node to get a new one.".into(),
            Refuse::Locked(s) => format!("Too many wrong codes. Try again in {s} s."),
            Refuse::Wrong => "That code is not right.".into(),
        }
    }
}

pub struct Adopt {
    code: String,
    issued: u64,
    wrong: Vec<u64>,
    total_wrong: u32,
    locked_until: u64,
}

fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        d |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(1);
    }
    d == 0
}

impl Adopt {
    pub fn new(code: String, now: u64) -> Adopt {
        Adopt { code, issued: now, wrong: Vec::new(), total_wrong: 0, locked_until: 0 }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn expired(&self, now: u64) -> bool {
        now >= self.issued + EXPIRES_SECS
    }

    /// The line for the console.
    pub fn console_text(&self, id: &str, now: u64) -> String {
        let left = (self.issued + EXPIRES_SECS).saturating_sub(now) / 60;
        format!("IYAKKI-ADOPT node {id}: open https://{id}.local:8443/ and type the code {} (valid {left} min, until the first administrator exists)", self.code)
    }

    /// The mDNS TXT entries.
    pub fn txt(&self, id: &str) -> Vec<String> {
        vec![format!("adopt={}", self.code), format!("id={id}"), format!("expires={}", self.issued + EXPIRES_SECS)]
    }

    /// May this peer try now? (Everything except the code itself.)
    pub fn admit(&self, peer: IpAddr, now: u64, admin_exists: bool) -> Result<(), Refuse> {
        if !is_local(peer) {
            return Err(Refuse::NotLocal);
        }
        if admin_exists {
            return Err(Refuse::NotNeeded);
        }
        if self.total_wrong >= BURN_AFTER {
            return Err(Refuse::Burnt);
        }
        if self.expired(now) {
            return Err(Refuse::Expired);
        }
        if now < self.locked_until {
            return Err(Refuse::Locked(self.locked_until - now));
        }
        Ok(())
    }

    /// Check a typed code (case and the dash do not matter). A wrong one counts.
    pub fn check(&mut self, given: &str, peer: IpAddr, now: u64, admin_exists: bool) -> Result<(), Refuse> {
        self.admit(peer, now, admin_exists)?;
        let norm = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
        if same(&norm(given), &norm(&self.code)) {
            return Ok(());
        }
        self.total_wrong += 1;
        self.wrong.retain(|t| now < t + WINDOW_SECS);
        self.wrong.push(now);
        if self.wrong.len() >= WINDOW_TRIES {
            self.locked_until = now + WINDOW_SECS;
            self.wrong.clear();
        }
        Err(Refuse::Wrong)
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

fn html(status: u16, inner: &str) -> Reply {
    Reply { status, content_type: "text/html; charset=utf-8", body: format!("<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>IYAKKI node</title><body style=\"font-family:sans-serif;max-width:30em;margin:3em auto\"><h1>IYAKKI node</h1>{inner}</body>") }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn form_value(body: &str, key: &str) -> String {
    let dec = |s: &str| {
        let b = s.as_bytes();
        let mut o = Vec::new();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => o.push(b' '),
                b'%' if i + 2 < b.len() => {
                    let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                    match h {
                        Some(v) => {
                            o.push(v);
                            i += 2;
                        }
                        None => o.push(b'%'),
                    }
                }
                c => o.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&o).into_owned()
    };
    body.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| dec(v)).unwrap_or_default()
}

const FORM: &str = "<p>This node has no administrator yet. Type the code shown on its console (or in its mDNS record) and choose the administrator's name and password.</p><form method=post action=/adopt><p><label>Code <input name=code autocomplete=off required></label></p><p><label>Name <input name=name value=admin required></label></p><p><label>Password (at least 10 characters) <input name=password type=password minlength=10 required></label></p><p><button>Create the administrator</button></p></form>";

/// One request. `create(name, password)` makes the first administrator (the caller's account store). Returns the reply and whether the
/// node is now adopted (the listener should then stop and the normal web front end start).
pub fn handle(st: &mut Adopt, method: &str, path: &str, body: &str, peer: IpAddr, now: u64, admin_exists: bool, create: &mut dyn FnMut(&str, &str) -> Result<(), String>) -> (Reply, bool) {
    if !is_local(peer) {
        return (html(403, &esc(&Refuse::NotLocal.text())), false);
    }
    match (method, path) {
        ("GET", "/healthz") => (Reply { status: 200, content_type: "application/json", body: "{\"status\":\"ok\",\"mode\":\"adopt\"}".into() }, false),
        ("GET", "/") => match st.admit(peer, now, admin_exists) {
            Ok(()) => (html(200, FORM), false),
            Err(r) => (html(403, &esc(&r.text())), false),
        },
        ("POST", "/adopt") => {
            if let Err(r) = st.check(&form_value(body, "code"), peer, now, admin_exists) {
                return (html(403, &format!("{}<p><a href=/>Back</a></p>", esc(&r.text()))), false);
            }
            let (name, pw) = (form_value(body, "name"), form_value(body, "password"));
            if name.is_empty() || name.len() > 32 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return (html(400, "<p>The name may use letters, digits, - and _ (at most 32).</p><p><a href=/>Back</a></p>"), false);
            }
            if pw.chars().count() < 10 {
                return (html(400, "<p>The password needs at least 10 characters.</p><p><a href=/>Back</a></p>"), false);
            }
            match create(&name, &pw) {
                Ok(()) => (html(200, "<p>The administrator was created. This page is closed; the sign-in page opens in a moment: <a href=/>continue</a>.</p>"), true),
                Err(e) => (html(500, &format!("<p>Could not create the account: {}</p>", esc(&e))), false),
            }
        }
        _ => (html(404, "<p>Not found.</p>"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn st() -> Adopt {
        Adopt::new("K7QD-4MXA".into(), 1000)
    }

    #[test]
    fn code_has_the_expected_shape_and_alphabet() {
        let c = code_from(&[0, 1, 2, 3, 4, 5, 6, 255]);
        assert_eq!(c.len(), 9);
        assert_eq!(c.as_bytes()[4], b'-');
        assert!(c.chars().filter(|c| *c != '-').all(|c| ALPHABET.contains(&(c as u8))));
        assert_ne!(code_from(&[1; 8]), code_from(&[2; 8]));
    }

    #[test]
    fn only_local_networks_are_answered() {
        for a in ["127.0.0.1", "10.1.2.3", "172.16.0.9", "192.168.1.5", "169.254.3.3", "::1", "fd12::1", "fe80::1", "::ffff:192.168.1.5"] {
            assert!(is_local(ip(a)), "{a}");
        }
        for a in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(!is_local(ip(a)), "{a}");
        }
        let mut s = st();
        assert_eq!(s.check("K7QD-4MXA", ip("8.8.8.8"), 1001, false), Err(Refuse::NotLocal));
    }

    #[test]
    fn the_right_code_works_in_any_case_and_a_wrong_one_does_not() {
        let mut s = st();
        assert_eq!(s.check("k7qd4mxa", ip("192.168.1.5"), 1001, false), Ok(()));
        assert_eq!(s.check("AAAA-AAAA", ip("192.168.1.5"), 1001, false), Err(Refuse::Wrong));
    }

    #[test]
    fn the_code_expires_after_fifteen_minutes() {
        let mut s = st();
        assert_eq!(s.check("K7QD-4MXA", ip("10.0.0.1"), 1000 + EXPIRES_SECS - 1, false), Ok(()));
        assert_eq!(s.check("K7QD-4MXA", ip("10.0.0.1"), 1000 + EXPIRES_SECS, false), Err(Refuse::Expired));
    }

    #[test]
    fn five_wrong_tries_lock_the_page_for_a_minute_and_twenty_burn_the_code() {
        let mut s = st();
        let p = ip("10.0.0.1");
        for i in 0..5 {
            assert_eq!(s.check("BAD", p, 1001 + i, false), Err(Refuse::Wrong));
        }
        assert!(matches!(s.check("K7QD-4MXA", p, 1010, false), Err(Refuse::Locked(_))), "even the right code waits");
        assert_eq!(s.check("K7QD-4MXA", p, 1001 + 4 + WINDOW_SECS, false), Ok(()));
        let mut s = st();
        let (mut t, mut counted) = (1001, 0);
        while counted < BURN_AFTER {
            match s.check("BAD", p, t, false) {
                Err(Refuse::Wrong) => counted += 1,
                Err(Refuse::Locked(_)) => t += WINDOW_SECS,
                other => panic!("{other:?}"),
            }
            t += 1;
        }
        assert!(t < 1000 + EXPIRES_SECS, "burnt before it expired");
        assert_eq!(s.check("K7QD-4MXA", p, t, false), Err(Refuse::Burnt));
    }

    #[test]
    fn nothing_works_once_an_administrator_exists() {
        let mut s = st();
        assert_eq!(s.check("K7QD-4MXA", ip("10.0.0.1"), 1001, true), Err(Refuse::NotNeeded));
    }

    #[test]
    fn console_and_txt_carry_the_code_and_the_id() {
        let s = st();
        let t = s.console_text("iyakki-a1b2", 1000);
        assert!(t.contains("https://iyakki-a1b2.local:8443/") && t.contains("K7QD-4MXA") && t.contains("15 min"), "{t}");
        let x = s.txt("iyakki-a1b2");
        assert!(x.contains(&"adopt=K7QD-4MXA".to_string()) && x.contains(&"expires=1900".to_string()));
    }

    #[test]
    fn the_page_makes_the_first_administrator_once() {
        let mut s = st();
        let mut made = Vec::new();
        let p = ip("192.168.1.9");
        let (r, done) = handle(&mut s, "GET", "/healthz", "", p, 1001, false, &mut |_, _| Ok(()));
        assert!(r.status == 200 && r.body.contains("adopt") && !done);
        let (r, _) = handle(&mut s, "GET", "/", "", p, 1001, false, &mut |_, _| Ok(()));
        assert!(r.status == 200 && r.body.contains("<form") && !r.body.contains("K7QD"), "the page never shows the code");
        let (r, done) = handle(&mut s, "POST", "/adopt", "code=BAD&name=admin&password=longenough1", p, 1001, false, &mut |_, _| Ok(()));
        assert!(r.status == 403 && !done);
        let (r, done) = handle(&mut s, "POST", "/adopt", "code=k7qd-4mxa&name=admin&password=short", p, 1002, false, &mut |_, _| Ok(()));
        assert!(r.status == 400 && !done);
        let (r, done) = handle(&mut s, "POST", "/adopt", "code=K7QD-4MXA&name=admin&password=a+long%21pass", p, 1003, false, &mut |n, pw| {
            made.push((n.to_string(), pw.to_string()));
            Ok(())
        });
        assert!(r.status == 200 && done);
        assert_eq!(made, vec![("admin".to_string(), "a long!pass".to_string())]);
        let (r, done) = handle(&mut s, "POST", "/adopt", "code=K7QD-4MXA&name=x&password=longenough1", p, 1004, true, &mut |_, _| panic!("no second administrator"));
        assert!(r.status == 403 && !done);
        let (r, _) = handle(&mut s, "GET", "/", "", ip("8.8.8.8"), 1001, false, &mut |_, _| Ok(()));
        assert_eq!(r.status, 403);
    }
}
