//! The model chooser shared by the chat, the AI IDE's chat and TALES: the models of every enabled provider (provider name on
//! the right), then one group per external coding agent (`Claude Code (agent)  ready`) with its models indented under it and
//! `Custom model id…`. The owner of the screen keeps the list in a field, forwards events to [`key`], draws it with [`draw`] and
//! acts on the [`Pick`] that comes back.

use iyakki_extagentx::{self as extagent, ModelInfo};

use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::scrolllist::ScrollList;
use iyakki_uikit::ui::{Event, Key, DIM, MUTED, OK, R, TEXT};

/// The hit id of the scroll bar strip.
pub const BAR: usize = 0x6d70;
/// A click on a provider heading is `Event::Tab(HEAD_TAB + group)`.
pub const HEAD_TAB: usize = 1_450_000;
/// A click on the footer that hides or shows the models the person's key cannot use.
pub const HIDE_TAB: usize = 1_449_999;
/// More plain models than this and the groups start collapsed (all but the one in use).
pub const LONG: usize = 12;

/// One provider's models: a heading and the lines under it.
#[derive(Clone, Debug)]
pub struct PGroup {
    pub id: String,
    /// The provider as the person knows it ("NVIDIA NIM", or the name they gave it).
    pub label: String,
    pub zone: Zone,
    /// The lines (indexes into `items`) of this provider, in the list's order.
    pub items: Vec<usize>,
    /// A prefix the model ids of this provider all carry (`nvidia/` under NVIDIA): left out of the shown name.
    pub own_prefix: String,
}

/// Where a model runs, from the most private (the order the groups are drawn in). The shell converts the router's zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Zone {
    Computer,
    Network,
    Internet,
}

pub fn chip(z: Zone) -> &'static str {
    match z {
        Zone::Computer => "local",
        Zone::Network => "network",
        Zone::Internet => "internet",
    }
}

pub const ACCENT: [u8; 4] = rgb(79, 140, 255);

/// One line of the chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum MRow {
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
pub struct Current {
    /// The extra first line is the choice.
    pub same: bool,
    /// (provider, model) in use.
    pub plain: Option<(String, String)>,
    /// (agent id, model id; empty = the CLI's default) in use.
    pub agent: Option<(String, String)>,
}

/// The extra first line: its name and the grey text on its right.
#[derive(Clone, Debug)]
pub struct TopRow {
    pub label: String,
    pub note: String,
}

/// The chooser: (provider, model) pairs, the highlighted one, and what each line is.
pub struct ModelList {
    pub items: Vec<(String, String)>,
    pub sel: usize,
    /// What each line is (same order as `items`).
    pub rows: Vec<MRow>,
    /// A model id being typed for an agent ("Custom model id…"): (agent id, text so far).
    pub custom: Option<(String, String)>,
    pub title: String,
    pub hint: String,
    pub current: Current,
    pub top: Option<TopRow>,
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
    pub follow: bool,
    /// The highlighted line the last draw saw: another one means a key (or a test) moved it.
    pub seen: usize,
    /// "This machine: fits: 7B Q4, 13B Q4. Add one in Settings, Models." (docs/AI-HARDWARE.md): what local models would suit this machine.
    /// A hint only: no model that is configured is ever left out of the list because of it.
    pub fits: Option<String>,
}

/// What the person chose.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// The extra first line.
    Top,
    Provider(String, String),
    /// An agent, keeping its current model.
    Agent(String),
    AgentModel { agent: String, model: String, custom: bool },
}

/// What a key or click did to the list.
#[derive(Debug, PartialEq)]
pub enum Out {
    Stay,
    Close,
    Pick(Pick),
    /// A model id was refused: the text to show; the list stays open.
    Bad(String),
}

/// One external coding agent as the list needs it: its id, the provider word it carries and the models to list under it.
#[derive(Clone, Debug)]
pub struct AgentInput {
    pub id: String,
    pub provider: String,
    pub models: Vec<ModelInfo>,
}

/// One provider of Settings, Models as the groups need it.
#[derive(Clone, Debug)]
pub struct ProviderInput {
    pub id: String,
    pub label: String,
    pub zone: Zone,
    pub own_prefix: String,
}

/// Everything [`build`] needs, gathered by the shell (settings, the router, the extension status, the recent-model file).
pub struct Inputs {
    /// The (provider, model) pairs, already filtered of models that cannot chat.
    pub plain: Vec<(String, String)>,
    pub top: Option<TopRow>,
    /// The external agents, in the order listed (empty when they do not follow).
    pub agents: Vec<AgentInput>,
    pub current: Current,
    pub title: String,
    pub hint: String,
    pub fits: Option<String>,
    /// The (provider, model) pairs the person's key cannot use.
    pub bad: Vec<(String, String)>,
    /// The person's recently used models, most recent first.
    pub used: Vec<(String, String)>,
    /// The configured providers (an id not found here is drawn under its own id, in the internet zone).
    pub providers: Vec<ProviderInput>,
}

/// Build the list. `None` when there is nothing to choose.
pub fn build(inp: Inputs) -> Option<ModelList> {
    let Inputs { plain, top, agents, current, title, hint, fits, bad, used, providers } = inp;
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
    {
        let (cur_agent, cur_model) = match &current.agent {
            Some((a, m)) => (Some(a.as_str()), Some(m.as_str())),
            None => (None, None),
        };
        // an agent, then its models indented under it, then "Custom model id…"
        for e in agents {
            if cur_agent == Some(e.id.as_str()) && cur_model == Some("") {
                chosen_at = Some(items.len());
            }
            items.push((e.provider.clone(), String::new()));
            rows.push(MRow::Head(e.id.clone()));
            for mi in e.models {
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
    let unusable: Vec<usize> = (offset..builtin).filter(|&i| bad.contains(&items[i])).collect();
    let (groups, collapsed) = {
        let mut groups: Vec<PGroup> = Vec::new();
        for i in offset..builtin {
            let pid = &items[i].0;
            if let Some(g) = groups.iter_mut().find(|g| g.id == *pid) {
                g.items.push(i);
                continue;
            }
            let g = match providers.iter().find(|c| c.id == *pid) {
                Some(c) => PGroup { id: pid.clone(), label: c.label.clone(), zone: c.zone, items: vec![i], own_prefix: c.own_prefix.clone() },
                None => PGroup { id: pid.clone(), label: pid.clone(), zone: Zone::Internet, items: vec![i], own_prefix: format!("{}/", pid.to_ascii_lowercase()) },
            };
            groups.push(g);
        }
        // this computer, the local network, the internet; inside a zone the provider used last comes first, then as configured
        let rank = |id: &str| used.iter().position(|(p, _)| p == id).unwrap_or(usize::MAX);
        groups.sort_by_key(|g| (g.zone, rank(&g.id)));
        let long = builtin - offset > LONG;
        let open = groups.iter().position(|g| g.items.contains(&sel)).unwrap_or(0);
        let collapsed = (0..groups.len()).map(|g| long && g != open).collect();
        (groups, collapsed)
    };
    let recent: Vec<usize> = used.iter().filter_map(|(p, m)| (offset..builtin).find(|&i| items[i].0 == *p && items[i].1 == *m)).collect();
    Some(ModelList { items, sel, rows, custom: None, title, hint, current, top, recent, groups, collapsed, unusable, show_hidden: false, head: None, query: String::new(), scroll: ScrollList::default(), follow: true, seen: usize::MAX, fits })
}

/// Keys and clicks (`Event::Row`) of the list.
pub fn key(m: &mut ModelList, ev: &Event) -> Out {
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
pub enum Nav {
    Item(usize),
    Head(usize),
}

pub fn navs(m: &ModelList) -> Vec<Nav> {
    lines(m)
        .into_iter()
        .filter_map(|l| match l {
            Line::Item(i) => Some(Nav::Item(i)),
            Line::Group(g) => Some(Nav::Head(g)),
            Line::Label(_) => None,
        })
        .collect()
}

pub fn group_of(m: &ModelList, i: usize) -> Option<usize> {
    m.groups.iter().position(|g| g.items.contains(&i))
}

/// Fold or unfold a group. A highlighted line inside a group that closes moves to its heading.
pub fn fold(m: &mut ModelList, g: usize, closed: bool) {
    m.collapsed[g] = closed;
    if closed && m.head.is_none() && m.groups[g].items.contains(&m.sel) && !m.recent.contains(&m.sel) {
        m.head = Some(g);
    }
    m.follow = true;
}

/// The text of a line that the search looks in.
pub fn haystack(m: &ModelList, i: usize) -> String {
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
pub fn matches(m: &ModelList, i: usize) -> bool {
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
pub fn retype(m: &mut ModelList, f: impl FnOnce(&mut String)) {
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
pub enum Line {
    Label(&'static str),
    Group(usize),
    Item(usize),
}

/// What is drawn, top to bottom: the extra first line, the "Recent" group, the providers' groups (this computer, the network, the
/// internet; each with a heading, its models under it unless folded) and then the external agents as the list gives them. A
/// search drops the headings and keeps the order; what is left of the groups follows under "All models".
pub fn lines(m: &ModelList) -> Vec<Line> {
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
pub fn visible(m: &ModelList) -> Vec<usize> {
    lines(m).into_iter().filter_map(|l| if let Line::Item(i) = l { Some(i) } else { None }).collect()
}

/// Enter or a click on the highlighted line.
pub fn choose(m: &mut ModelList) -> Out {
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
pub fn shown(m: &ModelList, i: usize) -> String {
    let model = &m.items[i].1;
    group_of(m, i).and_then(|g| model.strip_prefix(m.groups[g].own_prefix.as_str())).filter(|r| !r.is_empty()).map_or_else(|| model.clone(), str::to_string)
}

/// The panel over `area`: title, hint, the lines (scrolled so the highlighted one shows) and the id being typed.
pub fn draw(g: &mut Gfx, area: R, s: f32, ext_rows: &[extagent::Entry], login_tab: &dyn Fn(&str) -> Option<usize>, m: &mut ModelList) {
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
        g.text(px + 20.0 * s, py0 + 58.0 * s, 12.0 * s, DIM, &f, Face::Sans);
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
                g.text(px + pw - 24.0 * s - hw, y + 9.0 * s, 13.0 * s, if e.ready { OK } else if signin { iyakki_uikit::ui::ERR } else { DIM }, &hint, Face::Sans);
                // not signed in: the same Login button as everywhere else (the shell opens the dialog)
                if let (true, Some(tab)) = (signin, login_tab(&e.id)) {
                    g.button(px + pw - 24.0 * s - hw - btn, y + 1.0 * s, 12.0 * s, "Login", OK, Event::Tab(tab));
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

    fn inputs(plain: &[(&str, &str)]) -> Inputs {
        Inputs {
            plain: plain.iter().map(|(p, m)| (p.to_string(), m.to_string())).collect(),
            top: None,
            agents: vec![],
            current: Current::default(),
            title: "Choose".into(),
            hint: "hint".into(),
            fits: None,
            bad: vec![],
            used: vec![],
            providers: vec![
                ProviderInput { id: "cloud".into(), label: "Cloud".into(), zone: Zone::Internet, own_prefix: "cloud/".into() },
                ProviderInput { id: "home".into(), label: "Home".into(), zone: Zone::Computer, own_prefix: "home/".into() },
            ],
        }
    }

    #[test]
    fn nothing_to_choose_gives_no_list_and_groups_run_from_the_most_private() {
        assert!(build(inputs(&[])).is_none());
        let m = build(inputs(&[("cloud", "a"), ("home", "b"), ("cloud", "c")])).unwrap();
        assert_eq!(m.groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), ["home", "cloud"]);
        assert_eq!(m.groups[1].items, [0, 2]);
        assert_eq!(m.items.len(), 3);
    }

    #[test]
    fn the_unusable_and_recent_lines_are_found_by_pair_and_the_current_model_is_selected() {
        let mut i = inputs(&[("cloud", "a"), ("home", "b")]);
        i.bad = vec![("home".into(), "b".into())];
        i.used = vec![("home".into(), "b".into()), ("cloud".into(), "gone".into())];
        i.current.plain = Some(("home".into(), "b".into()));
        let m = build(i).unwrap();
        assert_eq!((m.unusable.clone(), m.recent.clone(), m.sel), (vec![1], vec![1], 1));
    }

    #[test]
    fn a_typed_agent_model_id_is_checked_before_it_is_picked() {
        let mut m = build(inputs(&[("cloud", "a")])).unwrap();
        m.custom = Some(("claude-code".into(), "bad id".into()));
        assert!(matches!(key(&mut m, &Event::Key(Key::Enter)), Out::Bad(_)));
        m.custom = Some(("claude-code".into(), "good-id".into()));
        assert_eq!(key(&mut m, &Event::Key(Key::Enter)), Out::Pick(Pick::AgentModel { agent: "claude-code".into(), model: "good-id".into(), custom: true }));
    }
}
