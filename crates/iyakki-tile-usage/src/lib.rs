//! USAGE: how many tokens each model has used, as a stacked bar chart you can switch between the last day, week,
//! month, three months and year. Counts are the provider's own when it reports them, otherwise estimates (about four
//! characters per token), of what was sent to a model and what came back, for everything the assistant does: chat, documents, agent runs. Only totals are kept.

use iyakki_tile_api::usage::{Range, Series};
use iyakki_tile_api::{App, Flow, TileCx};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::ui::{Event, Key, DIM, MUTED, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 130, 170);
const PALETTE: [[u8; 4]; 8] = [rgb(79, 140, 255), rgb(61, 214, 163), rgb(255, 184, 77), rgb(167, 116, 255), rgb(255, 107, 154), rgb(88, 190, 235), rgb(255, 150, 110), rgb(150, 220, 90)];

pub struct UsageApp {
    range: Range,
    everyone: bool,
    series: Option<(Range, bool, i64, Series)>,
}

/// `950`, `12.3k`, `4.5M`.
pub fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        if k < 10.0 { format!("{k:.1}k") } else { format!("{:.0}k", k) }
    } else if n < 1_000_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else {
        format!("{:.1}B", n as f64 / 1e9)
    }
}

/// A round number at or above `v`: 1, 2, 5 times a power of ten.
fn nice_max(v: u64) -> u64 {
    if v <= 10 {
        return 10;
    }
    let p = 10u64.pow((v as f64).log10().floor() as u32);
    [1, 2, 5, 10].iter().map(|m| m * p).find(|m| *m >= v).unwrap_or(10 * p)
}

impl Default for UsageApp {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageApp {
    pub fn new() -> UsageApp {
        UsageApp { range: Range::Week, everyone: false, series: None }
    }

    pub fn range(&self) -> Range {
        self.range
    }

    fn can_see_all(cx: &TileCx) -> bool {
        cx.manage_settings
    }

    fn series(&mut self, cx: &TileCx) -> &Series {
        let now = cx.usage.now_unix();
        let everyone = self.everyone && Self::can_see_all(cx);
        // recomputed when the choice changes or a minute has passed
        let stale = !matches!(&self.series, Some((r, e, at, _)) if *r == self.range && *e == everyone && now - *at < 60);
        if stale {
            let s = cx.usage.series(self.range, now, if everyone { None } else { Some(cx.user) });
            self.series = Some((self.range, everyone, now, s));
        }
        &self.series.as_ref().expect("just computed").3
    }
}

impl App for UsageApp {
    fn title(&self) -> &'static str {
        "Usage"
    }
    fn icon(&self) -> Icon {
        Icon::Usage
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "1-5 or ←→ day · week · month · 3 months · year · e everyone's usage (administrators) · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "Each bar is a stretch of time (an hour, a day, a week or a month); its colours are the models that used the tokens.",
            "1 · last 24 hours   2 · last 7 days   3 · last 30 days   4 · last 13 weeks   5 · last 12 months   (or Left / Right)",
            "Move the pointer over a bar to see its figures.",
            "e · administrators switch between their own usage and everyone's.",
            "Counts are the provider's own when it reports them, otherwise estimates (about four characters per token), of what was sent to a model and what came back. Only totals are kept, for a year, never what was said.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        let pick = |n: usize| Range::ALL[n.min(4)];
        match ev {
            Event::Tab(n) if *n < 5 => self.range = pick(*n),
            Event::Tab(10) => self.everyone = !self.everyone && Self::can_see_all(cx),
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Char(c)) if ('1'..='5').contains(c) => self.range = pick(*c as usize - '1' as usize),
            Event::Key(Key::Char('e')) => self.everyone = !self.everyone && Self::can_see_all(cx),
            Event::Key(Key::Left) => {
                let i = Range::ALL.iter().position(|r| *r == self.range).unwrap_or(0);
                self.range = pick(i.saturating_sub(1));
            }
            Event::Key(Key::Right) => {
                let i = Range::ALL.iter().position(|r| *r == self.range).unwrap_or(0);
                self.range = pick(i + 1);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, _tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // range buttons
        let ranges: Vec<(String, Event)> = Range::ALL.iter().enumerate().map(|(i, r)| (r.label().to_string(), Event::Tab(i))).collect();
        let cur = Range::ALL.iter().position(|r| *r == self.range).unwrap_or(0);
        iyakki_uikit::fx::tab_strip(g, x, y, s, w * 0.7, &ranges, cur, ACCENT);
        let everyone = self.everyone && Self::can_see_all(cx);
        if Self::can_see_all(cx) {
            let label = if everyone { "Everyone" } else { "Only me" };
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
            g.button(x + w - bw, y, 14.0 * s, label, if everyone { ACCENT } else { MUTED }, Event::Tab(10));
        }
        y += 52.0 * s;
        let range = self.range;
        let ser = self.series(cx).clone();
        let color = |i: usize| PALETTE[i % PALETTE.len()];
        // summary cards
        let cards: [(&str, String); 5] = [
            ("TOKENS", fmt_tokens(ser.input + ser.output)),
            ("SENT", fmt_tokens(ser.input)),
            ("RECEIVED", fmt_tokens(ser.output)),
            ("CALLS", ser.calls.to_string()),
            ("TOP MODEL", ser.models.first().cloned().unwrap_or_else(|| "–".into())),
        ];
        let cw = (w - 4.0 * 10.0 * s) / 5.0;
        for (i, (k, v)) in cards.iter().enumerate() {
            let cx0 = x + i as f32 * (cw + 10.0 * s);
            iyakki_uikit::fx::glass(g, cx0, y, cw, 62.0 * s, 12.0 * s, ACCENT);
            g.text(cx0 + 14.0 * s, y + 9.0 * s, 11.0 * s, DIM, k, Face::Bold);
            let v = g.fit(21.0 * s, v, Face::Bold, cw - 24.0 * s);
            g.text(cx0 + 14.0 * s, y + 28.0 * s, 21.0 * s, TEXT, &v, Face::Bold);
        }
        y += 74.0 * s;
        // chart and legend
        let legend_w = (w * 0.26).clamp_safe(170.0 * s, 300.0 * s);
        let chart = R::new(x, y, w - legend_w - 16.0 * s, area.y + area.h - y - 56.0 * s);
        iyakki_uikit::fx::panel(g, chart, s, ACCENT);
        let (pl, pr, pt, pb) = (54.0 * s, 16.0 * s, 16.0 * s, 30.0 * s);
        let plot = R::new(chart.x + pl, chart.y + pt, chart.w - pl - pr, chart.h - pt - pb);
        let peak = ser.buckets.iter().map(|b| b.total()).max().unwrap_or(0);
        let top = nice_max(peak);
        for i in 0..=4u64 {
            let v = top * i / 4;
            let gy = plot.y + plot.h - plot.h * i as f32 / 4.0;
            g.rect(plot.x, gy, plot.w, 1.0, rgb(34, 42, 72));
            let t = fmt_tokens(v);
            let tw = g.text_width(11.0 * s, &t, Face::Sans);
            g.text(plot.x - tw - 8.0 * s, gy - 7.0 * s, 11.0 * s, DIM, &t, Face::Sans);
        }
        let n = ser.buckets.len().max(1);
        let slot = plot.w / n as f32;
        let bar_w = (slot * 0.68).max(2.0);
        let mut hover: Option<usize> = None;
        let label_every = match range {
            Range::Day => 3,
            Range::Week => 1,
            Range::Month => 3,
            Range::Months => 2,
            Range::Year => 1,
        };
        for (i, b) in ser.buckets.iter().enumerate() {
            let bx = plot.x + i as f32 * slot + (slot - bar_w) / 2.0;
            let cell = R::new(plot.x + i as f32 * slot, plot.y, slot, plot.h);
            let hot = g.hovered(cell);
            if hot {
                hover = Some(i);
                g.rect(cell.x, cell.y, cell.w, cell.h, rgb(24, 32, 62));
            }
            let mut by = plot.y + plot.h;
            for (mi, v) in b.by_model.iter().enumerate() {
                if *v == 0 {
                    continue;
                }
                let bh = (plot.h * *v as f32 / top as f32).max(1.0);
                by -= bh;
                g.rect(bx, by, bar_w, bh, color(mi));
            }
            if i % label_every == 0 || i == n - 1 && label_every == 1 {
                let lw = g.text_width(11.0 * s, &b.label, Face::Sans);
                if lw <= slot * label_every as f32 + 2.0 {
                    g.text(plot.x + i as f32 * slot + (slot - lw) / 2.0, plot.y + plot.h + 8.0 * s, 11.0 * s, MUTED, &b.label, Face::Sans);
                }
            }
        }
        if peak == 0 {
            iyakki_uikit::fx::empty_state(g, plot, s, Icon::Usage, "No usage in this range", "Ask the assistant something, or pick a longer range.", ACCENT);
        }
        // the hover line under the chart
        let line_y = chart.y + chart.h + 12.0 * s;
        match hover.and_then(|i| ser.buckets.get(i)) {
            Some(b) if b.total() > 0 => {
                let parts: Vec<String> = b.by_model.iter().enumerate().filter(|(_, v)| **v > 0).map(|(i, v)| format!("{} {}", ser.models[i], fmt_tokens(*v))).collect();
                let t = format!("{}   ·   {} tokens   ·   {}", b.full, fmt_tokens(b.total()), parts.join("   ·   "));
                let t = g.fit(14.0 * s, &t, Face::Sans, w);
                g.text(x, line_y, 14.0 * s, TEXT, &t, Face::Sans);
            }
            Some(b) => {
                g.text(x, line_y, 14.0 * s, MUTED, &format!("{}   ·   no usage", b.full), Face::Sans);
            }
            None => {
                g.text(x, line_y, 14.0 * s, DIM, "Move over a bar for its figures.", Face::Sans);
            }
        }
        // legend
        let lx = chart.x + chart.w + 16.0 * s;
        iyakki_uikit::fx::glass(g, lx, y, legend_w, chart.h, 14.0 * s, ACCENT);
        iyakki_uikit::fx::section_title(g, lx + 8.0 * s, y + 12.0 * s, legend_w - 24.0 * s, s, "Models", ACCENT);
        let total: u64 = ser.totals.iter().sum();
        let rows = (((chart.h - 44.0 * s) / (40.0 * s)).floor() as usize).max(1);
        for (i, m) in ser.models.iter().enumerate().take(rows) {
            let ry = y + 38.0 * s + i as f32 * 40.0 * s;
            g.rrect(lx + 16.0 * s, ry + 4.0 * s, 12.0 * s, 12.0 * s, 3.0 * s, color(i));
            let name = g.fit(14.0 * s, m, Face::Bold, legend_w - 56.0 * s);
            g.text(lx + 36.0 * s, ry, 14.0 * s, TEXT, &name, Face::Bold);
            let pct = if total == 0 { 0.0 } else { ser.totals[i] as f32 * 100.0 / total as f32 };
            g.text(lx + 36.0 * s, ry + 19.0 * s, 11.0 * s, MUTED, &format!("{} tokens · {pct:.0}%", fmt_tokens(ser.totals[i])), Face::Sans);
        }
        if ser.models.len() > rows {
            g.text(lx + 16.0 * s, y + chart.h - 24.0 * s, 11.0 * s, DIM, &format!("+{} more", ser.models.len() - rows), Face::Sans);
        }
        if ser.models.is_empty() {
            g.text(lx + 16.0 * s, y + 40.0 * s, 13.0 * s, DIM, "none yet", Face::Sans);
        }
        let _ = (n, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_tile_api::usage::{Bucket, UsageSource};

    /// A fake usage log: (hours ago, user, model, input, output), one call each. Buckets are as long as the range's bucket size.
    struct FakeUsage {
        now: i64,
        calls: Vec<(i64, &'static str, &'static str, u64, u64)>,
    }

    impl UsageSource for FakeUsage {
        fn now_unix(&self) -> i64 {
            self.now
        }
        fn series(&self, range: Range, now: i64, user: Option<&str>) -> Series {
            // (bucket length in hours, number of buckets)
            let (len, n) = match range {
                Range::Day => (1, 24),
                Range::Week => (24, 7),
                Range::Month => (24, 30),
                Range::Months => (24 * 7, 13),
                Range::Year => (24 * 30, 12),
            };
            let mine: Vec<_> = self.calls.iter().filter(|c| user.is_none_or(|u| u == c.1) && c.0 < len * n && now == self.now).collect();
            let mut models: Vec<String> = Vec::new();
            for c in &mine {
                if !models.iter().any(|m| m == c.2) {
                    models.push(c.2.to_string());
                }
            }
            let mut buckets: Vec<Bucket> = (0..n)
                .map(|i| Bucket { label: format!("b{i}"), full: format!("bucket {i}"), start: 0, end: 0, by_model: vec![0; models.len()], input: 0, output: 0, calls: 0 })
                .collect();
            let mut totals = vec![0u64; models.len()];
            let (mut input, mut output) = (0, 0);
            for c in &mine {
                let b = &mut buckets[(n - 1 - c.0 / len) as usize];
                let mi = models.iter().position(|m| m == c.2).unwrap();
                b.by_model[mi] += c.3 + c.4;
                b.input += c.3;
                b.output += c.4;
                b.calls += 1;
                totals[mi] += c.3 + c.4;
                input += c.3;
                output += c.4;
            }
            Series { models, buckets, totals, input, output, calls: mine.len() as u64 }
        }
    }

    fn fake() -> FakeUsage {
        FakeUsage {
            now: 1_800_000_000,
            calls: vec![(1, "root", "qwen3:8b", 1200, 800), (3, "root", "qwen3:8b", 5000, 2500), (30, "root", "llama3.2", 900, 400), (24 * 5, "root", "big-model", 40_000, 20_000), (24 * 40, "root", "big-model", 90_000, 30_000), (2, "alice", "qwen3:8b", 100_000, 50_000)],
        }
    }

    fn cx_of<'a>(u: &'a FakeUsage, user: &'a str, admin: bool) -> TileCx<'a> {
        TileCx { user, manage_settings: admin, usage: u, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &iyakki_tile_api::cameras::NoCameras }
    }

    #[test]
    fn numbers_and_axes_are_rounded_for_reading() {
        assert_eq!((fmt_tokens(0), fmt_tokens(950), fmt_tokens(1500), fmt_tokens(12_345), fmt_tokens(4_500_000), fmt_tokens(2_100_000_000)), ("0".into(), "950".into(), "1.5k".into(), "12k".into(), "4.5M".into(), "2.1B".into()));
        assert_eq!((nice_max(0), nice_max(7), nice_max(11), nice_max(130), nice_max(2600), nice_max(5000), nice_max(5001)), (10, 10, 20, 200, 5000, 5000, 10_000));
    }

    #[test]
    fn the_ranges_switch_and_show_my_usage_or_everyones_for_administrators() {
        let u = fake();
        let cx = cx_of(&u, "root", true);
        let mut a = UsageApp::new();
        let mut g = Gfx::new(1200, 700);
        let draw = |a: &mut UsageApp, g: &mut Gfx| {
            g.clear(rgb(9, 11, 20));
            g.clear_hits();
            a.render(g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 0);
        };
        draw(&mut a, &mut g);
        assert_eq!(a.range(), Range::Week);
        assert_eq!(a.series.as_ref().unwrap().3.calls, 4, "mine in the last 7 days: 4 of root's 5 recent calls are within a week");
        for (key, r) in [('1', Range::Day), ('2', Range::Week), ('3', Range::Month), ('4', Range::Months), ('5', Range::Year)] {
            a.handle(&Event::Key(Key::Char(key)), &cx);
            assert_eq!(a.range(), r);
            draw(&mut a, &mut g);
        }
        assert_eq!(a.series.as_ref().unwrap().3.calls, 5, "a year: all five of mine");
        // clicks on the buttons, arrows
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.range(), Range::Day);
        a.handle(&Event::Key(Key::Right), &cx);
        assert_eq!(a.range(), Range::Week);
        a.handle(&Event::Key(Key::Left), &cx);
        a.handle(&Event::Key(Key::Left), &cx);
        assert_eq!(a.range(), Range::Day);
        draw(&mut a, &mut g);
        assert_eq!(a.series.as_ref().unwrap().3.calls, 2, "a day: mine from 1 and 3 hours ago (not 30)");
        // the administrator sees everyone's
        a.handle(&Event::Key(Key::Char('e')), &cx);
        draw(&mut a, &mut g);
        assert_eq!(a.series.as_ref().unwrap().3.calls, 3, "alice's call joins");
        assert!(a.series.as_ref().unwrap().3.models.contains(&"qwen3:8b".to_string()));
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        assert!(!a.help().is_empty());
    }

    #[test]
    fn a_person_without_the_settings_permission_only_ever_sees_their_own_usage() {
        let u = fake();
        let cx = cx_of(&u, "alice", false);
        let mut a = UsageApp::new();
        a.handle(&Event::Key(Key::Char('e')), &cx);
        a.handle(&Event::Tab(10), &cx);
        let mut g = Gfx::new(900, 600);
        a.render(&mut g, R::new(0.0, 46.0, 900.0, 554.0), 1.0, &cx, 0);
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(10))), "no everyone button");
        let s = &a.series.as_ref().unwrap().3;
        assert_eq!(s.calls, 1, "only alice's own call");
        assert_eq!(s.models, vec!["qwen3:8b"]);
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-tile-usage usage_picture -- --ignored`
    #[test]
    #[ignore]
    fn usage_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let mut u = fake();
        u.calls.clear();
        let mut seed = 7u64;
        let mut rnd = |m: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        for day in 0..45i64 {
            for (model, scale) in [("qwen3:8b", 30_000u64), ("llama3.2", 12_000), ("big-model", 8_000)] {
                if rnd(3) == 0 {
                    continue;
                }
                let n = rnd(scale) + 500;
                u.calls.push((day * 24 + rnd(24) as i64, "root", model, n, n / 2));
            }
        }
        let cx = cx_of(&u, "root", true);
        let mut a = UsageApp::new();
        a.range = Range::Month;
        let mut g = Gfx::new(1200, 700);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 0);
        g.save_png(std::path::Path::new(&out)).unwrap();
    }
}
