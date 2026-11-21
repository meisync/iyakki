//! The lead assistant's tools for the DOCTOR "Manual tests" tab (chat reach for F-DOCTOR-04): list the checklists a person goes through,
//! open one (its steps and what to expect), record a person's answer, and export the answers as one file.
//!
//! Reading is free. Recording and exporting always ask (`Risk::Ask`): a verdict is evidence that a person tried the thing, so the assistant
//! may only write down what the person told it they saw or heard, and the card shows the exact words before anything is written. Like
//! the tab, the tools only make the human level of evidence (`manual-tests/human/<ROW>.<clause>.json` in the person's home).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_shared::manualtest::{self as mt, ChkParser, EvidenceStore, Filter, Scenario, State, Verdict, Who};
pub use iyakki_shared::manualtest::PROMPT_RULE;

pub const LIST_TOOL: &str = "manual_tests_list";
pub const OPEN_TOOL: &str = "manual_test_open";
pub const RECORD_TOOL: &str = "manual_test_record";
pub const EXPORT_TOOL: &str = "manual_tests_export";

pub fn is_manual_tool(n: &str) -> bool {
    matches!(n, LIST_TOOL | OPEN_TOOL | RECORD_TOOL | EXPORT_TOOL)
}

pub trait Backend: Send + Sync {
    /// May this person open DOCTOR (the tab's rule)?
    fn may_use(&self) -> bool;
    /// The bundled checklists and the files that did not parse.
    fn scenarios(&self) -> (Vec<Scenario>, Vec<String>);
    fn who(&self) -> Who;
    fn hardware(&self) -> String;
    fn store(&self) -> &dyn EvidenceStore;
    fn now(&self) -> i64;
    fn audit(&self, action: &str, detail: &str);
}

pub struct ManualTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl ManualTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ManualTools {
        ManualTools { inner, backend }
    }

    fn find(&self, args: &Value) -> Result<Scenario, String> {
        let id = args["scenario"].as_str().ok_or("name the scenario (its id from manual_tests_list)")?.trim();
        let (all, _) = self.backend.scenarios();
        all.into_iter().find(|s| s.id.eq_ignore_ascii_case(id)).ok_or_else(|| format!("there is no checklist `{id}`; manual_tests_list shows the ids"))
    }

    fn states(&self, all: &[Scenario]) -> Vec<(State, Vec<Option<Verdict>>)> {
        let ev = mt::load_evidence(self.backend.store());
        let m = self.backend.who().machine;
        all.iter().map(|s| (s, mt::step_verdicts(s, &ev, &m))).map(|(s, v)| (mt::scenario_state(&mt::scored(s, &v)), v)).collect()
    }

    fn list(&self, args: &Value) -> Result<String, String> {
        let (all, bad) = self.backend.scenarios();
        let st = self.states(&all);
        let states: Vec<State> = st.iter().map(|x| x.0).collect();
        let f = Filter { this_machine: args["this_machine"].as_bool().unwrap_or(false), open_only: args["needs_person_only"].as_bool().unwrap_or(false) };
        let want_state = args["state"].as_str().map(str::to_lowercase);
        let area = args["area"].as_str().map(str::to_lowercase);
        let mut out = Vec::new();
        for i in mt::visible(&all, &states, &self.backend.hardware(), f) {
            if want_state.as_deref().is_some_and(|w| w != states[i].label()) || area.as_deref().is_some_and(|a| all[i].area.to_lowercase() != a) {
                continue;
            }
            out.push(format!("{} | {} | {} | {} | {} | {} steps", all[i].id, all[i].area, all[i].title, states[i].label(), all[i].machines_text(), all[i].steps.len()));
        }
        let idx: Vec<usize> = (0..all.len()).collect();
        let verd: Vec<Vec<Option<Verdict>>> = st.iter().map(|x| x.1.clone()).collect();
        let (done, total) = mt::progress(&all, &verd, &idx);
        let mut text = format!("{done} of {total} steps answered on this machine.\n");
        if out.is_empty() {
            text.push_str("No checklist matches.\n");
        }
        for l in out {
            text.push_str(&l);
            text.push('\n');
        }
        if !bad.is_empty() {
            text.push_str(&format!("{} checklist file(s) could not be read.\n", bad.len()));
        }
        Ok(text)
    }

    fn open(&self, args: &Value) -> Result<String, String> {
        let sc = self.find(args)?;
        let ev = mt::load_evidence(self.backend.store());
        let v = mt::step_verdicts(&sc, &ev, &self.backend.who().machine);
        let mut t = format!("{} ({}) - {}\nRows covered: {}\nFor: {}\nState: {}\n", sc.title, sc.id, sc.area, sc.rows.join(", "), sc.machines_text(), mt::scenario_state(&mt::scored(&sc, &v)).label());
        for p in &sc.prereq {
            t.push_str(&format!("Before you start: {p}\n"));
        }
        for (i, s) in sc.steps.iter().enumerate() {
            t.push_str(&format!("{}. {}\n   Expect: {}\n", i + 1, s.text, s.expect));
            if !s.clause.is_empty() {
                t.push_str(&format!("   Evidence for: {}\n", s.clause));
            }
            if let Some(a) = v[i] {
                t.push_str(&format!("   Answer: {}\n", a.label()));
            }
        }
        Ok(t)
    }

    /// (scenario, the steps (0-based) and the verdict) from the arguments.
    fn parse_record(&self, args: &Value) -> Result<(Scenario, Vec<usize>, Verdict, String), String> {
        let sc = self.find(args)?;
        let v = Verdict::parse(args["verdict"].as_str().ok_or("say the verdict: pass, fail, cannot-test or skip")?).ok_or("the verdict is pass, fail, cannot-test or skip")?;
        let steps: Vec<usize> = match &args["step"] {
            Value::String(s) if s.eq_ignore_ascii_case("all") => (0..sc.steps.len()).collect(),
            Value::Number(n) => {
                let k = n.as_u64().ok_or("the step is a number from 1")? as usize;
                if k == 0 || k > sc.steps.len() {
                    return Err(format!("{} has steps 1 to {}", sc.id, sc.steps.len()));
                }
                vec![k - 1]
            }
            _ => return Err("name the step (1, 2, ...) or \"all\"".into()),
        };
        let note: String = args["note"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(mt::MAX_NOTE).collect();
        Ok((sc, steps, v, note))
    }

    fn record(&self, args: &Value) -> Result<String, String> {
        let (sc, steps, v, note) = self.parse_record(args)?;
        let who = self.backend.who();
        let mut wrote = 0;
        for &i in &steps {
            for (row, clause) in sc.targets(i) {
                if let Some(e) = mt::make_evidence(&sc, &row, &clause, v, &note, &who, self.backend.now()) {
                    mt::record(self.backend.store(), &e, &who.machine)?;
                    wrote += 1;
                }
            }
        }
        self.backend.audit("manual.record", &format!("{} {} {} (by chat)", sc.id, if steps.len() == 1 { format!("step {}", steps[0] + 1) } else { "all steps".into() }, v.word()));
        Ok(if v == Verdict::Skip { "Skipped: nothing is written for a skip.".to_string() } else { format!("Recorded {} for {} ({} evidence file{} written).", v.label(), sc.id, wrote, if wrote == 1 { "" } else { "s" }) })
    }

    fn export(&self) -> Result<String, String> {
        let who = self.backend.who();
        let p = mt::export(self.backend.store(), &who.machine, self.backend.now())?;
        self.backend.audit("manual.export", &format!("{p} (by chat)"));
        Ok(format!("Exported to {p} in the home folder: one file the team can collect."))
    }

    fn gate(&self) -> Result<(), String> {
        if self.backend.may_use() { Ok(()) } else { Err("this account may not open DOCTOR".into()) }
    }
}

impl Tools for ManualTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }

    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_manual_tool(&d.spec.name));
        let mut add = |name: &str, desc: &str, schema: Value, risk: Risk| v.push(ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false });
        add(LIST_TOOL, "List the manual test checklists a person goes through (sound by ear, touch, Bluetooth, camera, hotspot, ...), each with its area, state on this computer and the machines it is for. Reads only. Filters: area, state, this_machine, needs_person_only.", json!({"type": "object", "properties": {"area": {"type": "string"}, "state": {"type": "string", "enum": ["not started", "in progress", "pass", "fail", "cannot test"]}, "this_machine": {"type": "boolean"}, "needs_person_only": {"type": "boolean"}}}), Risk::Safe);
        add(OPEN_TOOL, "Show one manual test checklist: the rows it covers, what to do before, each numbered step with what the person should see or hear, and the answers recorded so far. Reads only.", json!({"type": "object", "properties": {"scenario": {"type": "string"}}, "required": ["scenario"]}), Risk::Safe);
        add(RECORD_TOOL, "Record the person's own answer for one step (or all) of a manual test: pass, fail, cannot-test or skip, with an optional note. Write down ONLY what the person told you they saw or heard; never guess or assume a result. Always asks first. A skip writes nothing.", json!({"type": "object", "properties": {"scenario": {"type": "string"}, "step": {"description": "the step number from 1, or \"all\""}, "verdict": {"type": "string", "enum": ["pass", "fail", "cannot-test", "skip"]}, "note": {"type": "string"}}, "required": ["scenario", "step", "verdict"]}), Risk::Ask);
        add(EXPORT_TOOL, "Write every recorded manual-test answer as one file in the person's home folder (manual-tests/export) for the team to collect. Always asks first.", json!({"type": "object", "properties": {}}), Risk::Ask);
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST_TOOL => self.gate().and_then(|_| self.list(args)),
            OPEN_TOOL => self.gate().and_then(|_| self.open(args)),
            RECORD_TOOL => self.gate().and_then(|_| self.record(args)),
            EXPORT_TOOL => self.gate().and_then(|_| self.export()),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            LIST_TOOL => "List the manual test checklists".into(),
            OPEN_TOOL => "Show a manual test checklist".into(),
            RECORD_TOOL => match self.parse_record(args) {
                Ok((sc, steps, v, _)) => format!("Record {} for {} {}", v.label(), sc.id, if steps.len() == 1 { format!("step {}", steps[0] + 1) } else { "(every step)".into() }),
                Err(_) => "Record an answer for a manual test".into(),
            },
            EXPORT_TOOL => "Export the manual test answers as one file".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            RECORD_TOOL => Some(match self.parse_record(args) {
                Ok((sc, steps, v, note)) => {
                    let mut t = format!("This writes evidence that a person tried it: {} for {}, ", v.label(), sc.id);
                    t.push_str(&if steps.len() == 1 { format!("step {}: {}\nExpected: {}", steps[0] + 1, sc.steps[steps[0]].text, sc.steps[steps[0]].expect) } else { format!("every step ({}).", steps.len()) });
                    if !note.is_empty() {
                        t.push_str(&format!("\nNote: {note}"));
                    }
                    t.push_str("\nOnly say yes if you did this and that is what you saw or heard.");
                    t
                }
                Err(e) => format!("Cannot record: {e}"),
            }),
            EXPORT_TOOL => Some("Writes one file with every recorded answer into manual-tests/export in your home folder. Nothing is sent anywhere.".into()),
            LIST_TOOL | OPEN_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            LIST_TOOL | EXPORT_TOOL => self.gate().err(),
            OPEN_TOOL => self.gate().and_then(|_| self.find(args).map(|_| ())).err(),
            RECORD_TOOL => self.gate().and_then(|_| self.parse_record(args).map(|_| ())).err(),
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_manual_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !matches!(name, RECORD_TOOL | EXPORT_TOOL) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == RECORD_TOOL {
            Some("a recorded answer says a person tried it, so it asks every time".into())
        } else {
            self.inner.grant_blocker(name, args)
        }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

// ------------------------------------------------------------------------------------------------ the real backend

/// The person's home folder as an [`EvidenceStore`]: plain names inside the folder, no links, no hidden names.
pub struct FsStore {
    pub root: PathBuf,
}

impl FsStore {
    fn path(&self, rel: &str) -> Result<PathBuf, String> {
        iyakki_core_api::files::check_rel(rel)?;
        let mut p = self.root.clone();
        for n in rel.split('/').filter(|n| !n.is_empty()) {
            p.push(n);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("that path goes through a link, which is not followed".into());
            }
        }
        Ok(p)
    }
}

impl EvidenceStore for FsStore {
    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        let p = self.path(rel)?;
        match std::fs::read(&p) {
            Ok(b) if b.len() <= iyakki_core_api::files::MAX_READ => String::from_utf8(b).map(Some).map_err(|_| "that file is not text".to_string()),
            Ok(_) => Err("that file is too big".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("that file cannot be read".into()),
        }
    }
    fn write(&self, rel: &str, text: &str) -> Result<(), String> {
        let p = self.path(rel)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|_| "the folder could not be made".to_string())?;
        }
        let tmp = p.with_extension("saving");
        std::fs::write(&tmp, text).map_err(|_| "the file could not be written".to_string())?;
        std::fs::rename(&tmp, &p).map_err(|_| {
            let _ = std::fs::remove_file(&tmp);
            "the file could not be written".to_string()
        })
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        self.entries(dir, false)
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>, String> {
        self.entries(dir, true)
    }
}

impl FsStore {
    fn entries(&self, dir: &str, folders: bool) -> Result<Vec<String>, String> {
        let p = self.path(dir)?;
        let mut v: Vec<String> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink() && t.is_dir() == folders))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort();
        Ok(v)
    }
}

pub struct RealBackend {
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub store: FsStore,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl RealBackend {
    pub fn new(policy: iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> RealBackend {
        RealBackend { policy, user: user.into(), store: FsStore { root: crate::scope::home_root_for_writes(user, data_dir) }, audit }
    }
}

impl Backend for RealBackend {
    fn may_use(&self) -> bool {
        self.policy.allows_app(iyakki_sec::policy::App::Doctor)
    }
    fn scenarios(&self) -> (Vec<Scenario>, Vec<String>) {
        let mut names: Vec<String> = std::fs::read_dir(mt::BUNDLED_DIR).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".chk")).collect();
        names.sort();
        let files: Vec<(String, String)> = names.into_iter().filter_map(|n| std::fs::read_to_string(format!("{}/{n}", mt::BUNDLED_DIR)).ok().map(|t| (n, t))).collect();
        mt::load_all(&ChkParser, &files)
    }
    fn who(&self) -> Who {
        let rel = iyakki_shared::relinfo::ReleaseInfo::load(std::path::Path::new(iyakki_shared::relinfo::RELEASE_FILE));
        let machine = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "this-computer".into());
        Who { who: self.user.clone(), admin: self.policy.manage_settings, machine, image_commit: rel.map(|r| r.git_describe).unwrap_or_default() }
    }
    fn hardware(&self) -> String {
        iyakki_shared::doctor::hardware_text()
    }
    fn store(&self) -> &dyn EvidenceStore {
        &self.store
    }
    fn now(&self) -> i64 {
        iyakki_clock::now_unix()
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::manualtest::MemStore;

    const SC: &str = "scenario: SND-01\ntitle: Sound by ear\narea: Sound\nrows: F-SND-01, F-SND-02\nmachines: mac mini\nneeds: person\nprereq: Headphones are plugged in.\nstep: Press Play test tone.\nexpect: Two notes, left then right.\nclause: F-SND-01.tone\nstep: Press Stop.\nexpect: The sound stops.\nclause: stop\n";
    const OTHER: &str = "scenario: HOT-01\ntitle: Hotspot\narea: Network\nrows: F-HOTSPOT-12\nmachines: lenovo\nstep: Switch it on.\nexpect: It says on.\nclause: on\n";

    struct Fake {
        may: bool,
        store: MemStore,
        audit: std::sync::Mutex<Vec<String>>,
    }
    impl Backend for Fake {
        fn may_use(&self) -> bool {
            self.may
        }
        fn scenarios(&self) -> (Vec<Scenario>, Vec<String>) {
            mt::load_all(&ChkParser, &[("a.chk".into(), SC.into()), ("b.chk".into(), OTHER.into()), ("c.chk".into(), "junk".into())])
        }
        fn who(&self) -> Who {
            Who { who: "ana".into(), admin: true, machine: "m1".into(), image_commit: "abc".into() }
        }
        fn hardware(&self) -> String {
            "Apple Mac mini".into()
        }
        fn store(&self) -> &dyn EvidenceStore {
            &self.store
        }
        fn now(&self) -> i64 {
            1_800_000_000
        }
        fn audit(&self, a: &str, d: &str) {
            self.audit.lock().unwrap().push(format!("{a} {d}"));
        }
    }

    struct Empty;
    impl Tools for Empty {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _: &Value) -> Result<String, String> {
            Err(format!("inner got {name}"))
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            name.into()
        }
    }

    fn rig(may: bool) -> (ManualTools, Arc<Fake>) {
        let f = Arc::new(Fake { may, store: MemStore::default(), audit: Default::default() });
        (ManualTools::new(Arc::new(Empty), f.clone()), f)
    }

    #[test]
    fn reading_is_free_and_writing_always_asks() {
        let (t, _) = rig(true);
        let d = t.defs();
        assert_eq!(d.len(), 4);
        for x in &d {
            let writes = x.spec.name == RECORD_TOOL || x.spec.name == EXPORT_TOOL;
            assert_eq!(matches!(x.risk, Risk::Ask), writes, "{}", x.spec.name);
        }
        assert!(!t.pre_approved(RECORD_TOOL) && !t.approved_always(RECORD_TOOL) && !t.approved_always(EXPORT_TOOL));
        assert!(t.grant_blocker(RECORD_TOOL, &json!({})).is_some());
        assert!(t.call("other", &json!({})).unwrap_err().contains("inner got"));
    }

    #[test]
    fn list_filters_by_machine_state_and_area() {
        let (t, _) = rig(true);
        let all = t.call(LIST_TOOL, &json!({})).unwrap();
        assert!(all.contains("SND-01 | Sound | Sound by ear | not started") && all.contains("HOT-01") && all.contains("0 of 3 steps") && all.contains("1 checklist file(s) could not be read"), "{all}");
        let mine = t.call(LIST_TOOL, &json!({"this_machine": true})).unwrap();
        assert!(mine.contains("SND-01") && !mine.contains("HOT-01"), "{mine}");
        assert!(t.call(LIST_TOOL, &json!({"area": "network"})).unwrap().contains("HOT-01"));
        assert!(t.call(LIST_TOOL, &json!({"state": "pass"})).unwrap().contains("No checklist matches"));
    }

    #[test]
    fn open_shows_the_steps_and_what_to_expect_and_a_wrong_id_is_refused() {
        let (t, _) = rig(true);
        let o = t.call(OPEN_TOOL, &json!({"scenario": "snd-01"})).unwrap();
        assert!(o.contains("1. Press Play test tone.") && o.contains("Expect: Two notes, left then right.") && o.contains("Before you start: Headphones"), "{o}");
        assert!(t.call(OPEN_TOOL, &json!({"scenario": "nope"})).unwrap_err().contains("no checklist"));
        assert!(t.refusal(OPEN_TOOL, &json!({"scenario": "nope"})).is_some());
        assert!(t.call(OPEN_TOOL, &json!({})).is_err());
    }

    #[test]
    fn recording_writes_the_human_evidence_after_the_card_and_audits() {
        let (t, f) = rig(true);
        let a = json!({"scenario": "SND-01", "step": 1, "verdict": "pass", "note": "clear\u{7} sound"});
        let card = t.preview(RECORD_TOOL, &a).unwrap();
        assert!(card.contains("Pass for SND-01, step 1: Press Play test tone.") && card.contains("Expected: Two notes") && card.contains("Note: clear sound") && card.contains("Only say yes"), "{card}");
        assert!(t.describe(RECORD_TOOL, &a).contains("Record Pass for SND-01 step 1"));
        assert!(f.store.names().is_empty(), "the card writes nothing");
        assert!(t.call(RECORD_TOOL, &a).unwrap().contains("1 evidence file"));
        assert_eq!(f.store.names(), ["manual-tests/human/F-SND-01.tone.json"]);
        let e = &mt::load_evidence(&f.store)[0];
        assert_eq!((e.who.as_str(), e.result.as_str(), e.note.as_str(), e.machine.as_str()), ("ana", "pass", "clear sound", "m1"));
        assert!(f.audit.lock().unwrap().join("\n").contains("manual.record SND-01 step 1 pass (by chat)"));
        // the list now says in progress, and "all" fails the rest (a bare clause covers both rows)
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("in progress"));
        t.call(RECORD_TOOL, &json!({"scenario": "SND-01", "step": "all", "verdict": "fail"})).unwrap();
        assert_eq!(f.store.names().len(), 3, "the tone file is replaced, the bare clause writes for both rows");
        assert!(t.call(OPEN_TOOL, &json!({"scenario": "SND-01"})).unwrap().contains("State: fail"));
    }

    #[test]
    fn a_skip_writes_nothing_and_bad_arguments_are_refused_with_a_sentence() {
        let (t, f) = rig(true);
        assert!(t.call(RECORD_TOOL, &json!({"scenario": "SND-01", "step": 1, "verdict": "skip"})).unwrap().contains("nothing is written"));
        assert!(f.store.names().is_empty());
        for bad in [json!({"scenario": "SND-01", "step": 0, "verdict": "pass"}), json!({"scenario": "SND-01", "step": 9, "verdict": "pass"}), json!({"scenario": "SND-01", "step": 1, "verdict": "maybe"}), json!({"scenario": "SND-01", "verdict": "pass"}), json!({"scenario": "SND-01", "step": "two", "verdict": "pass"})] {
            assert!(t.refusal(RECORD_TOOL, &bad).is_some(), "{bad}");
            assert!(t.call(RECORD_TOOL, &bad).is_err(), "{bad}");
        }
        assert!(f.store.names().is_empty());
    }

    #[test]
    fn export_writes_one_file_and_an_account_without_doctor_gets_nothing() {
        let (t, f) = rig(true);
        t.call(RECORD_TOOL, &json!({"scenario": "SND-01", "step": 1, "verdict": "pass"})).unwrap();
        let o = t.call(EXPORT_TOOL, &json!({})).unwrap();
        assert!(o.contains("manual-tests/export/manual-evidence-m1-2027-01-15.json"), "{o}");
        assert!(t.preview(EXPORT_TOOL, &json!({})).unwrap().contains("Nothing is sent anywhere"));
        assert!(f.audit.lock().unwrap().join("\n").contains("manual.export"));
        let (t, f) = rig(false);
        for n in [LIST_TOOL, OPEN_TOOL, RECORD_TOOL, EXPORT_TOOL] {
            assert!(t.call(n, &json!({"scenario": "SND-01", "step": 1, "verdict": "pass"})).unwrap_err().contains("may not open DOCTOR"), "{n}");
            assert!(t.refusal(n, &json!({"scenario": "SND-01", "step": 1, "verdict": "pass"})).is_some(), "{n}");
        }
        assert!(f.store.names().is_empty());
    }

    #[test]
    fn the_folder_store_keeps_to_plain_names_makes_folders_and_never_follows_a_link() {
        let root = std::env::temp_dir().join(format!("iyakki-manualtools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let s = FsStore { root: root.clone() };
        s.write("manual-tests/human/F-A-01.c.json", "{}").unwrap();
        assert_eq!(s.read("manual-tests/human/F-A-01.c.json").unwrap().as_deref(), Some("{}"));
        assert_eq!(s.read("manual-tests/human/none.json").unwrap(), None);
        assert_eq!(s.list("manual-tests/human").unwrap(), ["F-A-01.c.json"]);
        assert_eq!(s.folders("manual-tests").unwrap(), ["human"]);
        assert!(s.list("absent").unwrap().is_empty());
        assert!(s.write("../x.json", "{}").is_err() && s.read("/etc/passwd").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("lnk")).unwrap();
            assert!(s.write("lnk/x.json", "{}").is_err());
            assert!(s.read("lnk/x.json").is_err());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
