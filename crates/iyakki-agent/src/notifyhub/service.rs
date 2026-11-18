//! The hub. `emit` writes the notification to the tenant's volume (a fan-out job in the state file, then each person's INBOX) before any
//! channel is tried; `tick` is the worker (queue, escalation, digest, approval reminders) and runs without any session, on a lease so that
//! only one replica delivers. All state is in `<volume>/cloud/notify/hub.json` and the INBOX files next to it, so a restart loses nothing.

use super::channel::{self, Channel, Kind as ChKind};
use super::escalation::{Esc, Step, Target};
use super::events::{self, Bus};
use super::expand::expand;
use super::lease::Lease;
use super::queue::{self, Delivery};
use super::redact::{redact, MAX_BODY, MAX_TITLE};
use super::reminders::{self, Stage};
use super::rules::{Route, Rules};
use super::sink::{Notification, NotifySink, Receipt, RoleSel, Recipients, Severity};
use super::transport::{MailJob, Net};
use crate::cloudroles::Roles;
use crate::inbox::{Inbox, Kind, Link};
use crate::tenantconfig::approvals::Pending;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const DEDUPE_SECS: u64 = 3600;
pub const LEASE_TTL: u64 = 60;
pub const PER_RECIPIENT_PER_HOUR: usize = 20;
const MAX_DIGEST: usize = 2000;
const MAX_ESC: usize = 500;

/// What the hub reads from the rest of the tenant. The real one reads the volume (`DirEnv`); tests supply a fake.
pub trait HubEnv: Send + Sync {
    fn roles(&self) -> Option<Roles>;
    fn channels(&self) -> Vec<Channel>;
    fn rules(&self, team: Option<&str>) -> Rules;
    /// The value behind a vault reference, for the moment of use.
    fn secret(&self, reference: &str) -> Option<String>;
    fn approvals(&self) -> Vec<Pending>;
    fn tenant_name(&self) -> String {
        "your tenant".into()
    }
}

pub struct DirEnv {
    pub dir: PathBuf,
}

impl HubEnv for DirEnv {
    fn roles(&self) -> Option<Roles> {
        crate::cloudaccess::Access::new(&self.dir).roles().ok().flatten()
    }
    fn channels(&self) -> Vec<Channel> {
        channel::load(&self.dir)
    }
    fn rules(&self, team: Option<&str>) -> Rules {
        channel::load_rules(&self.dir, team)
    }
    fn secret(&self, reference: &str) -> Option<String> {
        let r = crate::tenantconfig::schema::SecretRef::parse(reference)?;
        let mut out = None;
        let _ = crate::tenantconfig::vault::for_dir(&self.dir).use_secret(&r, "notify.channel", &mut |v| {
            out = Some(v.to_string());
            Ok(String::new())
        });
        out
    }
    fn approvals(&self) -> Vec<Pending> {
        crate::tenantconfig::approvals::snapshot(&self.dir)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    id: String,
    n: Notification,
    time: u64,
    inbox_done: BTreeSet<String>,
    /// An escalation step: the people join this escalation instead of starting one.
    esc: Option<String>,
    /// Sub-hub id of the notification, for the escalation record.
    #[serde(default)]
    inbox_ids: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DigestItem {
    channel: String,
    person: String,
    time: u64,
    title: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Track {
    created: u64,
    expires: u64,
    mask: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    seq: u64,
    jobs: Vec<Job>,
    deliveries: Vec<Delivery>,
    dead: Vec<Delivery>,
    dedupe: BTreeMap<String, u64>,
    esc: Vec<Esc>,
    digest: Vec<DigestItem>,
    last_digest_day: i64,
    sent: BTreeMap<String, Vec<u64>>,
    /// person -> channel keys they opted out of
    optout: BTreeMap<String, BTreeSet<String>>,
    appr: BTreeMap<String, Track>,
    suppressed: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub leader: bool,
    pub delivered: usize,
    pub retried: usize,
    pub dead: usize,
    pub escalated: usize,
    pub reminders: usize,
    pub digests: usize,
}

pub struct Hub {
    pub dir: PathBuf,
    pub tenant: String,
    pub inbox: Inbox,
    pub bus: Arc<Bus>,
    env: Box<dyn HubEnv>,
    lock: Mutex<()>,
    lease: Lease,
    holder: String,
}

fn kind_of(s: Severity) -> Kind {
    match s {
        Severity::Low | Severity::Normal => Kind::Info,
        Severity::High => Kind::Warning,
        Severity::Critical => Kind::Error,
    }
}

fn new_id() -> String {
    let mut r = [0u8; 6];
    let _ = iyakki_sec::random_fill(&mut r);
    r.iter().map(|b| format!("{b:02x}")).collect()
}

fn allow(sent: &mut BTreeMap<String, Vec<u64>>, key: &str, limit: usize, now: u64) -> bool {
    let v = sent.entry(key.to_string()).or_default();
    v.retain(|t| *t + 3600 > now);
    v.len() < limit
}

fn count_sent(sent: &mut BTreeMap<String, Vec<u64>>, key: &str, now: u64) {
    let v = sent.entry(key.to_string()).or_default();
    v.push(now);
    if v.len() > 10_000 {
        v.remove(0);
    }
}

impl Hub {
    pub fn new(dir: &Path, tenant: &str, env: Box<dyn HubEnv>, holder: &str) -> Hub {
        let base = dir.join("cloud").join("notify");
        Hub { dir: dir.into(), tenant: tenant.into(), inbox: Inbox::new(&base.join("inbox")), bus: events::bus_for(dir), env, lock: Mutex::new(()), lease: Lease::new(base.join("lease.json")), holder: holder.into() }
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("cloud").join("notify").join("hub.json")
    }

    /// Load, change, save: one at a time in this process (mutex) and across processes (file lock). Nothing slow runs inside.
    fn with_state<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<T, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let p = self.state_path();
        std::fs::create_dir_all(p.parent().ok_or("no directory")?).map_err(|e| e.to_string())?;
        let guard = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.with_extension("lock")).map_err(|e| e.to_string())?;
        guard.lock().map_err(|e| e.to_string())?;
        let mut st: State = match std::fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|_| {
                let _ = iyakki_schema::move_aside(&p, iyakki_schema::now_secs());
                State::default()
            }),
            Err(_) => State::default(),
        };
        let r = f(&mut st);
        let saved = iyakki_schema::write_atomic(&p, &serde_json::to_vec(&st).map_err(|e| e.to_string())?, 0o600);
        let _ = guard.unlock();
        saved.map(|_| r)
    }

    // ------------------------------------------------------------------------------------------------------------ emit

    pub fn emit_at(&self, n: &Notification, now: u64) -> Result<Receipt, String> {
        if n.tenant != self.tenant {
            return Err("this notification is for another tenant".into());
        }
        if let Some(t) = &n.team {
            if !crate::cloudstorage::plain_name(t) {
                return Err("not a team name".into());
            }
        }
        let mut n = n.clone();
        n.title = redact(&n.title, MAX_TITLE, false);
        n.body = redact(&n.body, MAX_BODY, true);
        n.source = redact(&n.source, 40, false);
        if n.title.is_empty() {
            return Err("a notification needs a title".into());
        }
        let id = new_id();
        let fresh = self.with_state(|st| {
            st.dedupe.retain(|_, t| *t + DEDUPE_SECS > now);
            if !n.dedupe_key.is_empty() && st.dedupe.contains_key(&n.dedupe_key) {
                return false;
            }
            if !n.dedupe_key.is_empty() {
                st.dedupe.insert(n.dedupe_key.clone(), now);
            }
            // durable first: from here a crash cannot lose it
            st.jobs.push(Job { id: id.clone(), n: n.clone(), time: now, inbox_done: BTreeSet::new(), esc: None, inbox_ids: BTreeMap::new() });
            true
        })?;
        if !fresh {
            return Ok(Receipt { id, deduped: true, recipients: vec![], queued: 0 });
        }
        let (recipients, queued) = self.process_job(&id, now)?;
        Ok(Receipt { id, deduped: false, recipients, queued })
    }

    /// Fan one stored job out: INBOX notes (each remembered as done), then in one step the channel deliveries, the escalation record and
    /// the removal of the job. Safe to run again after a crash.
    fn process_job(&self, job_id: &str, now: u64) -> Result<(Vec<String>, usize), String> {
        let Some(job) = self.with_state(|st| st.jobs.iter().find(|j| j.id == job_id).cloned())? else { return Ok((vec![], 0)) };
        let n = &job.n;
        let roles = self.env.roles();
        let people = expand(roles.as_ref(), n.team.as_deref(), &n.recipients);
        let mut ids: BTreeMap<String, String> = job.inbox_ids.clone();
        for p in &people {
            if job.inbox_done.contains(p) {
                continue;
            }
            let link = n.link.clone();
            let kind = if n.source == "approval" { Kind::Agent } else { kind_of(n.severity) };
            let note = self.inbox.add(p, kind, &n.title, &n.body, &n.source, link, now as i64);
            if let Ok(note) = &note {
                ids.insert(p.clone(), note.id.clone());
                let ev = events::event("notification", json!({"id": note.id, "kind": note.kind.name(), "title": note.title, "severity": n.severity.name(), "time": note.time}));
                self.bus.publish(p, &ev);
                self.publish_count(p, now);
            }
            let (p2, ids2) = (p.clone(), ids.clone());
            self.with_state(|st| {
                if let Some(j) = st.jobs.iter_mut().find(|j| j.id == job_id) {
                    j.inbox_done.insert(p2);
                    j.inbox_ids = ids2;
                }
            })?;
        }
        let rules = self.env.rules(n.team.as_deref());
        let channels = self.env.channels();
        let tenant_name = self.env.tenant_name();
        let queued = self.with_state(|st| {
            let mut queued = 0;
            for ch in channels.iter().filter(|c| c.team.is_none() || c.team == n.team) {
                let targets: Vec<String> = match ch.kind {
                    ChKind::Webhook => vec!["*".into()],
                    ChKind::Email => people.iter().filter(|p| ch.addresses.contains_key(*p)).cloned().collect(),
                };
                for person in targets {
                    if st.optout.get(&person).is_some_and(|s| s.contains(&ch.key)) {
                        continue;
                    }
                    match rules.route(n.severity, ch.min_severity, now) {
                        Route::Skip => continue,
                        Route::Digest => {
                            if st.digest.len() < MAX_DIGEST {
                                st.digest.push(DigestItem { channel: ch.key.clone(), person: person.clone(), time: now, title: n.title.clone() });
                            }
                            continue;
                        }
                        Route::Send(at) => {
                            let (ck, rk) = (format!("ch:{}", ch.key), format!("rc:{person}"));
                            if !allow(&mut st.sent, &ck, ch.max_per_hour as usize, now) || (person != "*" && !allow(&mut st.sent, &rk, PER_RECIPIENT_PER_HOUR, now)) {
                                st.suppressed += 1;
                                continue;
                            }
                            count_sent(&mut st.sent, &ck, now);
                            if person != "*" {
                                count_sent(&mut st.sent, &rk, now);
                            }
                            st.seq += 1;
                            let (subject, body) = render(ch, n, &tenant_name, &job.id, &person);
                            let d = Delivery { id: format!("{}-{}", job_id, st.seq), channel: ch.key.clone(), person: person.clone(), subject, body, severity: n.severity, attempts: 0, next_at: at, created: now, last_error: String::new() };
                            if queue::push(&mut st.deliveries, d).is_ok() {
                                queued += 1;
                            } else {
                                st.suppressed += 1;
                            }
                        }
                    }
                }
            }
            match &job.esc {
                Some(eid) => {
                    if let Some(e) = st.esc.iter_mut().find(|e| e.id == *eid) {
                        e.inbox.extend(ids.clone());
                    }
                }
                None => {
                    if let Some(after) = rules.escalate_after_secs.filter(|_| n.severity >= rules.escalate_min && !people.is_empty()) {
                        if st.esc.len() < MAX_ESC {
                            st.esc.push(Esc { id: job_id.to_string(), team: n.team.clone(), after, level: 0, last: now, acked: false, done: false, severity: n.severity, title: n.title.clone(), body: n.body.clone(), link: n.link.clone(), source: n.source.clone(), inbox: ids.clone() });
                        }
                    }
                }
            }
            st.jobs.retain(|j| j.id != job_id);
            queued
        })?;
        Ok((people, queued))
    }

    fn publish_count(&self, user: &str, now: u64) {
        let c = self.inbox.counts(user, now as i64);
        self.bus.publish(user, &events::event("count", json!({"unread": c.unread, "attention": c.attention, "overdue": c.overdue})));
    }

    // ------------------------------------------------------------------------------------------------------------ person's actions

    pub fn ack(&self, user: &str, inbox_id: &str, now: u64) -> Result<bool, String> {
        self.inbox.mark_read(user, inbox_id, true)?;
        let found = self.with_state(|st| {
            let mut any = false;
            for e in st.esc.iter_mut().filter(|e| e.inbox.get(user).map(String::as_str) == Some(inbox_id)) {
                e.ack();
                any = true;
            }
            any
        })?;
        self.publish_count(user, now);
        Ok(found)
    }

    pub fn set_optout(&self, user: &str, channel_key: &str, out: bool) -> Result<(), String> {
        self.with_state(|st| {
            let s = st.optout.entry(user.to_string()).or_default();
            if out { s.insert(channel_key.to_string()); } else { s.remove(channel_key); }
        })
    }

    /// The channels that reach this person themselves (their own address is on it), with whether they opted out. A shared webhook
    /// is the tenant's and not a personal choice, so it is not listed.
    pub fn personal_channels(&self, user: &str) -> Vec<(String, bool)> {
        let out = self.with_state(|st| st.optout.get(user).cloned().unwrap_or_default()).unwrap_or_default();
        self.env.channels().into_iter().filter(|c| matches!(c.kind, ChKind::Email) && c.addresses.contains_key(user)).map(|c| (c.key.clone(), out.contains(&c.key))).collect()
    }

    pub fn queue_len(&self) -> (usize, usize) {
        self.with_state(|st| (st.deliveries.len(), st.dead.len())).unwrap_or((0, 0))
    }

    pub fn deliveries(&self) -> Vec<Delivery> {
        self.with_state(|st| st.deliveries.clone()).unwrap_or_default()
    }

    pub fn suppressed(&self) -> u64 {
        self.with_state(|st| st.suppressed).unwrap_or(0)
    }

    // ------------------------------------------------------------------------------------------------------------ the worker

    /// One round of the worker. Without the lease it does nothing (another replica delivers).
    pub fn tick(&self, now: u64, net: &dyn Net) -> TickReport {
        let mut r = TickReport::default();
        if !self.lease.try_acquire(&self.holder, now, LEASE_TTL) {
            return r;
        }
        r.leader = true;
        // 1. work that a crash left half done
        if let Ok(ids) = self.with_state(|st| st.jobs.iter().map(|j| j.id.clone()).collect::<Vec<_>>()) {
            for id in ids {
                let _ = self.process_job(&id, now);
            }
        }
        // 2. approvals that wait: announce, remind at 50 % and 90 %, report expiry
        r.reminders = self.check_approvals(now);
        // 3. escalation
        r.escalated = self.escalate(now);
        // 4. digest
        r.digests = self.flush_digest(now);
        // 5. deliveries that are due
        self.deliver_due(now, net, &mut r);
        r
    }

    fn check_approvals(&self, now: u64) -> usize {
        let pend = self.env.approvals();
        let Some(roles) = self.env.roles() else { return 0 };
        let mut sent = 0;
        for p in pend.iter().filter(|p| p.state == "pending") {
            let mask = self.with_state(|st| st.appr.entry(p.id.clone()).or_insert(Track { created: p.created, expires: p.expires, mask: 0 }).mask).unwrap_or(255);
            for stage in reminders::due(p.created, p.expires, now, mask) {
                let route = crate::tenantconfig::approvals::route(&roles, &p.team, &p.requester, p.tenant_only, &[p.item.as_str(), p.kind.as_str()], p.role_only);
                let mut who = route.approvers.clone();
                let (title, sev) = match stage {
                    Stage::Announce => (format!("Approval needed: {} {} (asked by {})", p.kind, p.item, p.requester), Severity::Normal),
                    Stage::Half => (format!("Reminder: approval still waiting for {} {} (half the time has passed)", p.kind, p.item), Severity::Normal),
                    Stage::Late => (format!("Last reminder: approval for {} {} expires soon", p.kind, p.item), Severity::High),
                    Stage::Expired => {
                        who.push(p.requester.clone());
                        (format!("Approval expired: {} {} (ask again if it is still needed)", p.kind, p.item), Severity::Normal)
                    }
                };
                let n = Notification {
                    tenant: self.tenant.clone(),
                    team: Some(p.team.clone()),
                    recipients: Recipients::People(who),
                    severity: sev,
                    title,
                    body: "Sign in to IYAKKI and decide on the Notifications page. An approval always needs your own sign-in.".into(),
                    link: Some(Link::new("approvals", &p.id)),
                    dedupe_key: format!("appr:{}:{}", p.id, stage.key()),
                    source: "approval".into(),
                };
                if self.emit_at(&n, now).is_ok() {
                    let (id, bit) = (p.id.clone(), stage.bit());
                    let _ = self.with_state(|st| {
                        if let Some(t) = st.appr.get_mut(&id) {
                            t.mask |= bit;
                        }
                    });
                    sent += 1;
                }
            }
        }
        let _ = self.with_state(|st| st.appr.retain(|_, t| t.expires + 8 * 86_400 > now));
        sent
    }

    fn escalate(&self, now: u64) -> usize {
        let roles = self.env.roles();
        let steps: Vec<(Esc, Target)> = self
            .with_state(|st| {
                let mut v = Vec::new();
                for e in st.esc.iter_mut() {
                    let s = e.step(now);
                    if let Step::Escalate(t) = s {
                        v.push((e.clone(), t));
                    }
                    e.apply(s, now);
                }
                st.esc.retain(|e| !e.done || now < e.last + 86_400);
                v
            })
            .unwrap_or_default();
        let mut count = 0;
        for (e, t) in steps {
            let sel = match t {
                Target::TeamAdmins => RoleSel::TeamAdmin,
                Target::TenantAdmins => RoleSel::TenantAdmin,
            };
            let people = expand(roles.as_ref(), e.team.as_deref(), &Recipients::Roles(vec![sel]));
            if people.is_empty() {
                continue;
            }
            let n = Notification {
                tenant: self.tenant.clone(),
                team: e.team.clone(),
                recipients: Recipients::People(people),
                severity: e.severity,
                title: format!("Not acknowledged: {}", e.title),
                body: e.body.clone(),
                link: e.link.clone(),
                dedupe_key: String::new(),
                source: e.source.clone(),
            };
            let job_id = new_id();
            let esc_id = e.id.clone();
            let queued = self.with_state(|st| st.jobs.push(Job { id: job_id.clone(), n, time: now, inbox_done: BTreeSet::new(), esc: Some(esc_id), inbox_ids: BTreeMap::new() }));
            if queued.is_ok() && self.process_job(&job_id, now).is_ok() {
                count += 1;
            }
        }
        count
    }

    fn flush_digest(&self, now: u64) -> usize {
        let rules = self.env.rules(None);
        let channels = self.env.channels();
        let tenant_name = self.env.tenant_name();
        self.with_state(|st| {
            let Some(day) = rules.digest_due(now, st.last_digest_day) else { return 0 };
            st.last_digest_day = day;
            let items = std::mem::take(&mut st.digest);
            let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
            for i in items {
                groups.entry((i.channel, i.person)).or_default().push(i.title);
            }
            let mut n = 0;
            for ((chk, person), titles) in groups {
                let Some(ch) = channels.iter().find(|c| c.key == chk) else { continue };
                st.seq += 1;
                let subject = format!("[IYAKKI] Digest: {} notifications", titles.len());
                let mut body = format!("{} low-priority notifications are waiting for you in {tenant_name}.\n", titles.len());
                if ch.include_details {
                    for t in titles.iter().take(20) {
                        body.push_str(&format!("- {t}\n"));
                    }
                }
                body.push_str(&format!("Sign in to read them: {}\n", page_link(ch)));
                let body = if ch.kind == ChKind::Webhook { json!({"v": 1, "digest": titles.len(), "tenant": tenant_name, "link": page_link(ch)}).to_string() } else { body };
                let d = Delivery { id: format!("dg-{}", st.seq), channel: chk, person, subject, body, severity: Severity::Low, attempts: 0, next_at: now, created: now, last_error: String::new() };
                if queue::push(&mut st.deliveries, d).is_ok() {
                    n += 1;
                }
            }
            n
        })
        .unwrap_or(0)
    }

    fn deliver_due(&self, now: u64, net: &dyn Net, r: &mut TickReport) {
        let due: Vec<Delivery> = self.with_state(|st| queue::due(&st.deliveries, now).iter().filter_map(|id| st.deliveries.iter().find(|d| d.id == *id).cloned()).collect()).unwrap_or_default();
        let channels = self.env.channels();
        for d in due {
            let result = match channels.iter().find(|c| c.key == d.channel) {
                // the channel was switched off or removed after this was queued: nothing leaves
                None => Ok(()),
                Some(ch) => self.send_one(ch, &d, now, net),
            };
            let _ = self.with_state(|st| match &result {
                Ok(()) => {
                    queue::done(&mut st.deliveries, &d.id);
                    r.delivered += 1;
                }
                Err(e) => match queue::failed(&mut st.deliveries, &mut st.dead, &d.id, now, e) {
                    Some(queue::Outcome::Dead) => r.dead += 1,
                    Some(queue::Outcome::Retry(_)) => r.retried += 1,
                    None => {}
                },
            });
        }
    }

    fn send_one(&self, ch: &Channel, d: &Delivery, now: u64, net: &dyn Net) -> Result<(), String> {
        match ch.kind {
            ChKind::Webhook => {
                let secret = self.env.secret(&ch.signing_secret).ok_or("the signing secret is not available (vault locked or missing)")?;
                let payload: serde_json::Value = serde_json::from_str(&d.body).map_err(|_| "bad payload".to_string())?;
                let req = super::webhook::build(&ch.url, secret.as_bytes(), now as i64, &payload)?;
                let status = net.post(ch.team.as_deref(), &req)?;
                if super::webhook::status_ok(status) { Ok(()) } else { Err(format!("the receiver answered {status}")) }
            }
            ChKind::Email => {
                let to = ch.addresses.get(&d.person).ok_or("no address for this person")?.clone();
                let pass = if ch.password.is_empty() { None } else { Some(self.env.secret(&ch.password).ok_or("the mail password is not available (vault locked or missing)")?) };
                let job = MailJob { host: ch.host.clone(), port: ch.port, mode: ch.tls, user: (!ch.username.is_empty()).then(|| ch.username.clone()), pass, from: ch.from.clone(), to, subject: d.subject.clone(), body: d.body.clone(), now, id: d.id.clone() };
                net.send_mail(ch.team.as_deref(), &job)
            }
        }
    }
}

fn page_link(ch: &Channel) -> String {
    if ch.link_base.is_empty() { "open the Notifications page in IYAKKI".into() } else { format!("{}/ui/#/notifications", ch.link_base) }
}

/// What goes out. By default only a short summary and a link; details only when the channel says so and the admin acknowledged it.
/// There is never a link that approves or denies anything: a decision always needs the person's own sign-in.
fn render(ch: &Channel, n: &Notification, tenant_name: &str, id: &str, _person: &str) -> (String, String) {
    let approval = n.source == "approval";
    let summary = if approval { "An approval is waiting for you.".to_string() } else { format!("A {} notification is waiting for you.", n.severity.name()) };
    let link = page_link(ch);
    match ch.kind {
        ChKind::Email => {
            if ch.include_details {
                (format!("[IYAKKI] {}", n.title), format!("{}\n\n{}\n\nSign in to act on it: {link}\n", n.title, n.body))
            } else {
                ("[IYAKKI] You have a notification".into(), format!("{summary}\nTenant: {tenant_name}\nSign in to read it: {link}\n"))
            }
        }
        ChKind::Webhook => {
            let mut p = json!({"v": 1, "id": id, "severity": n.severity.name(), "tenant": tenant_name, "summary": summary, "link": link});
            if ch.include_details {
                p["title"] = json!(n.title);
                p["body"] = json!(n.body);
                p["source"] = json!(n.source);
                p["team"] = json!(n.team);
            }
            ("notification".into(), p.to_string())
        }
    }
}

impl NotifySink for Hub {
    fn emit(&self, n: &Notification) -> Result<Receipt, String> {
        self.emit_at(n, iyakki_schema::now_secs())
    }
}

/// The hub of the tenant whose volume is `dir`, made once.
pub fn hub_for(dir: &Path) -> Arc<Hub> {
    static R: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Hub>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(dir.to_path_buf())
        .or_insert_with(|| {
            let tenant = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Arc::new(Hub::new(dir, &tenant, Box::new(DirEnv { dir: dir.into() }), &format!("pid{}", std::process::id())))
        })
        .clone()
}
