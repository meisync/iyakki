//! Settings -> Assistant: what the assistant remembers of the work in a workspace (the history switch, Forget per workspace and per
//! chat) and how long a model may think or stay silent before a call is given up (all models, or one provider). Everything here is the
//! person's own: the history is kept per person, and the limits are the machine's choice that an administrator changes.
//! The same actions are chat tools (`assistant_memory_*`, `assistant_limits_*`), which ask every time.

use super::*;
use iyakki_agent::history::{History, WorkspaceMemory};
use iyakki_agent::settings::ProviderLimits;

/// The steps a limit moves through, in seconds (0 = the default).
pub(super) const STEPS: [u32; 8] = [0, 30, 60, 120, 180, 300, 600, 900];

#[derive(Default)]
pub(super) struct AsstPanel {
    /// The workspace whose chats are listed under it.
    pub(super) open: Option<String>,
    /// What the next press of the same key will forget (a sentence), so one slip never deletes anything.
    pub(super) confirm: Option<(String, char)>,
    /// The rows as they were last drawn or handled, for the buttons (which have no access to the stores).
    cache: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Switch,
    Workspace(WorkspaceMemory),
    Chat { workspace: String, chat: String, entries: usize },
    Limits(Option<String>),
    /// One of the "Waiting" settings (auto-continue, wakes per day, longest wait, follow-up thread): the key of the row.
    Waiting(&'static str),
}

pub(super) fn step_up(v: u32) -> u32 {
    STEPS.iter().copied().find(|s| *s > v).unwrap_or(*STEPS.last().unwrap_or(&900))
}

pub(super) fn step_down(v: u32) -> u32 {
    STEPS.iter().rev().copied().find(|s| *s < v).unwrap_or(0)
}

pub(super) fn secs_words(v: u32) -> String {
    match v {
        0 => "default".into(),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

impl SettingsApp {
    pub(super) fn asst_rows(&self, cx: &AppCx) -> Vec<Row> {
        let mut rows = vec![Row::Switch];
        for w in History::workspaces_of(&Self::svc(cx).kb, &cx.session.user) {
            let name = w.name.clone();
            rows.push(Row::Workspace(w));
            if self.asst.open.as_deref() == Some(name.as_str()) {
                for (chat, entries) in History::sessions_of(&Self::svc(cx).kb, &cx.session.user, &name) {
                    rows.push(Row::Chat { workspace: name.clone(), chat, entries });
                }
            }
        }
        rows.push(Row::Limits(None));
        for p in Self::providers(cx) {
            rows.push(Row::Limits(Some(p.id)));
        }
        for r in iyakki_agent::pendingplus::settings_rows(&iyakki_agent::pending::Settings::default(), 0) {
            rows.push(Row::Waiting(r.key));
        }
        rows
    }

    fn asst_ledger(cx: &AppCx) -> iyakki_agent::pending::Ledger {
        iyakki_agent::pending::Ledger::open(&Self::svc(cx).dir().join("pending"), &cx.session.user)
    }

    /// The row of the Waiting settings with this key, as the ledger holds it now.
    fn asst_waiting_row(cx: &AppCx, key: &str) -> Option<iyakki_agent::pendingplus::Row> {
        let l = Self::asst_ledger(cx);
        iyakki_agent::pendingplus::settings_rows(&l.settings(), l.wakes_left()).into_iter().find(|r| r.key == key)
    }

    /// Switch or step one Waiting setting: `dir` 0 flips an on/off one, -1 and +1 step a number. Saved, audited, said.
    fn asst_waiting_change(&mut self, cx: &AppCx, key: &str, dir: i32) {
        let l = Self::asst_ledger(cx);
        let mut s = l.settings();
        let value = match key {
            "auto_continue" => if s.auto_continue { "off" } else { "on" }.to_string(),
            "separate_thread" => if s.separate_thread { "off" } else { "on" }.to_string(),
            "max_wakes_per_day" => (i64::from(s.max_wakes_per_day) + if dir == 0 { 1 } else { i64::from(dir) } * 5).clamp(1, 500).to_string(),
            "max_wait_hours" => (i64::try_from(s.max_wait_secs / 3600).unwrap_or(24) + if dir == 0 { 1 } else { i64::from(dir) } * 6).clamp(1, 168).to_string(),
            _ => return,
        };
        match iyakki_agent::pendingplus::apply_setting(&mut s, key, &value) {
            Ok(()) => {
                l.set_settings(s);
                cx.audit.record(&cx.session.user, "pending.settings", &format!("{key} {value}"));
                self.ok(format!("Saved: {key} {value}."));
            }
            Err(e) => self.fail(e),
        }
    }

    fn asst_refresh(&mut self, cx: &AppCx) -> Vec<Row> {
        let rows = self.asst_rows(cx);
        self.asst.cache = rows.clone();
        rows
    }

    fn asst_limits(cx: &AppCx, provider: &Option<String>) -> (u32, u32) {
        let st = Self::svc(cx).settings.lock().unwrap_or_else(|e| e.into_inner());
        match provider {
            None => (st.get().think_limit_secs, st.get().silence_limit_secs),
            Some(p) => st.get().provider_limits.get(p).map_or((0, 0), |l| (l.think_secs, l.silence_secs)),
        }
    }

    fn asst_set_limits(&mut self, cx: &AppCx, provider: &Option<String>, think: u32, silence: u32) {
        if !cx.session.policy.manage_settings {
            self.fail("Only an administrator changes how long a model may think or stay silent.");
            return;
        }
        let r = Self::svc(cx).settings.lock().map(|mut s| {
            s.update(|s| match provider {
                None => {
                    s.think_limit_secs = think;
                    s.silence_limit_secs = silence;
                }
                Some(p) if think == 0 && silence == 0 => {
                    s.provider_limits.remove(p);
                }
                Some(p) => {
                    s.provider_limits.insert(p.clone(), ProviderLimits { think_secs: think, silence_secs: silence });
                }
            })
        });
        match r {
            Ok(Ok(_)) => {
                cx.audit.record(&cx.session.user, "settings.assistant.limits", &format!("{}: think {} silence {}", provider.as_deref().unwrap_or("all models"), secs_words(think), secs_words(silence)));
                self.ok(format!("Saved: {} may think for {} and stay silent for {}.", provider.as_deref().unwrap_or("every model"), secs_words(think), secs_words(silence)));
            }
            Ok(Err(e)) => self.fail(e.to_string()),
            Err(_) => self.fail("The settings are busy; try again."),
        }
    }

    pub(super) fn handle_assistant(&mut self, k: &Key, cx: &AppCx) {
        let rows = self.asst_refresh(cx);
        let len = rows.len();
        let sel = self.sel().min(len.saturating_sub(1));
        let row = rows.get(sel).cloned();
        // a confirmation holds for the same key only
        let pending = self.asst.confirm.take();
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Home => self.move_sel(-(len as i32), len),
            Key::End => self.move_sel(len as i32, len),
            Key::Enter | Key::Char('o') => match row {
                Some(Row::Workspace(w)) => self.asst.open = if self.asst.open.as_deref() == Some(w.name.as_str()) { None } else { Some(w.name) },
                Some(Row::Switch) => self.asst_toggle(cx),
                Some(Row::Waiting(rk)) => self.asst_waiting_change(cx, rk, 0),
                _ => {}
            },
            Key::Char('-') | Key::Char('+') => {
                if let Some(Row::Waiting(rk)) = row {
                    self.asst_waiting_change(cx, rk, if matches!(k, Key::Char('+')) { 1 } else { -1 });
                }
            }
            Key::Char('h') => self.asst_toggle(cx),
            Key::Char('f') => {
                let (what, key) = match &row {
                    Some(Row::Workspace(w)) => (format!("all {} entries of the workspace {}", w.entries, w.name), 'f'),
                    Some(Row::Chat { workspace, entries, .. }) => (format!("the {entries} entries of this chat in {workspace}"), 'f'),
                    _ => {
                        self.fail("Select a workspace or one of its chats first.");
                        return;
                    }
                };
                if pending.as_ref().is_some_and(|p| p.0 == what && p.1 == key) {
                    self.asst_forget(cx, row);
                } else {
                    self.status = Some((format!("This forgets {what} and cannot be undone. Press F again to do it."), false));
                    self.asst.confirm = Some((what, key));
                }
            }
            Key::Char(c @ ('t' | 'T' | 'q' | 'Q' | '0')) => {
                let Some(Row::Limits(p)) = row else {
                    self.fail("Select a line under Stall limits first.");
                    return;
                };
                let (t, q) = Self::asst_limits(cx, &p);
                let (t, q) = match c {
                    't' => (step_up(t), q),
                    'T' => (step_down(t), q),
                    'q' => (t, step_up(q)),
                    'Q' => (t, step_down(q)),
                    _ => (0, 0),
                };
                self.asst_set_limits(cx, &p, t, q);
            }
            _ => {}
        }
    }

    fn asst_toggle(&mut self, cx: &AppCx) {
        let now = Self::svc(cx).settings.lock().map(|s| s.get().no_workspace_history).unwrap_or(false);
        let r = Self::svc(cx).settings.lock().map(|mut s| s.update(|s| s.no_workspace_history = !now));
        match r {
            Ok(Ok(_)) => {
                cx.audit.record(&cx.session.user, "settings.assistant.history", if now { "on" } else { "off" });
                self.ok(if now { "The assistant remembers the work of a workspace again." } else { "The assistant no longer keeps a history of the work of a workspace. What is kept stays until you forget it." });
            }
            Ok(Err(e)) => self.fail(e.to_string()),
            Err(_) => self.fail("The settings are busy; try again."),
        }
    }

    fn asst_forget(&mut self, cx: &AppCx, row: Option<Row>) {
        let kb = &Self::svc(cx).kb;
        let (n, what) = match row {
            Some(Row::Workspace(w)) => (History::forget_workspace_of(kb, &cx.session.user, &w.name), format!("workspace {}", w.name)),
            Some(Row::Chat { workspace, chat, .. }) => (History::forget_session_of(kb, &cx.session.user, &workspace, &chat), format!("chat {chat} in {workspace}")),
            _ => return,
        };
        cx.audit.record(&cx.session.user, "settings.assistant.forget", &format!("{what}: {n} entries"));
        self.ok(format!("Forgot {n} entries of the {what}."));
        let len = self.asst_rows(cx).len();
        self.move_sel(0, len);
    }

    pub(super) fn assistant_buttons(&self) -> Vec<Btn> {
        let rows = &self.asst.cache;
        match rows.get(self.sel().min(rows.len().saturating_sub(1))) {
            Some(Row::Workspace(_)) => vec![("Show chats", ACCENT, 'o'), ("Forget this history", crate::ui::ERR, 'f'), ("Remember on/off", MUTED, 'h')],
            Some(Row::Chat { .. }) => vec![("Forget this chat", crate::ui::ERR, 'f'), ("Remember on/off", MUTED, 'h')],
            Some(Row::Limits(_)) => vec![("Think +", ACCENT, 't'), ("Think -", MUTED, 'T'), ("Silence +", ACCENT, 'q'), ("Silence -", MUTED, 'Q'), ("Default", MUTED, '0')],
            Some(Row::Waiting(_)) => vec![("Switch (Enter)", ACCENT, 'o'), ("More (+)", ACCENT, '+'), ("Less (-)", MUTED, '-')],
            _ => vec![("Remember on/off", ACCENT, 'h')],
        }
    }

    pub(super) fn render_assistant(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = self.asst_refresh(cx);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let admin = cx.session.policy.manage_settings;
        let remember = !Self::svc(cx).settings.lock().map(|s| s.get().no_workspace_history).unwrap_or(false);
        let rh = 46.0 * s;
        let mut y = a.y;
        let heading = |g: &mut Gfx, y: &mut f32, t: &str| {
            let t = g.fit(14.0 * s, t, Face::Bold, a.w);
            g.text(a.x, *y, 14.0 * s, MUTED, &t, Face::Bold);
            *y += 24.0 * s;
        };
        heading(g, &mut y, "What the assistant remembers");
        let mut limits_head = false;
        let mut waiting_head = false;
        for (i, r) in rows.iter().enumerate() {
            if matches!(r, Row::Limits(_)) && !limits_head {
                limits_head = true;
                y += 8.0 * s;
                heading(g, &mut y, if admin { "Stall limits: how long a model may think or stay silent" } else { "Stall limits (an administrator changes them)" });
            }
            if matches!(r, Row::Waiting(_)) && !waiting_head {
                waiting_head = true;
                y += 8.0 * s;
                heading(g, &mut y, "Waiting: when the assistant says it will report back");
            }
            if y + rh > a.y + a.h {
                break;
            }
            let (title, detail, indent) = match r {
                Row::Switch => ("Remember the work of each workspace".to_string(), if remember { "On: steps, results and summaries are kept per workspace so a later chat can look them up." } else { "Off: nothing new is kept. What is already kept stays until you forget it." }.to_string(), 0.0),
                Row::Workspace(w) => (w.name.clone(), format!("{} entries from {} chat{}", w.entries, w.chats, if w.chats == 1 { "" } else { "s" }), 0.0),
                Row::Chat { chat, entries, .. } => (format!("chat {chat}"), format!("{entries} entries"), 22.0 * s),
                Row::Waiting(k) => match Self::asst_waiting_row(cx, k) {
                    Some(w) => (w.label.to_string(), format!("{}: {}", w.value, w.hint), 0.0),
                    None => ((*k).to_string(), String::new(), 0.0),
                },
                Row::Limits(p) => {
                    let (t, q) = Self::asst_limits(cx, p);
                    (p.clone().unwrap_or_else(|| "All models".into()), format!("think {} · silent {}{}", secs_words(t), secs_words(q), if p.is_some() && t == 0 && q == 0 { " (follows all models)" } else { "" }), 0.0)
                }
            };
            let r_rect = R::new(a.x + indent, y, a.w - indent, rh - 4.0 * s);
            crate::fx::row_at(g, r_rect.x, r_rect.y, r_rect.w, r_rect.h, 10.0 * s, i == sel, g.hovered(r_rect), ACCENT);
            g.hit(r_rect, Event::Row(i), &title);
            let tw = r_rect.w - 24.0 * s;
            let title = g.fit(15.0 * s, &title, Face::Bold, tw);
            g.text(r_rect.x + 12.0 * s, y + 4.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
            let detail = g.fit(13.0 * s, &detail, Face::Sans, tw);
            g.text(r_rect.x + 12.0 * s, y + 24.0 * s, 13.0 * s, MUTED, &detail, Face::Sans);
            y += rh;
        }
    }
}
