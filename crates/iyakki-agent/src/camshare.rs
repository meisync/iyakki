// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! One camera connection shared by the recorder and the live view (F-CAM-19).
//!
//! Some cameras allow a single connection. The recorder opens it first (a recording runs whether or not anybody looks), so the live view
//! used to find the camera "busy". Here the recorder's connection is a tee: every unit it reads is also handed to whoever is looking, and
//! the live view takes its pictures from that instead of opening a second connection. Nothing is recorded or kept for the viewer: a viewer
//! holds a short queue of what arrived since it last asked; when it falls behind, the queue is dropped and an H.264 viewer waits for the next
//! key frame (a picture decoded from the middle of a group would be broken). A viewer that goes away costs the recorder nothing.
//!
//! No global state: the registry belongs to the hub ([`crate::cameras::Hub::share`]), which both the recorder and the live view already hold.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::camrec::{RecFeed, Unit};
use crate::camrec_format::RecKind;
use crate::cameras::FeedErr;

/// The most units a viewer is behind before its queue is dropped.
pub const QUEUE_MAX: usize = 48;
/// How long a viewer waits for the next unit before it reports that nothing arrived yet.
pub const WAIT: Duration = Duration::from_millis(500);

struct Queue {
    q: Mutex<VecDeque<Unit>>,
    cv: Condvar,
    closed: AtomicBool,
    /// H.264 only: the viewer must start (again) at a key frame.
    need_key: AtomicBool,
}

struct Tee {
    kind: RecKind,
    params: Vec<u8>,
    subs: Mutex<Vec<Arc<Queue>>>,
}

/// The connections the recorder currently holds, by camera id.
#[derive(Default)]
pub struct Share {
    tees: Mutex<HashMap<String, Arc<Tee>>>,
    /// Cameras that a recorder wants to record, with how many recorders (see [`Share::want`]).
    wanted: Mutex<HashMap<String, usize>>,
}

/// Held by a recorder while it wants a camera; dropping it says it does not.
pub struct Want {
    share: Arc<Share>,
    camera: String,
}

impl Drop for Want {
    fn drop(&mut self) {
        let mut w = self.share.wanted.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = w.get_mut(&self.camera) {
            *n -= 1;
            if *n == 0 {
                w.remove(&self.camera);
            }
        }
    }
}

impl Share {
    /// Put a connection the recorder opened in the table; every unit it reads goes to the viewers as well. The returned feed is what the
    /// recorder reads from. When it is dropped (the recording stopped or the camera was lost) the viewers are told and the table entry goes.
    pub fn tee(self: &Arc<Self>, camera: &str, inner: Box<dyn RecFeed>) -> Box<dyn RecFeed> {
        let tee = Arc::new(Tee { kind: inner.kind(), params: inner.params(), subs: Mutex::new(Vec::new()) });
        self.tees.lock().unwrap_or_else(|e| e.into_inner()).insert(camera.to_string(), tee.clone());
        Box::new(TeeFeed { inner, tee, share: self.clone(), camera: camera.to_string() })
    }

    /// A recorder says it wants this camera. While it does, a live view that opens the camera FIRST keeps its connection in the table too
    /// (the other direction of F-CAM-19), so the recorder looks through it instead of opening a second one that a one-connection camera refuses.
    pub fn want(self: &Arc<Self>, camera: &str) -> Want {
        *self.wanted.lock().unwrap_or_else(|e| e.into_inner()).entry(camera.to_string()).or_insert(0) += 1;
        Want { share: self.clone(), camera: camera.to_string() }
    }

    /// Whether a recorder wants this camera now.
    pub fn is_wanted(&self, camera: &str) -> bool {
        self.wanted.lock().unwrap_or_else(|e| e.into_inner()).contains_key(camera)
    }

    /// Whether a connection to this camera is held now (by the recorder, or by the live view that opened it first).
    pub fn held(&self, camera: &str) -> bool {
        self.tees.lock().unwrap_or_else(|e| e.into_inner()).contains_key(camera)
    }

    /// A viewer on the recorder's connection, or `None` when the recorder holds none (the live view then opens its own).
    pub fn subscribe(&self, camera: &str) -> Option<Viewer> {
        let tee = self.tees.lock().unwrap_or_else(|e| e.into_inner()).get(camera)?.clone();
        let q = Arc::new(Queue { q: Mutex::new(VecDeque::new()), cv: Condvar::new(), closed: AtomicBool::new(false), need_key: AtomicBool::new(tee.kind == RecKind::H264) });
        tee.subs.lock().unwrap_or_else(|e| e.into_inner()).push(q.clone());
        Some(Viewer { q, kind: tee.kind, params: tee.params.clone(), sent_params: false })
    }
}

struct TeeFeed {
    inner: Box<dyn RecFeed>,
    tee: Arc<Tee>,
    share: Arc<Share>,
    camera: String,
}

impl TeeFeed {
    fn publish(&self, u: &Unit) {
        let mut subs = self.tee.subs.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|q| !q.closed.load(Ordering::SeqCst) && Arc::strong_count(q) > 1);
        let is_key = matches!(u, Unit::Jpeg(_) | Unit::H264 { key: true, .. });
        for s in subs.iter() {
            if s.need_key.load(Ordering::SeqCst) {
                if !is_key {
                    continue;
                }
                s.need_key.store(false, Ordering::SeqCst);
            }
            let mut q = s.q.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= QUEUE_MAX {
                // behind: drop what it has; Motion-JPEG goes on with the newest picture, H.264 waits for the next key frame
                q.clear();
                if self.tee.kind == RecKind::H264 {
                    s.need_key.store(true, Ordering::SeqCst);
                    if !is_key {
                        continue;
                    }
                    s.need_key.store(false, Ordering::SeqCst);
                }
            }
            q.push_back(u.clone());
            s.cv.notify_one();
        }
    }
}

impl RecFeed for TeeFeed {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        let u = self.inner.next()?;
        self.publish(&u);
        Ok(u)
    }
    fn kind(&self) -> RecKind {
        self.inner.kind()
    }
    fn params(&self) -> Vec<u8> {
        self.inner.params()
    }
}

impl Drop for TeeFeed {
    fn drop(&mut self) {
        let mut t = self.share.tees.lock().unwrap_or_else(|e| e.into_inner());
        // only our own entry (a newer connection of the same camera may have replaced it)
        if t.get(&self.camera).is_some_and(|x| Arc::ptr_eq(x, &self.tee)) {
            t.remove(&self.camera);
        }
        for s in self.tee.subs.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            s.closed.store(true, Ordering::SeqCst);
            s.cv.notify_all();
        }
    }
}

/// One viewer of the recorder's connection. Dropping it leaves the connection alone.
pub struct Viewer {
    q: Arc<Queue>,
    pub kind: RecKind,
    params: Vec<u8>,
    sent_params: bool,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.q.closed.store(true, Ordering::SeqCst);
    }
}

impl Viewer {
    /// The next unit. `Timeout` when nothing arrived within [`WAIT`]; `Lost` when the recorder let go of the camera. H.264: the first unit
    /// carries the parameter sets in front of its key frame, so a decoder can start there.
    pub fn next(&mut self) -> Result<Unit, FeedErr> {
        let mut q = self.q.q.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(u) = q.pop_front() {
                return Ok(match u {
                    Unit::H264 { mut annex_b, key } if !self.sent_params && key => {
                        self.sent_params = true;
                        let mut v = self.params.clone();
                        v.append(&mut annex_b);
                        Unit::H264 { annex_b: v, key }
                    }
                    other => other,
                });
            }
            if self.q.closed.load(Ordering::SeqCst) {
                return Err(FeedErr::Lost("the recorder let go of the camera".into()));
            }
            let (g, r) = self.q.cv.wait_timeout(q, WAIT).unwrap_or_else(|e| e.into_inner());
            q = g;
            if r.timed_out() && q.is_empty() {
                return if self.q.closed.load(Ordering::SeqCst) { Err(FeedErr::Lost("the recorder let go of the camera".into())) } else { Err(FeedErr::Timeout) };
            }
        }
    }
}

/// A viewer is also a feed, so the recorder can read the live view's connection like one of its own. When the owner lets go the viewer
/// says `Lost` and the recorder opens its own connection after its usual pause.
impl RecFeed for Viewer {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        Viewer::next(self)
    }
    fn kind(&self) -> RecKind {
        self.kind
    }
    fn params(&self) -> Vec<u8> {
        self.params.clone()
    }
}

/// The owner of a connection that reads it itself (the live view that opened the camera): the H.264 parameter sets go in front of its
/// first key frame, as they do for a viewer, so a decoder can start there.
pub struct ParamsFirst {
    inner: Box<dyn RecFeed>,
    sent: bool,
}

impl ParamsFirst {
    pub fn new(inner: Box<dyn RecFeed>) -> ParamsFirst {
        ParamsFirst { inner, sent: false }
    }
}

impl RecFeed for ParamsFirst {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        Ok(match self.inner.next()? {
            Unit::H264 { mut annex_b, key } if !self.sent && key => {
                self.sent = true;
                let mut v = self.inner.params();
                v.append(&mut annex_b);
                Unit::H264 { annex_b: v, key }
            }
            other => other,
        })
    }
    fn kind(&self) -> RecKind {
        self.inner.kind()
    }
    fn params(&self) -> Vec<u8> {
        self.inner.params()
    }
}

/// Annex-B bytes (start codes `00 00 01` or `00 00 00 01`) to the NAL units of one access unit.
pub fn nals_from_annex_b(b: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (n, &(s, data)) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(b.len(), |x| x.0);
        // the fourth byte of a four byte start code belongs to the code, not to the unit before it
        if n + 1 < starts.len() && end > data && b[end - 1] == 0 {
            end -= 1;
        }
        let _ = s;
        if end > data {
            out.push(b[data..end].to_vec());
        }
    }
    out
}

#[cfg(test)]
#[path = "camshare_tests.rs"]
mod tests;
