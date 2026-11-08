//! The usage facade: the ranges, the chart data and the source a USAGE tile reads. Plain data; the agent's `usage` module has the same
//! shapes and the shell converts between them.

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

/// Where the tile gets its numbers. The shell implements it with the agent's usage log (in the machine's time zone); tests use a fake.
pub trait UsageSource {
    /// Seconds since 1970 now.
    fn now_unix(&self) -> i64;
    /// The chart data for `range` ending at `now_unix`, for one person (`Some(name)`) or for everyone (`None`).
    fn series(&self, range: Range, now_unix: i64, user: Option<&str>) -> Series;
}
