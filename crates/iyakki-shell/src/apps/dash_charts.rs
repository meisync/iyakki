//! The chart renderers of the DASH tile and the plain maths under them (ticks, scales, decimation, nearest point). Everything
//! draws with `Gfx`: anti-aliased lines, layered gradient areas, axes with nice ticks, a crosshair with a tooltip. The maths is
//! pure so it is tested without a screen.

use crate::safeclamp::SafeClamp;
use std::collections::BTreeMap;

use iyakki_agent::dash::frame::{format_time, ColType, Frame};
use iyakki_agent::dash::model::{format_value, threshold_color, Options, Unit, Visual};

use crate::fx::{self, with_alpha, CYAN, MAGENTA, PANEL};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, MUTED, R, TEXT};

pub const PALETTE: [[u8; 4]; 8] = [rgb(70, 225, 255), rgb(255, 80, 190), rgb(255, 196, 94), rgb(110, 224, 160), rgb(167, 116, 255), rgb(255, 140, 90), rgb(90, 160, 255), rgb(255, 110, 130)];

pub fn series_color(i: usize) -> [u8; 4] {
    PALETTE[i % PALETTE.len()]
}

// ------------------------------------------------------------------------------------------------------ maths

/// "Nice" tick values covering `[lo, hi]`: steps of 1, 2 or 5 times a power of ten, about `target` of them. The first tick is at
/// or below `lo` and the last at or above `hi`.
pub fn nice_ticks(lo: f64, hi: f64, target: usize) -> Vec<f64> {
    if !lo.is_finite() || !hi.is_finite() {
        return vec![0.0, 1.0];
    }
    let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    let span = if hi - lo < 1e-12 { lo.abs().max(1.0) * 0.5 } else { hi - lo };
    let raw = span / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw * 0.999).unwrap_or(10.0 * mag);
    let (lo2, hi2) = if hi - lo < 1e-12 { (lo - span, hi + span) } else { (lo, hi) };
    let first = (lo2 / step).floor();
    let last = (hi2 / step).ceil();
    let n = ((last - first) as usize).min(200);
    (0..=n).map(|i| ((first + i as f64) * step * 1e9).round() / 1e9).collect()
}

/// Time ticks for `[from, to]` (epoch ms) at a readable step (1s .. 30d), labelled by the step: `12:30`, `12:30:15`, `Oct 15`.
pub fn time_ticks(from: i64, to: i64, target: usize) -> Vec<(i64, String)> {
    const STEPS: [i64; 20] = [1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000, 7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000, 172_800_000, 604_800_000];
    let span = (to - from).max(1);
    let want = span / target.max(1) as i64;
    let step = *STEPS.iter().find(|s| **s >= want).unwrap_or(&2_592_000_000);
    let mut out = vec![];
    let mut t = (from + step - 1).div_euclid(step) * step;
    while t <= to && out.len() < 100 {
        out.push((t, tick_label(t, step, span)));
        t += step;
    }
    out
}

fn tick_label(t: i64, step: i64, span: i64) -> String {
    let full = format_time(t); // 2026-10-15 12:30:15
    if step >= 86_400_000 {
        let (y, m, d) = (&full[..4], &full[5..7], &full[8..10]);
        let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m.parse::<usize>().unwrap_or(1) - 1).min(11)];
        if span > 300 * 86_400_000 { format!("{mon} {y}") } else { format!("{mon} {d}") }
    } else if step >= 60_000 {
        if (t.div_euclid(step) * step).rem_euclid(86_400_000) == 0 && span > 86_400_000 {
            let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(full[5..7].parse::<usize>().unwrap_or(1) - 1).min(11)];
            format!("{mon} {}", &full[8..10])
        } else {
            full[11..16].to_string()
        }
    } else {
        full[11..19].to_string()
    }
}

/// Keep at most about `max` points, always keeping the lowest and highest of every bucket so peaks survive.
pub fn decimate(points: &[(i64, f64)], max: usize) -> Vec<(i64, f64)> {
    let max = max.max(4);
    if points.len() <= max {
        return points.to_vec();
    }
    let buckets = max / 2;
    let per = points.len().div_ceil(buckets);
    let mut out = Vec::with_capacity(max + 2);
    for ch in points.chunks(per) {
        let (mut lo, mut hi) = (ch[0], ch[0]);
        for p in ch {
            if p.1 < lo.1 {
                lo = *p;
            }
            if p.1 > hi.1 {
                hi = *p;
            }
        }
        if lo.0 <= hi.0 {
            out.push(lo);
            if hi != lo {
                out.push(hi);
            }
        } else {
            out.push(hi);
            out.push(lo);
        }
    }
    out
}

/// Index of the point of `points` (sorted by time) closest to time `t`.
pub fn nearest_index(points: &[(i64, f64)], t: i64) -> Option<usize> {
    if points.is_empty() {
        return None;
    }
    let i = points.partition_point(|p| p.0 < t);
    Some(match i {
        0 => 0,
        n if n >= points.len() => points.len() - 1,
        n => {
            if (t - points[n - 1].0).abs() <= (points[n].0 - t).abs() { n - 1 } else { n }
        }
    })
}

/// The mapping between data and pixels.
#[derive(Clone, Copy, Debug)]
pub struct Plot {
    pub r: R,
    pub t0: i64,
    pub t1: i64,
    pub y0: f64,
    pub y1: f64,
    /// Domain of the right-hand axis (equal to the left one when no line uses it).
    pub ry0: f64,
    pub ry1: f64,
}

impl Plot {
    pub fn x(&self, t: i64) -> f32 {
        self.r.x + ((t - self.t0) as f64 / (self.t1 - self.t0).max(1) as f64) as f32 * self.r.w
    }
    pub fn y(&self, v: f64) -> f32 {
        self.r.y + self.r.h - ((v - self.y0) / (self.y1 - self.y0).max(1e-12)) as f32 * self.r.h
    }
    /// Pixel row of a value on the left (axis 0) or right (axis 1) scale.
    pub fn y_on(&self, axis: u8, v: f64) -> f32 {
        if axis == 0 {
            self.y(v)
        } else {
            self.r.y + self.r.h - ((v - self.ry0) / (self.ry1 - self.ry0).max(1e-12)) as f32 * self.r.h
        }
    }
    pub fn t_at(&self, px: f32) -> i64 {
        self.t0 + (((px - self.r.x) / self.r.w.max(1.0)).clamp(0.0, 1.0) as f64 * (self.t1 - self.t0) as f64) as i64
    }
}

// ------------------------------------------------------------------------------------------------------ the data a chart draws

#[derive(Clone, Debug)]
pub struct Line {
    pub name: String,
    pub color: [u8; 4],
    pub points: Vec<(i64, f64)>,
    pub hidden: bool,
    pub axis: u8,
}

/// A frame prepared for drawing; built once when data arrives or the legend changes, not on every frame.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub lines: Vec<Line>,
    pub t0: i64,
    pub t1: i64,
    pub y0: f64,
    pub y1: f64,
    pub ry0: f64,
    pub ry1: f64,
    pub right_unit: Unit,
    /// For pies, tables: label and value pairs.
    pub slices: Vec<(String, f64)>,
}

impl Model {
    pub fn build(f: &Frame, visual: Visual, o: &Options, range: (i64, i64)) -> Model {
        let mut m = Model::default();
        for (i, s) in f.series.iter().enumerate() {
            m.lines.push(Line { name: s.name.clone(), color: series_color(i), points: s.points.clone(), hidden: o.hidden.contains(&s.name), axis: s.axis });
        }
        let vis: Vec<&Line> = m.lines.iter().filter(|l| !l.hidden && !l.points.is_empty()).collect();
        let (mut t0, mut t1) = (i64::MAX, i64::MIN);
        let stacked = visual == Visual::StackedBar || (visual == Visual::Area && false);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        if stacked {
            let mut sums: BTreeMap<i64, f64> = BTreeMap::new();
            for l in &vis {
                for p in &l.points {
                    *sums.entry(p.0).or_default() += p.1.max(0.0);
                }
            }
            for (t, v) in sums {
                t0 = t0.min(t);
                t1 = t1.max(t);
                hi = hi.max(v);
            }
            lo = 0.0;
        } else {
            for l in &vis {
                for p in &l.points {
                    t0 = t0.min(p.0);
                    t1 = t1.max(p.0);
                    lo = lo.min(p.1);
                    hi = hi.max(p.1);
                }
            }
        }
        if vis.is_empty() {
            (lo, hi) = (0.0, 1.0);
            (t0, t1) = range;
        }
        // the x axis is the dashboard's range when it is known, so charts of one dashboard line up
        if range.1 > range.0 {
            (t0, t1) = range;
        } else if t0 >= t1 {
            (t0, t1) = (t0 - 30_000, t1 + 30_000);
        }
        if matches!(visual, Visual::Area | Visual::Bar | Visual::StackedBar) {
            lo = lo.min(0.0);
        }
        if let Some(v) = o.min {
            lo = v;
        }
        if let Some(v) = o.max {
            hi = v;
        }
        if hi <= lo {
            hi = lo + 1.0;
        }
        let ticks = nice_ticks(lo, hi, 5);
        // keep the data range when the person fixed it; otherwise round out to the ticks
        m.y0 = if o.min.is_some() { lo } else { *ticks.first().unwrap_or(&lo) };
        m.y1 = if o.max.is_some() { hi } else { *ticks.last().unwrap_or(&hi) };
        m.t0 = t0;
        m.t1 = t1;
        // the right-hand axis: its own domain from the lines that use it (the left one then covers only the others)
        let right: Vec<&Line> = m.lines.iter().filter(|l| !l.hidden && l.axis == 1 && !l.points.is_empty()).collect();
        (m.ry0, m.ry1) = (m.y0, m.y1);
        if !right.is_empty() && matches!(visual, Visual::Line | Visual::Area) {
            let left_pts: Vec<f64> = m.lines.iter().filter(|l| !l.hidden && l.axis == 0).flat_map(|l| l.points.iter().map(|p| p.1)).collect();
            if !left_pts.is_empty() {
                let (mut a, mut b) = (left_pts.iter().cloned().fold(f64::INFINITY, f64::min), left_pts.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
                if visual == Visual::Area {
                    a = a.min(0.0);
                }
                if let Some(v) = o.min {
                    a = v;
                }
                if let Some(v) = o.max {
                    b = v;
                }
                if b <= a {
                    b = a + 1.0;
                }
                let t = nice_ticks(a, b, 5);
                (m.y0, m.y1) = (t[0], *t.last().unwrap_or(&b));
            }
            let (mut a, mut b) = (right.iter().flat_map(|l| l.points.iter().map(|p| p.1)).fold(f64::INFINITY, f64::min), right.iter().flat_map(|l| l.points.iter().map(|p| p.1)).fold(f64::NEG_INFINITY, f64::max));
            a = a.min(0.0);
            if b <= a {
                b = a + 1.0;
            }
            let t = nice_ticks(a, b, 5);
            (m.ry0, m.ry1) = (t[0], *t.last().unwrap_or(&b));
        }
        m.right_unit = f.meta.right_unit.as_deref().and_then(Unit::parse).unwrap_or(o.right_unit);
        // pie slices: the last value of every series, or the first text and number columns of a table
        if !f.series.is_empty() {
            m.slices = f.series.iter().filter(|s| !o.hidden.contains(&s.name)).filter_map(|s| s.points.last().map(|p| (s.name.clone(), p.1))).filter(|s| s.1 > 0.0).collect();
        } else if let (Some(lc), Some(vc)) = (f.columns.iter().position(|c| c.ty == ColType::Str || c.ty == ColType::Time), f.columns.iter().position(|c| matches!(c.ty, ColType::Int | ColType::Float))) {
            m.slices = (0..f.rows.len()).filter_map(|r| Some((f.cell(r, lc), f.number(r, vc)?))).filter(|s| s.1 > 0.0).collect();
        }
        m
    }

    pub fn plot(&self, r: R) -> Plot {
        Plot { r, t0: self.t0, t1: self.t1, y0: self.y0, y1: self.y1, ry0: self.ry0, ry1: self.ry1 }
    }
}

/// The values of every visible line at the time nearest to `t`: (line index, time, value).
pub fn values_at(m: &Model, t: i64) -> Vec<(usize, i64, f64)> {
    m.lines.iter().enumerate().filter(|(_, l)| !l.hidden).filter_map(|(i, l)| nearest_index(&l.points, t).map(|k| (i, l.points[k].0, l.points[k].1))).collect()
}

// ------------------------------------------------------------------------------------------------------ drawing

/// Click ids the charts register (the app decodes them); all below one million.
pub const PLOT_AREA: usize = 400_000;
pub const LEGEND_TAB: usize = 410_000;

/// What the app needs to draw last, over everything: the tooltip.
#[derive(Clone, Debug)]
pub struct Tooltip {
    pub x: f32,
    pub y: f32,
    pub title: String,
    pub rows: Vec<(String, String, [u8; 4])>,
}

#[derive(Clone, Debug, Default)]
pub struct ChartGeo {
    pub plot: Option<R>,
    pub tooltip: Option<Tooltip>,
}

/// Where the pointer is in a chart and what is being dragged.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    pub hover: Option<(i32, i32)>,
    /// A zoom drag in this chart: x of the press and x now.
    pub drag: Option<(f32, f32)>,
}

pub fn draw_axes(g: &mut Gfx, p: &Plot, s: f32, unit: Unit, decimals: Option<u8>, with_time: bool) {
    let size = 11.0 * s;
    for v in nice_ticks(p.y0, p.y1, 5) {
        if v < p.y0 - 1e-9 || v > p.y1 + 1e-9 {
            continue;
        }
        let y = p.y(v);
        g.rect(p.r.x, y, p.r.w, 1.0, with_alpha(rgb(120, 140, 200), 34));
        let label = format_value(v, unit, decimals);
        let w = g.text_width(size, &label, Face::Mono);
        g.text(p.r.x - w - 6.0 * s, y - size * 0.6, size, MUTED, &label, Face::Mono);
    }
    if with_time {
        let target = (p.r.w / (78.0 * s)).max(2.0) as usize;
        let mut last_end = f32::NEG_INFINITY;
        for (t, label) in time_ticks(p.t0, p.t1, target) {
            let x = p.x(t);
            g.rect(x, p.r.y, 1.0, p.r.h, with_alpha(rgb(120, 140, 200), 20));
            let w = g.text_width(size, &label, Face::Mono);
            if x - w / 2.0 > last_end + 6.0 * s && x + w / 2.0 < p.r.x + p.r.w + 3.0 * s {
                g.text(x - w / 2.0, p.r.y + p.r.h + 4.0 * s, size, MUTED, &label, Face::Mono);
                last_end = x + w / 2.0;
            }
        }
    }
}

fn draw_line_series(g: &mut Gfx, p: &Plot, l: &Line, fill: bool, s: f32, glow: bool) {
    let pts = decimate(&l.points, (p.r.w as usize).max(8));
    if pts.is_empty() {
        return;
    }
    let xy: Vec<(f32, f32)> = pts.iter().map(|q| (p.x(q.0).clamp_safe(p.r.x, p.r.x + p.r.w), p.y_on(l.axis, q.1).clamp_safe(p.r.y - 2.0, p.r.y + p.r.h + 2.0))).collect();
    let base = if l.axis == 0 { p.y(p.y0.max(0.0).min(p.y1)) } else { p.y_on(1, p.ry0.max(0.0).min(p.ry1)) };
    if fill && xy.len() > 1 {
        // a gradient: layers of the same polygon, each clipped at a lower level
        let top = xy.iter().map(|q| q.1).fold(f32::INFINITY, f32::min);
        let h = (base - top).max(1.0);
        for (k, a) in [(0.0f32, 26u8), (0.3, 22), (0.6, 18), (0.85, 14)] {
            let level = top + h * k;
            let mut poly: Vec<(f32, f32)> = xy.iter().map(|q| (q.0, q.1.max(level).min(base))).collect();
            poly.push((xy[xy.len() - 1].0, base));
            poly.push((xy[0].0, base));
            g.polygon(&poly, with_alpha(l.color, a));
        }
    }
    if xy.len() == 1 {
        g.circle(xy[0].0, xy[0].1, 3.0 * s, l.color);
        return;
    }
    if glow {
        for w in xy.windows(2) {
            g.line(w[0].0, w[0].1, w[1].0, w[1].1, 4.5 * s, with_alpha(l.color, 28));
        }
    }
    for w in xy.windows(2) {
        g.line(w[0].0, w[0].1, w[1].0, w[1].1, 1.7 * s, l.color);
    }
}

fn draw_bars(g: &mut Gfx, p: &Plot, m: &Model, stacked: bool, s: f32) {
    let vis: Vec<&Line> = m.lines.iter().filter(|l| !l.hidden && !l.points.is_empty()).collect();
    if vis.is_empty() {
        return;
    }
    // bar slots: the distinct times, thinned to the pixels available
    let mut times: Vec<i64> = vis.iter().flat_map(|l| l.points.iter().map(|q| q.0)).collect();
    times.sort_unstable();
    times.dedup();
    let slot = (p.r.w / times.len().max(1) as f32).max(1.0);
    let step = (3.0 / slot).ceil().max(1.0) as usize;
    let times: Vec<i64> = times.into_iter().step_by(step).collect();
    let slot = p.r.w / times.len().max(1) as f32;
    let n = if stacked { 1 } else { vis.len() };
    let bw = ((slot * 0.8) / n as f32).max(1.0);
    let zero = p.y(0.0f64.max(p.y0));
    for (ti, t) in times.iter().enumerate() {
        let x0 = p.r.x + ti as f32 * slot + slot * 0.1;
        let mut acc = 0.0;
        for (li, l) in vis.iter().enumerate() {
            let Some(k) = nearest_index(&l.points, *t).filter(|k| (l.points[*k].0 - t).abs() < (m.t1 - m.t0).max(1) / times.len().max(1) as i64 * 2 + 1) else { continue };
            let v = l.points[k].1;
            let (ya, yb, x) = if stacked {
                let a = acc;
                acc += v.max(0.0);
                (p.y(acc), p.y(a), x0)
            } else {
                (p.y(v), zero, x0 + li as f32 * bw)
            };
            let (top, h) = (ya.min(yb), (ya - yb).abs().max(1.0));
            g.rect(x, top, (bw - if bw > 3.0 { 1.0 } else { 0.0 }).max(1.0), h, with_alpha(l.color, 210));
        }
    }
    let _ = s;
}

fn draw_heat(g: &mut Gfx, p: &Plot, m: &Model) {
    let vis: Vec<&Line> = m.lines.iter().filter(|l| !l.hidden && !l.points.is_empty()).collect();
    if vis.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for l in &vis {
        for q in &l.points {
            lo = lo.min(q.1);
            hi = hi.max(q.1);
        }
    }
    let rh = p.r.h / vis.len() as f32;
    let cols = (p.r.w / 6.0).max(4.0) as usize;
    let cw = p.r.w / cols as f32;
    for (ri, l) in vis.iter().enumerate() {
        for c in 0..cols {
            let t = p.t0 + ((c as f64 + 0.5) / cols as f64 * (p.t1 - p.t0) as f64) as i64;
            if let Some(k) = nearest_index(&l.points, t) {
                if (l.points[k].0 - t).abs() > (p.t1 - p.t0) / cols as i64 * 3 {
                    continue;
                }
                let f = if hi > lo { ((l.points[k].1 - lo) / (hi - lo)) as f32 } else { 0.5 };
                let col = fx::mix(rgb(24, 40, 110), fx::mix(CYAN, MAGENTA, f), f.clamp(0.0, 1.0));
                g.rect(p.r.x + c as f32 * cw, p.r.y + ri as f32 * rh, cw + 0.5, rh - 1.0, col);
            }
        }
    }
}

/// Draw a time-series style panel body (line, area, bar, stacked bar, heatmap) in `body`. Returns the plot rectangle and the tooltip.
#[allow(clippy::too_many_arguments)]
pub fn draw_xy(g: &mut Gfx, body: R, s: f32, m: &Model, visual: Visual, o: &Options, ptr: Pointer, idx: usize, tick: u64) -> ChartGeo {
    let size = 11.0 * s;
    let leg_rows = if o.legend && m.lines.len() > 1 { legend_rows(g, body.w, s, m) } else { 0 };
    let leg_h = if leg_rows > 0 { leg_rows as f32 * 18.0 * s + 4.0 * s } else { 0.0 };
    let ymax_w = nice_ticks(m.y0, m.y1, 5).iter().map(|v| g.text_width(size, &format_value(*v, o.unit, o.decimals), Face::Mono)).fold(0.0f32, f32::max);
    let has_right = m.lines.iter().any(|l| l.axis == 1 && !l.hidden) && matches!(visual, Visual::Line | Visual::Area);
    let rmax_w = if has_right { nice_ticks(m.ry0, m.ry1, 5).iter().map(|v| g.text_width(size, &format_value(*v, m.right_unit, o.decimals), Face::Mono)).fold(0.0f32, f32::max) + 8.0 * s } else { 0.0 };
    let left = ymax_w + 10.0 * s;
    let plot_r = R::new(body.x + left, body.y + 6.0 * s, (body.w - left - 8.0 * s - rmax_w).max(20.0), (body.h - leg_h - 24.0 * s).max(20.0));
    let p = m.plot(plot_r);
    draw_axes(g, &p, s, o.unit, o.decimals, true);
    if has_right {
        for v in nice_ticks(p.ry0, p.ry1, 5) {
            if v < p.ry0 - 1e-9 || v > p.ry1 + 1e-9 {
                continue;
            }
            let label = format_value(v, m.right_unit, o.decimals);
            g.text(p.r.x + p.r.w + 5.0 * s, p.y_on(1, v) - size * 0.6, size, MUTED, &label, Face::Mono);
        }
    }
    let mut geo = ChartGeo { plot: Some(plot_r), tooltip: None };
    g.clipped(R::new(plot_r.x - 2.0, plot_r.y - 3.0, plot_r.w + 4.0, plot_r.h + 6.0), |g| match visual {
        Visual::Bar | Visual::StackedBar => draw_bars(g, &p, m, visual == Visual::StackedBar, s),
        Visual::Heatmap => draw_heat(g, &p, m),
        _ => {
            for l in m.lines.iter().filter(|l| !l.hidden) {
                draw_line_series(g, &p, l, visual == Visual::Area, s, tick % 1000 < 1000);
            }
        }
    });
    // thresholds as faint lines
    for t in &o.thresholds {
        if t.value > p.y0 && t.value < p.y1 {
            let y = p.y(t.value);
            let c = rgb(t.color[0], t.color[1], t.color[2]);
            g.rect(plot_r.x, y, plot_r.w, 1.0, with_alpha(c, 120));
        }
    }
    // the plot takes presses (zoom drag)
    g.hit(plot_r, Event::Area(PLOT_AREA + idx), "chart");
    // the legend
    if leg_rows > 0 {
        draw_legend(g, R::new(body.x + 4.0 * s, body.y + body.h - leg_h, body.w - 8.0 * s, leg_h), s, m, idx);
    }
    // zoom drag
    if let Some((a, b)) = ptr.drag {
        let (x0, x1) = (a.min(b).clamp_safe(plot_r.x, plot_r.x + plot_r.w), a.max(b).clamp_safe(plot_r.x, plot_r.x + plot_r.w));
        g.rect(x0, plot_r.y, (x1 - x0).max(1.0), plot_r.h, with_alpha(CYAN, 38));
        g.rect(x0, plot_r.y, 1.0, plot_r.h, with_alpha(CYAN, 160));
        g.rect(x1, plot_r.y, 1.0, plot_r.h, with_alpha(CYAN, 160));
    } else if let Some((hx, hy)) = ptr.hover.filter(|(x, y)| plot_r.contains(*x, *y)) {
        let t = p.t_at(hx as f32);
        let vals = values_at(m, t);
        if let Some(first) = vals.first() {
            let x = p.x(first.1);
            g.rect(x, plot_r.y, 1.0, plot_r.h, with_alpha(TEXT, 90));
            for (li, _, v) in &vals {
                if visual.is_timeseries() {
                    let yy = p.y_on(m.lines[*li].axis, *v).clamp_safe(plot_r.y, plot_r.y + plot_r.h);
                    g.circle(x, yy, 3.4 * s, m.lines[*li].color);
                    g.ring(x, yy, 5.5 * s, 1.2, with_alpha(m.lines[*li].color, 120));
                }
            }
            let mut rows: Vec<(String, String, [u8; 4])> = vals.iter().map(|(li, _, v)| (m.lines[*li].name.clone(), format_value(*v, if m.lines[*li].axis == 1 { m.right_unit } else { o.unit }, o.decimals), m.lines[*li].color)).collect();
            rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
            rows.truncate(8);
            geo.tooltip = Some(Tooltip { x: hx as f32, y: hy as f32, title: format_time(first.1), rows });
        }
    }
    geo
}

fn legend_layout(g: &mut Gfx, width: f32, s: f32, m: &Model) -> Vec<(usize, f32, usize, f32)> {
    // (line index, x, row, width)
    let (mut x, mut row, mut out) = (0.0f32, 0usize, vec![]);
    for (i, l) in m.lines.iter().enumerate() {
        let label = g.fit(11.0 * s, &l.name, Face::Sans, 220.0 * s);
        let w = g.text_width(11.0 * s, &label, Face::Sans) + 24.0 * s;
        if x + w > width && x > 0.0 {
            x = 0.0;
            row += 1;
        }
        if row >= 2 {
            break;
        }
        out.push((i, x, row, w));
        x += w + 6.0 * s;
    }
    out
}

fn legend_rows(g: &mut Gfx, width: f32, s: f32, m: &Model) -> usize {
    legend_layout(g, width, s, m).last().map_or(0, |l| l.2 + 1)
}

fn draw_legend(g: &mut Gfx, r: R, s: f32, m: &Model, idx: usize) {
    let lay = legend_layout(g, r.w, s, m);
    for (i, x, row, w) in &lay {
        let l = &m.lines[*i];
        let rect = R::new(r.x + x, r.y + *row as f32 * 18.0 * s, *w, 16.0 * s);
        let hot = g.hovered(rect);
        if hot {
            g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, with_alpha(CYAN, 28));
        }
        let c = if l.hidden { with_alpha(l.color, 70) } else { l.color };
        g.rrect(rect.x + 4.0 * s, rect.y + 5.0 * s, 10.0 * s, 6.0 * s, 3.0 * s, c);
        let label = g.fit(11.0 * s, &l.name, Face::Sans, 220.0 * s);
        g.text(rect.x + 18.0 * s, rect.y + 1.0 * s, 11.0 * s, if l.hidden { DIM } else { MUTED }, &label, Face::Sans);
        g.hit(rect, Event::Tab(LEGEND_TAB + idx * 100 + (*i).min(99)), &format!("legend {}", l.name));
    }
    if lay.len() < m.lines.len() {
        let more = format!("+{}", m.lines.len() - lay.len());
        let w = g.text_width(11.0 * s, &more, Face::Sans);
        g.text(r.x + r.w - w, r.y + 18.0 * s, 11.0 * s, DIM, &more, Face::Sans);
    }
}

/// The tooltip box, drawn last so nothing covers it, kept inside `screen`.
pub fn draw_tooltip(g: &mut Gfx, t: &Tooltip, s: f32, screen: R) {
    let size = 12.0 * s;
    let mut w = g.text_width(size, &t.title, Face::Bold);
    for (n, v, _) in &t.rows {
        let name = g.fit(size, n, Face::Sans, 220.0 * s);
        w = w.max(g.text_width(size, &name, Face::Sans) + g.text_width(size, v, Face::Mono) + 40.0 * s);
    }
    let w = w + 20.0 * s;
    let h = (t.rows.len() as f32 + 1.0) * (size + 5.0 * s) + 12.0 * s;
    let mut x = t.x + 14.0 * s;
    if x + w > screen.x + screen.w {
        x = t.x - w - 14.0 * s;
    }
    let y = (t.y - h / 2.0).clamp_safe(screen.y, (screen.y + screen.h - h).max(screen.y));
    let x = x.max(screen.x);
    g.rrect(x + 2.0, y + 3.0, w, h, 8.0 * s, rgb(2, 4, 12));
    g.rrect(x, y, w, h, 8.0 * s, rgb(14, 19, 40));
    g.rrect_outline(x, y, w, h, 8.0 * s, 1.2, with_alpha(CYAN, 150));
    g.text(x + 10.0 * s, y + 7.0 * s, size, TEXT, &t.title, Face::Bold);
    let mut ry = y + 7.0 * s + size + 5.0 * s;
    for (n, v, c) in &t.rows {
        g.circle(x + 14.0 * s, ry + size * 0.55, 3.5 * s, *c);
        let name = g.fit(size, n, Face::Sans, 220.0 * s);
        g.text(x + 24.0 * s, ry, size, MUTED, &name, Face::Sans);
        let vw = g.text_width(size, v, Face::Mono);
        g.text(x + w - 10.0 * s - vw, ry, size, TEXT, v, Face::Mono);
        ry += size + 5.0 * s;
    }
}

// ------------------------------------------------------------------------------------------------------ other visuals

/// The headline number of a frame: the last point of the first series, or the first number of the first numeric column.
pub fn headline(f: &Frame) -> Option<(f64, String)> {
    if let Some(s) = f.series.iter().find(|s| !s.points.is_empty()) {
        return s.points.last().map(|p| (p.1, s.name.clone()));
    }
    let c = f.columns.iter().position(|c| matches!(c.ty, ColType::Int | ColType::Float))?;
    let v = f.number(0, c)?;
    Some((v, f.columns[c].name.clone()))
}

pub fn draw_stat(g: &mut Gfx, body: R, s: f32, f: &Frame, o: &Options, base: [u8; 3]) {
    let Some((v, label)) = headline(f) else {
        g.text(body.x + 8.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "no data", Face::Sans);
        return;
    };
    let c = threshold_color(&o.thresholds, v, base);
    let col = rgb(c[0], c[1], c[2]);
    let text = format_value(v, o.unit, o.decimals);
    let spark_h = if f.series.first().is_some_and(|s| s.points.len() > 3) { body.h * 0.34 } else { 0.0 };
    let avail = (body.w - 16.0 * s).max(30.0);
    let mut size = ((body.h - spark_h) * 0.55).clamp_safe(16.0 * s, 64.0 * s);
    while size > 12.0 * s && g.text_width(size, &text, Face::Bold) > avail {
        size -= 2.0 * s;
    }
    let tw = g.text_width(size, &text, Face::Bold);
    let ty = body.y + (body.h - spark_h - size * 1.2) / 2.0 + 2.0 * s;
    g.text(body.x + (body.w - tw) / 2.0 - 0.5, ty, size, with_alpha(col, 70), &text, Face::Bold);
    g.text(body.x + (body.w - tw) / 2.0, ty, size, col, &text, Face::Bold);
    let lab = g.fit(11.0 * s, &label, Face::Sans, avail);
    let lw = g.text_width(11.0 * s, &lab, Face::Sans);
    g.text(body.x + (body.w - lw) / 2.0, ty + size * 1.15, 11.0 * s, DIM, &lab, Face::Sans);
    if spark_h > 0.0 {
        let line = &f.series[0];
        let r = R::new(body.x + 8.0 * s, body.y + body.h - spark_h - 2.0 * s, body.w - 16.0 * s, spark_h);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for q in &line.points {
            lo = lo.min(q.1);
            hi = hi.max(q.1);
        }
        if hi <= lo {
            hi = lo + 1.0;
        }
        let p = Plot { r, t0: line.points[0].0, t1: line.points[line.points.len() - 1].0.max(line.points[0].0 + 1), y0: lo, y1: hi, ry0: lo, ry1: hi };
        draw_line_series(g, &p, &Line { name: String::new(), color: col, points: line.points.clone(), hidden: false, axis: 0 }, true, s, false);
    }
}

pub fn draw_gauge(g: &mut Gfx, body: R, s: f32, f: &Frame, o: &Options, base: [u8; 3]) {
    let Some((v, label)) = headline(f) else {
        g.text(body.x + 8.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "no data", Face::Sans);
        return;
    };
    let lo = o.min.unwrap_or(0.0);
    let hi = o.max.unwrap_or(match o.unit {
        Unit::Percent => 100.0,
        Unit::Percent01 => 1.0,
        _ => o.thresholds.iter().map(|t| t.value).fold(v, f64::max) * 1.2,
    });
    let frac = if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let c = threshold_color(&o.thresholds, v, base);
    let col = rgb(c[0], c[1], c[2]);
    let radius = (body.w.min(body.h * 1.5) * 0.36).max(16.0);
    let cx = body.x + body.w / 2.0;
    let cy = body.y + body.h * 0.62;
    let th = (radius * 0.2).max(5.0 * s);
    let (a0, span) = (std::f32::consts::PI * 0.8, std::f32::consts::PI * 1.4);
    let arc = |g: &mut Gfx, from: f32, to: f32, w: f32, c: [u8; 4]| {
        let n = (((to - from) * 40.0).ceil() as usize).max(2);
        let mut prev = (cx + (a0 + span * from).cos() * radius, cy + (a0 + span * from).sin() * radius);
        for k in 1..=n {
            let a = a0 + span * (from + (to - from) * k as f32 / n as f32);
            let p = (cx + a.cos() * radius, cy + a.sin() * radius);
            g.line(prev.0, prev.1, p.0, p.1, w, c);
            prev = p;
        }
    };
    arc(g, 0.0, 1.0, th, rgb(26, 34, 62));
    if frac > 0.0 {
        arc(g, 0.0, frac, th * 2.2, with_alpha(col, 34));
        arc(g, 0.0, frac, th, col);
    }
    let text = format_value(v, o.unit, o.decimals);
    let mut size = (radius * 0.5).max(14.0 * s);
    while size > 11.0 * s && g.text_width(size, &text, Face::Bold) > radius * 1.5 {
        size -= 1.5 * s;
    }
    let tw = g.text_width(size, &text, Face::Bold);
    g.text(cx - tw / 2.0, cy - size * 0.55, size, col, &text, Face::Bold);
    let lab = g.fit(11.0 * s, &label, Face::Sans, body.w - 12.0 * s);
    let lw = g.text_width(11.0 * s, &lab, Face::Sans);
    g.text(cx - lw / 2.0, body.y + body.h - 16.0 * s, 11.0 * s, DIM, &lab, Face::Sans);
}

/// Slice angles: the fraction of each value, summing to 1. Zero or negative values are skipped by the caller.
pub fn fractions(values: &[f64]) -> Vec<f64> {
    let t: f64 = values.iter().filter(|v| **v > 0.0).sum();
    values.iter().map(|v| if t > 0.0 && *v > 0.0 { v / t } else { 0.0 }).collect()
}

pub fn draw_pie(g: &mut Gfx, body: R, s: f32, m: &Model, o: &Options, donut: bool) {
    if m.slices.is_empty() {
        g.text(body.x + 8.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "no data", Face::Sans);
        return;
    }
    let legend_w = (body.w * 0.4).clamp_safe(90.0 * s, 220.0 * s).min(body.w * 0.5);
    let radius = ((body.w - legend_w).min(body.h) / 2.0 - 6.0 * s).max(10.0);
    let (cx, cy) = (body.x + (body.w - legend_w) / 2.0, body.y + body.h / 2.0);
    let vals: Vec<f64> = m.slices.iter().map(|x| x.1).collect();
    let fr = fractions(&vals);
    let mut a = -std::f32::consts::FRAC_PI_2;
    for (i, f) in fr.iter().enumerate() {
        let da = *f as f32 * std::f32::consts::TAU;
        if da <= 0.0 {
            continue;
        }
        let n = ((da * 12.0).ceil() as usize).max(2);
        let inner = if donut { radius * 0.58 } else { 0.0 };
        let mut poly: Vec<(f32, f32)> = (0..=n).map(|k| (cx + (a + da * k as f32 / n as f32).cos() * radius, cy + (a + da * k as f32 / n as f32).sin() * radius)).collect();
        if donut {
            poly.extend((0..=n).rev().map(|k| (cx + (a + da * k as f32 / n as f32).cos() * inner, cy + (a + da * k as f32 / n as f32).sin() * inner)));
        } else {
            poly.push((cx, cy));
        }
        g.polygon(&poly, series_color(i));
        // a thin gap between slices
        g.line(cx + a.cos() * inner, cy + a.sin() * inner, cx + a.cos() * radius, cy + a.sin() * radius, 1.5, PANEL);
        a += da;
    }
    let total: f64 = vals.iter().filter(|v| **v > 0.0).sum();
    if donut {
        let t = format_value(total, o.unit, o.decimals);
        let tw = g.text_width(14.0 * s, &t, Face::Bold);
        g.text(cx - tw / 2.0, cy - 8.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
    }
    let rows = ((body.h - 8.0 * s) / (16.0 * s)).floor().max(1.0) as usize;
    let lx = body.x + body.w - legend_w;
    for (i, ((name, v), f)) in m.slices.iter().zip(&fr).enumerate().take(rows) {
        let y = body.y + 6.0 * s + i as f32 * 16.0 * s;
        g.rrect(lx, y + 3.0 * s, 9.0 * s, 9.0 * s, 3.0 * s, series_color(i));
        let label = g.fit(11.0 * s, &format!("{name}  {}", format_value(*v, o.unit, o.decimals)), Face::Sans, legend_w - 18.0 * s - 40.0 * s);
        g.text(lx + 14.0 * s, y, 11.0 * s, MUTED, &label, Face::Sans);
        let pct = format!("{:.0}%", f * 100.0);
        let pw = g.text_width(11.0 * s, &pct, Face::Mono);
        g.text(lx + legend_w - pw, y, 11.0 * s, DIM, &pct, Face::Mono);
    }
}

// ------------------------------------------------------------------------------------------------------ tables

pub const TABLE_HEAD: usize = 420_000;
pub const TABLE_CELL: usize = 430_000;
pub const TABLE_PAGE: usize = 425_000;

/// Table view state of one panel.
#[derive(Clone, Debug, Default)]
pub struct TableView {
    pub page: usize,
    pub sort: Option<(usize, bool)>,
    pub sel: Option<(usize, usize)>,
}

/// Row order after sorting (numbers by value, text by text, nulls last).
pub fn sorted_rows(f: &Frame, sort: Option<(usize, bool)>) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..f.rows.len()).collect();
    if let Some((c, desc)) = sort {
        let numeric = f.columns.get(c).is_some_and(|k| matches!(k.ty, ColType::Int | ColType::Float));
        idx.sort_by(|&a, &b| {
            let (x, y) = (f.rows[a].get(c), f.rows[b].get(c));
            let null = |v: Option<&serde_json::Value>| v.is_none_or(|v| v.is_null());
            let ord = match (null(x), null(y)) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ if numeric => f.number(a, c).unwrap_or(f64::NAN).partial_cmp(&f.number(b, c).unwrap_or(f64::NAN)).unwrap_or(std::cmp::Ordering::Equal),
                _ => f.cell(a, c).to_lowercase().cmp(&f.cell(b, c).to_lowercase()),
            };
            if desc { ord.reverse() } else { ord }
        });
    }
    idx
}

/// How a cell is shown: times as dates, numbers in the column's unit.
pub fn cell_text(f: &Frame, r: usize, c: usize, o: &Options) -> String {
    let col = &f.columns[c];
    match col.ty {
        ColType::Time => f.rows.get(r).and_then(|row| row.get(c)).and_then(iyakki_agent::dash::frame::value_time_ms).map_or_else(|| f.cell(r, c), format_time),
        ColType::Int | ColType::Float => match (f.number(r, c), o.column_units.get(&col.name)) {
            (Some(v), Some(u)) => format_value(v, *u, o.decimals),
            (Some(v), None) if o.unit != Unit::None => format_value(v, o.unit, o.decimals),
            _ => f.cell(r, c),
        },
        _ => f.cell(r, c),
    }
}

/// Draw a table in `body`; returns the number of pages.
pub fn draw_table(g: &mut Gfx, body: R, s: f32, f: &Frame, o: &Options, view: &mut TableView, idx: usize, accent: [u8; 4]) -> usize {
    let tbl = if f.columns.is_empty() && !f.series.is_empty() { f.series_table() } else { f.clone() };
    let f = &tbl;
    if f.columns.is_empty() {
        g.text(body.x + 8.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "no rows", Face::Sans);
        return 1;
    }
    let size = 12.0 * s;
    let row_h = 20.0 * s;
    let head_h = 24.0 * s;
    let foot = 22.0 * s;
    let page_rows = (((body.h - head_h - foot) / row_h).floor() as usize).max(1);
    let pages = f.rows.len().div_ceil(page_rows).max(1);
    view.page = view.page.min(pages - 1);
    let order = sorted_rows(f, view.sort);
    // column widths from the header and the first rows
    let ncols = f.columns.len();
    let mut widths: Vec<f32> = f.columns.iter().map(|c| g.text_width(size, &c.name, Face::Bold) + 22.0 * s).collect();
    for r in order.iter().take(60) {
        for (c, w) in widths.iter_mut().enumerate().take(ncols) {
            *w = w.max((g.text_width(size, &cell_text(f, *r, c, o), Face::Sans) + 16.0 * s).min(360.0 * s));
        }
    }
    let total: f32 = widths.iter().sum();
    if total < body.w {
        let extra = (body.w - total) / ncols as f32;
        widths.iter_mut().for_each(|w| *w += extra);
    } else {
        let k = body.w / total;
        widths.iter_mut().for_each(|w| *w = (*w * k).max(36.0 * s));
    }
    // header
    let mut x = body.x;
    g.rect(body.x, body.y, body.w, head_h, rgb(20, 27, 52));
    for (c, col) in f.columns.iter().enumerate() {
        let w = widths[c];
        let arrow = match view.sort {
            Some((sc, desc)) if sc == c => if desc { " ▼" } else { " ▲" },
            _ => "",
        };
        let label = g.fit(size, &format!("{}{arrow}", col.name), Face::Bold, w - 10.0 * s);
        let num = matches!(col.ty, ColType::Int | ColType::Float);
        let lw = g.text_width(size, &label, Face::Bold);
        g.text(if num { x + w - lw - 6.0 * s } else { x + 6.0 * s }, body.y + 5.0 * s, size, if view.sort.is_some_and(|v| v.0 == c) { accent } else { MUTED }, &label, Face::Bold);
        g.hit(R::new(x, body.y, w, head_h), Event::Tab(TABLE_HEAD + c.min(999)), &format!("sort {}", col.name));
        x += w;
    }
    // rows
    let first = view.page * page_rows;
    let shown = &order[first.min(order.len())..(first + page_rows).min(order.len())];
    for (i, r) in shown.iter().enumerate() {
        let y = body.y + head_h + i as f32 * row_h;
        if i % 2 == 1 {
            g.rect(body.x, y, body.w, row_h, with_alpha(rgb(120, 140, 220), 12));
        }
        let mut x = body.x;
        for (c, col) in f.columns.iter().enumerate() {
            let w = widths[c];
            let selected = view.sel == Some((*r, c));
            if selected {
                g.rect(x, y, w, row_h, with_alpha(accent, 60));
                g.rrect_outline(x + 0.5, y + 0.5, w - 1.0, row_h - 1.0, 2.0, 1.0, accent);
            }
            let t = g.fit(size, &cell_text(f, *r, c, o), Face::Sans, w - 10.0 * s);
            let num = matches!(col.ty, ColType::Int | ColType::Float);
            let tw = g.text_width(size, &t, Face::Sans);
            g.text(if num { x + w - tw - 6.0 * s } else { x + 6.0 * s }, y + 3.0 * s, size, TEXT, &t, Face::Sans);
            g.hit(R::new(x, y, w, row_h), Event::Tab(TABLE_CELL + (*r).min(9_000) * 64 + c.min(63)), "cell");
            x += w;
        }
    }
    // footer: rows and paging
    let fy = body.y + body.h - foot + 3.0 * s;
    let info = format!("{} rows{}  page {}/{}", f.rows.len(), if f.meta.truncated { " (cut)" } else { "" }, view.page + 1, pages);
    g.text(body.x + 4.0 * s, fy, 11.0 * s, DIM, &info, Face::Sans);
    if pages > 1 {
        let bx = body.x + body.w - 60.0 * s;
        for (k, (txt, d)) in [("‹", -1i32), ("›", 1)].iter().enumerate() {
            let r = R::new(bx + k as f32 * 30.0 * s, fy - 3.0 * s, 26.0 * s, 20.0 * s);
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, if hot { with_alpha(accent, 60) } else { rgb(24, 31, 58) });
            let tw = g.text_width(13.0 * s, txt, Face::Bold);
            g.text(r.x + (r.w - tw) / 2.0, r.y + 2.0 * s, 13.0 * s, TEXT, txt, Face::Bold);
            g.hit(r, Event::Tab(TABLE_PAGE + if *d < 0 { 0 } else { 1 } + idx * 2), if *d < 0 { "previous page" } else { "next page" });
        }
    }
    pages
}

// ------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::dash::frame::{Column, Series};
    use serde_json::json;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 33) as f64 / (1u64 << 31) as f64
    }

    #[test]
    fn ticks_cover_the_range_with_nice_steps() {
        let mut seed = 7;
        for _ in 0..2000 {
            let a = (lcg(&mut seed) - 0.5) * 10f64.powi((lcg(&mut seed) * 14.0) as i32 - 4);
            let b = a + lcg(&mut seed) * 10f64.powi((lcg(&mut seed) * 12.0) as i32 - 4);
            if (b - a) < a.abs().max(b.abs()) * 1e-6 {
                continue; // beyond what f64 can tell apart
            }
            let t = nice_ticks(a, b, 5);
            assert!(t.len() >= 2 && t.len() <= 12, "{a} {b} {}", t.len());
            assert!(t[0] <= a + 1e-9 * a.abs().max(1.0) && *t.last().unwrap() >= b - 1e-9 * b.abs().max(1.0), "{a} {b} {t:?}");
            assert!(t.windows(2).all(|w| w[1] > w[0]), "{t:?}");
            let step = t[1] - t[0];
            let mag = 10f64.powf(step.log10().floor());
            let m = (step / mag * 1000.0).round() / 1000.0;
            assert!([1.0, 2.0, 5.0, 10.0].iter().any(|x| (m - x).abs() < 1e-3), "step {step} is not 1/2/5 x 10^k ({a},{b})");
        }
        assert_eq!(nice_ticks(0.0, 100.0, 5), vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        assert!(nice_ticks(5.0, 5.0, 5).len() >= 2, "a flat range still gets an axis");
        assert_eq!(nice_ticks(f64::NAN, 1.0, 5), vec![0.0, 1.0]);
    }

    #[test]
    fn time_ticks_are_inside_the_range_ordered_and_few() {
        let mut seed = 11;
        for _ in 0..500 {
            let from = 1_700_000_000_000 + (lcg(&mut seed) * 1e10) as i64;
            let span = (10f64.powf(3.0 + lcg(&mut seed) * 8.0)) as i64;
            let t = time_ticks(from, from + span, 6);
            assert!(t.len() <= 40, "{span} {}", t.len());
            assert!(t.iter().all(|x| x.0 >= from && x.0 <= from + span));
            assert!(t.windows(2).all(|w| w[1].0 > w[0].0));
            assert!(t.iter().all(|x| !x.1.is_empty()));
        }
        let t = time_ticks(1_792_065_600_000, 1_792_069_200_000, 6);
        assert_eq!(t.first().unwrap().1, "12:00");
        assert_eq!(t[1].0 - t[0].0, 600_000);
    }

    #[test]
    fn decimation_keeps_the_extremes_and_the_budget() {
        let mut seed = 3;
        for n in [10usize, 100, 1000, 5000] {
            let pts: Vec<(i64, f64)> = (0..n).map(|i| (i as i64 * 1000, lcg(&mut seed) * 100.0)).collect();
            let d = decimate(&pts, 200);
            assert!(d.len() <= 2 * 200 + 2, "{n} -> {}", d.len());
            let mx = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
            let mn = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            assert!(d.iter().any(|p| p.1 == mx) && d.iter().any(|p| p.1 == mn), "{n}");
            assert!(d.windows(2).all(|w| w[0].0 <= w[1].0), "time order is kept");
        }
    }

    #[test]
    fn nearest_point_and_plot_mapping_round_trip() {
        let pts = vec![(0, 1.0), (10, 2.0), (30, 3.0)];
        assert_eq!(nearest_index(&pts, -5), Some(0));
        assert_eq!(nearest_index(&pts, 14), Some(1));
        assert_eq!(nearest_index(&pts, 21), Some(2));
        assert_eq!(nearest_index(&pts, 99), Some(2));
        assert_eq!(nearest_index(&[], 1), None);
        let p = Plot { r: R::new(100.0, 50.0, 400.0, 200.0), t0: 1000, t1: 2000, y0: 0.0, y1: 10.0, ry0: 0.0, ry1: 100.0 };
        assert!((p.x(1500) - 300.0).abs() < 1e-3 && (p.y(5.0) - 150.0).abs() < 1e-3);
        for px in [100.0, 217.0, 499.0] {
            let t = p.t_at(px);
            assert!((p.x(t) - px).abs() < 0.5);
        }
        assert_eq!(p.t_at(-50.0), 1000);
        assert_eq!(p.t_at(9999.0), 2000);
    }

    #[test]
    fn the_model_picks_domains_and_hides_series() {
        let f = Frame { series: vec![Series { name: "a".into(), points: vec![(0, 2.0), (1000, 8.0)], ..Default::default() }, Series { name: "b".into(), points: vec![(0, 50.0)], ..Default::default() }], ..Default::default() };
        let mut o = Options { legend: true, ..Default::default() };
        let m = Model::build(&f, Visual::Line, &o, (0, 1000));
        assert!(m.y0 <= 2.0 && m.y1 >= 50.0);
        o.hidden = vec!["b".into()];
        let m = Model::build(&f, Visual::Line, &o, (0, 1000));
        assert!(m.y1 >= 8.0 && m.y1 <= 10.0, "{}", m.y1);
        assert!(m.lines[1].hidden);
        assert_eq!(values_at(&m, 400).len(), 1);
        let stacked = Model::build(&f, Visual::StackedBar, &Options::default(), (0, 1000));
        assert!(stacked.y1 >= 52.0 && stacked.y0 == 0.0);
        let empty = Model::build(&Frame::default(), Visual::Line, &Options::default(), (0, 1000));
        assert!(empty.y1 > empty.y0 && empty.t1 > empty.t0);
        let pie = Model::build(&Frame { columns: vec![Column { name: "k".into(), ty: ColType::Str }, Column { name: "n".into(), ty: ColType::Int }], rows: vec![vec![json!("x"), json!(3)], vec![json!("y"), json!(0)], vec![json!("z"), json!(1)]], ..Default::default() }, Visual::Pie, &Options::default(), (0, 0));
        assert_eq!(pie.slices, vec![("x".to_string(), 3.0), ("z".to_string(), 1.0)]);
        assert!((fractions(&[3.0, 0.0, 1.0]).iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn tables_sort_numbers_as_numbers_and_nulls_last() {
        let f = Frame { columns: vec![Column { name: "n".into(), ty: ColType::Int }, Column { name: "s".into(), ty: ColType::Str }], rows: vec![vec![json!(10), json!("b")], vec![json!(9), json!("A")], vec![json!(null), json!("c")], vec![json!(100), json!(null)]], ..Default::default() };
        assert_eq!(sorted_rows(&f, Some((0, false))), vec![1, 0, 3, 2]);
        assert_eq!(sorted_rows(&f, Some((0, true))), vec![3, 0, 1, 2]);
        assert_eq!(sorted_rows(&f, Some((1, false))), vec![1, 0, 2, 3]);
        assert_eq!(sorted_rows(&f, None), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_right_axis_has_its_own_scale_and_values_map_onto_it() {
        let mk = |name: &str, axis: u8, v: f64| Series { name: name.into(), points: vec![(0, v), (60_000, v * 2.0)], axis, ..Default::default() };
        let mut f = Frame { series: vec![mk("cpu", 0, 50.0), mk("requests", 1, 20_000.0)], ..Default::default() };
        f.meta.right_unit = Some("number".into());
        let m = Model::build(&f, Visual::Line, &Options { legend: true, unit: Unit::Percent, ..Default::default() }, (0, 60_000));
        assert!(m.y1 <= 120.0 && m.ry1 >= 40_000.0, "{} {}", m.y1, m.ry1);
        assert_eq!(m.right_unit, Unit::Number);
        let p = m.plot(R::new(0.0, 0.0, 100.0, 100.0));
        assert!((p.y_on(0, m.y1) - 0.0).abs() < 1e-3 && (p.y_on(1, m.ry1) - 0.0).abs() < 1e-3);
        assert!((p.y_on(1, m.ry0) - 100.0).abs() < 1e-3);
        for (w, h) in [(800u32, 480u32), (300, 160)] {
            let mut g = Gfx::new(w, h);
            let geo = draw_xy(&mut g, R::new(4.0, 4.0, w as f32 - 8.0, h as f32 - 8.0), 0.9, &m, Visual::Line, &Options { legend: true, unit: Unit::Percent, ..Default::default() }, Pointer { hover: Some((w as i32 / 2, h as i32 / 2)), drag: None }, 0, 0);
            let plot = geo.plot.unwrap();
            assert!(plot.x + plot.w <= w as f32 && plot.w > 20.0, "{plot:?}");
            if let Some(t) = geo.tooltip {
                let rq = t.rows.iter().find(|r| r.0 == "requests").expect("the right-axis series is in the tooltip");
                assert!(rq.1.ends_with('K') || rq.1.parse::<f64>().is_ok(), "formatted with the right unit: {}", rq.1);
            }
        }
    }

    #[test]
    fn charts_draw_at_every_size_without_panicking_and_stay_inside() {
        let pts: Vec<(i64, f64)> = (0..300).map(|i| (1_792_065_600_000 + i * 12_000, ((i as f64) / 20.0).sin() * 40.0 + 50.0)).collect();
        let f = Frame { series: (0..5).map(|k| Series { name: format!("series number {k}"), points: pts.iter().map(|p| (p.0, p.1 + k as f64 * 3.0)).collect(), ..Default::default() }).collect(), ..Default::default() };
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (200, 120)] {
            for v in [Visual::Line, Visual::Area, Visual::Bar, Visual::StackedBar, Visual::Heatmap] {
                let mut g = Gfx::new(w, h);
                let o = Options { legend: true, unit: Unit::Percent, ..Default::default() };
                let m = Model::build(&f, v, &o, (1_792_065_600_000, 1_792_069_200_000));
                let body = R::new(10.0, 10.0, w as f32 - 20.0, h as f32 - 20.0);
                let geo = draw_xy(&mut g, body, 0.9, &m, v, &o, Pointer { hover: Some((w as i32 / 2, h as i32 / 2)), drag: None }, 0, 0);
                let plot = geo.plot.unwrap();
                assert!(plot.x >= body.x && plot.x + plot.w <= body.x + body.w + 0.5 && plot.y >= body.y && plot.y + plot.h <= body.y + body.h + 0.5, "{v:?} {w}x{h}: {plot:?}");
                if let Some(t) = geo.tooltip {
                    draw_tooltip(&mut g, &t, 0.9, R::new(0.0, 0.0, w as f32, h as f32));
                }
            }
        }
    }
}

/// Text cut to `n` characters with an ellipsis (for chip labels).
pub fn clip(s: &str, n: usize) -> String {
    iyakki_agent::dash::frame::clip(s, n)
}
