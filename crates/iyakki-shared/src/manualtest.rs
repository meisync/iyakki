//! Manual tests for people (the Doctor app's "Manual tests" tab and the chat tools `manual_*`): the checklist format, the human evidence
//! schema, the merge of evidence from several machines, and the pure rules (a scenario's state, the filters). No screen and no files of its
//! own: the bundled checklists arrive as text, the evidence goes through an [`EvidenceStore`] (the app gives it the core's file handles,
//! the chat tools give it the person's home folder), so everything here is tested with [`MemStore`].
//!
//! Levels stay apart: unit tests, QEMU runs, machine runs and human evidence are four different things. This module only ever makes the
//! human level: `human/<ROW>.<clause>.json` (the file name the runner's migrator reads), one file per row and clause.
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
pub const HUMAN_DIR: &str = "manual-tests/human";
pub const EVIDENCE_SCHEMA: &str = "iyakki.human-evidence.1";
pub const BUNDLE_SCHEMA: &str = "iyakki.human-evidence-bundle.1";
/// The biggest note kept with a verdict.
pub const MAX_NOTE: usize = 600;

/// What the lead assistant is told about the manual-test tools.
pub const PROMPT_RULE: &str = "\nMANUAL TESTS: manual_tests_list and manual_test_open show the checklists a person goes through on this computer (sound by ear, touch, Bluetooth, camera, hotspot, ...). manual_test_record writes the person's own answer, and always asks: record ONLY what the person told you they saw or heard, never a guess, never a result you inferred from a log; a skip writes nothing. manual_tests_export writes all answers as one file for the team. These answers are the human level of evidence, separate from unit tests and machine runs.";

// ------------------------------------------------------------------------------------------------ scenarios

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Step {
    pub text: String,
    pub expect: String,
    /// `ROW.clause` (that row only) or a bare clause (every row of the scenario). Empty: the step is guidance, no evidence is written.
    pub clause: String,
    pub shot: String,
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

fn row_ok(r: &str) -> bool {
    let mut p = r.split('-');
    p.next() == Some("F") && p.next().is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())) && p.next().is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) && p.next().is_none()
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

/// One person's answer for one clause of one row on one machine. The file name is `<ROW>.<clause>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub schema: String,
    pub row: String,
    pub clause: String,
    pub scenario: String,
    pub machine: String,
    pub image_commit: String,
    /// The signed-in person's account.
    pub who: String,
    /// `owner` for an administrator, `person` for anybody else (the runner's migrator accepts `owner`).
    pub by: String,
    /// `pass`, `fail` or `cannot-test`; a skip is not evidence and is never written.
    pub result: String,
    /// The same word again, under the name the checklist uses.
    pub verdict: String,
    pub note: String,
    /// `YYYY-MM-DDTHH:MM:SS`, UTC.
    pub date: String,
    pub when: i64,
}

/// A clause as used in a file name (the same cleaning the runner does).
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
    if out.is_empty() { "clause".into() } else { out }
}

pub fn clean_machine(m: &str) -> String {
    clean_clause(m).to_lowercase()
}

pub fn file_name(row: &str, clause: &str) -> String {
    format!("{row}.{}.json", clean_clause(clause))
}

/// Where the evidence of `machine` is kept when this machine is `own`: the machine's own answers are in `human/`, answers imported from
/// another machine in `human/<machine>/` (one file per row and clause would otherwise be replaced by the other machine's).
pub fn evidence_path(ev: &Evidence, own: &str) -> String {
    if clean_machine(&ev.machine) == clean_machine(own) { format!("{HUMAN_DIR}/{}", file_name(&ev.row, &ev.clause)) } else { format!("{HUMAN_DIR}/{}/{}", clean_machine(&ev.machine), file_name(&ev.row, &ev.clause)) }
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

/// Who is answering and on what.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Who {
    pub who: String,
    pub admin: bool,
    pub machine: String,
    pub image_commit: String,
}

pub fn make_evidence(sc: &Scenario, row: &str, clause: &str, v: Verdict, note: &str, who: &Who, now: i64) -> Option<Evidence> {
    if v == Verdict::Skip {
        return None;
    }
    let note: String = note.chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect();
    Some(Evidence {
        schema: EVIDENCE_SCHEMA.into(),
        row: row.into(),
        clause: clause.into(),
        scenario: sc.id.clone(),
        machine: who.machine.clone(),
        image_commit: who.image_commit.clone(),
        who: who.who.clone(),
        by: if who.admin { "owner".into() } else { "person".into() },
        result: v.word().into(),
        verdict: v.word().into(),
        note,
        date: iso(now),
        when: now,
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
    let path = evidence_path(ev, own);
    let text = serde_json::to_string_pretty(ev).map_err(|e| e.to_string())?;
    store.write(&path, &text)?;
    Ok(path)
}

fn parse_evidence(text: &str) -> Option<Evidence> {
    let e: Evidence = serde_json::from_str(text).ok()?;
    (e.schema == EVIDENCE_SCHEMA && !e.row.is_empty() && Verdict::parse(&e.result).is_some_and(|v| v != Verdict::Skip)).then_some(e)
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
                .filter_map(|(r, c)| evidence.iter().find(|e| e.row == *r && clean_clause(&e.clause) == clean_clause(c) && clean_machine(&e.machine) == clean_machine(own)).and_then(|e| Verdict::parse(&e.result)))
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
                    Some(o) if o.when >= e.when => rep.kept += 1,
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
            ("scenario: A\ntitle: t\narea: a\nrows: F-X-1x\nstep: s\nexpect: e\n", "not a feature row"),
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
        assert_eq!(file_name("F-SND-01", "tone"), "F-SND-01.tone.json");
        assert_eq!(file_name("F-SND-01", "left right!"), "F-SND-01.left-right.json");
        assert_eq!(file_name("F-X-01", "///"), "F-X-01.clause.json");
        assert_eq!(clean_machine("Mac Mini #2"), "mac-mini-2");
    }

    #[test]
    fn the_evidence_schema_round_trips_and_a_skip_is_never_evidence() {
        let s = sc();
        let st = MemStore::default();
        let e = make_evidence(&s, "F-SND-01", "tone", Verdict::Pass, "clear\nnotes\u{7}", &who(true, "iyakki-aaa"), 1_800_000_000).unwrap();
        assert_eq!((e.by.as_str(), e.result.as_str(), e.verdict.as_str(), e.note.as_str(), e.date.as_str()), ("owner", "pass", "pass", "clearnotes", "2027-01-15T08:00:00"));
        assert_eq!(make_evidence(&s, "F-SND-01", "tone", Verdict::Skip, "", &who(true, "m"), 1), None);
        assert_eq!(make_evidence(&s, "F-SND-01", "tone", Verdict::Fail, "", &who(false, "m"), 1).unwrap().by, "person");
        let p = record(&st, &e, "iyakki-aaa").unwrap();
        assert_eq!(p, "manual-tests/human/F-SND-01.tone.json");
        let back = load_evidence(&st);
        assert_eq!(back, vec![e.clone()]);
        let text = st.read(&p).unwrap().unwrap();
        for key in ["\"row\"", "\"clause\"", "\"machine\"", "\"by\"", "\"result\"", "\"note\"", "\"date\"", "\"who\"", "\"image_commit\"", "\"verdict\""] {
            assert!(text.contains(key), "{key}");
        }
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
        assert!(b.names().contains(&"manual-tests/human/m1/F-SND-01.tone.json".to_string()));
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
        assert_eq!(ev.iter().find(|e| e.machine == "m1" && e.clause == "stop").unwrap().result, "pass");
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
}
