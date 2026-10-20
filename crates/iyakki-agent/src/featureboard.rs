//! The feature tracking board behind the AI IDE: reads a project's features file (a markdown table such as the
//! repository's `FEATURES.md`), lets rows be changed, and writes the file back changing only the cells and rows that
//! were touched. Everything else in the file stays byte for byte.
//!
//! Parsing is driven by the table's header, so other projects' tables work: columns may be in any order, an extra
//! `Blockers` column is used when present, and columns this code does not know are carried along untouched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SUMMARY_START: &str = "<!-- summary:start -->";
pub const SUMMARY_END: &str = "<!-- summary:end -->";
/// The statuses the repository's matrix uses, in lane order. Other statuses found in a file get lanes after these.
pub const STATUSES: [&str; 5] = ["planned", "partial", "done", "blocked", "deprecated"];
pub const PHASES: [&str; 11] = ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10"];
pub const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

/// Which table column holds what (indexes into a row's cells).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cols {
    pub id: usize,
    pub title: Option<usize>,
    pub phase: Option<usize>,
    pub status: usize,
    pub priority: Option<usize>,
    pub progress: Option<usize>,
    pub tests: Option<usize>,
    pub notes: Option<usize>,
    pub blockers: Option<usize>,
    /// Optional `Started` / `Finished` columns (UTC `YYYY-MM-DD HH:MM`); added on the first timestamp when a file lacks them.
    pub started: Option<usize>,
    pub finished: Option<usize>,
    /// Optional `Depends` column: what a `pending` row waits for (another row's id or a few words).
    pub depends: Option<usize>,
    /// Optional `Files` column: the source files (or globs) a feature lives in, for per-feature coverage.
    pub files: Option<usize>,
    /// Optional `Acceptance` column: free-text acceptance criteria (not test names).
    pub acceptance: Option<usize>,
    /// Optional bug columns (rows with a `B-` id): severity, how to reproduce, the folder of captured logs, when it was found, the fix.
    pub severity: Option<usize>,
    pub repro: Option<usize>,
    pub logs: Option<usize>,
    pub found: Option<usize>,
    pub fix: Option<usize>,
}

fn header_role(h: &str) -> &'static str {
    match h.trim().to_lowercase().trim_matches('*') {
        "id" | "key" | "ref" => "id",
        "feature" | "title" | "name" | "summary" | "description" | "item" => "title",
        "phase" | "milestone" | "stage" => "phase",
        "status" | "state" => "status",
        "pri" | "priority" | "prio" => "priority",
        "progress" | "done %" | "percent" | "%" => "progress",
        "tests" | "test" | "evidence" => "tests",
        "notes" | "comments" | "comment" | "note" | "remarks" => "notes",
        "blockers" | "blocker" | "blocked by" | "blocked" => "blockers",
        "acceptance" | "acceptance criteria" | "criteria" => "acceptance",
        "severity" | "sev" => "severity",
        "repro" | "reproduction" | "steps to reproduce" => "repro",
        "logs" | "log files" | "log" => "logs",
        "found" | "found at" | "reported" | "reported at" => "found",
        "fix" | "fixed by" | "fix / notes" => "fix",
        "files" | "source files" | "source" | "sources" | "code" => "files",
        "depends" | "depends on" | "depends_on" | "waiting for" | "waits for" | "dependency" | "dependencies" => "depends",
        "started" | "start" | "started at" | "began" => "started",
        "finished" | "finish" | "finished at" | "ended" | "completed at" => "finished",
        _ => "",
    }
}

/// Split a table line into its cell texts (`| a | b |` gives `a`, `b`) with the byte span of each cell between pipes.
fn spans(line: &str) -> Vec<(usize, usize)> {
    let t = line.trim_end_matches(['\r', ' ']);
    let b = t.as_bytes();
    let mut pipes = Vec::new();
    for (i, c) in b.iter().enumerate() {
        if *c == b'|' && (i == 0 || b[i - 1] != b'\\') {
            pipes.push(i);
        }
    }
    let mut out = Vec::new();
    for w in pipes.windows(2) {
        out.push((w[0] + 1, w[1]));
    }
    // a row that does not close with a pipe
    if let Some(last) = pipes.last() {
        if *last + 1 < t.len() {
            out.push((*last + 1, t.len()));
        }
    }
    out
}

fn cells_of(line: &str) -> Vec<String> {
    spans(line).into_iter().map(|(a, b)| line[a..b].trim().to_string()).collect()
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RowLine {
    /// Index in `lines`.
    line: usize,
    cells: Vec<String>,
}

/// A parsed features file. `lines` is the file; rows point into it.
#[derive(Clone, Debug)]
pub struct Board {
    lines: Vec<String>,
    final_newline: bool,
    cols: Option<Cols>,
    headers: Vec<String>,
    rows: Vec<RowLine>,
    /// Line of the separator under the header (rows are added after the last row, or here when there are none).
    sep_line: usize,
}

/// One feature as the board shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feature {
    pub id: String,
    pub title: String,
    pub phase: String,
    pub status: String,
    pub priority: String,
    /// 0..=100 when the table has a percentage.
    pub progress: Option<u32>,
    pub tests: Vec<String>,
    /// The Files cell: source files or globs, one per entry (empty when the table has no such column).
    pub files: Vec<String>,
    /// Acceptance criteria: the Acceptance column, plus Tests entries that are sentences, not test names. Never counted as tests.
    pub criteria: Vec<String>,
    /// Bug columns as written (empty when the table has none): severity (low, med, high), repro, the log folders or files, when found, the fix.
    pub severity: String,
    pub repro: String,
    pub logs: Vec<String>,
    pub found: String,
    pub fix: String,
    pub notes: String,
    /// Text of the Blockers column (empty when there is none or it holds nothing).
    pub blockers_cell: String,
    /// The Started / Finished cells as written (empty when the table has none).
    pub started: String,
    pub finished: String,
    /// The Depends cell as written (empty when the table has none).
    pub depends_cell: String,
    /// Every cell, in file order, for the detail view.
    pub cells: Vec<String>,
}

impl Feature {
    /// The area prefix of an id (`F-BROW-03` gives `BROW`; ids of other shapes give the text before the last dash).
    pub fn area(&self) -> String {
        area_of(&self.id)
    }

    /// What blocks this feature: the Blockers column when the table has one, otherwise sentences of the notes that say
    /// "Blocked by", "Depends on" or "Waiting for", and for a blocked row with none of those, the first sentence of its notes.
    pub fn blockers(&self) -> Vec<String> {
        if !self.blockers_cell.is_empty() {
            return self.blockers_cell.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        let mut out = Vec::new();
        for s in sentences(&self.notes) {
            let l = s.to_lowercase();
            if l.contains("blocked by") || l.contains("depends on") || l.contains("waiting for") {
                out.push(s);
            }
        }
        if out.is_empty() && self.status == "blocked" && !self.notes.is_empty() {
            out.push(sentences(&self.notes).into_iter().next().unwrap_or_default());
        }
        out
    }

    /// The title cut to its first sentence (the cards show this; the drawer shows all of it).
    pub fn short_title(&self) -> String {
        sentences(&self.title).into_iter().next().unwrap_or_else(|| self.title.clone())
    }

    /// The problems the repository's own rules (`feature_matrix.rs`) would find in this row.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        let p = self.progress;
        match self.status.as_str() {
            "done" => {
                if p.is_some_and(|p| p != 100) {
                    w.push("done should be 100%".into());
                }
                if self.tests.is_empty() {
                    w.push("done needs at least one test listed".into());
                }
            }
            "planned" if p.is_some_and(|p| p != 0) => w.push("planned should be 0%".into()),
            "partial" => {
                if p.is_some_and(|p| !(1..=99).contains(&p)) {
                    w.push("partial should be 1%..99%".into());
                }
                if self.tests.is_empty() {
                    w.push("partial needs at least one test listed".into());
                }
                if self.notes.is_empty() {
                    w.push("partial needs a comment saying what is left".into());
                }
            }
            "blocked" => {
                if self.notes.is_empty() && self.blockers_cell.is_empty() {
                    w.push("blocked needs a comment or a blocker".into());
                }
                if p == Some(100) {
                    w.push("blocked cannot be 100%".into());
                }
            }
            "deprecated" if self.notes.is_empty() => w.push("deprecated needs a comment naming its replacement".into()),
            _ => {}
        }
        w
    }
}

pub fn area_of(id: &str) -> String {
    let t = id.strip_prefix("F-").or_else(|| id.strip_prefix("B-")).unwrap_or(id);
    match t.rsplit_once('-') {
        Some((a, _)) => a.to_string(),
        None => t.to_string(),
    }
}

/// Sentences of a note: cut after `. ` or `; ` (and `<br>`); a trailing full stop is dropped.
pub fn sentences(text: &str) -> Vec<String> {
    let t = text.replace("<br>", ". ");
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        cur.push(c);
        let end = (c == '.' || c == ';') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if end {
            let s = cur.trim().trim_end_matches(['.', ';']).trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
        i += 1;
    }
    let s = cur.trim().trim_end_matches(['.', ';']).trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

/// What to change on a row. `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub status: Option<String>,
    pub phase: Option<String>,
    pub priority: Option<String>,
    pub progress: Option<u32>,
    pub title: Option<String>,
    /// Replaces the notes cell.
    pub comment: Option<String>,
    /// Added to the blockers (see [`Board::add_blocker`]).
    pub blocker: Option<String>,
    pub tests: Option<Vec<String>>,
    /// The source files (or globs) of the feature: written to the Files column, which is added when the table has none.
    pub files: Option<Vec<String>>,
    /// Acceptance criteria (free text), written to the Acceptance column, which is added when the table has none.
    pub acceptance: Option<Vec<String>>,
    /// Bug fields: written to their columns, which are added when the table has none.
    pub severity: Option<String>,
    pub repro: Option<String>,
    pub logs: Option<Vec<String>>,
    pub found: Option<String>,
    pub fix: Option<String>,
    /// The time of this change (unix seconds). When set, a status change also writes Started / Finished (see [`Board::update`]).
    pub at: Option<i64>,
    /// What a `pending` row waits for: written to the Depends column, or as `Depends: …;` in front of the notes.
    pub depends: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewFeature {
    pub id: String,
    pub title: String,
    pub phase: String,
    pub status: String,
    pub priority: String,
    pub notes: String,
}

/// Does this look like the name or path of a test (`crate::mod::test_name`, `tests/a.py::test_b`, `TestAdd`, `cart.test.js`, `test_cart`),
/// rather than a sentence of acceptance criteria ("Backend deployment should be created successfully.")?
pub fn is_test_name(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if !t.contains(char::is_whitespace) {
        return true;
    }
    // a name with spaces (a jest or vitest title): code-like marks, or a short lowercase phrase that does not read as a sentence
    if t.contains("::") || t.contains('/') || t.contains('_') || t.contains('(') || t.contains(" > ") {
        return true;
    }
    let words = t.split_whitespace().count();
    !(t.ends_with('.') || t.chars().next().is_some_and(char::is_uppercase) || words >= 6)
}

/// Entries of a list cell: separated by `<br>`, commas, semicolons or newlines; backticks and spaces trimmed; blanks dropped.
pub fn split_list(cell: &str) -> Vec<String> {
    cell.split(|c| matches!(c, ',' | ';' | '\n')).flat_map(|p| p.split("<br>")).map(|t| t.trim().trim_matches('`').trim().to_string()).filter(|t| !t.is_empty() && !matches!(t.as_str(), "—" | "-" | "–")).collect()
}

/// The notes without a leading `Depends: …;` clause.
fn strip_depends(notes: &str) -> String {
    match notes.strip_prefix("Depends:") {
        Some(r) => r.split_once(';').map_or(String::new(), |x| x.1.trim().to_string()),
        None => notes.to_string(),
    }
}

/// UTC `YYYY-MM-DD HH:MM` for unix seconds (what the Started and Finished cells hold).
pub fn stamp_text(unix: i64) -> String {
    crate::clock::format(&crate::clock::Zone::utc(), unix, "%Y-%m-%d %H:%M")
}

/// Unix seconds of a cell written by [`stamp_text`] (also `T` between date and time, and seconds); None for anything else.
pub fn parse_stamp(t: &str) -> Option<i64> {
    let t = t.trim().trim_end_matches('Z');
    let (d, h) = t.split_once([' ', 'T'])?;
    let mut dp = d.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (dp.next()??, dp.next()??, dp.next()??);
    let mut hp = h.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm) = (hp.next()??, hp.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || !(0..24).contains(&hh) || !(0..60).contains(&mm) {
        return None;
    }
    Some(crate::clock::days_from_civil(y, m as u32, day as u32) * 86400 + hh * 3600 + mm * 60)
}

/// "under a minute", "3 min", "1 h 05 min", "2 d 3 h".
pub fn fmt_duration(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0 => "under a minute".into(),
        1..=59 => format!("{m} min"),
        60..=1439 => format!("{} h {:02} min", m / 60, m % 60),
        _ => format!("{} d {} h", m / 1440, (m % 1440) / 60),
    }
}

impl Feature {
    /// A bug: its id starts with `B-`.
    pub fn is_bug(&self) -> bool {
        self.id.starts_with("B-")
    }

    /// How many acceptance criteria the row lists.
    pub fn criteria_count(&self) -> usize {
        self.criteria.len()
    }

    /// How many tests the row lists.
    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    /// What this row waits for: the Depends column, else a leading `Depends: …;` in the notes.
    pub fn depends(&self) -> String {
        if !self.depends_cell.is_empty() {
            return self.depends_cell.clone();
        }
        self.notes.strip_prefix("Depends:").and_then(|r| r.split(';').next()).map(|x| x.trim().to_string()).unwrap_or_default()
    }

    /// Seconds spent: Started to Finished, or to `now` while the row is partial; None when it has not started or has no end.
    pub fn duration(&self, now: i64) -> Option<i64> {
        let s = parse_stamp(&self.started)?;
        let e = match parse_stamp(&self.finished) {
            Some(e) => e,
            None if self.status == "partial" => now,
            None => return None,
        };
        Some((e - s).max(0))
    }
}

/// A cell's text as it may be written into a table.
pub fn cell_text(t: &str) -> String {
    t.replace(['\n', '\r'], " ").replace('|', "/").trim().to_string()
}

impl Board {
    pub fn parse(text: &str) -> Board {
        let final_newline = text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        let lines: Vec<String> = if text.is_empty() { Vec::new() } else { body.split('\n').map(str::to_string).collect() };
        let mut b = Board { lines, final_newline, cols: None, headers: Vec::new(), rows: Vec::new(), sep_line: 0 };
        b.reparse();
        b
    }

    fn reparse(&mut self) {
        self.cols = None;
        self.headers.clear();
        self.rows.clear();
        let n = self.lines.len();
        let mut i = 0;
        while i + 1 < n {
            if self.lines[i].trim_start().starts_with('|') && is_separator(&self.lines[i + 1]) {
                let headers = cells_of(&self.lines[i]);
                let mut cols = Cols::default();
                let (mut id, mut status) = (None, None);
                for (c, h) in headers.iter().enumerate() {
                    match header_role(h) {
                        "id" if id.is_none() => id = Some(c),
                        "title" if cols.title.is_none() => cols.title = Some(c),
                        "phase" if cols.phase.is_none() => cols.phase = Some(c),
                        "status" if status.is_none() => status = Some(c),
                        "priority" if cols.priority.is_none() => cols.priority = Some(c),
                        "progress" if cols.progress.is_none() => cols.progress = Some(c),
                        "tests" if cols.tests.is_none() => cols.tests = Some(c),
                        "notes" if cols.notes.is_none() => cols.notes = Some(c),
                        "blockers" if cols.blockers.is_none() => cols.blockers = Some(c),
                        "acceptance" if cols.acceptance.is_none() => cols.acceptance = Some(c),
                        "severity" if cols.severity.is_none() => cols.severity = Some(c),
                        "repro" if cols.repro.is_none() => cols.repro = Some(c),
                        "logs" if cols.logs.is_none() => cols.logs = Some(c),
                        "found" if cols.found.is_none() => cols.found = Some(c),
                        "fix" if cols.fix.is_none() => cols.fix = Some(c),
                        "files" if cols.files.is_none() => cols.files = Some(c),
                        "depends" if cols.depends.is_none() => cols.depends = Some(c),
                        "started" if cols.started.is_none() => cols.started = Some(c),
                        "finished" if cols.finished.is_none() => cols.finished = Some(c),
                        _ => {}
                    }
                }
                if let (Some(idc), Some(sc)) = (id, status) {
                    cols.id = idc;
                    cols.status = sc;
                    self.cols = Some(cols);
                    self.headers = headers;
                    self.sep_line = i + 1;
                    let mut j = i + 2;
                    while j < n && self.lines[j].trim_start().starts_with('|') && !is_separator(&self.lines[j]) {
                        let cells = cells_of(&self.lines[j]);
                        if cells.get(idc).is_some_and(|c| !c.is_empty()) {
                            self.rows.push(RowLine { line: j, cells });
                        }
                        j += 1;
                    }
                    return;
                }
            }
            i += 1;
        }
    }

    /// The file text (equal to the parsed text when nothing was changed).
    pub fn render(&self) -> String {
        let mut s = self.lines.join("\n");
        if self.final_newline {
            s.push('\n');
        }
        s
    }

    pub fn has_table(&self) -> bool {
        self.cols.is_some()
    }
    pub fn cols(&self) -> Option<&Cols> {
        self.cols.as_ref()
    }
    pub fn headers(&self) -> &[String] {
        &self.headers
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn feature_of(&self, r: &RowLine) -> Feature {
        let c = self.cols.clone().unwrap_or_default();
        let get = |i: Option<usize>| i.and_then(|i| r.cells.get(i)).cloned().unwrap_or_default();
        let blank = |s: String| if matches!(s.as_str(), "—" | "-" | "–" | "none" | "n/a") { String::new() } else { s };
        let tests_cell = blank(get(c.tests));
        let (tests, mut criteria): (Vec<String>, Vec<String>) = split_list(&tests_cell).into_iter().partition(|t| is_test_name(t));
        criteria.extend(split_list(&blank(get(c.acceptance))));
        let files: Vec<String> = split_list(&blank(get(c.files)));
        let progress = blank(get(c.progress)).trim().trim_end_matches('%').trim().parse::<u32>().ok();
        Feature {
            id: r.cells[c.id].clone(),
            title: get(c.title),
            phase: get(c.phase),
            status: r.cells.get(c.status).cloned().unwrap_or_default().to_lowercase(),
            priority: get(c.priority),
            progress,
            tests,
            files,
            criteria,
            severity: blank(get(c.severity)).to_lowercase(),
            repro: blank(get(c.repro)),
            logs: split_list(&blank(get(c.logs))),
            found: blank(get(c.found)),
            fix: blank(get(c.fix)),
            notes: blank(get(c.notes)),
            blockers_cell: blank(get(c.blockers)),
            depends_cell: blank(get(c.depends)),
            started: blank(get(c.started)),
            finished: blank(get(c.finished)),
            cells: r.cells.clone(),
        }
    }

    pub fn features(&self) -> Vec<Feature> {
        self.rows.iter().map(|r| self.feature_of(r)).collect()
    }

    pub fn get(&self, id: &str) -> Option<Feature> {
        self.rows.iter().find(|r| self.cols.as_ref().is_some_and(|c| r.cells[c.id] == id)).map(|r| self.feature_of(r))
    }

    /// Statuses in lane order: the known ones that occur (or all five when `all`), then others in the order found.
    pub fn statuses(&self, all: bool) -> Vec<String> {
        let found: Vec<String> = self.features().into_iter().map(|f| f.status).collect();
        let mut out: Vec<String> = STATUSES.iter().filter(|s| all || found.iter().any(|f| f == *s)).map(|s| s.to_string()).collect();
        for f in found {
            if !out.contains(&f) && !f.is_empty() {
                out.push(f);
            }
        }
        out
    }

    pub fn count_by_status(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for f in self.features() {
            *m.entry(f.status).or_insert(0) += 1;
        }
        m
    }

    /// The share of features that are done, 0..=100 (deprecated rows do not count either way).
    pub fn percent_done(&self) -> u32 {
        let fs = self.features();
        let live: Vec<&Feature> = fs.iter().filter(|f| f.status != "deprecated").collect();
        if live.is_empty() {
            return 0;
        }
        (live.iter().filter(|f| f.status == "done").count() * 100 / live.len()) as u32
    }

    /// The totals text for the summary markers, in the order the file already uses (so a file with fewer statuses keeps its form).
    pub fn totals_text(&self) -> String {
        let counts = self.count_by_status();
        let order = self.summary_order();
        order.iter().map(|k| if k == "total" { format!("total {}", self.rows.len()) } else { format!("{k} {}", counts.get(k).copied().unwrap_or(0)) }).collect::<Vec<_>>().join(" · ")
    }

    fn summary_span(&self) -> Option<(usize, usize, usize)> {
        for (n, l) in self.lines.iter().enumerate() {
            if let (Some(a), Some(b)) = (l.find(SUMMARY_START), l.find(SUMMARY_END)) {
                if a + SUMMARY_START.len() <= b {
                    return Some((n, a + SUMMARY_START.len(), b));
                }
            }
        }
        None
    }

    fn summary_order(&self) -> Vec<String> {
        if let Some((n, a, b)) = self.summary_span() {
            let words: Vec<String> = self.lines[n][a..b].split(" · ").filter_map(|p| p.split_whitespace().next().map(str::to_string)).collect();
            if !words.is_empty() {
                return words;
            }
        }
        let mut v: Vec<String> = ["done", "partial", "planned", "blocked", "deprecated"].iter().map(|s| s.to_string()).collect();
        v.push("total".into());
        v
    }

    /// The totals line as the file has it now (without the markers).
    pub fn totals_in_file(&self) -> Option<String> {
        self.summary_span().map(|(n, a, b)| self.lines[n][a..b].to_string())
    }

    /// Bring the totals line (between the summary markers) and a `N features done, N partial, N planned` sentence in
    /// sync with the rows, the way the repository's `fixtotals` script does. Returns whether anything changed.
    pub fn sync_totals(&mut self) -> bool {
        let mut changed = false;
        let want = self.totals_text();
        if let Some((n, a, b)) = self.summary_span() {
            if self.lines[n][a..b] != want {
                let l = &self.lines[n];
                self.lines[n] = format!("{}{}{}", &l[..a], want, &l[b..]);
                changed = true;
            }
        }
        let counts = self.count_by_status();
        let c = |k: &str| counts.get(k).copied().unwrap_or(0);
        let want2 = format!("{} features done, {} partial, {} planned", c("done"), c("partial"), c("planned"));
        for l in self.lines.iter_mut() {
            if let Some(new) = replace_readme_totals(l, &want2) {
                if new != *l {
                    *l = new;
                    changed = true;
                }
            }
        }
        changed
    }

    fn row_index(&self, id: &str) -> Result<usize, String> {
        let c = self.cols.as_ref().ok_or("this file has no feature table")?;
        self.rows.iter().position(|r| r.cells[c.id] == id).ok_or_else(|| format!("no feature {id}"))
    }

    /// Set one cell of a row, keeping the row's column padding when it had any.
    fn write_cell(&mut self, row: usize, col: usize, value: &str) -> Result<(), String> {
        let value = cell_text(value);
        let li = self.rows[row].line;
        let line = self.lines[li].clone();
        let sp = spans(&line);
        let (a, b) = *sp.get(col).ok_or_else(|| format!("row has no column {col}"))?;
        let seg = &line[a..b];
        // an aligned table (every cell of the column, header included, is padded to one width) stays aligned
        let width = |l: &str| spans(l).get(col).map(|(x, y)| l[*x..*y].chars().count());
        let w0 = seg.chars().count();
        let aligned = self.rows.len() >= 2 && self.rows.iter().all(|r| width(&self.lines[r.line]) == Some(w0)) && self.lines.get(self.sep_line.saturating_sub(1)).and_then(|h| width(h)) == Some(w0);
        let mut new_seg = format!(" {value} ");
        if aligned {
            while new_seg.chars().count() < w0 {
                new_seg.push(' ');
            }
        }
        let mut out = String::new();
        out.push_str(&line[..a]);
        out.push_str(&new_seg);
        out.push_str(&line[b..]);
        self.lines[li] = out;
        let cells = cells_of(&self.lines[li]);
        self.rows[row].cells = cells;
        Ok(())
    }

    /// Change fields of a feature. Moving to `done` sets 100%, to `planned` sets 0% and to `partial` keeps a value in
    /// 1..99 (else 50%) when the table has a percentage. Returns the warnings the row has afterwards (nothing is refused
    /// for them; the board asks the person for a comment first).
    pub fn update(&mut self, id: &str, u: &Update) -> Result<Vec<String>, String> {
        let row = self.row_index(id)?;
        let c = self.cols.clone().ok_or("no table")?;
        let mut c = c;
        if let Some(st) = &u.status {
            let st = st.trim().to_lowercase();
            if st.is_empty() || st.contains('|') {
                return Err("a status is one word".into());
            }
            let before = self.feature_of(&self.rows[row].clone()).status;
            self.write_cell(row, c.status, &st)?;
            if let Some(at) = u.at {
                self.stamp_transition(row, &before, &st, at)?;
                c = self.cols.clone().ok_or("no table")?;
            }
            if let Some(pc) = c.progress {
                let cur = self.feature_of(&self.rows[row].clone()).progress;
                let new = match st.as_str() {
                    "done" => Some(100),
                    "planned" => Some(0),
                    "partial" if !cur.is_some_and(|p| (1..=99).contains(&p)) => Some(50),
                    "blocked" if cur == Some(100) => Some(90),
                    _ => None,
                };
                if let (Some(p), true) = (new, u.progress.is_none()) {
                    self.write_cell(row, pc, &format!("{p}%"))?;
                }
            }
        }
        if let Some(p) = &u.phase {
            let col = c.phase.ok_or("this table has no phase column")?;
            self.write_cell(row, col, p)?;
        }
        if let Some(p) = &u.priority {
            let col = c.priority.ok_or("this table has no priority column")?;
            self.write_cell(row, col, p)?;
        }
        if let Some(p) = u.progress {
            let col = c.progress.ok_or("this table has no progress column")?;
            self.write_cell(row, col, &format!("{}%", p.min(100)))?;
        }
        if let Some(t) = &u.title {
            let col = c.title.ok_or("this table has no title column")?;
            self.write_cell(row, col, t)?;
        }
        if let Some(t) = &u.tests {
            let col = match c.tests {
                Some(i) => i,
                None => self.ensure_col("Tests", |c| c.tests)?,
            };
            let v = if t.is_empty() { "—".to_string() } else { t.iter().map(|x| format!("`{}`", x.trim_matches('`'))).collect::<Vec<_>>().join("<br>") };
            self.write_cell(row, col, &v)?;
            c = self.cols.clone().ok_or("no table")?;
        }
        for (name, val, pick) in [
            ("Severity", u.severity.clone(), (|c: &Cols| c.severity) as fn(&Cols) -> Option<usize>),
            ("Repro", u.repro.clone(), |c: &Cols| c.repro),
            ("Logs", u.logs.as_ref().map(|l| l.iter().map(|x| x.trim().to_string()).collect::<Vec<_>>().join(", ")), |c: &Cols| c.logs),
            ("Found", u.found.clone(), |c: &Cols| c.found),
            ("Fix", u.fix.clone(), |c: &Cols| c.fix),
        ] {
            if let Some(v) = val {
                let col = match pick(&c) {
                    Some(i) => i,
                    None => self.ensure_col(name, pick)?,
                };
                self.write_cell(row, col, &if v.trim().is_empty() { "—".to_string() } else { v })?;
                c = self.cols.clone().ok_or("no table")?;
            }
        }
        if let Some(ac) = &u.acceptance {
            let col = match c.acceptance {
                Some(i) => i,
                None => self.ensure_col("Acceptance", |c| c.acceptance)?,
            };
            let v = if ac.is_empty() { "—".to_string() } else { ac.iter().map(|x| x.trim().to_string()).collect::<Vec<_>>().join("<br>") };
            self.write_cell(row, col, &v)?;
            c = self.cols.clone().ok_or("no table")?;
        }
        if let Some(fl) = &u.files {
            let col = match c.files {
                Some(i) => i,
                None => self.ensure_col("Files", |c| c.files)?,
            };
            let v = if fl.is_empty() { "—".to_string() } else { fl.iter().map(|x| x.trim().to_string()).collect::<Vec<_>>().join(", ") };
            self.write_cell(row, col, &v)?;
            c = self.cols.clone().ok_or("no table")?;
        }
        if let Some(t) = &u.comment {
            let col = c.notes.ok_or("this table has no notes column")?;
            self.write_cell(row, col, t)?;
        }
        if let Some(t) = &u.blocker {
            self.add_blocker(id, t)?;
        }
        if let Some(d) = &u.depends {
            let d = cell_text(d);
            // row ids get a column of their own (the Board's Table and Graph views read it); words stay in the notes of a table without one
            if c.depends.is_none() && !crate::boarddeps::parse_ids(&d).is_empty() {
                self.ensure_col("Depends", |c| c.depends)?;
                c = self.cols.clone().ok_or("no table")?;
            }
            if let Some(dc) = c.depends {
                self.write_cell(row, dc, &d)?;
            } else if let Some(nc) = c.notes {
                let f = self.feature_of(&self.rows[row].clone());
                let rest = strip_depends(&f.notes);
                let v = if d.is_empty() { rest } else if rest.is_empty() { format!("Depends: {d};") } else { format!("Depends: {d}; {rest}") };
                self.write_cell(row, nc, &v)?;
            } else {
                return Err("this table has neither a Depends nor a notes column".into());
            }
        }
        Ok(self.get(id).map(|f| f.warnings()).unwrap_or_default())
    }

    /// Started when a row first becomes partial (or done without having started), Finished when it becomes done; both
    /// cleared when it goes back to planned, Finished cleared when it leaves done. The columns are added the first time
    /// a value has to be written; what a person typed into them is kept unless the row goes back.
    fn stamp_transition(&mut self, row: usize, before: &str, now_status: &str, at: i64) -> Result<(), String> {
        let stamp = stamp_text(at);
        let f = self.feature_of(&self.rows[row].clone());
        let (has_s, has_f) = (!f.started.is_empty(), !f.finished.is_empty());
        let (set_start, set_finish, clear_start, clear_finish) = match now_status {
            "partial" => (!has_s, false, false, before == "done"),
            "done" => (!has_s, !has_f || before != "done", false, false),
            "planned" => (false, false, has_s, has_f),
            _ => (false, false, false, before == "done" && has_f),
        };
        if set_start || set_finish {
            self.ensure_time_cols()?;
        }
        let c = self.cols.clone().ok_or("no table")?;
        if let Some(i) = c.started {
            if set_start {
                self.write_cell(row, i, &stamp)?;
            } else if clear_start {
                self.write_cell(row, i, "")?;
            }
        }
        if let Some(i) = c.finished {
            if set_finish {
                self.write_cell(row, i, &stamp)?;
            } else if clear_finish {
                self.write_cell(row, i, "")?;
            }
        }
        Ok(())
    }

    /// A column the table lacks, added (before Notes, else at the end); returns its index.
    fn ensure_col(&mut self, name: &str, pick: impl Fn(&Cols) -> Option<usize>) -> Result<usize, String> {
        let c = self.cols.clone().ok_or("this file has no feature table")?;
        if let Some(i) = pick(&c) {
            return Ok(i);
        }
        let at = c.notes.unwrap_or(self.headers.len());
        self.insert_column(at, name);
        self.cols.as_ref().and_then(pick).ok_or_else(|| format!("the {name} column could not be added"))
    }

    /// Add the Started and Finished columns (before Notes, else at the end) when the table lacks them.
    pub fn ensure_time_cols(&mut self) -> Result<(), String> {
        let c = self.cols.clone().ok_or("this file has no feature table")?;
        if c.started.is_none() {
            let at = c.notes.unwrap_or(self.headers.len());
            self.insert_column(at, "Started");
        }
        let c = self.cols.clone().ok_or("no table")?;
        if c.finished.is_none() {
            let at = c.started.map_or(self.headers.len(), |i| i + 1);
            self.insert_column(at, "Finished");
        }
        Ok(())
    }

    fn insert_column(&mut self, at: usize, name: &str) {
        let ins = |line: &str, text: &str| -> String {
            let mut line = line.trim_end_matches(['\r', ' ']).to_string();
            let sp = spans(&line);
            if at < sp.len() {
                let p = sp[at].0;
                format!("{}{}|{}", &line[..p], text, &line[p..])
            } else {
                if !line.ends_with('|') {
                    line.push('|');
                }
                for _ in sp.len()..at {
                    line.push_str(" |");
                }
                format!("{line}{text}|")
            }
        };
        let header = self.sep_line.saturating_sub(1);
        let mut idx: Vec<usize> = vec![header, self.sep_line];
        idx.extend(self.rows.iter().map(|r| r.line));
        for (n, li) in idx.into_iter().enumerate() {
            let text = match n {
                0 => format!(" {name} "),
                1 => "---".to_string(),
                _ => " ".to_string(),
            };
            self.lines[li] = ins(&self.lines[li], &text);
        }
        self.reparse();
    }

    /// Record a blocker in the file's own convention: the Blockers column when the table has one, else
    /// `Blocked by: …;` in front of the notes.
    pub fn add_blocker(&mut self, id: &str, text: &str) -> Result<(), String> {
        let text = cell_text(text);
        if text.is_empty() {
            return Err("say what blocks it".into());
        }
        let row = self.row_index(id)?;
        let c = self.cols.clone().ok_or("no table")?;
        let f = self.feature_of(&self.rows[row].clone());
        if let Some(bc) = c.blockers {
            let v = if f.blockers_cell.is_empty() { text } else { format!("{}; {}", f.blockers_cell, text) };
            self.write_cell(row, bc, &v)
        } else {
            let nc = c.notes.ok_or("this table has neither a blockers nor a notes column")?;
            let v = if f.notes.is_empty() { format!("Blocked by: {text};") } else { format!("Blocked by: {text}; {}", f.notes) };
            self.write_cell(row, nc, &v)
        }
    }

    /// Clear the blockers (the Blockers column, or the `Blocked by …;` sentences of the notes).
    pub fn clear_blockers(&mut self, id: &str) -> Result<(), String> {
        let row = self.row_index(id)?;
        let c = self.cols.clone().ok_or("no table")?;
        let f = self.feature_of(&self.rows[row].clone());
        if let Some(bc) = c.blockers {
            return self.write_cell(row, bc, "");
        }
        let nc = c.notes.ok_or("no notes column")?;
        let keep: Vec<String> = sentences(&f.notes).into_iter().filter(|s| {
            let l = s.to_lowercase();
            !(l.contains("blocked by") || l.contains("depends on") || l.contains("waiting for"))
        }).collect();
        self.write_cell(row, nc, &keep.join(". "))
    }

    /// The next free id for an area: `F-BROW` gives `F-BROW-08` after `F-BROW-07`.
    pub fn next_id(&self, prefix: &str) -> String {
        self.next_id_of(prefix, 'F')
    }

    /// The next bug id: `B-<area>-NN`, numbered separately from the features of the same area.
    pub fn next_bug_id(&self, area: &str) -> String {
        self.next_id_of(area, 'B')
    }

    fn next_id_of(&self, prefix: &str, kind: char) -> String {
        let prefix = prefix.trim().trim_end_matches('-');
        let prefix = prefix.strip_prefix("F-").or_else(|| prefix.strip_prefix("B-")).unwrap_or(prefix);
        let prefix = format!("{kind}-{prefix}");
        let max = self.rows.iter().filter_map(|r| {
            let id = &self.cols.as_ref()?.id;
            let cell = &r.cells[*id];
            cell.strip_prefix(&format!("{prefix}-")).and_then(|n| n.parse::<u32>().ok())
        }).max().unwrap_or(0);
        format!("{prefix}-{:02}", max + 1)
    }

    /// The area prefixes in use, most used first.
    pub fn areas(&self) -> Vec<String> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for f in self.features() {
            *m.entry(f.area()).or_insert(0) += 1;
        }
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|(a, _)| a).collect()
    }

    /// Add a row after the last one. Cells the table has but this does not set stay empty (progress follows the status).
    pub fn add(&mut self, f: &NewFeature) -> Result<(), String> {
        let c = self.cols.clone().ok_or("this file has no feature table")?;
        let id = cell_text(&f.id);
        if id.is_empty() {
            return Err("a feature needs an id".into());
        }
        if self.get(&id).is_some() {
            return Err(format!("{id} already exists"));
        }
        if cell_text(&f.title).is_empty() && c.title.is_some() {
            return Err("a feature needs a title".into());
        }
        let n = self.headers.len();
        let mut cells = vec![String::new(); n];
        cells[c.id] = id;
        let status = if f.status.trim().is_empty() { "planned".to_string() } else { cell_text(&f.status).to_lowercase() };
        cells[c.status] = status.clone();
        if let Some(i) = c.title {
            cells[i] = cell_text(&f.title);
        }
        if let Some(i) = c.phase {
            cells[i] = if f.phase.is_empty() { "P5".into() } else { cell_text(&f.phase) };
        }
        if let Some(i) = c.priority {
            cells[i] = if f.priority.is_empty() { "P2".into() } else { cell_text(&f.priority) };
        }
        if let Some(i) = c.progress {
            cells[i] = match status.as_str() {
                "done" => "100%".into(),
                "partial" => "50%".into(),
                _ => "0%".into(),
            };
        }
        if let Some(i) = c.tests {
            cells[i] = "—".into();
        }
        if let Some(i) = c.notes {
            cells[i] = cell_text(&f.notes);
        }
        let line = format!("| {} |", cells.join(" | "));
        let at = self.rows.last().map(|r| r.line + 1).unwrap_or(self.sep_line + 1);
        self.lines.insert(at, line);
        self.reparse();
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let row = self.row_index(id)?;
        let li = self.rows[row].line;
        self.lines.remove(li);
        self.reparse();
        Ok(())
    }

    /// Row-level merge after the file changed on disk while the board had edits: start from `theirs`, then lay every row
    /// that differs between `base` (what the board loaded) and `mine` on top (rows I deleted are deleted, rows I added are added).
    pub fn merge(base: &Board, mine: &Board, theirs_text: &str) -> Board {
        let mut out = Board::parse(theirs_text);
        let Some(cols) = mine.cols.clone() else { return out };
        let base_ids: Vec<String> = base.features().into_iter().map(|f| f.id).collect();
        for f in mine.features() {
            let before = base.get(&f.id);
            if before.as_ref() == Some(&f) {
                continue;
            }
            match out.row_index(&f.id) {
                Ok(r) => {
                    for (ci, v) in f.cells.iter().enumerate() {
                        if before.as_ref().is_none_or(|b| b.cells.get(ci) != Some(v)) && ci < out.headers.len() {
                            let _ = out.write_cell(r, ci, v);
                        }
                    }
                }
                Err(_) => {
                    let line = mine.lines[mine.rows.iter().find(|r| r.cells[cols.id] == f.id).map(|r| r.line).unwrap_or(0)].clone();
                    let at = out.rows.last().map(|r| r.line + 1).unwrap_or(out.sep_line + 1);
                    out.lines.insert(at, line);
                    out.reparse();
                }
            }
        }
        for id in base_ids {
            if mine.get(&id).is_none() {
                let _ = out.delete(&id);
            }
        }
        out.sync_totals();
        out
    }
}

/// `N features done, N partial, N planned` replaced inside a line.
fn replace_readme_totals(line: &str, new: &str) -> Option<String> {
    let i = line.find(" features done, ")?;
    // digits before
    let before = &line[..i];
    let start = before.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |p| p + 1);
    if start == before.len() {
        return None;
    }
    let rest = &line[i + " features done, ".len()..];
    let d1 = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if d1 == 0 || !rest[d1..].starts_with(" partial, ") {
        return None;
    }
    let rest2 = &rest[d1 + " partial, ".len()..];
    let d2 = rest2.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if d2 == 0 || !rest2[d2..].starts_with(" planned") {
        return None;
    }
    let end = i + " features done, ".len() + d1 + " partial, ".len() + d2 + " planned".len();
    Some(format!("{}{}{}", &line[..start], new, &line[end..]))
}

// ------------------------------------------------------------------------------------------------ the file on disk

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Write a file atomically (temp file, then rename), keeping the old content as `<name>.bak` for one step of undo.
pub fn write_with_backup(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().ok_or("no parent folder")?;
    let name = path.file_name().ok_or("no file name")?.to_string_lossy().into_owned();
    if let Ok(old) = std::fs::read(path) {
        let bak = dir.join(format!("{name}.bak"));
        std::fs::write(&bak, old).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    let tmp = dir.join(format!(".{name}.iyakki-tmp"));
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o644);
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(|e| format!("{}: {e}", path.display()))
}

/// What the board knows about the file it loaded, to notice changes made by anyone else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub mtime_ns: u128,
    pub size: u64,
    pub hash: u64,
}

pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    let mtime_ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(Stamp { mtime_ns, size: m.len(), hash: 0 })
}

/// The features file of a project: its text as loaded, and what it looked like on disk when it was.
pub struct BoardFile {
    pub path: PathBuf,
    pub board: Board,
    /// The board as loaded or last saved (the base for a merge).
    pub saved: Board,
    stamp: Option<Stamp>,
    saved_hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disk {
    Same,
    /// Someone else wrote the file.
    Changed,
    Gone,
}

/// A board with the columns and no rows: what the first `board_add` of a workspace without a features file starts from.
pub const EMPTY_BOARD: &str = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 0 · blocked 0 · deprecated 0 · total 0<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Started | Finished | Notes |\n|---|---|---|---|---|---|---|\n";

impl BoardFile {
    /// A board that exists only in memory until [`BoardFile::save`] writes it (the file is created then).
    pub fn new_empty(path: &Path) -> BoardFile {
        let b = Board::parse(EMPTY_BOARD);
        BoardFile { path: path.into(), saved: b.clone(), board: b, stamp: None, saved_hash: 0 }
    }

    pub fn load(path: &Path) -> Result<BoardFile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if text.len() > 4 * 1024 * 1024 {
            return Err("the features file is too large".into());
        }
        let b = Board::parse(&text);
        let mut stamp = stamp_of(path);
        if let Some(s) = stamp.as_mut() {
            s.hash = fnv(text.as_bytes());
        }
        Ok(BoardFile { path: path.into(), saved: b.clone(), board: b, stamp, saved_hash: fnv(text.as_bytes()) })
    }

    pub fn dirty(&self) -> bool {
        self.board.render() != self.saved.render()
    }

    /// Compare the file with what was loaded: cheap (modification time and size) first, then the content.
    pub fn check_disk(&self) -> Disk {
        let Some(now) = stamp_of(&self.path) else { return Disk::Gone };
        match &self.stamp {
            Some(s) if s.mtime_ns == now.mtime_ns && s.size == now.size => Disk::Same,
            _ => match std::fs::read(&self.path) {
                Ok(b) if fnv(&b) == self.saved_hash => Disk::Same,
                Ok(_) => Disk::Changed,
                Err(_) => Disk::Gone,
            },
        }
    }

    /// Take the file as it is on disk, dropping unsaved edits.
    pub fn reload(&mut self) -> Result<(), String> {
        *self = BoardFile::load(&self.path)?;
        Ok(())
    }

    /// Keep the board's edits on top of the file's new content, row by row.
    pub fn merge_with_disk(&mut self) -> Result<(), String> {
        let theirs = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
        let merged = Board::merge(&self.saved, &self.board, &theirs);
        let fresh = BoardFile::load(&self.path)?;
        self.saved = fresh.saved;
        self.stamp = fresh.stamp;
        self.saved_hash = fresh.saved_hash;
        self.board = merged;
        Ok(())
    }

    /// Write the board (totals kept in sync), the old content going to `.bak`.
    pub fn save(&mut self) -> Result<(), String> {
        self.board.sync_totals();
        let text = self.board.render();
        write_with_backup(&self.path, &text)?;
        self.saved = self.board.clone();
        self.stamp = stamp_of(&self.path).map(|mut s| {
            s.hash = fnv(text.as_bytes());
            s
        });
        self.saved_hash = fnv(text.as_bytes());
        Ok(())
    }

    pub fn backup_path(&self) -> PathBuf {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.path.with_file_name(format!("{name}.bak"))
    }

    /// One step of undo: the `.bak` becomes the file again (and the current content becomes the `.bak`, so undo twice is redo).
    pub fn undo(&mut self) -> Result<(), String> {
        let bak = self.backup_path();
        let old = std::fs::read_to_string(&bak).map_err(|_| "nothing to undo".to_string())?;
        write_with_backup(&self.path, &old)?;
        self.reload()
    }
}

/// Where the tests named in a feature live: files of the project that define `fn <name>(`. `test` is `crate::path::name`
/// or just a name; up to `max` hits, as (relative path, 1-based line).
pub fn find_test(root: &Path, test: &str, max: usize) -> Vec<(String, usize)> {
    let name = test.rsplit("::").next().unwrap_or(test).trim().trim_matches('`');
    let mut out = Vec::new();
    if name.is_empty() || test.starts_with("script:") {
        if let Some(p) = test.strip_prefix("script:") {
            if root.join(p).exists() {
                out.push((p.to_string(), 1));
            }
        }
        return out;
    }
    let needles = [format!("fn {name}("), format!("def {name}("), format!("function {name}("), format!("it(\"{name}\"")];
    fn walk(dir: &Path, root: &Path, needles: &[String], max: usize, depth: usize, out: &mut Vec<(String, usize)>) {
        if depth > 8 || out.len() >= max {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !crate::workspace::hidden_dir(&fname) {
                    walk(&p, root, needles, max, depth + 1, out);
                }
            } else if ft.is_file() {
                let Ok(meta) = e.metadata() else { continue };
                if meta.len() > 512 * 1024 {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                for (n, l) in text.lines().enumerate() {
                    if needles.iter().any(|x| l.contains(x.as_str())) {
                        out.push((p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned(), n + 1));
                        if out.len() >= max {
                            return;
                        }
                    }
                }
            }
        }
    }
    walk(root, root, &needles, max, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Title\n\n**Totals:** <!-- summary:start -->done 1 · partial 1 · planned 1 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\nText before.\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | First thing. More words | P1 | done | P0 | 100% | `c::t::one` |  |\n| F-A-02 | Second | P2 | partial | P1 | 40% | `c::t::two` | Waiting for the vendor. Rest later |\n| F-B-01 | Third | P3 | planned | P2 | 0% | — |  |\n\nAfter text.\n";

    #[test]
    fn parses_rows_by_header_and_renders_the_same_bytes() {
        let b = Board::parse(SAMPLE);
        assert!(b.has_table());
        assert_eq!(b.len(), 3);
        assert_eq!(b.render(), SAMPLE);
        let f = b.get("F-A-02").unwrap();
        assert_eq!((f.status.as_str(), f.progress, f.phase.as_str(), f.priority.as_str()), ("partial", Some(40), "P2", "P1"));
        assert_eq!(f.tests, vec!["c::t::two"]);
        assert_eq!(f.blockers(), vec!["Waiting for the vendor"], "derived from the notes");
        assert_eq!(b.get("F-A-01").unwrap().short_title(), "First thing");
        assert_eq!(b.areas(), vec!["A", "B"]);
        assert_eq!(b.next_id("A"), "F-A-03");
        assert_eq!(b.next_id("F-NEW"), "F-NEW-01");
    }

    fn diff_lines(a: &str, b: &str) -> Vec<(String, String)> {
        a.lines().zip(b.lines()).filter(|(x, y)| x != y).map(|(x, y)| (x.to_string(), y.to_string())).collect()
    }

    #[test]
    fn changing_one_cell_changes_exactly_that_cell() {
        let mut b = Board::parse(SAMPLE);
        b.update("F-B-01", &Update { priority: Some("P0".into()), ..Default::default() }).unwrap();
        let out = b.render();
        let d = diff_lines(SAMPLE, &out);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].1, "| F-B-01 | Third | P3 | planned | P0 | 0% | — |  |");
        assert_eq!(out.lines().count(), SAMPLE.lines().count());
    }

    #[test]
    fn status_moves_keep_progress_consistent_and_sync_the_totals() {
        let mut b = Board::parse(SAMPLE);
        b.update("F-B-01", &Update { status: Some("partial".into()), ..Default::default() }).unwrap();
        assert_eq!(b.get("F-B-01").unwrap().progress, Some(50));
        let w = b.get("F-B-01").unwrap().warnings();
        assert!(w.iter().any(|x| x.contains("needs a comment")), "{w:?}");
        b.update("F-A-02", &Update { status: Some("done".into()), ..Default::default() }).unwrap();
        assert_eq!(b.get("F-A-02").unwrap().progress, Some(100));
        assert!(b.sync_totals());
        assert_eq!(b.totals_in_file().unwrap(), "done 2 · partial 1 · planned 0 · blocked 0 · deprecated 0 · total 3");
        assert!(!b.sync_totals(), "already in sync");
        assert_eq!(b.percent_done(), 66);
    }

    #[test]
    fn blockers_use_the_files_own_convention() {
        let mut b = Board::parse(SAMPLE);
        b.add_blocker("F-B-01", "the new API").unwrap();
        assert_eq!(b.get("F-B-01").unwrap().notes, "Blocked by: the new API;");
        assert_eq!(b.get("F-B-01").unwrap().blockers(), vec!["Blocked by: the new API"]);
        let with = "| ID | Feature | Status | Blockers | Owner |\n|---|---|---|---|---|\n| X-1 | thing | blocked | db | ann |\n| X-2 | other | planned |  | bob |\n";
        let mut b = Board::parse(with);
        assert_eq!(b.get("X-1").unwrap().blockers(), vec!["db"]);
        b.add_blocker("X-2", "X-1").unwrap();
        b.update("X-1", &Update { status: Some("done".into()), ..Default::default() }).unwrap();
        let out = b.render();
        assert!(out.contains("| X-2 | other | planned | X-1 | bob |"), "{out}");
        assert!(out.contains("| X-1 | thing | done | db | ann |"), "the unknown Owner column is kept: {out}");
        b.clear_blockers("X-2").unwrap();
        assert!(b.render().contains("| X-2 | other | planned |  | bob |"));
    }

    #[test]
    fn tolerant_of_other_tables_and_missing_columns() {
        let t = "intro\n\n| Key | Name | State |\n|:--|:--|:--|\n| K-1 | One | open |\n| K-2 | Two | closed |\n";
        let b = Board::parse(t);
        assert_eq!(b.len(), 2);
        assert_eq!(b.statuses(false), vec!["open", "closed"]);
        assert!(b.get("K-1").unwrap().progress.is_none());
        let mut b = b;
        assert!(b.update("K-1", &Update { phase: Some("P1".into()), ..Default::default() }).is_err());
        b.update("K-1", &Update { status: Some("closed".into()), ..Default::default() }).unwrap();
        assert!(b.render().contains("| K-1 | One | closed |"));
        assert!(!Board::parse("no table here\n").has_table());
        assert!(Board::parse("| a | b |\n|---|---|\n| 1 | 2 |\n").features().is_empty(), "no id/status columns");
    }

    #[test]
    fn aligned_tables_stay_aligned() {
        let t = "| ID   | Feature | Status  |\n|------|---------|---------|\n| F-1  | Thing   | planned |\n| F-22 | Other   | done    |\n";
        let mut b = Board::parse(t);
        b.update("F-1", &Update { status: Some("done".into()), ..Default::default() }).unwrap();
        let out = b.render();
        assert!(out.contains("| F-1  | Thing   | done    |"), "{out}");
        b.update("F-22", &Update { status: Some("partial".into()), ..Default::default() }).unwrap();
        assert!(b.render().contains("| F-22 | Other   | partial |"));
    }

    #[test]
    fn add_and_delete_rows() {
        let mut b = Board::parse(SAMPLE);
        let id = b.next_id("B");
        b.add(&NewFeature { id: id.clone(), title: "Brand | new".into(), phase: "P4".into(), status: "planned".into(), priority: "P1".into(), notes: String::new() }).unwrap();
        let out = b.render();
        assert!(out.contains("| F-B-02 | Brand / new | P4 | planned | P1 | 0% | — |  |\n\nAfter text."), "{out}");
        assert!(b.add(&NewFeature { id, title: "x".into(), ..Default::default() }).is_err(), "duplicate");
        b.delete("F-B-02").unwrap();
        b.delete("F-B-01").unwrap();
        assert_eq!(b.len(), 2);
        assert!(b.sync_totals());
        assert!(b.render().contains("planned 0 · blocked 0 · deprecated 0 · total 2"));
        assert!(b.delete("F-NOPE").is_err());
    }

    #[test]
    fn readme_style_totals_sentence_is_kept_in_sync() {
        let t = "We have 5 features done, 2 partial, 9 planned today.\n\n| ID | Feature | Status |\n|---|---|---|\n| A-1 | x | done |\n| A-2 | y | planned |\n";
        let mut b = Board::parse(t);
        assert!(b.sync_totals());
        assert!(b.render().starts_with("We have 1 features done, 0 partial, 1 planned today."), "{}", b.render());
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-board-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_is_atomic_keeps_a_bak_and_undo_restores() {
        let d = tmp("save");
        let p = d.join("FEATURES.md");
        std::fs::write(&p, SAMPLE).unwrap();
        let mut f = BoardFile::load(&p).unwrap();
        assert!(!f.dirty());
        f.board.update("F-B-01", &Update { priority: Some("P0".into()), ..Default::default() }).unwrap();
        assert!(f.dirty());
        f.save().unwrap();
        assert!(!f.dirty());
        assert!(std::fs::read_to_string(&p).unwrap().contains("| P0 | 0% |"));
        assert_eq!(std::fs::read_to_string(f.backup_path()).unwrap(), SAMPLE);
        assert!(std::fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with("iyakki-tmp")));
        f.undo().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), SAMPLE);
        assert_eq!(f.check_disk(), Disk::Same);
    }

    #[test]
    fn outside_changes_are_noticed_and_merged_by_row() {
        let d = tmp("ext");
        let p = d.join("FEATURES.md");
        std::fs::write(&p, SAMPLE).unwrap();
        let mut f = BoardFile::load(&p).unwrap();
        assert_eq!(f.check_disk(), Disk::Same);
        // somebody else edits another row
        let theirs = SAMPLE.replace("| F-A-01 | First thing. More words | P1 | done |", "| F-A-01 | First thing. More words | P1 | deprecated |");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, &theirs).unwrap();
        assert_eq!(f.check_disk(), Disk::Changed);
        f.board.update("F-B-01", &Update { priority: Some("P0".into()), ..Default::default() }).unwrap();
        f.merge_with_disk().unwrap();
        let out = f.board.render();
        assert!(out.contains("| deprecated |") && out.contains("| F-B-01 | Third | P3 | planned | P0 |"), "{out}");
        assert!(f.dirty(), "my edit is still unsaved");
        f.save().unwrap();
        assert_eq!(f.check_disk(), Disk::Same);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(f.check_disk(), Disk::Gone);
    }

    #[test]
    fn the_repositorys_own_features_file_round_trips_and_one_cell_changes_one_cell() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../FEATURES.md")).expect("FEATURES.md at the repository root");
        let b = Board::parse(&text);
        let rows = text.lines().filter(|l| l.starts_with("| F-")).count();
        assert!(rows > 200 && b.len() == rows, "every row parses: {} of {rows}", b.len());
        assert_eq!(b.render(), text, "parsing and rendering change nothing");
        let fs = b.features();
        for f in &fs {
            assert!(STATUSES.contains(&f.status.as_str()), "{}: {}", f.id, f.status);
            assert!(f.progress.is_some() && !f.phase.is_empty() && !f.priority.is_empty() && !f.title.is_empty(), "{}", f.id);
            assert!(f.warnings().is_empty(), "{} breaks the repository's own rules: {:?}", f.id, f.warnings());
            if f.status == "blocked" {
                assert!(!f.blockers().is_empty(), "{} is blocked, so its notes name what blocks it", f.id);
            }
        }
        assert_eq!(b.totals_in_file().unwrap(), b.totals_text(), "the file's totals line matches the rows");
        let mut same = b.clone();
        assert!(!same.sync_totals(), "already in sync, nothing to write");
        // one cell
        let target = fs.iter().find(|f| f.status == "planned").unwrap().id.clone();
        let mut b2 = b.clone();
        let old = b2.get(&target).unwrap();
        let newp = if old.priority == "P3" { "P1" } else { "P3" };
        b2.update(&target, &Update { priority: Some(newp.into()), ..Default::default() }).unwrap();
        let out = b2.render();
        let changed: Vec<(&str, &str)> = text.lines().zip(out.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(changed.len(), 1, "{changed:?}");
        let (a, b_) = changed[0];
        let (ca, cb) = (cells_of(a), cells_of(b_));
        assert_eq!(ca.len(), cb.len());
        assert_eq!(ca.iter().zip(&cb).filter(|(x, y)| x != y).count(), 1, "exactly one cell differs");
        assert_eq!(text.lines().count(), out.lines().count());
        // a status move changes that row and the totals line, nothing else
        let mut b3 = b.clone();
        b3.update(&target, &Update { status: Some("partial".into()), comment: Some("started".into()), ..Default::default() }).unwrap();
        b3.sync_totals();
        let out3 = b3.render();
        assert_eq!(text.lines().zip(out3.lines()).filter(|(a, b)| a != b).count(), 2);
        assert!(out3.contains(&format!("partial {}", b.count_by_status()["partial"] + 1)));
    }

    #[test]
    fn finds_tests_by_name_in_the_project() {
        let d = tmp("tests");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn x() {}\n#[test]\nfn finds_me() {}\n").unwrap();
        std::fs::write(d.join("target/b.rs"), "fn finds_me() {}\n").unwrap();
        let hits = find_test(&d, "c::mod::finds_me", 5);
        assert_eq!(hits, vec![("src/a.rs".to_string(), 3)], "target/ is skipped");
        assert!(find_test(&d, "nothing_here", 5).is_empty());
    }

    #[test]
    fn times_are_set_on_transitions_kept_across_rewrites_and_old_boards_get_the_columns_lazily() {
        let old = "# T\n\n| ID | Feature | Status | Tests | Notes |\n|---|---|---|---|---|\n| A-1 | one | planned | — | n1 |\n| A-2 | two | planned | — |  |\n";
        let mut b = Board::parse(old);
        // no `at`: nothing about time changes, the file gains no columns
        b.update("A-1", &Update { status: Some("partial".into()), comment: Some("x".into()), ..Default::default() }).unwrap();
        assert!(!b.render().contains("Started"));
        let t0 = parse_stamp("2026-10-14 09:12").unwrap();
        let mut b = Board::parse(old);
        b.update("A-1", &Update { status: Some("partial".into()), comment: Some("working".into()), at: Some(t0), ..Default::default() }).unwrap();
        let text = b.render();
        assert!(text.contains("| ID | Feature | Status | Tests | Started | Finished | Notes |"), "{text}");
        assert!(text.contains("| A-1 | one | partial | — | 2026-10-14 09:12 | | working |") || text.contains("2026-10-14 09:12"), "{text}");
        let f = b.get("A-1").unwrap();
        assert_eq!((f.started.as_str(), f.finished.as_str(), f.notes.as_str()), ("2026-10-14 09:12", "", "working"));
        assert_eq!(b.get("A-2").unwrap().started, "", "other rows get empty cells");
        assert_eq!(b.get("A-2").unwrap().notes, "");
        // done 3 minutes later; Started is kept
        b.update("A-1", &Update { status: Some("done".into()), at: Some(t0 + 180), ..Default::default() }).unwrap();
        let f = b.get("A-1").unwrap();
        assert_eq!((f.started.as_str(), f.finished.as_str()), ("2026-10-14 09:12", "2026-10-14 09:15"));
        assert_eq!(f.duration(t0 + 99999), Some(180));
        // a rewrite through the parser changes nothing, and a reparse sees the same values
        let again = Board::parse(&b.render());
        assert_eq!(again.render(), b.render());
        assert_eq!(again.get("A-1").unwrap().finished, "2026-10-14 09:15");
        // leaving done clears Finished; a typed Started is not overwritten; planned clears both
        b.update("A-1", &Update { status: Some("partial".into()), comment: Some("again".into()), at: Some(t0 + 600), ..Default::default() }).unwrap();
        let f = b.get("A-1").unwrap();
        assert_eq!((f.started.as_str(), f.finished.as_str()), ("2026-10-14 09:12", ""));
        assert_eq!(f.duration(t0 + 600), Some(600), "elapsed is live while partial");
        b.update("A-1", &Update { status: Some("planned".into()), at: Some(t0 + 700), ..Default::default() }).unwrap();
        assert_eq!(b.get("A-1").unwrap().started, "");
        // done straight from planned starts and finishes at once
        b.update("A-2", &Update { status: Some("done".into()), at: Some(t0 + 60), ..Default::default() }).unwrap();
        let f = b.get("A-2").unwrap();
        assert_eq!((f.started.as_str(), f.finished.as_str()), ("2026-10-14 09:13", "2026-10-14 09:13"));
        // a board that already has the columns keeps hand-written values
        let mut h = Board::parse("| ID | Feature | Status | Started | Finished |\n|---|---|---|---|---|\n| X-1 | a | planned | 2026-01-01 08:00 |  |\n");
        h.update("X-1", &Update { status: Some("partial".into()), comment: Some("c".into()), at: Some(t0), ..Default::default() }).ok();
        assert_eq!(h.get("X-1").unwrap().started, "2026-01-01 08:00");
    }

    #[test]
    fn stamps_and_durations_are_formatted_for_people() {
        let t = parse_stamp("2026-10-14 09:12").unwrap();
        assert_eq!(stamp_text(t), "2026-10-14 09:12");
        assert_eq!(parse_stamp("2026-10-14T09:12:30Z"), Some(t));
        assert_eq!(parse_stamp("yesterday"), None);
        assert_eq!(parse_stamp("2026-13-40 99:99"), None);
        assert_eq!([0, 59, 60, 180, 3599, 3600, 3900, 86400 + 3 * 3600, -5].map(fmt_duration), ["under a minute", "under a minute", "1 min", "3 min", "59 min", "1 h 00 min", "1 h 05 min", "1 d 3 h", "under a minute"]);
        let f = Board::parse("| ID | Feature | Status |\n|---|---|---|\n| A | a | partial |\n").get("A").unwrap();
        assert_eq!(f.duration(0), None, "no Started column: no duration");
    }

    #[test]
    fn pending_rows_carry_what_they_wait_for_in_a_column_or_in_the_notes() {
        let mut with = Board::parse("| ID | Feature | Status | Depends | Notes |\n|---|---|---|---|---|\n| P-1 | one | planned |  | n |\n| P-2 | two | planned |  |  |\n");
        with.update("P-1", &Update { status: Some("pending".into()), depends: Some("P-2".into()), ..Default::default() }).unwrap();
        let f = with.get("P-1").unwrap();
        assert_eq!((f.status.as_str(), f.depends().as_str(), f.notes.as_str()), ("pending", "P-2", "n"));
        assert!(with.statuses(false).contains(&"pending".to_string()), "a Pending lane appears when a row is pending");
        // no column: a leading `Depends: …;` in the notes, kept apart from the rest and removable
        let mut notes_only = Board::parse("| ID | Feature | Status | Notes |\n|---|---|---|---|\n| P-1 | one | planned | keep me |\n");
        notes_only.update("P-1", &Update { status: Some("pending".into()), depends: Some("the registry".into()), ..Default::default() }).unwrap();
        let f = notes_only.get("P-1").unwrap();
        assert_eq!((f.depends().as_str(), f.notes.as_str()), ("the registry", "Depends: the registry; keep me"));
        notes_only.update("P-1", &Update { status: Some("partial".into()), depends: Some(String::new()), ..Default::default() }).unwrap();
        assert_eq!(notes_only.get("P-1").unwrap().notes, "keep me");
        // a board without any of this, and the repository's own table, are unaffected
        assert!(Board::parse("| ID | Feature | Status |\n|---|---|---|\n| A | a | planned |\n").get("A").unwrap().depends().is_empty());
        assert!(!Board::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../FEATURES.md")).unwrap()).statuses(false).contains(&"pending".to_string()));
    }

    #[test]
    fn tests_are_counted_from_any_list_style_and_tests_and_files_columns_are_added_when_missing() {
        let old = "| ID | Feature | Status | Notes |\n|---|---|---|---|\n| T-1 | one | done | n |\n";
        let mut b = Board::parse(old);
        assert_eq!(b.get("T-1").unwrap().test_count(), 0);
        // the repository's own style and the plain styles all count
        let styles = [("`a::b::c`<br>`d::e`", 2), ("a::b::c, d::e; f", 3), ("—", 0), ("", 0), ("`one`", 1)];
        for (cell, n) in styles {
            let b2 = Board::parse(&format!("| ID | Feature | Status | Tests |\n|---|---|---|---|\n| T-1 | one | done | {cell} |\n"));
            assert_eq!(b2.get("T-1").unwrap().test_count(), n, "{cell:?}");
        }
        // linking tests to a board that has no Tests column adds it before Notes, nothing else changes
        b.update("T-1", &Update { tests: Some(vec!["a::b".into(), "c::d".into()]), files: Some(vec!["src/**/*.js".into()]), ..Default::default() }).unwrap();
        let text = b.render();
        assert!(text.contains("| ID | Feature | Status | Tests | Files | Notes |"), "{text}");
        let f = b.get("T-1").unwrap();
        assert_eq!((f.tests.clone(), f.files.clone(), f.notes.as_str()), (vec!["a::b".to_string(), "c::d".to_string()], vec!["src/**/*.js".to_string()], "n"));
        assert_eq!(Board::parse(&text).render(), text, "a rewrite changes nothing");
        // the repository's FEATURES.md has tests on its rows and no Files column
        let repo = Board::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../FEATURES.md")).unwrap());
        assert!(repo.features().iter().any(|f| f.test_count() >= 2) && repo.cols().unwrap().files.is_none());
    }

    #[test]
    fn bug_ids_are_numbered_apart_and_bug_columns_are_added_lazily_and_kept() {
        let mut b = Board::parse("| ID | Feature | Status | Notes |\n|---|---|---|---|\n| F-APP-01 | one | done |  |\n| F-APP-02 | two | planned |  |\n");
        assert_eq!(b.next_id("APP"), "F-APP-03");
        assert_eq!(b.next_bug_id("APP"), "B-APP-01", "bugs are numbered on their own");
        assert_eq!(area_of("B-APP-01"), "APP");
        assert_eq!(area_of("F-APP-02"), "APP");
        b.add(&NewFeature { id: "B-APP-01".into(), title: "crash".into(), status: "planned".into(), ..Default::default() }).unwrap();
        assert_eq!(b.next_bug_id("APP"), "B-APP-02");
        assert_eq!(b.next_bug_id("web"), "B-web-01");
        assert!(b.get("B-APP-01").unwrap().is_bug() && !b.get("F-APP-01").unwrap().is_bug());
        // the bug columns appear when the first bug field is written, nothing else changes, a rewrite changes nothing
        let before = b.render();
        assert!(!before.contains("Severity"));
        b.update("B-APP-01", &Update { severity: Some("high".into()), repro: Some("restart it".into()), logs: Some(vec![".iyakki/bugs/B-APP-01/".into(), "build.log".into()]), found: Some("2026-10-14 09:00".into()), fix: Some("F-APP-02".into()), ..Default::default() }).unwrap();
        let text = b.render();
        for h in ["Severity", "Repro", "Logs", "Found", "Fix"] {
            assert!(text.contains(h), "{h}: {text}");
        }
        let f = b.get("B-APP-01").unwrap();
        assert_eq!((f.severity.as_str(), f.repro.as_str(), f.logs.clone(), f.found.as_str(), f.fix.as_str()), ("high", "restart it", vec![".iyakki/bugs/B-APP-01/".to_string(), "build.log".to_string()], "2026-10-14 09:00", "F-APP-02"));
        assert_eq!(Board::parse(&text).render(), text);
        assert_eq!(b.get("F-APP-02").unwrap().severity, "", "features have empty bug cells");
        // the repository's own table has none of this and no bug rows
        let repo = Board::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../FEATURES.md")).unwrap());
        assert!(repo.features().iter().all(|f| !f.is_bug()) && repo.cols().unwrap().severity.is_none());
    }
}
