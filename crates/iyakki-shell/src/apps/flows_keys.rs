//! FLOWS, the keyboard, the button clicks, the dialogs, and the `App` impl. One keyboard owner at a time (the canvas, the form, the
//! palette search, the text view, the board search or the assistant); the one that has it is the only one to show a caret.

use iyakki_flow::engine::{ApplyScope, CardDecision};
use iyakki_flow::kinds::KindStatus;
use iyakki_flow::StartRequest;

use super::flows::*;
use super::flows_canvas::*;
use super::flows_props::Props;
use super::widgets::{Form, FormEvent, PickEvent, Picker};
use super::{App, AppCx, DropAction, Flow as AppFlow};
use crate::ctxmenu::Item;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, Key, Nav, Pointer, R};

impl FlowsApp {
    pub(super) fn set_tab(&mut self, t: Tab) {
        self.commit_props();
        self.tab = t;
        self.focus = Focus::Canvas;
        self.drag = None;
        self.refresh_runs();
        if t == Tab::Design {
            self.reload_flows();
        }
        if t == Tab::Board {
            self.board.next = self.next_runs();
        }
        self.redraw = true;
    }

    fn add_picker(&mut self) {
        let kinds: Vec<(String, String)> = self.reg.all().iter().map(|k| (k.id().to_string(), format!("{} ({}): {}{}", k.label(), k.category(), k.blurb(), if matches!(k.status(), KindStatus::Planned(_)) { " [planned]" } else { "" }))).collect();
        let ids = kinds.iter().map(|k| k.0.clone()).collect();
        self.modal = Some(Modal::AddNode(Picker::new("Add a step", kinds.into_iter().map(|k| k.1).collect()), ids));
    }

    fn name_form(&mut self, what: NameFor) {
        let (title, init) = match what {
            NameFor::NewFlow => ("New flow", String::new()),
            NameFor::Rename => ("Rename the flow", self.flow.name.clone()),
        };
        self.modal = Some(Modal::Name(Form::new(title).text("Name", &init, "what the flow does, in a few words"), what));
    }

    fn template_picker(&mut self) {
        let t = iyakki_flow::templates::all();
        let ids = t.iter().map(|t| t.id.clone()).collect();
        let items = t.iter().map(|t| format!("{}: {} (needs: {})", t.name, t.summary, t.needs.join(", "))).collect();
        self.modal = Some(Modal::Templates(Picker::new("Start from a template", items), ids));
    }

    fn import_form(&mut self) {
        self.modal = Some(Modal::Import(Form::new("Import a flow").path("File", "", "a flow file (JSON) in one of your folders, for example Documents/check.flow.json", crate::filepicker::PathKind::File, &["json"])));
    }

    fn zoom_button(&mut self, f: f32) {
        let c = self.geo.canvas;
        let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        if self.tab == Tab::Run {
            self.rv.view.zoom_at(c, cx, cy, f);
        } else {
            self.view.zoom_at(c, cx, cy, f);
        }
    }

    fn fit_button(&mut self) {
        let c = self.geo.canvas;
        if self.tab == Tab::Run {
            if let Some(r) = &self.run {
                self.rv.view = View::fit(&r.flow, c);
            }
        } else {
            self.view = View::fit(&self.flow, c);
        }
    }

    fn step_run(&mut self, newer: bool) {
        let ids: Vec<String> = self.runs.iter().filter(|r| self.cur.as_ref().is_none_or(|c| *c == r.flow_id)).map(|r| r.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let cur = ids.iter().position(|i| Some(i) == self.run_id.as_ref()).unwrap_or(0);
        let next = if newer { cur.saturating_sub(1) } else { (cur + 1).min(ids.len() - 1) };
        let id = ids[next].clone();
        self.select_run(&id, false);
    }

    fn card_action(&mut self, what: &str) {
        let (Some(run), Some(card)) = (self.run_id.clone(), self.open_card()) else { return };
        let who = self.env.who();
        let r = match what {
            "approve" => self.env.engine.card_decide(&run, &card.id, CardDecision::Approve { by: who }),
            "reject" => {
                // dismissing the card of a step that needs a permission withdraws the request (popup and INBOX entry)
                let user = self.env.user.clone();
                self.env.svc.browser.consent.withdraw_permissions(&user, &run);
                self.env.engine.card_decide(&run, &card.id, CardDecision::Reject { by: who, note: String::new() })
            }
            "ask" => self.env.engine.card_decide(&run, &card.id, CardDecision::Investigate { by: who }),
            _ => {
                let Some(p) = &card.patch else { return };
                let mut qb = super::dash_query::QueryBox::new(area::TEXT);
                qb.set_text(&serde_json::to_string_pretty(p).unwrap_or_default());
                self.modal = Some(Modal::EditPatch(format!("Edit the fix for `{}`", card.node), qb));
                return;
            }
        };
        match r {
            Ok(()) => self.say(match what {
                "approve" => "Approved. The fix is applied and the step runs again.".to_string(),
                "reject" => "Rejected.".to_string(),
                _ => "Asked the assistant to look.".to_string(),
            }, true),
            Err(e) => self.say(e, false),
        }
    }

    fn cycle_flow_filter(&mut self) {
        let ids: Vec<String> = self.flows.iter().map(|f| f.id.clone()).collect();
        self.board.flow_filter = match &self.board.flow_filter {
            None => ids.first().cloned(),
            Some(c) => ids.iter().position(|i| i == c).and_then(|p| ids.get(p + 1).cloned()),
        };
    }

    fn bulk(&mut self, cancel: bool) {
        let ids: Vec<String> = self.board.sel.iter().cloned().collect();
        let (mut ok, mut bad) = (0, 0);
        for id in ids {
            let r = if cancel {
                self.env.engine.cancel(&id)
            } else {
                match self.env.engine.get_run(&id) {
                    Some(run) => {
                        let mut req = StartRequest::new(&run.flow_id).trigger("manual", &self.env.who());
                        req.inputs = run.inputs.clone();
                        self.env.engine.start_run(req).map(|_| ())
                    }
                    None => Err("gone".into()),
                }
            };
            if r.is_ok() { ok += 1 } else { bad += 1 }
        }
        self.board.sel.clear();
        self.refresh_runs();
        self.say(format!("{} {}{}.", if cancel { "Cancelled" } else { "Started again" }, ok, if bad > 0 { format!(" ({bad} could not)") } else { String::new() }), bad == 0);
    }

    pub(super) fn tab_click(&mut self, n: usize) -> AppFlow {
        match n {
            _ if (id::TAB..id::TAB + 3).contains(&n) => self.set_tab([Tab::Design, Tab::Run, Tab::Board][n - id::TAB]),
            id::CHAT => {
                self.chat_open = !self.chat_open;
                self.focus = if self.chat_open { Focus::Chat } else { Focus::Canvas };
            }
            id::CHAT_HIDE => {
                self.chat_open = false;
                self.focus = Focus::Canvas;
            }
            id::CHAT_PANE => self.focus = Focus::Chat,
            id::FLOWS | id::CLOSE => {
                self.commit_props();
                if self.tab != Tab::Design {
                    self.tab = Tab::Design;
                }
                if self.cur.is_some() {
                    self.request_close();
                } else {
                    self.close_flow();
                }
            }
            id::R_BACK => self.leave_run(),
            id::R_AGAIN => self.run_again(),
            id::OFFER => self.toggle_offer(),
            id::SUG_OPEN => self.open_suggestions(),
            id::ASSIST_ROLLBACK => self.assist_rollback(),
            id::ASSIST_PAUSE => self.assist_toggle_pause(),
            id::FAIL_FIX => self.fix_requested = true,
            id::FAIL_DISMISS => self.dismiss_fail(),
            id::FAIL_LOG => {
                let run = self.fail.as_ref().map(|f| f.run_id.clone()).filter(|_| self.tab != Tab::Run);
                self.open_log(run);
            }
            id::NEW | id::B_NEW => {
                self.tab = Tab::Design;
                self.name_form(NameFor::NewFlow);
            }
            id::TEMPLATE => self.template_picker(),
            id::IMPORT => self.import_form(),
            id::EXPORT => self.export_flow(),
            id::ADD => self.add_picker(),
            id::UNDO => self.undo_step(),
            id::REDO => self.redo_step(),
            id::COPY => self.copy_selection(),
            id::PASTE => self.paste_clip(),
            id::DELETE => self.delete_selection(),
            id::ARRANGE => self.arrange(),
            id::FIT => self.fit_button(),
            id::ZOOM_IN => self.zoom_button(1.25),
            id::ZOOM_OUT => self.zoom_button(0.8),
            id::TEXT => self.toggle_text(),
            id::TEST => self.test_selected(),
            id::RUN => self.run_flow(false),
            id::RUN_DRAFT => self.run_flow(true),
            id::PUBLISH => self.open_publish(),
            id::HISTORY => self.open_history(),
            id::HOOKS => self.open_hooks(),
            id::APPLY_SCHEDULED => self.apply_update(ApplyScope::Scheduled),
            id::APPLY_RUNNING => self.apply_update(ApplyScope::Running),
            id::LEAVE_UPDATE => {
                self.updates.clear();
                self.updates_at = self.tick;
            }
            id::R_CANCEL => {
                if let Some(r) = self.run_id.clone() {
                    match self.env.engine.cancel(&r) {
                        Ok(()) => self.say(format!("Cancelling run {r}."), true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            id::R_RETRY | id::R_SKIP | id::R_RERUN | id::R_APPROVE | id::R_REJECT => {
                let (Some(r), Some(node)) = (self.run_id.clone(), self.run_node.clone()) else {
                    self.say("Click a step first.", false);
                    return AppFlow::Stay;
                };
                let who = self.env.who();
                let res = match n {
                    id::R_RETRY => self.env.engine.retry_node(&r, &node),
                    id::R_SKIP => self.env.engine.skip_node(&r, &node, &who),
                    id::R_RERUN => self.env.engine.rerun_from(&r, &node),
                    id::R_APPROVE => self.env.engine.decide(&r, &node, true, &who, ""),
                    _ => self.env.engine.decide(&r, &node, false, &who, ""),
                };
                match res {
                    Ok(()) => self.say("Done.", true),
                    Err(e) => self.say(e, false),
                }
            }
            id::R_ALLOW => {
                if let Some(p) = self.refused_program() {
                    self.modal = Some(Modal::Confirm(format!("Allow flow commands to start `{p}` for {}? Any step of any of this account's flows could then run it.", self.env.user), Confirmed::AllowProgram(p)));
                }
            }
            id::R_NEWER => self.step_run(true),
            id::R_OLDER => self.step_run(false),
            _ if (id::R_TAB..id::R_TAB + 4).contains(&n) => {
                self.rv.tab = [RunTab::Output, RunTab::Input, RunTab::Log, RunTab::Attempts][n - id::R_TAB];
                self.rv.shown = Default::default();
            }
            id::R_TAB_PICS => {
                self.rv.tab = RunTab::Pictures;
                self.rv.shown = Default::default();
            }
            id::R_TAB_REPORT => {
                self.rv.tab = RunTab::Report;
                self.rv.shown = Default::default();
            }
            id::R_EXPORT_HTML => self.export_report(true),
            id::R_EXPORT_JSON => self.export_report(false),
            _ if (id::R_PIC..id::R_PIC + 60).contains(&n) => self.rv.pic_sel = n - id::R_PIC,
            id::CARD_APPROVE => self.card_action("approve"),
            id::CARD_REJECT => self.card_action("reject"),
            id::CARD_EDIT => self.card_action("edit"),
            id::CARD_ASK => self.card_action("ask"),
            id::CARD_SETTINGS => self.open_flow_settings(),
            id::CARD_MODELS => self.jump = Some(("settings".into(), "tab:models".into())),
            id::CARD_REFRESH => self.refresh_models_and_retry(),
            _ if (id::B_LANE..id::B_LANE + 5).contains(&n) => self.board.lane = n - id::B_LANE,
            id::B_FLOW => self.cycle_flow_filter(),
            id::B_RUN_AGAIN => self.bulk(false),
            id::B_CANCEL_SEL => self.bulk(true),
            id::B_CLEAR => self.board.sel.clear(),
            _ if (id::PAL..id::PAL + 60).contains(&n) => {
                if let Some(k) = self.palette_ids.get(n - id::PAL).cloned() {
                    self.add_step(&k);
                }
            }
            _ if (id::P_CHOICE..id::P_CHOICE + 100 * id::P_CHOICE_STRIDE).contains(&n) => {
                let (f, o) = ((n - id::P_CHOICE) / id::P_CHOICE_STRIDE, (n - id::P_CHOICE) % id::P_CHOICE_STRIDE);
                self.choose(f, o);
            }
            _ => {}
        }
        AppFlow::Stay
    }

    // ------------------------------------------------------------------------------------------ keys

    pub(super) fn key(&mut self, k: &Key, ev: &Event, cx: &AppCx) -> AppFlow {
        match k {
            Key::F(6) if self.chat_open => {
                self.focus = if self.focus == Focus::Chat { Focus::Canvas } else { Focus::Chat };
                return AppFlow::Stay;
            }
            Key::Ctrl('i') => {
                self.chat_open = !self.chat_open;
                self.focus = if self.chat_open { Focus::Chat } else { Focus::Canvas };
                return AppFlow::Stay;
            }
            _ => {}
        }
        if self.focus == Focus::Chat && self.chat_open {
            return self.to_chat_key(ev, cx);
        }
        match self.focus {
            Focus::Search => {
                match k {
                    Key::Esc => self.focus = Focus::Canvas,
                    Key::Enter => {
                        if let Some(kind) = self.palette_ids.first().cloned() {
                            self.add_step(&kind);
                            self.palette.clear();
                            self.focus = Focus::Canvas;
                        }
                    }
                    other => {
                        self.palette.handle(other);
                    }
                }
                AppFlow::Stay
            }
            Focus::BoardSearch => {
                match k {
                    Key::Esc | Key::Enter => self.focus = Focus::Canvas,
                    other => {
                        self.board.search.handle(other);
                    }
                }
                AppFlow::Stay
            }
            Focus::Text => {
                if *k == Key::Esc {
                    self.toggle_text();
                } else if let Some(q) = self.text.as_mut() {
                    q.handle(ev);
                    self.text_changed();
                }
                AppFlow::Stay
            }
            Focus::Props => {
                self.props_key(k);
                AppFlow::Stay
            }
            _ => self.canvas_key(k),
        }
    }

    fn to_chat_key(&mut self, ev: &Event, cx: &AppCx) -> AppFlow {
        let f = self.chat.handle(ev, cx);
        if matches!(f, AppFlow::Back) {
            self.focus = Focus::Canvas;
        }
        AppFlow::Stay
    }

    fn canvas_key(&mut self, k: &Key) -> AppFlow {
        // keys of every tab
        match k {
            Key::Char('1') => return self.tab_click(id::TAB),
            Key::Char('2') => return self.tab_click(id::TAB + 1),
            Key::Char('3') => return self.tab_click(id::TAB + 2),
            Key::Char('/') => {
                match self.tab {
                    Tab::Board => self.focus = Focus::BoardSearch,
                    Tab::Design if self.cur.is_some() => self.focus = Focus::Search,
                    _ => {}
                }
                return AppFlow::Stay;
            }
            _ => {}
        }
        if self.tab != Tab::Board && self.cur.is_some() {
            if let Some(f) = self.assist_key(k) {
                return f;
            }
            if let Some(f) = self.fail_key(k) {
                return f;
            }
        }
        match self.tab {
            Tab::Design if self.cur.is_none() => self.list_key(k),
            Tab::Design => self.design_key(k),
            Tab::Run => self.run_key(k),
            Tab::Board => self.board_key(k),
        }
    }

    fn list_key(&mut self, k: &Key) -> AppFlow {
        let n = self.flows.len();
        match k {
            Key::Up => self.list_sel = self.list_sel.saturating_sub(1),
            Key::Down => self.list_sel = (self.list_sel + 1).min(n.saturating_sub(1)),
            Key::Enter => {
                if let Some(f) = self.flows.get(self.list_sel).cloned() {
                    self.open_flow(&f.id);
                }
            }
            Key::Char('n') => self.name_form(NameFor::NewFlow),
            Key::Char('t') => self.template_picker(),
            Key::Char('i') => self.import_form(),
            Key::Char('o') => self.toggle_offer(),
            Key::Delete | Key::Char('d') => {
                if let Some(f) = self.flows.get(self.list_sel) {
                    let n = self.env.store.versions(&f.id).len();
                    self.modal = Some(Modal::Confirm(format!("Delete the flow \"{}\" ({})? Its draft and {n} published version(s) are lost; its runs stay. Press y to delete, Esc or Enter keeps it.", f.name, f.id), Confirmed::DeleteFlow(f.id.clone())));
                }
            }
            Key::Esc => return AppFlow::Back,
            _ => {}
        }
        AppFlow::Stay
    }

    fn design_key(&mut self, k: &Key) -> AppFlow {
        let step = |shift: bool| if shift { GRID * 5.0 } else { GRID };
        match k {
            Key::Esc => {
                if !self.sel.is_empty() || self.sel_edge.is_some() {
                    self.sel.clear();
                    self.sel_edge = None;
                    self.props = Props::default();
                } else {
                    self.request_close();
                }
            }
            Key::Delete | Key::Backspace => self.delete_selection(),
            Key::Enter => {
                if self.sel.len() == 1 || self.sel_edge.is_some() {
                    self.sync_props();
                    self.props.focus = if self.sel_edge.is_some() { 0 } else { 1.min(self.props.fields.len().saturating_sub(1)) };
                    self.focus = Focus::Props;
                }
            }
            Key::Tab => self.cycle_selection(false),
            Key::BackTab => self.cycle_selection(true),
            Key::Left => self.move_selection(-step(false), 0.0),
            Key::Right => self.move_selection(step(false), 0.0),
            Key::Up => self.move_selection(0.0, -step(false)),
            Key::Down => self.move_selection(0.0, step(false)),
            Key::Nav { nav, shift, .. } => {
                let d = step(*shift);
                match nav {
                    Nav::Left => self.move_selection(-d, 0.0),
                    Nav::Right => self.move_selection(d, 0.0),
                    Nav::Up => self.move_selection(0.0, -d),
                    Nav::Down => self.move_selection(0.0, d),
                    _ => {}
                }
            }
            Key::Ctrl('z') => self.undo_step(),
            Key::Ctrl('y') | Key::Ctrl('Z') => self.redo_step(),
            Key::Ctrl('c') => self.copy_selection(),
            Key::Ctrl('x') => {
                self.copy_selection();
                self.delete_selection();
            }
            Key::Ctrl('v') => self.paste_clip(),
            Key::Ctrl('d') => {
                self.copy_selection();
                self.paste_clip();
            }
            Key::Ctrl('a') => {
                self.sel = self.flow.nodes.iter().map(|n| n.id.clone()).collect();
                self.props = Props::default();
            }
            Key::Ctrl('s') => self.say("Saved: a flow saves itself after every change. Publish makes a version.", true),
            Key::F(2) => self.name_form(NameFor::Rename),
            Key::Char('a') => self.add_picker(),
            Key::Char('s') => self.open_flow_settings(),
            Key::Char('f') => self.fit_button(),
            Key::Char('+') | Key::Char('=') => self.zoom_button(1.25),
            Key::Char('-') => self.zoom_button(0.8),
            Key::Char('g') => self.arrange(),
            Key::Char('t') => self.toggle_text(),
            Key::Char('?') => self.tip.show_now(),
            Key::Char('r') => self.run_flow(false),
            Key::Char('p') => self.open_publish(),
            Key::Char('h') => self.open_history(),
            Key::Char('w') => self.open_hooks(),
            Key::Char('T') => self.test_selected(),
            Key::Char('e') => {
                if self.sel.len() == 1 {
                    self.tab_click_enter();
                }
            }
            _ => {}
        }
        AppFlow::Stay
    }

    fn tab_click_enter(&mut self) {
        self.sync_props();
        self.props.focus = 1.min(self.props.fields.len().saturating_sub(1));
        self.focus = Focus::Props;
    }

    fn run_key(&mut self, k: &Key) -> AppFlow {
        match k {
            Key::Esc => self.leave_run(),
            Key::Char('a') => self.run_again(),
            Key::Left => self.step_run(false),
            Key::Right => self.step_run(true),
            Key::Tab | Key::BackTab => {
                if let Some(run) = &self.run {
                    let ids: Vec<String> = run.flow.nodes.iter().map(|n| n.id.clone()).collect();
                    if !ids.is_empty() {
                        let cur = self.run_node.as_ref().and_then(|c| ids.iter().position(|i| i == c));
                        let next = match (cur, *k == Key::Tab) {
                            (None, _) => 0,
                            (Some(i), true) => (i + 1) % ids.len(),
                            (Some(i), false) => (i + ids.len() - 1) % ids.len(),
                        };
                        self.run_node = Some(ids[next].clone());
                        self.rv.shown = Default::default();
                    }
                }
            }
            Key::Char('o') => self.rv.tab = RunTab::Output,
            Key::Char('i') => self.rv.tab = RunTab::Input,
            Key::Char('l') => self.open_log(None),
            Key::Char('?') => self.tip.show_now(),
            Key::Char('t') => self.rv.tab = RunTab::Attempts,
            Key::Char('p') if self.run_has_browser() => {
                self.rv.tab = RunTab::Pictures;
                self.rv.shown = Default::default();
            }
            Key::Char('q') if self.run_has_browser() => {
                self.rv.tab = RunTab::Report;
                self.rv.shown = Default::default();
            }
            Key::Char('h') if self.run_has_browser() && self.rv.tab == RunTab::Report => self.export_report(true),
            Key::Char('j') if self.run_has_browser() && self.rv.tab == RunTab::Report => self.export_report(false),
            Key::Char(']') if self.rv.tab == RunTab::Pictures => self.rv.pic_sel += 1,
            Key::Char('[') if self.rv.tab == RunTab::Pictures => self.rv.pic_sel = self.rv.pic_sel.saturating_sub(1),
            Key::Char('c') => return self.tab_click(id::R_CANCEL),
            Key::Char('r') => return self.tab_click(id::R_RETRY),
            Key::Char('s') => return self.tab_click(id::R_SKIP),
            Key::Char('R') => return self.tab_click(id::R_RERUN),
            Key::Char('y') => return self.tab_click(id::R_APPROVE),
            Key::Char('n') => return self.tab_click(id::R_REJECT),
            Key::Char('+') | Key::Char('=') => self.zoom_button(1.25),
            Key::Char('-') => self.zoom_button(0.8),
            Key::Char('f') => self.fit_button(),
            Key::Char('A') => self.card_action("approve"),
            Key::Char('X') => self.card_action("reject"),
            Key::PageUp => self.rv.log.scroll_by(10),
            Key::PageDown => self.rv.log.scroll_by(-10),
            _ => {}
        }
        AppFlow::Stay
    }

    fn board_key(&mut self, k: &Key) -> AppFlow {
        let rows = self.board_rows();
        let n = rows.len();
        match k {
            Key::Esc => return AppFlow::Back,
            Key::Down | Key::Right => self.board.cursor = (self.board.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Left => self.board.cursor = self.board.cursor.saturating_sub(1),
            Key::Enter => {
                if let Some(id) = rows.get(self.board.cursor).cloned() {
                    self.select_run(&id, true);
                }
            }
            Key::Char(' ') => {
                if let Some(id) = rows.get(self.board.cursor).cloned() {
                    if !self.board.sel.remove(&id) {
                        self.board.sel.insert(id);
                    }
                }
            }
            Key::Char('c') => self.bulk(true),
            Key::Char('a') => self.bulk(false),
            Key::Tab => self.board.lane = (self.board.lane + 1) % 5,
            Key::BackTab => self.board.lane = (self.board.lane + 4) % 5,
            Key::Char('f') => self.cycle_flow_filter(),
            Key::PageDown => self.board.hist_scroll += 3,
            Key::PageUp => self.board.hist_scroll = self.board.hist_scroll.saturating_sub(3),
            _ => {}
        }
        AppFlow::Stay
    }

    // ------------------------------------------------------------------------------------------ dialogs

    pub(super) fn modal_event(&mut self, ev: &Event, cx: &AppCx) -> AppFlow {
        let _ = cx;
        let modal = self.modal.take();
        match modal {
            None => {}
            Some(Modal::Name(mut form, what)) => match form.handle_event(ev) {
                FormEvent::Submit => {
                    let v = form.value(0).trim().to_string();
                    if v.is_empty() {
                        form.error = Some("Give it a name.".into());
                        self.modal = Some(Modal::Name(form, what));
                    } else {
                        match what {
                            NameFor::NewFlow => self.new_flow(&v),
                            NameFor::Rename => self.edit("renamed the flow", |f| f.name = v.clone()),
                        }
                    }
                }
                FormEvent::Cancel => {}
                _ => self.modal = Some(Modal::Name(form, what)),
            },
            Some(Modal::Import(mut form)) => match form.handle_event(ev) {
                FormEvent::Submit => {
                    let p = form.value(0).trim().to_string();
                    match self.import_path(&p) {
                        Ok(id) => {
                            self.open_flow(&id);
                            self.say(format!("Imported as `{id}`: a draft with its schedules off."), true);
                        }
                        Err(e) => {
                            form.error = Some(e);
                            self.modal = Some(Modal::Import(form));
                        }
                    }
                }
                FormEvent::Cancel => {}
                _ => self.modal = Some(Modal::Import(form)),
            },
            Some(Modal::RunInputs(mut form, draft, params)) => match form.handle_event(ev) {
                FormEvent::Submit => {
                    let missing = params.iter().enumerate().find(|(i, p)| p.required && p.default.is_none() && (if p.ty == "secret" { form.raw(*i) } else { form.value(*i) }).is_empty()).map(|(i, p)| (i, p.name.clone()));
                    match missing {
                        Some((i, n)) => {
                            form.focus = i;
                            form.error = Some(format!("`{n}` is needed to run this flow."));
                            self.modal = Some(Modal::RunInputs(form, draft, params));
                        }
                        None => self.run_inputs_submit(&form, draft, &params),
                    }
                }
                FormEvent::Cancel => self.say("Not started.", true),
                _ => self.modal = Some(Modal::RunInputs(form, draft, params)),
            },
            Some(Modal::Templates(mut p, ids)) => match p.handle_event(ev) {
                PickEvent::Chosen(_) => {
                    if let Some(i) = p.selected_index() {
                        let tid = ids[i.min(ids.len() - 1)].clone();
                        self.from_template(&tid);
                    }
                }
                PickEvent::Cancel => {}
                _ => self.modal = Some(Modal::Templates(p, ids)),
            },
            Some(Modal::AddNode(mut p, ids)) => match p.handle_event(ev) {
                PickEvent::Chosen(_) => {
                    if let Some(i) = p.selected_index() {
                        let kind = ids[i.min(ids.len() - 1)].clone();
                        self.add_step(&kind);
                    }
                }
                PickEvent::Cancel => {}
                _ => self.modal = Some(Modal::AddNode(p, ids)),
            },
            Some(Modal::Confirm(text, what)) => match ev {
                // deleting and dropping are never confirmed by Enter: the default is to keep everything
                Event::Key(Key::Enter) if matches!(what, Confirmed::DeleteFlow(_) | Confirmed::DropVersion(_) | Confirmed::CloseFlow) => {}
                Event::Key(Key::Char('y' | 'Y') | Key::Enter) => match what {
                    Confirmed::DropVersion(v) => {
                        if let Some(id) = self.cur.clone() {
                            match self.env.drop_version(&id, v) {
                                Ok(()) => self.say(format!("Dropped version {v} of `{id}`."), true),
                                Err(e) => self.say(e, false),
                            }
                        }
                    }
                    Confirmed::DeleteFlow(id) => self.delete_flow(&id),
                    Confirmed::CloseFlow => self.finish_close(),
                    Confirmed::AllowProgram(p) => match self.env.allow_program(&p) {
                        Ok(()) => {
                            if let (Some(r), Some(n)) = (self.run_id.clone(), self.run_node.clone()) {
                                let _ = self.env.engine.retry_node(&r, &n);
                            }
                            self.say(format!("`{p}` is allowed; the step runs again."), true);
                        }
                        Err(e) => self.say(e, false),
                    },
                    Confirmed::Rollback(v) => {
                        if let Some(id) = self.cur.clone() {
                            match self.env.store.rollback(&id, v, &self.env.who()) {
                                Ok(nv) => {
                                    if let Some(f) = self.env.store.get_draft(&id) {
                                        self.flow = f;
                                    }
                                    self.revalidate();
                                    self.assist_rolled_back();
                                    self.say(format!("Rolled back to v{v}: published as v{nv}."), true);
                                }
                                Err(e) => self.say(e, false),
                            }
                        }
                    }
                },
                Event::Key(Key::Char('n' | 'N') | Key::Esc) => {}
                _ => self.modal = Some(Modal::Confirm(text, what)),
            },
            Some(Modal::Publish(mut f)) => {
                match ev {
                    Event::Key(Key::Esc) => return AppFlow::Stay,
                    Event::Key(Key::Ctrl('s')) | Event::Key(Key::Enter) if f.focus == 1 => {
                        if f.blockers.is_empty() {
                            let (l, c) = (f.label.value.trim().to_string(), f.changelog.value.trim().to_string());
                            self.do_publish(&l, if c.is_empty() { "published" } else { &c });
                            return AppFlow::Stay;
                        }
                    }
                    Event::Key(Key::Tab | Key::Down) => f.focus = (f.focus + 1) % 2,
                    Event::Key(Key::BackTab | Key::Up) => f.focus = (f.focus + 1) % 2,
                    Event::Key(Key::Enter) => f.focus = 1,
                    Event::Key(kk) => {
                        if f.focus == 0 { f.label.handle(kk); } else { f.changelog.handle(kk); }
                    }
                    Event::Paste(t) => {
                        if f.focus == 0 { f.label.paste(t); } else { f.changelog.paste(t); }
                    }
                    Event::FocusAt(i, x) if *i >= 900 && *i < 902 => {
                        f.focus = *i - 900;
                        if f.focus == 0 { f.label.click(*x); } else { f.changelog.click(*x); }
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Publish(f));
            }
            Some(Modal::Suggestions(v)) => self.suggestions_event(v, ev),
            Some(Modal::Webhook(mut p)) => {
                if self.hook_event(&mut p, ev) {
                    self.modal = Some(Modal::Webhook(p));
                }
            }
            Some(Modal::History(mut h)) => {
                let mut keep = true;
                match ev {
                    Event::Key(Key::Esc) => keep = false,
                    Event::Key(Key::Up) | Event::Scroll(1..) => {
                        h.sel = h.sel.saturating_sub(1);
                        self.fill_diff(&mut h);
                    }
                    Event::Key(Key::Down) | Event::Scroll(..=-1) => {
                        h.sel = (h.sel + 1).min(h.entries.len() - 1);
                        self.fill_diff(&mut h);
                    }
                    Event::Row(i) => {
                        h.sel = (*i).min(h.entries.len() - 1);
                        self.fill_diff(&mut h);
                    }
                    Event::Key(Key::Char('x')) => {
                        let v = h.entries[h.sel].version;
                        let cur = self.cur.clone().unwrap_or_default();
                        let published = self.env.store.meta_of(&cur).published;
                        keep = false;
                        let name = self.flow.name.clone();
                        let note = if v == published { " This is the CURRENT published version: it will be refused; roll back to another version first." } else { "" };
                        self.modal = Some(Modal::Confirm(format!("Drop version {v} of \"{name}\" ({cur})? Its content is lost for good; runs keep their own copy.{note} Press y to drop, Esc or Enter keeps it."), Confirmed::DropVersion(v)));
                    }
                    Event::Key(Key::Char('r')) => {
                        let v = h.entries[h.sel].version;
                        keep = false;
                        self.modal = Some(Modal::Confirm(format!("Roll back to version {v}? Its content is published again as a new version; nothing is lost."), Confirmed::Rollback(v)));
                    }
                    _ => {}
                }
                if keep {
                    self.modal = Some(Modal::History(h));
                }
            }
            Some(Modal::Result(t, mut log)) => match ev {
                Event::Key(Key::Esc | Key::Enter) => {}
                other => {
                    log.handle(other);
                    if let Event::Scroll(d) = other {
                        log.scroll_by(*d * 3);
                    }
                    self.modal = Some(Modal::Result(t, log));
                }
            },
            Some(Modal::EditPatch(t, mut qb)) => match ev {
                Event::Key(Key::Esc) => {}
                Event::Key(Key::Ctrl('s')) => {
                    let parsed = serde_json::from_str::<serde_json::Value>(&qb.text()).map_err(|e| e.to_string()).and_then(|v| iyakki_flow::heal::Patch::from_value(&v));
                    match (parsed, self.run_id.clone(), self.open_card()) {
                        (Ok(p), Some(run), Some(card)) => match self.env.engine.card_decide(&run, &card.id, CardDecision::Edit { by: self.env.who(), patch: p }) {
                            Ok(()) => self.say("Approved your version of the fix.", true),
                            Err(e) => self.say(e, false),
                        },
                        (Err(e), ..) => {
                            self.say(format!("That is not a valid fix: {e}"), false);
                            self.modal = Some(Modal::EditPatch(t, qb));
                        }
                        _ => {}
                    }
                }
                Event::AreaAt(n, x, y) if *n == area::TEXT => {
                    qb.click(*x, *y, crate::ui::shift_held());
                    self.modal = Some(Modal::EditPatch(t, qb));
                }
                other => {
                    qb.handle(other);
                    self.modal = Some(Modal::EditPatch(t, qb));
                }
            },
        }
        AppFlow::Stay
    }
}

impl App for FlowsApp {
    fn take_jump(&mut self) -> Option<(String, String)> {
        self.jump.take()
    }

    fn title(&self) -> &'static str {
        "FLOWS"
    }

    fn icon(&self) -> Icon {
        Icon::Flows
    }

    fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    fn hint(&self) -> String {
        self.footer_hint()
    }

    fn help(&self) -> Vec<String> {
        vec![
            "1 2 3: DESIGN, RUN, BOARD".into(),
            "Esc: one step back. In a flow it deselects, then closes the flow (everything is saved; only text that was refused asks first). In a run it returns to where you came from. On the list or the board it leaves FLOWS".into(),
            "Run: r in DESIGN (a flow with required inputs asks for them first) · RUN: c stop · a run again · l open the log".into(),
            "A failed last run: a card offers F ask the agent to fix (it opens the assistant on this flow with the failure loaded; edits and runs still ask you; three attempts) · L open the log · x dismiss · o on the list turns the offer on or off".into(),
            "Ctrl+I: show or hide the assistant · F6: move the keyboard between the flow and the assistant".into(),
            "DESIGN: a add a step · f fit · + - zoom · g arrange · t text view · / search steps · Tab next step".into(),
            "DESIGN: arrows nudge the selection (Shift: 5 grid steps; none selected: pan) · Del remove".into(),
            "DESIGN: Ctrl+Z undo · Ctrl+Y redo · Ctrl+C copy · Ctrl+V paste · Ctrl+X cut · Ctrl+D duplicate · Ctrl+A all".into(),
            "DESIGN: Enter edits the selected step's settings · Ctrl+Enter commits a text box · Esc undoes the edit".into(),
            "DESIGN: T test the step · r run · p publish · h versions".into(),
            "Mouse: drag a step to move · drag from a round port to a step to connect · drag the background to pan · wheel zooms".into(),
            "RUN: Tab next step · o i l t output, input, log, attempts · p pictures, q report (h saves it as a web page, j as data) of a browser run, [ ] pictures · r retry · s skip · R rerun from here · c cancel the run".into(),
            "RUN: y approve · n reject (an approval step) · A approve the fix · X reject it · Left Right older and newer runs".into(),
            "BOARD: arrows move · Enter opens · Space selects · a run again · c cancel · Tab next lane · / search · f flow".into(),
        ]
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> AppFlow {
        self.handle_impl(ev, cx)
    }

    fn wants_redraw(&mut self) -> bool {
        let tip = self.tip.take_changed();
        std::mem::take(&mut self.redraw) || tip
    }

    fn pointer_hint(&self, x: i32, y: i32) -> Option<Pointer> {
        if !self.geo.canvas.contains(x, y) || self.modal.is_some() || self.text.is_some() {
            return None;
        }
        if matches!(self.drag, Some(Drag::Pan { .. })) {
            return Some(Pointer::Grab);
        }
        match &self.hover {
            Hit::Port(_) => Some(Pointer::Crosshair),
            Hit::Node(_) => Some(Pointer::Move),
            Hit::Edge(_) => Some(Pointer::Hand),
            Hit::Empty => Some(Pointer::Default),
        }
    }

    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    fn on_drop(&mut self, paths: &[std::path::PathBuf], _cx: &AppCx) -> DropAction {
        let mut done = 0;
        let mut last = None;
        for p in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "json")) {
            if let Some(s) = p.to_str() {
                match self.import_path(s) {
                    Ok(id) => {
                        done += 1;
                        last = Some(id);
                    }
                    Err(e) => self.say(e, false),
                }
            }
        }
        if let Some(id) = last {
            self.open_flow(&id);
            self.say(format!("Imported {done} flow(s) as drafts."), true);
            DropAction::Taken
        } else {
            DropAction::Ignore("FLOWS imports flow files (.json).".into())
        }
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        if self.tab != Tab::Design || self.cur.is_none() || !self.geo.canvas.contains(x, y) {
            return vec![];
        }
        let hit = hit_test(&self.flow, &self.reg, &self.view, self.geo.canvas, x as f32, y as f32);
        if let Hit::Node(id) = &hit {
            if !self.sel.contains(id) {
                let id = id.clone();
                self.select_node(&id);
            }
        }
        let mut v = Vec::new();
        match hit {
            Hit::Node(_) => {
                v.push(Item::new("flows.test", "Test this step", ""));
                v.push(Item::new("flows.copy", "Copy", ""));
                v.push(Item::new("flows.delete", "Delete", ""));
            }
            Hit::Edge(i) => {
                self.sel_edge = Some(i);
                self.sel.clear();
                v.push(Item::new("flows.delete", "Remove the connection", ""));
            }
            _ => {
                v.push(Item::new("flows.add", "Add a step...", ""));
                v.push(Item::new("flows.paste", "Paste", ""));
                v.push(Item::new("flows.arrange", "Arrange", ""));
            }
        }
        v
    }

    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> AppFlow {
        match id {
            "flows.test" => self.test_selected(),
            "flows.copy" => self.copy_selection(),
            "flows.delete" => self.delete_selection(),
            "flows.add" => self.add_picker(),
            "flows.paste" => self.paste_clip(),
            "flows.arrange" => self.arrange(),
            _ => {}
        }
        AppFlow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.render_impl(g, area, s, cx, tick);
    }
}
