//! Words and numbers about a frame, computed (never guessed): range, trend, peaks, jumps, comparison with an earlier window.
//! The assistant's `dash_explain` returns this text so its explanation rests on the data.

use super::frame::*;
use super::model::{format_value, Unit};

/// Points more than this many standard deviations from the mean count as peaks.
const PEAK_SIGMAS: f64 = 2.5;

fn when(ms: i64) -> String {
    format_time(ms)
}

/// Least-squares slope per hour and the change from the first to the last quarter of the points.
pub fn trend(points: &[(i64, f64)]) -> Option<(f64, f64)> {
    if points.len() < 4 {
        return None;
    }
    let n = points.len() as f64;
    let t0 = points[0].0 as f64;
    let xs: Vec<f64> = points.iter().map(|p| (p.0 as f64 - t0) / 3_600_000.0).collect();
    let (mx, my) = (xs.iter().sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, p) in xs.iter().zip(points) {
        sxy += (x - mx) * (p.1 - my);
        sxx += (x - mx).powi(2);
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let q = (points.len() / 4).max(1);
    let first: f64 = points[..q].iter().map(|p| p.1).sum::<f64>() / q as f64;
    let last: f64 = points[points.len() - q..].iter().map(|p| p.1).sum::<f64>() / q as f64;
    let change = if first.abs() > 1e-12 { (last - first) / first.abs() * 100.0 } else { 0.0 };
    Some((slope, change))
}

/// Times where the value is far above the mean (grouped when adjacent), at most `max`.
pub fn peaks(points: &[(i64, f64)], max: usize) -> Vec<(i64, f64)> {
    let Some(st) = (Series { name: String::new(), labels: Default::default(), points: points.to_vec(), axis: 0 }).stats() else { return vec![] };
    if st.std <= f64::EPSILON {
        return vec![];
    }
    let mut hot: Vec<(i64, f64)> = points.iter().filter(|p| (p.1 - st.avg) / st.std >= PEAK_SIGMAS).cloned().collect();
    // keep the highest point of each run of neighbours
    let gap = if points.len() > 1 { (points[points.len() - 1].0 - points[0].0) / (points.len() as i64 - 1) * 3 } else { 0 };
    hot.sort_by_key(|p| p.0);
    let mut runs: Vec<(i64, f64)> = vec![];
    for p in hot {
        match runs.last_mut() {
            Some(l) if p.0 - l.0 <= gap.max(1) => {
                if p.1 > l.1 {
                    *l = p;
                } else {
                    l.0 = p.0.max(l.0);
                    // the run's time is extended but the peak value kept: record the peak's own time instead
                }
            }
            _ => runs.push(p),
        }
    }
    runs.sort_by(|a, b| b.1.total_cmp(&a.1));
    runs.truncate(max);
    runs.sort_by_key(|p| p.0);
    runs
}

/// A paragraph per series (at most `max_series`), then a sentence for tables.
pub fn explain_frame(f: &Frame, unit: Unit, max_series: usize) -> String {
    let fv = |v: f64| format_value(v, unit, None);
    let mut out = String::new();
    for s in f.series.iter().take(max_series) {
        let Some(st) = s.stats() else {
            out.push_str(&format!("{}: no data points.\n", s.name));
            continue;
        };
        out.push_str(&format!("{}: {} points from {} to {}; now {}, min {} at {}, max {} at {}, average {}.", s.name, st.count, when(s.points[0].0), when(s.points[s.points.len() - 1].0), fv(st.last), fv(st.min), when(st.min_at), fv(st.max), when(st.max_at), fv(st.avg)));
        if let Some((slope, change)) = trend(&s.points) {
            let dir = if change.abs() < 5.0 { "roughly flat" } else if change > 0.0 { "rising" } else { "falling" };
            out.push_str(&format!(" Trend: {dir} ({change:+.0}% from the first quarter to the last, {} per hour).", fv(slope)));
        }
        let pk = peaks(&s.points, 3);
        if !pk.is_empty() {
            out.push_str(&format!(" Peaks: {}.", pk.iter().map(|(t, v)| format!("{} at {}", fv(*v), when(*t))).collect::<Vec<_>>().join(", ")));
        } else if st.count >= 4 {
            out.push_str(" No sharp peaks.");
        }
        // the biggest jump between neighbours
        let jump = s.points.windows(2).map(|w| (w[1].0, w[1].1 - w[0].1)).max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
        if let Some((t, d)) = jump.filter(|j| st.std > 0.0 && j.1.abs() > 2.0 * st.std) {
            out.push_str(&format!(" Biggest step: {}{} at {}.", if d > 0.0 { "+" } else { "-" }, fv(d.abs()), when(t)));
        }
        out.push('\n');
    }
    if f.series.len() > max_series {
        out.push_str(&format!("({} more series not described)\n", f.series.len() - max_series));
    }
    if f.series.len() > 1 {
        let mut ranked: Vec<(&str, f64)> = f.series.iter().filter_map(|s| s.stats().map(|t| (s.name.as_str(), t.avg))).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        if let (Some(top), Some(low)) = (ranked.first(), ranked.last()) {
            out.push_str(&format!("Highest average: {} ({}); lowest: {} ({}).\n", top.0, fv(top.1), low.0, fv(low.1)));
        }
    }
    if !f.columns.is_empty() && f.series.is_empty() {
        out.push_str(&format!("Table: {} rows, columns {}.\n", f.rows.len(), f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
        for (i, c) in f.columns.iter().enumerate() {
            if matches!(c.ty, ColType::Int | ColType::Float) {
                let vals: Vec<f64> = (0..f.rows.len()).filter_map(|r| f.number(r, i)).collect();
                if vals.is_empty() {
                    continue;
                }
                let sum: f64 = vals.iter().sum();
                let (mn, mx) = (vals.iter().cloned().fold(f64::INFINITY, f64::min), vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
                out.push_str(&format!("{}: sum {}, min {}, max {}, average {}.\n", c.name, short(sum), short(mn), short(mx), short(sum / vals.len() as f64)));
            }
        }
    }
    if out.is_empty() {
        out.push_str("No data in this range.\n");
    }
    if f.meta.truncated {
        out.push_str("(the answer was cut by a limit; the numbers cover what was returned)\n");
    }
    out
}

/// Compare the averages of two windows of the same series (by name): "api: 120 now vs 80 before (+50%)".
pub fn compare(now: &Frame, before: &Frame, unit: Unit) -> String {
    let fv = |v: f64| format_value(v, unit, None);
    let mut lines = vec![];
    for s in &now.series {
        let Some(a) = s.stats() else { continue };
        let Some(b) = before.series.iter().find(|x| x.name == s.name).and_then(|x| x.stats()) else {
            lines.push(format!("{}: average {} (no data in the earlier window)", s.name, fv(a.avg)));
            continue;
        };
        let pct = if b.avg.abs() > 1e-12 { format!("{:+.0}%", (a.avg - b.avg) / b.avg.abs() * 100.0) } else { "n/a".into() };
        lines.push(format!("{}: average {} now vs {} in the earlier window ({pct}); max {} vs {}", s.name, fv(a.avg), fv(b.avg), fv(a.max), fv(b.max)));
    }
    if lines.is_empty() { "Nothing to compare.".into() } else { lines.join("\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spike() -> Frame {
        let mut pts: Vec<(i64, f64)> = (0..60).map(|i| (1_792_065_600_000 + i * 60_000, 100.0 + (i % 3) as f64)).collect();
        for p in pts.iter_mut().skip(40).take(3) {
            p.1 = 900.0;
        }
        Frame { series: vec![Series { name: "latency".into(), points: pts, ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn a_spike_is_found_with_its_time_and_value() {
        let t = explain_frame(&spike(), Unit::Millis, 5);
        assert!(t.contains("latency: 60 points"), "{t}");
        assert!(t.contains("max 900 ms at 2026-10-15 12:40:00"), "{t}");
        assert!(t.contains("Peaks: 900 ms at 2026-10-15 12:4"), "{t}");
        assert!(t.contains("Biggest step: +"), "{t}");
    }

    #[test]
    fn trends_are_computed() {
        let up: Vec<(i64, f64)> = (0..20).map(|i| (i * 3_600_000, 10.0 + i as f64)).collect();
        let (slope, change) = trend(&up).unwrap();
        assert!((slope - 1.0).abs() < 1e-9 && change > 100.0, "{slope} {change}");
        let flat: Vec<(i64, f64)> = (0..20).map(|i| (i * 60_000, 5.0)).collect();
        assert_eq!(trend(&flat).unwrap().1, 0.0);
        assert!(trend(&flat[..3]).is_none());
        assert!(peaks(&flat, 3).is_empty());
        let t = explain_frame(&Frame { series: vec![Series { name: "x".into(), points: up, ..Default::default() }], ..Default::default() }, Unit::None, 3);
        assert!(t.contains("rising"), "{t}");
    }

    #[test]
    fn windows_compare_by_series_name() {
        let now = spike();
        let mut before = spike();
        for p in &mut before.series[0].points {
            p.1 = 50.0;
        }
        let t = compare(&now, &before, Unit::Millis);
        assert!(t.contains("latency: average") && t.contains("vs 50 ms") && t.contains('+'), "{t}");
        assert_eq!(compare(&now, &Frame::empty(), Unit::None).contains("no data in the earlier window"), true);
        assert_eq!(compare(&Frame::empty(), &now, Unit::None), "Nothing to compare.");
    }

    #[test]
    fn tables_and_empty_frames() {
        let f = Frame { columns: vec![Column { name: "q".into(), ty: ColType::Str }, Column { name: "ms".into(), ty: ColType::Float }], rows: vec![vec!["a".into(), 10.0.into()], vec!["b".into(), 30.0.into()]], ..Default::default() };
        let t = explain_frame(&f, Unit::None, 3);
        assert!(t.contains("ms: sum 40, min 10, max 30, average 20"), "{t}");
        assert_eq!(explain_frame(&Frame::empty(), Unit::None, 3), "No data in this range.\n");
        // NaN and odd values never panic
        let odd = Frame { series: vec![Series { name: "n".into(), points: vec![(0, f64::NAN), (1, f64::INFINITY)], ..Default::default() }], ..Default::default() };
        let _ = explain_frame(&odd, Unit::Bytes, 3);
    }
}
