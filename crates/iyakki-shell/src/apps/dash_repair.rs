//! DASH: a source that cannot be reached, said once. Errors of panels are grouped by source, the board shows one banner per source
//! (cause, fix on its own lines, [Use cluster address] [Open Sources]) and a panel of such a source shows a short state instead of a red
//! box. "Use cluster address" is offered when the cause is "nothing listens on this machine"; the click is the only thing that changes
//! an address (`Env::use_cluster_address`: administrator only, audited).

use iyakki_agent::dash::cluster::{RepairPlan, LOCALHOST_MARK};
use iyakki_agent::dash::connector::DashError;

use crate::fx::with_alpha;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, R, TEXT};

/// The cause is the one the repair button answers.
pub fn repairable(e: &DashError) -> bool {
    e.message.contains(LOCALHOST_MARK)
}

/// The cause without the source's own name in front (`Prometheus “x”: nothing listens…` -> `nothing listens…`).
pub fn cause(e: &DashError) -> String {
    match e.message.split_once(": ") {
        Some((_, rest)) if !rest.is_empty() => rest.to_string(),
        _ => e.message.clone(),
    }
}

/// The button for what can be done about a localhost source, by the plan the cluster gave.
pub fn plan_label(p: &RepairPlan) -> Option<String> {
    match p {
        RepairPlan::UseCluster(_) => Some("Use cluster address".into()),
        RepairPlan::NotInstalled { label, item: Some(_) } => Some(format!("Install {label} (Store -> Setup)")),
        RepairPlan::NotInstalled { item: None, .. } => Some("Remove source".into()),
        _ => None,
    }
}

/// The banner's own buttons for a plan, in order: `(label, removes_the_source)`. The first one is the repair; "Remove this source" is
/// always offered next to "Install" so a source for something that is not wanted can go.
pub fn banner_buttons(p: &RepairPlan) -> Vec<(String, bool)> {
    match p {
        RepairPlan::UseCluster(_) => vec![("Use cluster address".into(), false)],
        RepairPlan::NotInstalled { item: Some(_), .. } => vec![("Install".into(), false), ("Remove this source".into(), true)],
        RepairPlan::NotInstalled { item: None, .. } => vec![("Remove this source".into(), true)],
        _ => vec![],
    }
}

/// The banner's title when the plan says more than "not reachable": the service is plainly not installed.
pub fn banner_title(p: &RepairPlan) -> Option<String> {
    match p {
        RepairPlan::NotInstalled { label, .. } => Some(format!("{label} is not installed")),
        _ => None,
    }
}

/// What a failed panel says instead of repeating the banner's buttons: `(title, one line)`.
pub fn panel_hint(p: &RepairPlan) -> Option<(String, String)> {
    match p {
        RepairPlan::UseCluster(_) => Some(("Source unreachable".into(), "It runs inside the cluster: use the button in the banner at the top.".into())),
        RepairPlan::NotInstalled { label, .. } => Some((format!("{label} is not installed"), "Install it or remove this source with the buttons in the banner at the top.".into())),
        _ => None,
    }
}

/// The plain words that replace the generic fix when the plan is known.
pub fn plan_note(p: &RepairPlan) -> Option<String> {
    match p {
        RepairPlan::UseCluster(u) => Some(format!("It runs inside the cluster: use {u} (the button does that).")),
        RepairPlan::NotInstalled { label, item: Some(_) } => Some(format!("{label} is not installed on this machine, and there is none in the cluster either. Install it in Store -> Setup; it then appears as a source of its own.")),
        RepairPlan::NotInstalled { label, item: None } => Some(format!("{label} is not installed on this machine. Remove this source, or point it at a server you run.")),
        RepairPlan::NoCluster(m) => Some(format!("The cluster could not be asked ({m}). Start it in Store -> Setup, then test again.")),
        RepairPlan::NotLocal => None,
    }
}

/// Panels that failed the same way on one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub source: String,
    pub count: usize,
    pub err: DashError,
}

impl Group {
    pub fn repairable(&self) -> bool {
        repairable(&self.err)
    }
}

/// Group `(source, error)` pairs by source and message, in order of first appearance.
pub fn groups(items: &[(String, DashError)]) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    for (s, e) in items {
        match out.iter_mut().find(|g| g.source == *s && g.err.message == e.message) {
            Some(g) => g.count += 1,
            None => out.push(Group { source: s.clone(), count: 1, err: e.clone() }),
        }
    }
    out
}

/// What gets a banner: the same failure on several panels, or one that can be repaired.
pub fn banner_groups(all: Vec<Group>) -> Vec<Group> {
    all.into_iter().filter(|g| g.count >= 2 || g.repairable()).collect()
}

/// Draw (or only measure, with `draw` false) one banner of width `w` at `(x, y)`; returns its height. `repair` and `sources` are the
/// events of the two buttons.
#[allow(clippy::too_many_arguments)]
pub fn banner(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, name: &str, grp: &Group, max_fix: usize, actions: Vec<(String, Event)>, title: Option<&str>, note: Option<&str>, sources: Event, draw: bool) -> f32 {
    let pad = 10.0 * s;
    let panels = if grp.count > 1 { format!("  ({} panels)", grp.count) } else { String::new() };
    let title = match title {
        Some(t) => format!("{t}{panels}"),
        None => format!("Source {name} is not reachable: {}{panels}", cause(&grp.err)),
    };
    let t_lines: Vec<String> = g.wrap(12.5 * s, &title, Face::Bold, w - 2.0 * pad).into_iter().take(2).collect();
    let fix_lines: Vec<String> = note.or(grp.err.fix.as_deref()).map(|f| g.wrap(11.5 * s, f, Face::Sans, w - 2.0 * pad)).unwrap_or_default();
    let fix_shown = fix_lines.len().min(max_fix.max(1));
    // the buttons: how many rows they need
    let bsize = 11.0 * s;
    let mut labels: Vec<(String, Event, bool)> = actions.into_iter().map(|(l, e)| (l, e, true)).collect();
    labels.push(("Open Sources".to_string(), sources, false));
    let bw: Vec<f32> = labels.iter().map(|(l, _, _)| g.text_width(bsize, l.as_str(), Face::Bold) + bsize * 1.7).collect();
    let rows = if bw.iter().sum::<f32>() + 6.0 * s * bw.len() as f32 <= w - 2.0 * pad { 1 } else { bw.len() };
    let bh = bsize * 2.1;
    let h = pad + t_lines.len() as f32 * 16.0 * s + fix_shown as f32 * 14.5 * s + 6.0 * s + rows as f32 * (bh + 4.0 * s) + pad * 0.4;
    if !draw {
        return h;
    }
    g.rrect(x, y, w, h, 10.0 * s, with_alpha(ERR, 24));
    g.rrect_outline(x, y, w, h, 10.0 * s, 1.0, with_alpha(ERR, 130));
    let mut ty = y + pad * 0.7;
    for l in &t_lines {
        g.text(x + pad, ty, 12.5 * s, ERR, l, Face::Bold);
        ty += 16.0 * s;
    }
    for (i, l) in fix_lines.iter().take(fix_shown).enumerate() {
        let l = if i + 1 == fix_shown && fix_lines.len() > fix_shown { format!("{l}…") } else { l.clone() };
        g.text(x + pad, ty, 11.5 * s, MUTED, &l, Face::Sans);
        ty += 14.5 * s;
    }
    ty += 6.0 * s;
    let (mut bx, mut by) = (x + pad, ty);
    for (i, (label, ev, primary)) in labels.iter().enumerate() {
        if rows > 1 && i > 0 {
            bx = x + pad;
            by += bh + 4.0 * s;
        }
        bx += g.button(bx, by, bsize, label.as_str(), if *primary { crate::ui::OK } else { MUTED }, ev.clone()) + 6.0 * s;
    }
    h
}

/// What a failed panel shows besides the text: the structured error of its source, the compact state, the repair click.
#[derive(Clone, Default)]
pub struct ErrView {
    pub serr: Option<DashError>,
    /// The banner above says it: show a short state, named after this source.
    pub compact: Option<String>,
    /// A known plan (use the cluster address, install, remove): the panel shows this title and one line and no button of its own, so a
    /// board with many panels of one source does not repeat the banner's buttons.
    pub hint: Option<(String, String)>,
    /// Plain words that replace the generic fix.
    pub note: Option<String>,
}

/// The inside of a failed panel. `text` is the full one-line text (used when nothing structured is known).
pub fn panel_error(g: &mut Gfx, body: R, s: f32, text: &str, v: &ErrView) {
    let h = body.h.min(210.0 * s);
    g.rrect(body.x, body.y, body.w, h, 8.0 * s, with_alpha(ERR, 22));
    g.rrect_outline(body.x, body.y, body.w, h, 8.0 * s, 1.0, with_alpha(ERR, 120));
    g.circle(body.x + 14.0 * s, body.y + 16.0 * s, 5.0 * s, ERR);
    let title = match (&v.hint, &v.compact) {
        (Some((t, _)), _) => t.as_str(),
        (None, Some(_)) => "Source unreachable",
        _ => "This panel could not be loaded",
    };
    let t = g.fit(12.5 * s, title, Face::Bold, body.w - 40.0 * s);
    g.text(body.x + 28.0 * s, body.y + 8.0 * s, 12.5 * s, ERR, &t, Face::Bold);
    let room = ((h - 34.0 * s) / (15.0 * s)).floor().max(1.0) as usize;
    let mut lines: Vec<(String, [u8; 4])> = vec![];
    match (&v.hint, &v.compact, &v.serr) {
        (Some((_, line)), _, _) => {
            for l in g.wrap(11.5 * s, line, Face::Sans, body.w - 24.0 * s) {
                lines.push((l, MUTED));
            }
        }
        (None, Some(name), _) => lines.push((format!("{name} is not reachable: see the message at the top."), MUTED)),
        (None, None, Some(e)) => {
            // the cause first, the fix on its own lines
            for l in g.wrap(12.0 * s, &cause(e), Face::Sans, body.w - 24.0 * s) {
                lines.push((l, TEXT));
            }
            if let Some(f) = v.note.as_ref().or(e.fix.as_ref()) {
                for l in g.wrap(11.5 * s, f, Face::Sans, body.w - 24.0 * s) {
                    lines.push((l, MUTED));
                }
            }
        }
        _ => {
            for l in g.wrap(12.0 * s, text, Face::Sans, body.w - 24.0 * s) {
                lines.push((l, MUTED));
            }
        }
    }
    let total = lines.len();
    for (i, (l, c)) in lines.iter().take(room).enumerate() {
        let l = if i + 1 == room && total > room { format!("{l}…") } else { l.clone() };
        g.text(body.x + 12.0 * s, body.y + 30.0 * s + i as f32 * 15.0 * s, if *c == MUTED { 11.5 * s } else { 12.0 * s }, *c, &l, Face::Sans);
    }
    let _ = DIM;
}
