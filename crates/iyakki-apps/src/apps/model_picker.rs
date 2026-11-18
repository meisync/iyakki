//! The model chooser shared by the chat, the AI IDE's chat and TALES: the models of every enabled provider (provider name on
//! the right), then one group per external coding agent (`Claude Code (agent)  ready`) with its models indented under it and
//! `Custom model id…`. The owner of the screen keeps the list in a field, forwards events to [`key`], draws it with [`draw`] and
//! acts on the [`Pick`] that comes back.

use iyakki_extagentx::{self as extagent, ModelInfo};

use super::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key, DIM, MUTED, OK, R, TEXT};

/// The hit id of the scroll bar strip.
const BAR: usize = 0x6d70;
/// A click on a provider heading is `Event::Tab(HEAD_TAB + group)`.
const HEAD_TAB: usize = 1_450_000;
/// A click on the footer that hides or shows the models the person's key cannot use.
const HIDE_TAB: usize = 1_449_999;
/// More plain models than this and the groups start collapsed (all but the one in use).
const LONG: usize = 12;

/// One provider's models: a heading and the lines under it.
#[derive(Clone, Debug)]
pub(crate) struct PGroup {
    pub id: String,
    /// The provider as the person knows it ("NVIDIA NIM", or the name they gave it).
    pub label: String,
    pub zone: iyakki_shared::router_registry::Zone,
    /// The lines (indexes into `items`) of this provider, in the list's order.
    pub items: Vec<usize>,
    /// A prefix the model ids of this provider all carry (`nvidia/` under NVIDIA): left out of the shown name.
    pub own_prefix: String,
}

/// The provider as the person knows it: the well-known ones by their product name, others by the name they gave.
pub(crate) fn provider_label(cfg: &iyakki_shared::providers::ProviderCfg) -> String {
    match cfg.id.to_ascii_lowercase().as_str() {
        "nvidia" | "nvidia_nim" | "nim" => "NVIDIA NIM".into(),
        "openai" => "OpenAI".into(),
        "anthropic" | "claude" => "Anthropic Claude".into(),
        "gemini" | "google" => "Google Gemini".into(),
        "openrouter" => "OpenRouter".into(),
        "ollama" => "Ollama".into(),
        "lemonade" => "Lemonade".into(),
        _ => cfg.id.clone(),
    }
}

fn chip(z: iyakki_shared::router_registry::Zone) -> &'static str {
    use iyakki_shared::router_registry::Zone;
    match z {
        Zone::Computer => "local",
        Zone::Network => "network",
        Zone::Internet => "internet",
    }
}

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
    /// The line indexes (into `items`) of the recently used models that are still configured, most recent first.
    pub recent: Vec<usize>,
    /// The providers' groups in the order they are drawn: this computer, the local network, then the internet (the provider used
    /// most recently first, else as configured).
    pub groups: Vec<PGroup>,
    /// Which groups are folded away (same order as `groups`).
    pub collapsed: Vec<bool>,
    /// The lines whose model the person's key cannot use (learned from real calls, `iyakki_shared::modelavail`): hidden unless
    /// `show_hidden`.
    pub unusable: Vec<usize>,
    pub show_hidden: bool,
    /// A group heading is the highlighted line (then `sel` is not).
    pub head: Option<usize>,
    /// What the person typed to filter the lines (empty = all).
    pub query: String,
    /// The scroll state of the lines (wheel, bar, thumb drag).
    pub scroll: ScrollList,
    /// Scroll to show the highlighted line at the next draw (a key moved it, or the list just opened).
    follow: bool,
    /// The highlighted line the last draw saw: another one means a key (or a test) moved it.
    seen: usize,
    /// "This machine: fits: 7B Q4, 13B Q4. Add one in Settings, Models." (docs/AI-HARDWARE.md): what local models would suit this machine.
    /// A hint only: no model that is configured is ever left out of the list because of it.
    pub fits: Option<String>,
}

/// The hint line under the chooser's own hint: the AI hardware profile's `fits` line, and what to do about it.
pub(crate) fn fits_hint(p: &dyn iyakki_agent_api::AiMachine) -> String {
    if p.capable() || p.unknown() {
        format!("This machine: {}. Add one in Settings, Models.", p.fits_line())
    } else {
        format!("This machine: {}; the models above run on another computer or in the cloud.", p.fits_line())
    }
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
    // a list stored by an earlier run (before the chat-model filter) is filtered when it is read: NVIDIA's catalog holds detectors,
    // translators and vision-only models that cannot chat
    let plain: Vec<(String, String)> = {
        let st = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let nvidia: Vec<&str> = st.get().providers.iter().filter(|p| p.kind == iyakki_shared::providers::Kind::Nvidia).map(|p| p.id.as_str()).collect();
        plain.into_iter().filter(|(p, m)| !nvidia.contains(&p.as_str()) || iyakki_shared::modelavail::is_chat_model(m)).collect()
    };
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
        let cfg = cx.agents_cfg();
        let entries = cx.extagent_facet().entries();
        if entries.iter().any(|e| cx.ext_status().get(&e.id).is_none()) {
            cx.ext_status().refresh(cfg.clone());
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
            for mi in cx.ext_status().models_for(agent.as_ref()) {
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
    let fits = Some(fits_hint(cx.agent.ai_profile().machine(cx.data_dir()).as_ref()));
    // the person's recently used models that are still configured (and offered here), most recent first
    let unusable: Vec<usize> = {
        let mut ids: Vec<&String> = items[offset..builtin].iter().map(|(p, _)| p).collect();
        ids.dedup();
        let mut bad: Vec<(String, String)> = Vec::new();
        for pid in ids.into_iter().collect::<std::collections::BTreeSet<_>>() {
            bad.extend(cx.ai().unavailable_models(pid).into_iter().map(|m| (pid.clone(), m)));
        }
        (offset..builtin).filter(|&i| bad.contains(&items[i])).collect()
    };
    let (groups, collapsed) = {
        let st = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let cfgs = &st.get().providers;
        let used = crate::uisize::load_recent_models(cx.data_dir(), &cx.session.user);
        let mut groups: Vec<PGroup> = Vec::new();
        for i in offset..builtin {
            let pid = &items[i].0;
            if let Some(g) = groups.iter_mut().find(|g| g.id == *pid) {
                g.items.push(i);
                continue;
            }
            let cfg = cfgs.iter().find(|c| c.id == *pid);
            let zone = cfg.map_or(iyakki_shared::router_registry::Zone::Internet, |c| cx.agent.helpers_b6().router_zone(c));
            let label = cfg.map_or_else(|| pid.clone(), provider_label);
            let own_prefix = if cfg.is_some_and(|c| c.kind == iyakki_shared::providers::Kind::Nvidia) { "nvidia/".to_string() } else { format!("{}/", pid.to_ascii_lowercase()) };
            groups.push(PGroup { id: pid.clone(), label, zone, items: vec![i], own_prefix });
        }
        // this computer, the local network, the internet; inside a zone the provider used last comes first, then as configured
        let rank = |id: &str| used.iter().position(|(p, _)| p == id).unwrap_or(usize::MAX);
        groups.sort_by_key(|g| (g.zone, rank(&g.id)));
        let long = builtin - offset > LONG;
        let open = groups.iter().position(|g| g.items.contains(&sel)).unwrap_or(0);
        let collapsed = (0..groups.len()).map(|g| long && g != open).collect();
        (groups, collapsed)
    };
    let recent: Vec<usize> = crate::uisize::load_recent_models(cx.data_dir(), &cx.session.user).iter().filter_map(|(p, m)| (offset..builtin).find(|&i| items[i].0 == *p && items[i].1 == *m)).collect();
    Some(ModelList { items, sel, rows, custom: None, title: title.to_string(), hint: hint.to_string(), current, top, recent, groups, collapsed, unusable, show_hidden: false, head: None, query: String::new(), scroll: ScrollList::default(), follow: true, seen: usize::MAX, fits })
}

/// Remember a typed model id for the agent so it is a line of the list next time (administrators only: it is a system setting).
pub(crate) fn remember_custom(cx: &AppCx, agent: &str, model: &str) {
    if model.is_empty() || !cx.session.policy.manage_settings {
        return;
    }
    let (a, m) = (agent.to_string(), model.to_string());
    let _ = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| {
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
    let navs = navs(m);
    let here = m.head.map_or(Nav::Item(m.sel), Nav::Head);
    let at = navs.iter().position(|n| *n == here);
    let last = navs.len().saturating_sub(1);
    let moved = |m: &mut ModelList, to: usize| {
        match navs.get(to.min(last)) {
            Some(Nav::Item(i)) => {
                m.sel = *i;
                m.head = None;
            }
            Some(Nav::Head(g)) => m.head = Some(*g),
            None => return,
        }
        m.follow = true;
    };
    match ev {
        Event::Key(Key::Esc) if !m.query.is_empty() => retype(m, |q| q.clear()),
        Event::Key(Key::Esc | Key::Ctrl('p')) => return Out::Close,
        Event::Key(Key::Up) => moved(m, at.unwrap_or(0).saturating_sub(1)),
        Event::Key(Key::Down) => moved(m, at.map_or(0, |a| a + 1)),
        Event::Key(Key::PageUp) => moved(m, at.unwrap_or(0).saturating_sub(8)),
        Event::Key(Key::PageDown) => moved(m, at.unwrap_or(0) + 8),
        Event::Key(Key::Home) => moved(m, 0),
        Event::Key(Key::End) => moved(m, last),
        // type to search
        Event::Key(Key::Char(c)) if !c.is_control() => retype(m, |q| q.push(*c)),
        Event::Paste(t) => retype(m, |q| q.push_str(&t.replace(['\n', '\r'], " "))),
        Event::Key(Key::Backspace) => retype(m, |q| {
            q.pop();
        }),
        Event::Row(i) if *i < n => {
            m.sel = *i;
            return choose(m);
        }
        // fold and unfold a provider's group
        Event::Key(Key::Left) if m.query.is_empty() => {
            if let Some(g) = m.head.or_else(|| group_of(m, m.sel)) {
                fold(m, g, true);
            }
        }
        Event::Key(Key::Right) if m.query.is_empty() => {
            if let Some(g) = m.head {
                fold(m, g, false);
            }
        }
        Event::Key(Key::Enter) if m.head.is_some() => {
            if let Some(g) = m.head {
                fold(m, g, !m.collapsed[g]);
            }
        }
        Event::Tab(HIDE_TAB) | Event::Key(Key::Ctrl('u')) if !m.unusable.is_empty() => {
            m.show_hidden = !m.show_hidden;
            if let Some(&i) = visible(m).iter().find(|&&i| i == m.sel).or(visible(m).first()) {
                m.sel = i;
            }
        }
        Event::Tab(t) if *t >= HEAD_TAB && *t - HEAD_TAB < m.groups.len() => {
            let g = *t - HEAD_TAB;
            fold(m, g, !m.collapsed[g]);
            m.head = Some(g);
        }
        Event::Key(Key::Enter) if at.is_some() => return choose(m),
        // the wheel, the scroll bar and its thumb
        Event::Scroll(d) => {
            m.scroll.wheel(*d);
        }
        Event::AreaAt(id, _, y) if *id == BAR => {
            let (ty, th) = m.scroll.thumb();
            let abs = m.scroll.track.y + *y;
            if abs >= ty && abs <= ty + th {
                m.scroll.bar_down(*y);
            } else {
                // a click on the track pages towards it
                let page = (m.scroll.view.h - m.scroll.row).max(m.scroll.row);
                m.scroll.by(if abs < ty { -page } else { page });
            }
        }
        Event::MouseMove(..) | Event::MouseUp(..) => {
            m.scroll.event(ev, BAR);
        }
        _ => {}
    }
    Out::Stay
}

/// A line the highlight can stand on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Nav {
    Item(usize),
    Head(usize),
}

fn navs(m: &ModelList) -> Vec<Nav> {
    lines(m)
        .into_iter()
        .filter_map(|l| match l {
            Line::Item(i) => Some(Nav::Item(i)),
            Line::Group(g) => Some(Nav::Head(g)),
            Line::Label(_) => None,
        })
        .collect()
}

fn group_of(m: &ModelList, i: usize) -> Option<usize> {
    m.groups.iter().position(|g| g.items.contains(&i))
}

/// Fold or unfold a group. A highlighted line inside a group that closes moves to its heading.
fn fold(m: &mut ModelList, g: usize, closed: bool) {
    m.collapsed[g] = closed;
    if closed && m.head.is_none() && m.groups[g].items.contains(&m.sel) && !m.recent.contains(&m.sel) {
        m.head = Some(g);
    }
    m.follow = true;
}

/// The text of a line that the search looks in.
fn haystack(m: &ModelList, i: usize) -> String {
    let (p, mo) = &m.items[i];
    match &m.rows[i] {
        MRow::Plain => format!("{p} {mo}"),
        MRow::Top => m.top.as_ref().map(|t| format!("{} {}", t.label, t.note)).unwrap_or_default(),
        MRow::Head(id) => format!("{p} {id}"),
        MRow::Model(id, mi) => format!("{id} {} {}", mi.label, mi.id),
        MRow::Custom(_) => String::new(),
    }
}

/// Whether line `i` passes the search (every typed word is somewhere in it, any case).
fn matches(m: &ModelList, i: usize) -> bool {
    if !m.show_hidden && m.unusable.contains(&i) {
        return false;
    }
    if m.query.trim().is_empty() {
        return true;
    }
    let hay = haystack(m, i).to_lowercase();
    !matches!(m.rows[i], MRow::Custom(_)) && m.query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// Change the search text, then the highlighted line is the first one that still shows and the list starts at the top.
fn retype(m: &mut ModelList, f: impl FnOnce(&mut String)) {
    f(&mut m.query);
    m.scroll.to_top();
    m.follow = true;
    m.head = None;
    if let Some(&i) = visible(m).first() {
        m.sel = i;
    }
}

/// One line as drawn: a label, a provider's heading, or a line of the list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Line {
    Label(&'static str),
    Group(usize),
    Item(usize),
}

/// What is drawn, top to bottom: the extra first line, the "Recent" group, the providers' groups (this computer, the network, the
/// internet; each with a heading, its models under it unless folded) and then the external agents as the list gives them. A
/// search drops the headings and keeps the order; what is left of the groups follows under "All models".
pub(crate) fn lines(m: &ModelList) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let flat = !m.query.trim().is_empty();
    out.extend((0..m.items.len()).filter(|&i| m.rows[i] == MRow::Top && matches(m, i)).map(Line::Item));
    let recent: Vec<usize> = m.recent.iter().copied().filter(|&i| i < m.items.len() && matches(m, i)).collect();
    if !recent.is_empty() {
        out.push(Line::Label("Recent"));
        out.extend(recent.iter().map(|&i| Line::Item(i)));
    }
    let mut labelled = recent.is_empty() || !flat;
    for (gi, g) in m.groups.iter().enumerate() {
        let rest: Vec<usize> = g.items.iter().copied().filter(|i| !m.recent.contains(i) && matches(m, *i)).collect();
        if flat {
            if !rest.is_empty() && !labelled {
                out.push(Line::Label("All models"));
                labelled = true;
            }
            out.extend(rest.into_iter().map(Line::Item));
        } else {
            out.push(Line::Group(gi));
            if !m.collapsed[gi] {
                out.extend(rest.into_iter().map(Line::Item));
            }
        }
    }
    out.extend((0..m.items.len()).filter(|&i| !matches!(m.rows[i], MRow::Top | MRow::Plain) && matches(m, i)).map(Line::Item));
    out
}

/// The lines that can be chosen, in the order they are drawn.
pub(crate) fn visible(m: &ModelList) -> Vec<usize> {
    lines(m).into_iter().filter_map(|l| if let Line::Item(i) = l { Some(i) } else { None }).collect()
}

/// Enter or a click on the highlighted line.
fn choose(m: &mut ModelList) -> Out {
    if !visible(m).contains(&m.sel) {
        return Out::Stay;
    }
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

/// The name of a model as drawn: without the provider's own prefix (`nvidia/` under NVIDIA).
fn shown(m: &ModelList, i: usize) -> String {
    let model = &m.items[i].1;
    group_of(m, i).and_then(|g| model.strip_prefix(m.groups[g].own_prefix.as_str())).filter(|r| !r.is_empty()).map_or_else(|| model.clone(), str::to_string)
}

/// The panel over `area`: title, hint, the lines (scrolled so the highlighted one shows) and the id being typed.
pub(crate) fn draw(g: &mut Gfx, area: R, s: f32, cx: &AppCx, m: &mut ModelList) {
    g.clear_hits();
    let (px, pw) = (area.x + 24.0 * s, (area.w - 48.0 * s).min(720.0 * s));
    let (py0, ph) = (area.y + 8.0 * s, area.h - 24.0 * s);
    g.rrect(px, py0, pw, ph, 16.0 * s, rgb(16, 20, 36));
    g.rrect_outline(px, py0, pw, ph, 16.0 * s, 1.5 * s, ACCENT);
    let title = g.fit(18.0 * s, &m.title, Face::Bold, pw - 40.0 * s);
    g.text(px + 20.0 * s, py0 + 14.0 * s, 18.0 * s, TEXT, &title, Face::Bold);
    // what is typed to search replaces the hint line
    let hint_text = if m.query.is_empty() { m.hint.clone() } else { format!("Search models: {}▏  (Esc clears)", m.query) };
    let hint = g.fit(13.0 * s, &hint_text, Face::Sans, pw - 40.0 * s);
    g.text(px + 20.0 * s, py0 + 40.0 * s, 13.0 * s, if m.query.is_empty() { MUTED } else { TEXT }, &hint, Face::Sans);
    let ext_rows = cx.extagent_facet().entries();
    let (chosen_agent, chosen_model) = match &m.current.agent {
        Some((a, mo)) => (Some(a.as_str()), Some(mo.as_str())),
        None => (None, None),
    };
    // the typed id, if any, takes the bottom of the panel; the rows scroll above it
    let typing = m.custom.is_some();
    let (rh, top) = (34.0 * s, py0 + 78.0 * s);
    let note = !m.unusable.is_empty();
    let foot = if typing { 52.0 * s } else { 0.0 } + if note { 26.0 * s } else { 0.0 };
    let rows = (((ph - 98.0 * s - foot) / rh) as usize).max(1);
    // the full id of a model whose shown name is shorter, else the hardware hint
    let full = if m.head.is_none() && m.rows.get(m.sel) == Some(&MRow::Plain) && shown(m, m.sel) != m.items[m.sel].1 { Some(format!("Model id: {} · {}", m.items[m.sel].1, m.items[m.sel].0)) } else { None };
    if let Some(f) = full.as_ref().or(m.fits.as_ref()) {
        let f = g.fit(12.0 * s, f, Face::Sans, pw - 40.0 * s);
        g.text(px + 20.0 * s, py0 + 58.0 * s, 12.0 * s, crate::ui::DIM, &f, Face::Sans);
    }
    // the lines (headings and models) scroll in a window of whole lines; the bar shows where the window is
    let all = lines(m);
    let extra = if all.len() > rows { 16.0 * s } else { 0.0 };
    m.scroll.layout(R::new(px, top, pw, rows as f32 * rh), all.len() as f32 * rh, rh, s);
    if m.follow || m.seen != m.sel {
        let want = m.head.map_or(Line::Item(m.sel), Line::Group);
        if let Some(l) = all.iter().position(|l| *l == want) {
            // a "Recent" or "All models" label right above the line stays in view with it
            let mut l0 = l;
            while l0 > 0 && matches!(all[l0 - 1], Line::Label(_)) {
                l0 -= 1;
            }
            m.scroll.show(l0 as f32 * rh, (l + 1) as f32 * rh);
        }
        m.follow = false;
        m.seen = m.sel;
    }
    let first = ((m.scroll.offset / rh).round() as usize).min(all.len().saturating_sub(rows));
    if all.is_empty() {
        let none = g.fit(14.0 * s, &format!("No model matches \"{}\"", m.query), Face::Sans, pw - 48.0 * s);
        g.text(px + 24.0 * s, top + 6.0 * s, 14.0 * s, DIM, &none, Face::Sans);
    }
    for (li, line) in all.iter().enumerate().skip(first).take(rows) {
        let y = top + (li - first) as f32 * rh;
        let n = match line {
            Line::Label(t) => {
                g.text(px + 24.0 * s, y + 12.0 * s, 12.0 * s, DIM, &t.to_uppercase(), Face::Bold);
                continue;
            }
            Line::Group(gi) => {
                let grp = &m.groups[*gi];
                let pw = pw - extra;
                if m.head == Some(*gi) {
                    g.rrect(px + 12.0 * s, y, pw - 24.0 * s, rh - 4.0 * s, 10.0 * s, rgb(34, 44, 80));
                }
                g.hit(R::new(px + 12.0 * s, y, pw - 24.0 * s, rh - 4.0 * s), Event::Tab(HEAD_TAB + *gi), "provider");
                let count = format!("{}", grp.items.iter().filter(|i| m.show_hidden || !m.unusable.contains(i)).count());
                let cw = g.text_width(12.0 * s, &count, Face::Sans);
                let zone = chip(grp.zone);
                let zw = g.text_width(11.0 * s, zone, Face::Sans) + 12.0 * s;
                g.text(px + 24.0 * s, y + 8.0 * s, 14.0 * s, MUTED, if m.collapsed[*gi] { "▸" } else { "▾" }, Face::Bold);
                let name = g.fit(15.0 * s, &grp.label, Face::Bold, (pw - 100.0 * s - zw - cw).max(40.0 * s));
                let nw = g.text_width(15.0 * s, &name, Face::Bold);
                g.text(px + 44.0 * s, y + 7.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
                let cx0 = px + 44.0 * s + nw + 10.0 * s;
                g.rrect(cx0, y + 7.0 * s, zw, 18.0 * s, 9.0 * s, rgb(26, 33, 60));
                g.text(cx0 + 6.0 * s, y + 9.0 * s, 11.0 * s, MUTED, zone, Face::Sans);
                g.text(px + pw - 24.0 * s - cw, y + 9.0 * s, 12.0 * s, DIM, &count, Face::Sans);
                continue;
            }
            Line::Item(n) => *n,
        };
        let prov = &m.items[n].0;
        let pw = pw - extra;
        // under a heading the provider is known; in Recent and in a search it is shown on the right, so equal names can be told apart
        let flat = !m.query.is_empty() || m.recent.contains(&n);
        let indent = if flat || !matches!(m.rows[n], MRow::Plain) { 0.0 } else { 20.0 * s };
        let model_shown = shown(m, n);
        let (model, prov): (&String, String) = (&model_shown, if flat { group_of(m, n).map_or_else(|| prov.clone(), |g| m.groups[g].label.clone()) } else { String::new() });
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
                let (rp, rm) = &m.items[n];
                let on = chosen_agent.is_none() && !m.current.same && m.current.plain.as_ref().is_some_and(|(p, mo)| p == rp && mo == rm);
                let pwid = g.text_width(13.0 * s, &prov, Face::Sans).min(pw * 0.3);
                let label = if on { format!("✓ {model}") } else { model.clone() };
                let t = g.fit(15.0 * s, &label, if on { Face::Bold } else { Face::Sans }, (pw - 72.0 * s - pwid - indent).max(40.0 * s));
                g.text(px + 24.0 * s + indent, y + 7.0 * s, 15.0 * s, if on { OK } else { TEXT }, &t, if on { Face::Bold } else { Face::Sans });
                let p = g.fit(13.0 * s, &prov, Face::Sans, pw * 0.3);
                let w = g.text_width(13.0 * s, &p, Face::Sans);
                g.text(px + pw - 24.0 * s - w, y + 9.0 * s, 13.0 * s, DIM, &p, Face::Sans);
            }
        }
    }
    m.scroll.draw_bar(g, s, BAR, ACCENT);
    if note {
        let n = m.unusable.len();
        let t = if m.show_hidden { format!("Showing {n} models your key cannot use (hide)") } else { format!("{n} models are hidden because your key cannot use them (show)") };
        let t = g.fit(12.0 * s, &t, Face::Sans, pw - 40.0 * s);
        let y = py0 + ph - 24.0 * s - if typing { 46.0 * s } else { 0.0 };
        g.text(px + 20.0 * s, y, 12.0 * s, MUTED, &t, Face::Sans);
        g.hit(R::new(px + 12.0 * s, y - 4.0 * s, pw - 24.0 * s, 24.0 * s), Event::Tab(HIDE_TAB), "hidden models");
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
        let (cx, d) = cx_with_backend(tag, iyakki_shared::chat::Unconfigured);
        let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
        p.models = (0..n).map(|i| format!("model-{i:02}-with-a-fairly-long-name:latest")).collect();
        cx.settings().lock().unwrap().upsert_provider(p).unwrap();
        (cx, d)
    }

    fn plain(cx: &AppCx) -> Vec<(String, String)> {
        let st = cx.settings().lock().unwrap();
        st.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().map(move |m| (p.id.clone(), m.clone()))).collect()
    }

    fn draw_list(cx: &AppCx, m: &mut ModelList, w: u32, h: u32, s: f32) -> (Gfx, R) {
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
        assert_eq!(m.sel, 7, "eight lines down: the top line, the provider heading and six models");
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
                let s = crate::ui::scale_for(w, h) * text_scale;
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
                        let (g, area) = draw_list(&cx, &mut m, w, h, s);
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
    fn fake_profile(tag: &str, ram_gb: u64, card: Option<(&str, &str)>) -> iyakki_agent::aiprofile::AiProfile {
        let r = std::env::temp_dir().join(format!("iyakki-mp-{}-{:?}-{tag}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("proc")).unwrap();
        std::fs::create_dir_all(r.join("sys/class/dmi/id")).unwrap();
        std::fs::write(r.join("proc/meminfo"), format!("MemTotal: {} kB\n", ram_gb * 1024 * 1024)).unwrap();
        std::fs::write(r.join("sys/class/dmi/id/product_name"), "PC").unwrap();
        if let Some((v, d)) = card {
            let c = r.join("sys/bus/pci/devices/0000:01:00.0");
            std::fs::create_dir_all(&c).unwrap();
            for (f, t) in [("class", "0x030000"), ("vendor", v), ("device", d)] {
                std::fs::write(c.join(f), t).unwrap();
            }
        }
        let p = iyakki_agent::aiprofile::detect(&r);
        let _ = std::fs::remove_dir_all(&r);
        p
    }

    #[test]
    fn the_fits_hint_says_what_local_models_suit_this_machine_in_the_profiles_words() {
        let gpu = fake_profile("rtx", 32, Some(("0x10de", "0x2503")));
        assert_eq!(fits_hint(&gpu), format!("This machine: {}. Add one in Settings, Models.", gpu.fits_line()));
        assert!(fits_hint(&gpu).contains("7B Q4, 13B Q4"), "{}", fits_hint(&gpu));
        let cpu = fake_profile("cpu", 4, None);
        let h = fits_hint(&cpu);
        assert!(h.starts_with("This machine: fits: no model of useful size") && h.contains("run on another computer or in the cloud"), "{h}");
    }

    #[test]
    fn the_picker_carries_the_hint_hides_nothing_and_draws_it_at_every_size_inside_the_panel() {
        let (cx, _d) = with_models("pickfits", 6);
        let mut m = build(&cx, plain(&cx), top(), true, Current { same: true, ..Default::default() }, "Choose the model", "hint").unwrap();
        assert!(m.fits.as_ref().is_some_and(|f| f.starts_with("This machine: ")), "{:?}", m.fits);
        // every configured model is still in the list, whatever the hint says (a 6-model provider and the top line)
        assert_eq!(m.items.iter().zip(&m.rows).filter(|(_, r)| **r == MRow::Plain).count(), 6);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
            let s = crate::ui::scale_for(w, h);
            m.fits = None;
            let (g0, _) = draw_list(&cx, &mut m, w, h, s);
            m.fits = Some(fits_hint(&fake_profile("sz", 32, Some(("0x10de", "0x2503")))));
            let (g1, area) = draw_list(&cx, &mut m, w, h, s);
            assert_ne!(g0.rgba(), g1.rgba(), "{w}x{h}: the hint is drawn");
            let panel_bottom = area.y + 8.0 * s + (area.h - 24.0 * s);
            for r in g1.hits.iter().filter(|x| x.label == "model") {
                assert!(r.rect.y >= area.y + 8.0 * s + 78.0 * s - 0.5 && r.rect.y + r.rect.h <= panel_bottom + 0.5, "{w}x{h}: {:?}", r.rect);
            }
            assert!(g1.hits.iter().any(|x| x.label == "model"), "{w}x{h}: lines remain");
        }
    }

    fn name(i: usize) -> String {
        format!("model-{i:02}-with-a-fairly-long-name:latest")
    }

    fn note(cx: &AppCx, i: usize) {
        crate::uisize::note_recent_model(cx.data_dir(), &cx.session.user, "nvidia_ollama", &name(i));
    }

    fn open(cx: &AppCx) -> ModelList {
        build(cx, plain(cx), None, true, Current::default(), "Choose the model", "hint").unwrap()
    }

    /// The model lines drawn, as (line index, rect), in order.
    fn drawn(g: &Gfx) -> Vec<(usize, R)> {
        g.hits.iter().filter(|x| x.label == "model").filter_map(|x| if let Event::Row(i) = x.ev { Some((i, x.rect)) } else { None }).collect()
    }

    #[test]
    fn the_wheel_scrolls_the_list_three_lines_a_notch_and_stops_at_both_ends() {
        let (cx, _d) = with_models("pickwheel", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0);
        assert!(m.scroll.scrolls(), "60 models do not fit");
        // the wheel down (negative) moves the list, not the highlighted line
        assert_eq!(super::key(&mut m, &Event::Scroll(-1)), Out::Stay);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 2, "three lines down: the heading and two models are gone");
        assert_eq!(m.sel, 0);
        super::key(&mut m, &Event::Scroll(1));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0);
        super::key(&mut m, &Event::Scroll(1));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g)[0].0, 0, "stops at the top");
        for _ in 0..100 {
            super::key(&mut m, &Event::Scroll(-1));
        }
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        let rows = drawn(&g);
        assert_eq!(rows.last().unwrap().0, m.items.len() - 1, "the last line shows at the bottom");
        super::key(&mut m, &Event::Scroll(-1));
        let (g2, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g2)[0].0, rows[0].0, "stops at the bottom");
        // a pointer outside the list leaves the wheel alone
        m.scroll.pointer = Some((1, 1));
        let before = m.scroll.offset;
        super::key(&mut m, &Event::Scroll(1));
        assert_eq!(m.scroll.offset, before);
    }

    #[test]
    fn the_scroll_bar_thumb_follows_the_window_and_dragging_it_moves_the_window() {
        let (cx, _d) = with_models("pickbar", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert!(g.hits.iter().any(|h| h.label == "scroll bar"), "the strip is a hit region");
        let (ty0, th0) = m.scroll.thumb();
        let n = lines(&m).len() as f32;
        let rows = (m.scroll.view.h / m.scroll.row).round();
        assert!((th0 - m.scroll.track.h * rows / n).abs() < 2.0 || th0 >= 24.0, "thumb size follows the share shown");
        assert!(th0 < m.scroll.track.h && (ty0 - m.scroll.track.y).abs() < 0.5, "the thumb starts at the top");
        super::key(&mut m, &Event::Scroll(-2));
        draw_list(&cx, &mut m, 1280, 720, s);
        assert!(m.scroll.thumb().0 > ty0, "the thumb moved down with the wheel");
        // grab the thumb and drag it to the bottom of the track
        let (ty, th) = m.scroll.thumb();
        let ymid = ty + th / 2.0 - m.scroll.track.y;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, ymid));
        assert!(m.scroll.dragging());
        let bottom = (m.scroll.track.y + m.scroll.track.h) as i32 + 50;
        super::key(&mut m, &Event::MouseMove(0, bottom));
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(drawn(&g).last().unwrap().0, m.items.len() - 1, "dragged to the end");
        super::key(&mut m, &Event::MouseUp(0, 0));
        assert!(!m.scroll.dragging());
        // a click on the track above the thumb pages up by a screen less a line
        let before = m.scroll.offset;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, 2.0));
        assert!(m.scroll.offset < before && before - m.scroll.offset >= m.scroll.row, "paged up");
        let mid = m.scroll.offset;
        let low = m.scroll.track.h - 1.0;
        super::key(&mut m, &Event::AreaAt(BAR, 3.0, low));
        assert!(m.scroll.offset > mid, "paged down");
        // when everything fits there is no bar
        let (cx2, _d2) = with_models("pickbar2", 2);
        let mut small = open(&cx2);
        let (g, _) = draw_list(&cx2, &mut small, 1280, 720, s);
        assert!(!g.hits.iter().any(|h| h.label == "scroll bar"));
    }

    #[test]
    fn keys_keep_the_highlighted_line_in_view_in_a_long_list() {
        let (cx, _d) = with_models("pickkeysview", 60);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(800, 480);
        for k in [Key::End, Key::PageUp, Key::Up, Key::Home, Key::PageDown, Key::PageDown, Key::Down, Key::Down] {
            super::key(&mut m, &Event::Key(k));
            let (g, _) = draw_list(&cx, &mut m, 800, 480, s);
            assert!(m.head.is_some() || drawn(&g).iter().any(|(i, _)| *i == m.sel), "line {} in view", m.sel);
            assert!(m.head.is_none() || g.hits.iter().any(|h| h.label == "provider"), "the heading is in view");
        }
        // the wheel moved it away, a key brings it back
        for _ in 0..10 {
            super::key(&mut m, &Event::Scroll(1));
        }
        draw_list(&cx, &mut m, 800, 480, s);
        super::key(&mut m, &Event::Key(Key::Down));
        let (g, _) = draw_list(&cx, &mut m, 800, 480, s);
        assert!(drawn(&g).iter().any(|(i, _)| *i == m.sel));
    }

    #[test]
    fn recent_models_come_first_five_at_most_most_recent_first_with_headings_and_without_repeats() {
        let (cx, _d) = with_models("pickrecent", 12);
        assert!(open(&cx).recent.is_empty());
        let m = open(&cx);
        assert!(lines(&m).iter().all(|l| !matches!(l, Line::Label(_))), "no recents: no headings");
        for i in [1, 2, 3, 4, 5, 6, 2] {
            note(&cx, i);
        }
        let m = open(&cx);
        let order: Vec<usize> = m.recent.iter().map(|&i| i).collect();
        let names: Vec<&str> = order.iter().map(|&i| m.items[i].1.as_str()).collect();
        assert_eq!(names, [name(2), name(6), name(5), name(4), name(3)], "last 5 distinct, newest first");
        let l = lines(&m);
        assert_eq!(l[0], Line::Label("Recent"));
        assert!(l[1..6].iter().all(|x| matches!(x, Line::Item(i) if m.recent.contains(i))));
        assert_eq!(l[6], Line::Group(0), "the provider's heading follows the Recent group");
        // the rest keep their own order and the recents are not repeated there
        let rest: Vec<usize> = l[7..].iter().filter_map(|x| if let Line::Item(i) = x { Some(*i) } else { None }).collect();
        assert_eq!(rest.iter().filter(|i| m.rows[**i] == MRow::Plain).count(), 12 - 5);
        assert!(rest.windows(2).all(|w| w[0] < w[1]));
        // keys walk the drawn order: from the first recent down to the second
        let mut m = m;
        m.sel = m.recent[0];
        super::key(&mut m, &Event::Key(Key::Down));
        assert_eq!(m.sel, m.recent[1]);
        super::key(&mut m, &Event::Key(Key::Home));
        assert_eq!(m.sel, m.recent[0], "Home is the first drawn line");
        // a model that is no longer configured is skipped
        cx.settings().lock().unwrap().upsert_provider({
            let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
            p.models = (0..12).filter(|i| *i != 6).map(name).collect();
            p
        }).unwrap();
        let m = open(&cx);
        let names: Vec<&str> = m.recent.iter().map(|&i| m.items[i].1.as_str()).collect();
        assert_eq!(names, [name(2), name(5), name(4), name(3)]);
        // and a provider that is turned off
        let mut p = iyakki_shared::providers::ProviderCfg::new("nvidia_ollama", iyakki_shared::providers::Kind::Ollama);
        p.models = (0..12).map(name).collect();
        p.enabled = false;
        cx.settings().lock().unwrap().upsert_provider(p).unwrap();
        assert!(build(&cx, plain(&cx), None, false, Current::default(), "t", "h").is_none());
    }

    #[test]
    fn recents_are_kept_per_person_in_their_ui_file_and_survive_a_restart() {
        let (cx, d) = with_models("pickpersist", 8);
        note(&cx, 3);
        note(&cx, 1);
        let user = cx.session.user.clone();
        assert_eq!(crate::uisize::load_recent_models(cx.data_dir(), &user), [("nvidia_ollama".to_string(), name(1)), ("nvidia_ollama".to_string(), name(3))]);
        assert!(crate::uisize::load_recent_models(cx.data_dir(), "someone-else").is_empty());
        // other things in the same file stay
        crate::uisize::save(cx.data_dir(), &user, 2);
        note(&cx, 5);
        assert_eq!(crate::uisize::load(cx.data_dir(), &user), 2);
        assert_eq!(crate::uisize::load_recent_models(cx.data_dir(), &user).len(), 3);
        let _ = d;
    }

    #[test]
    fn the_search_filters_both_groups_keeps_the_order_and_goes_back_with_escape() {
        let (cx, _d) = with_models("picksearch", 30);
        for i in [21, 3, 25] {
            note(&cx, i);
        }
        let mut m = open(&cx);
        for c in "model-2".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        let l = lines(&m);
        let items: Vec<usize> = l.iter().filter_map(|x| if let Line::Item(i) = x { Some(*i) } else { None }).collect();
        let names: Vec<&str> = items.iter().filter(|&&i| m.rows[i] == MRow::Plain).map(|&i| m.items[i].1.as_str()).collect();
        // recents that match first (25 then 21; 3 does not match), then the other matches in list order, none repeated
        assert_eq!(names[..2], [name(25), name(21)]);
        assert_eq!(names.len(), 10, "model-20 to model-29");
        assert!(names[2..].windows(2).all(|w| w[0] < w[1]));
        assert_eq!(l[0], Line::Label("Recent"));
        assert_eq!(m.sel, items[0], "the first match is highlighted");
        // custom lines and agents that do not match are gone; Enter picks the highlighted match
        assert!(!items.iter().any(|&i| matches!(m.rows[i], MRow::Custom(_))));
        assert_eq!(super::key(&mut m, &Event::Key(Key::Enter)), Out::Pick(Pick::Provider("nvidia_ollama".into(), name(25))));
        // the search is not case sensitive, takes several words, and the keys stay inside the matches
        let mut m = open(&cx);
        for c in "NVIDIA 07".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        assert_eq!(visible(&m).len(), 1);
        super::key(&mut m, &Event::Key(Key::Down));
        assert_eq!(m.items[m.sel].1, name(7));
        // nothing matches: nothing to pick, and the text says so
        super::key(&mut m, &Event::Key(Key::Char('z')));
        assert!(visible(&m).is_empty());
        assert_eq!(super::key(&mut m, &Event::Key(Key::Enter)), Out::Stay);
        let s = crate::ui::scale_for(1280, 720);
        let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert!(drawn(&g).is_empty());
        // Backspace widens it again; Esc clears the search first and only then closes
        super::key(&mut m, &Event::Key(Key::Backspace));
        assert_eq!(visible(&m).len(), 1);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Esc)), Out::Stay);
        assert!(m.query.is_empty());
        assert!(visible(&m).len() > 30);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Esc)), Out::Close);
        assert_eq!(super::key(&mut m, &Event::Key(Key::Ctrl('p'))), Out::Close, "Ctrl+P toggles");
    }

    #[test]
    fn a_long_list_with_recents_and_a_search_draws_inside_the_window_at_every_size_and_the_largest_text() {
        let (cx, _d) = with_models("pickgeo2", 60);
        for i in [5, 9, 40, 41, 42, 43] {
            note(&cx, i);
        }
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
            for text_scale in [1.0f32, 2.0] {
                let s = crate::ui::scale_for(w, h) * text_scale;
                for q in ["", "model-4"] {
                    let mut m = open(&cx);
                    m.query = q.to_string();
                    for k in [Key::Home, Key::PageDown, Key::End, Key::Up] {
                        super::key(&mut m, &Event::Key(k));
                        let (g, area) = draw_list(&cx, &mut m, w, h, s);
                        let rows = drawn(&g);
                        assert!(!rows.is_empty(), "{w}x{h} x{text_scale} q={q}");
                        assert!(rows.iter().any(|(i, _)| *i == m.sel), "highlighted line in view {w}x{h} x{text_scale}");
                        let panel_bottom = area.y + 8.0 * s + (area.h - 24.0 * s);
                        for (_, r) in &rows {
                            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y >= area.y - 0.5 && r.y + r.h <= panel_bottom + 0.5, "inside at {w}x{h} x{text_scale}: {r:?}");
                        }
                        for p in rows.windows(2) {
                            assert!(p[0].1.y + p[0].1.h <= p[1].1.y + 0.5);
                        }
                        // the bar sits inside the panel and the rows do not run under it
                        if let Some(b) = g.hits.iter().find(|x| x.label == "scroll bar") {
                            assert!(b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y >= area.y - 0.5 && b.rect.y + b.rect.h <= panel_bottom + 0.5);
                            assert!(rows.iter().all(|(_, r)| r.x + r.w <= b.rect.x + 0.5 || r.x + r.w <= b.rect.x + b.rect.w), "no row under the strip");
                        }
                    }
                }
            }
        }
    }

    fn multi(tag: &str, nv: usize) -> (AppCx, std::path::PathBuf) {
        use iyakki_shared::providers::{Kind, ProviderCfg};
        let (cx, d) = cx_with_backend(tag, iyakki_shared::chat::Unconfigured);
        let mk = |id: &str, kind: Kind, base: Option<&str>, models: Vec<String>| {
            let mut p = ProviderCfg::new(id, kind);
            p.base_url = base.map(str::to_string);
            p.models = models;
            if kind.needs_key() {
                p.key_ref = Some("vault:k".into());
            }
            p
        };
        let mut st = cx.settings().lock().unwrap();
        // configured order: the cloud ones first, to show the zone decides
        st.upsert_provider(mk("openai", Kind::OpenAi, None, vec!["gpt-x".into(), "shared-name".into()])).unwrap();
        st.upsert_provider(mk("nvidia", Kind::Nvidia, None, (0..nv).map(|i| if i == 0 { "nvidia/nemotron-x".to_string() } else { format!("meta/llama-{i}") }).chain(["shared-name".to_string()]).collect())).unwrap();
        st.upsert_provider(mk("lab", Kind::Compat, Some("http://192.168.1.20:8000/v1"), vec!["lab-model".into()])).unwrap();
        st.upsert_provider(mk("ollama", Kind::Ollama, None, vec!["llama3".into(), "qwen3".into()])).unwrap();
        drop(st);
        (cx, d)
    }

    fn heads(m: &ModelList) -> Vec<(String, usize)> {
        lines(m).iter().filter_map(|l| if let Line::Group(g) = l { Some((m.groups[*g].label.clone(), m.groups[*g].items.len())) } else { None }).collect()
    }

    #[test]
    fn providers_are_grouped_this_computer_then_the_network_then_the_internet_with_the_last_used_first() {
        let (cx, _d) = multi("pickgroups", 3);
        let m = open(&cx);
        let names: Vec<String> = heads(&m).into_iter().map(|h| h.0).collect();
        assert_eq!(names, ["Ollama", "lab", "OpenAI", "NVIDIA NIM"], "computer, network, then the internet in configured order");
        assert_eq!(m.groups.iter().map(|g| chip(g.zone)).collect::<Vec<_>>(), ["local", "network", "internet", "internet"]);
        assert_eq!(heads(&m)[3].1, 4, "count of the provider's models");
        // the cloud provider used last comes first
        crate::uisize::note_recent_model(cx.data_dir(), &cx.session.user, "nvidia", "shared-name");
        let m = open(&cx);
        assert_eq!(m.groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Ollama", "lab", "NVIDIA NIM", "OpenAI"]);
        // the agents stay their own group after the providers
        let l = lines(&m);
        let last_group = l.iter().rposition(|x| matches!(x, Line::Group(_))).unwrap();
        let first_agent = l.iter().position(|x| matches!(x, Line::Item(i) if matches!(m.rows[*i], MRow::Head(_)))).unwrap();
        assert!(first_agent > last_group);
        // the shown name drops the provider's own prefix, the full id is in the status line
        let nem = m.items.iter().position(|(p, mo)| p == "nvidia" && mo == "nvidia/nemotron-x").unwrap();
        assert_eq!(shown(&m, nem), "nemotron-x");
        let llama = m.items.iter().position(|(_, mo)| mo == "meta/llama-1").unwrap();
        assert_eq!(shown(&m, llama), "meta/llama-1", "another vendor's prefix stays");
    }

    #[test]
    fn long_lists_start_folded_but_the_group_in_use_and_groups_fold_and_unfold_by_key_and_click() {
        let (cx, _d) = multi("pickfold", 20);
        let cur = Current { plain: Some(("nvidia".into(), "meta/llama-5".into())), ..Default::default() };
        let mut m = build(&cx, plain(&cx), None, false, cur, "t", "h").unwrap();
        let open_groups: Vec<&str> = m.groups.iter().zip(&m.collapsed).filter(|(_, c)| !**c).map(|(g, _)| g.label.as_str()).collect();
        assert_eq!(open_groups, ["NVIDIA NIM"], "only the group of the model in use is open");
        assert_eq!(m.items[m.sel].1, "meta/llama-5");
        let n_open = visible(&m).len();
        assert_eq!(n_open, 21, "the open group's models only");
        let k = |m: &mut ModelList, key: Key| super::key(m, &Event::Key(key));
        // Home is the first heading; Right opens it, Left closes it, Enter flips it
        k(&mut m, Key::Home);
        assert_eq!(m.head, Some(0));
        k(&mut m, Key::Right);
        assert!(!m.collapsed[0]);
        assert_eq!(visible(&m).len(), n_open + 2);
        k(&mut m, Key::Left);
        assert!(m.collapsed[0]);
        k(&mut m, Key::Enter);
        assert!(!m.collapsed[0]);
        // Down walks into the group, Left from a model jumps to the heading and folds it
        k(&mut m, Key::Down);
        assert_eq!(m.head, None);
        k(&mut m, Key::Left);
        assert!(m.collapsed[0] && m.head == Some(0));
        // a click on a heading flips it and highlights it
        let g = m.groups.iter().position(|g| g.label == "OpenAI").unwrap();
        assert_eq!(super::key(&mut m, &Event::Tab(HEAD_TAB + g)), Out::Stay);
        assert!(!m.collapsed[g] && m.head == Some(g));
        // Enter on a heading never picks; Enter on a model does
        assert_eq!(k(&mut m, Key::Enter), Out::Stay);
        assert!(m.collapsed[g]);
        k(&mut m, Key::Enter);
        k(&mut m, Key::Down);
        assert!(matches!(k(&mut m, Key::Enter), Out::Pick(Pick::Provider(p, _)) if p == "openai"));
        // the drawn heading is a click target
        let s = crate::ui::scale_for(1280, 720);
        let (gx, _) = draw_list(&cx, &mut m, 1280, 720, s);
        assert_eq!(gx.hits.iter().filter(|h| h.label == "provider").count(), m.groups.len());
    }

    #[test]
    fn a_search_flattens_the_groups_and_shows_the_provider_so_equal_names_are_told_apart() {
        let (cx, _d) = multi("pickdup", 20);
        let mut m = open(&cx);
        for c in "shared".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        let v = visible(&m);
        assert_eq!(v.len(), 2);
        assert!(lines(&m).iter().all(|l| !matches!(l, Line::Group(_))), "no headings while searching");
        let owners: Vec<&str> = v.iter().map(|&i| m.items[i].0.as_str()).collect();
        assert_eq!(owners, ["openai", "nvidia"], "in the groups' order");
        let s = crate::ui::scale_for(1280, 720);
        let (g1, _) = draw_list(&cx, &mut m, 1280, 720, s);
        let (mut a, mut b) = (open(&cx), open(&cx));
        a.query = "shared".into();
        b.query = "shared".into();
        b.sel = v[1];
        // the provider name is drawn: a search for the provider's label changes pixels versus a blank column
        assert!(drawn(&g1).len() == 2);
        let _ = (&mut a, &mut b);
        // keys stay among the matches and the groups' fold state is untouched
        assert_eq!(super::key(&mut m, &Event::Key(Key::Down)), Out::Stay);
        assert_eq!(m.sel, v[1]);
        super::key(&mut m, &Event::Key(Key::Esc));
        assert!(heads(&m).len() == 4);
    }

    #[test]
    fn models_the_key_cannot_use_are_hidden_with_a_footer_and_come_back_with_show() {
        let (cx, _d) = with_models("pickhidden", 10);
        let mut m = open(&cx);
        m.unusable = vec![2, 3, 4];
        let s = crate::ui::scale_for(1280, 720);
        assert!(!visible(&m).iter().any(|i| m.unusable.contains(i)));
        assert_eq!(m.groups[0].items.len(), 10, "the group still owns them");
        let (g, area) = draw_list(&cx, &mut m, 1280, 720, s);
        let foot = g.hits.iter().find(|h| h.label == "hidden models").expect("the footer is a click target");
        assert!(foot.rect.y + foot.rect.h <= area.y + 8.0 * s + (area.h - 24.0 * s) + 0.5);
        assert!(drawn(&g).iter().all(|(i, _)| !m.unusable.contains(i)));
        // search never finds a hidden one
        for c in "model-03".chars() {
            super::key(&mut m, &Event::Key(Key::Char(c)));
        }
        assert!(visible(&m).is_empty());
        super::key(&mut m, &Event::Key(Key::Esc));
        // show: a click on the footer (or Ctrl+U) brings them back, hide again hides them
        assert_eq!(super::key(&mut m, &Event::Tab(HIDE_TAB)), Out::Stay);
        assert!(m.show_hidden && visible(&m).contains(&3));
        super::key(&mut m, &Event::Key(Key::Ctrl('u')));
        assert!(!m.show_hidden && !visible(&m).contains(&3));
        // nothing unusable: no footer and the keys do nothing
        let mut clean = open(&cx);
        let (g, _) = draw_list(&cx, &mut clean, 1280, 720, s);
        assert!(!g.hits.iter().any(|h| h.label == "hidden models"));
        super::key(&mut clean, &Event::Key(Key::Ctrl('u')));
        assert!(!clean.show_hidden);
    }

    #[test]
    fn the_recent_heading_stays_in_view_with_its_first_row_at_the_top_of_the_list() {
        let (cx, _d) = with_models("pickheading", 60);
        note(&cx, 7);
        let mut m = open(&cx);
        let s = crate::ui::scale_for(1280, 720);
        for home in [false, true] {
            super::key(&mut m, &Event::Key(if home { Key::Home } else { Key::End }));
            let (g, _) = draw_list(&cx, &mut m, 1280, 720, s);
            if home {
                assert_eq!(m.scroll.offset, 0.0);
                assert_eq!(m.sel, m.recent[0]);
                assert!(g.text_log.iter().any(|t| t.2 == "RECENT"), "the heading is drawn above the selected first row");
            }
        }
    }

    #[test]
    fn an_old_stored_nvidia_list_is_filtered_when_the_picker_reads_it() {
        use iyakki_shared::providers::{Kind, ProviderCfg};
        let (cx, _d) = cx_with_backend("pickstored", iyakki_shared::chat::Unconfigured);
        let ids = ["nvidia/nemotron-nano-9b-v2", "nvidia/ai-synthetic-video-detector", "nvidia/cosmos-reason2-8b", "nvidia/neva-22b", "nvidia/riva-translate-4b-instruct", "nvidia/riva-translate-4b-instruct-v2", "nvidia/vila", "nvidia/llama3-chatqa-1.5-70b", "meta/llama-3.1-8b-instruct"];
        let mut p = ProviderCfg::new("nvidia", Kind::Nvidia);
        p.key_ref = Some("vault:k".into());
        p.models = ids.iter().map(|s| s.to_string()).collect();
        let mut o = ProviderCfg::new("ollama", Kind::Ollama);
        o.models = vec!["vila-local".into(), "neva-mini".into()];
        {
            let mut st = cx.settings().lock().unwrap();
            st.upsert_provider(p).unwrap();
            st.upsert_provider(o).unwrap();
        }
        let m = build(&cx, plain(&cx), None, false, Current::default(), "t", "h").unwrap();
        let shown: Vec<&str> = m.items.iter().filter(|(p, _)| p == "nvidia").map(|(_, mo)| mo.as_str()).collect();
        assert_eq!(shown, ["nvidia/nemotron-nano-9b-v2", "nvidia/llama3-chatqa-1.5-70b", "meta/llama-3.1-8b-instruct"]);
        assert_eq!(m.items.iter().filter(|(p, _)| p == "ollama").count(), 2, "other providers are not filtered");
        assert!(visible(&m).iter().all(|&i| !m.items[i].1.contains("detector") && !m.items[i].1.contains("riva")));
    }
}
