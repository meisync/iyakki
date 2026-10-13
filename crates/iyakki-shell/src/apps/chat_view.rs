//! The CHAT transcript view. The conversation is shown as turns: your message in a rounded box, then what the
//! assistant did as a timeline on the left (a grey dot while it thinks, a green dot and the tool's name for each tool
//! call with its `IN` and `OUT`, an amber dot while it waits for your decision, a red dot for a failure) and its
//! answers as flowing Markdown on the dark background.
//!
//! The turns are built from the saved messages (nothing new is stored), each block is laid out once for a width and
//! kept, so a redraw only paints, and a new message lays out just itself. Blocks that hold more than they show
//! (a tool call, the reasoning text) open with a click or with Up/Down then Enter.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use iyakki_agent::chat::{Message, Role};
use serde_json::Value;

use super::chatpic::{self, PicKind, PicSrc, Rendered};
use crate::gfx::{rgb, Face, Gfx};
use crate::mdtext;
use crate::textsel::{self, SelDoc, SelLine, Selection};
use crate::ui::{Event, Key, R, DIM, ERR, MUTED, OK, TEXT};

/// Clicks on blocks that open and close arrive as `Event::Tab(TOGGLE_BASE + n)`.
pub const TOGGLE_BASE: usize = 100_000;
/// The copy buttons that appear on a hovered message or code block send `Event::Tab(COPY_BASE + n)`.
pub const COPY_BASE: usize = 50_000;
/// The Copy and Save buttons under a picture send `Event::Tab(PIC_BASE + 2n)` and `Event::Tab(PIC_BASE + 2n + 1)` for the n-th picture.
pub const PIC_BASE: usize = 80_000;
/// The transcript is one area for text selection: presses and drags arrive as `Event::AreaAt(AREA, x, y)`.
pub const AREA: usize = 61_000;

const AMBER: [u8; 4] = rgb(255, 196, 94);
const USER_BG: [u8; 4] = rgb(14, 24, 44);
const USER_LINE: [u8; 4] = rgb(44, 112, 150);
const OUT_BG: [u8; 4] = rgb(11, 14, 26);
const OUT_LINE: [u8; 4] = rgb(38, 66, 98);
const IN_TEXT: [u8; 4] = rgb(176, 186, 212);
const OUT_TEXT: [u8; 4] = rgb(196, 206, 230);
const TIMELINE: [u8; 4] = rgb(42, 86, 118);
/// Lines of a result shown before "show more".
pub const OUT_LINES: usize = 6;
const MAX_EXPANDED: usize = 200;

// ------------------------------------------------------------------ the model

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolStatus {
    Running,
    Waiting,
    Ok,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Block {
    /// The model is thinking. `text` is the reasoning when the provider sends it; without it the block only shows activity.
    Thinking { text: String, live: bool, expanded: bool },
    /// An answer, as Markdown; `msg` is the message it came from (a click copies that message).
    Text { text: String, msg: usize },
    Tool { key: String, name: String, status: ToolStatus, input: String, input_full: String, output: Option<String>, hidden: bool, expanded: bool },
    /// The agent waits for a decision.
    Approval { tool: String, summary: String, detail: String },
    Image { path: String },
    /// A drawing or image the assistant wrote into its answer (SVG, a data address, base64); `key` opens its code.
    Picture { src: PicSrc, key: String, expanded: bool },
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Turn {
    /// (message index, text) of what the person wrote.
    pub user: Option<(usize, String)>,
    pub blocks: Vec<Block>,
}

/// What `build_turns` needs besides the messages.
pub struct Ctx<'a> {
    pub tool_ok: &'a HashMap<String, bool>,
    /// (call id, tool, what it will do, the exact change) while the agent waits.
    pub approval: Option<(&'a str, &'a str, &'a str, &'a str)>,
    /// A request is under way (an agent run or a plain answer).
    pub busy: bool,
    pub open: &'a HashSet<String>,
}

/// The tools whose results are private: the model gets them, the transcript says `(hidden)` (like the tool log).
pub fn is_private(tool: &str) -> bool {
    tool == "read_file"
}

/// One line and the full text of a call's arguments.
pub fn summarize_args(args: &Value) -> (String, String) {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let show = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if a.iter().all(|x| x.is_string()) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        other => other.to_string(),
    };
    match args {
        Value::Null => (String::new(), String::new()),
        Value::Object(m) if m.is_empty() => (String::new(), String::new()),
        Value::Object(m) => {
            const FIRST: [&str; 6] = ["command", "path", "url", "query", "prompt", "name"];
            let lead = FIRST.iter().find(|k| m.contains_key(**k)).copied();
            let mut parts: Vec<String> = Vec::new();
            if let Some(k) = lead {
                let mut v = show(&m[k]);
                if k == "command" {
                    if let Some(a) = m.get("args") {
                        v = format!("{v} {}", show(a));
                    }
                }
                parts.push(flat(&v));
            }
            for (k, v) in m {
                if Some(k.as_str()) == lead || (lead == Some("command") && k == "args") {
                    continue;
                }
                let t: String = flat(&show(v)).chars().take(60).collect();
                parts.push(if m.len() == 1 { t } else { format!("{k}={t}") });
            }
            let full: Vec<String> = m.iter().map(|(k, v)| format!("{k}: {}", show(v).chars().take(2000).collect::<String>())).collect();
            (parts.join("  "), full.join("\n"))
        }
        other => (flat(&show(other)), show(other)),
    }
}

/// Turns the saved messages into the turns that are shown. Tool results join their calls; the system prompt is not shown.
pub fn build_turns(msgs: &[Message], cx: &Ctx) -> Vec<Turn> {
    let results: HashMap<&str, &Message> = msgs.iter().filter(|m| m.role == Role::Tool).filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m))).collect();
    let mut turns: Vec<Turn> = Vec::new();
    for (idx, m) in msgs.iter().enumerate() {
        match m.role {
            Role::System | Role::Tool => {}
            Role::User => turns.push(Turn { user: Some((idx, m.content.clone())), blocks: vec![] }),
            Role::Assistant => {
                if turns.is_empty() {
                    turns.push(Turn::default());
                }
                let turn = turns.last_mut().expect("a turn exists");
                match chatpic::split(&m.content) {
                    Some(segs) => {
                        let mut n = 0;
                        for seg in segs {
                            match seg {
                                chatpic::Seg::Text(t) => turn.blocks.push(Block::Text { text: t, msg: idx }),
                                chatpic::Seg::Pic(src) => {
                                    let key = format!("pic:{idx}:{n}");
                                    n += 1;
                                    let expanded = cx.open.contains(&key);
                                    turn.blocks.push(Block::Picture { src, key, expanded });
                                }
                            }
                        }
                    }
                    None if !m.content.trim().is_empty() => turn.blocks.push(Block::Text { text: m.content.clone(), msg: idx }),
                    None => {}
                }
                for c in &m.tool_calls {
                    let (input, input_full) = summarize_args(&c.args);
                    let key = format!("tool:{}", c.id);
                    let mut images = Vec::new();
                    let (status, output) = match results.get(c.id.as_str()) {
                        Some(r) => {
                            let ok = cx.tool_ok.get(&c.id).copied().unwrap_or(true);
                            let mut keep = Vec::new();
                            for l in r.content.lines() {
                                match l.strip_prefix("IMAGE: ") {
                                    Some(p) => images.push(p.to_string()),
                                    None => keep.push(l),
                                }
                            }
                            (if ok { ToolStatus::Ok } else { ToolStatus::Failed }, Some(keep.join("\n")))
                        }
                        None if cx.approval.is_some_and(|a| a.0 == c.id) => (ToolStatus::Waiting, None),
                        None if cx.busy => (ToolStatus::Running, None),
                        None => (ToolStatus::Ok, None),
                    };
                    let expanded = cx.open.contains(&key);
                    turn.blocks.push(Block::Tool { key, name: c.name.clone(), status, input, input_full, output, hidden: is_private(&c.name), expanded });
                    turn.blocks.extend(images.into_iter().map(|path| Block::Image { path }));
                }
            }
        }
    }
    if turns.is_empty() && (cx.busy || cx.approval.is_some()) {
        turns.push(Turn::default());
    }
    if let (Some(turn), Some((_, tool, summary, detail))) = (turns.last_mut(), cx.approval) {
        turn.blocks.push(Block::Approval { tool: tool.into(), summary: summary.into(), detail: detail.into() });
    }
    if let Some(turn) = turns.last_mut() {
        let waiting = matches!(turn.blocks.last(), Some(Block::Approval { .. }) | Some(Block::Tool { status: ToolStatus::Running | ToolStatus::Waiting, .. }));
        if cx.busy && !waiting {
            turn.blocks.push(Block::Thinking { text: String::new(), live: true, expanded: cx.open.contains("think:live") });
        }
    }
    turns
}

fn hash_of<T: Hash + ?Sized>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

// ------------------------------------------------------------------ one laid-out block

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Idle,
    Live,
    Running,
    Waiting,
    Ok,
    Failed,
}

#[derive(Clone, Debug)]
enum Act {
    Toggle,
}

/// A part of a block that can be copied as a whole: a message, or a code block inside one. The copy button shows at its
/// top-right corner while the pointer is over it.
#[derive(Clone, Debug)]
pub struct Zone {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub code: bool,
}

#[derive(Clone)]
enum Prim {
    Fill { x: f32, y: f32, w: f32, h: f32, r: f32, c: [u8; 4] },
    Stroke { x: f32, y: f32, w: f32, h: f32, r: f32, bw: f32, c: [u8; 4] },
    Text { x: f32, y: f32, size: f32, c: [u8; 4], face: Face, text: String },
    Md { l: Rc<mdtext::Layout>, x: f32, y: f32, w: f32 },
    /// A vertical fade from clear to `c`.
    Fade { x: f32, y: f32, w: f32, h: f32, c: [u8; 4] },
    Image { pm: Arc<tiny_skia::Pixmap>, x: f32, y: f32, w: f32, h: f32 },
    Button { x: f32, y: f32, size: f32, label: String, c: [u8; 4], key: Key },
    Hit { x: f32, y: f32, w: f32, h: f32, act: Act },
    Chevron { x: f32, y: f32, size: f32, open: bool },
    PicBtn { x: f32, y: f32, size: f32, label: String, act: PicAct },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PicAct {
    Copy,
    Save,
    Toggle,
}

/// Where the transcript gets the pictures it shows. The shell implements this with its caches (chat.rs).
pub trait Pics {
    /// A picture file the assistant made (or the person owns), drawn to fit `max_w` pixels.
    fn file(&mut self, g: &mut Gfx, path: &str, max_w: f32, s: f32) -> Option<Arc<tiny_skia::Pixmap>>;
    /// A picture written into an answer.
    fn inline(&mut self, g: &mut Gfx, src: &PicSrc, max_w: f32, s: f32) -> Result<Rendered, String>;
}

/// No pictures at all (tests).
pub struct NoPics;

impl Pics for NoPics {
    fn file(&mut self, _: &mut Gfx, _: &str, _: f32, _: f32) -> Option<Arc<tiny_skia::Pixmap>> {
        None
    }
    fn inline(&mut self, _: &mut Gfx, _: &PicSrc, _: f32, _: f32) -> Result<Rendered, String> {
        Err("no pictures here".into())
    }
}

/// A block drawn once: its height and what to paint, relative to its top-left corner.
#[derive(Default)]
pub struct BlockLayout {
    pub h: f32,
    /// Where the timeline dot sits (from the top) and what it means.
    pub dot: Option<(f32, Dot)>,
    prims: Vec<Prim>,
    /// This block opens and closes.
    pub toggle: Option<String>,
    /// The text that can be selected, line by line (positions relative to the block's corner).
    sel: Vec<SelLine>,
    /// What the copy buttons copy.
    zones: Vec<Zone>,
}

impl BlockLayout {
    /// The strings drawn, for tests.
    pub fn texts(&self) -> Vec<String> {
        let mut v = Vec::new();
        for p in &self.prims {
            match p {
                Prim::Text { text, .. } => v.push(text.clone()),
                Prim::Md { l, .. } => v.extend(l.lines.iter().map(|x| x.pieces.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" "))),
                Prim::Button { label, .. } | Prim::PicBtn { label, .. } => v.push(label.clone()),
                _ => {}
            }
        }
        v
    }
    /// The Markdown layout of an answer, for tests.
    pub fn markdown(&self) -> Option<&mdtext::Layout> {
        self.prims.iter().find_map(|p| if let Prim::Md { l, .. } = p { Some(&**l) } else { None })
    }
    /// The buttons: (label, key it presses).
    pub fn buttons(&self) -> Vec<(String, Key)> {
        self.prims.iter().filter_map(|p| if let Prim::Button { label, key, .. } = p { Some((label.clone(), key.clone())) } else { None }).collect()
    }
    /// The rightmost edge of anything drawn.
    pub fn right_edge(&self) -> f32 {
        let mut r = 0.0f32;
        for p in &self.prims {
            r = r.max(match p {
                Prim::Fill { x, w, .. } | Prim::Stroke { x, w, .. } | Prim::Image { x, w, .. } | Prim::Fade { x, w, .. } => x + w,
                Prim::Md { x, l, .. } => x + l.width,
                _ => 0.0,
            });
        }
        r
    }
}

fn text(v: &mut Vec<Prim>, x: f32, y: f32, size: f32, c: [u8; 4], face: Face, t: impl Into<String>) {
    v.push(Prim::Text { x, y, size, c, face, text: t.into() });
}

fn layout_user(g: &mut Gfx, t: &str, _idx: usize, w: f32, s: f32) -> BlockLayout {
    let size = 16.0 * s;
    let lh = g.line_height(size, Face::Sans) * 1.22;
    let (px, py) = (14.0 * s, 11.0 * s);
    let mut lines = g.wrap(size, t.trim_end(), Face::Sans, w - 2.0 * px);
    let extra = lines.len().saturating_sub(40);
    lines.truncate(40);
    let mut prims = Vec::new();
    let rows = lines.len() + usize::from(extra > 0);
    let h = rows as f32 * lh + 2.0 * py;
    prims.push(Prim::Fill { x: 0.0, y: 0.0, w, h, r: 10.0 * s, c: USER_BG });
    prims.push(Prim::Stroke { x: 0.0, y: 0.0, w, h, r: 10.0 * s, bw: 1.0 * s, c: USER_LINE });
    let mut sel = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        text(&mut prims, px, py + i as f32 * lh, size, TEXT, Face::Sans, l.clone());
        sel.push(sel_line(g, px, py + i as f32 * lh, lh, size, Face::Sans, l, 0));
    }
    if extra > 0 {
        text(&mut prims, px, py + lines.len() as f32 * lh, size * 0.85, DIM, Face::Sans, format!("… {extra} more lines"));
    }
    BlockLayout { h, dot: None, prims, toggle: None, sel, zones: vec![Zone { x: 0.0, y: 0.0, w, h, text: t.trim_end().to_string(), code: false }] }
}

fn layout_text(g: &mut Gfx, md: &str, _idx: usize, w: f32, s: f32) -> BlockLayout {
    let l = mdtext::layout_with(g, md, 16.0 * s, w, 1.24);
    let h = l.height + 6.0 * s;
    let (sel, mut zones) = md_selection(g, &l, w, md);
    zones.insert(0, Zone { x: 0.0, y: 0.0, w, h, text: md.trim_end().to_string(), code: false });
    let prims = vec![Prim::Md { l: Rc::new(l), x: 0.0, y: 0.0, w }];
    BlockLayout { h, dot: None, prims, toggle: None, sel, zones }
}

/// One selectable line: `t` drawn from `x` at `size` in `face`, `h` tall, from `y`.
#[allow(clippy::too_many_arguments)]
fn sel_line(g: &mut Gfx, x: f32, y: f32, h: f32, size: f32, face: Face, t: &str, para: u32) -> SelLine {
    SelLine { y, h, text: t.to_string(), xs: textsel::char_xs(g, x, size, face, t), block: 0, para }
}

/// The code inside ``` fences of some Markdown, block by block.
fn fenced_blocks(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<&str>> = None;
    for l in md.lines() {
        if l.trim_start().starts_with("```") {
            match cur.take() {
                Some(c) => out.push(c.join("\n")),
                None => cur = Some(Vec::new()),
            }
        } else if let Some(c) = cur.as_mut() {
            c.push(l);
        }
    }
    if let Some(c) = cur {
        out.push(c.join("\n"));
    }
    out
}

/// The selectable lines of a Markdown layout (list markers included, as `•` or `1.`), and a copy zone for each code block.
fn md_selection(g: &mut Gfx, l: &mdtext::Layout, w: f32, md: &str) -> (Vec<SelLine>, Vec<Zone>) {
    use mdtext::{Kind, Style};
    let mut sel = Vec::new();
    let mut zones: Vec<Zone> = Vec::new();
    let (mut para, mut prev): (u32, Option<Kind>) = (0, None);
    let mut code_runs: Vec<(f32, f32, String)> = Vec::new();
    for line in &l.lines {
        if matches!(line.kind, Kind::Gap | Kind::Rule) {
            prev = None;
            continue;
        }
        if prev != Some(line.kind) || line.marker.is_some() {
            para += 1;
        }
        prev = Some(line.kind);
        let heading = line.kind == Kind::Heading;
        let tx = line.indent;
        let mut text = String::new();
        let mut lefts: Vec<f32> = Vec::new();
        let mut end = tx;
        let add = |text: &mut String, lefts: &mut Vec<f32>, end: &mut f32, t: &str, xs: &[f32]| {
            text.push_str(t);
            lefts.extend(xs.iter().take(xs.len().saturating_sub(1)));
            *end = xs.last().copied().unwrap_or(*end);
        };
        if let Some(m) = &line.marker {
            let mw = g.text_width(line.size, m, Face::Sans);
            let mx = tx - mw - line.size * 0.35;
            let xs = textsel::char_xs(g, mx, line.size, Face::Sans, m);
            add(&mut text, &mut lefts, &mut end, m, &xs);
            let e = end;
            add(&mut text, &mut lefts, &mut end, " ", &[e, tx]);
        }
        for p in &line.pieces {
            let start = tx + p.x;
            if !lefts.is_empty() && start > end + 0.5 {
                let e = end;
                add(&mut text, &mut lefts, &mut end, " ", &[e, start]);
            }
            let (face, size) = if line.kind == Kind::Code {
                (Face::Mono, line.size)
            } else if p.style == Style::Code {
                (Face::Mono, line.size * 0.92)
            } else if p.style == Style::Bold || heading {
                (Face::Bold, line.size)
            } else {
                (Face::Sans, line.size)
            };
            let xs = textsel::char_xs(g, start, size, face, &p.text);
            add(&mut text, &mut lefts, &mut end, &p.text, &xs);
        }
        if lefts.is_empty() {
            continue;
        }
        let mut xs = lefts;
        xs.push(end);
        if line.kind == Kind::Code {
            match code_runs.last_mut() {
                Some(r) if (r.1 - line.y).abs() < line.h * 1.6 && sel.last().is_some_and(|x: &SelLine| x.para == para) => {
                    r.1 = line.y + line.h;
                    r.2.push('\n');
                    r.2.push_str(&text);
                }
                _ => code_runs.push((line.y, line.y + line.h, text.clone())),
            }
        }
        sel.push(SelLine { y: line.y, h: line.h, text, xs, block: 0, para });
    }
    let fences = fenced_blocks(md);
    let exact = fences.len() == code_runs.len();
    for (k, (y0, y1, joined)) in code_runs.into_iter().enumerate() {
        zones.push(Zone { x: -4.0, y: y0, w: w + 8.0, h: y1 - y0, text: if exact { fences[k].clone() } else { joined }, code: true });
    }
    (sel, zones)
}

fn layout_thinking(g: &mut Gfx, reasoning: &str, live: bool, expanded: bool, w: f32, s: f32) -> BlockLayout {
    let size = 14.5 * s;
    let lh = g.line_height(size, Face::Sans);
    let mut prims = Vec::new();
    text(&mut prims, 0.0, 0.0, size, DIM, Face::Sans, "Thinking");
    let has = !reasoning.trim().is_empty();
    let mut h = lh;
    let mut sel = Vec::new();
    if has {
        let tw = g.text_width(size, "Thinking", Face::Sans);
        prims.push(Prim::Chevron { x: tw + 10.0 * s, y: lh / 2.0, size: 9.0 * s, open: expanded });
        if expanded {
            let sz = 13.5 * s;
            let l2 = g.line_height(sz, Face::Sans) * 1.25;
            let mut lines = g.wrap(sz, reasoning.trim(), Face::Sans, w);
            lines.truncate(MAX_EXPANDED);
            for (i, l) in lines.iter().enumerate() {
                text(&mut prims, 0.0, lh + 5.0 * s + i as f32 * l2, sz, MUTED, Face::Sans, l.clone());
                sel.push(sel_line(g, 0.0, lh + 5.0 * s + i as f32 * l2, l2, sz, Face::Sans, l, 0));
            }
            h = lh + 5.0 * s + lines.len() as f32 * l2;
        }
        // only the title line opens and closes it; the reasoning below can be selected
        prims.push(Prim::Hit { x: 0.0, y: 0.0, w, h: lh + 4.0 * s, act: Act::Toggle });
    }
    let toggle = has.then(|| if live { "think:live".to_string() } else { "think".to_string() });
    BlockLayout { h: h + 6.0 * s, dot: Some((lh / 2.0, if live { Dot::Live } else { Dot::Idle })), prims, toggle, sel, zones: vec![] }
}

/// `text` cut to a whole number of monospace columns, with an ellipsis when it was longer.
fn clip_cols(t: &str, cols: usize) -> String {
    let t = t.replace('\t', "    ");
    if t.chars().count() <= cols {
        return t;
    }
    let mut out: String = t.chars().take(cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Breaks `t` into rows of `cols` monospace columns.
fn chunk_cols(t: &str, cols: usize) -> Vec<String> {
    let t = t.replace('\t', "    ");
    let c: Vec<char> = t.chars().collect();
    if c.is_empty() {
        return vec![String::new()];
    }
    c.chunks(cols.max(4)).map(|x| x.iter().collect()).collect()
}

#[allow(clippy::too_many_arguments)]
fn layout_tool(g: &mut Gfx, name: &str, status: ToolStatus, input: &str, input_full: &str, output: Option<&str>, hidden: bool, expanded: bool, w: f32, s: f32) -> BlockLayout {
    let mut prims = Vec::new();
    let nsz = 15.0 * s;
    let nlh = g.line_height(nsz, Face::Bold);
    text(&mut prims, 0.0, 0.0, nsz, TEXT, Face::Bold, name);
    let mut sel = vec![sel_line(g, 0.0, 0.0, nlh, nsz, Face::Bold, name, 0)];
    let nw = g.text_width(nsz, name, Face::Bold);
    prims.push(Prim::Chevron { x: nw + 11.0 * s, y: nlh / 2.0, size: 9.0 * s, open: expanded });
    let (note, ncol) = match status {
        ToolStatus::Running => ("running", DIM),
        ToolStatus::Waiting => ("waiting for you", AMBER),
        ToolStatus::Failed => ("failed", ERR),
        ToolStatus::Ok => ("", DIM),
    };
    if !note.is_empty() {
        text(&mut prims, nw + 26.0 * s, nlh * 0.1, 12.5 * s, ncol, Face::Sans, note);
    }
    let dot = match status {
        ToolStatus::Running => Dot::Running,
        ToolStatus::Waiting => Dot::Waiting,
        ToolStatus::Ok => Dot::Ok,
        ToolStatus::Failed => Dot::Failed,
    };
    let msz = 12.5 * s;
    let mlh = g.line_height(msz, Face::Mono) * 1.18;
    let adv = g.text_width(msz, "0", Face::Mono).max(1.0);
    let lab = 38.0 * s;
    let bw = (w - lab).max(40.0 * s);
    let mut y = nlh + 6.0 * s;
    // IN: what the call was given
    text(&mut prims, 0.0, y + 1.0 * s, 10.5 * s, DIM, Face::Mono, "IN");
    let in_cols = ((bw - 2.0 * s) / adv).floor().max(4.0) as usize;
    let in_lines: Vec<String> = if expanded {
        let mut v: Vec<String> = input_full.lines().flat_map(|l| chunk_cols(l, in_cols)).collect();
        v.truncate(40);
        if v.is_empty() {
            v.push("(no arguments)".into());
        }
        v
    } else if input.is_empty() {
        vec!["(no arguments)".into()]
    } else {
        vec![clip_cols(input, in_cols)]
    };
    for (i, l) in in_lines.iter().enumerate() {
        text(&mut prims, lab, y + i as f32 * mlh, msz, IN_TEXT, Face::Mono, l.clone());
        sel.push(sel_line(g, lab, y + i as f32 * mlh, mlh, msz, Face::Mono, l, 1));
    }
    y += in_lines.len() as f32 * mlh + 7.0 * s;
    // OUT: the result, in a darker box that shows its first lines
    let pad = (9.0 * s, 7.0 * s);
    let cols = ((bw - 2.0 * pad.0) / adv).floor().max(4.0) as usize;
    let (rows, faded, total, dim): (Vec<String>, bool, usize, bool) = if hidden && output.is_some() {
        (vec!["(hidden)".into()], false, 1, true)
    } else {
        match output {
            None => (vec![if status == ToolStatus::Waiting { "waiting for your decision".into() } else if status == ToolStatus::Running { "running…".into() } else { "(no result)".into() }], false, 1, true),
            Some(o) => {
                let src: Vec<&str> = o.trim_end().lines().collect();
                if src.is_empty() {
                    (vec!["(empty)".into()], false, 1, true)
                } else if expanded {
                    let mut v: Vec<String> = src.iter().flat_map(|l| chunk_cols(l, cols)).collect();
                    let total = v.len();
                    v.truncate(MAX_EXPANDED);
                    (v, false, total, false)
                } else {
                    let rows: Vec<String> = src.iter().take(OUT_LINES).map(|l| clip_cols(l, cols)).collect();
                    (rows, src.len() > OUT_LINES, src.len(), false)
                }
            }
        }
    };
    let foot_h = g.line_height(11.5 * s, Face::Sans) + 4.0 * s;
    let more = total.saturating_sub(rows.len());
    let has_foot = faded || (expanded && (total > OUT_LINES || more > 0));
    let box_h = rows.len() as f32 * mlh + 2.0 * pad.1 + if has_foot { foot_h } else { 0.0 };
    text(&mut prims, 0.0, y + pad.1 + 1.0 * s, 10.5 * s, DIM, Face::Mono, "OUT");
    prims.push(Prim::Fill { x: lab, y, w: bw, h: box_h, r: 8.0 * s, c: OUT_BG });
    prims.push(Prim::Stroke { x: lab, y, w: bw, h: box_h, r: 8.0 * s, bw: 1.0 * s, c: OUT_LINE });
    for (i, l) in rows.iter().enumerate() {
        text(&mut prims, lab + pad.0, y + pad.1 + i as f32 * mlh, msz, if dim { DIM } else { OUT_TEXT }, Face::Mono, l.clone());
        if !dim {
            sel.push(sel_line(g, lab + pad.0, y + pad.1 + i as f32 * mlh, mlh, msz, Face::Mono, l, 2));
        }
    }
    if faded {
        let fh = mlh * 2.2;
        prims.push(Prim::Fade { x: lab + 1.0, y: y + pad.1 + rows.len() as f32 * mlh - fh, w: bw - 2.0, h: fh, c: OUT_BG });
        text(&mut prims, lab + pad.0, y + pad.1 + rows.len() as f32 * mlh + 1.0 * s, 11.5 * s, rgb(110, 160, 255), Face::Sans, format!("show more · {} more line{}", total - rows.len(), if total - rows.len() == 1 { "" } else { "s" }));
    } else if has_foot {
        let t = if more > 0 { format!("show less · {more} lines not shown") } else { "show less".to_string() };
        text(&mut prims, lab + pad.0, y + pad.1 + rows.len() as f32 * mlh + 1.0 * s, 11.5 * s, rgb(110, 160, 255), Face::Sans, t);
    }
    // the name row and the "show more" row open and close it; the text between them can be selected
    prims.push(Prim::Hit { x: 0.0, y: 0.0, w, h: nlh + 3.0 * s, act: Act::Toggle });
    if has_foot {
        prims.push(Prim::Hit { x: lab, y: y + box_h - foot_h, w: bw, h: foot_h, act: Act::Toggle });
    }
    y += box_h;
    BlockLayout { h: y + 8.0 * s, dot: Some((nlh / 2.0, dot)), prims, toggle: Some(String::new()), sel, zones: vec![] }
}

fn layout_approval(g: &mut Gfx, tool: &str, summary: &str, detail: &str, w: f32, s: f32) -> BlockLayout {
    let mut prims = Vec::new();
    let pad = 13.0 * s;
    let hsz = 15.0 * s;
    let hlh = g.line_height(hsz, Face::Bold);
    let inner = w - 2.0 * pad;
    let mut y = pad;
    text(&mut prims, pad, y, hsz, AMBER, Face::Bold, format!("Approval needed · {tool}"));
    let dot_y = y + hlh / 2.0;
    y += hlh + 5.0 * s;
    let bsz = 14.5 * s;
    let blh = g.line_height(bsz, Face::Sans) * 1.2;
    let mut sum = g.wrap(bsz, summary, Face::Sans, inner);
    sum.truncate(4);
    let mut sel = Vec::new();
    for l in &sum {
        text(&mut prims, pad, y, bsz, TEXT, Face::Sans, l.clone());
        sel.push(sel_line(g, pad, y, blh, bsz, Face::Sans, l, 0));
        y += blh;
    }
    y += 4.0 * s;
    let diff: Vec<&str> = detail.lines().take(10).collect();
    if !diff.is_empty() {
        let msz = 12.5 * s;
        let mlh = g.line_height(msz, Face::Mono) * 1.15;
        let adv = g.text_width(msz, "0", Face::Mono).max(1.0);
        let cols = ((inner - 12.0 * s) / adv).floor().max(4.0) as usize;
        let h = diff.len() as f32 * mlh + 10.0 * s;
        prims.push(Prim::Fill { x: pad, y, w: inner, h, r: 8.0 * s, c: rgb(20, 18, 12) });
        for (i, l) in diff.iter().enumerate() {
            let (col, bg) = match l.chars().next() {
                Some('+') => (rgb(170, 240, 180), Some(rgb(26, 56, 38))),
                Some('-') => (rgb(255, 170, 170), Some(rgb(70, 30, 34))),
                Some('@') => (DIM, None),
                _ => (MUTED, None),
            };
            let ly = y + 5.0 * s + i as f32 * mlh;
            if let Some(bg) = bg {
                prims.push(Prim::Fill { x: pad + 3.0 * s, y: ly, w: inner - 6.0 * s, h: mlh, r: 0.0, c: bg });
            }
            let shown = clip_cols(l, cols);
            sel.push(sel_line(g, pad + 8.0 * s, ly, mlh, msz, Face::Mono, &shown, 1));
            text(&mut prims, pad + 8.0 * s, ly, msz, col, Face::Mono, shown);
        }
        y += h + 8.0 * s;
    }
    // the buttons keep their keys: Ctrl+Y approve, Ctrl+U always, Ctrl+N deny
    let size = 14.0 * s;
    let bh = size * 2.1;
    let mut x = pad;
    for (label, c, key) in [("Approve (Ctrl+Y)".to_string(), OK, 'y'), (format!("Always allow {tool} (Ctrl+U)"), TEXT, 'u'), ("Deny (Ctrl+N)".to_string(), ERR, 'n')] {
        let bw = g.text_width(size, &label, Face::Bold) + size * 1.7;
        if x + bw > w - pad && x > pad {
            x = pad;
            y += bh + 6.0 * s;
        }
        prims.push(Prim::Button { x, y, size, label, c, key: Key::Ctrl(key) });
        x += bw + 10.0 * s;
    }
    y += bh + pad;
    prims.insert(0, Prim::Stroke { x: 0.0, y: 0.0, w, h: y, r: 10.0 * s, bw: 1.5 * s, c: AMBER });
    prims.insert(0, Prim::Fill { x: 0.0, y: 0.0, w, h: y, r: 10.0 * s, c: rgb(36, 30, 16) });
    BlockLayout { h: y + 4.0 * s, dot: Some((dot_y, Dot::Waiting)), prims, toggle: None, sel, zones: vec![] }
}

fn layout_image(pm: Option<Arc<tiny_skia::Pixmap>>, path: &str, w: f32, s: f32) -> BlockLayout {
    match pm {
        Some(pm) => {
            let (iw, ih) = (pm.width() as f32, pm.height() as f32);
            let k = (w / iw).min(380.0 * s / ih).min(1.0 * s.max(1.0));
            let (dw, dh) = (iw * k, ih * k);
            BlockLayout { h: dh + 8.0 * s, dot: None, prims: vec![Prim::Image { pm, x: 0.0, y: 0.0, w: dw, h: dh }], toggle: None, ..Default::default() }
        }
        None => {
            let name = path.rsplit('/').next().unwrap_or(path);
            let mut prims = Vec::new();
            text(&mut prims, 0.0, 0.0, 13.0 * s, DIM, Face::Sans, format!("(picture {name} is not available)"));
            BlockLayout { h: 22.0 * s, dot: None, prims, toggle: None, ..Default::default() }
        }
    }
}

/// The lines of a picture's source that fit a box `cols` wide; SVG keeps its lines, a data address is cut into rows.
fn code_rows(src: &PicSrc, cols: usize) -> (Vec<String>, usize) {
    match src.kind {
        PicKind::Svg => {
            let all: Vec<&str> = src.text.lines().collect();
            (all.iter().take(40).map(|l| clip_cols(l, cols)).collect(), all.len().saturating_sub(40))
        }
        PicKind::Data => {
            let chars: Vec<char> = src.text.chars().take(cols.max(8) * 6).collect();
            let rows: Vec<String> = chars.chunks(cols.max(8)).map(|c| c.iter().collect()).collect();
            (rows, src.text.chars().count().saturating_sub(chars.len()))
        }
    }
}

/// A picture the assistant wrote into its answer: the drawing, Copy / Save / Show code under it, and the code itself when
/// opened (or when the picture could not be drawn, so that nothing is lost).
fn layout_picture(g: &mut Gfx, src: &PicSrc, res: &Result<Rendered, String>, expanded: bool, w: f32, s: f32) -> BlockLayout {
    let mut prims = Vec::new();
    let mut zones = Vec::new();
    let svg = src.kind == PicKind::Svg;
    let mut y = 0.0f32;
    match res {
        Ok(r) => {
            let (dw, dh) = (r.pm.width() as f32 * r.k, r.pm.height() as f32 * r.k);
            prims.push(Prim::Fill { x: 0.0, y: 0.0, w: dw, h: dh, r: 4.0 * s, c: rgb(247, 248, 251) });
            prims.push(Prim::Image { pm: r.pm.clone(), x: 0.0, y: 0.0, w: dw, h: dh });
            y += dh + 8.0 * s;
        }
        Err(e) => {
            let note = if svg { format!("SVG could not be drawn: {e}") } else { format!("The picture could not be shown: {e}") };
            let nsz = 13.5 * s;
            let nlh = g.line_height(nsz, Face::Sans) * 1.15;
            for l in g.wrap(nsz, &note, Face::Sans, w) {
                text(&mut prims, 0.0, y, nsz, ERR, Face::Sans, l);
                y += nlh;
            }
            y += 4.0 * s;
        }
    }
    let size = 12.5 * s;
    let bh = size * 2.1;
    let mut x = 0.0f32;
    let copy = if svg {
        "Copy SVG"
    } else if src.text.starts_with("data:") {
        "Copy data URL"
    } else {
        "Copy base64"
    };
    let mut btns: Vec<(String, PicAct)> = vec![(copy.to_string(), PicAct::Copy)];
    if let Ok(r) = res {
        btns.push((if svg { "Save .svg".to_string() } else { format!("Save as .{}", r.ext) }, PicAct::Save));
        let what = if svg { "code" } else { "text" };
        btns.push((format!("{} {what}", if expanded { "Hide" } else { "Show" }), PicAct::Toggle));
    }
    for (label, act) in btns {
        let bw = g.text_width(size, &label, Face::Bold) + size * 1.7;
        prims.push(Prim::PicBtn { x, y, size, label, act });
        x += bw + 8.0 * s;
    }
    let ssz = 12.0 * s;
    let sum = src.summary();
    if x + g.text_width(ssz, &sum, Face::Sans) < w {
        text(&mut prims, x + 4.0 * s, y + (bh - g.line_height(ssz, Face::Sans)) / 2.0, ssz, DIM, Face::Sans, sum);
    }
    y += bh + 6.0 * s;
    if expanded || res.is_err() {
        let msz = 12.0 * s;
        let mlh = g.line_height(msz, Face::Mono) * 1.15;
        let adv = g.text_width(msz, "0", Face::Mono).max(1.0);
        let pad = 8.0 * s;
        let cols = ((w - 2.0 * pad) / adv).floor().max(8.0) as usize;
        let (rows, more) = code_rows(src, cols);
        let nrows = rows.len() + usize::from(more > 0);
        let bh2 = nrows as f32 * mlh + 2.0 * pad;
        prims.push(Prim::Fill { x: 0.0, y, w, h: bh2, r: 8.0 * s, c: OUT_BG });
        prims.push(Prim::Stroke { x: 0.0, y, w, h: bh2, r: 8.0 * s, bw: 1.0 * s, c: OUT_LINE });
        for (i, r) in rows.iter().enumerate() {
            text(&mut prims, pad, y + pad + i as f32 * mlh, msz, OUT_TEXT, Face::Mono, r.clone());
        }
        if more > 0 {
            let unit = if svg { "lines" } else { "characters" };
            text(&mut prims, pad, y + pad + rows.len() as f32 * mlh, msz, DIM, Face::Sans, format!("… {more} more {unit}"));
        }
        zones.push(Zone { x: 0.0, y, w, h: bh2, text: src.text.clone(), code: true });
        y += bh2 + 6.0 * s;
    }
    BlockLayout { h: y + 2.0 * s, dot: None, prims, toggle: res.is_ok().then(String::new), sel: vec![], zones }
}

// ------------------------------------------------------------------ the view

struct Item {
    y: f32,
    l: Rc<BlockLayout>,
    /// Offset from the column's left edge (the timeline gutter for the assistant's blocks).
    x: f32,
    turn: usize,
    /// Index in `toggle_keys` when the block opens and closes.
    toggle: Option<usize>,
    /// Where this block's lines start in the selectable document, and its first copy zone in `copy_texts`.
    first_line: usize,
    first_zone: usize,
    /// Index in `pic_list` for a picture block.
    pic: Option<usize>,
}

/// Where the transcript was drawn last: its rectangle and the y of the first block, for turning pointer positions into text positions.
#[derive(Clone, Copy, Default)]
struct Geo {
    log: Option<R>,
    top: f32,
}

pub struct ChatView {
    cache: HashMap<u64, Rc<BlockLayout>>,
    /// How many blocks were laid out so far (tests watch this to see that a redraw lays nothing out again).
    pub laid: usize,
    key_dims: (u32, u32),
    open: HashSet<String>,
    open_sig: u64,
    turns: Vec<Turn>,
    sigs: Vec<(u64, Vec<u64>)>,
    built_for: Option<u64>,
    items: Vec<Item>,
    toggle_keys: Vec<String>,
    /// The pictures written into answers, in order (the Copy and Save buttons name one by its index).
    pic_list: Vec<PicSrc>,
    pub focus: Option<String>,
    pub total: f32,
    col: (f32, f32),
    reveal: bool,
    /// Everything that can be selected, rebuilt when the layout changes.
    doc: SelDoc,
    doc_key: Option<(u64, (u32, u32), u32)>,
    sel: Selection,
    /// The texts the copy buttons copy.
    copy_texts: Vec<String>,
    geo: std::cell::Cell<Geo>,
    /// While a drag is held outside the transcript: how far to scroll per heartbeat (positive = towards older messages).
    auto: f32,
    ptr: Option<(f32, f32)>,
}

impl Default for ChatView {
    fn default() -> Self {
        Self::new()
    }
}

/// Width of the timeline gutter, in pixels at scale 1.
const GUTTER: f32 = 30.0;
const MAX_COLUMN: f32 = 900.0;

impl ChatView {
    pub fn new() -> ChatView {
        ChatView { cache: HashMap::new(), laid: 0, key_dims: (0, 0), open: HashSet::new(), open_sig: 0, turns: vec![], sigs: vec![], built_for: None, items: vec![], toggle_keys: vec![], pic_list: vec![], focus: None, total: 0.0, col: (0.0, 0.0), reveal: false, doc: SelDoc::new(), doc_key: None, sel: Selection::default(), copy_texts: vec![], geo: std::cell::Cell::new(Geo::default()), auto: 0.0, ptr: None }
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }
    pub fn is_open(&self, key: &str) -> bool {
        self.open.contains(key)
    }

    /// Opens or closes the block with this key.
    pub fn toggle(&mut self, key: &str) {
        if !self.open.remove(key) {
            self.open.insert(key.to_string());
        }
        self.open_sig = self.open.iter().fold(0u64, |a, k| a ^ hash_of(k));
        self.built_for = None;
    }

    /// A click on the n-th block that opens (`Event::Tab(TOGGLE_BASE + n)`).
    pub fn toggle_index(&mut self, n: usize) {
        if let Some(k) = self.toggle_keys.get(n).cloned() {
            self.focus = Some(k.clone());
            self.toggle(&k);
        }
    }

    /// Enter on the block that has the focus; false when nothing is focused.
    pub fn toggle_focused(&mut self) -> bool {
        match self.focus.clone() {
            Some(k) => {
                self.toggle(&k);
                true
            }
            None => false,
        }
    }

    /// Up (-1) and Down (+1) walk over the blocks that open; walking past the ends leaves them.
    pub fn move_focus(&mut self, dir: i32) {
        if self.toggle_keys.is_empty() {
            self.focus = None;
            return;
        }
        let cur = self.focus.as_ref().and_then(|f| self.toggle_keys.iter().position(|k| k == f));
        let n = self.toggle_keys.len() as i32;
        let next = match (cur, dir) {
            (None, d) if d < 0 => n - 1,
            (None, _) => 0,
            (Some(c), d) => (c as i32 + d).clamp(0, n - 1),
        };
        self.focus = Some(self.toggle_keys[next as usize].clone());
        self.reveal = true;
    }

    /// Has the block this key names been drawn (it is in the last layout)?
    /// Is there a block that opens and closes?
    pub fn has_focusable(&self) -> bool {
        !self.toggle_keys.is_empty()
    }

    pub fn has_block(&self, key: &str) -> bool {
        self.toggle_keys.iter().any(|k| k == key)
    }

    /// Lays the transcript out for a column `col_w` wide. Only blocks that changed (or a new width) are laid out again.
    /// Returns the total height.
    pub fn prepare(&mut self, g: &mut Gfx, msgs: &[Message], cx: &Ctx, col_w: f32, s: f32, pics: &mut dyn Pics) -> f32 {
        // rebuild the turns when something they are made of changed
        let mut h = DefaultHasher::new();
        for m in msgs {
            (m.role.as_str(), m.content.len(), m.tool_calls.len(), m.tool_call_id.as_deref()).hash(&mut h);
            m.content.get(..m.content.len().min(24)).hash(&mut h);
            let tail = m.content.len().saturating_sub(24);
            m.content.get(tail..).hash(&mut h);
            for c in &m.tool_calls {
                c.id.hash(&mut h);
            }
        }
        cx.tool_ok.iter().fold(0u64, |a, (k, v)| a ^ hash_of(&(k, v))).hash(&mut h);
        cx.approval.hash(&mut h);
        cx.busy.hash(&mut h);
        self.open_sig.hash(&mut h);
        let sig = h.finish();
        if self.built_for != Some(sig) {
            self.turns = build_turns(msgs, &Ctx { tool_ok: cx.tool_ok, approval: cx.approval, busy: cx.busy, open: &self.open });
            self.sigs = self.turns.iter().map(|t| (hash_of(&t.user), t.blocks.iter().map(hash_of).collect())).collect();
            self.built_for = Some(sig);
        }
        let dims = (col_w.to_bits(), s.to_bits());
        if dims != self.key_dims || self.cache.len() > 4000 {
            self.cache.clear();
            self.key_dims = dims;
        }
        let colw = col_w.min(MAX_COLUMN * s).max(60.0);
        self.col = ((col_w - colw) / 2.0, colw);
        let gut = GUTTER * s;
        let cw = (colw - gut).max(40.0);
        let turns = std::mem::take(&mut self.turns);
        let mut items: Vec<Item> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        let mut pic_list: Vec<PicSrc> = Vec::new();
        let max_pic = cw.min(chatpic::MAX_PIC_W * s);
        let mut y = 0.0f32;
        let cache = &mut self.cache;
        let laid = &mut self.laid;
        let mut fetch = |key: u64, make: &mut dyn FnMut() -> BlockLayout| -> Rc<BlockLayout> {
            if let Some(l) = cache.get(&key) {
                return l.clone();
            }
            *laid += 1;
            let l = Rc::new(make());
            cache.insert(key, l.clone());
            l
        };
        for (ti, turn) in turns.iter().enumerate() {
            let (usig, bsigs) = &self.sigs[ti];
            if let Some((idx, t)) = &turn.user {
                let l = fetch(hash_of(&(usig, 1u8)), &mut || layout_user(g, t, *idx, colw, s));
                let h = l.h;
                items.push(Item { y, l, x: 0.0, turn: ti, toggle: None, first_line: 0, first_zone: 0, pic: None });
                y += h + 14.0 * s;
            }
            for (bi, b) in turn.blocks.iter().enumerate() {
                let sg = bsigs[bi];
                let l = match b {
                    Block::Text { text, msg } => fetch(sg, &mut || layout_text(g, text, *msg, cw, s)),
                    Block::Thinking { text, live, expanded } => fetch(sg, &mut || layout_thinking(g, text, *live, *expanded, cw, s)),
                    Block::Tool { name, status, input, input_full, output, hidden, expanded, .. } => fetch(sg, &mut || layout_tool(g, name, *status, input, input_full, output.as_deref(), *hidden, *expanded, cw, s)),
                    Block::Approval { tool, summary, detail } => fetch(sg, &mut || layout_approval(g, tool, summary, detail, cw, s)),
                    Block::Image { path } => {
                        let pm = pics.file(g, path, max_pic, s);
                        let d = pm.as_ref().map_or(0, |p| p.width() * 7919 + p.height());
                        fetch(hash_of(&(sg, d)), &mut || layout_image(pm.clone(), path, cw, s))
                    }
                    Block::Picture { src, expanded, .. } => {
                        let res = pics.inline(g, src, max_pic, s);
                        let d = match &res {
                            Ok(r) => r.pm.width() * 7919 + r.pm.height(),
                            Err(e) => e.len() as u32,
                        };
                        fetch(hash_of(&(sg, d)), &mut || layout_picture(g, src, &res, *expanded, cw, s))
                    }
                };
                let toggle = l.toggle.as_ref().map(|_| {
                    let k = match b {
                        Block::Tool { key, .. } | Block::Picture { key, .. } => key.clone(),
                        Block::Thinking { live: true, .. } => "think:live".to_string(),
                        _ => format!("think:{ti}"),
                    };
                    keys.push(k);
                    keys.len() - 1
                });
                let pic = if let Block::Picture { src, .. } = b {
                    pic_list.push(src.clone());
                    Some(pic_list.len() - 1)
                } else {
                    None
                };
                let h = l.h;
                items.push(Item { y, l, x: gut, turn: ti, toggle, first_line: 0, first_zone: 0, pic });
                y += h + 3.0 * s;
            }
            y += 22.0 * s;
        }
        self.turns = turns;
        self.items = items;
        self.toggle_keys = keys;
        self.pic_list = pic_list;
        if self.focus.as_ref().is_some_and(|f| !self.toggle_keys.contains(f)) {
            self.focus = None;
        }
        // the last block ends a little above the bottom edge
        self.total = (y - 25.0 * s + 12.0 * s).max(0.0);
        // the selectable text, rebuilt only when the layout changed
        let (mut lines, mut zones) = (0, 0);
        for it in self.items.iter_mut() {
            it.first_line = lines;
            it.first_zone = zones;
            lines += it.l.sel.len();
            zones += it.l.zones.len();
        }
        let key = (self.built_for.unwrap_or(0), dims, self.total.to_bits());
        if self.doc_key != Some(key) {
            self.rebuild_doc();
            self.doc_key = Some(key);
        }
        self.total
    }

    fn rebuild_doc(&mut self) {
        let mut doc = SelDoc::new();
        let mut texts: Vec<String> = Vec::new();
        for (ii, it) in self.items.iter().enumerate() {
            for l in &it.l.sel {
                doc.push(SelLine { y: it.y + l.y, h: l.h, text: l.text.clone(), xs: l.xs.iter().map(|x| it.x + x).collect(), block: ii as u32, para: ii as u32 * 100_000 + l.para });
            }
            texts.extend(it.l.zones.iter().map(|z| z.text.clone()));
        }
        self.doc = doc;
        self.copy_texts = texts;
        self.sel.clamp(&self.doc);
    }

    // ------------------------------------------------------------------ selecting text

    /// The text position under a point given relative to the transcript's rectangle (what `Event::AreaAt` carries).
    fn doc_point(&self, xr: f32, yr: f32) -> (f32, f32) {
        let g = self.geo.get();
        let log_y = g.log.map_or(0.0, |l| l.y);
        (xr - self.col.0, log_y + yr - g.top)
    }

    /// A press in the transcript (`clicks` 1, 2 or 3, `shift` to extend): starts a selection, or selects a word or a paragraph.
    pub fn press(&mut self, xr: f32, yr: f32, clicks: u8, shift: bool) {
        let (x, y) = self.doc_point(xr, yr);
        self.sel.press(&self.doc, x, y, clicks, shift);
        self.auto = 0.0;
        self.ptr = Some((xr, yr));
    }

    /// The pointer moved with the button held; past the top or bottom edge the transcript scrolls (see [`ChatView::autoscroll`]).
    pub fn drag(&mut self, xr: f32, yr: f32) {
        let h = self.geo.get().log.map_or(0.0, |l| l.h);
        self.ptr = Some((xr, yr));
        self.auto = if yr < 0.0 {
            (6.0 + (-yr) * 0.4).min(60.0)
        } else if yr > h {
            -(6.0 + (yr - h) * 0.4).min(60.0)
        } else {
            0.0
        };
        let (x, y) = self.doc_point(xr, yr.clamp(0.0, h));
        self.sel.drag(&self.doc, x, y);
    }

    /// The button was released.
    pub fn release(&mut self) {
        self.auto = 0.0;
        self.ptr = None;
    }

    /// One heartbeat of scrolling while a drag is held past an edge. Returns true when it scrolled.
    pub fn autoscroll(&mut self, scroll_px: &mut f32, max_off: f32) -> bool {
        let (Some((xr, yr)), true) = (self.ptr, self.auto != 0.0) else { return false };
        let Some(log) = self.geo.get().log else { return false };
        let next = (*scroll_px + self.auto).clamp(0.0, max_off.max(0.0));
        if next == *scroll_px {
            return false;
        }
        *scroll_px = next;
        let mut g = self.geo.get();
        g.top = self.top(log, next);
        self.geo.set(g);
        let (x, y) = self.doc_point(xr, yr.clamp(0.0, log.h));
        self.sel.drag(&self.doc, x, y);
        true
    }

    /// Is the point (screen pixels) over the transcript?
    pub fn over(&self, x: f32, y: f32) -> bool {
        self.geo.get().log.is_some_and(|l| l.contains(x as i32, y as i32))
    }

    /// Is there any text to select?
    pub fn has_text(&self) -> bool {
        !self.doc.is_empty()
    }

    /// The lines that can be selected, in order (tests and tools).
    pub fn line_texts(&self) -> Vec<String> {
        self.doc.lines.iter().map(|l| l.text.clone()).collect()
    }

    /// Where character `ch` of selectable line `line` is, as drawn last: relative to the transcript's rectangle (what
    /// `Event::AreaAt` carries) or in screen pixels (where the pointer hovers). Tests and tools.
    pub fn point_of(&self, line: usize, ch: usize, screen: bool) -> Option<(f32, f32)> {
        let l = self.doc.lines.get(line)?;
        let g = self.geo.get();
        let log = g.log?;
        let (x, yr) = (l.xs[ch.min(l.len())] + self.col.0, l.y + l.h / 2.0 + g.top - log.y);
        Some(if screen { (log.x + x, log.y + yr) } else { (x, yr) })
    }

    pub fn has_selection(&self) -> bool {
        !self.sel.is_empty()
    }

    pub fn selected_text(&self) -> Option<String> {
        self.sel.text(&self.doc)
    }

    pub fn clear_selection(&mut self) {
        self.sel.clear();
    }

    pub fn select_all(&mut self) {
        self.sel.select_all(&self.doc);
    }

    /// The picture a Copy or Save button belongs to (`Event::Tab(PIC_BASE + 2n + 0 or 1)` is picture `n`).
    pub fn picture(&self, n: usize) -> Option<&PicSrc> {
        self.pic_list.get(n)
    }

    /// The text of a copy button (`Event::Tab(COPY_BASE + n)`).
    pub fn copy_text(&self, n: usize) -> Option<&str> {
        self.copy_texts.get(n).map(String::as_str)
    }

    /// What can be copied as a whole at a point of the screen: (the message, the code block).
    pub fn zones_at(&self, x: f32, y: f32) -> (Option<String>, Option<String>) {
        let g = self.geo.get();
        let Some(log) = g.log else { return (None, None) };
        let (dx, dy) = (x - log.x - self.col.0, y - g.top);
        let (mut msg, mut code) = (None, None);
        for it in &self.items {
            if dy < it.y || dy >= it.y + it.l.h {
                continue;
            }
            for z in &it.l.zones {
                let (zx, zy) = (it.x + z.x, it.y + z.y);
                if dx >= zx && dx < zx + z.w && dy >= zy && dy < zy + z.h {
                    if z.code {
                        code = Some(z.text.clone());
                    } else {
                        msg = Some(z.text.clone());
                    }
                }
            }
        }
        (msg, code)
    }

    fn top(&self, log: R, scroll_px: f32) -> f32 {
        if self.total < log.h {
            log.y
        } else {
            log.y + log.h - self.total + scroll_px.min(self.total - log.h).max(0.0)
        }
    }

    /// The scroll offset (pixels up from the bottom) that brings the focused block into view; unchanged otherwise.
    pub fn scroll_for_focus(&mut self, log_h: f32, scroll_px: f32) -> f32 {
        if !std::mem::take(&mut self.reveal) || self.total <= log_h {
            return scroll_px;
        }
        let Some(item) = self.focus.as_ref().and_then(|f| self.toggle_keys.iter().position(|k| k == f)).and_then(|n| self.items.iter().find(|i| i.toggle == Some(n))) else { return scroll_px };
        let top = log_h - self.total + scroll_px.min(self.total - log_h).max(0.0) + item.y;
        let bottom = top + item.l.h;
        if top < 0.0 {
            scroll_px - top + 8.0
        } else if bottom > log_h {
            (scroll_px - (bottom - log_h) - 8.0).max(0.0)
        } else {
            scroll_px
        }
    }

    /// Paints what is visible inside `log`. `scroll_px` is how far the person scrolled up from the newest message.
    /// `cev` turns a key into the event a button sends.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, g: &mut Gfx, log: R, scroll_px: f32, tick: u64, s: f32, accent: [u8; 4], cev: &dyn Fn(Key) -> Event) {
        let top = self.top(log, scroll_px);
        self.geo.set(Geo { log: Some(log), top });
        // the whole transcript takes presses for text selection; the toggles and copy buttons registered later sit on top of it
        g.hit(log, Event::Area(AREA), "text area");
        let col_x = log.x + self.col.0;
        let dx = col_x + 9.0 * s;
        // the timeline: one line from a turn's first dot to its last
        let mut span: HashMap<usize, (f32, f32)> = HashMap::new();
        for it in &self.items {
            if let Some((dy, _)) = it.l.dot {
                let y = top + it.y + dy;
                let e = span.entry(it.turn).or_insert((y, y));
                e.0 = e.0.min(y);
                e.1 = e.1.max(y);
            }
        }
        for (a, b) in span.values() {
            if b > a && *b >= log.y - 20.0 && *a <= log.y + log.h + 20.0 {
                g.line(dx, a.max(log.y - 20.0), dx, b.min(log.y + log.h + 20.0), 1.5 * s, TIMELINE);
            }
        }
        let clip = (log.y - 40.0 * s, log.y + log.h + 40.0 * s);
        let phase = (tick % 48) as f32 / 48.0;
        for it in &self.items {
            let oy = top + it.y;
            if oy + it.l.h < clip.0 || oy > clip.1 {
                continue;
            }
            let ox = col_x + it.x;
            let l = &*it.l;
            let wid = self.col.1 - it.x;
            if let Some(n) = it.toggle {
                let r = R::new(ox - 6.0 * s, oy - 3.0 * s, wid + 12.0 * s, l.h);
                if g.hovered(r) {
                    g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, [255, 255, 255, 9]);
                }
                if self.focus.as_ref() == self.toggle_keys.get(n) {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.5 * s, accent);
                }
            }
            for p in &l.prims {
                match p {
                    Prim::Fill { x, y, w, h, r, c } => {
                        if *r > 0.0 {
                            g.rrect(ox + x, oy + y, *w, *h, *r, *c)
                        } else {
                            g.rect(ox + x, oy + y, *w, *h, *c)
                        }
                    }
                    Prim::Stroke { x, y, w, h, r, bw, c } => g.rrect_outline(ox + x, oy + y, *w, *h, *r, *bw, *c),
                    Prim::Text { x, y, size, c, face, text } => {
                        g.text(ox + x, oy + y, *size, *c, text, *face);
                    }
                    Prim::Md { l, x, y, w } => mdtext::draw(g, l, ox + x, oy + y, TEXT, clip, *w),
                    Prim::Fade { x, y, w, h, c } => {
                        let n = 8;
                        for i in 0..n {
                            let a = ((i + 1) as f32 / n as f32 * 235.0) as u8;
                            g.rect(ox + x, oy + y + h * i as f32 / n as f32, *w, h / n as f32 + 1.0, [c[0], c[1], c[2], a]);
                        }
                    }
                    Prim::Image { pm, x, y, w, h } => {
                        let paint = tiny_skia::PixmapPaint { opacity: 1.0, quality: tiny_skia::FilterQuality::Bilinear, ..tiny_skia::PixmapPaint::default() };
                        let pixmap: &tiny_skia::Pixmap = pm;
                        g.pm.draw_pixmap(0, 0, pixmap.as_ref(), &paint, tiny_skia::Transform::from_translate(ox + x, oy + y).pre_scale(w / pm.width() as f32, h / pm.height() as f32), None);
                    }
                    Prim::PicBtn { x, y, size, label, act } => {
                        let ev = match act {
                            PicAct::Copy => it.pic.map(|n| Event::Tab(PIC_BASE + n * 2)),
                            PicAct::Save => it.pic.map(|n| Event::Tab(PIC_BASE + n * 2 + 1)),
                            PicAct::Toggle => it.toggle.map(|n| Event::Tab(TOGGLE_BASE + n)),
                        };
                        if let Some(ev) = ev {
                            g.button(ox + x, oy + y, *size, label, TEXT, ev);
                        }
                    }
                    Prim::Chevron { x, y, size, open } => {
                        let (cx, cy, r) = (ox + x, oy + y, size / 2.0);
                        let pts = if *open { [(cx - r, cy - r * 0.55), (cx + r, cy - r * 0.55), (cx, cy + r * 0.7)] } else { [(cx - r * 0.55, cy - r), (cx - r * 0.55, cy + r), (cx + r * 0.7, cy)] };
                        g.polygon(&pts, DIM);
                    }
                    Prim::Hit { x, y, w, h, act } => {
                        let (y0, y1) = ((oy + y).max(log.y), (oy + y + h).min(log.y + log.h));
                        if y1 > y0 {
                            let r = R::new(ox + x, y0, *w, y1 - y0);
                            match (act, it.toggle) {
                                (Act::Toggle, Some(n)) => g.hit(r, Event::Tab(TOGGLE_BASE + n), "toggle"),
                                _ => {}
                            }
                        }
                    }
                    Prim::Button { x, y, size, label, c, key } => {
                        g.button(ox + x, oy + y, *size, label, *c, cev(key.clone()));
                    }
                }
            }
            // the selection, in a translucent accent over the text
            for (j, sl) in l.sel.iter().enumerate() {
                if let Some((a, b)) = self.sel.span(&self.doc, it.first_line + j) {
                    let (ya, yb) = (oy + sl.y, oy + sl.y + sl.h);
                    if yb >= log.y && ya <= log.y + log.h {
                        g.rrect(col_x + a, ya, (b - a).max(3.0), sl.h, 2.0 * s, [accent[0], accent[1], accent[2], 96]);
                    }
                }
            }
            // a small copy button at the top-right of a hovered message or code block
            for (zi, z) in l.zones.iter().enumerate() {
                let zr = R::new(ox + z.x, oy + z.y, z.w, z.h);
                let vis = R::new(zr.x, zr.y.max(log.y), zr.w, (zr.y + zr.h).min(log.y + log.h) - zr.y.max(log.y));
                if vis.h > 4.0 && g.hovered(vis) {
                    let d = 24.0 * s;
                    // the code block's button sits left of the message's when they share a corner
                    let shift = if z.code && z.y < 26.0 * s { d + 4.0 * s } else { 0.0 };
                    let br = R::new(zr.x + zr.w - d - 6.0 * s - shift, (zr.y + 5.0 * s).max(log.y + 2.0 * s), d, d);
                    let hot = g.hovered(br);
                    g.rrect(br.x, br.y, br.w, br.h, 6.0 * s, if hot { rgb(60, 72, 110) } else { rgb(34, 42, 70) });
                    g.rrect_outline(br.x, br.y, br.w, br.h, 6.0 * s, 1.0, if hot { accent } else { rgb(70, 82, 120) });
                    let (q, o) = (d * 0.3, d * 0.12);
                    g.rrect_outline(br.x + d * 0.3 + o, br.y + d * 0.22, q * 1.5, q * 1.7, 2.0 * s, 1.3 * s, MUTED);
                    g.rrect(br.x + d * 0.22, br.y + d * 0.3 + o, q * 1.5 + 1.0, q * 1.7 + 1.0, 2.0 * s, if hot { rgb(60, 72, 110) } else { rgb(34, 42, 70) });
                    g.rrect_outline(br.x + d * 0.22, br.y + d * 0.3 + o, q * 1.5, q * 1.7, 2.0 * s, 1.3 * s, TEXT);
                    g.hit(br, Event::Tab(COPY_BASE + it.first_zone + zi), "copy");
                }
            }
            if let Some((dy, kind)) = l.dot {
                let (cx, cy, r) = (dx, oy + dy, 4.5 * s);
                let (col, ring) = match kind {
                    Dot::Idle => (rgb(122, 130, 154), false),
                    Dot::Live => (rgb(122, 130, 154), true),
                    Dot::Running => (OK, true),
                    Dot::Waiting => (AMBER, true),
                    Dot::Ok => (OK, false),
                    Dot::Failed => (ERR, false),
                };
                if ring {
                    let a = ((1.0 - phase) * 150.0) as u8;
                    g.ring(cx, cy, r + phase * 6.0 * s, 1.3 * s, [col[0], col[1], col[2], a]);
                }
                g.circle(cx, cy, r, col);
            }
        }
    }

    /// The dots, in order, for tests: (turn, kind).
    pub fn dots(&self) -> Vec<(usize, Dot)> {
        self.items.iter().filter_map(|i| i.l.dot.map(|(_, d)| (i.turn, d))).collect()
    }
    /// The layouts, top to bottom, for tests.
    pub fn layouts(&self) -> Vec<Rc<BlockLayout>> {
        self.items.iter().map(|i| i.l.clone()).collect()
    }
    /// Top and height of every block, relative to the start of the transcript.
    pub fn spans(&self) -> Vec<(f32, f32)> {
        self.items.iter().map(|i| (i.y, i.l.h)).collect()
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::chat::ToolCall;
    use serde_json::json;

    struct Rig {
        g: Gfx,
        v: ChatView,
        ok: HashMap<String, bool>,
        approval: Option<(String, String, String, String)>,
        busy: bool,
    }

    impl Rig {
        fn new() -> Rig {
            Rig { g: Gfx::new(1000, 700), v: ChatView::new(), ok: HashMap::new(), approval: None, busy: false }
        }
        fn prep(&mut self, msgs: &[Message], w: f32) -> f32 {
            let ap = self.approval.clone();
            let cx = Ctx { tool_ok: &self.ok, approval: ap.as_ref().map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str())), busy: self.busy, open: &HashSet::new() };
            let open = self.v.open.clone();
            let cx = Ctx { open: &open, ..cx };
            self.v.prepare(&mut self.g, msgs, &cx, w, 1.0, &mut NoPics)
        }
        fn draw(&mut self, scroll: f32, tick: u64) {
            self.g.clear(rgb(9, 11, 20));
            self.g.clear_hits();
            let log = R::new(20.0, 60.0, 960.0, 560.0);
            self.v.draw(&mut self.g, log, scroll, tick, 1.0, rgb(79, 140, 255), &|k| Event::Key(k));
        }
    }

    /// Knows one picture file.
    struct OnePic(Arc<tiny_skia::Pixmap>);

    impl Pics for OnePic {
        fn file(&mut self, _: &mut Gfx, path: &str, _: f32, _: f32) -> Option<Arc<tiny_skia::Pixmap>> {
            (path == "/tmp/x/image-1.png").then(|| self.0.clone())
        }
        fn inline(&mut self, _: &mut Gfx, _: &PicSrc, _: f32, _: f32) -> Result<Rendered, String> {
            Err("none".into())
        }
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), args }
    }

    fn tool_msgs(out: &str) -> Vec<Message> {
        let c = call("c1", "run_command", json!({"command": "ls", "args": ["-la", "/home"]}));
        vec![Message::system("sys"), Message::user("list my home"), Message::assistant_calls("Let me look.", vec![c.clone()]), Message::tool_result(&c, out), Message::assistant("Done.")]
    }

    #[test]
    fn a_turn_has_the_user_box_first_then_the_answer_as_markdown_with_bullets() {
        let mut r = Rig::new();
        let msgs = vec![Message::system("sys"), Message::user("what can you do?"), Message::assistant("Here is a list:\n\n- **first** item\n- second item with `code`\n\n1. one\n2. two".to_string())];
        let total = r.prep(&msgs, 960.0);
        assert_eq!(r.v.turns().len(), 1);
        assert_eq!(r.v.turns()[0].user.as_ref().unwrap().1, "what can you do?");
        let ls = r.v.layouts();
        assert_eq!(ls.len(), 2, "the box and the answer");
        assert!(ls[0].texts()[0].contains("what can you do?"));
        let md = ls[1].markdown().expect("the answer is Markdown");
        let bullets: Vec<_> = md.lines.iter().filter(|l| l.marker.as_deref() == Some("•")).collect();
        assert_eq!(bullets.len(), 2);
        assert!(bullets[0].indent > 0.0, "a hanging indent");
        assert!(!ls[1].texts().join(" ").contains("**"), "no raw markers");
        assert!(total > ls[0].h + ls[1].h);
        r.draw(0.0, 0);
        assert_eq!(r.g.hits.iter().filter(|h| h.label == "text area").count(), 1, "the transcript is one area for selecting text");
        assert!(r.v.dots().is_empty(), "plain answers have no timeline dot");
    }

    #[test]
    fn thinking_is_collapsed_dimmed_and_opens_only_when_there_is_reasoning() {
        let mut r = Rig::new();
        r.busy = true;
        let msgs = vec![Message::system("s"), Message::user("hi")];
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.dots(), vec![(0, Dot::Live)]);
        let l = r.v.layouts();
        assert_eq!(l[1].texts(), vec!["Thinking"]);
        assert!(l[1].toggle.is_none(), "no reasoning text: it only shows activity");
        // with reasoning: collapsed shows the label only; opening shows the text
        let l = layout_thinking(&mut r.g, "first I check the folder, then I answer", false, false, 600.0, 1.0);
        assert_eq!(l.texts(), vec!["Thinking"]);
        assert!(l.toggle.is_some());
        let open = layout_thinking(&mut r.g, "first I check the folder, then I answer", false, true, 600.0, 1.0);
        assert!(open.texts().iter().any(|t| t.contains("check the folder")) && open.h > l.h);
    }

    #[test]
    fn a_tool_call_shows_its_name_the_in_row_and_an_out_box_cut_after_six_lines() {
        let mut r = Rig::new();
        let out: String = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let msgs = tool_msgs(&out);
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.dots(), vec![(0, Dot::Ok)]);
        let ls = r.v.layouts();
        // user box, "Let me look.", the tool, "Done."
        assert_eq!(ls.len(), 4);
        let t = ls[2].texts();
        assert_eq!(t[0], "run_command");
        assert!(t.contains(&"IN".to_string()) && t.contains(&"OUT".to_string()));
        assert!(t.contains(&"ls -la /home".to_string()), "{t:?}");
        let shown = t.iter().filter(|x| x.starts_with("line ")).count();
        assert_eq!(shown, OUT_LINES);
        assert!(t.iter().any(|x| x.starts_with("show more") && x.contains("14 more lines")), "{t:?}");
        // a result that fits is shown whole, without "show more"
        let mut r2 = Rig::new();
        r2.prep(&tool_msgs("a\nb\nc"), 900.0);
        assert!(!r2.v.layouts()[2].texts().iter().any(|x| x.starts_with("show")));
    }

    #[test]
    fn long_lines_are_clipped_with_an_ellipsis_and_the_tool_arguments_are_summarised() {
        let mut r = Rig::new();
        let long = "x".repeat(400);
        let c = call("c1", "write_file", json!({"path": "/tmp/a.txt", "content": long}));
        let msgs = vec![Message::user("w"), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, long.clone())];
        r.prep(&msgs, 500.0);
        let ls = r.v.layouts();
        let t = ls[1].texts();
        assert!(t.iter().any(|x| x.starts_with("/tmp/a.txt  content=") ), "{t:?}");
        assert!(t.iter().filter(|x| x.ends_with('…')).count() >= 2, "IN and OUT are both cut: {t:?}");
        assert!(ls[1].right_edge() <= 500.0 - GUTTER + 1.0, "{}", ls[1].right_edge());
        let (one, full) = summarize_args(&json!({"command": "git", "args": ["status", "-s"]}));
        assert_eq!(one, "git status -s");
        assert_eq!(full, "args: status -s\ncommand: git");
        assert_eq!(summarize_args(&json!({"query": "weather"})).0, "weather");
        assert_eq!(summarize_args(&Value::Null).0, "");
    }

    #[test]
    fn a_tool_opens_and_closes_with_the_keys_and_with_a_click() {
        let mut r = Rig::new();
        let out: String = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let msgs = tool_msgs(&out);
        r.prep(&msgs, 900.0);
        r.draw(0.0, 0);
        assert!(r.v.focus.is_none() && !r.v.toggle_focused());
        let closed_h = r.v.layouts()[2].h;
        // Down focuses the first block that opens, Enter opens it
        r.v.move_focus(1);
        assert_eq!(r.v.focus.as_deref(), Some("tool:c1"));
        assert!(r.v.toggle_focused());
        r.prep(&msgs, 900.0);
        let t = r.v.layouts()[2].texts();
        assert_eq!(t.iter().filter(|x| x.starts_with("line ")).count(), 20, "everything is shown when open");
        assert!(t.iter().any(|x| x.starts_with("show less")));
        assert!(r.v.layouts()[2].h > closed_h);
        // a click on the block (its Tab event) closes it again
        r.draw(0.0, 0);
        let hit = r.g.hits.iter().find(|h| h.label == "toggle").expect("the tool block is a click target").clone();
        assert!(matches!(hit.ev, Event::Tab(n) if n >= TOGGLE_BASE));
        if let Event::Tab(n) = hit.ev {
            r.v.toggle_index(n - TOGGLE_BASE);
        }
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.layouts()[2].h, closed_h);
        // moving past the ends keeps the focus on the last one; Up from none starts at the end
        r.v.move_focus(1);
        assert_eq!(r.v.focus.as_deref(), Some("tool:c1"));
    }

    #[test]
    fn results_of_private_tools_are_hidden_and_failures_get_a_red_dot() {
        let mut r = Rig::new();
        let c = call("r1", "read_file", json!({"path": "/home/me/secret.txt"}));
        let msgs = vec![Message::user("read"), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, "the secret contents")];
        r.prep(&msgs, 900.0);
        let t = r.v.layouts()[1].texts();
        assert!(t.contains(&"(hidden)".to_string()) && !t.iter().any(|x| x.contains("secret contents")), "{t:?}");
        assert!(t.contains(&"/home/me/secret.txt".to_string()));
        let mut r = Rig::new();
        let c = call("f1", "run_command", json!({"command": "false"}));
        r.ok.insert("f1".into(), false);
        let msgs = vec![Message::user("x"), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, "exit status 1")];
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.dots(), vec![(0, Dot::Failed)]);
        assert!(r.v.layouts()[1].texts().contains(&"failed".to_string()));
    }

    #[test]
    fn a_call_in_progress_runs_and_a_pending_approval_is_an_amber_block_with_the_three_buttons() {
        let mut r = Rig::new();
        let c = call("w1", "write_file", json!({"path": "/tmp/s.txt", "content": "3pm"}));
        let msgs = vec![Message::user("save"), Message::assistant_calls("", vec![c])];
        r.busy = true;
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.dots(), vec![(0, Dot::Running)]);
        r.approval = Some(("w1".into(), "write_file".into(), "write 3 characters to /tmp/s.txt".into(), "+3pm".into()));
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.dots(), vec![(0, Dot::Waiting), (0, Dot::Waiting)], "the call and the question");
        let ap = r.v.layouts().pop().unwrap();
        let b = ap.buttons();
        assert_eq!(b.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["Approve (Ctrl+Y)", "Always allow write_file (Ctrl+U)", "Deny (Ctrl+N)"]);
        assert_eq!(b.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), vec![Key::Ctrl('y'), Key::Ctrl('u'), Key::Ctrl('n')]);
        assert!(ap.texts().iter().any(|t| t.contains("write 3 characters")) && ap.texts().contains(&"+3pm".to_string()));
        // no extra "Thinking" while it waits for the decision
        assert!(!r.v.layouts().iter().any(|l| l.texts() == vec!["Thinking"]));
        r.draw(0.0, 3);
        for want in ["Approve (Ctrl+Y)", "Deny (Ctrl+N)"] {
            assert!(r.g.hits.iter().any(|h| h.label == want), "{want}");
        }
        // narrow: the buttons wrap onto more rows instead of leaving the block
        r.prep(&msgs, 330.0);
        let ap = r.v.layouts().pop().unwrap();
        assert!(ap.h > 150.0);
    }

    #[test]
    fn a_new_message_lays_out_only_itself_and_the_rest_comes_from_the_cache() {
        let mut r = Rig::new();
        let mut msgs = vec![Message::system("s")];
        for i in 0..30 {
            msgs.push(Message::user(format!("question {i}")));
            msgs.push(Message::assistant(format!("answer {i} with **bold** text and a list\n- a\n- b")));
        }
        r.prep(&msgs, 900.0);
        let first = r.v.laid;
        assert_eq!(first, 60);
        let before = r.v.layouts();
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.laid, first, "a redraw lays nothing out again");
        msgs.push(Message::user("one more"));
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.laid, first + 1, "only the new box");
        let after = r.v.layouts();
        assert!(before.iter().zip(after.iter()).all(|(a, b)| Rc::ptr_eq(a, b)), "the earlier blocks are the same objects");
        // the answer grows: only the last block is laid out again
        msgs.push(Message::assistant("typing"));
        r.prep(&msgs, 900.0);
        let n = r.v.laid;
        msgs.last_mut().unwrap().content = "typing, and typing more".into();
        r.prep(&msgs, 900.0);
        assert_eq!(r.v.laid, n + 1);
    }

    #[test]
    fn the_layout_follows_the_width_and_never_draws_outside_it() {
        let mut r = Rig::new();
        let md = "A paragraph with a very long address https://example.test/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb and some more words to wrap.\n\n```\nlet very_long_code_line = call_a_function(with, many, arguments, that, go, on, and, on, and, on);\n```\n- item one that is long enough to wrap around the edge of a narrow column";
        let out = "o".repeat(300);
        let c = call("c1", "run_command", json!({"command": "echo ".to_string() + &"y".repeat(200)}));
        let msgs = vec![Message::user("a long question that has to wrap around ".repeat(8)), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, out), Message::assistant(md.to_string())];
        let wide = r.prep(&msgs, 960.0);
        for w in [700.0f32, 480.0, 320.0, 200.0] {
            let tall = r.prep(&msgs, w);
            assert!(tall > wide || w >= 700.0, "narrower is taller ({w}: {tall} vs {wide})");
            for (i, l) in r.v.layouts().iter().enumerate() {
                assert!(l.right_edge() <= w + 1.0, "block {i} at {w}: {}", l.right_edge());
            }
            r.draw(0.0, 0);
        }
        // the column is capped and centred on a very wide window
        r.prep(&msgs, 2400.0);
        assert!(r.v.col.1 <= MAX_COLUMN + 0.5 && r.v.col.0 > 100.0);
        // scale: bigger text, taller blocks
        let small = r.prep(&msgs, 900.0);
        let cx = Ctx { tool_ok: &HashMap::new(), approval: None, busy: false, open: &HashSet::new() };
        let big = r.v.prepare(&mut r.g, &msgs, &cx, 900.0, 2.0, &mut NoPics);
        assert!(big > small * 1.5);
    }

    #[test]
    fn the_newest_message_is_at_the_bottom_and_scrolling_up_moves_the_view() {
        let mut r = Rig::new();
        let mut msgs = vec![Message::system("s")];
        for i in 0..40 {
            msgs.push(Message::user(format!("question {i}")));
            msgs.push(Message::assistant(format!("answer {i}\n\nmore words on another paragraph")));
        }
        let total = r.prep(&msgs, 960.0);
        let log = R::new(20.0, 60.0, 960.0, 560.0);
        assert!(total > log.h);
        let last = *r.v.spans().last().unwrap();
        let bottom = r.v.top(log, 0.0) + last.0 + last.1;
        assert!((bottom - (log.y + log.h)).abs() <= 13.0, "the newest ends at the bottom edge: {bottom}");
        let up = r.v.top(log, 300.0);
        assert!((up - r.v.top(log, 0.0) - 300.0).abs() < 0.5);
        assert!((r.v.top(log, 1e9) - log.y).abs() < 0.5, "scrolling is limited to the start");
        // a short transcript starts at the top
        let mut r2 = Rig::new();
        r2.prep(&msgs[..3], 960.0);
        assert_eq!(r2.v.top(log, 0.0), log.y);
        // focusing a block that is off screen brings it in
        let c = call("c1", "run_command", json!({"command": "ls"}));
        let mut m2 = msgs.clone();
        m2.insert(2, Message::assistant_calls("", vec![c.clone()]));
        m2.insert(3, Message::tool_result(&c, "x"));
        r.prep(&m2, 960.0);
        r.v.move_focus(1);
        let sp = r.v.scroll_for_focus(log.h, 0.0);
        assert!(sp > 100.0, "scrolled up to the tool near the start: {sp}");
    }

    #[test]
    fn images_from_tool_results_follow_the_call_and_a_missing_one_is_a_note() {
        let mut r = Rig::new();
        let c = call("i1", "generate_image", json!({"prompt": "red"}));
        let msgs = vec![Message::user("draw"), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, "Drew it.\nIMAGE: /tmp/x/image-1.png")];
        let mut pm = tiny_skia::Pixmap::new(200, 100).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(230, 60, 90, 255));
        let pm = Arc::new(pm);
        let cx = Ctx { tool_ok: &HashMap::new(), approval: None, busy: false, open: &HashSet::new() };
        r.v.prepare(&mut r.g, &msgs, &cx, 900.0, 1.0, &mut OnePic(pm.clone()));
        assert!(matches!(r.v.turns()[0].blocks[1], Block::Image { .. }));
        assert!(!r.v.layouts()[1].texts().iter().any(|t| t.contains("IMAGE:")), "the marker line is not shown");
        r.draw(0.0, 0);
        let reds = r.g.rgba().chunks_exact(4).filter(|p| p[0] > 200 && p[1] < 90).count();
        assert!(reds > 5000, "{reds}");
        r.v.prepare(&mut r.g, &msgs, &cx, 900.0, 1.0, &mut NoPics);
        assert!(r.v.layouts()[2].texts()[0].contains("not available"));
    }

    #[test]
    fn odd_input_does_not_panic() {
        let mut r = Rig::new();
        r.busy = true;
        for w in [0.0f32, 10.0, 80.0] {
            r.prep(&[], w);
            r.prep(&[Message::assistant(String::new()), Message::user(String::new()), Message::assistant("```".to_string())], w);
            r.draw(0.0, 5);
        }
    }
}
