//! Setting the clock from the network: a small SNTP client (RFC 4330), in-house, no dependency.
//!
//! A machine with no battery-backed clock (a Mac mini whose clock battery is flat, a board with no RTC) starts at 1970.
//! Once the network is up, [`spawn_sync_thread`] asks a time server (UDP port 123) now and every few hours, and sets
//! the clock with `clock_settime` (root only). The rules for changing the clock are in [`decide`]: a clock that is
//! obviously unset (before 2024) is set to the server's time; a small difference is stepped; a difference that would put
//! the clock BACK by more than [`MAX_BACK_STEP_SECS`] is not applied by itself: it is kept in [`status`] and the person
//! is asked (Settings -> Time -> "Set the clock to the network time").

use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

/// Seconds between 1900-01-01 (NTP) and 1970-01-01 (Unix).
const NTP_TO_UNIX: u64 = 2_208_988_800;
/// Servers tried in turn when the person has not named one.
pub const DEFAULT_SERVERS: [&str; 2] = ["time.cloudflare.com", "pool.ntp.org"];
/// Going back further than this needs the person's say.
pub const MAX_BACK_STEP_SECS: f64 = 5.0;
/// Differences smaller than this are left alone (the clock is right enough).
pub const IGNORE_BELOW_SECS: f64 = 0.5;
/// How often to ask again.
pub const RESYNC_EVERY: Duration = Duration::from_secs(4 * 3600);

/// A host name or address that is safe to look up (letters, digits, dots, dashes, colons for IPv6).
pub fn valid_server(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// The 48-byte client request: version 4, mode 3, with our transmit time so the reply can be matched to it.
pub fn request(now: f64) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0] = 0x23; // LI 0, VN 4, mode 3
    p[40..48].copy_from_slice(&to_ntp(now));
    p
}

fn to_ntp(unix: f64) -> [u8; 8] {
    let t = unix + NTP_TO_UNIX as f64;
    let secs = t.floor();
    let frac = ((t - secs) * 4_294_967_296.0) as u64 as u32;
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&(secs as u64 as u32).to_be_bytes());
    b[4..].copy_from_slice(&frac.to_be_bytes());
    b
}

fn from_ntp(b: &[u8]) -> f64 {
    let secs = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64;
    let frac = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as f64 / 4_294_967_296.0;
    // era 1 (after 2036): a time whose top bit is clear is in the next 136-year era
    let secs = if secs < NTP_TO_UNIX as f64 { secs + 4_294_967_296.0 } else { secs };
    secs + frac - NTP_TO_UNIX as f64
}

/// What a reply says: the offset of our clock from the server's (positive = our clock is behind), the round trip, and the
/// server's stratum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reply {
    pub offset: f64,
    pub delay: f64,
    pub stratum: u8,
}

/// Read a server reply. `t1` is when we sent the request and `t4` when the reply arrived (both our clock, in unix
/// seconds). Refuses short packets, replies that are not from a server, an unsynchronised server (stratum 0, leap
/// indicator 3), and replies that do not echo our transmit time (somebody else's packet).
pub fn parse_reply(p: &[u8], t1: f64, t4: f64) -> Result<Reply, String> {
    if p.len() < 48 {
        return Err(format!("the reply is {} bytes, not 48", p.len()));
    }
    let (li, mode, stratum) = (p[0] >> 6, p[0] & 7, p[1]);
    if mode != 4 && mode != 5 {
        return Err(format!("not a server reply (mode {mode})"));
    }
    if stratum == 0 {
        return Err("the server refused (kiss-of-death)".into());
    }
    if li == 3 || stratum > 15 {
        return Err("the server's own clock is not set".into());
    }
    if p[24..32] != to_ntp(t1) {
        return Err("the reply does not answer our request".into());
    }
    let (t2, t3) = (from_ntp(&p[32..40]), from_ntp(&p[40..48]));
    if t3 == 0.0 || p[40..48].iter().all(|b| *b == 0) {
        return Err("the reply has no time in it".into());
    }
    Ok(Reply { offset: ((t2 - t1) + (t3 - t4)) / 2.0, delay: (t4 - t1) - (t3 - t2), stratum })
}

/// What to do with a measured offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Close enough.
    Leave,
    /// Set the clock now.
    Step,
    /// The clock would go back by more than the allowed amount: ask the person.
    Ask,
}

/// The rules for changing the clock. `now` is our clock (unix seconds) and `offset` the server's time minus ours.
pub fn decide(now: f64, offset: f64) -> Action {
    if crate::clock::looks_wrong(now as i64) {
        return Action::Step; // an unset clock: any correction is welcome
    }
    if offset.abs() < IGNORE_BELOW_SECS {
        Action::Leave
    } else if offset < -MAX_BACK_STEP_SECS {
        Action::Ask
    } else {
        Action::Step
    }
}

fn now_f64() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Ask `server` ("host" or "host:port") once.
pub fn query(server: &str, timeout: Duration) -> Result<Reply, String> {
    let addr_text = if server.contains(':') && !server.starts_with('[') && server.matches(':').count() == 1 { server.to_string() } else if server.matches(':').count() > 1 { format!("[{server}]:123") } else { format!("{server}:123") };
    let addrs: Vec<_> = addr_text.to_socket_addrs().map_err(|e| format!("{server}: {e}"))?.collect();
    let mut last = format!("{server}: no address");
    for addr in addrs.into_iter().take(4) {
        let sock = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let t1 = now_f64();
        if let Err(e) = sock.send_to(&request(t1), addr) {
            last = format!("{server}: {e}");
            continue;
        }
        let mut buf = [0u8; 128];
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == addr.ip() => match parse_reply(&buf[..n], t1, now_f64()) {
                Ok(r) => return Ok(r),
                Err(e) => last = format!("{server}: {e}"),
            },
            Ok(_) => last = format!("{server}: reply from somebody else"),
            Err(e) => last = format!("{server}: {e}"),
        }
    }
    Err(last)
}

/// Set the system clock to `unix` seconds (root only).
#[cfg(unix)]
pub fn set_clock(unix: f64) -> Result<(), String> {
    let ts = libc::timespec { tv_sec: unix.floor() as libc::time_t, tv_nsec: ((unix - unix.floor()) * 1e9) as _ };
    // SAFETY: `ts` is a valid timespec that lives for the call.
    let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) };
    if rc == 0 { Ok(()) } else { Err(format!("could not set the clock: {}", std::io::Error::last_os_error())) }
}

#[cfg(not(unix))]
pub fn set_clock(_unix: f64) -> Result<(), String> {
    Err("not available on this OS".into())
}

/// What the last attempt did, for Settings -> Time and the clock's tooltip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Unix time of the last answer from a server (0 = never).
    pub last_sync: i64,
    pub server: String,
    /// Our clock's error at that time, seconds (positive = we were behind).
    pub offset: f64,
    /// A plain sentence for the person.
    pub message: String,
    /// The clock is this far ahead of the network time and was left alone until the person agrees (seconds, positive).
    pub pending_back: Option<f64>,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);

pub fn status() -> Status {
    STATUS.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = STATUS.lock() {
        f(s.get_or_insert_with(Status::default));
    }
}

/// One round: ask the servers in turn, decide, act. `apply` is what sets the clock (the real one, or a test's).
/// `force_back` lets a backward step through (the person said yes).
pub fn sync_once(servers: &[String], force_back: bool, timeout: Duration, apply: &dyn Fn(f64) -> Result<(), String>) -> Status {
    let mut errors = Vec::new();
    for srv in servers {
        match query(srv, timeout) {
            Ok(r) => {
                let now = now_f64();
                let act = decide(now, r.offset);
                let act = if act == Action::Ask && force_back { Action::Step } else { act };
                let msg = match act {
                    Action::Leave => format!("The clock is right (within {:.1} s of {srv}).", r.offset.abs().max(0.1)),
                    Action::Step => match apply(now + r.offset) {
                        Ok(()) => format!("Clock set from {srv} (it was {:.1} s {}).", r.offset.abs(), if r.offset >= 0.0 { "behind" } else { "ahead" }),
                        Err(e) => format!("The network time is known ({srv}) but the clock could not be set: {e}"),
                    },
                    Action::Ask => format!("The clock is {:.0} s ahead of {srv}. It was not changed; choose \"Set the clock to the network time\" to go back.", -r.offset),
                };
                let applied = act == Action::Step && msg.starts_with("Clock set");
                set_status(|s| {
                    s.last_sync = now_f64() as i64;
                    s.server = srv.clone();
                    s.offset = if applied { 0.0 } else { r.offset };
                    s.message = msg;
                    s.pending_back = (act == Action::Ask).then_some(-r.offset);
                });
                return status();
            }
            Err(e) => errors.push(e),
        }
    }
    set_status(|s| s.message = format!("Could not reach a time server: {}", errors.join("; ")));
    status()
}

/// The servers to try for these settings.
pub fn servers_for(custom: &str) -> Vec<String> {
    if valid_server(custom) { vec![custom.to_string()] } else { DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect() }
}

/// Keep the clock right in the background: once now (the network has just come up), then every few hours. `settings`
/// is asked each round, so turning it off in Settings takes effect. Never blocks the caller.
pub fn spawn_sync_thread(settings: impl Fn() -> (bool, String) + Send + 'static) {
    let _ = std::thread::Builder::new().name("iyakki-ntp".into()).spawn(move || {
        let mut first = true;
        loop {
            let (enabled, custom) = settings();
            if enabled {
                let st = sync_once(&servers_for(&custom), false, Duration::from_secs(4), &set_clock);
                log::info!("clock: {}", st.message);
                // no answer yet (the network may still be coming up): try again soon rather than in four hours
                if st.last_sync == 0 || st.message.starts_with("Could not") {
                    std::thread::sleep(Duration::from_secs(if first { 20 } else { 120 }));
                    first = false;
                    continue;
                }
            }
            std::thread::sleep(RESYNC_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake server on loopback answering with a clock `skew` seconds ahead of ours; `mutate` can spoil the reply.
    fn fake_server(skew: f64, mutate: impl Fn(&mut [u8; 48]) + Send + 'static) -> String {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                if n < 48 {
                    continue;
                }
                let mut r = [0u8; 48];
                r[0] = 0x24; // LI 0, VN 4, mode 4
                r[1] = 2;
                r[24..32].copy_from_slice(&buf[40..48]); // echo the client's transmit time
                let t = to_ntp(now_f64() + skew);
                r[32..40].copy_from_slice(&t);
                r[40..48].copy_from_slice(&t);
                mutate(&mut r);
                let _ = sock.send_to(&r, from);
            }
        });
        format!("127.0.0.1:{}", addr.port())
    }

    #[test]
    fn the_request_is_a_version_4_client_packet_and_times_round_trip() {
        let p = request(1_790_000_000.25);
        assert_eq!((p[0] >> 3 & 7, p[0] & 7, p.len()), (4, 3, 48));
        assert!((from_ntp(&p[40..48]) - 1_790_000_000.25).abs() < 1e-6);
        assert!((from_ntp(&to_ntp(0.0)) - 0.0).abs() < 1e-6, "1970 is representable");
    }

    #[test]
    fn a_reply_gives_the_offset_and_bad_replies_are_refused() {
        let server = fake_server(10.0, |_| {});
        let r = query(&server, Duration::from_secs(2)).unwrap();
        assert!((r.offset - 10.0).abs() < 0.2, "{r:?}");
        assert_eq!(r.stratum, 2);
        // stratum 0 (kiss of death), unsynchronised server, wrong echo, short packet, wrong mode, empty time
        for (what, m) in [("refused", 0u8), ("clock is not set", 1), ("does not answer", 2), ("not a server", 3), ("no time", 4)] {
            let s = fake_server(0.0, move |r| match m {
                0 => r[1] = 0,
                1 => r[0] |= 0xC0,
                2 => r[24] ^= 0xFF,
                3 => r[0] = 0x23,
                _ => r[40..48].fill(0),
            });
            let e = query(&s, Duration::from_millis(500)).unwrap_err();
            assert!(e.contains(what), "{what}: {e}");
        }
        assert!(parse_reply(&[0u8; 20], 0.0, 0.0).unwrap_err().contains("20 bytes"));
        // a server that never answers
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(query(&format!("127.0.0.1:{}", silent.local_addr().unwrap().port()), Duration::from_millis(200)).is_err());
    }

    #[test]
    fn the_clock_is_only_stepped_within_the_rules() {
        let now = 1_790_000_000.0;
        assert_eq!(decide(now, 0.1), Action::Leave);
        assert_eq!(decide(now, -0.4), Action::Leave);
        assert_eq!(decide(now, 3.0), Action::Step);
        assert_eq!(decide(now, 4000.0), Action::Step, "forward is always allowed");
        assert_eq!(decide(now, -3.0), Action::Step, "a little back is fine");
        assert_eq!(decide(now, -30.0), Action::Ask, "a real step back asks first");
        assert_eq!(decide(0.0, 1_790_000_000.0), Action::Step, "a clock at 1970 is set");
        assert_eq!(decide(1_000_000.0, -10.0), Action::Step, "an unset clock is never asked about");
    }

    #[test]
    fn a_round_applies_leaves_or_waits_for_the_person() {
        use std::cell::RefCell;
        let set = RefCell::new(Vec::new());
        let apply = |t: f64| {
            set.borrow_mut().push(t);
            Ok(())
        };
        // 20 s behind: stepped forward
        let st = sync_once(&[fake_server(20.0, |_| {})], false, Duration::from_secs(2), &apply);
        assert!(st.message.starts_with("Clock set") && st.pending_back.is_none(), "{st:?}");
        assert_eq!(set.borrow().len(), 1);
        // 100 s ahead: nothing is set, the person is asked
        let st = sync_once(&[fake_server(-100.0, |_| {})], false, Duration::from_secs(2), &apply);
        assert!(st.pending_back.is_some_and(|b| (b - 100.0).abs() < 1.0), "{st:?}");
        assert_eq!(set.borrow().len(), 1, "not applied");
        // ... and they said yes
        let st = sync_once(&[fake_server(-100.0, |_| {})], true, Duration::from_secs(2), &apply);
        assert!(st.message.starts_with("Clock set") && st.pending_back.is_none());
        assert_eq!(set.borrow().len(), 2);
        // right already
        let st = sync_once(&[fake_server(0.05, |_| {})], false, Duration::from_secs(2), &apply);
        assert!(st.message.contains("is right"), "{st:?}");
        assert_eq!(set.borrow().len(), 2);
        // no server answers; the second one in the list does
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead = format!("127.0.0.1:{}", silent.local_addr().unwrap().port());
        let st = sync_once(&[dead.clone(), fake_server(5.0, |_| {})], false, Duration::from_millis(300), &apply);
        assert!(st.message.starts_with("Clock set"));
        let st = sync_once(&[dead], false, Duration::from_millis(200), &apply);
        assert!(st.message.starts_with("Could not reach"), "{st:?}");
    }

    #[test]
    fn server_names_are_checked() {
        assert!(valid_server("time.cloudflare.com") && valid_server("192.168.1.1") && valid_server("::1"));
        assert!(!valid_server("") && !valid_server("a b") && !valid_server("x;rm") && !valid_server("a/b"));
        assert_eq!(servers_for("").len(), 2);
        assert_eq!(servers_for("ntp.example.org"), vec!["ntp.example.org".to_string()]);
    }
}
