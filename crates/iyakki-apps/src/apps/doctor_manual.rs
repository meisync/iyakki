//! DOCTOR, tab "Manual tests": the checklists a PERSON goes through (sound by ear, touch, Bluetooth pairing, a microphone, a camera, a 4K
//! link, a hotspot with a second device, writing a startup disk, ...). The left side lists the scenarios by area with their state and the
//! machines they apply to; the main canvas shows the chosen one with numbered steps and the expected result under each.
//!
//! * The checklists ship read-only in the image (`dist/manual/*.chk`, parsed by `iyakki_shared::manualtest`); the tab asks its port for
//!   them and never reads a system path itself.
//! * A verdict (Pass, Fail, Cannot test) is the person's own click or key, and it writes the human evidence file
//!   `manual-tests/human/<ROW>.<clause>.json` into the person's home through the core's file handles (audited, file scope applied).
//!   Skip writes nothing. Export writes everything as one file the team collects; Import merges the files of other machines. This tab only
//!   ever makes the human level: unit tests, QEMU runs and machine runs are other levels and never come from here.
//! * Keys: n / p next and previous scenario, 1 Pass, 2 Fail, 3 Cannot test, 4 or s Skip (each answer moves to the next step), Enter next
//!   step, Up / Down choose a step, t edit the note (Enter or Esc ends it), f only this machine, o only what still needs a person,
//!   e export, i import, Esc back (to the list on a narrow screen, then to the findings).

use std::sync::Arc;

use iyakki_shared::manualtest::{self as mt, ChkParser, Filter, Scenario, State, Verdict, Who};

use super::doctor_port::{DoctorPort, PortStore};
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key};

pub(super) const T_NAV: usize = 1000;
pub(super) const T_STEP: usize = 2000;
/// The four verdict buttons of the chosen step (Pass, Fail, Cannot test, Skip).
pub(super) const T_SVERD: usize = 3000;
/// The same four for every step not yet answered.
pub(super) const T_AVERD: usize = 3010;
pub(super) const T_NOTE: usize = 3020;
pub(super) const T_EXPORT: usize = 3021;
pub(super) const T_IMPORT: usize = 3022;
pub(super) const T_F_MACHINE: usize = 3023;
pub(super) const T_F_OPEN: usize = 3024;
pub(super) const T_NEXT: usize = 3025;
pub(super) const T_PREV: usize = 3026;
pub(super) const T_BACKLIST: usize = 3027;
pub(super) const T_NEXT_STEP: usize = 3028;
pub(super) const T_DISMISS: usize = 3029;
pub(super) const A_NAV: usize = 61_312;
pub(super) const A_MAIN: usize = 61_313;

pub(super) const VERDICTS: [Verdict; 4] = [Verdict::Pass, Verdict::Fail, Verdict::CannotTest, Verdict::Skip];

pub struct ManualTab {
    pub(super) scenarios: Vec<Scenario>,
    /// Files that did not parse, named.
    pub(super) problems: Vec<String>,
    /// Per scenario, per step.
    pub(super) verdicts: Vec<Vec<Option<Verdict>>>,
    pub(super) notes: Vec<String>,
    pub(super) sel: usize,
    pub(super) step: usize,
    pub(super) filter: Filter,
    pub(super) hardware: String,
    pub(super) who: Who,
    pub(super) nav: ScrollList,
    pub(super) main: ScrollList,
    pub(super) msg: Option<(String, bool)>,
    pub(super) editing: bool,
    /// On a narrow screen the canvas replaces the list once a scenario is chosen.
    pub(super) opened: bool,
    /// Set by the last drawing: the screen was too narrow for both sides.
    pub(super) narrow: bool,
    pub(super) dirty: bool,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

/// What a key or click did to the tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Out {
    Stay,
    /// Leave the tab (back to the findings).
    Leave,
}

impl ManualTab {
    pub fn open(port: &dyn DoctorPort, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> ManualTab {
        let (scenarios, problems) = mt::load_all(&ChkParser, &port.manual_files());
        let n = scenarios.len();
        let mut t = ManualTab {
            verdicts: scenarios.iter().map(|s| vec![None; s.steps.len()]).collect(),
            notes: vec![String::new(); n],
            scenarios,
            problems,
            sel: 0,
            step: 0,
            filter: Filter::default(),
            hardware: port.manual_hardware(),
            who: port.manual_who(),
            nav: ScrollList::default(),
            main: ScrollList::default(),
            msg: None,
            editing: false,
            opened: false,
            narrow: false,
            dirty: true,
            now,
        };
        t.reload(port);
        if let Some(&i) = t.vis().first() {
            t.sel = i;
        }
        t
    }

    /// Read the answers kept in the person's home again (after opening, and after an import).
    pub(super) fn reload(&mut self, port: &dyn DoctorPort) {
        let ev = mt::load_evidence(&PortStore(port));
        for (i, sc) in self.scenarios.iter().enumerate() {
            let mut v = mt::step_verdicts(sc, &ev, &self.who.machine);
            // a skip made on this screen survives a reload; the files never hold one
            for (k, old) in self.verdicts[i].iter().enumerate() {
                if v[k].is_none() && *old == Some(Verdict::Skip) {
                    v[k] = Some(Verdict::Skip);
                }
            }
            self.verdicts[i] = v;
            if let Some(e) = ev.iter().filter(|e| e.scenario == sc.id && mt::clean_machine(&e.machine) == mt::clean_machine(&self.who.machine)).max_by_key(|e| e.when) {
                if self.notes[i].is_empty() {
                    self.notes[i] = e.note.clone();
                }
            }
        }
        self.dirty = true;
    }

    pub(super) fn state(&self, i: usize) -> State {
        mt::scenario_state(&mt::scored(&self.scenarios[i], &self.verdicts[i]))
    }

    pub(super) fn states(&self) -> Vec<State> {
        (0..self.scenarios.len()).map(|i| self.state(i)).collect()
    }

    /// The scenarios the filters let through, in list order.
    pub(super) fn vis(&self) -> Vec<usize> {
        mt::visible(&self.scenarios, &self.states(), &self.hardware, self.filter)
    }

    /// (answered, total steps, scenarios still open, failed scenarios) over the visible scenarios.
    pub(super) fn counts(&self) -> (usize, usize, usize, usize) {
        let vis = self.vis();
        let (done, total) = mt::progress(&self.scenarios, &self.verdicts, &vis);
        let open = vis.iter().filter(|&&i| !matches!(self.state(i), State::Pass | State::CannotTest)).count();
        let failed = vis.iter().filter(|&&i| self.state(i) == State::Fail).count();
        (done, total, open, failed)
    }

    pub(super) fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), true));
        self.dirty = true;
    }

    pub(super) fn fail(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), false));
        self.dirty = true;
    }

    pub(super) fn select(&mut self, i: usize) {
        if i < self.scenarios.len() {
            self.sel = i;
            self.step = self.verdicts[i].iter().position(Option::is_none).unwrap_or(0);
            self.main.to_top();
            self.opened = true;
            self.editing = false;
            self.dirty = true;
        }
    }

    /// Move to the next (`+1`) or previous (`-1`) visible scenario; the ends stay.
    pub(super) fn move_scenario(&mut self, dir: i32) {
        let vis = self.vis();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.sel);
        let to = match (pos, dir) {
            (Some(p), d) if d > 0 => vis[(p + 1).min(vis.len() - 1)],
            (Some(p), _) => vis[p.saturating_sub(1)],
            (None, d) if d > 0 => vis[0],
            (None, _) => vis[vis.len() - 1],
        };
        self.select(to);
    }

    /// Record `v` for step `i` of scenario `si`. A skip is remembered on screen only; any other answer writes the evidence files.
    pub(super) fn record_step(&mut self, si: usize, i: usize, v: Verdict, port: &dyn DoctorPort) -> bool {
        let Some(sc) = self.scenarios.get(si) else { return false };
        if i >= sc.steps.len() {
            return false;
        }
        if v == Verdict::Skip {
            if self.verdicts[si][i].is_none() {
                self.verdicts[si][i] = Some(Verdict::Skip);
            }
            return true;
        }
        let now = (self.now)();
        let store = PortStore(port);
        for (row, clause) in sc.targets(i) {
            let Some(ev) = mt::make_evidence(sc, &row, &clause, v, &self.notes[si], &self.who, now) else { continue };
            if let Err(e) = mt::record(&store, &ev, &self.who.machine) {
                self.fail(format!("The answer could not be saved: {e}"));
                return false;
            }
        }
        self.verdicts[si][i] = Some(v);
        port.audit("manual.record", &format!("{} step {} {}", sc.id, i + 1, v.word()));
        true
    }

    /// The person's answer for the chosen step; the tab moves on to the next step.
    pub(super) fn answer(&mut self, v: Verdict, port: &dyn DoctorPort) {
        let (si, i) = (self.sel, self.step);
        if self.scenarios.is_empty() || !self.record_step(si, i, v, port) {
            return;
        }
        let guidance = self.scenarios[si].targets(i).is_empty();
        let steps = self.scenarios[si].steps.len();
        let st = self.state(si);
        if i + 1 < steps {
            self.step = i + 1;
            self.ok(format!("Step {} {}{}.", i + 1, v.label().to_lowercase(), if guidance && v != Verdict::Skip { " (a guidance step: nothing is written)" } else { "" }));
        } else {
            self.ok(format!("Last step {}. The scenario is now: {}. Press n for the next one.", v.label().to_lowercase(), st.label()));
        }
        self.dirty = true;
    }

    /// One answer for every step that has none yet (or was skipped).
    pub(super) fn answer_all(&mut self, v: Verdict, port: &dyn DoctorPort) {
        let si = self.sel;
        if self.scenarios.is_empty() {
            return;
        }
        let mut n = 0;
        for i in 0..self.scenarios[si].steps.len() {
            if matches!(self.verdicts[si][i], None | Some(Verdict::Skip)) {
                if !self.record_step(si, i, v, port) {
                    return;
                }
                n += 1;
            }
        }
        self.ok(format!("{n} step{} marked {}.", if n == 1 { "" } else { "s" }, v.label().to_lowercase()));
    }

    pub(super) fn next_step(&mut self) {
        let Some(sc) = self.scenarios.get(self.sel) else { return };
        if self.step + 1 < sc.steps.len() {
            self.step += 1;
            self.dirty = true;
        } else {
            self.move_scenario(1);
        }
    }

    pub(super) fn export(&mut self, port: &dyn DoctorPort) {
        match mt::export(&PortStore(port), &self.who.machine, (self.now)()) {
            Ok(path) => {
                port.audit("manual.export", &path);
                self.ok(format!("Exported to {path} in your home folder. Send that one file to the team."));
            }
            Err(e) => self.fail(format!("Export failed: {e}")),
        }
    }

    pub(super) fn import(&mut self, port: &dyn DoctorPort) {
        let store = PortStore(port);
        // a stick that DISKS opened sits under media/
        let mut extra = Vec::new();
        for f in port.store_folders("media").unwrap_or_default().into_iter().take(4) {
            extra.push(format!("media/{f}"));
            extra.push(format!("media/{f}/{}", mt::EXPORT_DIR));
        }
        let r = mt::import(&store, &self.who.machine, &extra);
        port.audit("manual.import", &format!("{} files, {} new, {} newer", r.files, r.added, r.updated));
        self.reload(port);
        if r.files == 0 {
            self.fail(format!("No file to merge. Put the other machines' files in the folder {} in your home folder, or on a stick DISKS opened.", mt::IMPORT_DIR));
        } else if !r.refused.is_empty() && r.added + r.updated == 0 {
            self.fail(format!("Nothing merged: {}", r.refused.join("; ")));
        } else {
            let mut m = format!("Merged {} file{}: {} new, {} newer, {} already here.", r.files, if r.files == 1 { "" } else { "s" }, r.added, r.updated, r.kept);
            if !r.refused.is_empty() {
                m.push_str(&format!(" Skipped: {}.", r.refused.join("; ")));
            }
            self.ok(m);
        }
    }

    fn type_note(&mut self, c: char) {
        if !c.is_control() && self.notes[self.sel].chars().count() < mt::MAX_NOTE {
            self.notes[self.sel].push(c);
        }
        self.dirty = true;
    }

    pub(super) fn handle(&mut self, ev: &Event, port: &dyn DoctorPort) -> Out {
        self.dirty = true;
        if self.editing && !self.scenarios.is_empty() {
            match ev {
                Event::Key(Key::Esc) | Event::Key(Key::Enter) | Event::Tab(T_NOTE) => self.editing = false,
                Event::Key(Key::Backspace) => {
                    self.notes[self.sel].pop();
                }
                Event::Key(Key::Char(c)) => self.type_note(*c),
                Event::Paste(s) => s.chars().for_each(|c| self.type_note(c)),
                _ => {}
            }
            return Out::Stay;
        }
        match ev {
            Event::Key(Key::Esc) => {
                if self.narrow && self.opened {
                    self.opened = false;
                } else {
                    return Out::Leave;
                }
            }
            Event::Tab(T_BACKLIST) => self.opened = false,
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(n) if (T_NAV..T_NAV + 900).contains(n) => self.select(*n - T_NAV),
            Event::Tab(n) if (T_STEP..T_STEP + 900).contains(n) => {
                let i = *n - T_STEP;
                if self.scenarios.get(self.sel).is_some_and(|s| i < s.steps.len()) {
                    self.step = i;
                }
            }
            Event::Tab(n) if (T_SVERD..T_SVERD + 4).contains(n) => self.answer(VERDICTS[*n - T_SVERD], port),
            Event::Tab(n) if (T_AVERD..T_AVERD + 4).contains(n) => self.answer_all(VERDICTS[*n - T_AVERD], port),
            Event::Tab(T_NOTE) => self.editing = !self.scenarios.is_empty(),
            Event::Tab(T_EXPORT) | Event::Key(Key::Char('e')) => self.export(port),
            Event::Tab(T_IMPORT) | Event::Key(Key::Char('i')) => self.import(port),
            Event::Tab(T_F_MACHINE) | Event::Key(Key::Char('f')) => self.toggle_filter(true),
            Event::Tab(T_F_OPEN) | Event::Key(Key::Char('o')) => self.toggle_filter(false),
            Event::Tab(T_NEXT) | Event::Key(Key::Char('n')) => self.move_scenario(1),
            Event::Tab(T_PREV) | Event::Key(Key::Char('p')) => self.move_scenario(-1),
            Event::Tab(T_NEXT_STEP) | Event::Key(Key::Enter) => self.next_step(),
            Event::Key(Key::Char('1')) => self.answer(Verdict::Pass, port),
            Event::Key(Key::Char('2')) => self.answer(Verdict::Fail, port),
            Event::Key(Key::Char('3')) => self.answer(Verdict::CannotTest, port),
            Event::Key(Key::Char('4')) | Event::Key(Key::Char('s')) => self.answer(Verdict::Skip, port),
            Event::Key(Key::Char('t')) => self.editing = !self.scenarios.is_empty(),
            Event::Key(Key::Up) => self.step = self.step.saturating_sub(1),
            Event::Key(Key::Down) => {
                let last = self.scenarios.get(self.sel).map_or(0, |s| s.steps.len().saturating_sub(1));
                self.step = (self.step + 1).min(last);
            }
            Event::Key(k) => {
                if !self.main.key(k) {
                    self.nav.key(k);
                }
            }
            other => {
                if !self.main.event(other, A_MAIN) {
                    self.nav.event(other, A_NAV);
                }
            }
        }
        Out::Stay
    }

    fn toggle_filter(&mut self, machine: bool) {
        if machine {
            self.filter.this_machine = !self.filter.this_machine;
        } else {
            self.filter.open_only = !self.filter.open_only;
        }
        // keep the chosen scenario on screen: when it is filtered out the first visible one is chosen
        let vis = self.vis();
        if !vis.contains(&self.sel) {
            if let Some(&i) = vis.first() {
                self.select(i);
                self.opened = false;
            }
        }
        self.nav.to_top();
    }
}
