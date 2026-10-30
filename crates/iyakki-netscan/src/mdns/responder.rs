//! The device's side of mDNS (RFC 6762, the subset the stable device name needs), as a state machine with no sockets and no clock of
//! its own: packets and the time (milliseconds) go in, packets to send come out. [`super::net`] runs it on real sockets.
//!
//! * It answers questions for its own names ONLY: the host name (`iyakki-<id>.local`, A and AAAA), the service types it offers
//!   (`_iyakki-dev._tcp.local`, PTR with the SRV, TXT and addresses alongside) and the service-type meta query. Everything else is
//!   ignored in silence: no answer, no "does not exist" record, no forwarding, no cache. Responses from other machines are read
//!   only to find a name conflict.
//! * Start: three probes 250 ms apart for the host name (RFC 6762 section 8); a response or a lost tie-break with other data for
//!   the name means another machine owns it, so a suffix is added (`iyakki-<id>-2`) and the probing starts again. Then two
//!   announcements one second apart. Later address changes are announced again at once (and the old addresses get goodbyes).
//! * A machine that sees somebody else claiming the name while it is already running does NOT change its name (a stranger on the
//!   network must not be able to rename a device that computers have paired with): it defends the name by announcing again.
//! * Rate limits: at most 10 queries per second from one source are looked at, 40 responses per 10 seconds are sent, and the
//!   same multicast record is not sent twice within a second. A legacy unicast query (from a port other than 5353, which is what
//!   the client uses) is answered to that sender with its id, the question repeated and a short time to live.
//! * [`Responder::stop`] gives the goodbye packets (every record with a time to live of 0).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::wire::{self, Data, Message, Question, Record};

pub const MDNS_PORT: u16 = 5353;
pub const MDNS_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const TTL_HOST: u32 = 120;
const TTL_OTHER: u32 = 4500;
const MAX_QUESTIONS: usize = 8;
const MAX_RENAMES: u32 = 8;
/// Queries per second looked at, per source address.
pub const SOURCE_PER_SEC: u32 = 10;
/// Responses sent per 10 seconds, in all.
pub const RESPONSES_PER_10S: u32 = 40;
/// Largest query looked at: ours are tiny, and a query with a long known-answer list is not worth the work.
pub const MAX_QUERY: usize = 2048;

/// A service the device offers: `kind` is `_iyakki-dev._tcp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Svc {
    pub kind: String,
    pub port: u16,
    pub txt: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dest {
    Multicast,
    Unicast(SocketAddr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    pub dest: Dest,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Checking that nobody else uses the name.
    Probing,
    Announcing,
    Running,
    Stopped,
}

#[derive(Default)]
struct Limits {
    sources: HashMap<IpAddr, (u64, u32)>,
    global: (u64, u32),
    last_multicast: HashMap<(String, u16), u64>,
}

pub struct Responder {
    base: String,
    suffix: u32,
    v4: Vec<Ipv4Addr>,
    v6: Vec<Ipv6Addr>,
    svcs: Vec<Svc>,
    state: State,
    probes: u8,
    announces: u8,
    next_at: u64,
    renames: u32,
    seed: u32,
    goodbyes: Vec<Record>,
    lim: Limits,
    last_defence: u64,
    /// Counters for the diagnosis output.
    pub dropped: u64,
    pub rate_limited: u64,
    pub answered: u64,
}

impl Responder {
    /// `base` is the label of the host name without `.local` (`iyakki-a1b2c3`).
    pub fn new(base: &str, now: u64) -> Responder {
        let seed = base.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
        let mut r = Responder { base: base.to_ascii_lowercase(), suffix: 0, v4: vec![], v6: vec![], svcs: vec![], state: State::Probing, probes: 0, announces: 0, next_at: 0, renames: 0, seed, goodbyes: vec![], lim: Limits::default(), last_defence: 0, dropped: 0, rate_limited: 0, answered: 0 };
        r.next_at = now + r.jitter(now);
        r
    }

    fn jitter(&self, now: u64) -> u64 {
        (self.seed as u64 ^ now.wrapping_mul(2654435761)) % 250
    }

    pub fn label(&self) -> String {
        if self.suffix == 0 { self.base.clone() } else { format!("{}-{}", self.base, self.suffix) }
    }

    /// The host name, for example `iyakki-a1b2c3.local`.
    pub fn host(&self) -> String {
        format!("{}.local", self.label())
    }

    fn type_name(kind: &str) -> String {
        format!("{kind}.local")
    }

    fn instance(&self, kind: &str) -> String {
        format!("{}.{kind}.local", self.label())
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn addresses(&self) -> (Vec<Ipv4Addr>, Vec<Ipv6Addr>) {
        (self.v4.clone(), self.v6.clone())
    }

    pub fn services(&self) -> &[Svc] {
        &self.svcs
    }

    fn addr_records(&self, ttl: Option<u32>) -> Vec<Record> {
        let host = self.host();
        let ttl = |t: u32| ttl.map_or(t, |x| x.min(t));
        self.v4.iter().map(|a| Record { name: host.clone(), rtype: wire::TYPE_A, flush: true, ttl: ttl(TTL_HOST), data: Data::A(*a) }).chain(self.v6.iter().map(|a| Record { name: host.clone(), rtype: wire::TYPE_AAAA, flush: true, ttl: ttl(TTL_HOST), data: Data::Aaaa(*a) })).collect()
    }

    fn svc_records(&self, s: &Svc, ttl: Option<u32>) -> (Record, Record, Record) {
        let ttl = |t: u32| ttl.map_or(t, |x| x.min(t));
        let inst = self.instance(&s.kind);
        let ptr = Record { name: Self::type_name(&s.kind), rtype: wire::TYPE_PTR, flush: false, ttl: ttl(TTL_OTHER), data: Data::Ptr(inst.clone()) };
        let srv = Record { name: inst.clone(), rtype: wire::TYPE_SRV, flush: true, ttl: ttl(TTL_HOST), data: Data::Srv { priority: 0, weight: 0, port: s.port, target: self.host() } };
        let mut txt = vec!["v=1".to_string()];
        txt.extend(s.txt.iter().cloned());
        let txt = Record { name: inst, rtype: wire::TYPE_TXT, flush: true, ttl: ttl(TTL_OTHER), data: Data::Txt(txt) };
        (ptr, srv, txt)
    }

    /// Every record of the device (for announcements and goodbyes).
    fn all_records(&self, ttl: Option<u32>) -> Vec<Record> {
        let mut v = self.addr_records(ttl);
        for s in &self.svcs {
            let (p, srv, t) = self.svc_records(s, ttl);
            v.extend([p, srv, t]);
        }
        v
    }

    /// The device's addresses changed. They are announced again at once; the ones that went get a goodbye. Returns whether anything changed.
    pub fn set_addresses(&mut self, mut v4: Vec<Ipv4Addr>, mut v6: Vec<Ipv6Addr>, now: u64) -> bool {
        v4.sort();
        v4.dedup();
        v6.sort();
        v6.dedup();
        if v4 == self.v4 && v6 == self.v6 {
            return false;
        }
        let host = self.host();
        if self.state != State::Probing && self.state != State::Stopped {
            for a in self.v4.iter().filter(|a| !v4.contains(a)) {
                self.goodbyes.push(Record { name: host.clone(), rtype: wire::TYPE_A, flush: true, ttl: 0, data: Data::A(*a) });
            }
            for a in self.v6.iter().filter(|a| !v6.contains(a)) {
                self.goodbyes.push(Record { name: host.clone(), rtype: wire::TYPE_AAAA, flush: true, ttl: 0, data: Data::Aaaa(*a) });
            }
        }
        let had_none = self.v4.is_empty() && self.v6.is_empty();
        self.v4 = v4;
        self.v6 = v6;
        match self.state {
            State::Probing => {
                if had_none {
                    self.next_at = now + self.jitter(now);
                }
            }
            State::Announcing | State::Running => {
                self.state = State::Announcing;
                self.announces = 0;
                self.next_at = now;
            }
            State::Stopped => {}
        }
        true
    }

    /// The services changed (a port, the developer mode switch...). Announced again at once; removed ones get a goodbye.
    pub fn set_services(&mut self, svcs: Vec<Svc>, now: u64) -> bool {
        if svcs == self.svcs {
            return false;
        }
        if self.state != State::Probing && self.state != State::Stopped {
            let gone: Vec<Svc> = self.svcs.iter().filter(|s| !svcs.iter().any(|n| n.kind == s.kind)).cloned().collect();
            for s in gone {
                let (p, srv, t) = self.svc_records(&s, Some(0));
                self.goodbyes.extend([p, srv, t]);
            }
        }
        self.svcs = svcs;
        if matches!(self.state, State::Announcing | State::Running) {
            self.state = State::Announcing;
            self.announces = 0;
            self.next_at = now;
        }
        true
    }

    fn send(&self, m: &Message) -> Option<Out> {
        wire::encode(m).ok().map(|bytes| Out { dest: Dest::Multicast, bytes })
    }

    /// What is due now: probes, announcements, goodbyes of removed records.
    pub fn poll(&mut self, now: u64) -> Vec<Out> {
        let mut out = Vec::new();
        if self.state == State::Stopped {
            return out;
        }
        if !self.goodbyes.is_empty() {
            let g = std::mem::take(&mut self.goodbyes);
            out.extend(g.chunks(8).filter_map(|c| self.send(&Message::response(c.to_vec(), vec![]))));
        }
        if now < self.next_at || (self.v4.is_empty() && self.v6.is_empty()) {
            return out;
        }
        match self.state {
            State::Probing => {
                let q = Question { name: self.host(), qtype: wire::TYPE_ANY, unicast: true };
                let m = Message { questions: vec![q], authority: self.addr_records(None), ..Default::default() };
                out.extend(self.send(&m));
                self.probes += 1;
                self.next_at = now + 250;
                if self.probes >= 3 {
                    self.state = State::Announcing;
                    self.announces = 0;
                }
            }
            State::Announcing => {
                let all = self.all_records(None);
                let (answers, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|r| matches!(r.data, Data::Ptr(_)) || matches!(r.data, Data::A(_) | Data::Aaaa(_)));
                out.extend(self.send(&Message::response(answers, rest)));
                self.announces += 1;
                self.next_at = now + 1000;
                if self.announces >= 2 {
                    self.state = State::Running;
                }
            }
            State::Running | State::Stopped => {}
        }
        out
    }

    /// The goodbye packets for everything announced; afterwards nothing is sent or answered.
    pub fn stop(&mut self) -> Vec<Out> {
        let was = self.state;
        self.state = State::Stopped;
        if was == State::Probing || (self.v4.is_empty() && self.v6.is_empty()) {
            return vec![];
        }
        let recs = self.all_records(Some(0));
        let mut v: Vec<Record> = recs.into_iter().map(|mut r| {
            r.ttl = 0;
            r
        }).collect();
        v.extend(std::mem::take(&mut self.goodbyes));
        v.chunks(8).filter_map(|c| self.send(&Message::response(c.to_vec(), vec![]))).collect()
    }

    fn rename(&mut self, now: u64) -> bool {
        if self.renames >= MAX_RENAMES {
            return false;
        }
        self.renames += 1;
        self.suffix = if self.suffix == 0 { 2 } else { self.suffix + 1 };
        self.state = State::Probing;
        self.probes = 0;
        self.announces = 0;
        self.next_at = now + self.jitter(now);
        true
    }

    fn own_data(&self, d: &Data) -> bool {
        match d {
            Data::A(a) => self.v4.contains(a),
            Data::Aaaa(a) => self.v6.contains(a),
            _ => true,
        }
    }

    fn rate_ok(&mut self, from: IpAddr, now: u64) -> bool {
        if self.lim.sources.len() > 128 {
            self.lim.sources.clear();
        }
        let e = self.lim.sources.entry(from).or_insert((now, 0));
        if now.saturating_sub(e.0) >= 1000 {
            *e = (now, 0);
        }
        e.1 += 1;
        if e.1 > SOURCE_PER_SEC {
            self.rate_limited += 1;
            return false;
        }
        true
    }

    fn global_ok(&mut self, now: u64) -> bool {
        let g = &mut self.lim.global;
        if now.saturating_sub(g.0) >= 10_000 {
            *g = (now, 0);
        }
        if g.1 >= RESPONSES_PER_10S {
            self.rate_limited += 1;
            return false;
        }
        g.1 += 1;
        true
    }

    /// A packet arrived from `from`. The caller has already checked that `from` is on a local network and is not this machine.
    pub fn on_packet(&mut self, buf: &[u8], from: SocketAddr, now: u64) -> Vec<Out> {
        if self.state == State::Stopped {
            return vec![];
        }
        // responses can be bigger than queries; only a query is refused for size
        if buf.len() > MAX_QUERY && buf.get(2).is_some_and(|f| f & 0x80 == 0) {
            self.dropped += 1;
            return vec![];
        }
        let Ok(msg) = wire::decode(buf) else {
            self.dropped += 1;
            return vec![];
        };
        if msg.is_response() {
            return self.on_response(&msg, now);
        }
        if buf.len() > MAX_QUERY || msg.questions.len() > MAX_QUESTIONS {
            self.dropped += 1;
            return vec![];
        }
        if !self.rate_ok(from.ip(), now) {
            return vec![];
        }
        if self.state == State::Probing {
            self.tie_break(&msg, now);
            return vec![];
        }
        self.on_query(&msg, from, now)
    }

    fn on_response(&mut self, msg: &Message, now: u64) -> Vec<Out> {
        let host = self.host();
        let theirs = msg.answers.iter().chain(&msg.additional).any(|r| r.name == host && matches!(r.data, Data::A(_) | Data::Aaaa(_)) && r.ttl > 0 && !self.own_data(&r.data));
        if !theirs {
            return vec![];
        }
        match self.state {
            State::Probing => {
                self.rename(now);
                vec![]
            }
            State::Announcing | State::Running => {
                // somebody else claims the name: defend it (announce again), at most once in 10 seconds; never rename
                if now.saturating_sub(self.last_defence) >= 10_000 || self.last_defence == 0 {
                    self.last_defence = now.max(1);
                    self.state = State::Announcing;
                    self.announces = 1;
                    self.next_at = now;
                }
                vec![]
            }
            State::Stopped => vec![],
        }
    }

    /// RFC 6762 section 8.2: another machine probes for the same name while this one probes; the one with the lexicographically
    /// later data wins, the other takes another name.
    fn tie_break(&mut self, msg: &Message, now: u64) {
        let host = self.host();
        if !msg.questions.iter().any(|q| q.name == host) {
            return;
        }
        let key = |r: &Record| -> (u16, Vec<u8>) {
            (r.rtype, match &r.data {
                Data::A(a) => a.octets().to_vec(),
                Data::Aaaa(a) => a.octets().to_vec(),
                _ => vec![],
            })
        };
        let mut theirs: Vec<_> = msg.authority.iter().filter(|r| r.name == host && matches!(r.data, Data::A(_) | Data::Aaaa(_))).map(key).collect();
        if theirs.is_empty() {
            return;
        }
        let mut ours: Vec<_> = self.addr_records(None).iter().map(key).collect();
        theirs.sort();
        ours.sort();
        if ours < theirs {
            self.rename(now);
        }
    }

    fn on_query(&mut self, msg: &Message, from: SocketAddr, now: u64) -> Vec<Out> {
        let host = self.host();
        let mut answers: Vec<Record> = Vec::new();
        let mut extra: Vec<Record> = Vec::new();
        let mut qu = false;
        let addrs = self.addr_records(None);
        for q in &msg.questions {
            let qn = q.name.as_str();
            let before = answers.len() + extra.len();
            let wants = |t: u16| q.qtype == t || q.qtype == wire::TYPE_ANY;
            if qn == host {
                answers.extend(addrs.iter().filter(|r| wants(r.rtype)).cloned());
            } else if qn == "_services._dns-sd._udp.local" {
                if wants(wire::TYPE_PTR) {
                    for s in &self.svcs {
                        answers.push(Record { name: qn.to_string(), rtype: wire::TYPE_PTR, flush: false, ttl: TTL_OTHER, data: Data::Ptr(Self::type_name(&s.kind)) });
                    }
                }
            } else if let Some(s) = self.svcs.iter().find(|s| qn == Self::type_name(&s.kind)).cloned() {
                if wants(wire::TYPE_PTR) {
                    let (p, srv, t) = self.svc_records(&s, None);
                    answers.push(p);
                    extra.extend([srv, t]);
                    extra.extend(addrs.iter().cloned());
                }
            } else if let Some(s) = self.svcs.iter().find(|s| qn == self.instance(&s.kind)).cloned() {
                let (_, srv, t) = self.svc_records(&s, None);
                if wants(wire::TYPE_SRV) {
                    answers.push(srv);
                }
                if wants(wire::TYPE_TXT) {
                    answers.push(t);
                }
                if !answers.is_empty() {
                    extra.extend(addrs.iter().cloned());
                }
            }
            if answers.len() + extra.len() > before {
                qu |= q.unicast;
            }
        }
        // known-answer suppression (RFC 6762 section 7.1): the asker already holds this with more than half its life left
        answers.retain(|a| !msg.answers.iter().any(|k| k.name == a.name && k.rtype == a.rtype && k.data == a.data && k.ttl.saturating_mul(2) >= a.ttl));
        dedup(&mut answers);
        dedup(&mut extra);
        extra.retain(|e| !answers.contains(e));
        if answers.is_empty() {
            return vec![];
        }
        let legacy = from.port() != MDNS_PORT;
        let out = if legacy {
            let cap = |mut r: Record| {
                r.ttl = r.ttl.min(10);
                r.flush = false;
                r
            };
            let m = Message { id: msg.id, flags: 0x8400, questions: msg.questions.iter().map(|q| Question { unicast: false, ..q.clone() }).collect(), answers: answers.into_iter().map(cap).collect(), additional: extra.into_iter().map(cap).collect(), ..Default::default() };
            wire::encode(&m).ok().map(|bytes| Out { dest: Dest::Unicast(from), bytes })
        } else if qu {
            wire::encode(&Message::response(answers, extra)).ok().map(|bytes| Out { dest: Dest::Unicast(from), bytes })
        } else {
            // multicast: the same record is not sent twice within a second (RFC 6762 section 6)
            answers.retain(|a| self.lim.last_multicast.get(&(a.name.clone(), a.rtype)).is_none_or(|t| now.saturating_sub(*t) >= 1000));
            if answers.is_empty() {
                return vec![];
            }
            if self.lim.last_multicast.len() > 256 {
                self.lim.last_multicast.clear();
            }
            for a in &answers {
                self.lim.last_multicast.insert((a.name.clone(), a.rtype), now);
            }
            self.send(&Message::response(answers, extra))
        };
        match out {
            Some(o) if self.global_ok(now) => {
                self.answered += 1;
                vec![o]
            }
            _ => vec![],
        }
    }
}

fn dedup(v: &mut Vec<Record>) {
    let mut seen: Vec<Record> = Vec::new();
    v.retain(|r| {
        if seen.contains(r) {
            false
        } else {
            seen.push(r.clone());
            true
        }
    });
}
