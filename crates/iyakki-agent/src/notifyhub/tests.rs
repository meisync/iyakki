//! Tests of the notification hub on fakes: a fake clock (every function takes `now`), a fake network, a scripted SMTP server, loopback
//! only. Nothing here sends a real message or opens a real connection.

use super::channel::{Channel, Kind as ChKind};
use super::escalation::{Esc, Step, Target};
use super::events::{self, Bus};
use super::queue::{self, Delivery, Outcome};
use super::reminders::{self, Stage};
use super::rules::{Quiet, Route, Rules};
use super::service::{Hub, HubEnv};
use super::sink::{Notification, Recipients, RoleSel, Severity};
use super::smtp::{self, Auth, Mail, MailStream, TlsMode};
use super::transport::{MailJob, Net};
use super::webhook::HttpReq;
use crate::cloudroles::{NsRole, Roles, TenantRole};
use crate::inbox::Link;
use crate::tenantconfig::approvals::Pending;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

const T0: u64 = 1_700_000_000; // 2023-11-14 22:13:20 UTC

// ---------------------------------------------------------------------------------------------------------------- fakes

fn roles() -> Roles {
    let mut r = Roles::new("root").unwrap();
    for (n, role) in [("adm", TenantRole::Admin), ("ann", TenantRole::Member), ("bob", TenantRole::Member), ("cat", TenantRole::Member)] {
        r.add_tenant_member("root", n, role).unwrap();
    }
    r.create_namespace("root", "ops").unwrap();
    r.add_namespace_member("root", "ops", "ann", NsRole::Admin).unwrap();
    r.add_namespace_member("root", "ops", "bob", NsRole::Editor).unwrap();
    r
}

#[derive(Default)]
struct FakeEnv {
    channels: Mutex<Vec<Channel>>,
    rules: Mutex<Rules>,
    locked: Mutex<bool>,
    approvals: Mutex<Vec<Pending>>,
}

struct Env(Arc<FakeEnv>);

impl HubEnv for Env {
    fn roles(&self) -> Option<Roles> {
        Some(roles())
    }
    fn channels(&self) -> Vec<Channel> {
        self.0.channels.lock().unwrap().clone()
    }
    fn rules(&self, _: Option<&str>) -> Rules {
        self.0.rules.lock().unwrap().clone()
    }
    fn secret(&self, r: &str) -> Option<String> {
        if *self.0.locked.lock().unwrap() { None } else { r.starts_with("vault:notify.channel/").then(|| "whsec".to_string()) }
    }
    fn approvals(&self) -> Vec<Pending> {
        self.0.approvals.lock().unwrap().clone()
    }
    fn tenant_name(&self) -> String {
        "Acme".into()
    }
}

#[derive(Default)]
struct FakeNet {
    posts: Mutex<Vec<HttpReq>>,
    mails: Mutex<Vec<MailJob>>,
    fail: Mutex<bool>,
}

impl Net for FakeNet {
    fn post(&self, _: Option<&str>, r: &HttpReq) -> Result<u16, String> {
        if *self.fail.lock().unwrap() {
            return Err("connection refused".into());
        }
        self.posts.lock().unwrap().push(r.clone());
        Ok(200)
    }
    fn send_mail(&self, _: Option<&str>, j: &MailJob) -> Result<(), String> {
        if *self.fail.lock().unwrap() {
            return Err("connection refused".into());
        }
        self.mails.lock().unwrap().push(j.clone());
        Ok(())
    }
}

fn webhook(details: bool) -> Channel {
    Channel { key: "/hook".into(), team: None, kind: ChKind::Webhook, min_severity: Severity::Normal, include_details: details, max_per_hour: 60, link_base: "https://iyakki.example.org".into(), url: "https://hooks.example.org/in".into(), signing_secret: "vault:notify.channel/hook".into(), host: String::new(), port: 587, tls: TlsMode::Starttls, from: String::new(), username: String::new(), password: String::new(), addresses: BTreeMap::new() }
}

fn email(details: bool) -> Channel {
    let mut c = webhook(details);
    c.key = "/mail".into();
    c.kind = ChKind::Email;
    c.url = String::new();
    c.signing_secret = String::new();
    c.host = "smtp.example.org".into();
    c.from = "alerts@example.org".into();
    c.password = "vault:notify.channel/mailpw".into();
    c.username = "alerts".into();
    c.addresses = [("ann", "ann@example.org"), ("bob", "bob@example.org")].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    c
}

struct Rig {
    env: Arc<FakeEnv>,
    net: Arc<FakeNet>,
    hub: Hub,
    dir: std::path::PathBuf,
}

fn rig(name: &str, channels: Vec<Channel>) -> Rig {
    let dir = crate::testdir::unique("notifyhub", name);
    std::fs::create_dir_all(&dir).unwrap();
    let env = Arc::new(FakeEnv::default());
    *env.channels.lock().unwrap() = channels;
    let hub = Hub::new(&dir, "acme", Box::new(Env(env.clone())), "replica-a");
    Rig { env, net: Arc::new(FakeNet::default()), hub, dir }
}

impl Rig {
    fn another_replica(&self, holder: &str) -> Hub {
        Hub::new(&self.dir, "acme", Box::new(Env(self.env.clone())), holder)
    }
    fn tick(&self, now: u64) -> super::service::TickReport {
        self.hub.tick(now, self.net.as_ref())
    }
}

fn note(sev: Severity, title: &str, body: &str, to: Recipients) -> Notification {
    Notification { tenant: "acme".into(), team: Some("ops".into()), recipients: to, severity: sev, title: title.into(), body: body.into(), link: Some(Link::new("runs", "r1")), dedupe_key: String::new(), source: "flow:nightly".into() }
}

fn people(p: &[&str]) -> Recipients {
    Recipients::People(p.iter().map(|x| x.to_string()).collect())
}

// ---------------------------------------------------------------------------------------------------------------- pure parts

#[test]
fn recipients_are_expanded_against_the_tenant_roles_and_outsiders_get_nothing() {
    let r = roles();
    let ex = |t: Option<&str>, w: Recipients| super::expand::expand(Some(&r), t, &w);
    assert_eq!(ex(Some("ops"), people(&["ann", "zed", "bob", "ann"])), ["ann", "bob"], "a name outside the tenant is dropped, repeats collapse");
    assert_eq!(ex(Some("ops"), Recipients::Roles(vec![RoleSel::TeamAdmin])), ["ann", "root"], "root created the team and administers it");
    assert_eq!(ex(Some("ops"), Recipients::Roles(vec![RoleSel::TenantAdmin])), ["adm", "root"]);
    assert_eq!(ex(Some("ops"), Recipients::Team), ["ann", "bob", "root"], "cat is not in the team");
    assert_eq!(ex(None, Recipients::Team).len(), 5, "no team: the whole tenant");
    let mut r2 = roles();
    r2.change_approvers("ann", "ops", "prod", &["bob".to_string()], &[], true).unwrap();
    assert_eq!(super::expand::expand(Some(&r2), Some("ops"), &Recipients::Roles(vec![RoleSel::Approver("prod".into())])), ["bob"]);
    assert!(ex(Some("other"), Recipients::Roles(vec![RoleSel::TeamAdmin])).is_empty(), "a team that is not there has no admins");
}

#[test]
fn severity_thresholds_quiet_hours_and_critical_bypass_decide_the_route() {
    let mut rules = Rules { quiet: Some(Quiet { start_min: 22 * 60, end_min: 7 * 60 }), ..Rules::default() };
    assert!(rules.in_quiet(T0), "22:13 is inside 22:00-07:00 (the window wraps midnight)");
    assert_eq!(rules.quiet_end(T0), T0 + 31_600, "held until 07:00");
    assert_eq!(rules.route(Severity::Normal, Severity::Normal, T0), Route::Send(T0 + 31_600));
    assert_eq!(rules.route(Severity::Critical, Severity::Normal, T0), Route::Send(T0), "critical goes through quiet hours");
    assert_eq!(rules.route(Severity::Low, Severity::Normal, T0), Route::Skip, "below the channel's threshold");
    assert_eq!(rules.route(Severity::High, Severity::High, T0 + 40_000), Route::Send(T0 + 40_000), "outside the window: now");
    rules.offset_min = 120; // the tenant's local time is 00:13: still quiet, ends in 6h47
    assert!(rules.in_quiet(T0));
    rules.offset_min = 8 * 60; // 06:13 local on the next day: quiet, ends in 47 minutes
    assert_eq!(rules.quiet_end(T0), T0 + 46 * 60 + 40);
    rules.quiet = Some(Quiet { start_min: 9 * 60, end_min: 9 * 60 });
    assert!(!rules.in_quiet(T0), "equal start and end: no quiet hours");
}

#[test]
fn the_digest_takes_low_severity_and_is_due_once_a_day() {
    let rules = Rules { digest_min: Some(8 * 60), ..Rules::default() };
    assert_eq!(rules.route(Severity::Low, Severity::Low, T0), Route::Digest);
    assert_eq!(rules.route(Severity::Low, Severity::Normal, T0), Route::Skip, "a channel that does not take low gets no digest");
    let day = T0 / 86_400;
    assert_eq!(rules.digest_due(T0, 0), Some(day as i64), "22:13 is after 08:00: due");
    let early = day * 86_400 + 7 * 3600;
    assert_eq!(rules.digest_due(early, day as i64 - 1), None, "before 08:00");
    assert_eq!(rules.digest_due(T0, day as i64 - 1), Some(day as i64));
    assert_eq!(rules.digest_due(T0 + 60, day as i64), None, "already sent today");
}

fn esc(after: u64) -> Esc {
    Esc { id: "n1".into(), team: Some("ops".into()), after, level: 0, last: T0, acked: false, done: false, severity: Severity::High, title: "t".into(), body: String::new(), link: None, source: "x".into(), inbox: BTreeMap::new() }
}

#[test]
fn the_escalation_chain_walks_owner_team_admin_tenant_admin_on_a_fake_clock() {
    let mut e = esc(600);
    assert_eq!(e.step(T0 + 599), Step::Idle);
    let s = e.step(T0 + 600);
    assert_eq!(s, Step::Escalate(Target::TeamAdmins));
    e.apply(s, T0 + 600);
    assert_eq!(e.step(T0 + 1199), Step::Idle, "the next wait starts at the last step");
    let s = e.step(T0 + 1200);
    assert_eq!(s, Step::Escalate(Target::TenantAdmins));
    e.apply(s, T0 + 1200);
    assert_eq!(e.step(T0 + 1800), Step::Exhausted);
    e.apply(Step::Exhausted, T0 + 1800);
    assert_eq!(e.step(T0 + 99_999), Step::Idle, "finished");
    let mut a = esc(600);
    a.ack();
    assert_eq!(a.step(T0 + 99_999), Step::Idle, "an acknowledgement stops the chain");
}

#[test]
fn reminders_come_at_half_and_ninety_percent_and_expiry_once_each() {
    let (c, x) = (T0, T0 + 1000);
    assert_eq!(reminders::due(c, x, c, 0), [Stage::Announce]);
    assert_eq!(reminders::due(c, x, c + 499, reminders::ANNOUNCED), []);
    assert_eq!(reminders::due(c, x, c + 500, reminders::ANNOUNCED), [Stage::Half]);
    assert_eq!(reminders::due(c, x, c + 900, reminders::ANNOUNCED | reminders::HALF), [Stage::Late]);
    assert_eq!(reminders::due(c, x, c + 950, reminders::ANNOUNCED | reminders::HALF | reminders::LATE), []);
    assert_eq!(reminders::due(c, x, x, reminders::ANNOUNCED), [Stage::Expired], "past the end only the expiry is reported");
    assert_eq!(reminders::due(c, x, x + 5, reminders::EXPIRED), []);
}

fn delivery(id: &str) -> Delivery {
    Delivery { id: id.into(), channel: "/hook".into(), person: "*".into(), subject: "s".into(), body: "b".into(), severity: Severity::Normal, attempts: 0, next_at: T0, created: T0, last_error: String::new() }
}

#[test]
fn a_failing_delivery_backs_off_then_becomes_a_dead_letter() {
    assert_eq!([1, 2, 3, 4, 5, 6, 12].map(queue::backoff), [30, 60, 120, 240, 480, 960, 3600]);
    let (mut q, mut dead) = (vec![delivery("d1")], vec![]);
    let mut now = T0;
    for attempt in 1..queue::MAX_ATTEMPTS {
        assert_eq!(queue::due(&q, now), ["d1"]);
        assert_eq!(queue::failed(&mut q, &mut dead, "d1", now, "boom"), Some(Outcome::Retry(now + queue::backoff(attempt))));
        assert!(queue::due(&q, now).is_empty(), "waiting for the backoff");
        now += queue::backoff(attempt);
    }
    assert_eq!(queue::failed(&mut q, &mut dead, "d1", now, "boom"), Some(Outcome::Dead));
    assert!(q.is_empty() && dead.len() == 1 && dead[0].last_error == "boom");
    let mut full: Vec<Delivery> = (0..queue::MAX_QUEUE).map(|i| delivery(&format!("x{i}"))).collect();
    assert!(queue::push(&mut full, delivery("one-more")).is_err(), "the queue is bounded");
    let mut big = delivery("big");
    big.body = "é".repeat(queue::MAX_BODY);
    let mut q2 = vec![];
    queue::push(&mut q2, big).unwrap();
    assert!(q2[0].body.len() <= queue::MAX_BODY, "bodies are cut at a character boundary");
}

#[test]
fn secrets_and_vault_references_never_stay_in_a_body() {
    let t = "deploy done with key sk-abcdefghijklmnopqrstuvwxyz and password=hunter2xyz, see vault:team/ops/db-pass and https://bob:s3cr3tpw@git.example.org/x\u{7}";
    let r = super::redact::redact(t, 2000, true);
    for bad in ["sk-abcdefghij", "hunter2xyz", "vault:team", "s3cr3tpw", "\u{7}"] {
        assert!(!r.contains(bad), "{bad} survived in: {r}");
    }
    assert!(r.contains("deploy done") && r.contains("git.example.org"), "ordinary words stay: {r}");
    assert_eq!(super::redact::redact(&"word ".repeat(200), 100, false).chars().count(), 100);
}

// ---------------------------------------------------------------------------------------------------------------- webhook

#[test]
fn the_webhook_signature_matches_the_known_vectors_and_the_flows_gate() {
    // RFC 4231, test case 2: the HMAC-SHA256 itself
    let mac = crate::devmode::auth::hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(crate::devmode::auth::hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    // the signed message is "<timestamp>.<body>" (computed independently)
    let req = super::webhook::build("https://hooks.example.org/in", b"s3cret", 1_700_000_000, &serde_json::json!({"a": 1})).unwrap();
    assert_eq!(req.body, br#"{"a":1}"#);
    let h = |k: &str| req.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(h("X-Iyakki-Signature"), Some("sha256=1698a50bc74d1ff1db85c4e0a5297c2ad9fdba245d5737cdb789e4cc6e098940"));
    assert_eq!(h("X-Iyakki-Timestamp"), Some("1700000000"));
    assert_eq!(req.timeout_secs, 10);
    // the receiving gate of the flows accepts it, and refuses a stale or altered one
    let gate = crate::flows::webhook::WebhookGate::new();
    assert!(gate.check(b"s3cret", "p", 1_700_000_000, "1700000000", h("X-Iyakki-Signature").unwrap(), &req.body).is_ok());
    assert!(gate.check(b"s3cret", "p2", 1_700_000_000 + 4000, "1700000000", h("X-Iyakki-Signature").unwrap(), &req.body).is_err(), "stale");
    assert!(gate.check(b"s3cret", "p3", 1_700_000_000, "1700000000", h("X-Iyakki-Signature").unwrap(), b"{\"a\":2}").is_err(), "altered");
}

#[test]
fn a_webhook_address_must_be_https_without_credentials_and_the_payload_is_capped() {
    for bad in ["http://hooks.example.org/in", "https://u:p@hooks.example.org/in", "ftp://x", "https:///in", "https://a b/c", "hooks.example.org"] {
        assert!(super::webhook::https_host(bad).is_err(), "{bad}");
    }
    assert_eq!(super::webhook::https_host("https://Hooks.Example.org:8443/in?x=1").unwrap(), "hooks.example.org");
    let big = serde_json::json!({"x": "a".repeat(20_000)});
    assert!(super::webhook::build("https://h.example.org/", b"k", 1, &big).is_err());
    assert!(super::webhook::status_ok(204) && !super::webhook::status_ok(302) && !super::webhook::status_ok(500));
}

// ---------------------------------------------------------------------------------------------------------------- SMTP

struct FakeSmtp {
    tls: bool,
    offers_starttls: bool,
    offers_plain_auth: bool,
    out: VecDeque<String>,
    log: Vec<String>,
    buf: String,
    in_data: bool,
    data: String,
    bytes_written: usize,
    login_stage: u8,
}

impl FakeSmtp {
    fn new(tls: bool, offers_starttls: bool) -> FakeSmtp {
        FakeSmtp { tls, offers_starttls, offers_plain_auth: true, out: VecDeque::from(["220 mail.test ESMTP".to_string()]), log: vec![], buf: String::new(), in_data: false, data: String::new(), bytes_written: 0, login_stage: 0 }
    }
    fn say(&mut self, l: &str) {
        self.out.push_back(l.into());
    }
}

impl MailStream for FakeSmtp {
    fn read_line(&mut self) -> Result<String, String> {
        self.out.pop_front().ok_or_else(|| "eof".to_string())
    }
    fn write_all(&mut self, b: &[u8]) -> Result<(), String> {
        self.bytes_written += b.len();
        self.buf.push_str(&String::from_utf8_lossy(b));
        if self.in_data {
            if self.buf.ends_with("\r\n.\r\n") {
                self.data = std::mem::take(&mut self.buf);
                self.in_data = false;
                self.say("250 queued");
            }
            return Ok(());
        }
        while let Some(i) = self.buf.find("\r\n") {
            let line: String = self.buf.drain(..i + 2).collect();
            let line = line.trim_end().to_string();
            let upper = line.to_ascii_uppercase();
            self.log.push(if upper.starts_with("AUTH PLAIN") { "AUTH PLAIN <cred>".into() } else { line.clone() });
            if upper.starts_with("EHLO") {
                self.say("250-mail.test");
                if self.offers_starttls && !self.tls {
                    self.say("250-STARTTLS");
                }
                if self.tls {
                    self.say(if self.offers_plain_auth { "250 AUTH PLAIN LOGIN" } else { "250 AUTH LOGIN" });
                } else {
                    self.say("250 SIZE 1000000");
                }
            } else if upper == "STARTTLS" {
                self.say("220 ready");
            } else if upper.starts_with("AUTH PLAIN") {
                self.say("235 ok");
            } else if upper == "AUTH LOGIN" {
                self.login_stage = 1;
                self.say("334 VXNlcm5hbWU6");
            } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                self.say("250 ok");
            } else if upper == "DATA" {
                self.say("354 go");
                self.in_data = true;
            } else if upper == "QUIT" {
                self.say("221 bye");
            } else if self.login_stage == 1 {
                self.login_stage = 2;
                self.say("334 UGFzc3dvcmQ6");
            } else {
                self.login_stage = 0;
                self.say("235 ok"); // the password of AUTH LOGIN
            }
        }
        Ok(())
    }
    fn start_tls(&mut self) -> Result<(), String> {
        self.tls = true;
        Ok(())
    }
    fn is_tls(&self) -> bool {
        self.tls
    }
}

fn mail<'a>(to: &'a str, subject: &'a str, body: &'a str) -> Mail<'a> {
    Mail { from: "alerts@example.org", to, subject, body, ehlo_name: "iyakki.test", now: T0, id: "d1" }
}

#[test]
fn smtp_starttls_sends_the_commands_in_order_and_logs_in_only_after_tls() {
    let mut s = FakeSmtp::new(false, true);
    let auth = Auth { user: "alerts", pass: "pw" };
    smtp::send(&mut s, TlsMode::Starttls, Some(&auth), &mail("ann@example.org", "[IYAKKI] hi", "line one\nline two")).unwrap();
    let cmds: Vec<&str> = s.log.iter().map(String::as_str).collect();
    assert_eq!(cmds, ["EHLO iyakki.test", "STARTTLS", "EHLO iyakki.test", "AUTH PLAIN <cred>", "MAIL FROM:<alerts@example.org>", "RCPT TO:<ann@example.org>", "DATA", "QUIT"]);
    assert!(s.tls);
    for h in ["From: <alerts@example.org>", "To: <ann@example.org>", "Subject: [IYAKKI] hi", "Content-Type: text/plain; charset=utf-8", "Content-Transfer-Encoding: base64", "Date: Tue, 14 Nov 2023 22:13:20 +0000"] {
        assert!(s.data.contains(h), "{h} in {}", s.data);
    }
    assert!(s.data.ends_with("\r\n.\r\n") && !s.data.contains("line one"), "the text goes as base64");
}

#[test]
fn smtp_login_falls_back_to_login_and_implicit_tls_needs_an_encrypted_connection() {
    let mut s = FakeSmtp::new(true, false);
    s.offers_plain_auth = false;
    smtp::send(&mut s, TlsMode::Implicit, Some(&Auth { user: "u", pass: "p" }), &mail("ann@example.org", "s", "b")).unwrap();
    assert!(s.log.contains(&"AUTH LOGIN".to_string()) && !s.log.iter().any(|l| l.starts_with("STARTTLS")));
    let mut plain = FakeSmtp::new(false, false);
    let e = smtp::send(&mut plain, TlsMode::Implicit, None, &mail("ann@example.org", "s", "b")).unwrap_err();
    assert!(e.contains("not encrypted") && plain.bytes_written == 0, "nothing is sent over a plain connection: {e}");
}

#[test]
fn smtp_never_falls_back_to_plain_when_starttls_is_missing() {
    let mut s = FakeSmtp::new(false, false);
    let e = smtp::send(&mut s, TlsMode::Starttls, Some(&Auth { user: "u", pass: "p" }), &mail("ann@example.org", "s", "b")).unwrap_err();
    assert!(e.contains("STARTTLS") && e.contains("plain SMTP is never used"), "{e}");
    assert!(!s.log.iter().any(|l| l.starts_with("AUTH") || l.starts_with("MAIL") || l.starts_with("DATA")), "no login, no message: {:?}", s.log);
}

#[test]
fn smtp_refuses_header_injection_before_a_single_byte_is_written() {
    for (to, subject) in [("ann@example.org\r\nBcc: x@evil.test", "s"), ("ann@example.org", "hi\r\nBcc: x@evil.test"), ("ann@example.org", "hi\nX: y"), ("ann@example.org,bob@example.org", "s"), ("<ann@example.org>", "s"), ("ann@@example.org", "s"), ("ann@example.org", "nul\0byte")] {
        let mut s = FakeSmtp::new(true, true);
        let r = smtp::send(&mut s, TlsMode::Implicit, None, &mail(to, subject, "b"));
        assert!(r.is_err(), "{to:?} {subject:?}");
        assert_eq!(s.bytes_written, 0, "{to:?} {subject:?}");
        assert_eq!(s.out.len(), 1, "not even the greeting was read");
    }
    let mut s = FakeSmtp::new(true, true);
    let m = Mail { from: "a@example.org\r\nX: y", ..mail("ann@example.org", "s", "b") };
    assert!(smtp::send(&mut s, TlsMode::Implicit, None, &m).is_err() && s.bytes_written == 0, "the From address is checked too");
    assert!(smtp::valid_address("a.b+c@example.org") && !smtp::valid_address("a b@example.org") && !smtp::valid_address("a@localhost"));
    // a non-ASCII subject is encoded, never sent raw
    let msg = smtp::message(&mail("ann@example.org", "Grüße", "b")).unwrap();
    assert!(msg.contains("Subject: =?UTF-8?B?") && msg.is_ascii());
}

// ---------------------------------------------------------------------------------------------------------------- the hub

#[test]
fn a_notification_emitted_with_nobody_signed_in_is_delivered_to_a_channel_and_waits_in_the_inbox() {
    let r = rig("nobody", vec![webhook(false), email(false)]);
    let rec = r.hub.emit_at(&note(Severity::Normal, "Nightly backup finished", "all 12 volumes", people(&["ann", "bob", "zed"])), T0).unwrap();
    assert_eq!(rec.recipients, ["ann", "bob"]);
    assert_eq!(rec.queued, 3, "one webhook call and one email each for ann and bob");
    // it is on disk before any delivery: a second hub on the same volume (a restart) sees both inbox notes and the queue
    let again = r.another_replica("replica-b");
    assert_eq!(again.inbox.list("ann").len(), 1);
    assert_eq!(again.queue_len(), (3, 0));
    let rep = r.tick(T0 + 1);
    assert!(rep.leader);
    assert_eq!(rep.delivered, 3);
    assert_eq!(r.net.posts.lock().unwrap().len(), 1);
    assert_eq!(r.net.mails.lock().unwrap().len(), 2, "one message per recipient");
    // much later somebody signs in: the bell is rebuilt from the store
    let c = again.inbox.counts("ann", (T0 + 86_400) as i64);
    assert_eq!(c.unread, 1);
    assert_eq!(again.inbox.list("bob")[0].title, "Nightly backup finished");
    assert_eq!(r.hub.queue_len(), (0, 0));
}

#[test]
fn a_person_only_gets_their_own_and_another_tenant_is_refused() {
    let r = rig("scope", vec![]);
    r.hub.emit_at(&note(Severity::Normal, "for ann", "", people(&["ann"])), T0).unwrap();
    assert_eq!(r.hub.inbox.list("ann").len(), 1);
    assert!(r.hub.inbox.list("bob").is_empty() && r.hub.inbox.list("cat").is_empty());
    let mut other = note(Severity::Normal, "x", "", people(&["ann"]));
    other.tenant = "evil".into();
    assert!(r.hub.emit_at(&other, T0).unwrap_err().contains("another tenant"));
    assert_eq!(r.hub.inbox.list("ann").len(), 1);
    let mut bad_team = note(Severity::Normal, "x", "", Recipients::Team);
    bad_team.team = Some("../x".into());
    assert!(r.hub.emit_at(&bad_team, T0).is_err());
    assert!(r.hub.emit_at(&note(Severity::Normal, "   ", "", people(&["ann"])), T0).is_err(), "a title is needed");
}

#[test]
fn the_same_dedupe_key_within_an_hour_is_one_notification() {
    let r = rig("dedupe", vec![webhook(false)]);
    let mut n = note(Severity::Normal, "disk almost full", "", people(&["ann"]));
    n.dedupe_key = "disk:ops".into();
    assert!(!r.hub.emit_at(&n, T0).unwrap().deduped);
    assert!(r.hub.emit_at(&n, T0 + 100).unwrap().deduped);
    assert_eq!(r.hub.inbox.list("ann").len(), 1);
    assert_eq!(r.hub.queue_len().0, 1);
    assert!(!r.hub.emit_at(&n, T0 + 3601).unwrap().deduped, "after the window it is new again");
    // and the key is remembered across a restart
    let b = r.another_replica("replica-b");
    n.dedupe_key = "k2".into();
    b.emit_at(&n, T0 + 4000).unwrap();
    assert!(r.hub.emit_at(&n, T0 + 4001).unwrap().deduped);
}

#[test]
fn by_default_a_channel_carries_a_summary_and_a_link_never_the_text_or_an_action() {
    let r = rig("linkonly", vec![webhook(false), email(false)]);
    let mut n = note(Severity::High, "Payroll export failed for Acme GmbH", "customer 4711 token=abc123def456ghi789", people(&["ann"]));
    n.source = "approval".into();
    r.hub.emit_at(&n, T0).unwrap();
    r.tick(T0 + 1);
    let m = r.net.mails.lock().unwrap()[0].clone();
    let all = format!("{} {}", m.subject, m.body);
    for secret in ["Payroll", "4711", "abc123", "Acme GmbH"] {
        assert!(!all.contains(secret), "{secret} in {all}");
    }
    assert!(m.body.contains("An approval is waiting for you.") && m.body.contains("https://iyakki.example.org/ui/#/notifications"), "{}", m.body);
    for action in ["approve", "deny", "decide", "token", "?"] {
        assert!(!m.body.to_lowercase().contains(action), "an action link must not exist: {}", m.body);
    }
    let hook = String::from_utf8(r.net.posts.lock().unwrap()[0].body.clone()).unwrap();
    assert!(!hook.contains("Payroll") && !hook.contains("title") && hook.contains("\"link\""), "{hook}");
}

#[test]
fn details_leave_only_when_the_channel_says_so_and_the_admin_acknowledged() {
    use crate::tenantconfig::Item;
    let mk = |details: bool, ack: bool| Item {
        kind: "notify.channel".into(),
        name: "hook".into(),
        namespace: None,
        enabled: true,
        managed: false,
        values: [("type", serde_json::json!("webhook")), ("url", serde_json::json!("https://hooks.example.org/in")), ("signing_secret", serde_json::json!("vault:notify.channel/hook")), ("include_details", serde_json::json!(details)), ("details_ack", serde_json::json!(ack))].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    };
    assert!(!super::channel::from_item(&mk(false, false)).unwrap().include_details, "the default is off");
    assert!(!super::channel::from_item(&mk(true, false)).unwrap().include_details, "asked for, not acknowledged: still off");
    assert!(super::channel::from_item(&mk(true, true)).unwrap().include_details);
    let mut off = mk(false, false);
    off.enabled = false;
    assert!(super::channel::from_item(&off).is_err(), "a channel is off unless switched on");
    let mut http = mk(false, false);
    http.values.insert("url".into(), serde_json::json!("http://hooks.example.org/in"));
    assert!(super::channel::from_item(&http).is_err(), "no http");
    // and with details on, what goes out is already redacted
    let r = rig("details", vec![webhook(true)]);
    r.hub.emit_at(&note(Severity::High, "Key sk-abcdefghijklmnopqrstuvwxyz leaked", "see vault:team/ops/db and password=hunter2xyz", people(&["ann"])), T0).unwrap();
    r.tick(T0 + 1);
    let hook = String::from_utf8(r.net.posts.lock().unwrap()[0].body.clone()).unwrap();
    assert!(hook.contains("leaked") && !hook.contains("sk-abcdefgh") && !hook.contains("vault:team") && !hook.contains("hunter2xyz"), "{hook}");
}

#[test]
fn queued_deliveries_survive_a_restart_and_keep_retrying_until_the_receiver_answers() {
    let r = rig("restart", vec![webhook(false)]);
    *r.net.fail.lock().unwrap() = true;
    r.hub.emit_at(&note(Severity::High, "x", "", people(&["ann"])), T0).unwrap();
    let rep = r.tick(T0 + 1);
    assert_eq!((rep.delivered, rep.retried), (0, 1));
    // the pod restarts: a new hub, same volume
    let b = r.another_replica("replica-a");
    assert_eq!(b.queue_len(), (1, 0));
    assert_eq!(b.tick(T0 + 10, r.net.as_ref()).retried, 0, "still inside the backoff (30 s)");
    *r.net.fail.lock().unwrap() = false;
    let rep = b.tick(T0 + 40, r.net.as_ref());
    assert_eq!(rep.delivered, 1);
    assert_eq!(r.net.posts.lock().unwrap().len(), 1);
    assert_eq!(b.queue_len(), (0, 0));
}

#[test]
fn a_receiver_that_never_answers_ends_in_the_dead_letters() {
    let r = rig("dead", vec![webhook(false)]);
    *r.net.fail.lock().unwrap() = true;
    r.hub.emit_at(&note(Severity::High, "x", "", people(&["ann"])), T0).unwrap();
    let mut now = T0;
    let mut dead = 0;
    for _ in 0..12 {
        now += 4000;
        dead += r.tick(now).dead;
    }
    assert_eq!(dead, 1);
    assert_eq!(r.hub.queue_len(), (0, 1));
    assert_eq!(r.hub.inbox.list("ann").len(), 1, "the inbox note is untouched");
}

#[test]
fn a_locked_vault_holds_the_delivery_back_without_sending_unsigned() {
    let r = rig("locked", vec![webhook(false)]);
    *r.env.locked.lock().unwrap() = true;
    r.hub.emit_at(&note(Severity::High, "x", "", people(&["ann"])), T0).unwrap();
    assert_eq!(r.tick(T0 + 1).retried, 1);
    assert!(r.net.posts.lock().unwrap().is_empty());
    *r.env.locked.lock().unwrap() = false;
    assert_eq!(r.tick(T0 + 100).delivered, 1);
}

#[test]
fn only_the_replica_with_the_lease_delivers() {
    let r = rig("lease", vec![webhook(false)]);
    let b = r.another_replica("replica-b");
    r.hub.emit_at(&note(Severity::High, "x", "", people(&["ann"])), T0).unwrap();
    assert!(r.tick(T0 + 1).leader);
    let rep = b.tick(T0 + 2, r.net.as_ref());
    assert!(!rep.leader && rep.delivered == 0, "the second replica does nothing while the first holds the lease");
    assert_eq!(r.net.posts.lock().unwrap().len(), 1, "delivered once");
    // the first replica dies; after the lease runs out the second takes over
    b.emit_at(&note(Severity::High, "y", "", people(&["bob"])), T0 + 3).unwrap();
    assert!(!b.tick(T0 + 30, r.net.as_ref()).leader);
    let rep = b.tick(T0 + 1 + super::service::LEASE_TTL + 1, r.net.as_ref());
    assert!(rep.leader && rep.delivered == 1);
    assert_eq!(r.net.posts.lock().unwrap().len(), 2);
    // and the old holder is now locked out
    assert!(!r.tick(T0 + 1 + super::service::LEASE_TTL + 2).leader);
}

#[test]
fn quiet_hours_hold_a_message_until_morning_but_not_a_critical_one() {
    let r = rig("quiet", vec![webhook(false)]);
    *r.env.rules.lock().unwrap() = Rules { quiet: Some(Quiet { start_min: 22 * 60, end_min: 7 * 60 }), ..Rules::default() };
    r.hub.emit_at(&note(Severity::High, "later", "", people(&["ann"])), T0).unwrap();
    r.hub.emit_at(&note(Severity::Critical, "now", "", people(&["ann"])), T0).unwrap();
    assert_eq!(r.tick(T0 + 10).delivered, 1, "only the critical one");
    assert_eq!(r.hub.inbox.list("ann").len(), 2, "the inbox always gets both");
    assert_eq!(r.tick(T0 + 31_599).delivered, 0);
    assert_eq!(r.tick(T0 + 31_600).delivered, 1, "at 07:00");
}

#[test]
fn low_notifications_wait_for_the_daily_digest_which_goes_once() {
    let r = rig("digest", vec![{
        let mut c = webhook(false);
        c.min_severity = Severity::Low;
        c
    }]);
    *r.env.rules.lock().unwrap() = Rules { digest_min: Some(8 * 60), ..Rules::default() };
    let day = T0 / 86_400;
    let before = day * 86_400 + 3600; // 01:00, before the digest minute: not due
    for i in 0..3 {
        r.hub.emit_at(&note(Severity::Low, &format!("minor {i}"), "", people(&["ann"])), before).unwrap();
    }
    assert_eq!(r.hub.queue_len().0, 0, "nothing is sent for low severity");
    assert_eq!(r.tick(before + 10).digests, 0);
    let rep = r.tick(day * 86_400 + 8 * 3600 + 5);
    assert_eq!((rep.digests, rep.delivered), (1, 1));
    let body = String::from_utf8(r.net.posts.lock().unwrap()[0].body.clone()).unwrap();
    assert!(body.contains("\"digest\":3") && !body.contains("minor"), "{body}");
    assert_eq!(r.tick(day * 86_400 + 9 * 3600).digests, 0, "once a day");
}

#[test]
fn rate_limits_and_opt_outs_apply_per_channel_and_per_recipient() {
    let mut c = webhook(false);
    c.max_per_hour = 3;
    let r = rig("rate", vec![c, email(false)]);
    for i in 0..6 {
        r.hub.emit_at(&note(Severity::Normal, &format!("n{i}"), "", people(&["ann"])), T0 + i).unwrap();
    }
    let hooks = r.hub.deliveries().iter().filter(|d| d.channel == "/hook").count();
    assert_eq!(hooks, 3, "the channel's hourly cap");
    assert!(r.hub.suppressed() >= 3);
    assert_eq!(r.hub.inbox.list("ann").len(), 6, "the inbox is never limited");
    // an hour later there is room again
    r.hub.emit_at(&note(Severity::Normal, "later", "", people(&["ann"])), T0 + 4000).unwrap();
    assert_eq!(r.hub.deliveries().iter().filter(|d| d.channel == "/hook").count(), 4);
    // bob opts out of email; ann still gets it
    r.hub.set_optout("bob", "/mail", true).unwrap();
    let before = r.hub.deliveries().iter().filter(|d| d.channel == "/mail" && d.person == "bob").count();
    r.hub.emit_at(&note(Severity::Normal, "for both", "", people(&["bob"])), T0 + 5000).unwrap();
    assert_eq!(r.hub.deliveries().iter().filter(|d| d.channel == "/mail" && d.person == "bob").count(), before);
    // per recipient: 20 an hour
    let r2 = rig("rate2", vec![email(false)]);
    for i in 0..25 {
        r2.hub.emit_at(&note(Severity::Normal, &format!("n{i}"), "", people(&["ann"])), T0 + i).unwrap();
    }
    assert_eq!(r2.hub.deliveries().len(), super::service::PER_RECIPIENT_PER_HOUR);
}

#[test]
fn a_person_can_list_and_switch_off_only_their_own_mail_channel_and_the_inbox_note_is_still_made() {
    let r = rig("optout-api", vec![webhook(false), email(false)]);
    let v = super::api::channels(&r.hub, "bob");
    assert_eq!(v["channels"], serde_json::json!([{"channel": "/mail", "opted_out": false}]), "the shared webhook is not a personal choice");
    assert_eq!(super::api::channels(&r.hub, "zed")["channels"], serde_json::json!([]), "a person without an address on it sees none");
    assert_eq!(super::api::optout(&r.hub, "zed", &serde_json::json!({"channel": "/mail", "out": true})).unwrap_err().status, 404);
    assert_eq!(super::api::optout(&r.hub, "bob", &serde_json::json!({"channel": "/hook", "out": true})).unwrap_err().status, 404, "not a channel for bob");
    assert_eq!(super::api::optout(&r.hub, "bob", &serde_json::json!({"channel": "/mail"})).unwrap_err().status, 400);
    super::api::optout(&r.hub, "bob", &serde_json::json!({"channel": "/mail", "out": true})).unwrap();
    assert_eq!(super::api::channels(&r.hub, "bob")["channels"][0]["opted_out"], true);
    assert_eq!(super::api::channels(&r.hub, "ann")["channels"][0]["opted_out"], false, "ann is not affected");
    r.hub.emit_at(&note(Severity::Normal, "x", "", people(&["bob", "ann"])), T0).unwrap();
    assert!(r.hub.deliveries().iter().all(|d| !(d.channel == "/mail" && d.person == "bob")));
    assert!(r.hub.deliveries().iter().any(|d| d.channel == "/mail" && d.person == "ann"));
    assert_eq!(r.hub.inbox.list("bob").len(), 1, "the INBOX note is always made");
    super::api::optout(&r.hub, "bob", &serde_json::json!({"channel": "/mail", "out": false})).unwrap();
    assert_eq!(super::api::channels(&r.hub, "bob")["channels"][0]["opted_out"], false);
}

#[test]
fn an_unacknowledged_alert_climbs_from_the_owner_to_the_team_admin_to_the_tenant_admin() {
    let r = rig("escalate", vec![]);
    *r.env.rules.lock().unwrap() = Rules { escalate_after_secs: Some(600), ..Rules::default() };
    // cat is a plain tenant member and the "owner" of the alert; ann administers ops; adm and root administer the tenant
    let mut n = note(Severity::High, "prod deploy failed", "", people(&["cat"]));
    n.team = Some("ops".into());
    r.hub.emit_at(&n, T0).unwrap();
    assert_eq!(r.tick(T0 + 599).escalated, 0);
    assert_eq!(r.tick(T0 + 600).escalated, 1);
    assert!(r.hub.inbox.list("ann")[0].title.starts_with("Not acknowledged"), "the team admin hears of it");
    assert!(r.hub.inbox.list("adm").is_empty());
    assert_eq!(r.tick(T0 + 1200).escalated, 1);
    assert!(r.hub.inbox.list("adm")[0].title.contains("prod deploy failed") && !r.hub.inbox.list("root").is_empty(), "then the tenant admins");
    // ann acknowledges her copy: the chain of this alert is over
    let r2 = rig("escalate2", vec![]);
    *r2.env.rules.lock().unwrap() = Rules { escalate_after_secs: Some(600), ..Rules::default() };
    r2.hub.emit_at(&n, T0).unwrap();
    let id = r2.hub.inbox.list("cat")[0].id.clone();
    assert!(r2.hub.ack("cat", &id, T0 + 10).unwrap(), "the acknowledgement found its escalation");
    assert_eq!(r2.tick(T0 + 5000).escalated, 0);
    assert!(r2.hub.inbox.list("cat")[0].read);
    // below the threshold nothing escalates
    let r3 = rig("escalate3", vec![]);
    *r3.env.rules.lock().unwrap() = Rules { escalate_after_secs: Some(600), ..Rules::default() };
    let mut low = n.clone();
    low.severity = Severity::Normal;
    r3.hub.emit_at(&low, T0).unwrap();
    assert_eq!(r3.tick(T0 + 5000).escalated, 0);
}

fn pending(id: &str, created: u64, ttl: u64) -> Pending {
    Pending { id: id.into(), kind: "k3s.cluster".into(), team: "ops".into(), item: "prod".into(), requester: "bob".into(), created, expires: created + ttl, tenant_only: false, role_only: false, state: "pending".into(), decided_by: None }
}

#[test]
fn a_waiting_approval_is_announced_reminded_at_half_and_ninety_percent_and_reported_expired() {
    let r = rig("approvals", vec![]);
    r.env.approvals.lock().unwrap().push(pending("a1", T0, 1000));
    // nobody is signed in: the approver (ann, the team's admin; bob asked) hears of it
    assert_eq!(r.tick(T0 + 1).reminders, 1);
    let ann = r.hub.inbox.list("ann");
    assert_eq!(ann.len(), 1);
    assert!(ann[0].title.starts_with("Approval needed") && ann[0].link.as_ref().unwrap().app == "approvals");
    assert!(r.hub.inbox.list("bob").is_empty(), "the requester does not approve their own");
    assert_eq!(r.tick(T0 + 2).reminders, 0, "no second announcement");
    assert_eq!(r.tick(T0 + 499).reminders, 0);
    assert_eq!(r.tick(T0 + 500).reminders, 1);
    assert_eq!(r.tick(T0 + 501).reminders, 0, "once");
    assert_eq!(r.tick(T0 + 900).reminders, 1);
    assert!(r.hub.inbox.list("ann").iter().any(|n| n.title.starts_with("Last reminder")));
    assert_eq!(r.tick(T0 + 1000).reminders, 1);
    assert!(r.hub.inbox.list("bob")[0].title.starts_with("Approval expired"), "the requester is told");
    assert_eq!(r.hub.inbox.list("ann").len(), 4, "announce, 50 %, 90 %, expiry");
    assert_eq!(r.tick(T0 + 5000).reminders, 0);
    // a decided request is left alone
    let r2 = rig("approvals2", vec![]);
    let mut p = pending("a2", T0, 1000);
    p.state = "approved".into();
    r2.env.approvals.lock().unwrap().push(p);
    assert_eq!(r2.tick(T0 + 600).reminders, 0);
    // the reminders survive a restart without repeating
    let r3 = rig("approvals3", vec![]);
    r3.env.approvals.lock().unwrap().push(pending("a3", T0, 1000));
    r3.tick(T0 + 1);
    let b = r3.another_replica("replica-a");
    assert_eq!(b.tick(T0 + 2, r3.net.as_ref()).reminders, 0);
}

#[test]
fn a_second_decision_on_an_approval_is_refused_with_a_clear_message() {
    use crate::tenantconfig::api::{Caller, Env as CEnv, NoAudit};
    let dir = crate::testdir::unique("notifyhub", "decide");
    std::fs::create_dir_all(&dir).unwrap();
    let roles = roles();
    let env = CEnv { dir: &dir, vault: Arc::new(crate::tenantconfig::vault::NoVault), audit: &NoAudit };
    let p = crate::tenantconfig::approvals::request(&env, &roles, "k3s.cluster", "ops", "prod", "bob", false, false, T0).unwrap();
    let ann = Caller { user: "ann", roles: Some(&roles) };
    assert!(crate::tenantconfig::approvals::decide(&env, &ann, &p.id, true, T0 + 5).is_ok());
    let e = crate::tenantconfig::approvals::decide(&env, &ann, &p.id, true, T0 + 6).unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (409, "already decided"));
    let e = crate::tenantconfig::approvals::decide(&env, &ann, &p.id, false, T0 + 7).unwrap_err();
    assert_eq!(e.status, 409, "deny after approve is refused too");
    // the hub's snapshot shows the stored state, readable by the worker
    assert_eq!(crate::tenantconfig::approvals::snapshot(&dir)[0].state, "approved");
}

// ---------------------------------------------------------------------------------------------------------------- the live stream

#[test]
fn the_bus_sends_each_person_only_their_own_events() {
    let bus = Bus::new();
    let (a1, a2, b1) = (bus.subscribe("ann").unwrap(), bus.subscribe("ann").unwrap(), bus.subscribe("bob").unwrap());
    assert_eq!(bus.publish("ann", &events::event("notification", serde_json::json!({"id": "1"}))), 2);
    assert_eq!(bus.drain(a1, std::time::Duration::ZERO).unwrap().len(), 1);
    assert_eq!(bus.drain(a2, std::time::Duration::ZERO).unwrap().len(), 1);
    assert!(bus.drain(b1, std::time::Duration::ZERO).unwrap().is_empty(), "bob hears nothing of ann's");
    assert_eq!(bus.publish("nobody", "x"), 0);
    bus.unsubscribe(a1);
    assert!(bus.drain(a1, std::time::Duration::ZERO).is_none());
}

#[test]
fn a_slow_reader_loses_its_oldest_events_and_connections_are_capped() {
    let bus = Bus::new();
    let id = bus.subscribe("ann").unwrap();
    for i in 0..events::QUEUE_CAP + 7 {
        bus.publish("ann", &format!("e{i}"));
    }
    let got = bus.drain(id, std::time::Duration::ZERO).unwrap();
    assert_eq!(got.len(), events::QUEUE_CAP);
    assert_eq!(got[0], "e7", "the oldest seven were dropped");
    assert_eq!(got.last().unwrap(), &format!("e{}", events::QUEUE_CAP + 6));
    assert_eq!(bus.dropped(id), 7);
    for _ in 1..events::MAX_PER_PERSON {
        bus.subscribe("ann").unwrap();
    }
    assert!(bus.subscribe("ann").unwrap_err().contains("person"));
    let bus2 = Bus::new();
    for i in 0..events::MAX_PER_TENANT {
        bus2.subscribe(&format!("u{}", i / 2)).unwrap();
    }
    assert!(bus2.subscribe("fresh").unwrap_err().contains("tenant"));
}

#[test]
fn the_hub_publishes_a_notification_and_the_count_to_the_person_it_is_for() {
    let r = rig("publish", vec![]);
    let ann = r.hub.bus.subscribe("ann").unwrap();
    let cat = r.hub.bus.subscribe("cat").unwrap();
    r.hub.emit_at(&note(Severity::High, "disk full", "", people(&["ann"])), T0).unwrap();
    let evs = r.hub.bus.drain(ann, std::time::Duration::ZERO).unwrap();
    assert!(evs.iter().any(|e| e.contains("\"type\":\"notification\"") && e.contains("disk full")) && evs.iter().any(|e| e.contains("\"type\":\"count\"")));
    assert!(r.hub.bus.drain(cat, std::time::Duration::ZERO).unwrap().is_empty());
}
