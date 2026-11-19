//! Which peer runs the next task (F-NET-04). Pure: no network and no clock of its own (the caller passes milliseconds), so every rule is
//! tested without a swarm. Used by `Node::submit_scheduled`.
//!
//! * A peer takes at most `max_in_flight` tasks at once; with every candidate full the answer is "busy", not a queue that grows.
//! * A peer that failed at the transport (the request did not get an answer) is left alone for a while that doubles with each failure in
//!   a row (5 s, 10 s, ... at most 60 s) and is forgiven by its next success. A task that ran and failed on its own (fuel, a trap, a
//!   refused module) is the task's failure, not the peer's: it is not retried elsewhere and does not put the peer on hold.
//! * Policies: `LeastLoaded` (fewest tasks in flight, then the faster one, a peer never tried counts as fastest so new peers are used),
//!   `RoundRobin` (the peer chosen longest ago), `Fastest` (lowest smoothed answer time, untried peers first).

use std::collections::HashMap;
use std::hash::Hash;

pub const BASE_HOLD_MS: u64 = 5_000;
pub const MAX_HOLD_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Policy {
    #[default]
    LeastLoaded,
    RoundRobin,
    Fastest,
}

impl Policy {
    pub fn parse(s: &str) -> Option<Policy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "least-loaded" | "least_loaded" | "leastloaded" => Some(Policy::LeastLoaded),
            "round-robin" | "round_robin" | "roundrobin" => Some(Policy::RoundRobin),
            "fastest" => Some(Policy::Fastest),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Policy::LeastLoaded => "least-loaded",
            Policy::RoundRobin => "round-robin",
            Policy::Fastest => "fastest",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerStat {
    pub in_flight: u32,
    pub done: u64,
    pub failed: u64,
    pub failures_in_a_row: u32,
    /// Smoothed answer time in ms (weight 1/4 on the newest), `None` until one task came back.
    pub avg_ms: Option<f64>,
    pub held_until_ms: u64,
    /// A counter of the order peers were chosen in (for round robin).
    pub last_pick: u64,
}

/// Why no peer could be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPick {
    /// There are no candidate peers at all (none connected, or every one already tried for this task).
    NoPeers,
    /// Candidates exist but each is on hold after failures.
    AllHeld,
    /// Candidates exist but each already has `max_in_flight` tasks.
    AllBusy,
}

pub struct Scheduler<K: Clone + Eq + Hash> {
    pub policy: Policy,
    pub max_in_flight: u32,
    stats: HashMap<K, PeerStat>,
    picks: u64,
}

impl<K: Clone + Eq + Hash + Ord> Scheduler<K> {
    pub fn new(policy: Policy, max_in_flight: u32) -> Scheduler<K> {
        Scheduler { policy, max_in_flight: max_in_flight.max(1), stats: HashMap::new(), picks: 0 }
    }

    pub fn stat(&self, k: &K) -> PeerStat {
        self.stats.get(k).cloned().unwrap_or_default()
    }

    /// Everything known, in a stable order.
    pub fn all(&self) -> Vec<(K, PeerStat)> {
        let mut v: Vec<_> = self.stats.iter().map(|(k, s)| (k.clone(), s.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Choose among `peers`, leaving out `tried`. Counts the task as started on the chosen peer (call [`Scheduler::finished`] when it ends).
    pub fn pick(&mut self, peers: &[K], tried: &[K], now_ms: u64) -> Result<K, NoPick> {
        let mut cands: Vec<&K> = peers.iter().filter(|p| !tried.contains(p)).collect();
        cands.sort();
        cands.dedup();
        if cands.is_empty() {
            return Err(NoPick::NoPeers);
        }
        let open: Vec<&K> = cands.iter().copied().filter(|p| self.stat(p).held_until_ms <= now_ms).collect();
        if open.is_empty() {
            return Err(NoPick::AllHeld);
        }
        let free: Vec<&K> = open.iter().copied().filter(|p| self.stat(p).in_flight < self.max_in_flight).collect();
        if free.is_empty() {
            return Err(NoPick::AllBusy);
        }
        // a peer never heard from counts as the fastest, so new peers join in at once
        let speed = |s: &PeerStat| s.avg_ms.unwrap_or(-1.0);
        let key = |p: &K| {
            let s = self.stat(p);
            match self.policy {
                Policy::LeastLoaded => (s.in_flight as u64, speed(&s), s.last_pick),
                Policy::RoundRobin => (0, 0.0, s.last_pick),
                Policy::Fastest => (0, speed(&s), s.last_pick),
            }
        };
        let best = free.into_iter().min_by(|a, b| {
            let (x, y) = (key(a), key(b));
            x.0.cmp(&y.0).then(x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal)).then(x.2.cmp(&y.2)).then(a.cmp(b))
        });
        let chosen = best.cloned().ok_or(NoPick::NoPeers)?;
        self.picks += 1;
        let s = self.stats.entry(chosen.clone()).or_default();
        s.in_flight += 1;
        s.last_pick = self.picks;
        Ok(chosen)
    }

    /// The task on `peer` ended. `answered` is whether an answer came back (even a failed task); `elapsed_ms` is how long it took.
    pub fn finished(&mut self, peer: &K, answered: bool, elapsed_ms: u64, now_ms: u64) {
        let s = self.stats.entry(peer.clone()).or_default();
        s.in_flight = s.in_flight.saturating_sub(1);
        if answered {
            s.done += 1;
            s.failures_in_a_row = 0;
            s.held_until_ms = 0;
            let e = elapsed_ms as f64;
            s.avg_ms = Some(s.avg_ms.map_or(e, |a| a * 0.75 + e * 0.25));
        } else {
            s.failed += 1;
            s.failures_in_a_row = s.failures_in_a_row.saturating_add(1);
            let hold = BASE_HOLD_MS.saturating_mul(1u64 << (s.failures_in_a_row - 1).min(10)).min(MAX_HOLD_MS);
            s.held_until_ms = now_ms.saturating_add(hold);
        }
    }

    /// Forget peers that are no longer connected and have nothing in flight (so the table does not grow for ever).
    pub fn forget_gone(&mut self, connected: &[K]) {
        self.stats.retain(|k, s| s.in_flight > 0 || connected.contains(k));
    }
}

/// Run one task with up to `attempts` peers: pick, send, and on a transport failure pick another one. `send` is the real request in the
/// node and a fake in tests. Returns the peer that answered and the answer, or why nobody could.
pub async fn run_scheduled<K, T, E, F, Fut>(sched: &std::sync::Mutex<Scheduler<K>>, peers: Vec<K>, attempts: usize, now: impl Fn() -> u64, mut send: F) -> Result<(K, T), RunError<E>>
where
    K: Clone + Eq + Hash + Ord,
    F: FnMut(K) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut tried: Vec<K> = Vec::new();
    let mut last: Option<E> = None;
    for _ in 0..attempts.max(1) {
        let pick = sched.lock().unwrap_or_else(|e| e.into_inner()).pick(&peers, &tried, now());
        let peer = match pick {
            Ok(p) => p,
            Err(why) => return Err(match last {
                Some(e) => RunError::Failed(e),
                None => RunError::NoPick(why),
            }),
        };
        let t0 = now();
        let r = send(peer.clone()).await;
        let t1 = now();
        sched.lock().unwrap_or_else(|e| e.into_inner()).finished(&peer, r.is_ok(), t1.saturating_sub(t0), t1);
        match r {
            Ok(v) => return Ok((peer, v)),
            Err(e) => {
                tried.push(peer);
                last = Some(e);
            }
        }
    }
    Err(RunError::Failed(last.expect("at least one attempt was made")))
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunError<E> {
    NoPick(NoPick),
    /// Every attempt failed at the transport; the last error.
    Failed(E),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn s(policy: Policy) -> Scheduler<u32> {
        Scheduler::new(policy, 2)
    }

    #[test]
    fn least_loaded_spreads_tasks_and_prefers_peers_never_tried() {
        let mut sc = s(Policy::LeastLoaded);
        let peers = [1, 2, 3];
        let got: Vec<u32> = (0..6).map(|_| sc.pick(&peers, &[], 0).unwrap()).collect();
        // each peer takes two before any takes a third; with all full the next pick is busy
        assert_eq!(got.iter().filter(|p| **p == 1).count(), 2);
        assert_eq!(got.iter().filter(|p| **p == 2).count(), 2);
        assert_eq!(got.iter().filter(|p| **p == 3).count(), 2);
        assert_eq!(sc.pick(&peers, &[], 0), Err(NoPick::AllBusy));
        // a finished task frees a slot, and the fast peer is preferred over the slow one at equal load
        sc.finished(&1, true, 10, 0);
        sc.finished(&2, true, 900, 0);
        assert_eq!(sc.pick(&peers, &[], 0), Ok(1));
    }

    #[test]
    fn round_robin_takes_the_peer_chosen_longest_ago() {
        let mut sc = Scheduler::new(Policy::RoundRobin, 10);
        let peers = [7, 8, 9];
        let got: Vec<u32> = (0..6).map(|_| sc.pick(&peers, &[], 0).unwrap()).collect();
        assert_eq!(got, vec![7, 8, 9, 7, 8, 9]);
        // a peer that joins late is used at once
        let got = sc.pick(&[7, 8, 9, 10], &[], 0).unwrap();
        assert_eq!(got, 10);
    }

    #[test]
    fn fastest_uses_the_lowest_smoothed_time_but_tries_new_peers_first() {
        let mut sc = Scheduler::new(Policy::Fastest, 10);
        sc.pick(&[1], &[], 0).unwrap();
        sc.finished(&1, true, 500, 0);
        sc.pick(&[2], &[], 0).unwrap();
        sc.finished(&2, true, 50, 0);
        assert_eq!(sc.pick(&[1, 2, 3], &[], 0), Ok(3), "never tried counts as fastest");
        sc.finished(&3, true, 900, 0);
        assert_eq!(sc.pick(&[1, 2, 3], &[], 0), Ok(2));
        // the average moves by a quarter of the new time
        sc.finished(&2, true, 450, 0);
        let a = sc.stat(&2).avg_ms.unwrap();
        assert!((a - (50.0 * 0.75 + 450.0 * 0.25)).abs() < 1e-9, "{a}");
    }

    #[test]
    fn a_peer_that_failed_is_left_alone_for_longer_each_time_and_forgiven_by_a_success() {
        let mut sc = s(Policy::LeastLoaded);
        sc.pick(&[1], &[], 0).unwrap();
        sc.finished(&1, false, 0, 1_000);
        assert_eq!(sc.pick(&[1], &[], 1_001), Err(NoPick::AllHeld));
        assert_eq!(sc.pick(&[1, 2], &[], 1_001), Ok(2), "another peer is used meanwhile");
        sc.finished(&2, true, 5, 1_001);
        // after the hold (5 s) it may be chosen again; a second failure in a row holds it 10 s, then 20 s, at most 60 s
        sc.pick(&[1], &[], 6_001).unwrap();
        sc.finished(&1, false, 0, 6_001);
        assert_eq!(sc.stat(&1).held_until_ms, 6_001 + 10_000);
        for n in 3..12 {
            sc.pick(&[1], &[], 1_000_000 * n).unwrap();
            sc.finished(&1, false, 0, 1_000_000 * n);
        }
        assert_eq!(sc.stat(&1).held_until_ms, 11_000_000 + MAX_HOLD_MS, "the hold stops growing at a minute");
        // a success clears it
        sc.pick(&[1], &[], 20_000_000).unwrap();
        sc.finished(&1, true, 7, 20_000_000);
        let st = sc.stat(&1);
        assert_eq!((st.failures_in_a_row, st.held_until_ms, st.done), (0, 0, 1));
    }

    #[test]
    fn peers_already_tried_for_this_task_and_gone_peers_are_not_chosen() {
        let mut sc = s(Policy::LeastLoaded);
        assert_eq!(sc.pick(&[], &[], 0), Err(NoPick::NoPeers));
        assert_eq!(sc.pick(&[1, 2], &[1, 2], 0), Err(NoPick::NoPeers));
        assert_eq!(sc.pick(&[1, 2], &[1], 0), Ok(2));
        sc.finished(&2, true, 1, 0);
        sc.forget_gone(&[1]);
        assert_eq!(sc.all().len(), 0, "2 is gone and idle, 1 never had a task");
        assert_eq!(Policy::parse("Round-Robin"), Some(Policy::RoundRobin));
        assert_eq!(Policy::parse("nope"), None);
    }

    #[tokio::test]
    async fn a_task_moves_to_another_peer_after_a_transport_failure_and_gives_up_after_the_attempts() {
        let sc = Mutex::new(Scheduler::new(Policy::LeastLoaded, 4));
        let clock = std::sync::atomic::AtomicU64::new(1_000);
        let now = || clock.fetch_add(10, std::sync::atomic::Ordering::SeqCst);
        // peer 1 is down, peer 2 answers
        let r = run_scheduled(&sc, vec![1u32, 2], 3, now, |p| async move { if p == 1 { Err("down") } else { Ok(p * 10) } }).await;
        assert_eq!(r, Ok((2, 20)));
        let st = sc.lock().unwrap().stat(&1);
        assert_eq!((st.failed, st.in_flight), (1, 0));
        assert!(st.held_until_ms > 1_000, "the failed peer is on hold");
        // every peer down: the last error comes back after trying each once
        let calls = Mutex::new(Vec::new());
        let r: Result<(u32, u32), _> = run_scheduled(&sc, vec![3u32, 4], 5, now, |p| {
            calls.lock().unwrap().push(p);
            async move { Err("down") }
        })
        .await;
        assert_eq!(r, Err(RunError::Failed("down")));
        let mut c = calls.into_inner().unwrap();
        c.sort();
        assert_eq!(c, vec![3, 4], "each peer once, not five times");
        // nobody to ask
        let r: Result<(u32, u32), RunError<&str>> = run_scheduled(&sc, vec![], 3, now, |_p| async move { Ok(1) }).await;
        assert_eq!(r, Err(RunError::NoPick(NoPick::NoPeers)));
    }

    #[tokio::test]
    async fn concurrent_tasks_are_spread_over_the_peers_and_never_exceed_the_limit_on_one() {
        let sc = std::sync::Arc::new(Mutex::new(Scheduler::new(Policy::LeastLoaded, 2)));
        let seen = std::sync::Arc::new(Mutex::new(std::collections::HashMap::<u32, (u32, u32)>::new()));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let (sc, seen) = (sc.clone(), seen.clone());
            handles.push(tokio::spawn(async move {
                run_scheduled(&*sc, vec![1u32, 2, 3], 1, || 0, |p| {
                    let seen = seen.clone();
                    async move {
                        {
                            let mut m = seen.lock().unwrap();
                            let e = m.entry(p).or_insert((0, 0));
                            e.0 += 1;
                            e.1 = e.1.max(e.0);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        seen.lock().unwrap().get_mut(&p).unwrap().0 -= 1;
                        Ok::<_, ()>(p)
                    }
                })
                .await
            }));
        }
        let mut used = std::collections::BTreeSet::new();
        for h in handles {
            used.insert(h.await.unwrap().unwrap().0);
        }
        assert_eq!(used.len(), 3, "all three peers took work: {used:?}");
        assert!(seen.lock().unwrap().values().all(|(_, max)| *max <= 2), "{:?}", seen.lock().unwrap());
    }
}
