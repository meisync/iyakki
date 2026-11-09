//! In-house motion detection on decoded pictures. Pure code (no files, no network, no serde, no clock of its own: the caller gives the
//! time of every picture), so the same bytes and times always give the same answers and it can be tested and measured alone.
//!
//! How it works, in the order it happens to each picture:
//!
//! 1. The picture is shrunk to a small grid of average brightness (64 cells wide, the same shape as the picture).
//! 2. A background model, one number per cell, is kept as a running average. Cells that look still learn it fast (rate 0.05); cells
//!    that keep looking changed learn it slowly at first (0.004) and faster the longer they stay changed (up to about 0.054 after
//!    400 pictures), so a parked car or a moved chair is absorbed after about a minute and a half instead of alarming for ever.
//! 3. What the lighting did to the whole picture is taken out first: the median brightness ratio and then the median difference over
//!    all watched cells. A cloud, a lamp, a flicker or the camera's own exposure change moves nearly every cell the same way and is not
//!    motion; an object that covers less than half the picture cannot hide in the median.
//! 4. Each cell has its own noise estimate (a running variance of what it did while quiet). A cell is "changed" when it differs from
//!    the model by more than the larger of a fixed floor (18 grey levels at sensitivity 5, 1.4 times that at 1, 0.6 times at 10) and
//!    three times its own noise. Noisy cells need a bigger change.
//! 5. Isolated changed cells (fewer than two changed neighbours) are dropped, and the rest are joined into blobs. A blob must cover
//!    at least 0.4 % of the watched area (and 3 cells) to count.
//! 6. A change that touches more than 60 % of the watched cells, or a huge jump of the lighting, is a sudden global change (lights on
//!    or off, the camera switching to night mode): the model is replaced by the new picture, nothing is reported, and the next 5
//!    pictures only settle the model.
//! 7. Debounce and cool-down: motion must be seen in 2 pictures in a row to start an event; an event ends after 4 seconds without
//!    motion; a new event can start 2 seconds after the last one ended; one event never lasts more than 10 minutes.
//!
//! Zones: with at least one include zone only those areas are watched (each with its own sensitivity); exclude zones are never watched.
//! Privacy masks are not handled here: the caller blackens them before the picture gets here ([`super::geom::blacken`]).
//!
//! Limits, honestly: this finds *change*, not people. Wind in a tree, rain, insects at night, a reflection or a flashing screen can
//! still alarm, and a slow walker in a dark corner can be missed. A person standing in front of the lens that fills more than 60 % of
//! the watched area looks like a global change. False alarm rates on real scenes are not known.

use super::geom::{self, Pt, Zone};

/// The name used when no include zone is drawn.
pub const WHOLE: &str = "whole picture";

const BASE_THRESHOLD: f32 = 18.0;
const MAX_THRESHOLD: f32 = 90.0;
const GLOBAL_FRACTION: f32 = 0.6;
const RATE_QUIET: f32 = 0.05;
const RATE_MOVING: f32 = 0.004;
const AGE_FULL: f32 = 400.0;

#[derive(Clone, Debug)]
pub struct Params {
    /// The camera's own sensitivity, 1 (least) to 10 (most); 5 is the default.
    pub sensitivity: u8,
    pub grid_w: usize,
    pub start_frames: u32,
    pub quiet_ms: u64,
    pub cooldown_ms: u64,
    pub max_event_ms: u64,
    /// Smallest blob, as a fraction of the watched cells.
    pub min_blob: f32,
    pub settle_frames: u32,
}

impl Default for Params {
    fn default() -> Params {
        Params { sensitivity: 5, grid_w: 64, start_frames: 2, quiet_ms: 4000, cooldown_ms: 2000, max_event_ms: 600_000, min_blob: 0.004, settle_frames: 5 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MotionEvent {
    /// Motion began. `ts_ms` is the time of the first picture with motion; `bbox` is `[left, top, right, bottom]` of the largest
    /// changed area, 0.0 to 1.0.
    Start { ts_ms: u64, zone: String, score: u8, bbox: [f32; 4] },
    /// Motion ended. `end_ms` is the time of the last picture with motion; `peak` is the highest score.
    End { start_ms: u64, end_ms: u64, zone: String, peak: u8 },
}

enum State {
    Idle,
    Active { start_ms: u64, last_motion: u64, zone: String, peak: u8 },
}

struct Hit {
    zone: String,
    score: u8,
    bbox: [f32; 4],
}

pub struct Detector {
    params: Params,
    zones: Vec<Zone>,
    dims: (u32, u32),
    gw: usize,
    gh: usize,
    /// Per cell: -1 not watched, `zones.len()` the whole picture, otherwise the zone's index.
    map: Vec<i16>,
    sens: Vec<u8>,
    watched: usize,
    bg: Vec<f32>,
    var: Vec<f32>,
    age: Vec<f32>,
    have_bg: bool,
    settle: u32,
    state: State,
    streak: u32,
    first_ms: u64,
    cooldown_until: u64,
    /// Pictures seen since the model was made (tests, status).
    pub pictures: u64,
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    let (_, m, _) = v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    *m
}

impl Detector {
    pub fn new(params: Params, zones: Vec<Zone>) -> Detector {
        Detector {
            params,
            zones,
            dims: (0, 0),
            gw: 0,
            gh: 0,
            map: vec![],
            sens: vec![],
            watched: 0,
            bg: vec![],
            var: vec![],
            age: vec![],
            have_bg: false,
            settle: 0,
            state: State::Idle,
            streak: 0,
            first_ms: 0,
            cooldown_until: 0,
            pictures: 0,
        }
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Forget the background (new zones, a changed sensitivity, a camera that came back). An event in progress is closed by the
    /// caller, not here.
    pub fn reset(&mut self) {
        self.have_bg = false;
        self.dims = (0, 0);
        self.streak = 0;
    }

    /// Whether an event is in progress.
    pub fn active(&self) -> bool {
        matches!(self.state, State::Active { .. })
    }

    fn rebuild(&mut self, w: u32, h: u32) {
        self.dims = (w, h);
        self.gw = self.params.grid_w.clamp(8, 160);
        self.gh = ((self.gw as f32 * h as f32 / w as f32).round() as usize).clamp(8, 160);
        let (gw, gh) = (self.gw, self.gh);
        let any_include = self.zones.iter().any(|z| z.include);
        self.map = vec![-1; gw * gh];
        self.sens = vec![self.params.sensitivity; gw * gh];
        self.watched = 0;
        for cy in 0..gh {
            for cx in 0..gw {
                let (x, y) = ((cx as f32 + 0.5) / gw as f32, (cy as f32 + 0.5) / gh as f32);
                if self.zones.iter().any(|z| !z.include && geom::contains(&z.points, x, y)) {
                    continue;
                }
                let i = cy * gw + cx;
                if any_include {
                    if let Some(zi) = self.zones.iter().position(|z| z.include && geom::contains(&z.points, x, y)) {
                        self.map[i] = zi as i16;
                        self.sens[i] = self.zones[zi].sensitivity.clamp(1, 10);
                        self.watched += 1;
                    }
                } else {
                    self.map[i] = self.zones.len() as i16;
                    self.watched += 1;
                }
            }
        }
        self.bg = vec![0.0; gw * gh];
        self.var = vec![4.0; gw * gh];
        self.age = vec![0.0; gw * gh];
        self.have_bg = false;
    }

    fn zone_name(&self, idx: i16) -> String {
        self.zones.get(idx as usize).map(|z| z.name.clone()).unwrap_or_else(|| WHOLE.to_string())
    }

    fn gray(&self, w: u32, h: u32, rgba: &[u8]) -> Vec<f32> {
        let (w, h, gw, gh) = (w as usize, h as usize, self.gw, self.gh);
        let mut out = vec![0.0f32; gw * gh];
        for gy in 0..gh {
            let (y0, y1) = (gy * h / gh, ((gy + 1) * h / gh).max(gy * h / gh + 1).min(h));
            for gx in 0..gw {
                let (x0, x1) = (gx * w / gw, ((gx + 1) * w / gw).max(gx * w / gw + 1).min(w));
                let mut sum = 0u32;
                for y in y0..y1 {
                    let row = &rgba[(y * w + x0) * 4..(y * w + x1) * 4];
                    for px in row.chunks_exact(4) {
                        sum += (77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8;
                    }
                }
                out[gy * gw + gx] = sum as f32 / ((y1 - y0) * (x1 - x0)) as f32;
            }
        }
        out
    }

    /// Look at one picture (straight RGBA, `w * h * 4` bytes) taken at `now_ms`. Returns the events this picture caused (usually none).
    pub fn push(&mut self, now_ms: u64, w: u32, h: u32, rgba: &[u8]) -> Vec<MotionEvent> {
        if w < 8 || h < 8 || rgba.len() < w as usize * h as usize * 4 {
            return vec![];
        }
        if self.dims != (w, h) {
            self.rebuild(w, h);
        }
        if self.watched == 0 {
            return self.advance(now_ms, None);
        }
        self.pictures += 1;
        let cur = self.gray(w, h, rgba);
        if !self.have_bg {
            self.bg = cur;
            self.have_bg = true;
            self.settle = self.params.settle_frames;
            return self.advance(now_ms, None);
        }
        let n = self.gw * self.gh;
        // 3. the lighting of the whole picture: a ratio and then a shift, both medians over the watched cells
        let mut ratios: Vec<f32> = (0..n).filter(|&i| self.map[i] >= 0 && self.bg[i] >= 24.0).map(|i| (cur[i] + 1.0) / (self.bg[i] + 1.0)).collect();
        let g = if ratios.len() * 4 >= self.watched { median(&mut ratios).clamp(0.2, 5.0) } else { 1.0 };
        let mut diffs: Vec<f32> = (0..n).filter(|&i| self.map[i] >= 0).map(|i| cur[i] - self.bg[i] * g).collect();
        let shift = median(&mut diffs);
        let resid: Vec<f32> = (0..n).map(|i| if self.map[i] >= 0 { cur[i] - self.bg[i] * g - shift } else { 0.0 }).collect();
        // 4. changed cells
        let mut thr = vec![f32::MAX; n];
        let mut hit = vec![false; n];
        let mut n_hit = 0usize;
        for i in 0..n {
            if self.map[i] < 0 {
                continue;
            }
            let factor = 1.5 - 0.1 * self.sens[i] as f32;
            thr[i] = (BASE_THRESHOLD * factor).max(3.0 * self.var[i].sqrt()).min(MAX_THRESHOLD);
            if resid[i].abs() > thr[i] {
                hit[i] = true;
                n_hit += 1;
            }
        }
        // 6. a sudden global change: take the new picture as the model and report nothing
        if n_hit as f32 > GLOBAL_FRACTION * self.watched as f32 || (g - 1.0).abs() > 0.6 || shift.abs() > 70.0 {
            self.bg = cur;
            self.age.iter_mut().for_each(|a| *a = 0.0);
            self.settle = self.params.settle_frames;
            return self.advance(now_ms, None);
        }
        if self.settle > 0 {
            self.settle -= 1;
            for i in 0..n {
                if self.map[i] >= 0 {
                    self.bg[i] += 0.5 * (cur[i] - self.bg[i]);
                }
            }
            return self.advance(now_ms, None);
        }
        // 5. drop lone cells, join the rest into blobs
        let (gw, gh) = (self.gw, self.gh);
        let mut keep = vec![false; n];
        for cy in 0..gh {
            for cx in 0..gw {
                let i = cy * gw + cx;
                if !hit[i] {
                    continue;
                }
                let mut nb = 0;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if (dx, dy) == (0, 0) {
                            continue;
                        }
                        let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                        if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh && hit[y as usize * gw + x as usize] {
                            nb += 1;
                        }
                    }
                }
                keep[i] = nb >= 2;
            }
        }
        let min_cells = ((self.params.min_blob * self.watched as f32).ceil() as usize).max(3);
        let mut seen = vec![false; n];
        let mut best: Option<(usize, [usize; 4], Vec<i16>)> = None;
        let mut total = 0usize;
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..n {
            if !keep[start] || seen[start] {
                continue;
            }
            let (mut cells, mut bb, mut zs) = (0usize, [gw, gh, 0usize, 0usize], Vec::<i16>::new());
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (cx, cy) = (i % gw, i / gw);
                cells += 1;
                bb = [bb[0].min(cx), bb[1].min(cy), bb[2].max(cx + 1), bb[3].max(cy + 1)];
                zs.push(self.map[i]);
                let mut try_push = |x: i32, y: i32| {
                    if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
                        let j = y as usize * gw + x as usize;
                        if keep[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                };
                try_push(cx as i32 - 1, cy as i32);
                try_push(cx as i32 + 1, cy as i32);
                try_push(cx as i32, cy as i32 - 1);
                try_push(cx as i32, cy as i32 + 1);
            }
            if cells >= min_cells {
                total += cells;
                if best.as_ref().is_none_or(|b| cells > b.0) {
                    best = Some((cells, bb, zs));
                }
            }
        }
        let motion = best.map(|(_, bb, zs)| {
            // the zone with most cells in the largest blob
            let mut counts: Vec<(i16, usize)> = Vec::new();
            for z in zs {
                match counts.iter_mut().find(|c| c.0 == z) {
                    Some(c) => c.1 += 1,
                    None => counts.push((z, 1)),
                }
            }
            let zone = counts.into_iter().max_by_key(|c| c.1).map(|c| c.0).unwrap_or(0);
            let score = (100.0 * total as f32 / (0.10 * self.watched as f32)).round().clamp(1.0, 100.0) as u8;
            Hit { zone: self.zone_name(zone), score, bbox: [bb[0] as f32 / gw as f32, bb[1] as f32 / gh as f32, bb[2] as f32 / gw as f32, bb[3] as f32 / gh as f32] }
        });
        // 2. the model learns
        for i in 0..n {
            if self.map[i] < 0 {
                continue;
            }
            let moving = keep[i];
            if moving {
                self.age[i] = (self.age[i] + 1.0).min(AGE_FULL);
            } else {
                self.age[i] = (self.age[i] - 2.0).max(0.0);
                let r = resid[i].clamp(-2.0 * thr[i], 2.0 * thr[i]);
                self.var[i] = (0.98 * self.var[i] + 0.02 * r * r).clamp(1.0, 900.0);
            }
            let rate = if moving { RATE_MOVING + 0.05 * self.age[i] / AGE_FULL } else { RATE_QUIET };
            self.bg[i] += rate * (cur[i] - self.bg[i]);
        }
        self.advance(now_ms, motion)
    }

    /// 7. debounce, quiet time and cool-down.
    fn advance(&mut self, now: u64, hit: Option<Hit>) -> Vec<MotionEvent> {
        let mut out = Vec::new();
        match &mut self.state {
            State::Idle => match hit {
                Some(h) => {
                    if self.streak == 0 {
                        self.first_ms = now;
                    }
                    self.streak += 1;
                    if self.streak >= self.params.start_frames && now >= self.cooldown_until {
                        out.push(MotionEvent::Start { ts_ms: self.first_ms, zone: h.zone.clone(), score: h.score, bbox: h.bbox });
                        self.state = State::Active { start_ms: self.first_ms, last_motion: now, zone: h.zone, peak: h.score };
                    }
                }
                None => self.streak = 0,
            },
            State::Active { start_ms, last_motion, zone, peak } => {
                if let Some(h) = hit {
                    *last_motion = now;
                    *peak = (*peak).max(h.score);
                }
                let too_long = now.saturating_sub(*start_ms) >= self.params.max_event_ms;
                if too_long || now.saturating_sub(*last_motion) >= self.params.quiet_ms {
                    out.push(MotionEvent::End { start_ms: *start_ms, end_ms: *last_motion, zone: zone.clone(), peak: *peak });
                    self.cooldown_until = if too_long { now } else { now + self.params.cooldown_ms };
                    self.state = State::Idle;
                    self.streak = 0;
                }
            }
        }
        out
    }

    /// Close an event in progress now (the camera was lost, watching was switched off).
    pub fn finish(&mut self, now: u64) -> Option<MotionEvent> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Active { start_ms, last_motion, zone, peak } => {
                let _ = now;
                self.streak = 0;
                Some(MotionEvent::End { start_ms, end_ms: last_motion, zone, peak })
            }
            State::Idle => None,
        }
    }
}

/// Outline of a rectangle as a polygon (for tests and for the tile's "whole picture" shortcut).
pub fn rect_points(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Pt> {
    vec![Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic pseudo random stream (no clock, no system random).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
        /// -amp ..= amp
        fn noise(&mut self, amp: i32) -> i32 {
            (self.next() % (2 * amp as u32 + 1)) as i32 - amp
        }
    }

    const W: u32 = 320;
    const H: u32 = 240;
    const STEP: u64 = 200; // 5 pictures a second

    /// A textured scene: a gradient with a coarse check pattern, so cells differ from each other.
    fn scene(gain: f32, add: f32) -> Vec<u8> {
        let mut v = vec![255u8; (W * H * 4) as usize];
        for y in 0..H {
            for x in 0..W {
                let base = 60.0 + 80.0 * (x as f32 / W as f32) + 40.0 * (y as f32 / H as f32) + if ((x / 20) + (y / 20)) % 2 == 0 { 25.0 } else { 0.0 };
                let val = (base * gain + add).clamp(0.0, 255.0) as u8;
                let o = ((y * W + x) * 4) as usize;
                v[o] = val;
                v[o + 1] = val;
                v[o + 2] = val;
            }
        }
        v
    }

    fn fill(img: &mut [u8], x0: u32, y0: u32, w: u32, h: u32, val: u8) {
        for y in y0..(y0 + h).min(H) {
            for x in x0..(x0 + w).min(W) {
                let o = ((y * W + x) * 4) as usize;
                img[o] = val;
                img[o + 1] = val;
                img[o + 2] = val;
            }
        }
    }

    struct Run {
        det: Detector,
        t: u64,
        events: Vec<MotionEvent>,
    }

    impl Run {
        fn new() -> Run {
            Run { det: Detector::new(Params::default(), vec![]), t: 1_000_000, events: vec![] }
        }
        fn with(det: Detector) -> Run {
            Run { det, t: 1_000_000, events: vec![] }
        }
        fn feed(&mut self, img: &[u8]) {
            let e = self.det.push(self.t, W, H, img);
            self.events.extend(e);
            self.t += STEP;
        }
        fn hold(&mut self, img: &[u8], n: usize) {
            for _ in 0..n {
                self.feed(img);
            }
        }
        fn starts(&self) -> usize {
            self.events.iter().filter(|e| matches!(e, MotionEvent::Start { .. })).count()
        }
        fn ends(&self) -> usize {
            self.events.iter().filter(|e| matches!(e, MotionEvent::End { .. })).count()
        }
    }

    #[test]
    fn a_moving_rectangle_gives_one_event_with_a_start_time_a_zone_a_score_and_a_place() {
        let mut r = Run::new();
        let still = scene(1.0, 0.0);
        r.hold(&still, 20);
        assert!(r.events.is_empty(), "a still scene is quiet");
        let t_move = r.t;
        for k in 0..25u32 {
            let mut f = still.clone();
            fill(&mut f, 20 + k * 10, 100, 40, 40, 250);
            r.feed(&f);
        }
        r.hold(&still, 40);
        assert_eq!((r.starts(), r.ends()), (1, 1), "one event: {:?}", r.events);
        match &r.events[0] {
            MotionEvent::Start { ts_ms, zone, score, bbox } => {
                assert!(*ts_ms >= t_move && *ts_ms <= t_move + STEP, "starts with the first picture that moved");
                assert_eq!(zone, WHOLE);
                assert!(*score >= 1 && *score <= 100);
                assert!(bbox[0] < 0.3 && bbox[1] > 0.25 && bbox[3] < 0.8, "the box is around the rectangle: {bbox:?}");
            }
            e => panic!("{e:?}"),
        }
        match &r.events[1] {
            MotionEvent::End { start_ms, end_ms, peak, .. } => {
                assert!(end_ms > start_ms && *peak >= 1);
                assert!(*end_ms >= t_move + 20 * STEP && *end_ms <= t_move + 26 * STEP, "ends at the last picture with motion");
            }
            e => panic!("{e:?}"),
        }
        // and it works again afterwards
        let mut f = still.clone();
        for k in 0..10u32 {
            f = still.clone();
            fill(&mut f, 250 - k * 10, 60, 40, 40, 10);
            r.feed(&f);
        }
        r.hold(&still, 40);
        assert_eq!(r.starts(), 2);
        let _ = f;
    }

    #[test]
    fn slow_lighting_drift_is_not_motion() {
        let mut r = Run::new();
        r.hold(&scene(1.0, 0.0), 15);
        // two minutes at 5 pictures a second: the light grows 60 % brighter and 25 levels up, a little different in every cell
        for k in 0..600 {
            let p = k as f32 / 600.0;
            r.feed(&scene(1.0 + 0.6 * p, 25.0 * p));
        }
        r.hold(&scene(1.6, 25.0), 30);
        assert!(r.events.is_empty(), "{:?}", r.events);
        // and the other way round
        for k in 0..300 {
            let p = 1.0 - k as f32 / 300.0;
            r.feed(&scene(1.0 + 0.6 * p, 25.0 * p));
        }
        assert!(r.events.is_empty(), "{:?}", r.events);
    }

    #[test]
    fn mains_flicker_is_not_motion() {
        let mut r = Run::new();
        r.hold(&scene(1.0, 0.0), 15);
        let (dim, bright) = (scene(0.82, -6.0), scene(1.18, 6.0));
        for k in 0..120 {
            r.feed(if k % 2 == 0 { &dim } else { &bright });
        }
        // a slower pulse too (every third picture)
        for k in 0..120 {
            r.feed(if k % 3 == 0 { &dim } else { &bright });
        }
        assert!(r.events.is_empty(), "{:?}", r.events);
    }

    #[test]
    fn camera_noise_and_hot_pixels_are_not_motion() {
        let mut r = Run::new();
        let still = scene(1.0, 0.0);
        let mut rng = Lcg(7);
        for _ in 0..120 {
            let mut f = still.clone();
            for px in f.chunks_exact_mut(4) {
                for c in px.iter_mut().take(3) {
                    *c = (*c as i32 + rng.noise(30)).clamp(0, 255) as u8;
                }
            }
            // a few hot pixels and one small bright speck
            for _ in 0..30 {
                let (x, y) = (rng.next() % W, rng.next() % H);
                fill(&mut f, x, y, 1, 1, 255);
            }
            r.feed(&f);
        }
        assert!(r.events.is_empty(), "{:?}", r.events);
    }

    #[test]
    fn a_sudden_global_change_is_not_motion_and_detection_recovers() {
        let mut r = Run::new();
        let still = scene(1.0, 0.0);
        r.hold(&still, 20);
        // lights on: much brighter at once; later lights off: much darker; then a night-mode like change of look
        r.hold(&scene(1.0, 90.0), 40);
        r.hold(&scene(0.3, 0.0), 40);
        r.hold(&scene(1.0, 0.0), 40);
        assert!(r.events.is_empty(), "{:?}", r.events);
        // a person walking in is still seen afterwards
        for k in 0..20u32 {
            let mut f = still.clone();
            fill(&mut f, 30 + k * 10, 90, 50, 70, 250);
            r.feed(&f);
        }
        r.hold(&still, 40);
        assert_eq!(r.starts(), 1, "{:?}", r.events);
    }

    #[test]
    fn a_speck_a_single_blink_and_a_stationary_change_are_handled() {
        let still = scene(1.0, 0.0);
        // far too small
        let mut r = Run::new();
        r.hold(&still, 20);
        for k in 0..30u32 {
            let mut f = still.clone();
            fill(&mut f, 50 + k * 4, 100, 3, 3, 255);
            r.feed(&f);
        }
        assert!(r.events.is_empty(), "a 3 pixel speck is below the minimum size");
        // one picture of motion is a blink, two in a row start an event
        let mut r = Run::new();
        r.hold(&still, 20);
        let mut f = still.clone();
        fill(&mut f, 100, 80, 60, 60, 255);
        r.feed(&f);
        r.hold(&still, 30);
        assert!(r.events.is_empty(), "debounce: one picture is not an event");
        let mut r = Run::new();
        r.hold(&still, 20);
        r.feed(&f);
        let mut f2 = still.clone();
        fill(&mut f2, 110, 80, 60, 60, 255);
        r.feed(&f2);
        r.hold(&still, 30);
        assert_eq!((r.starts(), r.ends()), (1, 1));
        // something that arrives and stays is one event, then the model absorbs it and the event ends by itself
        let mut r = Run::new();
        r.hold(&still, 20);
        let mut parked = still.clone();
        fill(&mut parked, 120, 100, 80, 60, 250);
        r.hold(&parked, 700);
        assert_eq!(r.starts(), 1, "{:?}", r.events);
        assert_eq!(r.ends(), 1, "a parked object stops being motion after a while: {:?}", r.events);
        assert!(!r.det.active());
    }

    #[test]
    fn events_are_joined_by_the_quiet_time_and_separated_by_a_long_gap() {
        let still = scene(1.0, 0.0);
        let burst = |r: &mut Run, n: u32| {
            for k in 0..n {
                let mut f = still.clone();
                fill(&mut f, 20 + k * 12, 100, 40, 40, 250);
                r.feed(&f);
            }
        };
        // 2 seconds of calm between two walks: still one event (quiet time is 4 s)
        let mut r = Run::new();
        r.hold(&still, 20);
        burst(&mut r, 10);
        r.hold(&still, 10);
        burst(&mut r, 10);
        r.hold(&still, 40);
        assert_eq!((r.starts(), r.ends()), (1, 1), "{:?}", r.events);
        // 8 seconds apart: two events
        let mut r = Run::new();
        r.hold(&still, 20);
        burst(&mut r, 10);
        r.hold(&still, 40);
        burst(&mut r, 10);
        r.hold(&still, 40);
        assert_eq!((r.starts(), r.ends()), (2, 2), "{:?}", r.events);
        // an endless walker is cut into events of at most max_event_ms
        let mut r = Run::with(Detector::new(Params { max_event_ms: 5000, ..Params::default() }, vec![]));
        r.hold(&still, 20);
        for k in 0..100u32 {
            let mut f = still.clone();
            fill(&mut f, 20 + (k % 20) * 12, 100, 40, 40, if k % 2 == 0 { 250 } else { 5 });
            r.feed(&f);
        }
        assert!(r.starts() >= 3 && r.ends() >= 3, "{:?}", r.events);
    }

    #[test]
    fn zones_decide_where_motion_counts() {
        let still = scene(1.0, 0.0);
        let left = Zone { name: "Driveway".into(), include: true, sensitivity: 5, points: rect_points(0.0, 0.0, 0.5, 1.0) };
        let walk = |r: &mut Run, x0: u32| {
            for k in 0..14u32 {
                let mut f = still.clone();
                fill(&mut f, x0 + k * 4, 100, 40, 40, 250);
                r.feed(&f);
            }
            r.hold(&still, 40);
        };
        // walking on the right of an include zone on the left: nothing
        let mut r = Run::with(Detector::new(Params::default(), vec![left.clone()]));
        r.hold(&still, 20);
        walk(&mut r, 200);
        assert!(r.events.is_empty(), "{:?}", r.events);
        // walking inside it: an event that names the zone
        walk(&mut r, 10);
        match r.events.first() {
            Some(MotionEvent::Start { zone, .. }) => assert_eq!(zone, "Driveway"),
            e => panic!("{e:?}"),
        }
        // an exclude zone over the path silences it (a tree in the wind), the rest of the picture still works
        let tree = Zone { name: "Tree".into(), include: false, sensitivity: 5, points: rect_points(0.0, 0.3, 0.6, 0.8) };
        let mut r = Run::with(Detector::new(Params::default(), vec![tree]));
        r.hold(&still, 20);
        walk(&mut r, 10);
        assert!(r.events.is_empty(), "{:?}", r.events);
        let mut f = still.clone();
        for k in 0..12u32 {
            f = still.clone();
            fill(&mut f, 100 + k * 8, 10, 50, 40, 250); // above the tree
            r.feed(&f);
        }
        r.hold(&still, 40);
        let _ = f;
        assert_eq!(r.starts(), 1, "{:?}", r.events);
    }

    #[test]
    fn each_zone_has_its_own_sensitivity() {
        let still = scene(1.0, 0.0);
        // a faint change: 22 grey levels over the background in a 70 x 70 patch
        let faint = |r: &mut Run| {
            for k in 0..12u32 {
                let mut f = still.clone();
                for y in 80..150u32 {
                    for x in (30 + k * 6)..(100 + k * 6) {
                        let o = ((y * W + x) * 4) as usize;
                        for c in 0..3 {
                            f[o + c] = f[o + c].saturating_add(22);
                        }
                    }
                }
                r.feed(&f);
            }
            r.hold(&still, 40);
        };
        let zone = |s: u8| Zone { name: "Door".into(), include: true, sensitivity: s, points: rect_points(0.0, 0.0, 1.0, 1.0) };
        let mut low = Run::with(Detector::new(Params::default(), vec![zone(1)]));
        low.hold(&still, 20);
        faint(&mut low);
        assert_eq!(low.starts(), 0, "sensitivity 1 ignores a faint change");
        let mut high = Run::with(Detector::new(Params::default(), vec![zone(9)]));
        high.hold(&still, 20);
        faint(&mut high);
        assert_eq!(high.starts(), 1, "sensitivity 9 sees it: {:?}", high.events);
    }

    #[test]
    fn bad_input_changes_nothing_and_finishing_closes_an_event() {
        let mut r = Run::new();
        assert!(r.det.push(1, 0, 0, &[]).is_empty());
        assert!(r.det.push(1, 320, 240, &[0u8; 10]).is_empty(), "a short buffer is ignored");
        let still = scene(1.0, 0.0);
        r.hold(&still, 20);
        for k in 0..8u32 {
            let mut f = still.clone();
            fill(&mut f, 20 + k * 12, 100, 40, 40, 250);
            r.feed(&f);
        }
        assert!(r.det.active());
        assert!(matches!(r.det.finish(r.t), Some(MotionEvent::End { .. })));
        assert!(r.det.finish(r.t).is_none() && !r.det.active());
    }
}
