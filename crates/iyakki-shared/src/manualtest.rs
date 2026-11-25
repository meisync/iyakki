//! Manual tests for people (the Doctor app's "Manual tests" tab and the chat tools `manual_*`): the checklist format, the human evidence
//! schema, the merge of evidence from several machines, and the pure rules (a scenario's state, the filters). No screen and no files of its
//! own: the bundled checklists arrive as text, the evidence goes through an [`EvidenceStore`] (the app gives it the core's file handles,
//! the chat tools give it the person's home folder), so everything here is tested with [`MemStore`].
//!
//! Levels stay apart: unit tests, QEMU runs, machine runs and human evidence are four different things. This module only ever makes the
//! human level, in the runner's own schema and file names: `human/<ROW>.<clause>.<machine>.json` (docs/development/AUTOTEST.md, "Human
//! checklist contract"; the same fields `dist/autotest/humanschema.py` validates, no field outside that list).
//!
//! Two sources of checklists are read side by side: the in-house `.chk` files (`dist/manual`) and the runner's checklist files
//! (`dist/autotest/checklists/<ROW>.json`, [`RunnerParser`]). Answers from the first H1 layout (`iyakki.human-evidence.1`, no machine in the
//! file name) are still read; only the runner's schema is written.
//!
//! The checklist format (one scenario per file, `dist/manual/*.chk`; lines `key: value`, `#` comments, a line that starts with two
//! spaces continues the line before):
//!
//! ```text
//! scenario: SND-EAR-01
//! title: Sound by ear
//! area: Sound
//! rows: F-SND-01, F-SND-02
//! machines: any            (or kinds that must occur in the hardware text: mac mini, lenovo, thinkpad)
//! needs: person            (the Manual column says needed)
//! prereq: Speakers or headphones are plugged in.
//! step: Open Settings, Sound, press Play test tone.
//! expect: Two notes, left then right.
//! clause: F-SND-01.tone    (optional; a bare clause counts for every row of the scenario)
//! shot: shots/sound.png    (optional example screenshot, relative to the image's manual folder)
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The folder in the person's home where evidence is kept.
pub const DIR: &str = "manual-tests";
/// One file the team collects, made by Export.
pub const EXPORT_DIR: &str = "manual-tests/export";
/// Files from other machines are put here (or on a stick DISKS opened) and merged by Import.
pub const IMPORT_DIR: &str = "manual-tests/import";
/// Where the image keeps the checklists (read-only data; the Doctor app asks its port for them).
pub const BUNDLED_DIR: &str = "/usr/share/iyakki/manual";
/// The runner's checklists (`dist/autotest/checklists/*.json`) and the screenshots they name, next to the `.chk` files.
pub const BUNDLED_CHECKLIST_DIR: &str = "/usr/share/iyakki/manual/checklists";
pub const HUMAN_DIR: &str = "manual-tests/human";
pub const EVIDENCE_SCHEMA: u32 = 1;
/// The first layout of the tab, still read.
pub const LEGACY_EVIDENCE_SCHEMA: &str = "iyakki.human-evidence.1";
pub const BUNDLE_SCHEMA: &str = "iyakki.human-evidence-bundle.1";
/// The biggest note kept with a verdict.
pub const MAX_NOTE: usize = 600;

/// What the lead assistant is told about the manual-test tools.
pub const PROMPT_RULE: &str = "\nMANUAL TESTS: manual_tests_list and manual_test_open show the checklists a person goes through on this computer (sound by ear, touch, Bluetooth, camera, hotspot, ...). manual_test_record writes the person's own answer, and always asks: record ONLY what the person told you they saw or heard, never a guess, never a result you inferred from a log; a skip writes nothing. manual_tests_export writes all answers as one file for the team and manual_tests_import merges the files other machines sent (it asks too). manual_machine_set keeps which computer this is (only the kind the person names, it asks; answers are refused until it is set, never guess it from the hardware). These answers are the human level of evidence, separate from unit tests and machine runs.";

// ------------------------------------------------------------------------------------------------ scenarios

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Step {
    pub text: String,
    pub expect: String,
    /// `ROW.clause` (that row only) or a bare clause (every row of the scenario). Empty: the step is guidance, no evidence is written.
    pub clause: String,
    pub shot: String,
    /// What the runner already did for this step (runner checklists only; shown, never needed).
    pub runner_did: String,
}

/// Where a checklist comes from; both are shown in the left navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// `dist/manual/*.chk`.
    #[default]
    Chk,
    /// `dist/autotest/checklists/<ROW>.json`, written by the autotest runner's side of the contract.
    Runner,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Chk => "manual checklist",
            Source::Runner => "autotest checklist",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Scenario {
    pub id: String,
    pub title: String,
    pub area: String,
    pub rows: Vec<String>,
    /// Kinds of machine it applies to; empty or `any` means all.
    pub machines: Vec<String>,
    pub needs_person: bool,
    pub prereq: Vec<String>,
    pub steps: Vec<Step>,
    pub source: Source,
    /// The hash of the Feature text the clauses were written for (runner checklists).
    pub feature_sha: String,
}

impl Scenario {
    pub fn applies_to(&self, hardware: &str) -> bool {
        let h = hardware.to_lowercase();
        self.machines.is_empty() || self.machines.iter().any(|m| m == "any" || h.contains(m.as_str()))
    }

    /// The (row, clause) pairs that step `i` answers.
    pub fn targets(&self, i: usize) -> Vec<(String, String)> {
        let Some(s) = self.steps.get(i) else { return vec![] };
        if s.clause.is_empty() {
            return vec![];
        }
        match s.clause.split_once('.') {
            Some((row, c)) if self.rows.iter().any(|r| r == row) => vec![(row.to_string(), c.to_string())],
            _ => self.rows.iter().map(|r| (r.clone(), s.clause.clone())).collect(),
        }
    }

    pub fn machines_text(&self) -> String {
        if self.machines.is_empty() { "any machine".into() } else { self.machines.join(", ") }
    }
}

/// Anything that turns checklist text into scenarios (the runner's own scenario format can get a parser of its own later).
pub trait ScenarioParser {
    fn parse(&self, name: &str, text: &str) -> Result<Vec<Scenario>, String>;
}

/// The in-house `.chk` format.
pub struct ChkParser;

/// A row id: `F-AREA-12`, or a split sub-row `F-AREA-12a` (one lowercase letter after the number, as FEATURES.md names the parts of a split row;
/// the same rule as `ROW_RE` in `dist/autotest/humanschema.py`).
fn row_ok(r: &str) -> bool {
    let mut p = r.split('-');
    let number_ok = |n: &str| {
        let digits = n.strip_suffix(|c: char| c.is_ascii_lowercase()).unwrap_or(n);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    };
    p.next() == Some("F") && p.next().is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())) && p.next().is_some_and(number_ok) && p.next().is_none()
}

fn shot_ok(p: &str) -> bool {
    !p.starts_with('/') && !p.contains('\\') && p.split('/').all(|s| !s.is_empty() && s != "..")
}

fn list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
}

impl ScenarioParser for ChkParser {
    fn parse(&self, name: &str, text: &str) -> Result<Vec<Scenario>, String> {
        // join continuation lines first
        let mut logical: Vec<(usize, String)> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
                continue;
            }
            if raw.starts_with("  ") {
                match logical.last_mut() {
                    Some(last) => {
                        last.1.push(' ');
                        last.1.push_str(raw.trim());
                    }
                    None => return Err(format!("{name}:{}: a continuation with nothing before it", n + 1)),
                }
            } else {
                logical.push((n + 1, raw.trim().to_string()));
            }
        }
        let mut out: Vec<Scenario> = Vec::new();
        for (n, line) in logical {
            let at = |m: &str| format!("{name}:{n}: {m}");
            let Some((k, v)) = line.split_once(':') else { return Err(at("expected `key: value`")) };
            let (k, v) = (k.trim(), v.trim());
            if k == "scenario" {
                out.push(Scenario { id: v.to_string(), ..Scenario::default() });
                continue;
            }
            let Some(sc) = out.last_mut() else { return Err(at("the file must start with `scenario:`")) };
            match k {
                "title" => sc.title = v.to_string(),
                "area" => sc.area = v.to_string(),
                "rows" => {
                    sc.rows = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                    if let Some(bad) = sc.rows.iter().find(|r| !row_ok(r)) {
                        return Err(at(&format!("`{bad}` is not a feature row id (F-AREA-00)")));
                    }
                }
                "machines" => sc.machines = list(v),
                "needs" => sc.needs_person = v == "person",
                "prereq" => sc.prereq.push(v.to_string()),
                "step" => sc.steps.push(Step { text: v.to_string(), ..Step::default() }),
                "expect" | "clause" | "shot" => {
                    let Some(st) = sc.steps.last_mut() else { return Err(at(&format!("`{k}` comes after a `step`"))) };
                    match k {
                        "expect" => st.expect = v.to_string(),
                        "clause" => st.clause = v.to_string(),
                        _ => {
                            if !shot_ok(v) {
                                return Err(at("a screenshot path is relative and stays inside the folder"));
                            }
                            st.shot = v.to_string();
                        }
                    }
                }
                other => return Err(at(&format!("unknown key `{other}`"))),
            }
        }
        for sc in &out {
            let id = &sc.id;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("{name}: a scenario id is letters, digits, - and _ (got `{id}`)"));
            }
            if sc.title.is_empty() || sc.area.is_empty() {
                return Err(format!("{name}: {id} needs a title and an area"));
            }
            if sc.rows.is_empty() {
                return Err(format!("{name}: {id} names no feature row"));
            }
            if sc.steps.is_empty() {
                return Err(format!("{name}: {id} has no step"));
            }
            for (i, s) in sc.steps.iter().enumerate() {
                if s.expect.is_empty() {
                    return Err(format!("{name}: {id} step {} has no expected result", i + 1));
                }
                if let Some((row, _)) = s.clause.split_once('.') {
                    if !sc.rows.iter().any(|r| r == row) && row_ok(row) {
                        return Err(format!("{name}: {id} step {} names the row {row}, which the scenario does not cover", i + 1));
                    }
                }
            }
        }
        if out.is_empty() {
            return Err(format!("{name}: no scenario in the file"));
        }
        Ok(out)
    }
}

/// The machine kinds a runner checklist may name (`dist/autotest/humanschema.py`, MACHINE_KINDS).
pub const MACHINE_KINDS: [&str; 9] = ["any", "qemu", "mac-mini-2014", "mac-mini-2018", "macbook-pro", "lenovo", "hp", "t7-ssd", "other"];
/// The longest text of a runner checklist field.
pub const MAX_TEXT: usize = 2000;
/// The area the runner's checklists are listed under in the left navigation.
pub const RUNNER_AREA: &str = "Autotest checklists";

/// The words of the hardware text that stand for a machine kind (the "this machine" filter looks for them); kinds that no hardware text can
/// show (`t7-ssd`, `other`) apply everywhere.
fn kind_words(kind: &str) -> Option<&'static str> {
    match kind {
        "qemu" => Some("qemu"),
        "mac-mini-2014" | "mac-mini-2018" => Some("mac mini"),
        "macbook-pro" => Some("macbook"),
        "lenovo" => Some("lenovo"),
        "hp" => Some("hp"),
        _ => None,
    }
}

/// The runner's checklist file, `dist/autotest/checklists/<ROW>.json` (contract: docs/development/AUTOTEST.md). One scenario per file, named by
/// its row. Each clause becomes its steps; the LAST step of a clause is the one that answers it (the person gives one verdict per clause, which
/// is what the runner's evidence file holds), the earlier ones are guidance.
pub struct RunnerParser;

fn plain_id(x: &str) -> bool {
    !x.is_empty() && x.len() <= 48 && x.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn rel_ok(p: &str) -> bool {
    !p.is_empty() && shot_ok(p) && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

fn text_of(v: &serde_json::Value, key: &str, at: &str, required: bool) -> Result<String, String> {
    match v.get(key) {
        Some(serde_json::Value::String(t)) if t.chars().count() <= MAX_TEXT && (!required || !t.trim().is_empty()) => Ok(t.trim().to_string()),
        Some(serde_json::Value::String(t)) if t.chars().count() > MAX_TEXT => Err(format!("{at}.{key} is longer than {MAX_TEXT}")),
        None if !required => Ok(String::new()),
        _ => Err(format!("{at}.{key} must be a non-empty text")),
    }
}

impl ScenarioParser for RunnerParser {
    fn parse(&self, name: &str, text: &str) -> Result<Vec<Scenario>, String> {
        let bad = |m: String| format!("{name}: {m}");
        let c: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(format!("not JSON ({e})")))?;
        if c.get("schema").and_then(|v| v.as_u64()) != Some(1) {
            return Err(bad("schema must be 1".into()));
        }
        let row = c.get("row").and_then(|v| v.as_str()).unwrap_or("");
        if !row_ok(row) {
            return Err(bad("row is not a row id".into()));
        }
        let sha = c.get("feature_sha").and_then(|v| v.as_str()).unwrap_or("");
        if sha.len() != 16 || !sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err(bad("feature_sha must be the 16 hex digits of the Feature text hash".into()));
        }
        let title = text_of(&c, "title", "checklist", true).map_err(&bad)?;
        let clauses = c.get("clauses").and_then(|v| v.as_array()).filter(|a| !a.is_empty()).ok_or_else(|| bad("clauses must be a non-empty list".into()))?;
        let mut sc = Scenario { id: row.to_string(), title: format!("{row} {title}"), area: RUNNER_AREA.into(), rows: vec![row.to_string()], needs_person: true, source: Source::Runner, feature_sha: sha.to_string(), ..Scenario::default() };
        let mut seen: Vec<String> = Vec::new();
        let mut any = false;
        let mut kinds: Vec<&'static str> = Vec::new();
        for (i, cl) in clauses.iter().enumerate() {
            let w = format!("clauses[{i}]");
            let id = cl.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !plain_id(id) {
                return Err(bad(format!("{w}.id is not a plain id")));
            }
            if seen.iter().any(|s| s == id) {
                return Err(bad(format!("{w}.id `{id}` is used twice")));
            }
            seen.push(id.to_string());
            let ctext = text_of(cl, "text", &w, true).map_err(&bad)?;
            let ms: Vec<&str> = cl.get("machines").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|m| m.as_str()).collect()).unwrap_or_default();
            let named = cl.get("machines").and_then(|v| v.as_array()).map_or(0, Vec::len);
            if ms.is_empty() || ms.len() != named {
                return Err(bad(format!("{w}.machines must be a non-empty list of {}", MACHINE_KINDS.join(", "))));
            }
            for m in &ms {
                match MACHINE_KINDS.iter().find(|k| *k == m) {
                    None => return Err(bad(format!("{w}.machines has `{m}`, which is not one of {}", MACHINE_KINDS.join(", ")))),
                    Some(k) => match kind_words(k) {
                        Some(words) => kinds.push(words),
                        None => any = true,
                    },
                }
            }
            let steps = cl.get("steps").and_then(|v| v.as_array()).filter(|a| !a.is_empty()).ok_or_else(|| bad(format!("{w}.steps must be a non-empty list")))?;
            sc.prereq.push(format!("Clause {id}: {ctext} (on: {})", ms.join(", ")));
            for (j, st) in steps.iter().enumerate() {
                let sw = format!("{w}.steps[{j}]");
                let mut step = Step { text: text_of(st, "do", &sw, true).map_err(&bad)?, expect: text_of(st, "expect", &sw, true).map_err(&bad)?, runner_did: text_of(st, "runner_did", &sw, false).map_err(&bad)?, ..Step::default() };
                if let Some(p) = st.get("screenshot") {
                    match p.as_str() {
                        Some(p) if rel_ok(p) => step.shot = p.to_string(),
                        _ => return Err(bad(format!("{sw}.screenshot must be a relative path of plain characters, not {p}"))),
                    }
                }
                if j + 1 == steps.len() {
                    step.clause = id.to_string();
                }
                sc.steps.push(step);
            }
        }
        if !any {
            kinds.sort_unstable();
            kinds.dedup();
            sc.machines = kinds.into_iter().map(str::to_string).collect();
        }
        Ok(vec![sc])
    }
}

/// Parse several sources at once, each with its own parser (the in-house `.chk` files and the runner's JSON checklists): one list, sorted by
/// area then id, a duplicate id keeps the first (the `.chk` source is given first).
pub fn load_sources(sources: &[(&dyn ScenarioParser, &[(String, String)])]) -> (Vec<Scenario>, Vec<String>) {
    let mut all: Vec<Scenario> = Vec::new();
    let mut bad = Vec::new();
    for (parser, files) in sources {
        let (v, b) = load_all(*parser, files);
        bad.extend(b);
        for s in v {
            if all.iter().any(|a| a.id == s.id) {
                bad.push(format!("the id {} is already used", s.id));
            } else {
                all.push(s);
            }
        }
    }
    all.sort_by(|a, b| (a.area.as_str(), a.id.as_str()).cmp(&(b.area.as_str(), b.id.as_str())));
    (all, bad)
}

/// Parse every (file name, text) pair; a file that does not parse is named in the second list and skipped. Sorted by area, then id; a
/// duplicate id keeps the first.
pub fn load_all(parser: &dyn ScenarioParser, files: &[(String, String)]) -> (Vec<Scenario>, Vec<String>) {
    let mut all: Vec<Scenario> = Vec::new();
    let mut bad = Vec::new();
    for (name, text) in files {
        match parser.parse(name, text) {
            Ok(v) => {
                for s in v {
                    if all.iter().any(|a| a.id == s.id) {
                        bad.push(format!("{name}: the id {} is already used", s.id));
                    } else {
                        all.push(s);
                    }
                }
            }
            Err(e) => bad.push(e),
        }
    }
    all.sort_by(|a, b| (a.area.as_str(), a.id.as_str()).cmp(&(b.area.as_str(), b.id.as_str())));
    (all, bad)
}

// ------------------------------------------------------------------------------------------------ verdicts and states

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Pass,
    Fail,
    CannotTest,
    Skip,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::CannotTest => "cannot-test",
            Verdict::Skip => "skip",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "Pass",
            Verdict::Fail => "Fail",
            Verdict::CannotTest => "Cannot test",
            Verdict::Skip => "Skip",
        }
    }
    pub fn parse(s: &str) -> Option<Verdict> {
        match s.trim().to_lowercase().replace([' ', '_'], "-").as_str() {
            "pass" | "passed" => Some(Verdict::Pass),
            "fail" | "failed" => Some(Verdict::Fail),
            "cannot-test" | "cannot" => Some(Verdict::CannotTest),
            "skip" | "skipped" => Some(Verdict::Skip),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    NotStarted,
    InProgress,
    Pass,
    Fail,
    CannotTest,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::NotStarted => "not started",
            State::InProgress => "in progress",
            State::Pass => "pass",
            State::Fail => "fail",
            State::CannotTest => "cannot test",
        }
    }
}

/// The state of a scenario from the verdict of each step (`None` = not answered).
pub fn scenario_state(verdicts: &[Option<Verdict>]) -> State {
    let answered = verdicts.iter().filter(|v| v.is_some()).count();
    if answered == 0 {
        return State::NotStarted;
    }
    if verdicts.contains(&Some(Verdict::Fail)) {
        return State::Fail;
    }
    if answered < verdicts.len() || verdicts.contains(&Some(Verdict::Skip)) {
        return State::InProgress;
    }
    if verdicts.contains(&Some(Verdict::CannotTest)) { State::CannotTest } else { State::Pass }
}

// ------------------------------------------------------------------------------------------------ evidence

/// One person's answer for one clause of one row on one machine, in the runner's schema (`dist/autotest/humanschema.py`, validate_human):
/// exactly these fields, none else (a file with another field is refused). The file name is `<ROW>.<clause>.<machine>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema: u32,
    pub row: String,
    pub clause: String,
    pub machine: String,
    /// The commit of the image under test: 7 to 40 hex digits.
    pub image_commit: String,
    /// The role of the person answering: `owner`, `admin` or `person` ([`BY_ROLES`]); never a name.
    pub by: String,
    /// `pass`, `fail` or `cannot-test`; a skip is not evidence and is never written.
    pub verdict: String,
    /// `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS`, UTC.
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// A photo, relative path (optional).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub photo: String,
}

/// The roles `by` may hold. The enforcer (`dist/autotest/evidence.py`) accepts all three as a human pass and records the sampling rule
/// (a tenth of the passes of team members is looked at again by the owner).
pub const BY_ROLES: [&str; 3] = ["owner", "admin", "person"];

/// The first layout of this tab (before the join with the runner): `iyakki.human-evidence.1`, no machine in the file name. Still read.
#[derive(Deserialize)]
struct LegacyEvidence {
    schema: String,
    row: String,
    clause: String,
    machine: String,
    image_commit: String,
    #[serde(default)]
    by: String,
    #[serde(default)]
    result: String,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    date: String,
}

impl LegacyEvidence {
    fn convert(self) -> Option<Evidence> {
        if self.schema != LEGACY_EVIDENCE_SCHEMA {
            return None;
        }
        let verdict = if self.verdict.is_empty() { self.result } else { self.verdict };
        Some(Evidence {
            schema: EVIDENCE_SCHEMA,
            row: self.row,
            clause: clean_clause(&self.clause),
            machine: clean_machine(&self.machine),
            image_commit: commit_of(&self.image_commit)?,
            by: if BY_ROLES.contains(&self.by.as_str()) { self.by } else { "person".into() },
            verdict,
            date: self.date,
            note: self.note,
            photo: String::new(),
        })
    }
}

impl Evidence {
    /// Unix seconds of the date (a date alone is midnight); 0 when it does not read.
    pub fn when(&self) -> i64 {
        parse_iso(&self.date).unwrap_or(0)
    }

    /// Why this record is not a valid human evidence file (the runner's rules), or Ok.
    pub fn check(&self) -> Result<(), String> {
        if self.schema != EVIDENCE_SCHEMA {
            return Err(format!("schema must be {EVIDENCE_SCHEMA}"));
        }
        if !row_ok(&self.row) {
            return Err("the row is not a feature row id".into());
        }
        if !plain_id(&self.clause) {
            return Err("the clause is not a plain id".into());
        }
        if !plain_id(&self.machine) {
            return Err("the machine is not a plain machine id".into());
        }
        if !(7..=40).contains(&self.image_commit.len()) || !self.image_commit.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err("the image commit is not known (this computer did not start from a release image), so an answer would prove nothing about an image".into());
        }
        if !BY_ROLES.contains(&self.by.as_str()) {
            return Err(format!("`by` must be one of {}", BY_ROLES.join(", ")));
        }
        if !matches!(self.verdict.as_str(), "pass" | "fail" | "cannot-test") {
            return Err("the verdict must be pass, fail or cannot-test".into());
        }
        if parse_iso(&self.date).is_none() {
            return Err("the date must be YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS".into());
        }
        if self.note.chars().count() > MAX_TEXT {
            return Err("the note is too long".into());
        }
        if !self.photo.is_empty() && !rel_ok(&self.photo) {
            return Err("the photo must be a relative path of plain characters".into());
        }
        Ok(())
    }
}

/// The commit of an image from what the machine knows: a plain hex commit as it is, or the `g<hex>` part of a `git describe` text.
pub fn commit_of(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let hex = |t: &str| (7..=40).contains(&t.len()) && t.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    if hex(&s) {
        return Some(s);
    }
    s.rsplit('-').find_map(|p| p.strip_prefix('g').filter(|h| hex(h)).map(str::to_string))
}

/// The machine id answers are filed under, from the hardware text: the runner's kind (`lenovo`, `mac-mini-2018`, ...) when the text names
/// one, so the runner's `machine-<name>` evidence and the person's answers meet. `None` when the text names no kind.
pub fn machine_kind(hardware: &str) -> Option<&'static str> {
    let h = hardware.to_ascii_lowercase();
    let has = |w: &str| h.contains(w);
    if has("qemu") {
        Some("qemu")
    } else if has("macmini7") || (has("mac mini") && has("2014")) {
        Some("mac-mini-2014")
    } else if has("macmini8") || (has("mac mini") && has("2018")) {
        Some("mac-mini-2018")
    } else if has("macbookpro") || has("macbook pro") {
        Some("macbook-pro")
    } else if has("lenovo") || has("thinkpad") {
        Some("lenovo")
    } else if h.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "hp") || has("hewlett") {
        Some("hp")
    } else {
        None
    }
}

/// The machine id answers are filed under: the runner's kind when the hardware text names one, else the computer's own name.
pub fn own_machine(hardware: &str, hostname: &str) -> String {
    machine_kind(hardware).map(str::to_string).unwrap_or_else(|| if hostname.trim().is_empty() { "this-computer".into() } else { clean_machine(hostname) })
}

/// The kinds a person may pick for this computer, in the order the list shows them (every runner kind except `any` and `qemu`).
pub const PICK_KINDS: [&str; 7] = ["mac-mini-2014", "mac-mini-2018", "macbook-pro", "lenovo", "hp", "t7-ssd", "other"];
/// Where the person's pick is kept, inside the person's home folder (through the core's file handles).
pub const MACHINE_FILE: &str = "manual-tests/machine.json";
/// What the tab and the chat tool say when answers are asked for before the person has picked.
pub const PICK_FIRST: &str = "Pick which computer this is first (Manual tests, key k): the answers are filed under that name and the program never guesses it.";

/// The words the list shows for a pickable kind.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "mac-mini-2014" => "Mac mini (2014)",
        "mac-mini-2018" => "Mac mini (2018)",
        "macbook-pro" => "MacBook Pro",
        "lenovo" => "Lenovo",
        "hp" => "HP",
        "t7-ssd" => "T7 SSD (the stick, whatever it runs in)",
        "other" => "Other computer",
        _ => "",
    }
}

/// The kind the person picked once, or `None` when nothing was picked yet or the file is not a valid pick. Nothing is ever derived from the
/// hardware text here.
pub fn picked_kind(store: &dyn EvidenceStore) -> Option<String> {
    let text = store.read(MACHINE_FILE).ok().flatten()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let k = v.get("kind")?.as_str()?;
    PICK_KINDS.contains(&k).then(|| k.to_string())
}

/// Keep the person's pick. Only a listed kind is written; the store (the core's file handles) applies its own scope, audit and approval.
pub fn pick_kind(store: &dyn EvidenceStore, kind: &str) -> Result<(), String> {
    if !PICK_KINDS.contains(&kind) {
        return Err(format!("`{kind}` is not one of the computers on the list: {}", PICK_KINDS.join(", ")));
    }
    store.write(MACHINE_FILE, &serde_json::json!({ "schema": 1, "kind": kind }).to_string())
}

/// The folders of a stick that DISKS opened (under `media/`) where another machine's exported answers may lie: at most four sticks.
pub fn stick_dirs(store: &dyn EvidenceStore) -> Vec<String> {
    let mut extra = Vec::new();
    for f in store.folders("media").unwrap_or_default().into_iter().take(4) {
        extra.push(format!("media/{f}"));
        extra.push(format!("media/{f}/{EXPORT_DIR}"));
    }
    extra
}

/// A clause as used in a file name (the same cleaning the runner does): plain characters, at most 48, starting with a letter or digit.
pub fn clean_clause(c: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in c.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(ch);
        } else {
            gap = true;
        }
    }
    let out: String = out.trim_start_matches(['-', '_']).chars().take(48).collect();
    if out.is_empty() { "clause".into() } else { out }
}

pub fn clean_machine(m: &str) -> String {
    clean_clause(m).to_lowercase()
}

/// `<ROW>.<clause>.<machine>.json`, the runner's file name.
pub fn file_name(row: &str, clause: &str, machine: &str) -> String {
    format!("{row}.{}.{}.json", clean_clause(clause), clean_machine(machine))
}

/// Where the evidence of `machine` is kept when this machine is `own`: the machine's own answers are in `human/`, answers imported from
/// another machine in `human/<machine>/` (the runner's enforcer reads both).
pub fn evidence_path(ev: &Evidence, own: &str) -> String {
    let name = file_name(&ev.row, &ev.clause, &ev.machine);
    if clean_machine(&ev.machine) == clean_machine(own) { format!("{HUMAN_DIR}/{name}") } else { format!("{HUMAN_DIR}/{}/{name}", clean_machine(&ev.machine)) }
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SS` (UTC).
pub fn iso(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let s = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// The inverse of [`iso`]: `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS` to unix seconds (UTC); `None` when it is not a real date.
pub fn parse_iso(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { t.get(r).filter(|x| x.bytes().all(|c| c.is_ascii_digit())).and_then(|x| x.parse().ok()) };
    let full = match b.len() {
        10 => false,
        19 => true,
        _ => return None,
    };
    if b[4] != b'-' || b[7] != b'-' || (full && (b[10] != b'T' || b[13] != b':' || b[16] != b':')) {
        return None;
    }
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = if full { (num(11..13)?, num(14..16)?, num(17..19)?) } else { (0, 0, 0) };
    if !(1..=12).contains(&m) || d < 1 || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][(m - 1) as usize];
    if d > dim {
        return None;
    }
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Who is answering and on what.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Who {
    pub who: String,
    pub admin: bool,
    pub machine: String,
    pub image_commit: String,
}

/// The answer of `who` for one clause (`None` for a skip). The result is checked when it is written ([`record`]).
pub fn make_evidence(_sc: &Scenario, row: &str, clause: &str, v: Verdict, note: &str, who: &Who, now: i64) -> Option<Evidence> {
    if v == Verdict::Skip {
        return None;
    }
    let note: String = note.chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect();
    Some(Evidence {
        schema: EVIDENCE_SCHEMA,
        row: row.into(),
        clause: clean_clause(clause),
        machine: clean_machine(&who.machine),
        image_commit: commit_of(&who.image_commit).unwrap_or_default(),
        by: if who.admin { "admin".into() } else { "person".into() },
        verdict: v.word().into(),
        date: iso(now),
        note,
        photo: String::new(),
    })
}

/// Where evidence lives. `rel` is a path inside the person's home folder (the core's file handles apply their own scope and audit).
pub trait EvidenceStore {
    fn read(&self, rel: &str) -> Result<Option<String>, String>;
    fn write(&self, rel: &str, text: &str) -> Result<(), String>;
    /// File names (not folders) of `dir`; an absent folder is an empty list.
    fn list(&self, dir: &str) -> Result<Vec<String>, String>;
    /// Sub-folder names of `dir`.
    fn folders(&self, dir: &str) -> Result<Vec<String>, String>;
}

/// A store in memory (tests of the app, the tools and this module).
#[derive(Default)]
pub struct MemStore {
    pub files: std::sync::Mutex<BTreeMap<String, String>>,
}

impl MemStore {
    pub fn put(&self, rel: &str, text: &str) {
        self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(rel.into(), text.into());
    }
    pub fn names(&self) -> Vec<String> {
        self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).keys().cloned().collect()
    }
}

impl EvidenceStore for MemStore {
    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        Ok(self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(rel).cloned())
    }
    fn write(&self, rel: &str, text: &str) -> Result<(), String> {
        self.put(rel, text);
        Ok(())
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        let p = format!("{dir}/");
        Ok(self.names().into_iter().filter_map(|n| n.strip_prefix(&p).filter(|r| !r.contains('/')).map(str::to_string)).collect())
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>, String> {
        let p = format!("{dir}/");
        let mut v: Vec<String> = self.names().into_iter().filter_map(|n| n.strip_prefix(&p).and_then(|r| r.split_once('/')).map(|(d, _)| d.to_string())).collect();
        v.dedup();
        Ok(v)
    }
}

/// Write one answer. Returns the path written.
pub fn record(store: &dyn EvidenceStore, ev: &Evidence, own: &str) -> Result<String, String> {
    ev.check()?;
    let path = evidence_path(ev, own);
    let text = serde_json::to_string_pretty(ev).map_err(|e| e.to_string())?;
    store.write(&path, &text)?;
    Ok(path)
}

/// One answer from its text: the runner's schema (unknown fields refused) or the first layout of this tab; a file that is neither, or whose
/// fields break the runner's rules, is not evidence.
pub fn parse_evidence(text: &str) -> Option<Evidence> {
    let e = match serde_json::from_str::<Evidence>(text) {
        Ok(e) => e,
        Err(_) => serde_json::from_str::<LegacyEvidence>(text).ok()?.convert()?,
    };
    e.check().ok()?;
    Some(e)
}

/// Every answer kept here: this machine's (`human/`) and the ones imported from others (`human/<machine>/`). Files that do not read are
/// skipped.
pub fn load_evidence(store: &dyn EvidenceStore) -> Vec<Evidence> {
    let mut out = Vec::new();
    let mut dirs = vec![HUMAN_DIR.to_string()];
    dirs.extend(store.folders(HUMAN_DIR).unwrap_or_default().into_iter().map(|f| format!("{HUMAN_DIR}/{f}")));
    for d in dirs {
        for n in store.list(&d).unwrap_or_default() {
            if n.ends_with(".json") {
                if let Some(e) = store.read(&format!("{d}/{n}")).ok().flatten().and_then(|t| parse_evidence(&t)) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// The verdict of each step of `sc` on machine `own`, from the answers kept (a step with several rows is as bad as its worst row).
pub fn step_verdicts(sc: &Scenario, evidence: &[Evidence], own: &str) -> Vec<Option<Verdict>> {
    (0..sc.steps.len())
        .map(|i| {
            let t = sc.targets(i);
            let found: Vec<Verdict> = t
                .iter()
                .filter_map(|(r, c)| evidence.iter().find(|e| e.row == *r && clean_clause(&e.clause) == clean_clause(c) && clean_machine(&e.machine) == clean_machine(own)).and_then(|e| Verdict::parse(&e.verdict)))
                .collect();
            if found.is_empty() || found.len() < t.len() {
                return None;
            }
            [Verdict::Fail, Verdict::CannotTest, Verdict::Pass].into_iter().find(|v| found.contains(v))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ export and import

/// What Export writes: everything this machine and the imported answers hold, as one file.
pub fn export(store: &dyn EvidenceStore, own: &str, now: i64) -> Result<String, String> {
    let mut items = load_evidence(store);
    items.sort_by(|a, b| (&a.machine, &a.row, &a.clause).cmp(&(&b.machine, &b.row, &b.clause)));
    let bundle = serde_json::json!({"schema": BUNDLE_SCHEMA, "machine": own, "exported": iso(now), "count": items.len(), "items": items});
    let path = format!("{EXPORT_DIR}/manual-evidence-{}-{}.json", clean_machine(own), &iso(now)[..10]);
    store.write(&path, &serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?)?;
    Ok(path)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub files: usize,
    pub added: usize,
    pub updated: usize,
    /// Answers that were older than the ones already here, or the same.
    pub kept: usize,
    pub refused: Vec<String>,
}

/// Merge the bundles in [`IMPORT_DIR`] (and, when given, the extra folders, such as a stick). Per (machine, row, clause) the newer `when`
/// wins; an equal one changes nothing. A file that is not a bundle or a single answer is named in `refused`.
pub fn import(store: &dyn EvidenceStore, own: &str, extra_dirs: &[String]) -> ImportReport {
    let mut rep = ImportReport::default();
    let have = load_evidence(store);
    let mut dirs = vec![IMPORT_DIR.to_string()];
    dirs.extend(extra_dirs.iter().cloned());
    for d in dirs {
        for n in store.list(&d).unwrap_or_default() {
            if !n.ends_with(".json") {
                continue;
            }
            rep.files += 1;
            let Some(text) = store.read(&format!("{d}/{n}")).ok().flatten() else {
                rep.refused.push(format!("{n}: cannot be read"));
                continue;
            };
            let items: Vec<Evidence> = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) if v["schema"] == BUNDLE_SCHEMA => v["items"].as_array().map(|a| a.iter().filter_map(|i| parse_evidence(&i.to_string())).collect()).unwrap_or_default(),
                Ok(_) => parse_evidence(&text).into_iter().collect(),
                Err(_) => vec![],
            };
            if items.is_empty() {
                rep.refused.push(format!("{n}: no human evidence in it"));
                continue;
            }
            for e in items {
                let same = |h: &&Evidence| clean_machine(&h.machine) == clean_machine(&e.machine) && h.row == e.row && clean_clause(&h.clause) == clean_clause(&e.clause);
                let old = have.iter().find(same);
                // the files written during this import count too
                match old {
                    Some(o) if o.when() >= e.when() => rep.kept += 1,
                    Some(_) => {
                        if record(store, &e, own).is_ok() {
                            rep.updated += 1;
                        }
                    }
                    None => {
                        if record(store, &e, own).is_ok() {
                            rep.added += 1;
                        }
                    }
                }
            }
        }
    }
    rep
}

// ------------------------------------------------------------------------------------------------ the list on the left

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only scenarios for this machine's kind.
    pub this_machine: bool,
    /// Only scenarios that still need a person (not yet pass).
    pub open_only: bool,
}

pub fn visible(scs: &[Scenario], states: &[State], hardware: &str, f: Filter) -> Vec<usize> {
    (0..scs.len())
        .filter(|&i| (!f.this_machine || scs[i].applies_to(hardware)) && (!f.open_only || (scs[i].needs_person && states[i] != State::Pass)))
        .collect()
}

/// The verdicts that count for the state: the steps that write evidence (a guidance step leaves no file, so it could never be answered
/// again after a restart). A scenario with only guidance steps counts all of them.
pub fn scored(sc: &Scenario, verdicts: &[Option<Verdict>]) -> Vec<Option<Verdict>> {
    let idx: Vec<usize> = (0..sc.steps.len()).filter(|&i| !sc.targets(i).is_empty()).collect();
    if idx.is_empty() { verdicts.to_vec() } else { idx.into_iter().map(|i| verdicts.get(i).copied().flatten()).collect() }
}

/// (answered, total) steps over the given scenarios, for the progress count at the top.
pub fn progress(scs: &[Scenario], verdicts: &[Vec<Option<Verdict>>], idx: &[usize]) -> (usize, usize) {
    let mut total = 0;
    let mut done = 0;
    for &i in idx {
        let v = scored(&scs[i], &verdicts[i]);
        total += v.len();
        done += v.iter().filter(|v| matches!(v, Some(Verdict::Pass | Verdict::Fail | Verdict::CannotTest))).count();
    }
    (done, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# a sound check\nscenario: SND-01\ntitle: Sound by ear\narea: Sound\nrows: F-SND-01, F-SND-02\nmachines: mac mini, lenovo\nneeds: person\nprereq: Headphones are plugged in.\nstep: Open Sound and press Play test tone.\n  Keep the volume low.\nexpect: Two notes, left then right.\nclause: F-SND-01.tone\nshot: shots/sound.png\nstep: Press Stop.\nexpect: The sound stops at once.\nclause: stop\nstep: Look at the card.\nexpect: It names the output.\n";

    fn who(admin: bool, machine: &str) -> Who {
        Who { who: "ana".into(), admin, machine: machine.into(), image_commit: "abc1234".into() }
    }

    fn sc() -> Scenario {
        ChkParser.parse("a.chk", SAMPLE).unwrap().remove(0)
    }

    #[test]
    fn the_checklist_format_reads_steps_clauses_continuations_and_comments() {
        let s = sc();
        assert_eq!((s.id.as_str(), s.title.as_str(), s.area.as_str()), ("SND-01", "Sound by ear", "Sound"));
        assert_eq!(s.rows, ["F-SND-01", "F-SND-02"]);
        assert_eq!(s.machines, ["mac mini", "lenovo"]);
        assert!(s.needs_person);
        assert_eq!(s.prereq, ["Headphones are plugged in."]);
        assert_eq!(s.steps.len(), 3);
        assert_eq!(s.steps[0].text, "Open Sound and press Play test tone. Keep the volume low.");
        assert_eq!((s.steps[0].clause.as_str(), s.steps[0].shot.as_str()), ("F-SND-01.tone", "shots/sound.png"));
    }

    #[test]
    fn a_bad_file_is_refused_with_its_line() {
        let p = ChkParser;
        for (text, word) in [
            ("title: x\n", "start with"),
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-1xy\nstep: s\nexpect: e\n", "not a feature row"),
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-01\nstep: s\n", "no expected"),
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-01\n", "no step"),
            ("scenario: A\ntitle: t\narea: a\nstep: s\nexpect: e\n", "no feature row"),
            ("scenario: A B\ntitle: t\narea: a\nrows: F-X-01\nstep: s\nexpect: e\n", "letters, digits"),
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-01\nstep: s\nexpect: e\nshot: ../x.png\n", "relative"),
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-01\nstep: s\nexpect: e\nclause: F-Y-02.c\n", "does not cover"),
            ("scenario: A\nbogus: 1\n", "unknown key"),
            ("scenario: A\nexpect: e\n", "after a `step`"),
            ("nonsense\n", "key: value"),
            ("", "no scenario"),
        ] {
            let e = p.parse("f.chk", text).unwrap_err();
            assert!(e.contains(word), "{text:?} -> {e}");
        }
        assert!(ChkParser.parse("f.chk", "scenario: A\ntitle: t\narea: a\nrows: F-X-01\nstep: s\nexpect: e\nshot: ../x.png\n").unwrap_err().starts_with("f.chk:7:"));
    }

    #[test]
    fn loading_sorts_by_area_skips_bad_files_and_duplicate_ids() {
        let mk = |id: &str, area: &str| format!("scenario: {id}\ntitle: t\narea: {area}\nrows: F-X-01\nstep: s\nexpect: e\n");
        let files = vec![("b.chk".to_string(), mk("B1", "Zed")), ("a.chk".into(), mk("A1", "Alpha")), ("bad.chk".into(), "junk".into()), ("dup.chk".into(), mk("A1", "Alpha"))];
        let (all, bad) = load_all(&ChkParser, &files);
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["A1", "B1"]);
        assert_eq!(bad.len(), 2);
        assert!(bad.iter().any(|b| b.contains("already used")));
    }

    #[test]
    fn a_clause_with_a_row_counts_for_that_row_and_a_bare_one_for_all_rows() {
        let s = sc();
        assert_eq!(s.targets(0), vec![("F-SND-01".to_string(), "tone".to_string())]);
        assert_eq!(s.targets(1), vec![("F-SND-01".to_string(), "stop".to_string()), ("F-SND-02".to_string(), "stop".to_string())]);
        assert!(s.targets(2).is_empty() && s.targets(9).is_empty());
    }

    #[test]
    fn the_state_follows_the_answers() {
        use Verdict::*;
        assert_eq!(scenario_state(&[None, None]), State::NotStarted);
        assert_eq!(scenario_state(&[Some(Pass), None]), State::InProgress);
        assert_eq!(scenario_state(&[Some(Pass), Some(Pass)]), State::Pass);
        assert_eq!(scenario_state(&[Some(Pass), Some(Fail)]), State::Fail);
        assert_eq!(scenario_state(&[Some(Fail), None]), State::Fail);
        assert_eq!(scenario_state(&[Some(Pass), Some(CannotTest)]), State::CannotTest);
        assert_eq!(scenario_state(&[Some(Pass), Some(Skip)]), State::InProgress);
        assert_eq!(scenario_state(&[]), State::NotStarted);
    }

    #[test]
    fn the_scenario_applies_to_a_machine_by_its_hardware_text() {
        let s = sc();
        assert!(s.applies_to("Apple Mac mini (2018)") && s.applies_to("LENOVO ThinkPad") && !s.applies_to("HP Pavilion"));
        let any = Scenario { machines: vec![], ..s.clone() };
        assert!(any.applies_to("anything"));
        let any2 = Scenario { machines: vec!["any".into()], ..s };
        assert!(any2.applies_to("anything"));
    }

    #[test]
    fn dates_are_utc_iso() {
        assert_eq!(iso(0), "1970-01-01T00:00:00");
        assert_eq!(iso(1_800_000_000), "2027-01-15T08:00:00");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(iso(-1), "1969-12-31T23:59:59");
    }

    #[test]
    fn file_names_are_the_runners_names() {
        assert_eq!(file_name("F-SND-01", "tone", "lenovo"), "F-SND-01.tone.lenovo.json");
        assert_eq!(file_name("F-SND-01", "left right!", "Mac Mini 2018"), "F-SND-01.left-right.mac-mini-2018.json");
        assert_eq!(file_name("F-X-01", "///", "m"), "F-X-01.clause.m.json");
        assert_eq!(clean_machine("Mac Mini #2"), "mac-mini-2");
        assert_eq!(clean_clause("_-a"), "a");
        assert_eq!(clean_clause(&"x".repeat(80)).len(), 48);
    }

    #[test]
    fn the_evidence_schema_round_trips_and_a_skip_is_never_evidence() {
        let s = sc();
        let st = MemStore::default();
        let e = make_evidence(&s, "F-SND-01", "tone", Verdict::Pass, "clear\nnotes\u{7}", &who(true, "iyakki-aaa"), 1_800_000_000).unwrap();
        assert_eq!((e.by.as_str(), e.verdict.as_str(), e.note.as_str(), e.date.as_str(), e.schema), ("admin", "pass", "clearnotes", "2027-01-15T08:00:00", 1));
        assert_eq!(make_evidence(&s, "F-SND-01", "tone", Verdict::Skip, "", &who(true, "m"), 1), None);
        assert_eq!(make_evidence(&s, "F-SND-01", "tone", Verdict::Fail, "", &who(false, "m"), 1).unwrap().by, "person");
        let p = record(&st, &e, "iyakki-aaa").unwrap();
        assert_eq!(p, "manual-tests/human/F-SND-01.tone.iyakki-aaa.json");
        let back = load_evidence(&st);
        assert_eq!(back, vec![e.clone()]);
        let text = st.read(&p).unwrap().unwrap();
        // exactly the runner's fields: nothing else is written, no name of a person
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["by", "clause", "date", "image_commit", "machine", "note", "row", "schema", "verdict"]);
        assert!(!text.contains("ana"));
        // a long note is cut
        let long = make_evidence(&s, "F-SND-01", "tone", Verdict::Fail, &"x".repeat(5000), &who(true, "m"), 1).unwrap();
        assert_eq!(long.note.len(), MAX_NOTE);
    }

    #[test]
    fn step_verdicts_come_back_from_the_files_for_this_machine_only() {
        let s = sc();
        let st = MemStore::default();
        for (clause, v, m) in [("tone", Verdict::Pass, "m1"), ("stop", Verdict::Pass, "m1"), ("stop", Verdict::Fail, "m2")] {
            for row in ["F-SND-01", "F-SND-02"] {
                if clause == "tone" && row == "F-SND-02" {
                    continue;
                }
                record(&st, &make_evidence(&s, row, clause, v, "", &who(true, m), 10).unwrap(), "m1").unwrap();
            }
        }
        let ev = load_evidence(&st);
        assert_eq!(ev.len(), 5);
        assert_eq!(step_verdicts(&s, &ev, "m1"), vec![Some(Verdict::Pass), Some(Verdict::Pass), None]);
        assert_eq!(step_verdicts(&s, &ev, "m2"), vec![None, Some(Verdict::Fail), None]);
        // one of two rows answered is not an answer yet
        let st2 = MemStore::default();
        record(&st2, &make_evidence(&s, "F-SND-01", "stop", Verdict::Pass, "", &who(true, "m1"), 1).unwrap(), "m1").unwrap();
        assert_eq!(step_verdicts(&s, &load_evidence(&st2), "m1")[1], None);
    }

    #[test]
    fn export_then_import_on_another_machine_merges_newest_wins_per_machine() {
        let s = sc();
        let a = MemStore::default();
        record(&a, &make_evidence(&s, "F-SND-01", "tone", Verdict::Pass, "", &who(true, "m1"), 100).unwrap(), "m1").unwrap();
        record(&a, &make_evidence(&s, "F-SND-01", "stop", Verdict::Fail, "no stop", &who(true, "m1"), 100).unwrap(), "m1").unwrap();
        let path = export(&a, "m1", 200).unwrap();
        assert_eq!(path, "manual-tests/export/manual-evidence-m1-1970-01-01.json");
        let bundle = a.read(&path).unwrap().unwrap();
        // machine b has its own answer for the same clause and an older and a newer copy of a's
        let b = MemStore::default();
        record(&b, &make_evidence(&s, "F-SND-01", "tone", Verdict::Fail, "mine", &who(true, "m2"), 150).unwrap(), "m2").unwrap();
        b.put("manual-tests/import/a.json", &bundle);
        let r = import(&b, "m2", &[]);
        assert_eq!((r.files, r.added, r.updated, r.kept), (1, 2, 0, 0), "{r:?}");
        // the other machine's answers sit in their own folder, mine is untouched
        assert!(b.names().contains(&"manual-tests/human/m1/F-SND-01.tone.m1.json".to_string()));
        let mine = load_evidence(&b).into_iter().find(|e| e.machine == "m2").unwrap();
        assert_eq!(mine.note, "mine");
        // importing again changes nothing
        let r2 = import(&b, "m2", &[]);
        assert_eq!((r2.added, r2.updated, r2.kept), (0, 0, 2));
        // a newer answer replaces the old one
        let newer = make_evidence(&s, "F-SND-01", "stop", Verdict::Pass, "fixed", &who(true, "m1"), 900).unwrap();
        b.put("manual-tests/import/new.json", &serde_json::to_string(&newer).unwrap());
        let r3 = import(&b, "m2", &[]);
        assert_eq!(r3.updated, 1, "{r3:?}");
        let ev = load_evidence(&b);
        assert_eq!(ev.iter().find(|e| e.machine == "m1" && e.clause == "stop").unwrap().verdict, "pass");
        // junk is named, not merged
        b.put("manual-tests/import/junk.json", "{not json");
        b.put("manual-tests/import/other.json", "{\"schema\":\"x\"}");
        let r4 = import(&b, "m2", &[]);
        assert_eq!(r4.refused.len(), 2, "{r4:?}");
        // an extra folder (a stick) is read too
        b.put("stick/one.json", &serde_json::to_string(&make_evidence(&s, "F-SND-02", "stop", Verdict::Pass, "", &who(true, "m3"), 5).unwrap()).unwrap());
        assert_eq!(import(&b, "m2", &["stick".into()]).added, 1);
    }

    #[test]
    fn files_that_are_not_evidence_are_skipped_when_loading() {
        let st = MemStore::default();
        st.put("manual-tests/human/x.json", "garbage");
        st.put("manual-tests/human/y.txt", "{}");
        assert!(load_evidence(&st).is_empty());
    }

    #[test]
    fn filters_and_progress() {
        let mk = |id: &str, m: &[&str], needs: bool| Scenario { id: id.into(), machines: m.iter().map(|s| s.to_string()).collect(), needs_person: needs, steps: vec![Step::default(); 2], ..Scenario::default() };
        let scs = vec![mk("a", &["mac mini"], true), mk("b", &[], true), mk("c", &["hp"], false)];
        let states = [State::Pass, State::NotStarted, State::NotStarted];
        let all = Filter::default();
        assert_eq!(visible(&scs, &states, "Mac mini", all), vec![0, 1, 2]);
        assert_eq!(visible(&scs, &states, "Mac mini", Filter { this_machine: true, open_only: false }), vec![0, 1]);
        assert_eq!(visible(&scs, &states, "Mac mini", Filter { this_machine: false, open_only: true }), vec![1]);
        let v = vec![vec![Some(Verdict::Pass), Some(Verdict::Skip)], vec![None, None], vec![Some(Verdict::Fail), Some(Verdict::CannotTest)]];
        assert_eq!(progress(&scs, &v, &[0, 1, 2]), (3, 6));
    }

    #[test]
    fn verdict_words_parse() {
        assert_eq!(Verdict::parse("Cannot test"), Some(Verdict::CannotTest));
        assert_eq!(Verdict::parse("PASS"), Some(Verdict::Pass));
        assert_eq!(Verdict::parse("maybe"), None);
    }

    const RUNNER: &str = r#"{"schema":1,"row":"F-X-1","feature_sha":"0123456789abcdef","title":"the tone plays",
      "clauses":[{"id":"ears-1","text":"two notes, left then right","machines":["lenovo","mac-mini-2018"],
        "steps":[{"do":"open Sound","expect":"the card shows","runner_did":"booted the image"},{"do":"press Play","expect":"two notes","screenshot":"shots/tone.png"}]},
        {"id":"stop","text":"stop is immediate","machines":["lenovo"],"steps":[{"do":"press Stop","expect":"silence"}]}]}"#;

    fn runner() -> Scenario {
        RunnerParser.parse("F-X-1.json", RUNNER).unwrap().remove(0)
    }

    #[test]
    fn a_runner_checklist_becomes_one_scenario_answered_per_clause() {
        let s = runner();
        assert_eq!((s.id.as_str(), s.source, s.area.as_str(), s.feature_sha.as_str()), ("F-X-1", Source::Runner, RUNNER_AREA, "0123456789abcdef"));
        assert_eq!(s.title, "F-X-1 the tone plays");
        assert_eq!(s.rows, ["F-X-1"]);
        assert_eq!(s.machines, ["lenovo", "mac mini"]);
        assert_eq!(s.steps.len(), 3);
        // only the last step of a clause answers it; the first is guidance
        assert!(s.targets(0).is_empty());
        assert_eq!(s.targets(1), vec![("F-X-1".to_string(), "ears-1".to_string())]);
        assert_eq!(s.targets(2), vec![("F-X-1".to_string(), "stop".to_string())]);
        assert_eq!((s.steps[0].runner_did.as_str(), s.steps[1].shot.as_str()), ("booted the image", "shots/tone.png"));
        assert!(s.prereq[0].starts_with("Clause ears-1: two notes") && s.prereq.len() == 2);
        assert!(s.applies_to("LENOVO ThinkPad") && !s.applies_to("HP Pavilion"));
        // a clause for `any` makes the whole checklist apply everywhere
        let any = RunnerParser.parse("x", &RUNNER.replace("[\"lenovo\"]", "[\"any\"]")).unwrap().remove(0);
        assert!(any.machines.is_empty());
    }

    #[test]
    fn a_bad_runner_checklist_is_refused_for_the_runners_reasons() {
        for (from, to, word) in [
            ("\"schema\":1", "\"schema\":2", "schema"),
            ("F-X-1", "X-1", "row"),
            ("0123456789abcdef", "xyz", "feature_sha"),
            ("\"id\":\"stop\"", "\"id\":\"ears-1\"", "used twice"),
            ("\"id\":\"stop\"", "\"id\":\"a b\"", "plain id"),
            ("\"machines\":[\"lenovo\"]", "\"machines\":[\"toaster\"]", "not one of"),
            ("\"machines\":[\"lenovo\"]", "\"machines\":[]", "machines"),
            ("\"screenshot\":\"shots/tone.png\"", "\"screenshot\":\"../tone.png\"", "screenshot"),
            ("\"expect\":\"silence\"", "\"expect\":\" \"", "expect"),
            ("\"steps\":[{\"do\":\"press Stop\"", "\"steps\":[],\"x\":[{\"do\":\"press Stop\"", "steps"),
            ("\"title\":\"the tone plays\"", "\"title\":\"\"", "title"),
        ] {
            let e = RunnerParser.parse("c.json", &RUNNER.replace(from, to)).unwrap_err();
            assert!(e.contains(word) && e.starts_with("c.json:"), "{from} -> {e}");
        }
        assert!(RunnerParser.parse("c.json", "{not json").unwrap_err().contains("not JSON"));
        assert!(RunnerParser.parse("c.json", "{\"schema\":1,\"row\":\"F-X-1\",\"feature_sha\":\"0123456789abcdef\",\"title\":\"t\",\"clauses\":[]}").unwrap_err().contains("clauses"));
    }

    #[test]
    fn the_checklists_of_the_repository_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/autotest/checklists");
        let files: Vec<(String, String)> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".json")).filter_map(|e| std::fs::read_to_string(e.path()).ok().map(|t| (e.file_name().to_string_lossy().into_owned(), t))).collect();
        let (all, bad) = load_all(&RunnerParser, &files);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(all.len(), files.len());
    }

    #[test]
    fn a_row_id_may_name_a_split_sub_row_with_one_lowercase_letter() {
        for ok in ["F-SND-15", "F-SND-15a", "F-GEMVOICE-13b", "F-A11Y-02a", "F-X-1"] {
            assert!(row_ok(ok), "{ok}");
        }
        for bad in ["F-SND-15A", "F-SND-15ab", "F-SND-a", "F-SND-", "F-snd-15", "F-SND-15a-1", "G-SND-15", "F-SND", "", "F-SND-1_5"] {
            assert!(!row_ok(bad), "{bad}");
        }
    }

    #[test]
    fn both_sources_are_listed_together_each_with_its_own_parser() {
        let chk = vec![("a.chk".to_string(), SAMPLE.to_string())];
        let json = vec![("F-X-1.json".to_string(), RUNNER.to_string()), ("bad.json".to_string(), "x".to_string())];
        let (all, bad) = load_sources(&[(&ChkParser, &chk), (&RunnerParser, &json)]);
        assert_eq!(all.iter().map(|s| (s.id.as_str(), s.source)).collect::<Vec<_>>(), [("F-X-1", Source::Runner), ("SND-01", Source::Chk)]);
        assert_eq!(all[0].area, RUNNER_AREA);
        assert_eq!(bad.len(), 1);
        // the same id in both sources keeps the first
        let clash = vec![("x.json".to_string(), RUNNER.to_string())];
        let chk2 = vec![("a.chk".to_string(), "scenario: F-X-1\ntitle: t\narea: a\nrows: F-X-01\nstep: s\nexpect: e\n".to_string())];
        let (all, bad) = load_sources(&[(&ChkParser, &chk2), (&RunnerParser, &clash)]);
        assert_eq!((all.len(), all[0].source, bad.len()), (1, Source::Chk, 1));
    }

    #[test]
    fn an_answer_on_a_runner_checklist_is_the_runners_file() {
        let s = runner();
        let st = MemStore::default();
        let w = Who { who: "ana".into(), admin: false, machine: "lenovo".into(), image_commit: "v0.1.0-alpha.8-3-gabc1234f".into() };
        let e = make_evidence(&s, "F-X-1", "ears-1", Verdict::Pass, "", &w, 1_800_000_000).unwrap();
        assert_eq!((e.by.as_str(), e.image_commit.as_str(), e.machine.as_str()), ("person", "abc1234f", "lenovo"));
        assert_eq!(record(&st, &e, "lenovo").unwrap(), "manual-tests/human/F-X-1.ears-1.lenovo.json");
        assert!(!st.read("manual-tests/human/F-X-1.ears-1.lenovo.json").unwrap().unwrap().contains("note"));
        assert_eq!(step_verdicts(&s, &load_evidence(&st), "lenovo"), vec![None, Some(Verdict::Pass), None]);
        // a machine that cannot name its commit writes nothing
        let w2 = Who { image_commit: "unknown".into(), ..w };
        let e2 = make_evidence(&s, "F-X-1", "stop", Verdict::Pass, "", &w2, 1).unwrap();
        assert!(record(&st, &e2, "lenovo").unwrap_err().contains("commit"));
        assert_eq!(st.names().len(), 1);
    }

    #[test]
    fn unknown_fields_and_bad_values_make_a_file_no_evidence() {
        let ok = r#"{"schema":1,"row":"F-X-1","clause":"c","machine":"lenovo","image_commit":"abc1234","by":"owner","verdict":"pass","date":"2026-11-22"}"#;
        assert!(parse_evidence(ok).is_some());
        for (from, to) in [
            ("\"by\":\"owner\"", "\"by\":\"owner\",\"who\":\"ana\""),
            ("\"by\":\"owner\"", "\"by\":\"ana\""),
            ("\"verdict\":\"pass\"", "\"verdict\":\"skip\""),
            ("abc1234", "xyz"),
            ("2026-11-22", "2026-02-30"),
            ("\"clause\":\"c\"", "\"clause\":\"a b\""),
            ("\"schema\":1", "\"schema\":2"),
        ] {
            assert!(parse_evidence(&ok.replace(from, to)).is_none(), "{to}");
        }
        for role in BY_ROLES {
            assert!(parse_evidence(&ok.replace("owner", role)).is_some(), "{role}");
        }
        assert!(parse_evidence(&ok.replace("\"date\"", "\"photo\":\"p/x.jpg\",\"note\":\"n\",\"date\"")).is_some());
        assert!(parse_evidence(&ok.replace("\"date\"", "\"photo\":\"/etc/x\",\"date\"")).is_none());
    }

    #[test]
    fn the_first_layout_is_still_read_and_written_in_the_new_one() {
        let old = r#"{"schema":"iyakki.human-evidence.1","row":"F-SND-01","clause":"tone","scenario":"SND-01","machine":"iyakki-aaa","image_commit":"v0.1.0-alpha.7-12-gabc1234","who":"ana","by":"owner","result":"pass","verdict":"pass","note":"clear","date":"2027-01-15T08:00:00","when":1800000000}"#;
        let st = MemStore::default();
        st.put("manual-tests/human/F-SND-01.tone.json", old);
        let ev = load_evidence(&st);
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].schema, ev[0].image_commit.as_str(), ev[0].by.as_str(), ev[0].verdict.as_str(), ev[0].when()), (1, "abc1234", "owner", "pass", 1_800_000_000));
        // a bundle of the first layout imports into the new file names
        st.put("manual-tests/import/b.json", &format!("{{\"schema\":\"{BUNDLE_SCHEMA}\",\"items\":[{}]}}", old.replace("iyakki-aaa", "other").replace("F-SND-01", "F-SND-02")));
        let r = import(&st, "iyakki-aaa", &[]);
        assert_eq!(r.added, 1, "{r:?}");
        assert!(st.names().contains(&"manual-tests/human/other/F-SND-02.tone.other.json".to_string()));
    }

    #[test]
    fn dates_round_trip_and_bad_dates_are_refused() {
        for t in [0, 951_782_400, 1_800_000_000, -1, 4_102_444_799] {
            assert_eq!(parse_iso(&iso(t)), Some(t));
        }
        assert_eq!(parse_iso("2027-01-15"), Some(1_799_971_200 + 0));
        for bad in ["", "2026-13-01", "2026-00-10", "2026-02-29", "2026-1-1", "2026-11-22T25:00:00", "2026-11-22 10:00:00", "20261122", "2026-11-22T10:00"] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
        assert_eq!(parse_iso("2028-02-29").is_some(), true);
    }

    #[test]
    fn commits_and_machine_kinds_come_from_what_the_machine_knows() {
        assert_eq!(commit_of("ABC1234"), Some("abc1234".into()));
        assert_eq!(commit_of("v0.1.0-alpha.8-12-gdeadbeef"), Some("deadbeef".into()));
        assert_eq!(commit_of("v0.1.0-alpha.8"), None);
        assert_eq!(commit_of(""), None);
        assert_eq!(commit_of("abc"), None);
        assert_eq!(machine_kind("Apple Macmini8,1"), Some("mac-mini-2018"));
        assert_eq!(machine_kind("Apple Mac mini (Late 2014)"), Some("mac-mini-2014"));
        assert_eq!(machine_kind("MacBookPro14,1"), Some("macbook-pro"));
        assert_eq!(machine_kind("LENOVO 20XW"), Some("lenovo"));
        assert_eq!(machine_kind("HP Pavilion Laptop 16"), Some("hp"));
        assert_eq!(machine_kind("QEMU Standard PC"), Some("qemu"));
        assert_eq!(machine_kind("chipset only"), None);
        assert_eq!(own_machine("LENOVO 20XW", "iyakki-aaa"), "lenovo");
        assert_eq!(own_machine("unknown box", "Iyakki AAA"), "iyakki-aaa");
        assert_eq!(own_machine("", " "), "this-computer");
    }

    #[test]
    fn sticks_are_looked_for_under_media() {
        let st = MemStore::default();
        for n in ["media/a/x.txt", "media/b/y.txt", "media/c/z.txt", "media/d/z.txt", "media/e/z.txt"] {
            st.put(n, "");
        }
        let d = stick_dirs(&st);
        assert_eq!(d.len(), 8);
        assert_eq!(d[0], "media/a");
        assert_eq!(d[1], "media/a/manual-tests/export");
    }

    #[test]
    fn the_person_picks_the_machine_once_and_nothing_is_guessed() {
        let st = MemStore::default();
        assert_eq!(picked_kind(&st), None, "nothing picked: no kind, whatever the hardware says");
        assert!(pick_kind(&st, "qemu").is_err() && pick_kind(&st, "any").is_err() && pick_kind(&st, "Lenovo").is_err(), "only the seven listed kinds");
        assert_eq!(st.names(), Vec::<String>::new(), "a refused pick writes nothing");
        for k in PICK_KINDS {
            pick_kind(&st, k).unwrap();
            assert_eq!(picked_kind(&st).as_deref(), Some(k));
            assert!(!kind_label(k).is_empty());
        }
        assert!(PICK_KINDS.iter().all(|k| MACHINE_KINDS.contains(k)), "every pickable kind is a kind the runner's checklists know");
        st.put(MACHINE_FILE, "not json");
        assert_eq!(picked_kind(&st), None);
        st.put(MACHINE_FILE, r#"{"schema":1,"kind":"toaster"}"#);
        assert_eq!(picked_kind(&st), None, "a file edited by hand to an unknown kind is not a pick");
    }

    #[test]
    fn the_pick_names_the_evidence_file() {
        let st = MemStore::default();
        pick_kind(&st, "mac-mini-2018").unwrap();
        let who = Who { who: "ana".into(), admin: false, machine: picked_kind(&st).unwrap(), image_commit: String::new() };
        assert_eq!(evidence_path(&make_evidence(&Scenario::default(), "F-SND-01", "tone", Verdict::Pass, "", &who, 1_800_000_000).unwrap(), &who.machine), "manual-tests/human/F-SND-01.tone.mac-mini-2018.json");
    }
}
