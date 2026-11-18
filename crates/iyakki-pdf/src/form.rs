//! Filling in the fields of a PDF form and writing the result (F-BROX-08). The change is an *incremental update*: the original bytes
//! stay exactly as they were and the changed objects, a new cross-reference section and a trailer that points back to the old one are
//! appended, so nothing of the original is rewritten and the file opens in any reader. Text and choice fields get a new appearance
//! (Helvetica, the size and colour of the field's default appearance string) so that readers which draw the stored appearance show
//! the new value, and the form is marked `NeedAppearances` so readers that draw their own do so too.
//!
//! What it does: text fields (single and multi-line, password), choice fields (the value must be one of the options unless the
//! field accepts typed text), check boxes. What it refuses, with the reason: protected files, files whose cross-reference was damaged
//! and rebuilt, read-only fields, radio buttons, push buttons and signatures. Text outside Windows-1252 is stored in the value but
//! drawn as `?` in the appearance.

use crate::doc::{text_string, Document};
use crate::file::File;
use crate::obj::{Dict, Obj};
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text { multiline: bool, password: bool, max_len: Option<usize> },
    /// `on` is the name of the checked state.
    Check { on: String },
    /// `options` are the display texts; `edit` when typed text is accepted.
    Choice { options: Vec<String>, edit: bool },
    Radio,
    Push,
    Signature,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditableField {
    pub name: String,
    pub kind: FieldKind,
    pub value: String,
    pub read_only: bool,
    /// The field's own object.
    pub obj: (u32, u16),
    /// The objects that draw it (the field itself when it is its own widget).
    pub widgets: Vec<(u32, u16)>,
}

impl EditableField {
    /// Can the viewer change this field?
    pub fn editable(&self) -> bool {
        !self.read_only && matches!(self.kind, FieldKind::Text { .. } | FieldKind::Check { .. } | FieldKind::Choice { .. })
    }
}

fn flag(ff: i64, bit: u32) -> bool {
    ff & (1i64 << (bit - 1)) != 0
}

fn rect4(f: &File, d: &Dict) -> Option<[f64; 4]> {
    let a = f.get(d, "Rect");
    let v: Vec<f64> = a.arr()?.iter().filter_map(|x| f.resolve(x).num()).collect();
    if v.len() < 4 || v.iter().any(|x| !x.is_finite() || x.abs() > 1e7) {
        return None;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn show(f: &File, v: &Obj) -> String {
    match f.resolve(v) {
        Obj::Str(s) => text_string(&s),
        Obj::Name(n) => n.to_string(),
        Obj::Arr(a) => a.iter().filter_map(|x| f.resolve(x).str_bytes().map(text_string)).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

struct Inherited {
    ft: Option<String>,
    ff: i64,
    da: Option<String>,
    q: i64,
    max_len: Option<usize>,
    opt: Option<Obj>,
    v: Option<Obj>,
}

impl Document {
    /// The fields of the form with what is needed to change them.
    pub fn editable_fields(&self) -> Vec<EditableField> {
        let f = &self.file;
        let root = f.get(&f.trailer, "Root");
        let Some(r) = root.dict() else { return vec![] };
        let af = f.get(r, "AcroForm");
        let Some(afd) = af.dict() else { return vec![] };
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let top = Inherited { ft: None, ff: 0, da: f.get(afd, "DA").str_bytes().map(text_string), q: f.get_num(afd, "Q").unwrap_or(0.0) as i64, max_len: None, opt: None, v: None };
        if let Some(fields) = f.get(afd, "Fields").arr() {
            for fl in fields.iter() {
                self.walk_fields(fl, "", &top, &mut out, &mut seen, 0);
            }
        }
        out
    }

    fn walk_fields(&self, node: &Obj, prefix: &str, inh: &Inherited, out: &mut Vec<EditableField>, seen: &mut HashSet<u32>, depth: usize) {
        if depth > 20 || out.len() > 5000 {
            return;
        }
        let Obj::Ref(num, gen) = node else { return };
        if !seen.insert(*num) {
            return;
        }
        let f = &self.file;
        let o = f.load(*num);
        let Some(d) = o.dict() else { return };
        let part = f.get(d, "T").str_bytes().map(text_string).unwrap_or_default();
        let name = if prefix.is_empty() { part.clone() } else if part.is_empty() { prefix.to_string() } else { format!("{prefix}.{part}") };
        let me = Inherited {
            ft: f.get(d, "FT").name().map(str::to_string).or_else(|| inh.ft.clone()),
            ff: d.get("Ff").and_then(|x| f.resolve(x).int()).unwrap_or(inh.ff),
            da: f.get(d, "DA").str_bytes().map(text_string).or_else(|| inh.da.clone()),
            q: f.get_num(d, "Q").map(|q| q as i64).unwrap_or(inh.q),
            max_len: f.get_num(d, "MaxLen").map(|n| n.max(0.0) as usize).or(inh.max_len),
            opt: d.get("Opt").cloned().or_else(|| inh.opt.clone()),
            v: d.get("V").cloned().or_else(|| inh.v.clone()),
        };
        let kids = f.get(d, "Kids").arr().cloned();
        if let Some(kids) = &kids {
            // kids that carry their own names are fields; kids without are only the places where this field is drawn
            let named = kids.iter().any(|k| matches!(k, Obj::Ref(n, _) if f.load(*n).dict().is_some_and(|kd| kd.contains_key("T"))));
            if named {
                for k in kids.iter() {
                    self.walk_fields(k, &name, &me, out, seen, depth + 1);
                }
                return;
            }
        }
        let widgets: Vec<(u32, u16)> = match &kids {
            Some(k) => k.iter().filter_map(|x| if let Obj::Ref(n, g) = x { Some((*n, *g)) } else { None }).collect(),
            None => vec![(*num, *gen)],
        };
        let ff = me.ff;
        let kind = match me.ft.as_deref() {
            Some("Tx") => FieldKind::Text { multiline: flag(ff, 13), password: flag(ff, 14), max_len: me.max_len.filter(|n| *n > 0) },
            Some("Btn") if flag(ff, 17) => FieldKind::Push,
            Some("Btn") if flag(ff, 16) => FieldKind::Radio,
            Some("Btn") => {
                let on = widgets.iter().find_map(|(n, _)| on_state(f, &f.load(*n))).unwrap_or_else(|| "Yes".to_string());
                FieldKind::Check { on }
            }
            Some("Ch") => {
                let options = match me.opt.as_ref().map(|x| f.resolve(x)) {
                    Some(Obj::Arr(a)) => a
                        .iter()
                        .map(|e| match f.resolve(e) {
                            Obj::Arr(p) => p.get(1).or(p.first()).map(|x| show(f, x)).unwrap_or_default(),
                            other => show(f, &other),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                FieldKind::Choice { options, edit: flag(ff, 19) && flag(ff, 18) }
            }
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Other,
        };
        let value = me.v.as_ref().map(|v| show(f, v)).unwrap_or_default();
        out.push(EditableField { name, kind, value, read_only: flag(ff, 1), obj: (*num, *gen), widgets });
    }

    /// The file with the changes written into it: `(field name, new value)` pairs. A check box takes `true`/`yes`/`on`/`1`/its own
    /// on-name for checked and anything else for unchecked.
    pub fn fill_form(&self, changes: &[(String, String)]) -> Result<Vec<u8>, String> {
        let f = &self.file;
        if f.trailer.contains_key("Encrypt") {
            return Err("this PDF is protected with a password; filling it in and saving is not supported".into());
        }
        if f.repaired {
            return Err("the index of this PDF was damaged and had to be rebuilt, so a safe update cannot be written".into());
        }
        if changes.is_empty() {
            return Err("nothing to change".into());
        }
        let fields = self.editable_fields();
        let mut dicts: BTreeMap<u32, (u16, Dict)> = BTreeMap::new();
        let mut extra: Vec<(u32, Vec<u8>)> = Vec::new();
        let size = f.trailer.get("Size").and_then(|s| s.int()).unwrap_or(0).max(0) as u32;
        let mut next = size.max(f.all_numbers().into_iter().max().unwrap_or(0) + 1);
        let load = |dicts: &mut BTreeMap<u32, (u16, Dict)>, n: u32, g: u16| -> Result<(), String> {
            if !dicts.contains_key(&n) {
                let d = f.load(n).dict().map(|d| (**d).clone()).ok_or("a field object could not be read")?;
                dicts.insert(n, (g, d));
            }
            Ok(())
        };
        for (name, value) in changes {
            let matches: Vec<&EditableField> = fields.iter().filter(|fl| fl.name == *name).collect();
            let fl = match matches.as_slice() {
                [one] => *one,
                [] => {
                    let ci: Vec<&EditableField> = fields.iter().filter(|fl| fl.name.eq_ignore_ascii_case(name)).collect();
                    match ci.as_slice() {
                        [one] => *one,
                        [] => return Err(format!("there is no field named {name:?}")),
                        _ => return Err(format!("{name:?} matches several fields; use the full name")),
                    }
                }
                _ => return Err(format!("{name:?} names several fields")),
            };
            if fl.read_only {
                return Err(format!("the field {:?} is read-only", fl.name));
            }
            let (fnum, fgen) = fl.obj;
            load(&mut dicts, fnum, fgen)?;
            match &fl.kind {
                FieldKind::Text { multiline, password, max_len } => {
                    if let Some(m) = max_len {
                        if value.chars().count() > *m {
                            return Err(format!("the field {:?} takes at most {m} characters", fl.name));
                        }
                    }
                    if !*multiline && value.contains(['\n', '\r']) {
                        return Err(format!("the field {:?} is one line", fl.name));
                    }
                    dicts.get_mut(&fnum).unwrap().1.insert("V".into(), Obj::Str(Rc::new(pdf_text(value))));
                    for &(wn, wg) in &fl.widgets {
                        load(&mut dicts, wn, wg)?;
                        self.give_appearance(&mut dicts, &mut extra, &mut next, wn, value, *multiline, *password, &fl.name)?;
                    }
                }
                FieldKind::Choice { options, edit } => {
                    if !*edit && !options.is_empty() && !options.iter().any(|o| o == value) {
                        return Err(format!("{value:?} is not one of the choices of {:?} ({})", fl.name, options.join(", ")));
                    }
                    dicts.get_mut(&fnum).unwrap().1.insert("V".into(), Obj::Str(Rc::new(pdf_text(value))));
                    for &(wn, wg) in &fl.widgets {
                        load(&mut dicts, wn, wg)?;
                        self.give_appearance(&mut dicts, &mut extra, &mut next, wn, value, false, false, &fl.name)?;
                    }
                }
                FieldKind::Check { on } => {
                    let v = value.trim().to_ascii_lowercase();
                    let checked = matches!(v.as_str(), "true" | "yes" | "on" | "1" | "checked" | "x") || *value == *on;
                    let state = |checked_name: &str| Obj::Name(Rc::from(if checked { checked_name } else { "Off" }));
                    dicts.get_mut(&fnum).unwrap().1.insert("V".into(), state(on));
                    for &(wn, wg) in &fl.widgets {
                        load(&mut dicts, wn, wg)?;
                        let own = on_state(f, &Obj::Dict(Rc::new(dicts[&wn].1.clone()))).unwrap_or_else(|| on.clone());
                        dicts.get_mut(&wn).unwrap().1.insert("AS".into(), state(&own));
                    }
                }
                other => return Err(format!("the field {:?} is a {} and cannot be filled in here", fl.name, match other { FieldKind::Radio => "radio button", FieldKind::Push => "push button", FieldKind::Signature => "signature", _ => "field of a kind this viewer does not edit" })),
            }
        }
        // the form asks readers to draw the fields again
        let Some(Obj::Ref(rn, rg)) = f.trailer.get("Root").cloned() else { return Err("the document has no catalog".into()) };
        let root = f.load(rn);
        let rd = root.dict().ok_or("the catalog could not be read")?;
        match rd.get("AcroForm") {
            Some(Obj::Ref(an, ag)) => {
                load(&mut dicts, *an, *ag)?;
                dicts.get_mut(an).unwrap().1.insert("NeedAppearances".into(), Obj::Bool(true));
            }
            Some(Obj::Dict(ad)) => {
                let mut ad = (**ad).clone();
                ad.insert("NeedAppearances".into(), Obj::Bool(true));
                load(&mut dicts, rn, rg)?;
                dicts.get_mut(&rn).unwrap().1.insert("AcroForm".into(), Obj::Dict(Rc::new(ad)));
            }
            _ => return Err("the document has no form".into()),
        }
        self.append_update(dicts, extra, next)
    }

    #[allow(clippy::too_many_arguments)]
    fn give_appearance(&self, dicts: &mut BTreeMap<u32, (u16, Dict)>, extra: &mut Vec<(u32, Vec<u8>)>, next: &mut u32, wn: u32, value: &str, multiline: bool, password: bool, field: &str) -> Result<(), String> {
        let f = &self.file;
        let wd = &dicts[&wn].1;
        let Some([x0, y0, x1, y1]) = rect4(f, wd) else { return Ok(()) };
        let (w, h) = (x1 - x0, y1 - y0);
        if w < 1.0 || h < 1.0 {
            return Ok(());
        }
        let da = f.get(wd, "DA").str_bytes().map(text_string).or_else(|| self.form_da());
        let q = f.get_num(wd, "Q").map(|q| q as i64).unwrap_or(0);
        let content = appearance_stream(value, w, h, da.as_deref(), q, multiline, password);
        let num = *next;
        *next += 1;
        if *next > 8_000_000 {
            return Err(format!("too many objects to update {field:?}"));
        }
        let mut body = format!("<< /Type /XObject /Subtype /Form /BBox [0 0 {} {}] /Resources << /Font << /Helv << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >> >> /Length {} >>\nstream\n", num4(w), num4(h), content.len()).into_bytes();
        body.extend_from_slice(&content);
        body.extend_from_slice(b"\nendstream");
        extra.push((num, body));
        let mut ap = Dict::new();
        ap.insert("N".into(), Obj::Ref(num, 0));
        dicts.get_mut(&wn).unwrap().1.insert("AP".into(), Obj::Dict(Rc::new(ap)));
        Ok(())
    }

    fn form_da(&self) -> Option<String> {
        let f = &self.file;
        let root = f.get(&f.trailer, "Root");
        let af = f.get(root.dict()?, "AcroForm");
        f.get(af.dict()?, "DA").str_bytes().map(text_string)
    }

    fn append_update(&self, dicts: BTreeMap<u32, (u16, Dict)>, extra: Vec<(u32, Vec<u8>)>, next: u32) -> Result<Vec<u8>, String> {
        let f = &self.file;
        let data = &f.data;
        let tail = data.len().saturating_sub(2048);
        let sx = (tail..data.len().saturating_sub(8)).rev().find(|&i| data[i..].starts_with(b"startxref")).ok_or("the file has no startxref")?;
        let prev: usize = String::from_utf8_lossy(&data[sx + 9..]).split_whitespace().next().and_then(|s| s.parse().ok()).ok_or("the file's startxref is not a number")?;
        let classic = data.get(prev..prev + 20).is_some_and(|s| s.iter().skip_while(|b| b.is_ascii_whitespace()).take(4).copied().collect::<Vec<u8>>() == b"xref");
        let mut out = data.clone();
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        let mut written: BTreeMap<u32, (u16, usize)> = BTreeMap::new();
        for (n, (g, d)) in &dicts {
            written.insert(*n, (*g, out.len()));
            out.extend_from_slice(format!("{n} {g} obj\n").as_bytes());
            ser(&Obj::Dict(Rc::new(d.clone())), &mut out);
            out.extend_from_slice(b"\nendobj\n");
        }
        for (n, body) in &extra {
            written.insert(*n, (0, out.len()));
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let top = written.keys().next_back().copied().unwrap_or(0).max(next.saturating_sub(1));
        let mut trailer_common = Vec::new();
        for key in ["Root", "Info", "ID"] {
            if let Some(v) = f.trailer.get(key) {
                trailer_common.extend_from_slice(format!(" /{key} ").as_bytes());
                ser(v, &mut trailer_common);
            }
        }
        let xref_at = out.len();
        // runs of consecutive object numbers
        let mut runs: Vec<(u32, Vec<(u16, usize)>)> = Vec::new();
        for (n, (g, off)) in &written {
            match runs.last_mut() {
                Some((s, v)) if *s + v.len() as u32 == *n => v.push((*g, *off)),
                _ => runs.push((*n, vec![(*g, *off)])),
            }
        }
        if classic {
            out.extend_from_slice(b"xref\n");
            for (s, v) in &runs {
                out.extend_from_slice(format!("{s} {}\n", v.len()).as_bytes());
                for (g, off) in v {
                    out.extend_from_slice(format!("{off:010} {g:05} n \n").as_bytes());
                }
            }
            out.extend_from_slice(format!("trailer\n<< /Size {}{} /Prev {prev} >>\n", top + 1, String::from_utf8_lossy(&trailer_common)).as_bytes());
        } else {
            let xn = top + 1;
            let mut index = String::new();
            let mut rows: Vec<u8> = Vec::new();
            for (s, v) in &runs {
                index.push_str(&format!("{s} {} ", v.len()));
                for (g, off) in v {
                    rows.push(1);
                    rows.extend_from_slice(&(*off as u32).to_be_bytes());
                    rows.extend_from_slice(&g.to_be_bytes());
                }
            }
            index.push_str(&format!("{xn} 1"));
            rows.push(1);
            rows.extend_from_slice(&(xref_at as u32).to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(format!("{xn} 0 obj\n<< /Type /XRef /Size {} /W [1 4 2] /Index [{index}]{} /Prev {prev} /Length {} >>\nstream\n", xn + 1, String::from_utf8_lossy(&trailer_common), rows.len()).as_bytes());
            out.extend_from_slice(&rows);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        }
        out.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
        Ok(out)
    }
}

/// The check box's checked state: the key of its normal appearance that is not `Off`.
fn on_state(f: &File, widget: &Obj) -> Option<String> {
    let d = widget.dict()?;
    let ap = f.get(d, "AP");
    let n = f.get(ap.dict()?, "N");
    n.dict()?.keys().find(|k| k.as_str() != "Off").cloned()
}

fn num4(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

/// A text string for a PDF: plain bytes when Windows-1252 can hold it, else UTF-16 with a byte-order mark.
fn pdf_text(s: &str) -> Vec<u8> {
    let latin: Option<Vec<u8>> = s.chars().map(|c| if (c as u32) < 0x80 { Some(c as u8) } else { None }).collect();
    match latin {
        Some(b) => b,
        None => {
            let mut v = vec![0xfe, 0xff];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }
    }
}

fn win(s: &str) -> Vec<u8> {
    s.chars().filter(|c| *c != '\r').map(|c| crate::write::winansi(c).unwrap_or(b'?')).collect()
}

fn esc(b: &[u8], out: &mut Vec<u8>) {
    for &c in b {
        if matches!(c, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(c);
    }
}

/// The width of Windows-1252 bytes in Helvetica at `size`.
pub(crate) fn helv_len(bytes: &[u8], size: f64) -> f64 {
    bytes.iter().map(|&b| crate::font::helv_width(if (0x80..0xa0).contains(&b) { crate::font::CP1252_HIGH[(b - 0x80) as usize] } else { b as char }) as f64).sum::<f64>() * size / 1000.0
}

/// One paragraph broken into lines no wider than `max`: at spaces, a word wider than a line cut where it no longer fits (a field too
/// narrow for one letter still gets one letter a line, never an endless loop).
fn wrap_helv(para: &[u8], size: f64, max: f64) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for word in para.split(|b| *b == b' ') {
        let mut w = word.to_vec();
        loop {
            let joined = if cur.is_empty() { w.clone() } else { [&cur[..], b" ", &w[..]].concat() };
            if helv_len(&joined, size) <= max || (cur.is_empty() && w.len() <= 1) {
                cur = joined;
                break;
            }
            if cur.is_empty() {
                let mut cut = 1;
                while cut < w.len() && helv_len(&w[..cut + 1], size) <= max {
                    cut += 1;
                }
                lines.push(w[..cut].to_vec());
                w = w[cut..].to_vec();
                if w.is_empty() {
                    break;
                }
            } else {
                lines.push(std::mem::take(&mut cur));
            }
        }
    }
    lines.push(cur);
    lines
}

pub(crate) fn appearance_stream(value: &str, w: f64, h: f64, da: Option<&str>, q: i64, multiline: bool, password: bool) -> Vec<u8> {
    // the default appearance string: "/Helv 12 Tf 0 g"
    let toks: Vec<&str> = da.unwrap_or("").split_whitespace().collect();
    let mut size = 0.0f64;
    let mut colour: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "Tf" && i >= 2 {
            size = toks[i - 1].parse().unwrap_or(0.0);
            colour.truncate(colour.len().saturating_sub(2));
        } else {
            colour.push(toks[i]);
        }
        i += 1;
    }
    let colour = if colour.is_empty() { "0 g".to_string() } else { colour.join(" ") };
    let size = if size > 0.5 && size.is_finite() { size.min(200.0) } else if multiline { 12.0 } else { (h - 4.0).clamp(4.0, 12.0) };
    let shown: String = if password { "*".repeat(value.chars().count()) } else { value.to_string() };
    // a multi-line field breaks its lines at the field's width (at spaces; a word wider than the field is cut), as a viewer does
    let lines: Vec<Vec<u8>> = if multiline { shown.split('\n').flat_map(|p| wrap_helv(&win(p), size, w - 4.0)).collect() } else { vec![win(&shown)] };
    let lead = size * 1.15;
    let mut o = Vec::new();
    o.extend_from_slice(format!("/Tx BMC q 1 1 {} {} re W n BT {colour} /Helv {} Tf\n", num4(w - 2.0), num4(h - 2.0), num4(size)).as_bytes());
    for (k, line) in lines.iter().take(500).enumerate() {
        let bytes = line.clone();
        let tw = helv_len(&bytes, size);
        let x = match q {
            1 => ((w - tw) / 2.0).max(2.0),
            2 => (w - 2.0 - tw).max(2.0),
            _ => 2.0,
        };
        let y = if multiline { h - 2.0 - size - k as f64 * lead } else { (h - 0.72 * size) / 2.0 };
        o.extend_from_slice(format!("1 0 0 1 {} {} Tm (", num4(x), num4(y)).as_bytes());
        esc(&bytes, &mut o);
        o.extend_from_slice(b") Tj\n");
    }
    o.extend_from_slice(b"ET Q EMC");
    o
}

/// An object as PDF text. Strings are written in hex so no byte needs escaping.
fn ser(o: &Obj, out: &mut Vec<u8>) {
    match o {
        Obj::Null | Obj::Stream(..) => out.extend_from_slice(b"null"),
        Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Obj::Real(r) => out.extend_from_slice(num4(if r.is_finite() { *r } else { 0.0 }).as_bytes()),
        Obj::Str(s) => {
            out.push(b'<');
            for b in s.iter() {
                out.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Obj::Name(n) => name(n, out),
        Obj::Arr(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                ser(x, out);
            }
            out.push(b']');
        }
        Obj::Dict(d) => {
            out.extend_from_slice(b"<<");
            for (k, v) in d.iter() {
                out.push(b' ');
                name(k, out);
                out.push(b' ');
                ser(v, out);
            }
            out.extend_from_slice(b" >>");
        }
        Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
    }
}

fn name(n: &str, out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in n.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'*' | b',' | b':' | b';' | b'=' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'|' | b'^' | b'~') {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}
