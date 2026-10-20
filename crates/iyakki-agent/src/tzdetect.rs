//! Detect the time zone automatically (Settings → Time, "Detect automatically"): opt-in, because it asks an internet service
//! which zone the machine's **public IP address** is in. Off by default; while it is on, the zone is looked up when it is switched
//! on, after sign-in and then every [`RECHECK_SECS`], and only a real zone name this machine knows is ever applied. Choosing a zone
//! by hand switches it off.
//!
//! The services are asked in order ([`SOURCES`]); each answer is checked ([`valid_zone_name`], then the zone must load from the
//! zone files or the built-in table). The HTTP call is a parameter ([`detect`] takes a fetch function), so the tests never use the
//! network; the real one goes through the services' own HTTPS client ([`fetch_with`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_proxy::Upstream;

/// How often an automatic zone is looked up again.
pub const RECHECK_SECS: i64 = 6 * 3600;
/// After a failure (no network yet) it is tried again sooner.
pub const RETRY_SECS: i64 = 10 * 60;

/// How a service answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The body is the zone name.
    Plain,
    /// The body is JSON with the zone in this field.
    Json(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: &'static str,
    pub url: &'static str,
    pub answer: Answer,
}

/// Asked in this order. Both see the public IP address of this network (that is how they know); nothing else is sent.
pub const SOURCES: &[Source] = &[
    Source { name: "ipapi.co", url: "https://ipapi.co/timezone/", answer: Answer::Plain },
    Source { name: "worldtimeapi.org", url: "https://worldtimeapi.org/api/ip", answer: Answer::Json("timezone") },
];

/// What Settings says about it (the label is explicit about what is sent).
pub const EXPLAIN: &str = "Detect automatically asks ipapi.co (then worldtimeapi.org) which time zone this network's public IP address is in. Your IP address is visible to that service; nothing else is sent. Off unless you switch it on.";

/// A zone name as the zone database writes them: `Area/Place[/Sub]` or `UTC`, letters, digits, `_`, `-`, `+`, no `..`.
pub fn valid_zone_name(s: &str) -> bool {
    let s = s.trim();
    if s == "UTC" {
        return true;
    }
    !s.is_empty() && s.len() <= 64 && s.contains('/') && !s.contains("..") && !s.starts_with('/') && !s.ends_with('/') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/'))
}

/// The zone in a service's answer, if it gave a well-formed one.
pub fn parse_answer(a: Answer, body: &str) -> Option<String> {
    let z = match a {
        Answer::Plain => body.trim().to_string(),
        Answer::Json(field) => serde_json::from_str::<serde_json::Value>(body).ok()?.get(field)?.as_str()?.trim().to_string(),
    };
    valid_zone_name(&z).then_some(z)
}

/// Ask the services in order; the first one that names a zone this machine knows (`known`) wins: `(zone, service)`.
pub fn detect(fetch: &dyn Fn(&str) -> Result<String, String>, known: &dyn Fn(&str) -> bool) -> Result<(String, &'static str), String> {
    let mut why = Vec::new();
    for s in SOURCES {
        match fetch(s.url) {
            Ok(body) => match parse_answer(s.answer, &body) {
                Some(z) if known(&z) => return Ok((z, s.name)),
                Some(z) => why.push(format!("{}: {z} is not a zone this machine knows", s.name)),
                None => why.push(format!("{}: no time zone in the answer", s.name)),
            },
            Err(e) => why.push(format!("{}: {e}", s.name)),
        }
    }
    Err(format!("Could not detect the time zone ({})", why.join("; ")))
}

/// The real fetch: a GET through the given HTTPS client, only a 200 with a short body counts.
pub fn fetch_with(up: Arc<dyn Upstream>) -> impl Fn(&str) -> Result<String, String> {
    move |url: &str| {
        let r = up.request("GET", url, &[("Accept".into(), "application/json, text/plain".into()), ("User-Agent".into(), format!("IYAKKI/{}", crate::relinfo::VERSION))], &[])?;
        if r.status != 200 {
            return Err(format!("HTTP {}", r.status));
        }
        if r.body.len() > 16 * 1024 {
            return Err("the answer is too long".into());
        }
        Ok(String::from_utf8_lossy(&r.body).into_owned())
    }
}

/// A zone this machine can show (zone files, else the built-in table).
pub fn known_zone(name: &str) -> bool {
    crate::clock::Zone::named(name).is_some()
}

/// Is a lookup due (automatic on, and long enough since the last one; sooner after a failure)?
pub fn due(t: &crate::settings::TimeSettings, now: i64) -> bool {
    if !t.zone_auto {
        return false;
    }
    let failed = t.zone_note.starts_with("Could not");
    now - t.zone_checked_at >= if failed { RETRY_SECS } else { RECHECK_SECS }
}

/// Put a lookup's result into the settings: the zone when it worked (and still automatic), the note either way. Returns the new
/// zone when it changed.
pub fn apply(t: &mut crate::settings::TimeSettings, result: &Result<(String, &'static str), String>, now: i64) -> Option<String> {
    t.zone_checked_at = now;
    match result {
        Ok((z, src)) => {
            t.zone_note = format!("{src}: {z}");
            if t.zone_auto && t.zone != *z {
                t.zone = z.clone();
                return Some(z.clone());
            }
            None
        }
        Err(e) => {
            t.zone_note = e.clone();
            None
        }
    }
}

static BUSY: AtomicBool = AtomicBool::new(false);

/// Look the zone up on a thread of its own when it is due (or now, with `force`), and save it. `on_change` is told the new zone
/// (the shell applies it to the process-wide clock). Does nothing while a lookup is running or when automatic is off.
pub fn refresh_in_background(svc: &Arc<crate::services::Services>, force: bool, on_change: Arc<dyn Fn(&str) + Send + Sync>) -> bool {
    let t = match svc.settings.lock() {
        Ok(s) => s.get().time.clone(),
        Err(_) => return false,
    };
    let now = crate::clock::now_unix();
    if !t.zone_auto || !(force || due(&t, now)) {
        return false;
    }
    if BUSY.swap(true, Ordering::SeqCst) {
        return false;
    }
    let svc = svc.clone();
    let spawned = std::thread::Builder::new().name("iyakki-tzdetect".into()).spawn(move || {
        let fetch = fetch_with(svc.quick_upstream());
        let r = detect(&fetch, &known_zone);
        let mut changed = None;
        if let Ok(mut s) = svc.settings.lock() {
            let _ = s.update(|st| changed = apply(&mut st.time, &r, crate::clock::now_unix()));
        }
        if let Some(z) = changed {
            on_change(&z);
        }
        BUSY.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        BUSY.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_names_are_checked_before_anything_is_applied() {
        for ok in ["Asia/Kolkata", "America/Argentina/Buenos_Aires", "UTC", "Etc/GMT+5", "America/Port-au-Prince"] {
            assert!(valid_zone_name(ok), "{ok}");
        }
        for bad in ["", "Kolkata", "../etc/passwd", "/etc/localtime", "Asia/Kol kata", "Asia/<script>", "Europe/", &"A/".repeat(40)] {
            assert!(!valid_zone_name(bad), "{bad}");
        }
        assert_eq!(parse_answer(Answer::Plain, "Asia/Kolkata\n").as_deref(), Some("Asia/Kolkata"));
        assert_eq!(parse_answer(Answer::Plain, "<html>rate limited</html>"), None);
        assert_eq!(parse_answer(Answer::Json("timezone"), r#"{"timezone":"Europe/Paris","utc_offset":"+02:00"}"#).as_deref(), Some("Europe/Paris"));
        assert_eq!(parse_answer(Answer::Json("timezone"), r#"{"error":true}"#), None);
        assert_eq!(parse_answer(Answer::Json("timezone"), "not json"), None);
    }

    #[test]
    fn the_first_service_with_a_known_zone_wins_and_failures_fall_through_with_reasons() {
        let known = |z: &str| z != "Mars/Olympus_Mons";
        let asked = std::cell::RefCell::new(Vec::new());
        let fetch = |url: &str| -> Result<String, String> {
            asked.borrow_mut().push(url.to_string());
            if url.contains("ipapi") { Err("offline".into()) } else { Ok(r#"{"timezone":"Asia/Kolkata"}"#.into()) }
        };
        assert_eq!(detect(&fetch, &known).unwrap(), ("Asia/Kolkata".to_string(), "worldtimeapi.org"));
        assert_eq!(asked.borrow().len(), 2);
        let first = |_: &str| -> Result<String, String> { Ok("Europe/Berlin".into()) };
        assert_eq!(detect(&first, &known).unwrap().1, "ipapi.co");
        let unknown = |url: &str| -> Result<String, String> { if url.contains("ipapi") { Ok("Mars/Olympus_Mons".into()) } else { Err("HTTP 429".into()) } };
        let e = detect(&unknown, &known).unwrap_err();
        assert!(e.starts_with("Could not detect") && e.contains("not a zone this machine knows") && e.contains("HTTP 429"), "{e}");
    }

    #[test]
    fn a_result_is_applied_only_while_automatic_is_on_and_lookups_come_due_again() {
        let mut t = crate::settings::TimeSettings { zone_auto: true, zone: "UTC".into(), ..Default::default() };
        assert!(due(&t, 1_000_000), "never looked up");
        assert_eq!(apply(&mut t, &Ok(("Asia/Kolkata".into(), "ipapi.co")), 1_000_000).as_deref(), Some("Asia/Kolkata"));
        assert_eq!((t.zone.as_str(), t.zone_note.as_str(), t.zone_checked_at), ("Asia/Kolkata", "ipapi.co: Asia/Kolkata", 1_000_000));
        assert!(!due(&t, 1_000_000 + 60));
        assert!(due(&t, 1_000_000 + RECHECK_SECS));
        assert_eq!(apply(&mut t, &Ok(("Asia/Kolkata".into(), "ipapi.co")), 1_000_100), None, "the same zone is no change");
        // a failure keeps the zone, says why, and comes due sooner
        assert_eq!(apply(&mut t, &Err("Could not detect the time zone (offline)".into()), 2_000_000), None);
        assert_eq!(t.zone, "Asia/Kolkata");
        assert!(due(&t, 2_000_000 + RETRY_SECS) && !due(&t, 2_000_000 + 60));
        // off: never due, and a late answer does not change the hand-picked zone
        t.zone_auto = false;
        t.zone = "Europe/Paris".into();
        assert!(!due(&t, i64::MAX / 2));
        assert_eq!(apply(&mut t, &Ok(("Asia/Tokyo".into(), "ipapi.co")), 3_000_000), None);
        assert_eq!(t.zone, "Europe/Paris");
    }

    #[test]
    fn the_real_fetch_only_accepts_a_short_200() {
        struct Up(u16, usize);
        impl Upstream for Up {
            fn request(&self, m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                assert_eq!(m, "GET");
                Ok(iyakki_proxy::UpstreamResponse { status: self.0, body: vec![b'a'; self.1] })
            }
        }
        assert!(fetch_with(Arc::new(Up(200, 12)))("https://x").is_ok());
        assert_eq!(fetch_with(Arc::new(Up(429, 12)))("https://x").unwrap_err(), "HTTP 429");
        assert!(fetch_with(Arc::new(Up(200, 20_000)))("https://x").is_err());
    }

    #[test]
    fn nothing_happens_in_the_background_while_automatic_is_off() {
        let d = std::env::temp_dir().join(format!("iyakki-tzd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct Never;
        impl Upstream for Never {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                panic!("no network in tests");
            }
        }
        let up: Arc<dyn Upstream> = Arc::new(Never);
        let svc = crate::services::Services::open(&d, up.clone(), up).unwrap();
        assert!(!refresh_in_background(&svc, true, Arc::new(|_| {})), "off: no lookup even when forced");
        let _ = std::fs::remove_dir_all(&d);
    }
}
