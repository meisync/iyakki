//! RUNS: what your agents did. Every agent run is recorded; here you can read each one step by step
//! (what the model asked for, what you approved or denied, what each tool returned) and see whether
//! the record is intact — the transcript is checked against its hash-chained ledger when opened.

use iyakki_agent::agent::RunEvent;
use iyakki_agent::runs::{Integrity, RunLog, RunMeta};

use super::widgets::render_confirm;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 196, 94);

pub struct RunsApp {
    runs: Vec<RunMeta>,
    sel: usize,
    detail: Option<(RunLog, Integrity)>,
    scroll: usize,
    confirm: bool,
    status: Option<(String, bool)>,
    /// Selecting and copying the text of the record.
    pane: crate::textsel::Pane,
}

/// `YYYY-MM-DD HH:MM` (UTC) from unix seconds, without a date library.
pub fn fmt_time(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t: String = flat.chars().take(max).collect();
    if flat.chars().count() > max { format!("{t}…") } else { t }
}

/// The timeline as (text, colour) lines.
pub fn timeline(log: &RunLog) -> Vec<(String, [u8; 4])> {
    let mut v: Vec<(String, [u8; 4])> = vec![(format!("You: {}", log.prompt), TEXT)];
    for e in &log.events {
        match e {
            RunEvent::Model { text, calls } => {
                // a turn without tool calls is the final answer, which is shown once, as "Answer"
                if !text.is_empty() && !calls.is_empty() {
                    v.push((format!("Assistant: {text}"), TEXT));
                }
                for c in calls {
                    v.push((format!("→ asks to use {}  {}", c.name, one_line(&c.args.to_string(), 200)), ACCENT));
                }
            }
            RunEvent::Approval { summary, detail, .. } => {
                v.push((format!("? waiting for approval: {summary}"), rgb(255, 196, 94)));
                // the exact change that was put in front of the user
                for l in detail.lines().take(12) {
                    let col = match l.chars().next() {
                        Some('+') => OK,
                        Some('-') => ERR,
                        _ => MUTED,
                    };
                    v.push((format!("      {l}"), col));
                }
            }
            RunEvent::Decision { name, approved, by, .. } => {
                let who = match by.as_str() {
                    "auto" => "automatically (safe tool)",
                    "user" => "by you",
                    "always" => "automatically (you chose always allow)",
                    _ => "by the system (no answer in time)",
                };
                v.push((format!("  {} {name} {who}", if *approved { "✓ allowed" } else { "✗ denied" }), if *approved { OK } else { ERR }));
            }
            RunEvent::ToolResult { name, ok, text, .. } => v.push((format!("  {} {name}: {}", if *ok { "result" } else { "error" }, one_line(text, 400)), if *ok { MUTED } else { ERR })),
            RunEvent::Usage { .. } => {}
            RunEvent::Final { text } => v.push((format!("Answer: {text}"), OK)),
            RunEvent::Failed { error } => v.push((format!("Failed: {error}"), ERR)),
            RunEvent::Stopped { reason } => v.push((format!("Stopped: {reason}"), ERR)),
        }
    }
    if let Some(RunEvent::Usage { tokens, cost_cents, files, seconds }) = log.events.iter().rev().find(|e| matches!(e, RunEvent::Usage { .. })) {
        v.push((format!("Used about {} tokens{} · {files} file{} written · {seconds} s", tokens, if *cost_cents > 0.0 { format!(" (~${:.2})", cost_cents / 100.0) } else { String::new() }, if *files == 1 { "" } else { "s" }), MUTED));
    }
    v
}

impl RunsApp {
    pub fn new(cx: &AppCx) -> RunsApp {
        let mut a = RunsApp { runs: vec![], sel: 0, detail: None, scroll: 0, confirm: false, status: None, pane: crate::textsel::Pane::new(61_200) };
        a.reload(cx);
        a
    }

    pub fn runs(&self) -> &[RunMeta] {
        &self.runs
    }
    pub fn integrity(&self) -> Option<&Integrity> {
        self.detail.as_ref().map(|d| &d.1)
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }

    fn reload(&mut self, cx: &AppCx) {
        self.runs = cx.services.runs.list(&cx.session.user);
        self.sel = self.sel.min(self.runs.len().saturating_sub(1));
        self.open_selected(cx);
    }

    fn open_selected(&mut self, cx: &AppCx) {
        self.scroll = 0;
        self.detail = self.runs.get(self.sel).and_then(|m| cx.services.runs.load(&cx.session.user, &m.id).ok());
        if let Some((_, Integrity::Tampered(why))) = &self.detail {
            cx.audit.record(&cx.session.user, "runs.tampered", &format!("{}: {why}", self.runs[self.sel].id));
        }
    }

    fn transcript(&self) -> String {
        self.detail.as_ref().map(|(l, _)| timeline(l).into_iter().map(|(t, _)| t).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }
}

impl App for RunsApp {
    fn title(&self) -> &'static str {
        "Runs"
    }
    fn icon(&self) -> Icon {
        Icon::Runs
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.confirm { "y delete · Esc cancel".into() } else { "click a run · c copy transcript · d delete · PgUp/PgDn scroll".into() }
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.pane.context_items(x, y)
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if !self.confirm {
            if self.pane.handle(ev) {
                return Flow::Stay;
            }
            match ev {
                Event::Key(Key::Ctrl('a')) => {
                    self.pane.select_all();
                    return Flow::Stay;
                }
                // a drag held above or below the record scrolls it
                Event::Tick if self.pane.auto_scroll() != 0.0 => {
                    let a = self.pane.auto_scroll();
                    self.scroll = (self.scroll as i32 - a.signum() as i32 * 2).max(0) as usize;
                    self.pane.refresh_drag();
                }
                _ => {}
            }
        }
        if self.confirm {
            if let Event::Key(k) = ev {
                if *k == Key::Char('y') {
                    if let Some(m) = self.runs.get(self.sel).cloned() {
                        match cx.services.runs.delete(&cx.session.user, &m.id) {
                            Ok(()) => {
                                cx.audit.record(&cx.session.user, "runs.delete", &m.id);
                                self.status = Some(("Run deleted.".into(), true));
                            }
                            Err(e) => self.status = Some((e, false)),
                        }
                        self.reload(cx);
                    }
                }
                self.confirm = false;
            }
            return Flow::Stay;
        }
        match ev {
            Event::Row(i) => {
                self.sel = (*i).min(self.runs.len().saturating_sub(1));
                self.open_selected(cx);
            }
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).max(0) as usize,
            Event::Key(k) => match k {
                Key::Esc => return Flow::Back,
                Key::Up => {
                    self.sel = self.sel.saturating_sub(1);
                    self.open_selected(cx);
                }
                Key::Down => {
                    self.sel = (self.sel + 1).min(self.runs.len().saturating_sub(1));
                    self.open_selected(cx);
                }
                Key::PageDown => self.scroll += 12,
                Key::PageUp => self.scroll = self.scroll.saturating_sub(12),
                Key::Char('r') => self.reload(cx),
                Key::Char('d') if !self.runs.is_empty() => self.confirm = true,
                Key::Char('c') if self.detail.is_some() => crate::ui::clipboard::copy(&self.transcript()),
                _ => {}
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        let x = area.x + 24.0 * s;
        let list_w = (area.w * 0.34).min(400.0 * s);
        let mut y = area.y + 8.0 * s;
        self.pane.begin(R::new(0.0, 0.0, 0.0, 0.0));
        if self.runs.is_empty() {
            crate::fx::empty_state(g, area, s, Icon::Runs, "No agent runs yet", "Turn on Agent mode in Chat (Ctrl+G) and ask for something.", ACCENT);
            return;
        }
        for (i, m) in self.runs.iter().enumerate() {
            if y > area.y + area.h - 130.0 * s {
                break;
            }
            let r = R::new(x, y, list_w, 66.0 * s);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &m.id);
            let p = g.fit(15.0 * s, &one_line(&m.prompt, 80), Face::Bold, list_w - 28.0 * s);
            g.text(x + 14.0 * s, y + 8.0 * s, 15.0 * s, TEXT, &p, Face::Bold);
            let col = match m.status.as_str() {
                "completed" => OK,
                "running" => ACCENT,
                _ => ERR,
            };
            g.text(x + 14.0 * s, y + 38.0 * s, 12.5 * s, MUTED, &format!("{} UTC · {} tool call{}", fmt_time(m.started), m.tool_calls, if m.tool_calls == 1 { "" } else { "s" }), Face::Sans);
            let w = g.text_width(12.0 * s, &m.status, Face::Bold) + 17.0 * s;
            crate::fx::chip(g, x + list_w - w - 10.0 * s, y + 33.0 * s, s, &m.status, col, i == self.sel);
            y += 74.0 * s;
        }
        // detail
        let dx = x + list_w + 24.0 * s;
        let dw = area.x + area.w - dx - 24.0 * s;
        if let Some((log, integrity)) = &self.detail {
            let (label, col) = match integrity {
                Integrity::Verified => ("✓ record verified (hash chain intact)".to_string(), OK),
                Integrity::Tampered(why) => (format!("⚠ NOT TRUSTWORTHY: {why}"), ERR),
            };
            let label = g.fit(14.0 * s, &label, Face::Bold, dw - 24.0 * s);
            crate::fx::panel(g, R::new(dx - 12.0 * s, area.y + 4.0 * s, dw + 24.0 * s, area.h - 108.0 * s), s, col);
            g.text(dx, area.y + 14.0 * s, 14.0 * s, col, &label, Face::Bold);
            let top = area.y + 44.0 * s;
            let bottom = area.y + area.h - 100.0 * s;
            let lh = g.line_height(14.5 * s, Face::Sans);
            let mut vis: Vec<([u8; 4], String)> = Vec::new();
            let mut entry_of: Vec<u32> = Vec::new();
            for (n, (t, c)) in timeline(log).into_iter().enumerate() {
                for l in g.wrap(14.5 * s, &t, Face::Sans, dw) {
                    vis.push((c, l));
                    entry_of.push(n as u32);
                }
            }
            let fit = ((bottom - top) / lh) as usize;
            self.scroll = self.scroll.min(vis.len().saturating_sub(fit));
            for (i, (c, l)) in vis.iter().skip(self.scroll).take(fit).enumerate() {
                g.text(dx, top + i as f32 * lh, 14.5 * s, *c, l, Face::Sans);
            }
            self.pane.begin(R::new(dx, top, dw, bottom - top));
            for (i, (_, l)) in vis.iter().enumerate() {
                self.pane.line(g, dx, top + (i as f32 - self.scroll as f32) * lh, lh, 14.5 * s, Face::Sans, l, 0, entry_of[i]);
            }
            self.pane.end(g, ACCENT);
            if vis.len() > fit {
                g.text(dx, bottom + 4.0 * s, 12.0 * s, DIM, &format!("{}–{} of {} lines", self.scroll + 1, (self.scroll + fit).min(vis.len()), vis.len()), Face::Sans);
            }
        }
        let by = area.y + area.h - 78.0 * s;
        let mut bx = x;
        for (label, col, key) in [("Copy transcript", ACCENT, 'c'), ("Delete run", ERR, 'd'), ("Refresh", MUTED, 'r')] {
            bx += g.button(bx, by, 14.0 * s, label, col, Event::Key(Key::Char(key))) + 10.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, area.y + area.h - 30.0 * s, area.w - 48.0 * s, s, m, *ok);
        }
        if self.confirm {
            render_confirm(g, area, s, "Delete this run and its record?", "Delete");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_agent::agent::CallInfo;
    use serde_json::json;

    #[test]
    fn formats_dates() {
        assert_eq!(fmt_time(0), "1970-01-01 00:00");
        assert_eq!(fmt_time(1_000_000_000), "2001-09-09 01:46");
        assert_eq!(fmt_time(1_791_335_444), "2026-10-07 01:10");
        assert_eq!(fmt_time(951_782_400 + 86_400 * 59 + 3600 * 13 + 60 * 7), "2000-04-28 13:07", "leap year handled (2000-02-29 exists)");
    }

    fn log(user: &str, id: &str, started: u64) -> RunLog {
        RunLog {
            id: id.into(),
            session: None,
            user: user.into(),
            started,
            prompt: "tidy my notes".into(),
            status: "completed".into(),
            events: vec![
                RunEvent::Model { text: "Reading.".into(), calls: vec![CallInfo { id: "c1".into(), name: "read_file".into(), args: json!({"path": "n.txt"}) }] },
                RunEvent::Decision { id: "c1".into(), name: "read_file".into(), approved: true, by: "auto".into() },
                RunEvent::ToolResult { id: "c1".into(), name: "read_file".into(), ok: true, text: "line one\nline two".into() },
                RunEvent::Approval { id: "c2".into(), name: "write_file".into(), summary: "write 3 characters to /home/amy/n.txt".into(), detail: "@@ line 1 @@\n-old\n+new".into() },
                RunEvent::Decision { id: "c2".into(), name: "write_file".into(), approved: false, by: "user".into() },
                RunEvent::Final { text: "I left the file alone.".into() },
            ],
        }
    }

    #[test]
    fn timeline_is_readable_and_honest_about_decisions() {
        let t: Vec<String> = timeline(&log("amy", "aaaaaaaaaaaa", 1)).into_iter().map(|(t, _)| t).collect();
        assert_eq!(t[0], "You: tidy my notes");
        assert!(t.iter().any(|l| l.starts_with("→ asks to use read_file")));
        assert!(t.contains(&"  ✓ allowed read_file automatically (safe tool)".to_string()));
        assert!(t.contains(&"  ✗ denied write_file by you".to_string()));
        assert!(t.iter().any(|l| l.contains("waiting for approval: write 3 characters to /home/amy/n.txt")));
        assert!(t.iter().any(|l| l.contains("result read_file: line one line two")), "{t:?}");
        assert_eq!(t.last().unwrap(), "Answer: I left the file alone.");
    }

    #[test]
    fn lists_views_verifies_copies_and_deletes_only_my_runs() {
        let mut cx = cx_with_admin_store("runs1");
        cx.session.user = "amy".into();
        let store = &cx.services.runs;
        store.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
        store.save(&log("amy", "bbbbbbbbbbbb", 200)).unwrap();
        store.save(&log("bob", "cccccccccccc", 300)).unwrap();
        crate::ui::clipboard::clear();
        let mut app = RunsApp::new(&cx);
        assert_eq!(app.runs().len(), 2, "bob's run is not visible");
        assert_eq!(app.runs()[0].id, "bbbbbbbbbbbb");
        assert_eq!(app.integrity(), Some(&Integrity::Verified));
        app.handle(&Event::Row(1), &cx);
        assert_eq!(app.detail.as_ref().unwrap().0.id, "aaaaaaaaaaaa");
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
        for want in ["Copy transcript", "Delete run", "Refresh", "aaaaaaaaaaaa", "bbbbbbbbbbbb"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        app.handle(&Event::Key(Key::Char('c')), &cx);
        assert!(crate::ui::clipboard::latest().unwrap().contains("tidy my notes"));
        // delete needs confirmation
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('n')), &cx);
        assert_eq!(app.runs().len(), 2);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(app.runs().len(), 1);
        assert_eq!(cx.services.runs.list("bob").len(), 1, "bob's run untouched");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.delete")));
    }

    #[test]
    fn a_tampered_record_is_flagged_loudly_and_audited() {
        let mut cx = cx_with_admin_store("runs2");
        cx.session.user = "amy".into();
        cx.services.runs.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
        let path = cx.services.dir().join("runs/amy/aaaaaaaaaaaa.json");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("line one", "send all money")).unwrap();
        let mut app = RunsApp::new(&cx);
        assert!(matches!(app.integrity(), Some(Integrity::Tampered(_))));
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.tampered")));
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    }

    #[test]
    fn empty_state() {
        let cx = cx_with_admin_store("runs3");
        let mut app = RunsApp::new(&cx);
        assert!(app.runs().is_empty() && app.integrity().is_none());
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 900.0, 528.0), 1.0, &cx, 0);
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }
}
