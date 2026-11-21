//! Sizes and styles for scripts, kept apart from the page object: the estimate of where boxes would be
//! ([`crate::modules::boxes`]) and what `getComputedStyle` answers, both made from the page's style sheets on the first
//! question after the tree changed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iyakki_web::css::Viewport;
use iyakki_web::style::{self, Computed, Display, StyleSheets};
use iyakki_web::{Fetch, Url};

use super::boxes::{self, Boxes};

/// What the layout needs from the page's surroundings: its address, how to fetch a style sheet under the account's host
/// policy, and the window it is laid out for.
pub struct LayoutEnv {
    pub base: Url,
    pub web: Arc<dyn Fetch>,
    pub allow: Mutex<Box<dyn Fn(&str) -> bool + Send>>,
    pub vp: Viewport,
}

/// Style sheets are fetched at most this many times per page; after that a changed tree is measured with the last sheets.
const MAX_RUNS: u32 = 30;
/// Trees bigger than this are not measured (every size is zero, as it was before).
const MAX_NODES: usize = 60_000;

/// The part of a computed style that scripts ask for.
#[derive(Clone, Debug)]
pub struct CompStyle {
    /// The display above is the page's (or the browser's style sheet's) word, not a guess from the tag.
    pub display_set: bool,
    pub display: &'static str,
    pub color: Option<[u8; 4]>,
    pub bg: Option<[u8; 4]>,
    pub font_px: f32,
    pub bold: bool,
    pub italic: bool,
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub border: [f32; 4],
    pub hidden: bool,
    pub zero_opacity: bool,
    pub overflow_hidden: bool,
    pub align: &'static str,
    pub position: &'static str,
    pub float: &'static str,
    pub border_box: bool,
    pub offsets: [Option<f32>; 4],
    pub line_height: style::LineHeight,
    /// `z-index` when it was given as a number.
    pub z: Option<i32>,
}

/// What the browser's own style sheet says (the subset the layout understands): the defaults of CSS 2.1 and what the web has relied on since.
const UA_CSS: &str = "\
body{margin:8px}\
li,summary{display:list-item}table{display:table}tr{display:table-row}td,th{display:table-cell}thead{display:table-header-group}tbody{display:table-row-group}tfoot{display:table-footer-group}caption{display:table-caption}colgroup{display:table-column-group}col{display:table-column}\
small,sub,sup{font-size:smaller}\
button{text-align:center}\
p,dl,menu,pre,figure{margin:1em 0}\
blockquote{margin:1em 40px}\
figure{margin:1em 40px}\
ul,ol,menu{margin:1em 0;padding-left:40px}\
ul ul,ul ol,ol ul,ol ol,ul menu,menu ul{margin:0}\
dd{margin-left:40px}\
h1{font-size:2em;margin:.67em 0}h2{font-size:1.5em;margin:.83em 0}h3{font-size:1.17em;margin:1em 0}h4{margin:1.33em 0}h5{font-size:.83em;margin:1.67em 0}h6{font-size:.67em;margin:2.33em 0}\
h1,h2,h3,h4,h5,h6,b,strong,th{font-weight:bold}\
i,em,cite,var,dfn,address{font-style:italic}\
a[href]{color:#0000ee}\
pre,code,kbd,samp,tt{font-size:.8125em}\
hr{margin:.5em auto;color:#808080;border-width:1px}\
table{box-sizing:border-box}\
td,th{padding:1px}\
th{text-align:center}\
button,input,select,textarea{font-size:13.3333px}\
button{box-sizing:border-box;padding:1px 6px;border-width:2px;background-color:#efefef}\
input{padding:1px 2px;border-width:2px;background-color:#ffffff}\
textarea,select{background-color:#ffffff}\
";

/// The browser's style sheet, parsed; its selectors count for nothing in the cascade (the page's rules win over every one of them).
fn ua_sheet(vp: &Viewport) -> iyakki_web::css::Sheet {
    let mut order = 0u32;
    let mut sheet = iyakki_web::css::parse_sheet_cached(UA_CSS, vp, &mut order);
    for r in &mut sheet.rules {
        for s in &mut r.selectors {
            s.specificity = (0, 0, 0);
        }
    }
    sheet
}

fn comp_of(c: &Computed) -> CompStyle {
    CompStyle {
        display_set: c.display_kw.is_some() || c.display != Display::Default,
        display: if let Some(k) = c.display_kw { k } else { match c.display {
            Display::None => "none",
            Display::Inline => "inline",
            Display::InlineBlock => "inline-block",
            Display::Flex => "flex",
            Display::Grid => "grid",
            _ => "block",
        } },
        color: c.color,
        bg: c.bg,
        font_px: c.font_px,
        bold: c.bold == Some(true),
        italic: c.italic == Some(true),
        margin: c.margin,
        padding: c.padding,
        border: c.border_w,
        hidden: c.hidden,
        zero_opacity: c.zero_opacity,
        overflow_hidden: c.overflow_hidden,
        position: match c.fx.pos {
            style::Pos::Static => "static",
            style::Pos::Relative => "relative",
            style::Pos::Absolute => "absolute",
            style::Pos::Fixed => "fixed",
            style::Pos::Sticky => "sticky",
        },
        float: match c.fx.float {
            style::Float::None => "none",
            style::Float::Left => "left",
            style::Float::Right => "right",
        },
        border_box: c.border_box,
        offsets: c.fx.off,
        z: c.fx.z_set.then_some(c.fx.z),
        line_height: c.line_height,
        align: match c.align {
            style::Align::Center => "center",
            style::Align::Right => "right",
            _ => "left",
        },
    }
}

pub struct Laid {
    pub rev: u64,
    pub boxes: Boxes,
    pub styles: Vec<Option<CompStyle>>,
}

#[derive(Default)]
pub struct LayoutState {
    pub env: Option<LayoutEnv>,
    pub laid: Option<Laid>,
    pub runs: u32,
    sheets: Mutex<HashMap<String, Option<String>>>,
    /// Web fonts by address (read once; `None` when the file could not be fetched or read).
    font_files: Mutex<HashMap<String, Option<std::sync::Arc<super::webfont::FontMetrics>>>>,
}

impl LayoutState {
    /// The page's web fonts: the `@font-face` rules of its style sheets (the ones fetched and the ones written in `<style>`), each font
    /// fetched at most once (at most 8 files of 4 MiB, under the account's host rules) and read for its widths. A font that cannot be
    /// fetched or read is left out, so its text is measured with the common tables.
    fn load_fonts(&self, env: &LayoutEnv, tree: &iyakki_web::html::Tree) -> super::webfont::FontSet {
        let mut texts: Vec<(String, Url)> = Vec::new();
        if let Ok(m) = self.sheets.lock() {
            for (k, v) in m.iter() {
                if let (Some(t), Ok(u)) = (v, Url::parse(k)) {
                    texts.push((t.clone(), u));
                }
            }
        }
        for n in 0..tree.nodes.len() {
            if tree.tag(n) == Some("style") {
                let t: String = tree.text_of(n);
                texts.push((t, env.base.clone()));
            }
        }
        let mut fonts = HashMap::new();
        for (css, base) in texts {
            for rule in super::webfont::font_face_rules(&css) {
                let (url, key_name) = (rule.url, super::webfont::variant_key(&rule.family, rule.bold, rule.italic));
                if fonts.len() >= 8 || fonts.contains_key(&key_name) {
                    continue;
                }
                let Ok(u) = base.join(&url) else { continue };
                let key = u.to_string();
                let got = match self.font_files.lock().ok().and_then(|m| m.get(&key).cloned()) {
                    Some(g) => g,
                    None => {
                        let g = fetch_font(env, &u).and_then(|b| super::webfont::FontMetrics::parse(&b)).map(std::sync::Arc::new);
                        if let Ok(mut m) = self.font_files.lock() {
                            m.insert(key, g.clone());
                        }
                        g
                    }
                };
                if let Some(f) = got {
                    fonts.insert(key_name, f);
                }
            }
        }
        super::webfont::FontSet::new(fonts)
    }

    /// Bring the boxes up to date with the tree (revision `rev`); false when nothing can be measured.
    pub fn ensure(&mut self, tree: &iyakki_web::html::Tree, rev: u64) -> bool {
        let Some(env) = &self.env else { return false };
        if let Some(l) = &self.laid {
            if l.rev == rev || self.runs >= MAX_RUNS {
                return true;
            }
        }
        if tree.nodes.len() > MAX_NODES {
            return false;
        }
        let memo = &self.sheets;
        let get = |u: &Url| -> Option<String> {
            let key = u.to_string();
            if let Some(v) = memo.lock().ok()?.get(&key) {
                return v.clone();
            }
            let got = fetch_sheet(env, u);
            if let Ok(mut m) = memo.lock() {
                m.insert(key, got.clone());
            }
            got
        };
        // content of shadow roots is shown in place of the host's own children: measure a copy of the tree arranged that way
        let hosts: Vec<(usize, usize)> = (0..tree.nodes.len()).filter_map(|n| tree.attr(n, "data-iyakki-shadow").and_then(|v| v.parse::<usize>().ok()).filter(|&i| i < tree.nodes.len()).map(|sid| (n, sid))).collect();
        let arranged;
        let tree = if hosts.is_empty() {
            tree
        } else {
            let mut t = tree.clone();
            for (host, sid) in hosts {
                for c in t.nodes[host].children.clone() {
                    t.detach(c);
                }
                for c in t.nodes[sid].children.clone() {
                    let _ = t.append(host, c);
                }
            }
            arranged = t;
            &arranged
        };
        let mut sheets = style::collect_sheets(tree, &env.base, &env.vp, &get);
        // the browser's own style sheet comes first and loses to every rule of the page whatever its selectors (as the cascade puts the page's
        // origin above the browser's): headings, links, lists, the body's margin, tables and form controls look as in other browsers
        let ua = ua_sheet(&env.vp);
        let shift = ua.rules.iter().map(|r| r.order).max().map_or(0, |m| m + 1);
        for s in &mut sheets {
            for r in &mut s.rules {
                r.order += shift;
            }
        }
        sheets.insert(0, ua);
        let ss = StyleSheets::new(&sheets);
        let styles = style::compute(tree, &ss, &env.vp);
        let fonts = self.load_fonts(env, tree);
        let boxes = boxes::layout_with_fonts(tree, &styles, &env.vp, false, &fonts);
        let comp = styles
            .iter()
            .enumerate()
            .map(|(i, s)| {
                s.as_ref().map(|c| {
                    let mut cs = comp_of(c);
                    // a margin or padding given as a percentage is told as the pixels it came to: a share of the containing block's width
                    if c.pad_pct.iter().chain(c.margin_pct.iter()).any(|p| p.is_some()) {
                        let p = tree.nodes[i].parent;
                        let inner = match (boxes.rects.get(p).copied().flatten(), styles.get(p).and_then(|x| x.as_ref())) {
                            (Some(r), Some(ps)) => (r.w - ps.padding[1] - ps.padding[3] - ps.border_w[1] - ps.border_w[3]).max(0.0),
                            _ => env.vp.width,
                        };
                        for k in 0..4 {
                            if let Some(v) = c.pad_pct[k] {
                                cs.padding[k] = (inner * v / 100.0).max(0.0);
                            }
                            if let Some(v) = c.margin_pct[k] {
                                cs.margin[k] = inner * v / 100.0;
                            }
                        }
                    }
                    // a picture or a frame is inline to a script, while the layout gives it a box of its own
                    if c.display == Display::Default && matches!(tree.tag(i), Some("img" | "canvas" | "video" | "audio" | "iframe" | "svg" | "embed" | "object")) {
                        cs.display = "inline";
                        cs.display_set = true;
                    }
                    cs
                })
            })
            .collect();
        self.laid = Some(Laid { rev, boxes, styles: comp });
        self.runs += 1;
        true
    }

    /// The elements under a point of the page (page coordinates), the one on top first. Painting order is the one of CSS in short:
    /// boxes inside a positioned box with a `z-index` stack with it; negative `z-index` below the normal flow, then boxes in flow,
    /// then floats, then positioned boxes, then positive `z-index`; later in the document is on top of earlier.
    pub fn hit(&self, tree: &iyakki_web::html::Tree, x: f32, y: f32) -> Vec<usize> {
        let Some(l) = self.laid.as_ref() else { return Vec::new() };
        let mut found: Vec<((i32, u8), usize, usize)> = Vec::new();
        let mut order = 0usize;
        // (node, z of the stacking context it is in)
        let mut stack: Vec<(usize, i32)> = tree.nodes.first().map(|r| r.children.iter().rev().map(|&c| (c, 0)).collect()).unwrap_or_default();
        let mut guard = 0usize;
        while let Some((n, ctx)) = stack.pop() {
            guard += 1;
            if guard > 200_000 || n >= tree.nodes.len() {
                break;
            }
            let st = l.styles.get(n).and_then(|s| s.as_ref());
            let mut z = ctx;
            let mut tier = 0u8;
            if let Some(s) = st {
                if s.position != "static" {
                    tier = 2;
                    if let Some(v) = s.z {
                        z = v;
                    }
                } else if s.float != "none" {
                    tier = 1;
                }
            }
            if let (Some(r), Some(s)) = (l.boxes.rects.get(n).copied().flatten(), st) {
                if !s.hidden && !s.zero_opacity && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h {
                    found.push(((z, tier), order, n));
                }
            }
            order += 1;
            for &c in tree.nodes[n].children.iter().rev() {
                stack.push((c, z));
            }
        }
        found.sort();
        found.into_iter().rev().map(|f| f.2).collect()
    }

    /// The size of the content of a box as scripts see it in `scrollWidth` and `scrollHeight`: the far edges of every box below it, measured
    /// from its top left corner (never less than the box itself).
    pub fn scroll_size(&self, tree: &iyakki_web::html::Tree, n: usize) -> [f32; 2] {
        let Some(l) = self.laid.as_ref() else { return [0.0; 2] };
        let Some(me) = l.boxes.rects.get(n).copied().flatten() else { return [0.0; 2] };
        let (mut right, mut bottom) = (me.x + me.w, me.y + me.h);
        let mut stack: Vec<usize> = tree.nodes.get(n).map(|k| k.children.clone()).unwrap_or_default();
        let mut guard = 0usize;
        while let Some(c) = stack.pop() {
            guard += 1;
            if guard > 200_000 || c >= tree.nodes.len() {
                break;
            }
            if let Some(r) = l.boxes.rects.get(c).copied().flatten() {
                right = right.max(r.x + r.w);
                bottom = bottom.max(r.y + r.h);
            }
            stack.extend(tree.nodes[c].children.iter().copied());
        }
        [right - me.x, bottom - me.y]
    }

    /// `[x, y, width, height, border left, top, right, bottom, 1 if the box is inline, 1 if there is a box]`, all zero for a node with no box.
    pub fn rect(&self, n: usize) -> [f32; 10] {
        let Some(l) = self.laid.as_ref() else { return [0.0; 10] };
        match l.boxes.rects.get(n).copied().flatten() {
            Some(r) => {
                let b = l.styles.get(n).and_then(|s| s.as_ref()).map_or([0.0; 4], |c| c.border);
                [r.x, r.y, r.w, r.h, b[3], b[0], b[1], b[2], (l.boxes.display[n] == "inline") as u8 as f32, 1.0]
            }
            None => [0.0; 10],
        }
    }
}

fn fetch_sheet(env: &LayoutEnv, url: &Url) -> Option<String> {
    let mut cur = url.without_fragment();
    for _ in 0..4 {
        if !env.allow.lock().ok()?(&cur.host) {
            return None;
        }
        let r = env.web.get(&cur).ok()?;
        if (300..400).contains(&r.status) {
            cur = cur.join(r.location.as_deref()?).ok()?.without_fragment();
            continue;
        }
        let ct = r.content_type.to_ascii_lowercase();
        let ok = r.status < 400 && r.body.len() <= style::MAX_SHEET_BYTES && (ct.is_empty() || ct.contains("css") || ct.contains("text/plain"));
        return ok.then(|| String::from_utf8_lossy(&r.body).into_owned());
    }
    None
}

fn fetch_font(env: &LayoutEnv, url: &Url) -> Option<Vec<u8>> {
    let mut cur = url.without_fragment();
    for _ in 0..4 {
        if !env.allow.lock().ok()?(&cur.host) {
            return None;
        }
        let r = env.web.get(&cur).ok()?;
        if (300..400).contains(&r.status) {
            cur = cur.join(r.location.as_deref()?).ok()?.without_fragment();
            continue;
        }
        return (r.status < 400 && r.body.len() <= 4 * 1024 * 1024).then(|| r.body.to_vec());
    }
    None
}

fn rgb(c: [u8; 4]) -> String {
    if c[3] == 255 { format!("rgb({}, {}, {})", c[0], c[1], c[2]) } else { format!("rgba({}, {}, {}, {:.3})", c[0], c[1], c[2], c[3] as f32 / 255.0) }
}

fn px(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{}px", r as i64) } else { format!("{r}px") }
}

/// `getComputedStyle` values for a node, as property name to text.
pub fn computed_json(l: &LayoutState, n: usize) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    let Some(Some(cs)) = l.laid.as_ref().and_then(|l| l.styles.get(n)) else { return serde_json::Value::Object(m) };
    let r = l.rect(n);
    let shown = cs.display != "none";
    let mut put = |k: &str, v: String| {
        m.insert(k.to_string(), serde_json::Value::String(v));
    };
    let shown_as = if cs.display_set { cs.display } else { l.laid.as_ref().map_or(cs.display, |x| x.boxes.display.get(n).copied().filter(|d| *d != "none").unwrap_or(cs.display)) };
    put("display", shown_as.to_string());
    put("visibility", if cs.hidden { "hidden".into() } else { "visible".into() });
    put("color", rgb(cs.color.unwrap_or([0, 0, 0, 255])));
    put("background-color", rgb(cs.bg.unwrap_or([0, 0, 0, 0])));
    put("font-size", px(cs.font_px));
    put("font-weight", if cs.bold { "700".into() } else { "400".into() });
    put("font-style", if cs.italic { "italic".into() } else { "normal".into() });
    put("opacity", if cs.zero_opacity { "0".into() } else { "1".into() });
    put("position", cs.position.into());
    put("float", cs.float.into());
    for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
        put(side, cs.offsets[i].map_or("auto".to_string(), px));
    }
    put("overflow", if cs.overflow_hidden { "hidden".into() } else { "visible".into() });
    put("text-align", cs.align.to_string());
    put("box-sizing", if cs.border_box { "border-box" } else { "content-box" }.into());
    put("z-index", match (cs.z, cs.position) {
        (Some(z), p) if p != "static" => z.to_string(),
        _ => "auto".into(),
    });
    put("line-height", match cs.line_height {
        style::LineHeight::Normal => "normal".to_string(),
        l => px(l.px(cs.font_px, 1.2)),
    });
    put("pointer-events", "auto".into());
    put("transform", "none".into());
    if shown {
        put("width", px((r[2] - cs.padding[1] - cs.padding[3] - cs.border[1] - cs.border[3]).max(0.0)));
        put("height", px((r[3] - cs.padding[0] - cs.padding[2] - cs.border[0] - cs.border[2]).max(0.0)));
    }
    for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
        put(&format!("margin-{side}"), px(cs.margin[i]));
        put(&format!("padding-{side}"), px(cs.padding[i]));
        put(&format!("border-{side}-width"), px(cs.border[i]));
    }
    serde_json::Value::Object(m)
}
