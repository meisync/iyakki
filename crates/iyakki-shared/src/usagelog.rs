//! How many tokens each model has used, kept for a year so it can be charted by day, week, month or year.
//!
//! Every answer the assistant gets from a model is counted here (the text sent and the text received, at about four
//! characters per token, because not every provider reports exact usage). The log keeps one line per call and merges
//! them into hourly totals when the system starts, so it stays small. Only the totals are kept: never what was said.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use iyakki_clock::{civil, days_from_civil, Zone};

pub const KEEP_DAYS: i64 = 400;
pub const DAY: i64 = 86_400;

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
    pub path: PathBuf,
    lock: Mutex<usize>,
}

pub fn now() -> i64 {
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

