//! The model chooser shared by the chat, the AI IDE's chat and TALES: the models of every enabled provider (provider name on
//! the right), then one group per external coding agent (`Claude Code (agent)  ready`) with its models indented under it and
//! `Custom model id…`. The owner of the screen keeps the list in a field, forwards events to [`key`], draws it with [`draw`] and
//! acts on the [`Pick`] that comes back.

use iyakki_agent::extagent::{self, ModelInfo};

use super::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(79, 140, 255);

/// One line of the chooser.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MRow {
    /// A model of a provider of Settings → Models.
    Plain,
    /// The extra first line some screens add ("Same as chat", "Automatic").
    Top,
    /// An external coding agent (choosing it keeps its current model).
    Head(String),
    /// One of the agent's models, shown indented under it.
    Model(String, ModelInfo),
    /// "Custom model id…" under the agent.
    Custom(String),
}

/// What is in use now: marks the line and decides where the list opens.
#[derive(Clone, Debug, Default)]
pub(crate) struct Current {
    /// The extra first line is the choice.
    pub same: bool,
    /// (provider, model) in use.
    pub plain: Option<(String, String)>,
    /// (agent id, model id; empty = the CLI's default) in use.
    pub agent: Option<(String, String)>,
}

/// The extra first line: its name and the grey text on its right.
#[derive(Clone, Debug)]
pub(crate) struct TopRow {
    pub label: String,
    pub note: String,
}

/// The chooser: (provider, model) pairs, the highlighted one, and what each line is.
pub(crate) struct ModelList {
    pub items: Vec<(String, String)>,
    pub sel: usize,
    /// What each line is (same order as `items`).
    pub rows: Vec<MRow>,
    /// A model id being typed for an agent ("Custom model id…"): (agent id, text so far).
    pub custom: Option<(String, String)>,
    pub title: String,
    pub hint: String,
    pub current: Current,
    top: Option<TopRow>,
}

/// What the person chose.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pick {
    /// The extra first line.
    Top,
    Provider(String, String),
    /// An agent, keeping its current model.
    Agent(String),
    AgentModel { agent: String, model: String, custom: bool },
}

/// What a key or click did to the list.
#[derive(Debug, PartialEq)]
pub(crate) enum Out {
    Stay,
    Close,
    Pick(Pick),
    /// A model id was refused: the text to show; the list stays open.
    Bad(String),
}

/// Build the list. `plain` are the (provider, model) pairs, `top` the optional extra first line, `agents` whether the
/// external agents follow. `None` when there is nothing to choose.
pub(crate) fn build(cx: &AppCx, plain: Vec<(String, String)>, top: Option<TopRow>, agents: bool, current: Current, title: &str, hint: &str) -> Option<ModelList> {
    let mut items: Vec<(String, String)> = Vec::new();
    let mut rows: Vec<MRow> = Vec::new();
    if top.is_some() {
        items.push((String::new(), String::new()));
        rows.push(MRow::Top);
    }
    let offset = items.len();
    for p in plain {
        items.push(p);
        rows.push(MRow::Plain);
    }
    let builtin = items.len();
    let mut sel = if current.same && top.is_some() { 0 } else { current.plain.as_ref().and_then(|(p, m)| items.iter().skip(offset).position(|(ip, im)| ip == p && im == m).map(|i| i + offset)).unwrap_or(0) };
    let mut chosen_at = None;
    if agents {
        let cfg = cx.services.agents_cfg();
        let entries = extagent::entries(&cfg, &cx.services.ext_status);
        if entries.iter().any(|e| cx.services.ext_status.get(&e.id).is_none()) {
            cx.services.ext_status.refresh(cfg.clone());
        }
        let (cur_agent, cur_model) = match &current.agent {
            Some((a, m)) => (Some(a.as_str()), Some(m.as_str())),
            None => (None, None),
        };
        // an agent, then its models indented under it: the CLI's own list when it printed one, else the documented one, then
        // the ids the person typed, and "Custom model id…"
        for e in &entries {
            let Some(agent) = cfg.agent(&e.id) else { continue };
            if cur_agent == Some(e.id.as_str()) && cur_model == Some("") {
                chosen_at = Some(items.len());
            }
            items.push((e.provider.clone(), String::new()));
            rows.push(MRow::Head(e.id.clone()));
            for mi in cx.services.ext_status.models_for(agent.as_ref()) {
                if cur_agent == Some(e.id.as_str()) && cur_model == Some(mi.id.as_str()) {
                    chosen_at = Some(items.len());
                }
                items.push((e.provider.clone(), if mi.id.is_empty() { "default".to_string() } else { mi.id.clone() }));
                rows.push(MRow::Model(e.id.clone(), mi));
            }
            items.push((e.provider.clone(), "custom model id".to_string()));
            rows.push(MRow::Custom(e.id.clone()));
        }
        if let Some(c) = chosen_at {
            sel = c;
        } else if let Some(id) = cur_agent {
            sel = items.iter().position(|(p, m)| extagent::agent_of(p) == Some(id) && m.is_empty()).unwrap_or(builtin);
        }
    }
    if items.len() == offset {
        return None;
    }
    Some(ModelList { items, sel, rows, custom: None, title: title.to_string(), hint: hint.to_string(), current, top })
}

/// Remember a typed model id for the agent so it is a line of the list next time (administrators only: it is a system setting).
pub(crate) fn remember_custom(cx: &AppCx, agent: &str, model: &str) {
    if model.is_empty() || !cx.session.policy.manage_settings {
        return;
    }
    let (a, m) = (agent.to_string(), model.to_string());
    let _ = cx.services.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| {
        let list = match a.as_str() {
            extagent::CLAUDE_CODE => Some(&mut s.agents.claude.custom_models),
            extagent::COPILOT => Some(&mut s.agents.copilot.custom_models),
            other => other.strip_prefix("custom:").and_then(|c| s.agents.custom.iter_mut().find(|x| x.id == c)).map(|c| &mut c.custom_models),
        };
        if let Some(l) = list {
            if !l.contains(&m) && l.len() < 12 {
                l.push(m);
            }
        }
    });
}

/// Keys and clicks (`Event::Row`) of the list.
pub(crate) fn key(m: &mut ModelList, ev: &Event) -> Out {
    // typing a model id for an agent
    if let Some((agent, text)) = m.custom.as_mut() {
        match ev {
            Event::Key(Key::Esc) => m.custom = None,
            Event::Key(Key::Backspace) => {
                text.pop();
            }
            Event::Key(Key::Char(c)) => text.push(*c),
            Event::Paste(t) => text.push_str(t.trim()),
            Event::Key(Key::Enter) => {
                let (agent, id) = (agent.clone(), text.trim().to_string());
                if !extagent::valid_model_id(&id) {
                    return Out::Bad("A model id uses letters, digits and . _ - : / @ + [ ] only: no spaces, quotes or shell characters.".into());
                }
                return Out::Pick(Pick::AgentModel { agent, model: id, custom: true });
            }
            _ => {}
        }
        return Out::Stay;
    }
    let n = m.items.len();
    match ev {
        Event::Key(Key::Esc) => return Out::Close,
        Event::Key(Key::Up) => m.sel = m.sel.saturating_sub(1),
        Event::Key(Key::Down) => m.sel = (m.sel + 1).min(n - 1),
        Event::Key(Key::PageUp) => m.sel = m.sel.saturating_sub(8),
        Event::Key(Key::PageDown) => m.sel = (m.sel + 8).min(n - 1),
        Event::Key(Key::Home) => m.sel = 0,
        Event::Key(Key::End) => m.sel = n - 1,
        Event::Row(i) if *i < n => {
            m.sel = *i;
            return choose(m);
        }
        Event::Key(Key::Enter) => return choose(m),
        _ => {}
    }
    Out::Stay
}

/// Enter or a click on the highlighted line.
fn choose(m: &mut ModelList) -> Out {
    match m.rows.get(m.sel).cloned() {
        // stays open: the id is typed in the list itself
        Some(MRow::Custom(agent)) => {
            m.custom = Some((agent, String::new()));
            Out::Stay
        }
        Some(MRow::Head(agent)) => Out::Pick(Pick::Agent(agent)),
        Some(MRow::Model(agent, mi)) => Out::Pick(Pick::AgentModel { agent, model: mi.id, custom: false }),
        Some(MRow::Top) => Out::Pick(Pick::Top),
        Some(MRow::Plain) => m.items.get(m.sel).map_or(Out::Stay, |(p, mo)| Out::Pick(Pick::Provider(p.clone(), mo.clone()))),
        None => Out::Stay,
    }
}

/// The panel over `area`: title, hint, the lines (scrolled so the highlighted one shows) and the id being typed.
pub(crate) fn draw(g: &mut Gfx, area: R, s: f32, cx: &AppCx, m: &ModelList) {
    g.clear_hits();
    let (px, pw) = (area.x + 24.0 * s, (area.w - 48.0 * s).min(720.0 * s));
    let (py0, ph) = (area.y + 8.0 * s, area.h - 24.0 * s);
    g.rrect(px, py0, pw, ph, 16.0 * s, rgb(16, 20, 36));
    g.rrect_outline(px, py0, pw, ph, 16.0 * s, 1.5 * s, ACCENT);
    let title = g.fit(18.0 * s, &m.title, Face::Bold, pw - 40.0 * s);
    g.text(px + 20.0 * s, py0 + 14.0 * s, 18.0 * s, TEXT, &title, Face::Bold);
    let hint = g.fit(13.0 * s, &m.hint, Face::Sans, pw - 40.0 * s);
    g.text(px + 20.0 * s, py0 + 40.0 * s, 13.0 * s, MUTED, &hint, Face::Sans);
    let ext_rows = extagent::entries(&cx.services.agents_cfg(), &cx.services.ext_status);
    let (chosen_agent, chosen_model) = match &m.current.agent {
        Some((a, mo)) => (Some(a.as_str()), Some(mo.as_str())),
        None => (None, None),
    };
    // the typed id, if any, takes the bottom of the panel; the rows scroll above it
    let typing = m.custom.is_some();
    let (rh, top) = (34.0 * s, py0 + 70.0 * s);
    let foot = if typing { 52.0 * s } else { 0.0 };
    let rows = (((ph - 90.0 * s - foot) / rh) as usize).max(1);
    let first = m.sel.saturating_sub(rows - 1);
    for (n, (prov, model)) in m.items.iter().enumerate().skip(first).take(rows) {
        let y = top + (n - first) as f32 * rh;
        if n == m.sel {
            g.rrect(px + 12.0 * s, y, pw - 24.0 * s, rh - 4.0 * s, 10.0 * s, rgb(34, 44, 80));
        }
        g.hit(R::new(px + 12.0 * s, y, pw - 24.0 * s, rh - 4.0 * s), Event::Row(n), "model");
        match &m.rows[n] {
            MRow::Head(id) => {
                let Some(e) = ext_rows.iter().find(|e| e.id == *id) else { continue };
                // an external coding agent: a grey hint when it cannot answer yet
                let on = chosen_agent == Some(e.id.as_str());
                let color = if !e.ready { DIM } else if on { OK } else { TEXT };
                let signin = !e.ready && e.chip == "sign in";
                let hint = if e.ready { e.chip.clone() } else if signin { "Sign in first".to_string() } else { format!("{} · select for the fix", e.chip) };
                let hw = g.text_width(13.0 * s, &hint, Face::Sans);
                let btn = if signin { g.text_width(12.0 * s, "Login", Face::Bold) + 12.0 * s * 1.7 + 12.0 * s } else { 0.0 };
                let t = g.fit(15.0 * s, &e.label, Face::Bold, (pw - 90.0 * s - hw - btn).max(40.0 * s));
                g.text(px + 24.0 * s, y + 7.0 * s, 15.0 * s, color, &t, Face::Bold);
                g.text(px + pw - 24.0 * s - hw, y + 9.0 * s, 13.0 * s, if e.ready { OK } else if signin { crate::ui::ERR } else { DIM }, &hint, Face::Sans);
                // not signed in: the same Login button as everywhere else (the shell opens the dialog)
                if let (true, Some(i)) = (signin, crate::agent_login::AGENTS.iter().position(|a| *a == e.id)) {
                    g.button(px + pw - 24.0 * s - hw - btn, y + 1.0 * s, 12.0 * s, "Login", OK, Event::Tab(crate::agent_login::LOGIN_TAB + i));
                }
            }
            MRow::Model(id, mi) => {
                let on = chosen_agent == Some(id.as_str()) && chosen_model == Some(mi.id.as_str());
                let dim = mi.available == Some(false);
                let label = format!("› {}", mi.label);
                let note = if mi.note.is_empty() { mi.source.label().to_string() } else { format!("{} · {}", mi.note, mi.source.label()) };
                let note = if dim { format!("not available on your plan · {note}") } else { note };
                let nw = g.text_width(12.0 * s, &note, Face::Sans).min(pw * 0.5);
                let t = g.fit(14.0 * s, &label, if on { Face::Bold } else { Face::Sans }, (pw - 100.0 * s - nw).max(40.0 * s));
                g.text(px + 44.0 * s, y + 8.0 * s, 14.0 * s, if dim { DIM } else if on { OK } else { TEXT }, &t, if on { Face::Bold } else { Face::Sans });
                let note = g.fit(12.0 * s, &note, Face::Sans, pw * 0.5);
                let w = g.text_width(12.0 * s, &note, Face::Sans);
                g.text(px + pw - 24.0 * s - w, y + 10.0 * s, 12.0 * s, DIM, &note, Face::Sans);
            }
            MRow::Custom(_) => {
                g.text(px + 44.0 * s, y + 8.0 * s, 14.0 * s, MUTED, "› Custom model id…", Face::Sans);
            }
            MRow::Top => {
                let Some(t) = &m.top else { continue };
                let on = m.current.same;
                let nw = g.text_width(13.0 * s, &t.note, Face::Sans).min(pw * 0.45);
                let name = g.fit(15.0 * s, &t.label, if on { Face::Bold } else { Face::Sans }, (pw - 72.0 * s - nw).max(40.0 * s));
                g.text(px + 24.0 * s, y + 7.0 * s, 15.0 * s, if on { OK } else { TEXT }, &name, if on { Face::Bold } else { Face::Sans });
                let note = g.fit(13.0 * s, &t.note, Face::Sans, pw * 0.45);
                let w = g.text_width(13.0 * s, &note, Face::Sans);
                g.text(px + pw - 24.0 * s - w, y + 9.0 * s, 13.0 * s, DIM, &note, Face::Sans);
            }
            MRow::Plain => {
                let on = chosen_agent.is_none() && !m.current.same && m.current.plain.as_ref().is_some_and(|(p, mo)| p == prov && mo == model);
                let pwid = g.text_width(13.0 * s, prov, Face::Sans).min(pw * 0.3);
                let t = g.fit(15.0 * s, model, if on { Face::Bold } else { Face::Sans }, (pw - 72.0 * s - pwid).max(40.0 * s));
                g.text(px + 24.0 * s, y + 7.0 * s, 15.0 * s, if on { OK } else { TEXT }, &t, if on { Face::Bold } else { Face::Sans });
                let p = g.fit(13.0 * s, prov, Face::Sans, pw * 0.3);
                let w = g.text_width(13.0 * s, &p, Face::Sans);
                g.text(px + pw - 24.0 * s - w, y + 9.0 * s, 13.0 * s, DIM, &p, Face::Sans);
            }
        }
    }
    if let Some((agent, text)) = &m.custom {
        let y = py0 + ph - 46.0 * s;
        g.rrect(px + 12.0 * s, y, pw - 24.0 * s, 34.0 * s, 10.0 * s, rgb(10, 13, 26));
        g.rrect_outline(px + 12.0 * s, y, pw - 24.0 * s, 34.0 * s, 10.0 * s, 1.5 * s, ACCENT);
        let line = g.fit(14.0 * s, &format!("Model id for {agent}: {text}▏  (Enter uses it, Esc cancels)"), Face::Sans, pw - 48.0 * s);
        g.text(px + 24.0 * s, y + 9.0 * s, 14.0 * s, TEXT, &line, Face::Sans);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_backend;

    fn with_models(tag: &str, n: usize) -> (AppCx, std::path::PathBuf) {
        let (cx, d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
        let mut p = iyakki_agent::providers::ProviderCfg::new("nvidia_ollama", iyakki_agent::providers::Kind::Ollama);
        p.models = (0..n).map(|i| format!("model-{i:02}-with-a-fairly-long-name:latest")).collect();
        cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
        (cx, d)
    }

    fn plain(cx: &AppCx) -> Vec<(String, String)> {
        let st = cx.services.settings.lock().unwrap();
        st.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().map(move |m| (p.id.clone(), m.clone()))).collect()
    }

    fn draw_list(cx: &AppCx, m: &ModelList, w: u32, h: u32, s: f32) -> (Gfx, R) {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        draw(&mut g, area, s, cx, m);
        (g, area)
    }

    fn top() -> Option<TopRow> {
        Some(TopRow { label: "Same as chat".into(), note: "qwen3:8b · nvidia_ollama".into() })
    }

    #[test]
    fn the_list_has_the_top_line_the_providers_models_and_the_agents_groups_in_order() {
        let (cx, _d) = with_models("pickorder", 3);
        let m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "hint").unwrap();
        assert_eq!(m.rows[0], MRow::Top);
        assert!(m.rows[1..4].iter().all(|r| *r == MRow::Plain));
        let heads: Vec<usize> = m.rows.iter().enumerate().filter(|(_, r)| matches!(r, MRow::Head(_))).map(|(i, _)| i).collect();
        assert!(heads.len() >= 2 && heads[0] == 4, "{heads:?}");
        for h in &heads {
            // an agent's models follow it and "Custom model id…" closes its group
            let next_head = m.rows[h + 1..].iter().position(|r| matches!(r, MRow::Head(_))).map_or(m.rows.len(), |p| p + h + 1);
            assert!(matches!(m.rows[next_head - 1], MRow::Custom(_)), "group at {h}");
            assert!(m.rows[h + 1..next_head - 1].iter().all(|r| matches!(r, MRow::Model(..))));
        }
        assert_eq!(m.sel, 0, "same as chat is the one in use");
        // without the top line and without agents (the picture variant): only the models
        let p = build(&cx, plain(&cx), None, false, Current::default(), "t", "h").unwrap();
        assert_eq!(p.items.len(), 3);
        assert!(build(&cx, vec![], None, false, Current::default(), "t", "h").is_none(), "nothing to choose");
    }

    #[test]
    fn keys_clicks_and_the_custom_model_id_give_the_pick() {
        let (cx, _d) = with_models("pickkeys", 12);
        let mut m = build(&cx, plain(&cx), top(), true, Current { plain: Some(("nvidia_ollama".into(), "model-02-with-a-fairly-long-name:latest".into())), ..Default::default() }, "t", "h").unwrap();
        assert_eq!(m.sel, 3, "opens on the model in use (line 0 is the top line)");
        let k = |m: &mut ModelList, key: Key| super::key(m, &Event::Key(key));
        assert_eq!(k(&mut m, Key::Home), Out::Stay);
        assert_eq!(m.sel, 0);
        assert_eq!(k(&mut m, Key::Enter), Out::Pick(Pick::Top));
        k(&mut m, Key::PageDown);
        assert_eq!(m.sel, 8);
        k(&mut m, Key::PageUp);
        k(&mut m, Key::Up);
        assert_eq!(m.sel, 0);
        k(&mut m, Key::End);
        assert_eq!(m.sel, m.items.len() - 1);
        k(&mut m, Key::Down);
        assert_eq!(m.sel, m.items.len() - 1, "stops at the end");
        assert_eq!(super::key(&mut m, &Event::Row(2)), Out::Pick(Pick::Provider("nvidia_ollama".into(), "model-01-with-a-fairly-long-name:latest".into())));
        // an agent's header keeps its model; its model line picks both
        let head = m.rows.iter().position(|r| matches!(r, MRow::Head(a) if a == "claude-code")).unwrap();
        assert_eq!(super::key(&mut m, &Event::Row(head)), Out::Pick(Pick::Agent("claude-code".into())));
        assert_eq!(super::key(&mut m, &Event::Row(head + 1)), Out::Pick(Pick::AgentModel { agent: "claude-code".into(), model: String::new(), custom: false }));
        // the custom id is typed in the list: refused when it has shell characters, taken otherwise, Esc leaves the typing
        let custom = m.rows.iter().position(|r| matches!(r, MRow::Custom(a) if a == "claude-code")).unwrap();
        assert_eq!(super::key(&mut m, &Event::Row(custom)), Out::Stay);
        assert!(m.custom.is_some());
        assert_eq!(super::key(&mut m, &Event::Row(0)), Out::Stay, "clicks wait while an id is typed");
        for c in "a b".chars() {
            k(&mut m, Key::Char(c));
        }
        assert!(matches!(k(&mut m, Key::Enter), Out::Bad(e) if e.contains("letters, digits")));
        assert!(m.custom.is_some(), "stays open");
        k(&mut m, Key::Backspace);
        k(&mut m, Key::Backspace);
        k(&mut m, Key::Backspace);
        for c in "claude-opus-5-5".chars() {
            k(&mut m, Key::Char(c));
        }
        assert_eq!(k(&mut m, Key::Enter), Out::Pick(Pick::AgentModel { agent: "claude-code".into(), model: "claude-opus-5-5".into(), custom: true }));
        super::key(&mut m, &Event::Row(custom));
        assert_eq!(k(&mut m, Key::Esc), Out::Stay);
        assert!(m.custom.is_none());
        assert_eq!(k(&mut m, Key::Esc), Out::Close);
    }

    #[test]
    fn the_list_fits_scrolls_and_is_clickable_at_every_screen_size_and_text_scale() {
        let (cx, _d) = with_models("pickgeo", 40);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::shell::scale_for(w, h) * text_scale;
                let mut m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "Enter or click chooses · Esc closes · Settings → Models adds providers, Settings → Agents sets up Claude Code and others.").unwrap();
                let n = m.items.len();
                let (px, py0) = (24.0 * s, 46.0 * s + 8.0 * s);
                for typing in [false, true] {
                    if typing {
                        let c = m.rows.iter().position(|r| matches!(r, MRow::Custom(_))).unwrap();
                        super::key(&mut m, &Event::Row(c));
                        assert!(m.custom.is_some());
                    }
                    for step in [Key::Home, Key::Down, Key::PageDown, Key::End, Key::PageUp, Key::Up] {
                        if !typing {
                            super::key(&mut m, &Event::Key(step));
                        } else {
                            m.sel = [0, 1, 9, n - 1, 1, 0][m.sel % 6];
                        }
                        let (g, area) = draw_list(&cx, &m, w, h, s);
                        let rows: Vec<_> = g.hits.iter().filter(|x| x.label == "model").collect();
                        assert!(!rows.is_empty(), "{w}x{h} x{text_scale}");
                        assert!(rows.iter().any(|r| matches!(r.ev, Event::Row(i) if i == m.sel)), "the highlighted line is in view at {w}x{h} x{text_scale} (sel {})", m.sel);
                        let panel_bottom = py0 + (area.h - 24.0 * s);
                        for r in &rows {
                            let r = r.rect;
                            assert!(r.x >= px - 0.5 && r.x + r.w <= w as f32 + 0.5 && r.y >= area.y - 0.5 && r.y + r.h <= panel_bottom + 0.5, "line inside the panel at {w}x{h} x{text_scale}: {r:?}");
                            if typing {
                                assert!(r.y + r.h <= panel_bottom - 46.0 * s + 0.5, "lines stay above the id box at {w}x{h} x{text_scale}");
                            }
                        }
                        for p in rows.windows(2) {
                            assert!(p[0].rect.y + p[0].rect.h <= p[1].rect.y + 0.5, "no overlap");
                        }
                        // at 800x480 at least a few lines show at normal text size
                        if text_scale == 1.0 && !typing {
                            assert!(rows.len() >= 3, "{} lines at {w}x{h}", rows.len());
                        }
                    }
                    if typing {
                        super::key(&mut m, &Event::Key(Key::Esc));
                    }
                }
            }
        }
    }
}
