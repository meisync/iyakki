//! Dependencies between rows of a features file: the optional `Depends` column holds row ids (`F-APP-01, F-DB-02`). From it this module
//! computes what the Board shows and what the schedulers obey:
//!
//! * [`parse_ids`]: the ids in a Depends cell (free words such as "the registry" are not ids and are ignored here);
//! * [`validate`]: unknown ids, a row waiting for itself and cycles are refused with the cycle path;
//! * [`Graph`]: per row its [`DepState`] (Ready, Waiting on ids, Blocked, Done), the rows it unblocks, the layered layout of the
//!   dependency graph and the critical path (the longest chain of unfinished rows);
//! * [`ready_rows`]: the rows an agent may start now. Nothing starts a row whose dependencies are not done unless the person says so.
//!
//! Older files that use the cell for words ("the registry") keep working: a cell without a single id never blocks anything.

use std::collections::{BTreeMap, BTreeSet};

use crate::featureboard::{Board, Feature};

/// Does this word look like a row id (`F-APP-01`, `B-DB-3`, `P-2`): letters, then dash-separated parts, the last one ending in a digit?
pub fn looks_like_id(w: &str) -> bool {
    let w = w.trim();
    if w.len() < 3 || w.len() > 40 || !w.contains('-') {
        return false;
    }
    let first = w.chars().next().unwrap_or(' ');
    first.is_ascii_alphabetic() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && w.chars().last().is_some_and(|c| c.is_ascii_digit()) && !w.ends_with('-')
}

/// The ids named in a Depends cell, in order, once each. Separators: comma, semicolon, space, `+`, `/`, `&`; backticks and brackets are dropped.
pub fn parse_ids(cell: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in cell.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '+' | '/' | '&' | '(' | ')' | '[' | ']' | '`')) {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if looks_like_id(w) && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// Writes ids as a cell: `F-A-1, F-B-2`.
pub fn join_ids(ids: &[String]) -> String {
    ids.join(", ")
}

/// What a row can do right now, from its own status and its dependencies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepState {
    /// Finished (done or deprecated).
    Done,
    /// Open and every dependency is done: an agent may take it.
    Ready,
    /// Open, and these dependencies are not done yet (they are on their way).
    Waiting(Vec<String>),
    /// Cannot proceed: the row itself is blocked, or these dependencies are blocked (or part of a cycle).
    Blocked(Vec<String>),
}

impl DepState {
    pub fn label(&self) -> String {
        match self {
            DepState::Done => "done".into(),
            DepState::Ready => "ready".into(),
            DepState::Waiting(v) => format!("waiting on {}", v.join(", ")),
            DepState::Blocked(v) if v.is_empty() => "blocked".into(),
            DepState::Blocked(v) => format!("blocked by {}", v.join(", ")),
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, DepState::Ready)
    }
}

fn finished(status: &str) -> bool {
    matches!(status, "done" | "deprecated")
}

/// The rows of one board and what they wait for.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// In board order: (id, title, status).
    pub nodes: Vec<(String, String, String)>,
    /// Known dependencies per row (ids that exist), in cell order.
    pub deps: BTreeMap<String, Vec<String>>,
    /// Ids named in a Depends cell that are not rows of the board.
    pub unknown: BTreeMap<String, Vec<String>>,
    /// Rows that wait for this one.
    pub dependants: BTreeMap<String, Vec<String>>,
    states: BTreeMap<String, DepState>,
    cycle: Option<Vec<String>>,
}

impl Graph {
    /// Bugs (`B-` rows) are part of the graph only when something names them.
    pub fn build(features: &[Feature]) -> Graph {
        let mut g = Graph::default();
        let known: BTreeSet<&str> = features.iter().map(|f| f.id.as_str()).collect();
        for f in features {
            g.nodes.push((f.id.clone(), f.title.clone(), f.status.clone()));
            let mut ok = Vec::new();
            let mut bad = Vec::new();
            for d in parse_ids(&f.depends()) {
                if d == f.id {
                    continue;
                }
                if known.contains(d.as_str()) {
                    ok.push(d);
                } else {
                    bad.push(d);
                }
            }
            for d in &ok {
                g.dependants.entry(d.clone()).or_default().push(f.id.clone());
            }
            if !ok.is_empty() {
                g.deps.insert(f.id.clone(), ok);
            }
            if !bad.is_empty() {
                g.unknown.insert(f.id.clone(), bad);
            }
        }
        g.cycle = find_cycle(&g.deps);
        let in_cycle: BTreeSet<String> = g.cycle.clone().unwrap_or_default().into_iter().collect();
        let status: BTreeMap<&str, &str> = g.nodes.iter().map(|n| (n.0.as_str(), n.2.as_str())).collect();
        let mut states: BTreeMap<String, DepState> = BTreeMap::new();
        // blocked-ness spreads along dependencies: iterate to a fixed point (the graph is small)
        let mut blocked: BTreeSet<String> = g.nodes.iter().filter(|n| n.2 == "blocked" || in_cycle.contains(&n.0)).map(|n| n.0.clone()).collect();
        loop {
            let mut grew = false;
            for (id, ds) in &g.deps {
                if blocked.contains(id) || finished(status.get(id.as_str()).copied().unwrap_or("")) {
                    continue;
                }
                if ds.iter().any(|d| blocked.contains(d) && !finished(status.get(d.as_str()).copied().unwrap_or(""))) {
                    blocked.insert(id.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        for (id, _, st) in &g.nodes {
            let unmet: Vec<String> = g.deps.get(id).map(|ds| ds.iter().filter(|d| !finished(status.get(d.as_str()).copied().unwrap_or(""))).cloned().collect()).unwrap_or_default();
            let s = if finished(st) {
                DepState::Done
            } else if blocked.contains(id) {
                let by: Vec<String> = unmet.iter().filter(|d| blocked.contains(*d)).cloned().collect();
                DepState::Blocked(if in_cycle.contains(id) { g.cycle.clone().unwrap_or_default().into_iter().filter(|x| x != id).collect() } else { by })
            } else if unmet.is_empty() {
                DepState::Ready
            } else {
                DepState::Waiting(unmet)
            };
            states.insert(id.clone(), s);
        }
        g.states = states;
        g
    }

    pub fn from_board(b: &Board) -> Graph {
        Graph::build(&b.features())
    }

    pub fn state(&self, id: &str) -> Option<&DepState> {
        self.states.get(id)
    }

    /// Rows that wait for `id` and are not finished: "unblocks N".
    pub fn unblocks(&self, id: &str) -> usize {
        self.dependants.get(id).map_or(0, |v| v.iter().filter(|d| self.states.get(*d).is_some_and(|s| *s != DepState::Done)).count())
    }

    /// The rows with a dependency or a dependant (the nodes of the drawn graph).
    pub fn connected(&self) -> Vec<String> {
        self.nodes.iter().filter(|n| self.deps.contains_key(&n.0) || self.dependants.contains_key(&n.0)).map(|n| n.0.clone()).collect()
    }

    /// A cycle in the file (hand edits can make one): the ids around it, first id repeated at the end.
    pub fn cycle(&self) -> Option<&[String]> {
        self.cycle.as_deref()
    }

    /// Layers, left to right: a row sits one layer after its deepest dependency. Rows of a layer are ordered to keep edges short
    /// (a few barycenter sweeps). Only connected rows are laid out.
    pub fn layers(&self) -> Vec<Vec<String>> {
        let nodes = self.connected();
        let set: BTreeSet<&str> = nodes.iter().map(String::as_str).collect();
        let mut depth: BTreeMap<String, usize> = BTreeMap::new();
        fn d(id: &str, g: &Graph, memo: &mut BTreeMap<String, usize>, stack: &mut Vec<String>) -> usize {
            if let Some(v) = memo.get(id) {
                return *v;
            }
            if stack.iter().any(|s| s == id) {
                return 0;
            }
            stack.push(id.to_string());
            let v = g.deps.get(id).map_or(0, |ds| ds.iter().map(|x| d(x, g, memo, stack) + 1).max().unwrap_or(0));
            stack.pop();
            memo.insert(id.to_string(), v);
            v
        }
        for n in &nodes {
            let v = d(n, self, &mut depth, &mut Vec::new());
            depth.insert(n.clone(), v);
        }
        let top = depth.values().copied().max().unwrap_or(0);
        let mut layers: Vec<Vec<String>> = vec![Vec::new(); if nodes.is_empty() { 0 } else { top + 1 }];
        for n in &nodes {
            layers[depth[n]].push(n.clone());
        }
        let pos = |layers: &Vec<Vec<String>>, id: &str| -> f64 { layers.iter().find_map(|l| l.iter().position(|x| x == id)).unwrap_or(0) as f64 };
        for pass in 0..4 {
            for li in 0..layers.len() {
                if li == 0 && pass % 2 == 0 {
                    continue;
                }
                let snapshot = layers.clone();
                let mut keyed: Vec<(f64, String)> = layers[li]
                    .iter()
                    .map(|id| {
                        let mut nb: Vec<f64> = Vec::new();
                        if pass % 2 == 0 {
                            if let Some(ds) = self.deps.get(id) {
                                nb.extend(ds.iter().filter(|x| set.contains(x.as_str())).map(|x| pos(&snapshot, x)));
                            }
                        } else if let Some(ds) = self.dependants.get(id) {
                            nb.extend(ds.iter().filter(|x| set.contains(x.as_str())).map(|x| pos(&snapshot, x)));
                        }
                        let key = if nb.is_empty() { pos(&snapshot, id) } else { nb.iter().sum::<f64>() / nb.len() as f64 };
                        (key, id.clone())
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
                layers[li] = keyed.into_iter().map(|k| k.1).collect();
            }
        }
        layers
    }

    /// The longest chain of unfinished rows, first to last (the work that decides when everything can be done). Empty when no unfinished row has a dependency.
    pub fn critical_path(&self) -> Vec<String> {
        let open: BTreeSet<&str> = self.nodes.iter().filter(|n| !finished(&n.2)).map(|n| n.0.as_str()).collect();
        let mut best: BTreeMap<String, (usize, Option<String>)> = BTreeMap::new();
        fn len(id: &str, g: &Graph, open: &BTreeSet<&str>, memo: &mut BTreeMap<String, (usize, Option<String>)>, stack: &mut Vec<String>) -> usize {
            if let Some(v) = memo.get(id) {
                return v.0;
            }
            if stack.iter().any(|s| s == id) {
                return 1;
            }
            stack.push(id.to_string());
            let mut top: (usize, Option<String>) = (1, None);
            if let Some(ds) = g.deps.get(id) {
                for dd in ds.iter().filter(|x| open.contains(x.as_str())) {
                    let l = len(dd, g, open, memo, stack) + 1;
                    if l > top.0 {
                        top = (l, Some(dd.clone()));
                    }
                }
            }
            stack.pop();
            memo.insert(id.to_string(), top.clone());
            top.0
        }
        let mut end: Option<(usize, String)> = None;
        for id in open.iter() {
            let l = len(id, self, &open, &mut best, &mut Vec::new());
            if end.as_ref().is_none_or(|e| l > e.0) {
                end = Some((l, id.to_string()));
            }
        }
        let Some((l, mut cur)) = end else { return vec![] };
        if l < 2 {
            return vec![];
        }
        let mut path = vec![cur.clone()];
        let mut guard = 0;
        while let Some((_, Some(prev))) = best.get(&cur).cloned() {
            path.push(prev.clone());
            cur = prev;
            guard += 1;
            if guard > self.nodes.len() {
                break;
            }
        }
        path.reverse();
        path
    }
}

/// A cycle in `id -> dependencies`: the ids around it with the first repeated at the end (`A -> B -> A`).
pub fn find_cycle(deps: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    fn visit(id: &str, deps: &BTreeMap<String, Vec<String>>, state: &mut BTreeMap<String, u8>, path: &mut Vec<String>) -> Option<Vec<String>> {
        match state.get(id) {
            Some(2) => return None,
            Some(1) => {
                let at = path.iter().position(|p| p == id).unwrap_or(0);
                let mut c: Vec<String> = path[at..].to_vec();
                c.push(id.to_string());
                return Some(c);
            }
            _ => {}
        }
        state.insert(id.to_string(), 1);
        path.push(id.to_string());
        if let Some(ds) = deps.get(id) {
            for d in ds {
                if let Some(c) = visit(d, deps, state, path) {
                    return Some(c);
                }
            }
        }
        path.pop();
        state.insert(id.to_string(), 2);
        None
    }
    let mut state = BTreeMap::new();
    for id in deps.keys() {
        if let Some(c) = visit(id, deps, &mut state, &mut Vec::new()) {
            return Some(c);
        }
    }
    None
}

/// May `id` be given the Depends text `cell`? Ok with the ids it names (empty for a free-text cell). Err names the problem: an id that is
/// not a row, the row itself, or the cycle it would close (`F-A-1 -> F-B-1 -> F-A-1`).
pub fn validate(features: &[Feature], id: &str, cell: &str) -> Result<Vec<String>, String> {
    let ids = parse_ids(cell);
    let known: BTreeSet<&str> = features.iter().map(|f| f.id.as_str()).collect();
    for d in &ids {
        if d == id {
            return Err(format!("{id} cannot depend on itself"));
        }
        if !known.contains(d.as_str()) {
            return Err(format!("{d} is not a row of this board (Depends takes row ids)"));
        }
    }
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in features {
        if f.id != id {
            let v = parse_ids(&f.depends());
            if !v.is_empty() {
                deps.insert(f.id.clone(), v);
            }
        }
    }
    if !ids.is_empty() {
        deps.insert(id.to_string(), ids.clone());
    }
    // a cycle that goes through this row (cycles elsewhere in a hand-edited file are not this call's business)
    if let Some(c) = find_cycle(&deps) {
        if c.iter().any(|x| x == id) {
            let at = c.iter().position(|x| x == id).unwrap_or(0);
            let mut around: Vec<String> = c[at..c.len() - 1].to_vec();
            around.extend(c[..at].iter().cloned());
            around.push(id.to_string());
            return Err(format!("that would make a cycle: {}", around.join(" -> ")));
        }
    }
    Ok(ids)
}

/// Open rows (not bugs) an agent may start: every dependency done. In board order, the partial ones first.
pub fn ready_rows(features: &[Feature]) -> Vec<String> {
    let g = Graph::build(features);
    let mut v: Vec<&Feature> = features.iter().filter(|f| !f.is_bug() && matches!(f.status.as_str(), "planned" | "partial") && g.state(&f.id).is_some_and(DepState::is_ready)).collect();
    v.sort_by_key(|f| f.status != "partial");
    v.into_iter().map(|f| f.id.clone()).collect()
}

/// Why `id` may not be started yet, when it may not (None: it can start, or it has no dependencies).
pub fn not_ready_reason(features: &[Feature], id: &str) -> Option<String> {
    let g = Graph::build(features);
    match g.state(id)? {
        DepState::Waiting(v) => Some(format!("{id} waits for {}; they are not done", v.join(", "))),
        DepState::Blocked(v) if !v.is_empty() => Some(format!("{id} is blocked by {}", v.join(", "))),
        _ => None,
    }
}

/// One line for a row of the PROJECT STATE or a list: ` · ready`, ` · waits: F-A-1`.
pub fn state_suffix(g: &Graph, id: &str) -> String {
    match g.state(id) {
        Some(DepState::Waiting(v)) => format!(" · waits: {}", v.join(", ")),
        Some(DepState::Blocked(v)) if !v.is_empty() => format!(" · blocked by: {}", v.join(", ")),
        Some(DepState::Ready) if g.deps.contains_key(id) => " · ready (dependencies done)".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(rows: &[(&str, &str, &str)]) -> Vec<Feature> {
        let mut t = String::from("| ID | Feature | Status | Depends |\n|---|---|---|---|\n");
        for (id, st, dep) in rows {
            t.push_str(&format!("| {id} | title {id} | {st} | {dep} |\n"));
        }
        Board::parse(&t).features()
    }

    #[test]
    fn ids_are_read_from_a_cell_and_words_are_not_ids() {
        assert_eq!(parse_ids("F-APP-01, F-DB-02"), ["F-APP-01", "F-DB-02"]);
        assert_eq!(parse_ids("F-APP-01 F-DB-02;F-APP-01 `B-X-3`"), ["F-APP-01", "F-DB-02", "B-X-3"]);
        assert!(parse_ids("the registry").is_empty());
        assert!(parse_ids("task-runner and docker-compose").is_empty());
        assert!(parse_ids("").is_empty());
    }

    #[test]
    fn states_ready_waiting_blocked_done() {
        let f = board(&[("F-A-1", "done", ""), ("F-A-2", "planned", "F-A-1"), ("F-A-3", "planned", "F-A-2"), ("F-A-4", "blocked", ""), ("F-A-5", "planned", "F-A-4"), ("F-A-6", "planned", "F-A-5, F-A-1"), ("F-A-7", "planned", "the registry")]);
        let g = Graph::build(&f);
        assert_eq!(g.state("F-A-1"), Some(&DepState::Done));
        assert_eq!(g.state("F-A-2"), Some(&DepState::Ready));
        assert_eq!(g.state("F-A-3"), Some(&DepState::Waiting(vec!["F-A-2".into()])));
        assert_eq!(g.state("F-A-4"), Some(&DepState::Blocked(vec![])));
        assert_eq!(g.state("F-A-5"), Some(&DepState::Blocked(vec!["F-A-4".into()])));
        assert_eq!(g.state("F-A-6"), Some(&DepState::Blocked(vec!["F-A-5".into()])), "blocked spreads");
        assert_eq!(g.state("F-A-7"), Some(&DepState::Ready), "words never block");
        assert_eq!(g.unblocks("F-A-2"), 1);
        assert_eq!(g.unblocks("F-A-1"), 2);
        assert_eq!(ready_rows(&f), ["F-A-2", "F-A-7"]);
        assert_eq!(not_ready_reason(&f, "F-A-3").unwrap(), "F-A-3 waits for F-A-2; they are not done");
        assert!(not_ready_reason(&f, "F-A-2").is_none());
    }

    #[test]
    fn partial_rows_come_first_and_deprecated_deps_count_as_done() {
        let f = board(&[("F-A-1", "deprecated", ""), ("F-A-2", "planned", "F-A-1"), ("F-A-3", "partial", "")]);
        assert_eq!(ready_rows(&f), ["F-A-3", "F-A-2"]);
    }

    #[test]
    fn validation_refuses_unknown_self_and_cycles_with_the_path() {
        let f = board(&[("F-A-1", "planned", ""), ("F-A-2", "planned", "F-A-1"), ("F-A-3", "planned", "F-A-2")]);
        assert_eq!(validate(&f, "F-A-3", "F-A-1, F-A-2").unwrap(), ["F-A-1", "F-A-2"]);
        assert!(validate(&f, "F-A-3", "the registry").unwrap().is_empty());
        assert!(validate(&f, "F-A-3", "F-Z-9").unwrap_err().contains("F-Z-9 is not a row"));
        assert!(validate(&f, "F-A-3", "F-A-3").unwrap_err().contains("itself"));
        let e = validate(&f, "F-A-1", "F-A-3").unwrap_err();
        assert!(e.contains("cycle") && e.contains("F-A-1 -> F-A-3 -> F-A-2 -> F-A-1"), "{e}");
        assert!(validate(&f, "F-A-1", "").unwrap().is_empty());
    }

    #[test]
    fn a_hand_made_cycle_blocks_its_rows_and_is_reported() {
        let f = board(&[("F-A-1", "planned", "F-A-2"), ("F-A-2", "planned", "F-A-1"), ("F-A-3", "planned", "")]);
        let g = Graph::build(&f);
        let c = g.cycle().unwrap();
        assert_eq!(c.first(), c.last());
        assert!(matches!(g.state("F-A-1"), Some(DepState::Blocked(_))));
        assert_eq!(ready_rows(&f), ["F-A-3"]);
    }

    #[test]
    fn layers_follow_depth_and_the_critical_path_is_the_longest_open_chain() {
        let f = board(&[("F-A-1", "done", ""), ("F-A-2", "planned", "F-A-1"), ("F-A-3", "planned", "F-A-2"), ("F-A-4", "planned", "F-A-3"), ("F-A-5", "planned", "F-A-1"), ("F-A-9", "planned", "")]);
        let g = Graph::build(&f);
        let l = g.layers();
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], ["F-A-1"]);
        assert_eq!(l[3], ["F-A-4"]);
        assert!(l[1].contains(&"F-A-2".to_string()) && l[1].contains(&"F-A-5".to_string()));
        assert!(!l.iter().flatten().any(|x| x == "F-A-9"), "an unconnected row is not drawn");
        assert_eq!(g.critical_path(), ["F-A-2", "F-A-3", "F-A-4"], "done rows are not part of what is left");
        assert!(Graph::build(&board(&[("F-A-1", "planned", "")])).critical_path().is_empty());
    }
}
