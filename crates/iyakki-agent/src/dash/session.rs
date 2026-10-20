//! The dashboard that is open on the screen, shared between the screen and the assistant's tools. Every change goes through
//! [`DashState::edit`], which keeps an undo stack, bumps a revision number the screen polls, and remembers which panel to
//! highlight. The assistant sees the dashboard only through [`context_text`]: compact, with no secrets (sources by name and
//! kind only).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use super::frame::{clip, Frame};
use super::model::*;

pub const UNDO_DEPTH: usize = 50;
/// The context sent to the model never exceeds this many characters (about 800 tokens).
pub const CONTEXT_CHARS: usize = 3200;

/// What the last run of a panel produced, in words and numbers (never the data itself).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PanelResult {
    pub summary: String,
    pub error: Option<String>,
    pub at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct SourceBrief {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub language: String,
}

#[derive(Debug)]
pub struct DashState {
    pub dashboard: Dashboard,
    pub selected: Option<String>,
    pub results: HashMap<String, PanelResult>,
    pub errors: VecDeque<String>,
    undo: Vec<Dashboard>,
    redo: Vec<Dashboard>,
    /// Panel to flash, and the revision at which it was set.
    pub highlight: Option<String>,
    /// Bumped by every change of the document; the screen redraws and re-runs panels when it moves.
    pub rev: u64,
    pub saved_rev: u64,
    /// A window the person pointed at ("Ask AI about this"): from and to in epoch milliseconds.
    pub focus_window: Option<(i64, i64)>,
}

pub type Shared = Arc<Mutex<DashState>>;

pub fn shared(d: Dashboard) -> Shared {
    Arc::new(Mutex::new(DashState::new(d)))
}

impl DashState {
    pub fn new(d: Dashboard) -> DashState {
        DashState { dashboard: d, selected: None, results: HashMap::new(), errors: VecDeque::new(), undo: vec![], redo: vec![], highlight: None, rev: 1, saved_rev: 1, focus_window: None }
    }

    pub fn dirty(&self) -> bool {
        self.rev != self.saved_rev
    }

    pub fn mark_saved(&mut self) {
        self.saved_rev = self.rev;
    }

    /// Replace the whole document (opening another dashboard): history and results restart.
    pub fn load(&mut self, d: Dashboard) {
        self.dashboard = d;
        self.undo.clear();
        self.redo.clear();
        self.results.clear();
        self.selected = None;
        self.highlight = None;
        self.rev += 1;
        self.saved_rev = self.rev;
    }

    /// Change the dashboard with an undo step. If `f` fails nothing changes and no step is kept.
    pub fn edit<T>(&mut self, f: impl FnOnce(&mut Dashboard) -> Result<T, String>) -> Result<T, String> {
        let before = self.dashboard.clone();
        match f(&mut self.dashboard) {
            Ok(v) => {
                if self.dashboard != before {
                    self.undo.push(before);
                    if self.undo.len() > UNDO_DEPTH {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.rev += 1;
                }
                Ok(v)
            }
            Err(e) => {
                self.dashboard = before;
                Err(e)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.dashboard, prev));
                self.fix_selection();
                self.rev += 1;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.dashboard, next));
                self.fix_selection();
                self.rev += 1;
                true
            }
            None => false,
        }
    }

    fn fix_selection(&mut self) {
        if self.selected.as_ref().is_some_and(|s| self.dashboard.panel(s).is_none()) {
            self.selected = None;
        }
        if self.highlight.as_ref().is_some_and(|s| self.dashboard.panel(s).is_none()) {
            self.highlight = None;
        }
    }

    pub fn note_result(&mut self, panel: &str, f: &Frame, now_ms: i64) {
        self.results.insert(panel.to_string(), PanelResult { summary: f.summary(), error: None, at_ms: now_ms });
    }

    pub fn note_error(&mut self, panel: &str, title: &str, e: &str, now_ms: i64) {
        self.results.insert(panel.to_string(), PanelResult { summary: String::new(), error: Some(e.to_string()), at_ms: now_ms });
        self.errors.push_back(format!("{title}: {}", clip(e, 160)));
        while self.errors.len() > 5 {
            self.errors.pop_front();
        }
    }
}

/// The compact text the assistant gets with every question. Pure function of the state and the source list.
pub fn context_text(st: &DashState, sources: &[SourceBrief], range_now_ms: i64) -> String {
    let d = &st.dashboard;
    let mut o = String::new();
    o.push_str(&format!("DASHBOARD \"{}\"{} · time {} to {}{} · {} panel(s)\n", clip(&d.title, 60), if st.dirty() { " (unsaved)" } else { "" }, d.time.from, d.time.to, if d.refresh_s > 0 { format!(" · refresh {}s", d.refresh_s) } else { String::new() }, d.panels.len()));
    if !d.variables.is_empty() {
        o.push_str(&format!("variables: {}\n", d.variables.iter().map(|v| format!("${}={}", v.name, if v.current.is_empty() { "(first)".to_string() } else { v.current.join("|") })).collect::<Vec<_>>().join(", ")));
    }
    o.push_str("sources (names and kinds only): ");
    if sources.is_empty() {
        o.push_str("none yet (the person adds one in the connector manager)\n");
    } else {
        o.push_str(&sources.iter().map(|s| format!("{} [{}, {}]", clip(&s.name, 30), s.kind, s.language)).collect::<Vec<_>>().join("; "));
        o.push('\n');
    }
    for p in &d.panels {
        let sel = if st.selected.as_deref() == Some(p.id.as_str()) { " <selected>" } else { "" };
        let src = sources.iter().find(|s| s.id == p.source).map_or(p.source.as_str(), |s| s.name.as_str());
        let extra = if p.queries.is_empty() && p.expressions.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = p.queries.iter().map(|q| sources.iter().find(|s| s.id == q.source).map_or(q.source.clone(), |s| s.name.clone())).collect();
            format!(" [+{} quer{} on {}, {} expression(s)]", p.queries.len(), if p.queries.len() == 1 { "y" } else { "ies" }, clip(&names.join("/"), 30), p.expressions.len())
        };
        o.push_str(&format!("- {} \"{}\" {} on {}{}: {}{}\n", p.id, clip(&p.title, 40), p.visual.label(), clip(src, 24), sel, clip(&p.query.text.replace('\n', " "), 110), extra));
        if let Some(r) = st.results.get(&p.id) {
            match &r.error {
                Some(e) => o.push_str(&format!("    error: {}\n", clip(e, 140))),
                None if st.selected.as_deref() == Some(p.id.as_str()) && !r.summary.is_empty() => o.push_str(&format!("    last result: {}\n", clip(&r.summary, 300))),
                None => {}
            }
        }
        if o.chars().count() > CONTEXT_CHARS - 500 {
            o.push_str("- (more panels not listed)\n");
            break;
        }
    }
    if let Some((a, b)) = st.focus_window {
        o.push_str(&format!("the person is looking at {} to {} (UTC)\n", super::frame::format_time(a), super::frame::format_time(b)));
    }
    if !st.errors.is_empty() {
        o.push_str(&format!("recent errors: {}\n", st.errors.iter().map(|e| clip(e, 100)).collect::<Vec<_>>().join(" | ")));
    }
    let _ = range_now_ms;
    if o.chars().count() > CONTEXT_CHARS {
        o = o.chars().take(CONTEXT_CHARS - 20).collect::<String>() + "\n(cut)\n";
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> DashState {
        let mut d = Dashboard::new("d", "Checkout");
        d.add_panel(Panel::new("", "CPU", "s1", Visual::Line, "rate(cpu[5m])", Grid::new(0, 0, 12, 6))).unwrap();
        DashState::new(d)
    }

    #[test]
    fn edits_undo_and_redo() {
        let mut s = st();
        let r0 = s.rev;
        s.edit(|d| d.add_panel(Panel::new("", "Mem", "s1", Visual::Stat, "mem", Grid::new(0, 0, 6, 4)))).unwrap();
        assert_eq!(s.dashboard.panels.len(), 2);
        assert!(s.rev > r0 && s.dirty() && s.can_undo());
        assert!(s.edit(|d| -> Result<(), String> {
            d.panels.clear();
            Err("nope".into())
        })
        .is_err());
        assert_eq!(s.dashboard.panels.len(), 2, "a failed edit changes nothing");
        assert!(s.undo());
        assert_eq!(s.dashboard.panels.len(), 1);
        assert!(s.redo());
        assert_eq!(s.dashboard.panels.len(), 2);
        assert!(!s.redo());
        s.edit(|d| Ok(d.remove_panel("p1"))).unwrap();
        s.selected = Some("p1".into());
        assert!(s.undo());
        assert!(s.undo());
        assert!(!s.undo(), "nothing left");
        assert_eq!(s.dashboard.panels.len(), 1);
        s.mark_saved();
        assert!(!s.dirty());
    }

    #[test]
    fn a_no_op_edit_adds_no_step_and_the_stack_is_bounded() {
        let mut s = st();
        s.edit(|_| Ok(())).unwrap();
        assert!(!s.can_undo());
        for i in 0..(UNDO_DEPTH + 10) {
            s.edit(|d| Ok(d.title = format!("t{i}"))).unwrap();
        }
        let mut n = 0;
        while s.undo() {
            n += 1;
        }
        assert_eq!(n, UNDO_DEPTH);
    }

    #[test]
    fn the_context_is_compact_and_has_no_addresses_or_secrets() {
        let mut s = st();
        s.selected = Some("p1".into());
        s.results.insert("p1".into(), PanelResult { summary: "2 series; a: last 1, min 0, max 2, avg 1".into(), error: None, at_ms: 0 });
        s.note_error("p2", "Disk", "ClickHouse: Table x does not exist", 0);
        let src = vec![SourceBrief { id: "s1".into(), name: "Prod Prometheus".into(), kind: "prometheus".into(), language: "PromQL".into() }];
        let t = context_text(&s, &src, 0);
        assert!(t.contains("DASHBOARD \"Checkout\"") && t.contains("Prod Prometheus [prometheus, PromQL]") && t.contains("<selected>") && t.contains("last result: 2 series") && t.contains("recent errors"), "{t}");
        assert!(t.chars().count() <= CONTEXT_CHARS);
        // many panels with long queries still fit
        let mut big = st();
        for i in 0..50 {
            let _ = big.dashboard.add_panel(Panel::new("", &format!("Panel number {i} with a long title indeed"), "s1", Visual::Line, &"sum(rate(http_requests_total[5m])) by (service) ".repeat(10), Grid::new(0, 0, 6, 3)));
        }
        let t = context_text(&big, &src, 0);
        assert!(t.chars().count() <= CONTEXT_CHARS && t.contains("more panels not listed"), "{}", t.len());
    }
}
