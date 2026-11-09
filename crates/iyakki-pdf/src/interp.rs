//! The content-stream interpreter. It keeps the graphics state and tells a [`Sink`] what to draw (paths, glyphs, images); text
//! extraction and rendering are two sinks over the same interpreter. Bounded: operator count, operand stack, nesting of forms and of
//! q/Q are capped, so a hostile page ends early instead of running away.

use crate::color::{Cs, Rgb};
use crate::file::File;
use crate::font::Font;
use crate::geom::*;
use crate::obj::*;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

pub const MAX_OPS: usize = 4_000_000;
const MAX_FORM_DEPTH: usize = 10;
const MAX_Q: usize = 256;

#[derive(Clone)]
pub struct TextState {
    pub font: Option<Rc<Font>>,
    pub size: f64,
    pub tc: f64,
    pub tw: f64,
    pub th: f64,
    pub tl: f64,
    pub rise: f64,
    pub mode: i64,
}

#[derive(Clone)]
pub struct GState {
    pub ctm: Matrix,
    pub fill_cs: Cs,
    pub stroke_cs: Cs,
    pub fill: Rgb,
    pub stroke: Rgb,
    pub fill_alpha: f32,
    pub stroke_alpha: f32,
    pub line_width: f64,
    pub cap: i64,
    pub join: i64,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub phase: f64,
    pub text: TextState,
}

impl GState {
    pub fn new(ctm: Matrix) -> GState {
        GState {
            ctm,
            fill_cs: Cs::Gray,
            stroke_cs: Cs::Gray,
            fill: [0.0; 3],
            stroke: [0.0; 3],
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            line_width: 1.0,
            cap: 0,
            join: 0,
            miter: 10.0,
            dash: vec![],
            phase: 0.0,
            text: TextState { font: None, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0, mode: 0 },
        }
    }
}

/// One glyph as shown: its character(s), the matrix that maps the em square (0..1) to the target space, and the advance in em.
pub struct Glyph<'a> {
    pub text: &'a str,
    pub trm: Matrix,
    pub advance: f64,
    pub font: &'a Font,
}

pub enum ImageSrc<'a> {
    Xobject(&'a Obj),
    Inline(&'a Dict, &'a [u8]),
}

pub trait Sink {
    fn save(&mut self) {}
    fn restore(&mut self) {}
    fn fill(&mut self, _path: &[Seg], _even_odd: bool, _gs: &GState) {}
    fn stroke(&mut self, _path: &[Seg], _gs: &GState) {}
    fn clip(&mut self, _path: &[Seg], _even_odd: bool) {}
    fn glyph(&mut self, _g: &Glyph, _gs: &GState) {}
    fn image(&mut self, _f: &File, _src: ImageSrc, _res: &Dict, _gs: &GState) {}
    /// A new line of text starts (a text object began, or a move that is not a continuation).
    fn text_break(&mut self) {}
}

#[derive(Default, Debug)]
pub struct Report {
    /// Things on the page that this reader does not show (patterns, shadings, rotated text, ...).
    pub unsupported: BTreeSet<String>,
    pub ops: usize,
    pub stopped_early: bool,
    pub glyphs_without_text: usize,
}

pub struct Interp<'a> {
    pub f: &'a File,
    pub rep: Report,
    fonts: HashMap<u32, Rc<Font>>,
    forms: Vec<u32>,
}

fn nums(args: &[Obj]) -> Vec<f64> {
    args.iter().filter_map(|o| o.num()).collect()
}

impl<'a> Interp<'a> {
    pub fn new(f: &'a File) -> Interp<'a> {
        Interp { f, rep: Report::default(), fonts: HashMap::new(), forms: vec![] }
    }

    fn lookup(&self, res: &Dict, kind: &str, name: &str) -> Option<Obj> {
        let d = self.f.get(res, kind);
        let d = d.dict()?;
        d.get(name).cloned()
    }

    fn font(&mut self, res: &Dict, name: &str) -> Option<Rc<Font>> {
        let o = self.lookup(res, "Font", name)?;
        if let Obj::Ref(n, _) = o {
            if let Some(f) = self.fonts.get(&n) {
                return Some(f.clone());
            }
            let d = self.f.resolve(&o);
            let font = Rc::new(Font::load(self.f, d.dict()?, 0));
            if self.fonts.len() < 2000 {
                self.fonts.insert(n, font.clone());
            }
            return Some(font);
        }
        let d = self.f.resolve(&o);
        Some(Rc::new(Font::load(self.f, d.dict()?, 0)))
    }

    /// Run the content of a page (or form) with `res`; `gs` is the starting state.
    pub fn run(&mut self, data: &[u8], res: &Dict, gs: GState, sink: &mut dyn Sink, depth: usize) {
        let mut st = vec![gs];
        let mut cur: GState;
        let mut stack: Vec<Obj> = Vec::new();
        let mut path: Path = Vec::new();
        let mut pending_clip: Option<bool> = None;
        let mut tm = Matrix::ID;
        let mut tlm = Matrix::ID;
        let mut lx = Lexer::new(data, 0);
        let mut cp = (0f64, 0f64);
        let mut sub_start = (0f64, 0f64);
        cur = st.pop().unwrap_or_else(|| GState::new(Matrix::ID));
        let mut qdepth = 0usize;
        let mut saved: Vec<GState> = Vec::new();
        loop {
            if self.rep.ops >= MAX_OPS {
                self.rep.stopped_early = true;
                break;
            }
            let t = match parse_obj(&mut lx, false, 0) {
                Ok(t) => t,
                Err(_) => {
                    stack.clear();
                    continue;
                }
            };
            match t {
                Tok::Eof => break,
                Tok::Obj(o) => {
                    if stack.len() >= 64 {
                        stack.remove(0);
                    }
                    stack.push(o);
                }
                Tok::ArrClose | Tok::DictClose | Tok::ArrOpen | Tok::DictOpen => {}
                Tok::Keyword(op) => {
                    self.rep.ops += 1;
                    let a = std::mem::take(&mut stack);
                    let n = nums(&a);
                    macro_rules! num {
                        ($i:expr) => {
                            n.get($i).copied().unwrap_or(0.0)
                        };
                    }
                    match op.as_str() {
                        "q" => {
                            if qdepth < MAX_Q {
                                saved.push(cur.clone());
                                qdepth += 1;
                                sink.save();
                            }
                        }
                        "Q" => {
                            if let Some(s) = saved.pop() {
                                cur = s;
                                qdepth -= 1;
                                sink.restore();
                            }
                        }
                        "cm" if n.len() >= 6 => {
                            if let Some(m) = Matrix::from_arr(&n) {
                                cur.ctm = m.then(&cur.ctm);
                            }
                        }
                        "w" => cur.line_width = num!(0).abs().min(1e5),
                        "J" => cur.cap = num!(0) as i64,
                        "j" => cur.join = num!(0) as i64,
                        "M" => cur.miter = num!(0).clamp(1.0, 1000.0),
                        "d" => {
                            if let Some(Obj::Arr(arr)) = a.first() {
                                let v: Vec<f64> = arr.iter().filter_map(|x| x.num()).map(|x| x.abs().min(1e6)).collect();
                                cur.dash = if v.iter().sum::<f64>() > 1e-6 { v } else { vec![] };
                                cur.phase = a.get(1).and_then(|x| x.num()).unwrap_or(0.0);
                            }
                        }
                        "gs" => {
                            if let Some(name) = a.first().and_then(|x| x.name()) {
                                if let Some(d) = self.lookup(res, "ExtGState", name).map(|o| self.f.resolve(&o)).and_then(|o| o.dict().cloned()) {
                                    if let Some(v) = self.f.get_num(&d, "CA") {
                                        cur.stroke_alpha = v.clamp(0.0, 1.0) as f32;
                                    }
                                    if let Some(v) = self.f.get_num(&d, "ca") {
                                        cur.fill_alpha = v.clamp(0.0, 1.0) as f32;
                                    }
                                    if let Some(v) = self.f.get_num(&d, "LW") {
                                        cur.line_width = v.abs().min(1e5);
                                    }
                                    if let Some(v) = self.f.get(&d, "LC").int() {
                                        cur.cap = v;
                                    }
                                    if let Some(v) = self.f.get(&d, "LJ").int() {
                                        cur.join = v;
                                    }
                                    if let Obj::Dict(_) = self.f.get(&d, "SMask") {
                                        self.rep.unsupported.insert("soft masks (transparency)".into());
                                    }
                                    if let Some(fa) = self.f.get(&d, "Font").arr() {
                                        if let (Some(fo), Some(sz)) = (fa.first(), fa.get(1).and_then(|x| self.f.resolve(x).num())) {
                                            if let Some(fd) = self.f.resolve(fo).dict() {
                                                cur.text.font = Some(Rc::new(Font::load(self.f, fd, 0)));
                                                cur.text.size = sz;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // paths
                        "m" => {
                            let p = cur.ctm.apply(num!(0), num!(1));
                            path.push(Seg::Move(p.0, p.1));
                            cp = (num!(0), num!(1));
                            sub_start = cp;
                        }
                        "l" => {
                            let p = cur.ctm.apply(num!(0), num!(1));
                            path.push(Seg::Line(p.0, p.1));
                            cp = (num!(0), num!(1));
                        }
                        "c" | "v" | "y" if path.len() < MAX_PATH_SEGS => {
                            let (x1, y1, x2, y2, x3, y3) = match op.as_str() {
                                "c" => (num!(0), num!(1), num!(2), num!(3), num!(4), num!(5)),
                                "v" => (cp.0, cp.1, num!(0), num!(1), num!(2), num!(3)),
                                _ => (num!(0), num!(1), num!(2), num!(3), num!(2), num!(3)),
                            };
                            let (p1, p2, p3) = (cur.ctm.apply(x1, y1), cur.ctm.apply(x2, y2), cur.ctm.apply(x3, y3));
                            path.push(Seg::Curve(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1));
                            cp = (x3, y3);
                        }
                        "h" => {
                            path.push(Seg::Close);
                            cp = sub_start;
                        }
                        "re" if path.len() < MAX_PATH_SEGS => {
                            let (x, y, w, h) = (num!(0), num!(1), num!(2), num!(3));
                            for (i, (px, py)) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)].iter().enumerate() {
                                let p = cur.ctm.apply(*px, *py);
                                path.push(if i == 0 { Seg::Move(p.0, p.1) } else { Seg::Line(p.0, p.1) });
                            }
                            path.push(Seg::Close);
                            cp = (x, y);
                            sub_start = cp;
                        }
                        "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                            if matches!(op.as_str(), "s" | "b" | "b*") {
                                path.push(Seg::Close);
                            }
                            match op.as_str() {
                                "f" | "F" | "B" | "b" => sink.fill(&path, false, &cur),
                                "f*" | "B*" | "b*" => sink.fill(&path, true, &cur),
                                _ => {}
                            }
                            if matches!(op.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*") {
                                sink.stroke(&path, &cur);
                            }
                            if let Some(eo) = pending_clip.take() {
                                sink.clip(&path, eo);
                            }
                            path.clear();
                        }
                        "W" => pending_clip = Some(false),
                        "W*" => pending_clip = Some(true),
                        // colour
                        "g" | "G" => {
                            let cs = Cs::Gray;
                            let c = cs.rgb(&n);
                            set_color(&mut cur, op == "g", cs, c);
                        }
                        "rg" | "RG" => {
                            let cs = Cs::Rgb;
                            let c = cs.rgb(&n);
                            set_color(&mut cur, op == "rg", cs, c);
                        }
                        "k" | "K" => {
                            let cs = Cs::Cmyk;
                            let c = cs.rgb(&n);
                            set_color(&mut cur, op == "k", cs, c);
                        }
                        "cs" | "CS" => {
                            if let Some(o) = a.first() {
                                let cs = Cs::parse(self.f, o, self.f.get(res, "ColorSpace").dict().map(|d| &**d), 0);
                                let c = cs.rgb(&cs.initial());
                                set_color(&mut cur, op == "cs", cs, c);
                            }
                        }
                        "sc" | "scn" | "SC" | "SCN" => {
                            let fill = op.starts_with('s');
                            let cs = if fill { cur.fill_cs.clone() } else { cur.stroke_cs.clone() };
                            if matches!(cs, Cs::Pattern) || a.last().map(|x| x.name().is_some()).unwrap_or(false) {
                                self.rep.unsupported.insert("pattern fills (drawn grey)".into());
                                set_color(&mut cur, fill, Cs::Pattern, [0.6; 3]);
                            } else {
                                let c = cs.rgb(&n);
                                set_color(&mut cur, fill, cs, c);
                            }
                        }
                        "sh" => {
                            self.rep.unsupported.insert("shading fills (gradients)".into());
                        }
                        // text
                        "BT" => {
                            tm = Matrix::ID;
                            tlm = Matrix::ID;
                            sink.text_break();
                        }
                        "ET" => sink.text_break(),
                        "Tc" => cur.text.tc = num!(0),
                        "Tw" => cur.text.tw = num!(0),
                        "Tz" => cur.text.th = num!(0) / 100.0,
                        "TL" => cur.text.tl = num!(0),
                        "Ts" => cur.text.rise = num!(0),
                        "Tr" => cur.text.mode = num!(0) as i64,
                        "Tf" => {
                            if let Some(name) = a.first().and_then(|x| x.name()) {
                                cur.text.font = self.font(res, name);
                            }
                            cur.text.size = n.first().copied().unwrap_or(0.0).clamp(-10_000.0, 10_000.0);
                        }
                        "Td" | "TD" => {
                            if op == "TD" {
                                cur.text.tl = -num!(1);
                            }
                            tlm = Matrix([1.0, 0.0, 0.0, 1.0, num!(0), num!(1)]).then(&tlm);
                            tm = tlm;
                            sink.text_break();
                        }
                        "Tm" if n.len() >= 6 => {
                            if let Some(m) = Matrix::from_arr(&n) {
                                tm = m;
                                tlm = m;
                                sink.text_break();
                            }
                        }
                        "T*" => {
                            tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -cur.text.tl]).then(&tlm);
                            tm = tlm;
                            sink.text_break();
                        }
                        "Tj" | "'" | "\"" | "TJ" => {
                            if op == "'" || op == "\"" {
                                if op == "\"" {
                                    cur.text.tw = num!(0);
                                    cur.text.tc = num!(1);
                                }
                                tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -cur.text.tl]).then(&tlm);
                                tm = tlm;
                                sink.text_break();
                            }
                            let items: Vec<Obj> = match a.last() {
                                Some(Obj::Arr(arr)) if op == "TJ" => (**arr).clone(),
                                Some(o @ Obj::Str(_)) => vec![o.clone()],
                                _ => vec![],
                            };
                            for it in items {
                                match &it {
                                    Obj::Str(s) => self.show(s, &cur, &mut tm, sink),
                                    o => {
                                        if let Some(v) = o.num() {
                                            let tx = -v / 1000.0 * cur.text.size * cur.text.th;
                                            tm = Matrix([1.0, 0.0, 0.0, 1.0, tx, 0.0]).then(&tm);
                                        }
                                    }
                                }
                            }
                        }
                        "Do" => {
                            if let Some(name) = a.first().and_then(|x| x.name()) {
                                if let Some(xo) = self.lookup(res, "XObject", name) {
                                    self.do_xobject(&xo, res, &cur, sink, depth);
                                }
                            }
                        }
                        "BI" => {
                            self.inline_image(&mut lx, res, &cur, sink);
                        }
                        _ => {} // d0 d1 BMC BDC EMC MP DP BX EX ri i and unknown operators
                    }
                }
            }
        }
        while saved.pop().is_some() {
            sink.restore();
        }
        let _ = st;
    }

    fn show(&mut self, s: &[u8], gs: &GState, tm: &mut Matrix, sink: &mut dyn Sink) {
        let Some(font) = gs.text.font.clone() else { return };
        let ts = &gs.text;
        for code in font.codes(s) {
            let w = font.width(code);
            let uni = font.unicode(code);
            let text = match &uni {
                Some(t) => t.as_str(),
                None => {
                    self.rep.glyphs_without_text += 1;
                    ""
                }
            };
            let trm = Matrix([ts.size * ts.th, 0.0, 0.0, ts.size, 0.0, ts.rise]).then(tm).then(&gs.ctm);
            let g = Glyph { text, trm, advance: w / 1000.0, font: &font };
            sink.glyph(&g, gs);
            let word = if code == 32 && !font.two_byte { ts.tw } else { 0.0 };
            let tx = (w / 1000.0 * ts.size + ts.tc + word) * ts.th;
            *tm = if font.vertical { Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.size + ts.tc + word]).then(tm) } else { Matrix([1.0, 0.0, 0.0, 1.0, tx, 0.0]).then(tm) };
        }
    }

    fn do_xobject(&mut self, xo: &Obj, res: &Dict, gs: &GState, sink: &mut dyn Sink, depth: usize) {
        let num = if let Obj::Ref(n, _) = xo { Some(*n) } else { None };
        let o = self.f.resolve(xo);
        let Some(d) = o.dict().cloned() else { return };
        match self.f.get(&d, "Subtype").name() {
            Some("Image") => sink.image(self.f, ImageSrc::Xobject(&o), res, gs),
            Some("Form") => {
                if depth >= MAX_FORM_DEPTH || num.map(|n| self.forms.contains(&n)).unwrap_or(false) {
                    self.rep.unsupported.insert("forms nested too deeply".into());
                    return;
                }
                if let Some(n) = num {
                    self.forms.push(n);
                }
                self.draw_form(&o, &d, res, gs, sink, depth + 1);
                if num.is_some() {
                    self.forms.pop();
                }
            }
            _ => {}
        }
    }

    /// Draw a form XObject with `gs.ctm` as the matrix it is placed under.
    pub fn draw_form(&mut self, o: &Obj, d: &Dict, res: &Dict, gs: &GState, sink: &mut dyn Sink, depth: usize) {
        let mut g = gs.clone();
        if let Some(m) = self.f.get(d, "Matrix").arr().map(|a| a.iter().filter_map(|x| self.f.resolve(x).num()).collect::<Vec<_>>()).and_then(|v| Matrix::from_arr(&v)) {
            g.ctm = m.then(&g.ctm);
        }
        sink.save();
        if let Some(bb) = self.f.get(d, "BBox").arr().map(|a| a.iter().filter_map(|x| self.f.resolve(x).num()).collect::<Vec<_>>()).filter(|v| v.len() >= 4) {
            let pts = [(bb[0], bb[1]), (bb[2], bb[1]), (bb[2], bb[3]), (bb[0], bb[3])];
            let mut p = Vec::new();
            for (i, (x, y)) in pts.iter().enumerate() {
                let q = g.ctm.apply(*x, *y);
                p.push(if i == 0 { Seg::Move(q.0, q.1) } else { Seg::Line(q.0, q.1) });
            }
            p.push(Seg::Close);
            sink.clip(&p, false);
        }
        let fres = match self.f.get(d, "Resources") {
            Obj::Dict(r) => r,
            _ => Rc::new(res.clone()),
        };
        if let Ok(data) = self.f.stream_data(o) {
            self.run(&data.data, &fres, g, sink, depth);
        }
        sink.restore();
    }

    fn inline_image(&mut self, lx: &mut Lexer, res: &Dict, gs: &GState, sink: &mut dyn Sink) {
        let mut d = Dict::new();
        loop {
            match parse_obj(lx, false, 0) {
                Ok(Tok::Obj(Obj::Name(k))) => {
                    if let Ok(Tok::Obj(v)) = parse_obj(lx, false, 0) {
                        let key = match &*k {
                            "BPC" => "BitsPerComponent",
                            "CS" => "ColorSpace",
                            "D" => "Decode",
                            "DP" => "DecodeParms",
                            "F" => "Filter",
                            "H" => "Height",
                            "IM" => "ImageMask",
                            "I" => "Interpolate",
                            "W" => "Width",
                            other => other,
                        };
                        if d.len() < 32 {
                            d.insert(key.to_string(), v);
                        }
                    }
                }
                Ok(Tok::Keyword(k)) if k == "ID" => break,
                Ok(Tok::Eof) | Err(_) => return,
                _ => {}
            }
        }
        let start = (lx.pos + 1).min(lx.data.len());
        let end;
        // find "EI" delimited by whitespace
        let mut i = start;
        loop {
            let Some(p) = lx.data[i.min(lx.data.len())..].windows(2).position(|w| w == b"EI") else {
                end = lx.data.len();
                break;
            };
            let at = i + p;
            let before_ok = at == start || is_ws(lx.data[at - 1]);
            let after_ok = at + 2 >= lx.data.len() || is_ws(lx.data[at + 2]);
            if before_ok && after_ok {
                end = at;
                lx.pos = at + 2;
                break;
            }
            i = at + 1;
            if i >= lx.data.len() {
                end = lx.data.len();
                lx.pos = lx.data.len();
                break;
            }
        }
        let data = &lx.data[start..end.max(start)];
        sink.image(self.f, ImageSrc::Inline(&d, data), res, gs);
    }
}

fn set_color(g: &mut GState, fill: bool, cs: Cs, rgb: Rgb) {
    if fill {
        g.fill_cs = cs;
        g.fill = rgb;
    } else {
        g.stroke_cs = cs;
        g.stroke = rgb;
    }
}

