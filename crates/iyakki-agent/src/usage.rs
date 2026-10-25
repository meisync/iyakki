//! How many tokens each model has used, kept for a year so it can be charted by day, week, month or year.
//!
//! Every answer the assistant gets from a model is counted here (the text sent and the text received, at about four
//! characters per token, because not every provider reports exact usage). The log keeps one line per call and merges
//! them into hourly totals when the system starts, so it stays small. Only the totals are kept: never what was said.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::clock::{civil, days_from_civil, Zone};
use crate::services::Services;

const KEEP_DAYS: i64 = 400;
const DAY: i64 = 86_400;

/// Who made the calls of the assistant itself (the chat, agents, dictation); old lines have no source and read as this.
pub const SRC_ASSISTANT: &str = "assistant";

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One hour of calls of one kind. The fields after `calls` are newer than the first log files: they are left out of a line while empty
/// and read as empty when missing, so old lines still load and a downgrade only loses them (docs/development/SDK-API.md, section 6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Hours since the epoch (UTC).
    pub h: i64,
    pub user: String,
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub calls: u64,
    /// Who made the calls: `assistant`, `app:<id>` (a tile app), `mcp:<server>`, `ext:<agent>`, `svc:<instance>`, `system`. Empty: the assistant.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub src: String,
    /// What for: `llm`, `assistant`, `tts`, `stt`, `embed`. Empty: the assistant.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cat: String,
    /// Calls that failed (they carry no tokens).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub errors: u64,
    /// Milliseconds the calls took, summed (divide by `calls` for the mean).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ms: u64,
    /// Some of the numbers are length estimates, not what the provider reported.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub est: bool,
}

/// One call to record (see [`UsageLog::record_call`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Call {
    pub src: String,
    pub cat: String,
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub estimated: bool,
    pub failed: bool,
    pub ms: u64,
}

/// What one source used in a time range: the read side for the Settings page, `iyakki dev usage` and `iyakki.llm.usage()`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub src: String,
    pub cat: String,
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub errors: u64,
    pub ms: u64,
    pub estimated: bool,
}

impl Totals {
    pub fn tokens(&self) -> u64 {
        self.input + self.output
    }

    /// Mean milliseconds per call (0 when there were none).
    pub fn mean_ms(&self) -> u64 {
        self.ms.checked_div(self.calls).unwrap_or(0)
    }

    fn add(&mut self, e: &Entry) {
        self.calls += e.calls;
        self.input += e.input;
        self.output += e.output;
        self.errors += e.errors;
        self.ms += e.ms;
        self.estimated |= e.est;
    }
}

fn key_of(e: &Entry) -> (i64, &str, &str, &str, &str, &str) {
    (e.h, &e.user, &e.provider, &e.model, &e.src, &e.cat)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// The last 24 hours, by hour.
    Day,
    /// The last 7 days, by day.
    Week,
    /// The last 30 days, by day.
    Month,
    /// The last 13 weeks, by week.
    Months,
    /// The last 12 months, by month.
    Year,
}

impl Range {
    pub const ALL: [Range; 5] = [Range::Day, Range::Week, Range::Month, Range::Months, Range::Year];
    pub fn label(self) -> &'static str {
        match self {
            Range::Day => "Day",
            Range::Week => "Week",
            Range::Month => "Month",
            Range::Months => "3 months",
            Range::Year => "Year",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bucket {
    pub label: String,
    /// A longer name for the hover line, such as `Tue 7 Oct`.
    pub full: String,
    pub start: i64,
    pub end: i64,
    /// Tokens (input + output) by model, in the order of `Series::models`.
    pub by_model: Vec<u64>,
    pub input: u64,
    pub output: u64,
    pub calls: u64,
}

impl Bucket {
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Series {
    /// Models with any use in the range, biggest first.
    pub models: Vec<String>,
    pub buckets: Vec<Bucket>,
    pub totals: Vec<u64>,
    pub input: u64,
    pub output: u64,
    pub calls: u64,
}

pub struct UsageLog {
    path: PathBuf,
    lock: Mutex<usize>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl UsageLog {
    pub fn open(path: &Path) -> UsageLog {
        let l = UsageLog { path: path.into(), lock: Mutex::new(0) };
        l.compact(now());
        l
    }

    fn load(&self) -> Vec<Entry> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Merges the lines into hourly totals and drops what is older than the kept year.
    fn compact(&self, now: i64) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let keep_from = (now - KEEP_DAYS * DAY) / 3600;
        let mut merged: Vec<Entry> = Vec::new();
        let mut all = self.load();
        let lines = all.len();
        all.sort_by(|a, b| key_of(a).cmp(&key_of(b)));
        for e in all.into_iter().filter(|e| e.h >= keep_from) {
            match merged.last_mut() {
                Some(m) if key_of(m) == key_of(&e) => {
                    m.input += e.input;
                    m.output += e.output;
                    m.calls += e.calls;
                    m.errors += e.errors;
                    m.ms += e.ms;
                    m.est |= e.est;
                }
                _ => merged.push(e),
            }
        }
        if merged.len() == lines {
            return;
        }
        let text: String = merged.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        let tmp = self.path.with_extension("tmp");
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp) {
            if f.write_all(text.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn record(&self, user: &str, provider: &str, model: &str, input: u64, output: u64) {
        self.record_at(now(), user, provider, model, input, output);
    }

    pub fn record_at(&self, ts: i64, user: &str, provider: &str, model: &str, input: u64, output: u64) {
        self.append(Entry { h: ts.div_euclid(3600), user: user.into(), provider: provider.into(), model: model.into(), input, output, calls: 1, ..Default::default() }, ts);
    }

    /// A call with its source, kind, outcome and time (apps, MCP servers, speech). A failed call counts as a call and an error, with no tokens.
    pub fn record_call(&self, ts: i64, user: &str, c: &Call) {
        let (input, output) = if c.failed { (0, 0) } else { (c.input, c.output) };
        self.append(Entry { h: ts.div_euclid(3600), user: user.into(), provider: c.provider.clone(), model: c.model.clone(), input, output, calls: 1, src: c.src.clone(), cat: c.cat.clone(), errors: c.failed as u64, ms: c.ms, est: c.estimated && !c.failed }, ts);
    }

    fn append(&self, e: Entry, ts: i64) {
        let Ok(line) = serde_json::to_string(&e) else { return };
        let n = {
            let mut g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&self.path) {
                let _ = writeln!(f, "{line}");
            }
            *g += 1;
            *g
        };
        if n % 500 == 0 {
            self.compact(ts);
        }
    }

    /// Everything recorded for `user` (or for everyone when `None`).
    pub fn entries(&self, user: Option<&str>) -> Vec<Entry> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().into_iter().filter(|e| user.is_none_or(|u| e.user == u)).collect()
    }

    pub fn series(&self, range: Range, now_unix: i64, zone: &Zone, user: Option<&str>) -> Series {
        series_of(&self.entries(user), range, now_unix, zone)
    }

    /// What every source used since `from_unix` (to the hour), biggest first: for the "by app" list of Settings -> Usage and
    /// `iyakki dev usage`. Lines without a source are the assistant's.
    pub fn by_source(&self, user: Option<&str>, from_unix: i64) -> Vec<Totals> {
        let mut map: std::collections::BTreeMap<(String, String), Totals> = Default::default();
        for e in self.entries(user).iter().filter(|e| (e.h + 1) * 3600 > from_unix) {
            let (src, cat) = (source_of(e), category_of(e));
            map.entry((src.clone(), cat.clone())).or_insert_with(|| Totals { src, cat, ..Default::default() }).add(e);
        }
        let mut v: Vec<Totals> = map.into_values().collect();
        v.sort_by(|a, b| b.tokens().cmp(&a.tokens()).then(b.calls.cmp(&a.calls)).then(a.src.cmp(&b.src)));
        v
    }

    /// What one source used since `from_unix`; of one kind (`llm`, `tts`) or, with `None`, all kinds together.
    pub fn totals_of(&self, user: Option<&str>, src: &str, cat: Option<&str>, from_unix: i64) -> Totals {
        let mut t = Totals { src: src.into(), ..Default::default() };
        for e in self.entries(user).iter().filter(|e| (e.h + 1) * 3600 > from_unix && source_of(e) == src && cat.is_none_or(|c| category_of(e) == c)) {
            t.add(e);
        }
        t
    }

    /// One source's use per local day for the last `days` days, oldest first: (long day name, totals).
    pub fn days_of(&self, user: Option<&str>, src: &str, cat: Option<&str>, days: usize, now_unix: i64, zone: &Zone) -> Vec<(String, Totals)> {
        let range = if days <= 7 { Range::Week } else { Range::Month };
        let entries: Vec<Entry> = self.entries(user).into_iter().filter(|e| source_of(e) == src && cat.is_none_or(|c| category_of(e) == c)).collect();
        let mut out: Vec<(String, Totals)> = buckets_for(range, now_unix, zone)
            .into_iter()
            .map(|(from, to, _, full)| {
                let mut t = Totals { src: src.into(), ..Default::default() };
                for e in entries.iter().filter(|e| e.h * 3600 >= from && e.h * 3600 < to) {
                    t.add(e);
                }
                (full, t)
            })
            .collect();
        let skip = out.len().saturating_sub(days);
        out.drain(..skip);
        out
    }

    /// Start of today in `zone` and of tomorrow (when a daily cap resets).
    pub fn today_span(now_unix: i64, zone: &Zone) -> (i64, i64) {
        let from = local_midnight(zone, now_unix);
        (from, local_midnight(zone, from + DAY + 12 * 3600))
    }
}

/// Who made the calls of a line (`assistant` for lines from before sources were kept).
pub fn source_of(e: &Entry) -> String {
    if e.src.is_empty() { SRC_ASSISTANT.into() } else { e.src.clone() }
}

fn category_of(e: &Entry) -> String {
    if e.cat.is_empty() { SRC_ASSISTANT.into() } else { e.cat.clone() }
}

/// Start of the local day that contains `t`, as a UTC instant.
fn local_midnight(zone: &Zone, t: i64) -> i64 {
    let l = zone.local(t);
    let days = days_from_civil(l.year, l.month, l.day);
    // the offset in force at midnight can differ from the one now (a clock change during the day)
    let mut m = days * DAY - l.offset as i64;
    m = days * DAY - zone.local(m).offset as i64;
    days * DAY - zone.local(m).offset as i64
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// The buckets of a range, oldest first: (start, end, short label, long label).
pub fn buckets_for(range: Range, now_unix: i64, zone: &Zone) -> Vec<(i64, i64, String, String)> {
    let mut v = Vec::new();
    match range {
        Range::Day => {
            let this_hour = now_unix.div_euclid(3600) * 3600;
            for i in (0..24).rev() {
                let s = this_hour - i * 3600;
                let l = zone.local(s);
                v.push((s, s + 3600, format!("{:02}", l.hour), format!("{} {} {:02}:00", DAYS[l.weekday as usize], l.day, l.hour)));
            }
        }
        Range::Week | Range::Month => {
            let n = if range == Range::Week { 7 } else { 30 };
            let today = local_midnight(zone, now_unix);
            for i in (0..n).rev() {
                // noon of the day i days ago keeps a daylight-saving change from moving the date
                let noon = today - i * DAY + 12 * 3600;
                let s = local_midnight(zone, noon);
                let l = zone.local(s + 3600);
                let label = if range == Range::Week { DAYS[l.weekday as usize].to_string() } else { l.day.to_string() };
                v.push((s, local_midnight(zone, s + DAY + 12 * 3600), label, format!("{} {} {}", DAYS[l.weekday as usize], l.day, MONTHS[l.month as usize - 1])));
            }
        }
        Range::Months => {
            let today = local_midnight(zone, now_unix);
            let wd = zone.local(today + 3600).weekday as i64;
            let this_monday = today - ((wd + 6) % 7) * DAY;
            for i in (0..13).rev() {
                let s = local_midnight(zone, this_monday - i * 7 * DAY + 12 * 3600);
                let l = zone.local(s + 3600);
                v.push((s, s + 7 * DAY, format!("{} {}", l.day, MONTHS[l.month as usize - 1]), format!("week of {} {}", l.day, MONTHS[l.month as usize - 1])));
            }
        }
        Range::Year => {
            let l = zone.local(now_unix);
            let (mut y, mut m) = (l.year, l.month);
            let mut starts = Vec::new();
            for _ in 0..12 {
                starts.push((y, m));
                if m == 1 { y -= 1; m = 12 } else { m -= 1 }
            }
            starts.reverse();
            let at = |y: i64, m: u32| local_midnight(zone, days_from_civil(y, m, 1) * DAY + 12 * 3600);
            for (i, (y, m)) in starts.iter().enumerate() {
                let s = at(*y, *m);
                let e = starts.get(i + 1).map_or_else(|| { let (ny, nm) = if *m == 12 { (*y + 1, 1) } else { (*y, *m + 1) }; at(ny, nm) }, |(ny, nm)| at(*ny, *nm));
                v.push((s, e, MONTHS[*m as usize - 1].to_string(), format!("{} {}", MONTHS[*m as usize - 1], y)));
            }
        }
    }
    let _ = civil;
    v
}

pub fn series_of(entries: &[Entry], range: Range, now_unix: i64, zone: &Zone) -> Series {
    let bs = buckets_for(range, now_unix, zone);
    let (from, to) = (bs.first().map_or(0, |b| b.0), bs.last().map_or(0, |b| b.1));
    let mut by_model: std::collections::BTreeMap<String, u64> = Default::default();
    let inrange: Vec<&Entry> = entries.iter().filter(|e| e.h * 3600 >= from && e.h * 3600 < to).collect();
    for e in &inrange {
        *by_model.entry(e.model.clone()).or_default() += e.input + e.output;
    }
    let mut models: Vec<(String, u64)> = by_model.into_iter().collect();
    models.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let names: Vec<String> = models.iter().map(|m| m.0.clone()).collect();
    let mut buckets: Vec<Bucket> = bs.into_iter().map(|(start, end, label, full)| Bucket { label, full, start, end, by_model: vec![0; names.len()], input: 0, output: 0, calls: 0 }).collect();
    for e in inrange {
        let t = e.h * 3600;
        let Some(b) = buckets.iter_mut().find(|b| t >= b.start && t < b.end) else { continue };
        let mi = names.iter().position(|n| *n == e.model).unwrap_or(0);
        b.by_model[mi] += e.input + e.output;
        b.input += e.input;
        b.output += e.output;
        b.calls += e.calls;
    }
    let (input, output, calls) = buckets.iter().fold((0, 0, 0), |a, b| (a.0 + b.input, a.1 + b.output, a.2 + b.calls));
    Series { totals: models.iter().map(|m| m.1).collect(), models: names, buckets, input, output, calls }
}

/// Wraps a model backend and counts what goes through it, for one person.
pub struct Metered {
    inner: Arc<dyn ChatBackend>,
    services: Arc<Services>,
    user: String,
}

impl Metered {
    pub fn new(inner: Arc<dyn ChatBackend>, services: Arc<Services>, user: &str) -> Metered {
        Metered { inner, services, user: user.into() }
    }

    fn note(&self, input: u64, output: u64, estimated: bool, since: std::time::Instant) {
        let (provider, model) = self.services.settings.lock().unwrap().get().chat.as_ref().map_or(("?".to_string(), "unknown".to_string()), |c| (c.provider.clone(), c.model.clone()));
        let call = Call { src: SRC_ASSISTANT.into(), cat: SRC_ASSISTANT.into(), provider, model, input, output, estimated, failed: false, ms: since.elapsed().as_millis() as u64 };
        self.services.usage.record_call(now(), &self.user, &call);
    }
}

impl ChatBackend for Metered {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let r = self.inner.complete(history)?;
        // what the provider reported when it did, the length-based estimate otherwise
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((crate::agent::estimate_tokens(history), (r.chars().count() as u64).div_ceil(4)));
        self.note(i, o, reported.is_none(), t0);
        Ok(r)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let t = self.inner.complete_tools(history, tools)?;
        let out = crate::agent::estimate_tokens(&[Message::assistant_calls(t.text.clone(), t.calls.clone())]);
        let est_in = crate::agent::estimate_tokens(history) + tools.iter().map(|x| (x.name.len() + x.description.len() + x.schema.to_string().len()) as u64 / 4).sum::<u64>();
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((est_in, out));
        self.note(i, o, reported.is_none(), t0);
        Ok(t)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let t = self.inner.complete_tools_stream(history, tools, on_text)?;
        let out = crate::agent::estimate_tokens(&[Message::assistant_calls(t.text.clone(), t.calls.clone())]);
        let est_in = crate::agent::estimate_tokens(history) + tools.iter().map(|x| (x.name.len() + x.description.len() + x.schema.to_string().len()) as u64 / 4).sum::<u64>();
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((est_in, out));
        self.note(i, o, reported.is_none(), t0);
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(tag: &str) -> UsageLog {
        let d = std::env::temp_dir().join(format!("iyakki-usage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        UsageLog::open(&d.join("usage.jsonl"))
    }

    fn t(y: i64, m: u32, d: u32, h: i64) -> i64 {
        days_from_civil(y, m, d) * DAY + h * 3600
    }

    #[test]
    fn tokens_are_added_up_by_model_in_buckets_for_each_range() {
        let l = log("series");
        let z = Zone::utc();
        let now = t(2026, 10, 11, 14) + 1800;
        l.record_at(now - 600, "amy", "ollama", "qwen3:8b", 100, 50);
        l.record_at(now - 1200, "amy", "ollama", "qwen3:8b", 10, 5);
        l.record_at(now - 5 * 3600, "amy", "cloud", "big-model", 1000, 500);
        l.record_at(now - 2 * DAY, "bob", "ollama", "qwen3:8b", 7, 3);
        l.record_at(now - 40 * DAY, "amy", "cloud", "big-model", 2000, 1000);
        l.record_at(now - 200 * DAY, "amy", "cloud", "big-model", 4000, 2000);
        // a day: 24 hourly buckets, today's models only
        let day = l.series(Range::Day, now, &z, Some("amy"));
        assert_eq!(day.buckets.len(), 24);
        assert_eq!((day.input, day.output, day.calls), (1110, 555, 3));
        assert_eq!(day.models, vec!["big-model", "qwen3:8b"], "biggest first");
        assert_eq!(day.totals, vec![1500, 165]);
        assert_eq!(day.buckets[23].total(), 165, "the current hour is the last bar");
        assert_eq!(day.buckets[23].label, "14");
        // a week and a month are daily; the 2-day-old call is in both, the 40-day-old one in neither
        let week = l.series(Range::Week, now, &z, None);
        assert_eq!(week.buckets.len(), 7);
        assert_eq!(week.calls, 4);
        assert_eq!(week.buckets[6].full, "Sun 11 Oct");
        assert_eq!(week.buckets[4].total(), 10, "Fri 9 Oct");
        let month = l.series(Range::Month, now, &z, None);
        assert_eq!((month.buckets.len(), month.calls), (30, 4));
        // 13 weeks reaches back 90 days, a year 12 months
        let months = l.series(Range::Months, now, &z, Some("amy"));
        assert_eq!(months.buckets.len(), 13);
        assert_eq!(months.calls, 4, "the 40-day-old call joins");
        let year = l.series(Range::Year, now, &z, Some("amy"));
        assert_eq!(year.buckets.len(), 12);
        assert_eq!(year.calls, 5, "and the 200-day-old one");
        assert_eq!(year.buckets[11].label, "Oct");
        assert_eq!(year.buckets[0].label, "Nov", "twelve months ending in October start in November");
        // buckets tile the range without gaps or overlaps
        for r in Range::ALL {
            let b = buckets_for(r, now, &z);
            assert!(b.windows(2).all(|w| w[0].1 == w[1].0), "{r:?}");
            assert!(b.last().unwrap().1 > now && b.last().unwrap().0 <= now, "{r:?}: the last bucket holds now");
        }
        // only one person's usage when asked
        assert_eq!(l.series(Range::Week, now, &z, Some("bob")).calls, 1);
        assert_eq!(l.series(Range::Week, now, &z, Some("nobody")).calls, 0);
    }

    #[test]
    fn local_days_follow_the_time_zone_and_daylight_saving() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let now = t(2026, 3, 9, 20); // the day after clocks went forward
        let days = buckets_for(Range::Week, now, &z);
        assert_eq!(days.len(), 7);
        assert!(days.windows(2).all(|w| w[0].1 == w[1].0));
        assert_eq!(days[5].3, "Sun 8 Mar", "{:?}", days.iter().map(|d| d.3.clone()).collect::<Vec<_>>());
        assert_eq!(days[5].1 - days[5].0, 23 * 3600, "the short day");
        assert_eq!(days[6].3, "Mon 9 Mar");
        // a call at 23:30 local on the 8th lands on the 8th even though it is the 9th in UTC
        let e = Entry { h: (t(2026, 3, 9, 6) + 1800).div_euclid(3600), user: "a".into(), provider: "p".into(), model: "m".into(), input: 5, output: 5, calls: 1, ..Default::default() };
        let s = series_of(&[e], Range::Week, now, &z);
        assert_eq!(s.buckets[5].total(), 10, "{:?}", s.buckets.iter().map(|b| (b.full.clone(), b.total())).collect::<Vec<_>>());
    }

    #[test]
    fn lines_are_merged_into_hourly_totals_and_old_ones_dropped() {
        let d = std::env::temp_dir().join(format!("iyakki-usage-compact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("usage.jsonl");
        let l = UsageLog::open(&p);
        let n = now();
        for _ in 0..20 {
            l.record_at(n - 60, "amy", "p", "m", 10, 5);
        }
        l.record_at(n - 500 * DAY, "amy", "p", "m", 99, 99);
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 21);
        let l2 = UsageLog::open(&p);
        let lines = std::fs::read_to_string(&p).unwrap().lines().count();
        assert!(lines <= 2, "merged into one hour (maybe two if the hour just changed): {lines}");
        let e = l2.entries(None);
        assert_eq!(e.iter().map(|x| x.calls).sum::<u64>(), 20);
        assert_eq!(e.iter().map(|x| x.input).sum::<u64>(), 200);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn old_lines_still_read_and_new_fields_stay_out_of_a_line_until_used() {
        let d = std::env::temp_dir().join(format!("iyakki-usage-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("usage.jsonl");
        // a line as the first versions wrote it
        std::fs::write(&p, "{\"h\":490000,\"user\":\"amy\",\"provider\":\"ollama\",\"model\":\"qwen3:8b\",\"input\":100,\"output\":50,\"calls\":2}\nnot json\n").unwrap();
        let l = UsageLog::open(&p);
        let e = l.entries(None);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].src.as_str(), e[0].errors, e[0].ms, e[0].est), ("", 0, 0, false));
        assert_eq!(source_of(&e[0]), "assistant", "a line without a source is the assistant's");
        // writing an old-style record adds nothing new to the line
        l.record_at(490_000 * 3600, "amy", "ollama", "qwen3:8b", 1, 1);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.lines().last().unwrap().ends_with("\"calls\":1}"), "{text}");
        // a new record carries source, kind, errors and time, and reads back
        l.record_call(490_000 * 3600, "amy", &Call { src: "app:calc".into(), cat: "llm".into(), provider: "p".into(), model: "m".into(), input: 10, output: 5, estimated: true, failed: false, ms: 300 });
        let last = l.entries(None).pop().unwrap();
        assert_eq!((last.src.as_str(), last.cat.as_str(), last.ms, last.est, last.errors), ("app:calc", "llm", 300, true, 0));
        // a reader that knows nothing of the new fields (an older program) still reads the line
        let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&p).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!((v["input"].as_u64(), v["calls"].as_u64()), (Some(10), Some(1)));
    }

    #[test]
    fn calls_are_attributed_to_apps_with_errors_and_time_and_merge_by_source() {
        let l = log("attr");
        let z = Zone::utc();
        let now = t(2026, 10, 11, 14) + 1800;
        let call = |src: &str, cat: &str, i, o, failed, ms| Call { src: src.into(), cat: cat.into(), provider: "p".into(), model: "m".into(), input: i, output: o, estimated: false, failed, ms };
        l.record_call(now - 60, "amy", &call("app:calc", "llm", 100, 20, false, 200));
        l.record_call(now - 90, "amy", &call("app:calc", "llm", 50, 10, false, 400));
        l.record_call(now - 30, "amy", &call("app:calc", "llm", 999, 999, true, 100));
        l.record_call(now - 30, "amy", &call("app:calc", "tts", 40, 0, false, 900));
        l.record_call(now - 30, "amy", &call("mcp:files", "llm", 5, 5, false, 50));
        l.record_call(now - 30, "bob", &call("app:calc", "llm", 7, 3, false, 10));
        l.record_at(now - 30, "amy", "p", "m", 1000, 500);
        l.record_call(now - 3 * DAY, "amy", &call("app:calc", "llm", 1, 1, false, 5));
        let by = l.by_source(Some("amy"), now - DAY);
        let find = |src: &str, cat: &str| by.iter().find(|t| t.src == src && t.cat == cat).cloned().unwrap();
        let c = find("app:calc", "llm");
        assert_eq!((c.calls, c.input, c.output, c.errors, c.ms, c.mean_ms()), (3, 150, 30, 1, 700, 233), "a failed call counts as a call and an error, with no tokens");
        assert_eq!(find("app:calc", "tts").input, 40, "characters, not tokens");
        assert_eq!(find("assistant", "assistant").tokens(), 1500, "old-style records are the assistant's");
        assert_eq!(by.iter().map(|t| t.src.as_str()).collect::<Vec<_>>(), ["assistant", "app:calc", "app:calc", "mcp:files"], "biggest first: {by:?}");
        assert!(by.iter().all(|t| t.src != "app:other"));
        // one app, all kinds; the person filter; the range
        assert_eq!(l.totals_of(Some("amy"), "app:calc", None, now - DAY).calls, 4);
        assert_eq!(l.totals_of(Some("bob"), "app:calc", None, now - DAY).calls, 1, "another account's apps are not visible");
        assert_eq!(l.totals_of(Some("amy"), "app:calc", None, now - 5 * DAY).calls, 5);
        assert_eq!(l.totals_of(Some("amy"), "app:calc", Some("tts"), now - DAY).calls, 1, "one kind alone");
        // days: oldest first, the long name of each local day
        let days = l.days_of(Some("amy"), "app:calc", None, 7, now, &z);
        assert_eq!(days.len(), 7);
        assert_eq!(days[6].0, "Sun 11 Oct");
        assert_eq!((days[6].1.calls, days[3].1.calls), (4, 1), "{days:?}");
        assert_eq!(l.days_of(Some("amy"), "app:calc", None, 30, now, &z).len(), 30);
        assert_eq!(l.days_of(Some("amy"), "app:calc", None, 1, now, &z).len(), 1);
        // today's span in a time zone
        let (from, to) = UsageLog::today_span(now, &z);
        assert_eq!((from, to - from), (t(2026, 10, 11, 0), DAY));
        // the old charts still add everything up
        let w = l.series(Range::Week, now, &z, Some("amy"));
        assert_eq!(w.calls, 7);
        // merging keeps sources apart and adds errors and time
        let l2 = UsageLog::open(&l.path);
        let text = std::fs::read_to_string(&l2.path).unwrap();
        assert!(text.lines().count() <= 7, "{text}");
        let c2 = l2.totals_of(Some("amy"), "app:calc", None, now - DAY);
        assert_eq!((c2.calls, c2.errors, c2.ms), (4, 1, 1600));
    }

    #[test]
    fn the_assistants_calls_are_marked_and_timed() {
        use crate::chat::{ChatBackend, Message};
        struct Echo;
        impl ChatBackend for Echo {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                std::thread::sleep(std::time::Duration::from_millis(15));
                Ok("x".repeat(40))
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-usage-assistant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let net = std::sync::Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        let m = Metered::new(std::sync::Arc::new(Echo), svc.clone(), "ann");
        m.complete(&[Message::user("hello there")]).unwrap();
        let e = svc.usage.entries(Some("ann"));
        assert_eq!((e[0].src.as_str(), e[0].cat.as_str(), e[0].est, e[0].errors), ("assistant", "assistant", true, 0));
        assert!(e[0].ms >= 10, "{e:?}");
        let by = svc.usage.by_source(Some("ann"), 0);
        assert_eq!((by.len(), by[0].src.as_str(), by[0].calls), (1, "assistant", 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_usage_log_takes_the_providers_exact_counts_and_estimates_only_when_none_were_given() {
        use crate::chat::{ChatBackend, Message};
        struct Reporting(bool);
        impl ChatBackend for Reporting {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                // a provider client would have set these; this stands in for one
                if self.0 {
                    crate::providers::note_usage(&serde_json::json!({"usage": {"prompt_tokens": 1000, "completion_tokens": 77}}));
                }
                Ok("x".repeat(40))
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-usage-exact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let net = std::sync::Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        let exact = Metered::new(std::sync::Arc::new(Reporting(true)), svc.clone(), "ann");
        exact.complete(&[Message::user("hello there")]).unwrap();
        let guess = Metered::new(std::sync::Arc::new(Reporting(false)), svc.clone(), "ann");
        guess.complete(&[Message::user("hello there")]).unwrap();
        let ser = svc.usage.series(Range::Week, crate::clock::now_unix(), &Zone::utc(), Some("ann"));
        assert_eq!((ser.input, ser.output, ser.calls), (1000 + 6, 77 + 10, 2), "the first call is exact, the second estimated (about 4 characters a token)");
        let _ = std::fs::remove_dir_all(&d);
    }
}
