//! The cascade: which declarations apply to which element, and what they come to. Rules are matched with the
//! selector engine of [`crate::domtree`], ordered by importance, specificity and position, and the result is one
//! [`Computed`] per element with the properties a reading browser can use. Inheritance, custom properties
//! (`var(--x)`), shorthands and the `inherit` / `initial` keywords are handled; everything else is ignored.

use std::collections::HashMap;
use std::rc::Rc;

use crate::css::{split_top, media_matches, parse_color, parse_decls, parse_gradient, parse_length, parse_shadow, parse_sheet, Decl, Gradient, Length, Rgba, Rule, Shadow, Sheet, Viewport};
use crate::url::Url;
use crate::domtree::PseudoEl;
use crate::html::{NodeKind, Tree};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    /// The page did not say; the element behaves as its tag does.
    #[default]
    Default,
    None,
    Inline,
    Block,
    /// `inline-block`: an atomic box in the line of text.
    InlineBlock,
    Flex,
    Grid,
}

impl Display {
    /// Sits in a line of text (as `inline` or `inline-block`).
    pub fn is_inline(self) -> bool {
        matches!(self, Display::Inline | Display::InlineBlock)
    }
}

/// One column of a grid as the page sizes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    Px(f32),
    /// `1fr`: a share of what is left.
    Fr(f32),
    /// `auto`, `min-content`...: as wide as the content.
    Content,
}

/// `vertical-align` of an atomic inline box (and of controls): the approximations the line layout makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VAlign {
    #[default]
    Middle,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pos {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    #[default]
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

/// Floats, positioning, shadows, gradients and opacity of one box.
#[derive(Clone, Debug, PartialEq)]
pub struct Fx {
    pub float: Float,
    pub clear: Clear,
    pub pos: Pos,
    /// top, right, bottom, left offsets in pixels (`None`: auto)
    pub off: [Option<f32>; 4],
    pub z: i32,
    pub opacity: f32,
    pub shadow: Option<Shadow>,
    pub gradient: Option<Gradient>,
    pub fit: Fit,
}

impl Default for Fx {
    fn default() -> Self {
        Fx { float: Float::None, clear: Clear::None, pos: Pos::Static, off: [None; 4], z: 0, opacity: 1.0, shadow: None, gradient: None, fit: Fit::Fill }
    }
}

impl Fx {
    /// Anything here changes how a box is laid out or drawn.
    pub fn is_set(&self) -> bool {
        *self != Fx::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transform {
    #[default]
    None,
    Upper,
    Lower,
    Capitalize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    Between,
    Around,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Computed {
    pub display: Display,
    // inherited
    pub color: Option<Rgba>,
    pub font_px: f32,
    /// A font size was given somewhere up the tree (so headings and the like do not use their default).
    pub font_set: bool,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: bool,
    pub align: Align,
    pub transform: Transform,
    pub list_none: bool,
    pub mono: bool,
    pub vars: Rc<HashMap<String, String>>,
    // not inherited
    pub bg: Option<Rgba>,
    /// Takes no part in the page (`visibility:hidden`, `opacity:0`, clipped to nothing, moved off screen).
    pub hidden: bool,
    /// `opacity:0` was asked for (kept visible for fade-ins, but a check box with it is a hidden toggle switch).
    pub zero_opacity: bool,
    /// top, right, bottom, left, in pixels
    pub margin: [f32; 4],
    pub auto_left: bool,
    pub auto_right: bool,
    pub padding: [f32; 4],
    pub border_w: [f32; 4],
    pub border_c: [Rgba; 4],
    pub radius: f32,
    pub width: Length,
    pub max_width: Length,
    pub min_width: Length,
    pub height: Length,
    pub min_height: Length,
    pub valign: VAlign,
    /// `white-space: nowrap` (inherited).
    pub nowrap: bool,
    /// Inside `<center>` (or `text-align:-webkit-center`): boxes and tables with a width centre themselves (inherited).
    pub hcenter: bool,
    pub overflow_hidden: bool,
    pub flex_row: bool,
    pub wrap: bool,
    pub justify: Justify,
    pub gap: f32,
    pub row_gap: f32,
    pub grow: f32,
    pub basis: Length,
    pub grid_cols: u32,
    /// `repeat(auto-fit, minmax(Npx, 1fr))`: the narrowest a column may be.
    pub grid_min: f32,
    /// `grid-template-areas`: the names in each row (`.` is an empty cell), the column sizes, and the area this
    /// element sits in (`grid-area: name`).
    pub grid_areas: Option<Rc<Vec<Vec<String>>>>,
    pub grid_tracks: Option<Rc<Vec<Track>>>,
    pub grid_area: Option<Rc<str>>,
    pub position_off: bool,
    pub fx: Fx,
    /// The raw value of `content` (for `::before`/`::after`; not inherited).
    pub content: Option<Rc<str>>,
}

pub const ROOT_PX: f32 = 16.0;

impl Default for Computed {
    fn default() -> Self {
        Computed {
            display: Display::Default,
            color: None,
            font_px: ROOT_PX,
            font_set: false,
            bold: None,
            italic: None,
            underline: None,
            strike: false,
            align: Align::Left,
            transform: Transform::None,
            list_none: false,
            mono: false,
            vars: Rc::new(HashMap::new()),
            bg: None,
            hidden: false,
            zero_opacity: false,
            margin: [0.0; 4],
            auto_left: false,
            auto_right: false,
            padding: [0.0; 4],
            border_w: [0.0; 4],
            border_c: [[0, 0, 0, 255]; 4],
            radius: 0.0,
            width: Length::Auto,
            max_width: Length::Auto,
            min_width: Length::Auto,
            height: Length::Auto,
            min_height: Length::Auto,
            valign: VAlign::Middle,
            nowrap: false,
            hcenter: false,
            overflow_hidden: false,
            flex_row: true,
            wrap: false,
            justify: Justify::Start,
            gap: 0.0,
            row_gap: 0.0,
            grow: 0.0,
            basis: Length::Auto,
            grid_cols: 0,
            grid_min: 0.0,
            grid_areas: None,
            grid_tracks: None,
            grid_area: None,
            position_off: false,
            fx: Fx::default(),
            content: None,
        }
    }
}

impl Computed {
    /// The element's own starting point: what it inherits from its parent, everything else at its default.
    fn inheriting(parent: &Computed) -> Computed {
        Computed {
            color: parent.color,
            font_px: parent.font_px,
            font_set: parent.font_set,
            bold: parent.bold,
            italic: parent.italic,
            underline: parent.underline,
            strike: parent.strike,
            align: parent.align,
            transform: parent.transform,
            list_none: parent.list_none,
            mono: parent.mono,
            nowrap: parent.nowrap,
            hcenter: parent.hcenter,
            vars: parent.vars.clone(),
            hidden: false,
            ..Computed::default()
        }
    }

    pub fn has_box(&self) -> bool {
        self.bg.is_some_and(|c| c[3] > 0) || self.has_box_without_background()
    }

    pub fn has_box_without_background(&self) -> bool {
        self.border_w.iter().any(|w| *w > 0.0)
            || self.padding.iter().any(|p| *p > 0.0)
            || self.margin.iter().any(|m| *m != 0.0)
            || self.auto_left
            || self.auto_right
            || !matches!(self.width, Length::Auto)
            || !matches!(self.max_width, Length::Auto)
            || self.radius > 0.0
            || self.fx.is_set()
    }
}

/// Every rule of the page's sheets, indexed by the id, class or tag at the right end of each selector.
pub struct StyleSheets {
    rules: Vec<Rule>,
    by_id: HashMap<String, Vec<(usize, usize)>>,
    by_class: HashMap<String, Vec<(usize, usize)>>,
    by_tag: HashMap<String, Vec<(usize, usize)>>,
    universal: Vec<(usize, usize)>,
    /// Selectors ending in `::before` / `::after`.
    pseudo: Vec<(usize, usize, PseudoEl)>,
}

impl StyleSheets {
    pub fn new(sheets: &[Sheet]) -> StyleSheets {
        let mut s = StyleSheets { rules: Vec::new(), by_id: HashMap::new(), by_class: HashMap::new(), by_tag: HashMap::new(), universal: Vec::new(), pseudo: Vec::new() };
        for sh in sheets {
            s.rules.extend(sh.rules.iter().cloned());
        }
        for (ri, r) in s.rules.iter().enumerate() {
            for (si, sel) in r.selectors.iter().enumerate() {
                if sel.never {
                    continue;
                }
                if let Some(which) = sel.pseudo_el {
                    s.pseudo.push((ri, si, which));
                    continue;
                }
                match sel.key() {
                    (Some(id), _, _) => s.by_id.entry(id.to_string()).or_default().push((ri, si)),
                    (None, Some(c), _) => s.by_class.entry(c.to_string()).or_default().push((ri, si)),
                    (None, None, Some(t)) => s.by_tag.entry(t.to_string()).or_default().push((ri, si)),
                    _ => s.universal.push((ri, si)),
                }
            }
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Does any rule style generated content?
    pub fn has_pseudo(&self) -> bool {
        !self.pseudo.is_empty()
    }
}

/// The computed style of every element (`None` for text). Indexed like `tree.nodes`.
pub fn compute(tree: &Tree, sheets: &StyleSheets, vp: &Viewport) -> Vec<Option<Computed>> {
    let mut out: Vec<Option<Computed>> = vec![None; tree.nodes.len()];
    let root = Computed::default();
    let mut stack: Vec<(usize, Computed)> = vec![(0, root)];
    while let Some((n, parent)) = stack.pop() {
        let c = match &tree.nodes[n].kind {
            NodeKind::Element { .. } if n != 0 => element_style(tree, n, &parent, sheets, vp, None),
            NodeKind::Element { .. } => parent.clone(),
            NodeKind::Text(_) => continue,
        };
        for &ch in tree.nodes[n].children.iter().rev() {
            if matches!(tree.nodes[ch].kind, NodeKind::Element { .. }) {
                stack.push((ch, c.clone()));
            }
        }
        out[n] = Some(c);
    }
    out
}

struct Applied<'a> {
    layer: u8,
    spec: (u32, u32, u32),
    order: u32,
    decl: &'a Decl,
}

/// The generated boxes of the page: for each element with a `::before` / `::after` that has a `content`, the text it
/// shows (CSS escapes, `attr()` and quotes resolved) and its computed style (inheriting from the element). Boxes
/// with `content: none`/`normal`, hidden ones and boxes whose text comes out empty are left out, except that an empty
/// string is kept (a block with a background is an empty box).
pub fn compute_pseudo(tree: &Tree, sheets: &StyleSheets, vp: &Viewport, styles: &[Option<Computed>]) -> HashMap<(usize, PseudoEl), (String, Computed)> {
    let mut out = HashMap::new();
    if !sheets.has_pseudo() {
        return out;
    }
    // elements that could have one: those matched by some pseudo rule
    for (n, st) in styles.iter().enumerate() {
        let Some(parent) = st else { continue };
        if n == 0 || st.as_ref().is_some_and(|c| c.display == Display::None) {
            continue;
        }
        for which in [PseudoEl::Before, PseudoEl::After] {
            if !sheets.pseudo.iter().any(|&(ri, si, w)| w == which && tree.pseudo_matches(n, &sheets.rules[ri].selectors[si], which)) {
                continue;
            }
            let c = element_style(tree, n, parent, sheets, vp, Some(which));
            let Some(raw) = c.content.clone() else { continue };
            let Some(text) = content_text(&raw, tree, n) else { continue };
            out.insert((n, which), (text, c));
        }
    }
    out
}

/// The text of a `content` value: strings (with escapes), `attr(name)`, `open-quote` / `close-quote`, joined. `None`
/// for `none`, `normal` or anything unsupported (counters, images).
pub fn content_text(raw: &str, tree: &Tree, n: usize) -> Option<String> {
    let v = raw.trim();
    let lv = v.to_ascii_lowercase();
    if matches!(lv.as_str(), "none" | "normal" | "inherit" | "initial" | "unset" | "") {
        return None;
    }
    let b: Vec<char> = v.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        if ch.is_whitespace() || ch == '/' {
            // `/ "alt text"` after the content is the accessible name: stop there
            if ch == '/' {
                break;
            }
            i += 1;
        } else if ch == '"' || ch == '\'' {
            i += 1;
            while i < b.len() && b[i] != ch {
                if b[i] == '\\' {
                    i += 1;
                    let Some(&e) = b.get(i) else { break };
                    if e.is_ascii_hexdigit() {
                        let mut j = i;
                        let mut code = 0u32;
                        while j < b.len() && j < i + 6 && b[j].is_ascii_hexdigit() {
                            code = code * 16 + b[j].to_digit(16).unwrap_or(0);
                            j += 1;
                        }
                        // one white space ends the escape
                        if b.get(j).is_some_and(|c| c.is_whitespace()) {
                            j += 1;
                        }
                        out.push(char::from_u32(code).filter(|c| *c != '\0').unwrap_or('\u{fffd}'));
                        i = j;
                        continue;
                    } else if e == '\n' {
                        i += 1;
                        continue;
                    }
                    out.push(e);
                    i += 1;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            i += 1;
        } else {
            // an identifier or function
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || matches!(b[i], '-' | '_')) {
                i += 1;
            }
            let name: String = b[start..i].iter().collect::<String>().to_ascii_lowercase();
            if b.get(i) == Some(&'(') {
                let close = b[i..].iter().position(|&c| c == ')').map(|k| i + k)?;
                let arg: String = b[i + 1..close].iter().collect();
                i = close + 1;
                if name == "attr" {
                    let attr = arg.trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                    out.push_str(tree.attr(n, &attr).unwrap_or(""));
                } else {
                    return None;
                }
            } else {
                match name.as_str() {
                    "open-quote" => out.push('\u{201c}'),
                    "close-quote" => out.push('\u{201d}'),
                    "no-open-quote" | "no-close-quote" => {}
                    _ => return None,
                }
            }
        }
    }
    Some(out)
}

fn element_style(tree: &Tree, n: usize, parent: &Computed, sheets: &StyleSheets, vp: &Viewport, pseudo: Option<PseudoEl>) -> Computed {
    let mut c = Computed::inheriting(parent);
    let tag = tree.tag(n).unwrap_or("");
    if pseudo.is_none() {
        presentational(tree, n, tag, parent, &mut c, vp);
    }
    // matching rules
    let mut cands: Vec<(usize, usize)> = Vec::new();
    if let Some(which) = pseudo {
        cands.extend(sheets.pseudo.iter().filter(|p| p.2 == which).map(|p| (p.0, p.1)));
    } else if let Some(id) = tree.attr(n, "id") {
        if let Some(v) = sheets.by_id.get(id) {
            cands.extend(v);
        }
    }
    if let Some(cl) = tree.attr(n, "class") {
        for name in cl.split_whitespace() {
            if let Some(v) = sheets.by_class.get(name) {
                cands.extend(v);
            }
        }
    }
    if let Some(v) = sheets.by_tag.get(tag) {
        cands.extend(v);
    }
    cands.extend(&sheets.universal);
    let mut best: HashMap<usize, (u32, u32, u32)> = HashMap::new();
    for (ri, si) in cands {
        let sel = &sheets.rules[ri].selectors[si];
        if if let Some(which) = pseudo { tree.pseudo_matches(n, sel, which) } else { tree.selector_matches(n, sel) } {
            let e = best.entry(ri).or_insert((0, 0, 0));
            if sel.specificity > *e {
                *e = sel.specificity;
            }
        }
    }
    let inline: Vec<Decl> = if pseudo.is_some() { Vec::new() } else { tree.attr(n, "style").map(parse_decls).unwrap_or_default() };
    let mut list: Vec<Applied> = Vec::new();
    for (ri, spec) in &best {
        let r = &sheets.rules[*ri];
        for d in &r.decls {
            list.push(Applied { layer: if d.important { 2 } else { 0 }, spec: *spec, order: r.order, decl: d });
        }
    }
    for d in &inline {
        list.push(Applied { layer: if d.important { 3 } else { 1 }, spec: (0, 0, 0), order: u32::MAX, decl: d });
    }
    list.sort_by(|a, b| (a.layer, a.spec, a.order).cmp(&(b.layer, b.spec, b.order)));
    // custom properties first, so `var()` can see them whatever the order
    let mut vars: Option<HashMap<String, String>> = None;
    for a in &list {
        if a.decl.prop.starts_with("--") {
            vars.get_or_insert_with(|| (*c.vars).clone()).insert(a.decl.prop.clone(), a.decl.value.clone());
        }
    }
    if let Some(v) = vars {
        c.vars = Rc::new(v);
    }
    let resolved: Vec<(&str, String)> = list
        .iter()
        .filter(|a| !a.decl.prop.starts_with("--"))
        .filter_map(|a| resolve_vars(&a.decl.value, &c.vars, 0).map(|v| (a.decl.prop.as_str(), v)))
        .collect();
    // font size first: every other length may be in `em`
    for (p, v) in &resolved {
        if matches!(*p, "font-size" | "font") {
            apply_font(&mut c, parent, p, v, vp);
        }
    }
    for (p, v) in &resolved {
        if !matches!(*p, "font-size") {
            apply(&mut c, parent, p, v, vp);
        }
    }
    // a box with a width inside `<center>` sits in the middle
    if parent.hcenter
        && !matches!(c.width, Length::Auto)
        && !c.auto_left
        && !c.auto_right
        && c.margin[1] == 0.0
        && c.margin[3] == 0.0
        && matches!(tag, "div" | "p" | "form" | "ul" | "ol" | "section" | "article" | "header" | "footer" | "nav" | "main" | "blockquote" | "fieldset" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    {
        c.auto_left = true;
        c.auto_right = true;
    }
    finish(&mut c);
    c
}

/// What old-style markup says with tags and attributes instead of style sheets (`<center>`, `align`, `bgcolor`,
/// `<font>`, `<u>`, `<small>`...), as the lowest-priority style: the page's own rules override it.
fn presentational(tree: &Tree, n: usize, tag: &str, parent: &Computed, c: &mut Computed, vp: &Viewport) {
    let attr = |k: &str| tree.attr(n, k).map(str::trim).filter(|v| !v.is_empty());
    match tag {
        "center" => {
            c.align = Align::Center;
            c.hcenter = true;
        }
        "u" | "ins" => c.underline = Some(true),
        "s" | "strike" | "del" => c.strike = true,
        "mark" => c.bg = Some([255, 235, 59, 255]),
        "small" | "sub" | "sup" => {
            c.font_px = (parent.font_px * 0.83).max(8.0);
            c.font_set = true;
        }
        "big" => {
            c.font_px = parent.font_px * 1.2;
            c.font_set = true;
        }
        "font" => {
            if let Some(k) = attr("color").and_then(parse_color) {
                c.color = Some(k);
            }
            if let Some(sz) = attr("size") {
                let (rel, digits) = match sz.as_bytes()[0] {
                    b'+' => (1, &sz[1..]),
                    b'-' => (-1, &sz[1..]),
                    _ => (0, sz),
                };
                if let Ok(k) = digits.parse::<i32>() {
                    let level = if rel == 0 { k } else { 3 + rel * k }.clamp(1, 7);
                    c.font_px = [10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0][(level - 1) as usize];
                    c.font_set = true;
                }
            }
        }
        _ => {}
    }
    // in quirks mode (no doctype) a table does not take its surroundings' `text-align`
    if tag == "table" && tree.attr(0, "data-iyakki-quirks").is_some() {
        c.align = Align::Left;
    }
    if matches!(tag, "td" | "th") && tree.attr(0, "data-iyakki-quirks").is_some() {
        c.hcenter = false;
    }
    if matches!(tag, "body" | "table" | "tr" | "td" | "th" | "div" | "p" | "thead" | "tbody") {
        if let Some(k) = attr("bgcolor").and_then(parse_color) {
            c.bg = Some(k);
        }
    }
    if matches!(tag, "div" | "p" | "td" | "th" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "caption" | "thead" | "tbody") {
        match attr("align").map(str::to_ascii_lowercase).as_deref() {
            Some("center") | Some("middle") => c.align = Align::Center,
            Some("right") => c.align = Align::Right,
            Some("left") => c.align = Align::Left,
            _ => {}
        }
    }
    // `width` and `height` of a table part, as pixels or a percentage
    if matches!(tag, "table" | "td" | "th" | "col") {
        if let Some(l) = attr("width").and_then(|w| parse_length(&if w.chars().all(|ch| ch.is_ascii_digit()) { format!("{w}px") } else { w.to_string() }, c.font_px, ROOT_PX, vp)) {
            c.width = l;
        }
    }
}

/// Replace `var(--name, fallback)`; `None` when a variable is not defined and has no fallback.
fn resolve_vars(value: &str, vars: &HashMap<String, String>, depth: u32) -> Option<String> {
    if !value.contains("var(") {
        return Some(value.to_string());
    }
    if depth > 8 {
        return None;
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find("var(") {
        out.push_str(&rest[..i]);
        let body_start = i + 4;
        let mut depth_p = 1;
        let mut end = None;
        for (k, ch) in rest[body_start..].char_indices() {
            match ch {
                '(' => depth_p += 1,
                ')' => {
                    depth_p -= 1;
                    if depth_p == 0 {
                        end = Some(body_start + k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        let inner = &rest[body_start..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let v = match vars.get(name) {
            Some(v) => resolve_vars(v, vars, depth + 1)?,
            None => resolve_vars(fallback?, vars, depth + 1)?,
        };
        out.push_str(&v);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn words(v: &str) -> Vec<&str> {
    // split on spaces outside parentheses
    let mut out = Vec::new();
    let (mut depth, mut start) = (0, None);
    for (i, ch) in v.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&v[s..i]);
                }
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&v[s..]);
    }
    out
}

fn px(v: &str, font: f32, vp: &Viewport) -> Option<f32> {
    match parse_length(v, font, ROOT_PX, vp)? {
        Length::Px(p) => Some(p),
        _ => None,
    }
}

fn color_in(v: &str) -> Option<Rgba> {
    // a colour among the words of a shorthand, or the first colour inside a gradient
    if let Some(c) = parse_color(v) {
        return Some(c);
    }
    if let Some(i) = v.find("gradient(") {
        let inner = &v[i + 9..];
        for part in inner.split(',') {
            for w in words(part.trim_end_matches(')')) {
                if let Some(c) = parse_color(w) {
                    return Some(c);
                }
            }
        }
        return None;
    }
    words(v).into_iter().find_map(parse_color)
}

fn font_size(v: &str, parent: &Computed, vp: &Viewport) -> Option<f32> {
    let lv = v.trim().to_ascii_lowercase();
    let kw = match lv.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "larger" => Some(parent.font_px * 1.2),
        "smaller" => Some(parent.font_px / 1.2),
        "inherit" => Some(parent.font_px),
        "initial" | "unset" => Some(ROOT_PX),
        _ => None,
    };
    if let Some(k) = kw {
        return Some(k);
    }
    match parse_length(&lv, parent.font_px, ROOT_PX, vp)? {
        Length::Px(p) => Some(p),
        Length::Percent(p) => Some(parent.font_px * p / 100.0),
        Length::Auto | Length::Calc { .. } | Length::Pick { .. } => None,
    }
}

fn apply_font(c: &mut Computed, parent: &Computed, prop: &str, v: &str, vp: &Viewport) {
    if prop == "font-size" {
        if let Some(sz) = font_size(v, parent, vp) {
            c.font_px = sz.clamp(0.0, 400.0);
            c.font_set = true;
        }
        return;
    }
    // font: [style] [weight] size[/line-height] family
    let lv = v.to_ascii_lowercase();
    if matches!(lv.as_str(), "inherit" | "initial" | "unset") || lv.starts_with("caption") || lv.starts_with("menu") || lv.starts_with("status-bar") {
        return;
    }
    for w in words(&lv) {
        let size_part = w.split('/').next().unwrap_or(w);
        if let Some(sz) = font_size(size_part, parent, vp) {
            if !matches!(w, "normal" | "bold" | "italic" | "bolder" | "lighter" | "oblique") && !w.chars().all(|ch| ch.is_ascii_digit()) {
                c.font_px = sz.clamp(0.0, 400.0);
                c.font_set = true;
                break;
            }
        }
    }
}

fn weight(v: &str) -> Option<bool> {
    match v {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        n => n.parse::<u32>().ok().map(|w| w >= 600),
    }
}

/// Margin, padding or border-width shorthand: 1 to 4 values.
fn four(v: &str, font: f32, vp: &Viewport) -> Option<[Length; 4]> {
    let ws: Vec<Length> = words(v).into_iter().map(|w| parse_length(w, font, ROOT_PX, vp)).collect::<Option<_>>()?;
    Some(match ws.as_slice() {
        [a] => [*a, *a, *a, *a],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return None,
    })
}

fn px_of(l: Length) -> f32 {
    l.px_part()
}

fn set_side(c: &mut Computed, field: &str, side: usize, l: Length) {
    match field {
        "margin" => {
            c.margin[side] = px_of(l);
            match side {
                1 => c.auto_right = matches!(l, Length::Auto),
                3 => c.auto_left = matches!(l, Length::Auto),
                _ => {}
            }
        }
        _ => c.padding[side] = px_of(l).max(0.0),
    }
}

fn border_parts(c: &Computed, v: &str, vp: &Viewport) -> Option<(f32, Option<Rgba>)> {
    let mut w = None;
    let mut col = None;
    let mut style_none = false;
    for t in words(v) {
        match t {
            "none" | "hidden" => style_none = true,
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {}
            "thin" => w = Some(1.0),
            "medium" => w = Some(3.0),
            "thick" => w = Some(5.0),
            "currentcolor" => col = c.color.or(Some([0, 0, 0, 255])),
            t => {
                if let Some(x) = px(t, c.font_px, vp) {
                    w = Some(x);
                } else if let Some(k) = parse_color(t) {
                    col = Some(k);
                }
            }
        }
    }
    if style_none {
        return Some((0.0, col));
    }
    Some((w.unwrap_or(3.0), col))
}

fn apply(c: &mut Computed, parent: &Computed, prop: &str, raw: &str, vp: &Viewport) {
    let v = raw.trim();
    if prop == "content" {
        c.content = Some(Rc::from(v));
        return;
    }
    let lv = v.to_ascii_lowercase();
    let inherit = lv == "inherit";
    let initial = lv == "initial" || lv == "unset" && !matches!(prop, "color" | "font-weight" | "font-style" | "text-align" | "text-transform" | "list-style" | "list-style-type" | "text-decoration");
    match prop {
        "display" => {
            c.display = match lv.split_whitespace().next().unwrap_or("") {
                "none" => Display::None,
                "block" | "list-item" | "flow-root" | "table" => Display::Block,
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" => Display::Grid,
                "inline-block" => Display::InlineBlock,
                "inline" | "inline-table" => Display::Inline,
                _ => Display::Default,
            };
        }
        "color" => {
            if inherit {
                c.color = parent.color;
            } else if lv == "currentcolor" || lv == "initial" {
                // keeps what it inherited
            } else if let Some(k) = parse_color(&lv) {
                c.color = Some(k);
            }
        }
        "background-image" => c.fx.gradient = if inherit || initial { None } else { parse_gradient(&lv) },
        "float" => {
            c.fx.float = match lv.as_str() {
                "left" | "inline-start" => Float::Left,
                "right" | "inline-end" => Float::Right,
                _ => Float::None,
            }
        }
        "clear" => {
            c.fx.clear = match lv.as_str() {
                "left" | "inline-start" => Clear::Left,
                "right" | "inline-end" => Clear::Right,
                "both" => Clear::Both,
                _ => Clear::None,
            }
        }
        "z-index" => c.fx.z = lv.trim().parse::<i32>().unwrap_or(0).clamp(-10_000, 10_000),
        "box-shadow" => c.fx.shadow = if inherit || initial { None } else { parse_shadow(&lv, c.font_px, vp) },
        "object-fit" => {
            c.fx.fit = match lv.split_whitespace().next() {
                Some("contain") => Fit::Contain,
                Some("cover") => Fit::Cover,
                Some("none") => Fit::None,
                Some("scale-down") => Fit::ScaleDown,
                _ => Fit::Fill,
            }
        }
        "inset" => {
            if let Some(four) = four(&lv, c.font_px, vp) {
                for (i, l) in four.iter().enumerate() {
                    c.fx.off[i] = match l {
                        Length::Px(p) => Some(*p),
                        _ => None,
                    };
                }
            }
        }
        "background-color" | "background" => {
            if prop == "background" {
                c.fx.gradient = if inherit || initial { None } else { parse_gradient(&lv) };
            }
            if inherit || initial || lv == "none" {
                c.bg = None;
            } else if let Some(k) = color_in(&lv) {
                c.bg = Some(k);
            } else if prop == "background" && !lv.contains("url(") {
                c.bg = None;
            }
        }
        "font-weight" => {
            if let Some(b) = weight(&lv) {
                c.bold = Some(b);
            } else if inherit {
                c.bold = parent.bold;
            }
        }
        "font-style" => match lv.as_str() {
            "italic" | "oblique" => c.italic = Some(true),
            "normal" => c.italic = Some(false),
            _ => {}
        },
        "font" => {
            for w in words(&lv) {
                match w {
                    "italic" | "oblique" => c.italic = Some(true),
                    w if weight(w).is_some() && w != "normal" => c.bold = weight(w),
                    _ => {}
                }
            }
            if lv.contains("monospace") || lv.contains("courier") {
                c.mono = true;
            }
        }
        "font-family" => {
            c.mono = ["monospace", "courier", "consolas", "menlo", "monaco", "mono"].iter().any(|f| lv.contains(f));
        }
        "text-decoration" | "text-decoration-line" => {
            if lv.contains("none") {
                c.underline = Some(false);
                c.strike = false;
            } else {
                if lv.contains("underline") {
                    c.underline = Some(true);
                }
                if lv.contains("line-through") {
                    c.strike = true;
                }
            }
        }
        "text-align" => {
            c.align = match lv.as_str() {
                "center" | "-webkit-center" => {
                    c.hcenter = lv == "-webkit-center" || c.hcenter;
                    Align::Center
                }
                "right" | "end" => Align::Right,
                "left" | "start" | "justify" => Align::Left,
                _ => c.align,
            }
        }
        "text-transform" => {
            c.transform = match lv.as_str() {
                "uppercase" => Transform::Upper,
                "lowercase" => Transform::Lower,
                "capitalize" => Transform::Capitalize,
                "none" => Transform::None,
                _ => c.transform,
            }
        }
        "visibility" => c.hidden = matches!(lv.as_str(), "hidden" | "collapse"),
        "opacity" => {
            let t = lv.trim();
            let v = match t.strip_suffix('%') {
                Some(n) => n.trim().parse::<f32>().ok().map(|p| p / 100.0),
                None => t.parse::<f32>().ok(),
            };
            c.zero_opacity = matches!(v, Some(o) if o < 0.05);
            // `opacity:0` is how pages fade content in, so it stays fully shown
            c.fx.opacity = match v {
                Some(o) if o >= 0.05 => o.min(1.0),
                _ => 1.0,
            };
        }
        "list-style" | "list-style-type" => c.list_none = lv.split_whitespace().any(|w| w == "none"),
        "overflow" | "overflow-x" | "overflow-y" => c.overflow_hidden = matches!(lv.split_whitespace().next(), Some("hidden" | "clip")),
        "position" => {
            c.fx.pos = match lv.as_str() {
                "relative" => Pos::Relative,
                "absolute" => Pos::Absolute,
                "fixed" => Pos::Fixed,
                "sticky" | "-webkit-sticky" => Pos::Sticky,
                _ => Pos::Static,
            }
        }
        "left" | "top" | "right" | "bottom" | "inset-inline-start" => {
            let side = match prop {
                "top" => Some(0),
                "right" => Some(1),
                "bottom" => Some(2),
                "left" | "inset-inline-start" => Some(3),
                _ => None,
            };
            if let Some(i) = side {
                c.fx.off[i] = px(&lv, c.font_px, vp);
            }
            if let Some(x) = px(&lv, c.font_px, vp) {
                if x <= -900.0 {
                    c.position_off = true;
                }
            }
        }
        "text-indent" => {
            if let Some(x) = px(&lv, c.font_px, vp) {
                if x <= -900.0 {
                    c.position_off = true;
                }
            }
        }
        "margin" | "padding" => {
            if let Some(four) = four(&lv, c.font_px, vp) {
                for (i, l) in four.iter().enumerate() {
                    set_side(c, prop, i, *l);
                }
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            let (f, side) = prop.split_once('-').unwrap_or((prop, ""));
            let i = ["top", "right", "bottom", "left"].iter().position(|s| *s == side).unwrap_or(0);
            if let Some(l) = parse_length(&lv, c.font_px, ROOT_PX, vp) {
                set_side(c, f, i, l);
            }
        }
        "margin-inline" | "padding-inline" | "margin-block" | "padding-block" => {
            let f = if prop.starts_with("margin") { "margin" } else { "padding" };
            if let Some(four) = four(&lv, c.font_px, vp) {
                let (a, b) = (four[0], four[1]);
                let sides = if prop.ends_with("inline") { [3, 1] } else { [0, 2] };
                set_side(c, f, sides[0], a);
                set_side(c, f, sides[1], b);
            }
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            if let Some((w, col)) = border_parts(c, &lv, vp) {
                let sides: Vec<usize> = match prop {
                    "border-top" => vec![0],
                    "border-right" => vec![1],
                    "border-bottom" => vec![2],
                    "border-left" => vec![3],
                    _ => vec![0, 1, 2, 3],
                };
                for s in sides {
                    c.border_w[s] = w;
                    if let Some(k) = col {
                        c.border_c[s] = k;
                    } else if let Some(k) = c.color {
                        c.border_c[s] = k;
                    }
                }
            }
        }
        "border-width" => {
            if let Some(four) = four(&lv, c.font_px, vp) {
                for (i, l) in four.iter().enumerate() {
                    c.border_w[i] = px_of(*l);
                }
            }
        }
        "border-color" => {
            let cols: Vec<Rgba> = words(&lv).into_iter().filter_map(parse_color).collect();
            let sides: Option<[Rgba; 4]> = match cols.as_slice() {
                [a] => Some([*a; 4]),
                [a, b] => Some([*a, *b, *a, *b]),
                [a, b, d] => Some([*a, *b, *d, *b]),
                [a, b, d, e] => Some([*a, *b, *d, *e]),
                _ => None,
            };
            if let Some(s) = sides {
                c.border_c = s;
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            let i = ["top", "right", "bottom", "left"].iter().position(|s| prop.contains(s)).unwrap_or(0);
            if let Some(x) = px(&lv, c.font_px, vp) {
                c.border_w[i] = x;
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            let i = ["top", "right", "bottom", "left"].iter().position(|s| prop.contains(s)).unwrap_or(0);
            if let Some(k) = parse_color(&lv) {
                c.border_c[i] = k;
            }
        }
        "border-style" => {
            if lv.split_whitespace().all(|w| w == "none" || w == "hidden") {
                c.border_w = [0.0; 4];
            }
        }
        "border-radius" => {
            if let Some(x) = lv.split(['/', ' ']).next().and_then(|w| px(w, c.font_px, vp)) {
                c.radius = x.clamp(0.0, 999.0);
            } else if lv.starts_with("50%") {
                c.radius = 999.0;
            }
        }
        "width" | "inline-size" => c.width = if initial { Length::Auto } else { parse_length(&lv, c.font_px, ROOT_PX, vp).unwrap_or(c.width) },
        "max-width" | "max-inline-size" => c.max_width = if lv == "none" { Length::Auto } else { parse_length(&lv, c.font_px, ROOT_PX, vp).unwrap_or(c.max_width) },
        "min-width" => c.min_width = parse_length(&lv, c.font_px, ROOT_PX, vp).unwrap_or(c.min_width),
        "height" | "max-height" => {
            if let Some(l) = parse_length(&lv, c.font_px, ROOT_PX, vp) {
                if prop == "max-height" && !matches!(l, Length::Px(p) if p <= 1.0) {
                    return;
                }
                c.height = l;
            }
        }
        "min-height" => c.min_height = parse_length(&lv, c.font_px, ROOT_PX, vp).unwrap_or(c.min_height),
        "white-space" => c.nowrap = matches!(lv.as_str(), "nowrap" | "pre"),
        "vertical-align" => {
            c.valign = match lv.as_str() {
                "top" | "text-top" => VAlign::Top,
                "bottom" | "text-bottom" => VAlign::Bottom,
                _ => VAlign::Middle,
            }
        }
        "flex-direction" => c.flex_row = !lv.starts_with("column"),
        "flex-wrap" => c.wrap = lv.starts_with("wrap"),
        "flex-flow" => {
            c.flex_row = !lv.contains("column");
            c.wrap = lv.contains("wrap") && !lv.contains("nowrap");
        }
        "justify-content" => {
            c.justify = match lv.as_str() {
                "center" => Justify::Center,
                "flex-end" | "end" | "right" => Justify::End,
                "space-between" => Justify::Between,
                "space-around" | "space-evenly" => Justify::Around,
                _ => Justify::Start,
            }
        }
        "gap" | "grid-gap" => {
            let ws: Vec<f32> = words(&lv).into_iter().filter_map(|w| px(w, c.font_px, vp)).collect();
            if let Some(a) = ws.first() {
                c.row_gap = *a;
                c.gap = *ws.get(1).unwrap_or(a);
            }
        }
        "column-gap" | "grid-column-gap" => c.gap = px(&lv, c.font_px, vp).unwrap_or(c.gap),
        "row-gap" | "grid-row-gap" => c.row_gap = px(&lv, c.font_px, vp).unwrap_or(c.row_gap),
        "flex" => {
            let ws = words(&lv);
            match lv.as_str() {
                "none" => c.grow = 0.0,
                "auto" => c.grow = 1.0,
                _ => {
                    if let Some(g) = ws.first().and_then(|w| w.parse::<f32>().ok()) {
                        c.grow = g.max(0.0);
                    }
                    if let Some(b) = ws.iter().find_map(|w| parse_length(w, c.font_px, ROOT_PX, vp).filter(|l| !matches!(l, Length::Auto))) {
                        c.basis = b;
                    }
                }
            }
        }
        "flex-grow" => c.grow = lv.parse::<f32>().unwrap_or(c.grow).max(0.0),
        "flex-basis" => c.basis = parse_length(&lv, c.font_px, ROOT_PX, vp).unwrap_or(c.basis),
        "grid-template-areas" => c.grid_areas = grid_areas(v),
        "grid-area" => c.grid_area = Some(v).filter(|a| !a.is_empty() && !a.contains('/') && *a != "auto").map(Rc::from),
        "grid-template" => {
            // `rows / columns`, possibly with the area names in strings
            if v.contains('"') || v.contains('\'') {
                c.grid_areas = grid_areas(v);
            }
            if let Some(cols) = split_top(v, b'/').get(1) {
                c.grid_tracks = grid_tracks(cols, c.font_px, vp);
                let n = c.grid_tracks.as_ref().map_or(0, |t| t.len() as u32);
                if n > 0 {
                    c.grid_cols = n.clamp(1, 12);
                }
            }
        }
        "grid-template-columns" => {
            c.grid_tracks = grid_tracks(&lv, c.font_px, vp);
            if let Some(i) = lv.find("repeat(") {
                let inner = lv[i + 7..].trim_end_matches(')');
                let (count, rest) = inner.split_once(',').unwrap_or((inner, ""));
                match count.trim().parse::<u32>() {
                    Ok(n) => c.grid_cols = n.clamp(1, 12),
                    Err(_) => {
                        // auto-fit / auto-fill with minmax(Npx, 1fr)
                        c.grid_cols = 0;
                        if let Some(m) = rest.find("minmax(") {
                            let min = rest[m + 7..].split(',').next().unwrap_or("");
                            c.grid_min = px(min.trim(), c.font_px, vp).unwrap_or(0.0);
                        }
                    }
                }
            } else {
                let n = words(&lv).iter().filter(|w| !w.starts_with('[')).count() as u32;
                c.grid_cols = n.clamp(1, 12);
            }
        }
        _ => {}
    }
}

/// The rows of `grid-template-areas: 'a b' 'c d'` as lists of names.
fn grid_areas(v: &str) -> Option<Rc<Vec<Vec<String>>>> {
    let mut rows = Vec::new();
    let mut rest = v;
    while let Some(i) = rest.find(['"', '\'']) {
        let q = rest.as_bytes()[i] as char;
        let after = &rest[i + 1..];
        let Some(j) = after.find(q) else { break };
        rows.push(after[..j].split_whitespace().map(str::to_string).collect::<Vec<_>>());
        rest = &after[j + 1..];
    }
    let n = rows.first().map_or(0, Vec::len);
    (n > 0 && rows.iter().all(|r| r.len() == n)).then(|| Rc::new(rows))
}

/// The column sizes of `grid-template-columns`; `None` when something in it is not understood (`repeat()`...).
fn grid_tracks(v: &str, font: f32, vp: &Viewport) -> Option<Rc<Vec<Track>>> {
    let mut out = Vec::new();
    for w in words(v.trim()) {
        if w.starts_with('[') {
            continue;
        }
        let w = w.to_ascii_lowercase();
        let t = if let Some(n) = w.strip_suffix("fr") {
            Track::Fr(n.parse::<f32>().ok()?.max(0.0))
        } else if let Some(inner) = w.strip_prefix("minmax(").and_then(|r| r.strip_suffix(')')) {
            let parts = split_top(inner, b',');
            let max = parts.get(1)?.trim();
            if let Some(n) = max.strip_suffix("fr") {
                Track::Fr(n.parse::<f32>().ok()?.max(0.0))
            } else if let Some(p) = px(max, font, vp) {
                Track::Px(p)
            } else {
                Track::Content
            }
        } else if matches!(w.as_str(), "auto" | "min-content" | "max-content") || w.starts_with("fit-content(") {
            Track::Content
        } else if let Some(p) = px(&w, font, vp) {
            Track::Px(p)
        } else {
            return None;
        };
        out.push(t);
    }
    (!out.is_empty()).then(|| Rc::new(out))
}

/// Things that depend on several properties at once.
fn finish(c: &mut Computed) {
    let tiny = |l: Length| matches!(l, Length::Px(p) if p <= 1.0);
    if c.overflow_hidden && (tiny(c.height) || tiny(c.width)) {
        c.hidden = true;
    }
    if c.position_off {
        c.hidden = true;
    }
    if c.font_set && c.font_px < 1.0 {
        c.hidden = true;
    }
}

/// Limits for the style sheets of one page.
pub const MAX_SHEETS: usize = 40;
pub const MAX_SHEET_BYTES: usize = 2560 * 1024;
pub const MAX_TOTAL_BYTES: usize = 6 * 1024 * 1024;

/// What a page asks for in the way of style: `<style>` text and `<link rel=stylesheet>` addresses, in page order, and
/// the pieces fetched for them with `get` (which does the policy, cookie and size checks). `@import`s are followed one
/// level deep. Sheets that do not apply to a screen (`media="print"`) are left out.
pub fn collect_sheets(tree: &Tree, base: &Url, vp: &Viewport, get: &(dyn Fn(&Url) -> Option<String> + Sync)) -> Vec<Sheet> {
    enum Piece {
        Inline(String),
        Link(Url),
    }
    let mut pieces: Vec<Piece> = Vec::new();
    // a <base href> changes what relative addresses mean
    let base = tree.find("base").and_then(|b| tree.attr(b, "href")).and_then(|h| base.join(h).ok()).unwrap_or_else(|| base.clone());
    let mut todo: Vec<usize> = vec![0];
    let mut order: Vec<usize> = Vec::new();
    while let Some(n) = todo.pop() {
        order.push(n);
        todo.extend(tree.nodes[n].children.iter().rev());
    }
    for n in order {
        match tree.tag(n) {
            Some("style") => {
                if tree.attr(n, "media").is_none_or(|m| media_matches(m, vp)) && tree.attr(n, "type").is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("text/css")) {
                    let text: String = tree.nodes[n].children.iter().filter_map(|&c| if let NodeKind::Text(t) = &tree.nodes[c].kind { Some(t.as_str()) } else { None }).collect();
                    pieces.push(Piece::Inline(text));
                }
            }
            Some("link") => {
                let rel = tree.attr(n, "rel").unwrap_or("").to_ascii_lowercase();
                let is_sheet = rel.split_whitespace().any(|r| r == "stylesheet") && !rel.split_whitespace().any(|r| r == "alternate");
                if is_sheet && tree.attr(n, "disabled").is_none() && tree.attr(n, "media").is_none_or(|m| media_matches(m, vp)) {
                    if let Some(u) = tree.attr(n, "href").and_then(|h| base.join(h).ok()) {
                        // the same sheet linked twice (a head and a body that repeat each other) counts once
                        if !pieces.iter().any(|p| matches!(p, Piece::Link(v) if *v == u)) {
                            pieces.push(Piece::Link(u));
                        }
                    }
                }
            }
            _ => {}
        }
        if pieces.len() >= MAX_SHEETS {
            break;
        }
    }
    // fetch every linked sheet at once (a few at a time)
    let links: Vec<(usize, Url)> = pieces.iter().enumerate().filter_map(|(i, p)| if let Piece::Link(u) = p { Some((i, u.clone())) } else { None }).collect();
    let mut texts: Vec<Option<String>> = vec![None; pieces.len()];
    for chunk in links.chunks(6) {
        let got: Vec<Option<String>> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk.iter().map(|(_, u)| sc.spawn(move || get(u))).collect();
            hs.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        for ((i, _), t) in chunk.iter().zip(got) {
            texts[*i] = t;
        }
    }
    let mut out = Vec::new();
    let mut order_no = 0u32;
    let mut total = 0usize;
    for (i, p) in pieces.into_iter().enumerate() {
        let (text, at) = match p {
            Piece::Inline(t) => (Some(t), base.clone()),
            Piece::Link(u) => (texts[i].take(), u),
        };
        let Some(text) = text else { continue };
        if text.len() > MAX_SHEET_BYTES || total + text.len() > MAX_TOTAL_BYTES {
            continue;
        }
        total += text.len();
        let start = order_no;
        let mut sheet = parse_sheet(&text, vp, &mut order_no);
        // imports come first (a sheet's own rules override what it imports)
        let imports: Vec<Url> = sheet.imports.iter().filter_map(|h| at.join(h).ok()).collect();
        let imported: Vec<Option<String>> = std::thread::scope(|sc| {
            let hs: Vec<_> = imports.iter().map(|u| sc.spawn(move || get(u))).collect();
            hs.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        for t in imported.into_iter().flatten() {
            if t.len() <= MAX_SHEET_BYTES && total + t.len() <= MAX_TOTAL_BYTES {
                total += t.len();
                out.push(parse_sheet(&t, vp, &mut order_no));
            }
        }
        // the importing sheet's own rules come after the imported ones in cascade order
        let after_imports = order_no;
        for r in &mut sheet.rules {
            r.order = r.order - start + after_imports;
        }
        order_no = after_imports + (sheet.rules.last().map_or(0, |r| r.order - after_imports));
        out.push(sheet);
    }
    out
}

/// A page with its style sheets fetched through `get` and applied.
pub fn styled_doc(tree: &Tree, url: &Url, vp: &Viewport, get: &(dyn Fn(&Url) -> Option<String> + Sync)) -> crate::doc::Doc {
    let sheets = collect_sheets(tree, url, vp, get);
    crate::doc::build_styled(tree, &StyleSheets::new(&sheets), vp)
}

/// Relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Rgba) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `top` laid over `under` (alpha blending; the result is opaque).
pub fn over(top: Rgba, under: Rgba) -> Rgba {
    let a = top[3] as f32 / 255.0;
    let m = |t: u8, u: u8| (t as f32 * a + u as f32 * (1.0 - a)).round() as u8;
    [m(top[0], under[0]), m(top[1], under[1]), m(top[2], under[2]), 255]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_sheet;
    use crate::html::parse;

    fn styles(html: &str, css: &str) -> (Tree, Vec<Option<Computed>>) {
        let tree = parse(html);
        let vp = Viewport::default();
        let mut n = 0;
        let sheet = parse_sheet(css, &vp, &mut n);
        let c = compute(&tree, &StyleSheets::new(&[sheet]), &vp);
        (tree, c)
    }

    fn of<'a>(t: &Tree, c: &'a [Option<Computed>], id: &str) -> &'a Computed {
        let n = (0..t.nodes.len()).find(|&i| t.attr(i, "id") == Some(id)).unwrap();
        c[n].as_ref().unwrap()
    }

    #[test]
    fn specificity_order_importance_and_inline_decide() {
        let css = "p{color:red} .a{color:blue} #x{color:green} p{color:#111111} .b{color:#222222 !important} ";
        let (t, c) = styles("<p id=x class=a>one</p><p id=y class=a>two</p><p id=z class=b style='color:#333333'>three</p><p id=w style='color:#444444'>four</p>", css);
        assert_eq!(of(&t, &c, "x").color, Some([0, 128, 0, 255]), "an id beats a class beats a tag");
        assert_eq!(of(&t, &c, "y").color, Some([0, 0, 255, 255]));
        assert_eq!(of(&t, &c, "z").color, Some([0x22, 0x22, 0x22, 255]), "important beats inline");
        assert_eq!(of(&t, &c, "w").color, Some([0x44, 0x44, 0x44, 255]), "inline beats a plain rule");
    }

    #[test]
    fn inheritance_and_what_is_not_inherited() {
        let css = "#a{color:red;font-size:20px;background:#102030;padding:4px;font-weight:bold;text-align:center}";
        let (t, c) = styles("<div id=a><span id=b>x</span></div>", css);
        let b = of(&t, &c, "b");
        assert_eq!((b.color, b.font_px, b.bold, b.align), (Some([255, 0, 0, 255]), 20.0, Some(true), Align::Center));
        assert_eq!((b.bg, b.padding), (None, [0.0; 4]), "backgrounds and padding stay on their element");
    }

    #[test]
    fn em_and_percent_follow_the_parent_font_and_shorthands_expand() {
        let css = "#a{font-size:10px} #b{font-size:2em;margin:1em 2em 3em;padding:0 5px} #c{font-size:150%;border:2px solid #ff0000;border-left:0}";
        let (t, c) = styles("<div id=a><p id=b><i id=c>x</i></p></div>", css);
        let b = of(&t, &c, "b");
        assert_eq!(b.font_px, 20.0);
        assert_eq!(b.margin, [20.0, 40.0, 60.0, 40.0]);
        assert_eq!(b.padding, [0.0, 5.0, 0.0, 5.0]);
        let cc = of(&t, &c, "c");
        assert_eq!(cc.font_px, 30.0);
        assert_eq!(cc.border_w, [2.0, 2.0, 2.0, 0.0]);
        assert_eq!(cc.border_c[0], [255, 0, 0, 255]);
    }

    #[test]
    fn custom_properties_with_fallbacks_and_inheritance() {
        let css = ":root{--main:#0000ff;--gap:8px} #a{color:var(--main);margin:var(--gap)} #b{color:var(--nope, red)} #c{color:var(--nope)} #d{--main:#00ff00;color:var(--main)} .p{--x:var(--main)} #e{color:var(--x)}";
        let (t, c) = styles("<html id=r><body><p id=a>1</p><p id=b>2</p><p id=c>3</p><div id=d><i id=d2>4</i></div><div class=p><p id=e>5</p></div></body></html>", css);
        assert_eq!(of(&t, &c, "a").color, Some([0, 0, 255, 255]));
        assert_eq!(of(&t, &c, "a").margin[0], 8.0);
        assert_eq!(of(&t, &c, "b").color, Some([255, 0, 0, 255]));
        assert_eq!(of(&t, &c, "c").color, None, "an undefined variable without a fallback drops the declaration");
        assert_eq!(of(&t, &c, "d").color, Some([0, 255, 0, 255]));
        assert_eq!(of(&t, &c, "d2").color, Some([0, 255, 0, 255]), "children inherit the colour");
        assert_eq!(of(&t, &c, "e").color, Some([0, 0, 255, 255]), "a variable defined through another one");
    }

    #[test]
    fn display_none_and_the_hiding_tricks() {
        let css = ".n{display:none} .v{visibility:hidden} .o{opacity:0} .sr{position:absolute;width:1px;height:1px;overflow:hidden} .off{position:absolute;left:-9999px} .z{font-size:0}";
        let (t, c) = styles("<p id=a class=n>a</p><p id=b class=v>b</p><p id=c class=o>c</p><p id=d class=sr>d</p><p id=e class=off>e</p><p id=f class=z>f</p><p id=g>g</p>", css);
        assert_eq!(of(&t, &c, "a").display, Display::None);
        assert!(!of(&t, &c, "c").hidden, "opacity:0 is how pages fade content in, so the text stays");
        for id in ["b", "d", "e", "f"] {
            assert!(of(&t, &c, id).hidden, "{id}");
        }
        assert!(!of(&t, &c, "g").hidden);
    }

    #[test]
    fn flex_grid_widths_and_centering() {
        let css = ".row{display:flex;justify-content:space-between;gap:12px} .col{flex:1 1 0} .box{width:50%;max-width:600px;margin:0 auto} .g{display:grid;grid-template-columns:repeat(3,1fr)} .f{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr))}";
        let (t, c) = styles("<div id=a class=row><div id=b class=col>x</div></div><div id=m class=box></div><div id=g class=g></div><div id=f class=f></div>", css);
        let a = of(&t, &c, "a");
        assert_eq!((a.display, a.justify, a.gap), (Display::Flex, Justify::Between, 12.0));
        assert_eq!(of(&t, &c, "b").grow, 1.0);
        let m = of(&t, &c, "m");
        assert_eq!((m.width, m.max_width, m.auto_left, m.auto_right), (Length::Percent(50.0), Length::Px(600.0), true, true));
        assert_eq!(of(&t, &c, "g").grid_cols, 3);
        assert_eq!((of(&t, &c, "f").grid_cols, of(&t, &c, "f").grid_min), (0, 200.0));
    }

    #[test]
    fn text_properties_and_keywords() {
        let css = "#a{text-decoration:none;font:italic bold 18px/1.4 Georgia,serif;text-transform:uppercase} #b{text-decoration:underline line-through;font-family:'Courier New',monospace} #c{font-weight:300;font-style:oblique;font-size:larger} #d{color:inherit;font-weight:inherit} #p{font-weight:700}";
        let (t, c) = styles("<div id=p><i id=a>a</i><i id=b>b</i><i id=c>c</i><i id=d>d</i></div>", css);
        let a = of(&t, &c, "a");
        assert_eq!((a.underline, a.italic, a.bold, a.font_px, a.transform), (Some(false), Some(true), Some(true), 18.0, Transform::Upper));
        let b = of(&t, &c, "b");
        assert_eq!((b.underline, b.strike, b.mono), (Some(true), true, true));
        let cc = of(&t, &c, "c");
        assert_eq!((cc.bold, cc.italic), (Some(false), Some(true)));
        assert!((cc.font_px - 19.2).abs() < 0.01);
        assert_eq!(of(&t, &c, "d").bold, Some(true), "inherit");
    }

    #[test]
    fn selectors_with_combinators_and_pseudo_classes_apply() {
        let css = "ul > li:first-child{color:red} li + li{color:blue} nav a{color:green} a:hover{color:pink} li:nth-child(odd){margin-top:5px} p:not(.x){padding:2px}";
        let (t, c) = styles("<ul><li id=a>1</li><li id=b>2</li><li id=c>3</li></ul><nav><a id=l href=x>l</a></nav><p id=p1>p</p><p id=p2 class=x>q</p>", css);
        assert_eq!(of(&t, &c, "a").color, Some([255, 0, 0, 255]));
        assert_eq!(of(&t, &c, "b").color, Some([0, 0, 255, 255]));
        assert_eq!(of(&t, &c, "l").color, Some([0, 128, 0, 255]), ":hover never matches");
        assert_eq!((of(&t, &c, "a").margin[0], of(&t, &c, "b").margin[0], of(&t, &c, "c").margin[0]), (5.0, 0.0, 5.0));
        assert_eq!((of(&t, &c, "p1").padding[0], of(&t, &c, "p2").padding[0]), (2.0, 0.0));
    }

    #[test]
    fn style_and_link_elements_are_collected_in_order_and_imports_come_first() {
        let html = "<head><link rel=stylesheet href=/a.css><style>p{color:#030303}</style><link rel='stylesheet' href=b.css media=print><link rel=icon href=/i.png><link rel='alternate stylesheet' href=/alt.css><style media=print>p{color:red}</style></head><body><p id=x>t</p></body>";
        let tree = parse(html);
        let base = Url::parse("https://e.org/dir/page").unwrap();
        let asked = std::sync::Mutex::new(Vec::<String>::new());
        let get = |u: &Url| {
            asked.lock().unwrap().push(u.to_string());
            match u.path.as_str() {
                "/a.css" => Some("@import url('imp.css'); p{color:#010101}".to_string()),
                "/imp.css" => Some("p{color:#020202; margin:3px}".to_string()),
                _ => None,
            }
        };
        let vp = Viewport::default();
        let sheets = collect_sheets(&tree, &base, &vp, &get);
        let mut a = asked.lock().unwrap().clone();
        a.sort();
        assert_eq!(a, vec!["https://e.org/a.css", "https://e.org/imp.css"], "print and alternate sheets are never fetched");
        let c = compute(&tree, &StyleSheets::new(&sheets), &vp);
        let n = (0..tree.nodes.len()).find(|&i| tree.attr(i, "id") == Some("x")).unwrap();
        let x = c[n].as_ref().unwrap();
        // order: imp.css, a.css, then the <style> element: the later one wins
        assert_eq!(x.color, Some([3, 3, 3, 255]));
        assert_eq!(x.margin[0], 3.0, "an imported rule that nobody overrides still applies");
    }

    #[test]
    fn oversized_and_unreachable_sheets_are_skipped() {
        let tree = parse("<link rel=stylesheet href=/big.css><link rel=stylesheet href=/gone.css><link rel=stylesheet href=/ok.css>");
        let base = Url::parse("https://e.org/").unwrap();
        let get = |u: &Url| match u.path.as_str() {
            "/big.css" => Some("a{color:red}".repeat(MAX_SHEET_BYTES)),
            "/ok.css" => Some("a{color:blue}".into()),
            _ => None,
        };
        let sheets = collect_sheets(&tree, &base, &Viewport::default(), &get);
        assert_eq!(sheets.len(), 1);
    }

    #[test]
    fn contrast_and_blending() {
        assert!(contrast([0, 0, 0, 255], [255, 255, 255, 255]) > 20.0);
        assert!(contrast([20, 20, 20, 255], [30, 30, 30, 255]) < 1.2);
        assert_eq!(over([255, 0, 0, 128], [0, 0, 0, 255]), [128, 0, 0, 255]);
        assert_eq!(over([1, 2, 3, 0], [9, 9, 9, 255]), [9, 9, 9, 255]);
        assert!(luminance([255, 255, 255, 255]) > 0.99 && luminance([0, 0, 0, 255]) < 0.01);
    }

    #[test]
    fn a_big_page_with_many_rules_is_styled_quickly() {
        let mut css = String::new();
        for i in 0..3000 {
            css.push_str(&format!(".c{i}{{color:#{:06x}}} .c{i} > span.d{i}{{margin:{i}px}}\n", i * 7));
        }
        let mut html = String::new();
        for i in 0..3000 {
            html.push_str(&format!("<div class='c{i} other'><span class='d{i}'>x</span></div>"));
        }
        let t0 = std::time::Instant::now();
        let (t, c) = styles(&html, &css);
        assert!(t0.elapsed().as_secs() < 5, "{:?}", t0.elapsed());
        assert_eq!(c.iter().flatten().filter(|x| x.color.is_some()).count(), 6000);
        let _ = t;
    }

    #[test]
    fn floats_positions_shadows_gradients_opacity_and_fit_are_computed() {
        let (t, c) = styles(
            "<div id=a></div><div id=b></div><img id=i>",
            "#a{float:right;clear:both;position:absolute;top:5px;left:-3px;z-index:7;opacity:.5;box-shadow:1px 2px 3px red;background:linear-gradient(to left,red,blue)} #b{inset:4px 6px;position:relative;opacity:0} #i{object-fit:cover}",
        );
        let a = of(&t, &c, "a");
        assert_eq!((a.fx.float, a.fx.clear, a.fx.pos, a.fx.z), (Float::Right, Clear::Both, Pos::Absolute, 7));
        assert_eq!(a.fx.off, [Some(5.0), None, None, Some(-3.0)]);
        assert_eq!(a.fx.opacity, 0.5);
        assert_eq!(a.fx.shadow.map(|s| (s.dx, s.dy, s.blur)), Some((1.0, 2.0, 3.0)));
        assert_eq!(a.fx.gradient.as_ref().map(|g| g.angle), Some(270.0));
        assert!(a.has_box(), "a float is a box");
        let b = of(&t, &c, "b");
        assert_eq!(b.fx.off, [Some(4.0), Some(6.0), Some(4.0), Some(6.0)]);
        assert_eq!(b.fx.opacity, 1.0, "opacity:0 is how pages fade in, so it is ignored");
        assert_eq!(of(&t, &c, "i").fx.fit, Fit::Cover);
    }
}
