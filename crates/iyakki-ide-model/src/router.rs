//! The ROUTER panel of the AI IDE's bottom area (F-ROUTER-10): the plan of the model router's run in progress, one row per step with its
//! status and the model that has it. Read only: starting, changing and cancelling a run are the chat tools (`router_run`, `router_cancel`) with
//! their approvals. Without a run it says so.

use iyakki_uikit::gfx::{Face, Gfx};
use iyakki_uikit::ui::{DIM, ERR, MUTED, OK, R, TEXT};

use iyakki_uikit::fx::{AMBER, CYAN};

/// One step of the plan as the panel shows it (the agent hands over plain values; see `Services::router_plan_rows`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub after: Vec<String>,
    /// `waiting`, `running`, `done`, `needs a person` or `skipped`.
    pub status: &'static str,
    pub model: Option<String>,
}

/// The colour of a status word.
pub fn status_colour(status: &str) -> [u8; 4] {
    match status {
        "done" => OK,
        "running" => CYAN,
        "needs a person" => AMBER,
        "skipped" => ERR,
        _ => MUTED,
    }
}

pub fn draw(g: &mut Gfx, r: R, s: f32, rows: &[Row]) {
    let size = 14.0 * s;
    let lh = g.line_height(size, Face::Sans) * 1.3;
    let x = r.x + 10.0 * s;
    let mut y = r.y + 6.0 * s;
    if rows.is_empty() {
        g.text(x, y, size, DIM, "No model router run is going on. Ask the assistant to run a plan (router_run); its steps show here while it runs.", Face::Sans);
        return;
    }
    let done = rows.iter().filter(|r| r.status == "done").count();
    g.text(x, y, size, TEXT, &format!("Router plan: {done} of {} steps done", rows.len()), Face::Bold);
    y += lh;
    let idw = rows.iter().map(|r| g.text_width(size, &r.id, Face::Mono)).fold(0.0, f32::max) + 12.0 * s;
    let stw = 120.0 * s;
    for row in rows {
        if y + lh > r.y + r.h {
            let more = rows.len() - rows.iter().position(|q| q.id == row.id).unwrap_or(0);
            g.text(x, y, size, DIM, &format!("... and {more} more"), Face::Sans);
            break;
        }
        g.text(x, y, size, MUTED, &row.id, Face::Mono);
        g.text(x + idw, y, size, status_colour(row.status), row.status, Face::Bold);
        let who = row.model.as_ref().map(|m| format!("on {m}")).unwrap_or_default();
        let after = if row.after.is_empty() { String::new() } else { format!(" (after {})", row.after.join(", ")) };
        let text = g.fit(size, &format!("{} {who}{after}", row.title), Face::Sans, (r.w - idw - stw - 24.0 * s).max(40.0 * s));
        g.text(x + idw + stw, y, size, TEXT, &text, Face::Sans);
        y += lh;
    }
}
