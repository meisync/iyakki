//! The file layer: header, cross-reference tables and streams (with incremental updates and hybrid files), object streams,
//! repair by scanning when the structure is damaged, and stream decoding. All of it reads untrusted bytes.

use crate::filter;
use crate::obj::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

pub const MAX_FILE: usize = 400 * 1024 * 1024;
const MAX_XREF_ENTRIES: usize = 8_000_000;
const MAX_SECTIONS: usize = 1024;
const TOTAL_BUDGET: usize = 384 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    NotPdf,
    Encrypted,
    Damaged(String),
    TooLarge,
    NoPages,
    PageOutOfRange,
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::NotPdf => write!(f, "this is not a PDF file"),
            PdfError::Encrypted => write!(f, "password-protected PDFs are not supported yet"),
            PdfError::Damaged(s) => write!(f, "the PDF is damaged: {s}"),
            PdfError::TooLarge => write!(f, "the PDF is too large to open here"),
            PdfError::NoPages => write!(f, "the PDF has no pages"),
            PdfError::PageOutOfRange => write!(f, "there is no such page"),
        }
    }
}

impl std::error::Error for PdfError {}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Free,
    At(usize),
    InStm(u32, u32),
}

struct ObjStm {
    /// (object number, offset inside the decoded data)
    index: Vec<(u32, usize)>,
    data: Vec<u8>,
    first: usize,
}

/// A stream after its normal filters: the bytes, and the image filter (DCT, JPX, CCITT, JBIG2) that is left, with its parameters.
pub struct Decoded {
    pub data: Vec<u8>,
    pub image_filter: Option<(String, Obj)>,
}

pub struct File {
    pub data: Vec<u8>,
    xref: HashMap<u32, Entry>,
    pub trailer: Dict,
    cache: RefCell<HashMap<u32, Obj>>,
    loading: RefCell<HashSet<u32>>,
    objstm: RefCell<HashMap<u32, Option<Rc<ObjStm>>>>,
    budget: Cell<usize>,
    /// The cross-reference data was damaged and the file was rebuilt by scanning.
    pub repaired: bool,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() || from > hay.len() - needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = from;
    let last = hay.len() - needle.len();
    while i <= last {
        match hay[i..=last].iter().position(|&b| b == first) {
            None => return None,
            Some(p) => {
                i += p;
                if &hay[i..i + needle.len()] == needle {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).rev().find(|&i| &hay[i..i + needle.len()] == needle)
}

impl File {
    pub fn open(data: Vec<u8>) -> Result<File, PdfError> {
        if data.len() > MAX_FILE {
            return Err(PdfError::TooLarge);
        }
        if find(&data[..data.len().min(1100)], b"%PDF-", 0).is_none() {
            return Err(PdfError::NotPdf);
        }
        let mut f = File {
            data,
            xref: HashMap::new(),
            trailer: Dict::new(),
            cache: RefCell::new(HashMap::new()),
            loading: RefCell::new(HashSet::new()),
            objstm: RefCell::new(HashMap::new()),
            budget: Cell::new(TOTAL_BUDGET),
            repaired: false,
        };
        let ok = f.read_xref_chain().is_ok() && f.root_ok();
        if !ok {
            f.repair();
        }
        if f.trailer.contains_key("Encrypt") {
            return Err(PdfError::Encrypted);
        }
        if !f.root_ok() {
            return Err(PdfError::Damaged("no document catalog was found".into()));
        }
        Ok(f)
    }

    /// Give the decoding budget back (call at the start of each page render or text pass).
    pub fn reset_budget(&self) {
        self.budget.set(TOTAL_BUDGET);
    }

    pub fn take_budget(&self, n: usize) -> bool {
        let b = self.budget.get();
        if n > b {
            self.budget.set(0);
            false
        } else {
            self.budget.set(b - n);
            true
        }
    }

    fn root_ok(&self) -> bool {
        match self.trailer.get("Root") {
            Some(r) => self.resolve(r).dict().map(|d| d.contains_key("Pages")).unwrap_or(false),
            None => false,
        }
    }

    // ---- cross-reference -------------------------------------------------------------------------------------------------

    fn read_xref_chain(&mut self) -> Result<(), PdfError> {
        let tail_start = self.data.len().saturating_sub(2048);
        let sx = rfind(&self.data[tail_start..], b"startxref").ok_or_else(|| PdfError::Damaged("no startxref".into()))? + tail_start;
        let mut lx = Lexer::new(&self.data, sx + 9);
        let start = match lx.next() {
            Tok::Obj(Obj::Int(n)) if n >= 0 => n as usize,
            _ => return Err(PdfError::Damaged("bad startxref".into())),
        };
        let mut todo = vec![start];
        let mut seen = HashSet::new();
        let mut first = true;
        while let Some(off) = todo.pop() {
            if !seen.insert(off) || seen.len() > MAX_SECTIONS {
                continue;
            }
            let t = self.read_section(off)?;
            if first {
                self.trailer = (*t).clone();
                first = false;
            } else {
                for (k, v) in t.iter() {
                    self.trailer.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            if let Some(Obj::Int(p)) = t.get("Prev") {
                if *p >= 0 {
                    todo.push(*p as usize);
                }
            }
            if let Some(Obj::Int(p)) = t.get("XRefStm") {
                if *p >= 0 {
                    todo.push(*p as usize);
                }
            }
        }
        Ok(())
    }

    fn add_entry(&mut self, num: u32, e: Entry) -> Result<(), PdfError> {
        if self.xref.len() >= MAX_XREF_ENTRIES {
            return Err(PdfError::Damaged("too many objects".into()));
        }
        self.xref.entry(num).or_insert(e);
        Ok(())
    }

    fn read_section(&mut self, off: usize) -> Result<Rc<Dict>, PdfError> {
        if off >= self.data.len() {
            return Err(PdfError::Damaged("cross-reference outside the file".into()));
        }
        let mut lx = Lexer::new(&self.data, off);
        lx.skip_ws();
        if self.data[lx.pos..].starts_with(b"xref") {
            lx.pos += 4;
            let mut entries: Vec<(u32, Entry)> = Vec::new();
            loop {
                match lx.next() {
                    Tok::Keyword(k) if k == "trailer" => break,
                    Tok::Obj(Obj::Int(start)) => {
                        let Tok::Obj(Obj::Int(count)) = lx.next() else { return Err(PdfError::Damaged("bad xref subsection".into())) };
                        if !(0..=MAX_XREF_ENTRIES as i64).contains(&count) || start < 0 || start > u32::MAX as i64 {
                            return Err(PdfError::Damaged("bad xref subsection".into()));
                        }
                        for i in 0..count {
                            let (Tok::Obj(Obj::Int(o)), Tok::Obj(Obj::Int(_g)), Tok::Keyword(k)) = (lx.next(), lx.next(), lx.next()) else {
                                return Err(PdfError::Damaged("bad xref entry".into()));
                            };
                            let e = if k == "n" && o > 0 { Entry::At(o as usize) } else { Entry::Free };
                            entries.push(((start + i) as u32, e));
                        }
                    }
                    _ => {
                        return Err(PdfError::Damaged("bad xref table".into()));
                    }
                }
            }
            let t = match parse_obj(&mut lx, true, 0) {
                Ok(Tok::Obj(Obj::Dict(d))) => d,
                _ => return Err(PdfError::Damaged("bad trailer".into())),
            };
            for (n, e) in entries {
                self.add_entry(n, e)?;
            }
            Ok(t)
        } else {
            // a cross-reference stream: "N G obj << ... >> stream"
            let (_, o) = self.parse_indirect_at(off, None).ok_or_else(|| PdfError::Damaged("bad xref stream".into()))?;
            let Obj::Stream(d, _, _) = &o else { return Err(PdfError::Damaged("bad xref stream".into())) };
            if d.get("Type").and_then(|t| t.name()) != Some("XRef") {
                return Err(PdfError::Damaged("bad xref stream".into()));
            }
            let dec = self.stream_data(&o)?;
            let w: Vec<usize> = d.get("W").and_then(|w| w.arr()).map(|a| a.iter().map(|x| x.int().unwrap_or(0).clamp(0, 8) as usize).collect()).unwrap_or_default();
            if w.len() < 3 {
                return Err(PdfError::Damaged("bad xref stream widths".into()));
            }
            let size = d.get("Size").and_then(|s| s.int()).unwrap_or(0).clamp(0, MAX_XREF_ENTRIES as i64);
            let index: Vec<i64> = match d.get("Index").and_then(|i| i.arr()) {
                Some(a) => a.iter().map(|x| x.int().unwrap_or(0)).collect(),
                None => vec![0, size],
            };
            let rec = w[0] + w[1] + w[2];
            if rec == 0 {
                return Err(PdfError::Damaged("bad xref stream widths".into()));
            }
            let mut p = 0usize;
            for pair in index.chunks(2) {
                if pair.len() < 2 || pair[0] < 0 || pair[1] < 0 {
                    break;
                }
                for i in 0..pair[1] {
                    if p + rec > dec.data.len() {
                        break;
                    }
                    let field = |from: usize, len: usize, dflt: u64| -> u64 {
                        if len == 0 {
                            return dflt;
                        }
                        dec.data[p + from..p + from + len].iter().fold(0u64, |a, &b| (a << 8) | b as u64)
                    };
                    let ty = field(0, w[0], 1);
                    let f2 = field(w[0], w[1], 0);
                    let f3 = field(w[0] + w[1], w[2], 0);
                    p += rec;
                    let num = (pair[0] + i) as u64;
                    if num > u32::MAX as u64 {
                        continue;
                    }
                    let e = match ty {
                        0 => Entry::Free,
                        1 => Entry::At(f2 as usize),
                        2 => Entry::InStm(f2 as u32, f3 as u32),
                        _ => continue,
                    };
                    self.add_entry(num as u32, e)?;
                }
            }
            Ok(d.clone())
        }
    }

    // ---- repair ----------------------------------------------------------------------------------------------------------

    fn repair(&mut self) {
        self.repaired = true;
        self.xref.clear();
        self.cache.borrow_mut().clear();
        self.objstm.borrow_mut().clear();
        let mut trailer = Dict::new();
        let mut from = 0;
        while let Some(p) = find(&self.data, b"obj", from) {
            from = p + 3;
            if p == 0 || self.data[p - 1] == b'd' {
                continue; // "endobj"
            }
            // walk back over "num gen "
            let mut i = p;
            while i > 0 && is_ws(self.data[i - 1]) {
                i -= 1;
            }
            let ge = i;
            while i > 0 && self.data[i - 1].is_ascii_digit() {
                i -= 1;
            }
            if i == ge {
                continue;
            }
            while i > 0 && is_ws(self.data[i - 1]) {
                i -= 1;
            }
            let ne = i;
            while i > 0 && self.data[i - 1].is_ascii_digit() {
                i -= 1;
            }
            if i == ne || ne - i > 10 {
                continue;
            }
            let Some(num) = std::str::from_utf8(&self.data[i..ne]).ok().and_then(|s| s.parse::<u64>().ok()).filter(|n| *n <= u32::MAX as u64) else { continue };
            if self.xref.len() >= MAX_XREF_ENTRIES {
                break;
            }
            self.xref.insert(num as u32, Entry::At(i));
        }
        let mut from = 0;
        while let Some(p) = find(&self.data, b"trailer", from) {
            from = p + 7;
            let mut lx = Lexer::new(&self.data, p + 7);
            if let Ok(Tok::Obj(Obj::Dict(d))) = parse_obj(&mut lx, true, 0) {
                for (k, v) in d.iter() {
                    trailer.insert(k.clone(), v.clone());
                }
            }
        }
        self.trailer = trailer;
        // object streams and cross-reference streams: their contents and trailers
        let nums: Vec<u32> = self.xref.keys().copied().take(600_000).collect();
        let mut stm = Vec::new();
        for n in &nums {
            let o = self.load(*n);
            if let Obj::Stream(d, _, _) = &o {
                match d.get("Type").and_then(|t| t.name()) {
                    Some("ObjStm") => stm.push(*n),
                    Some("XRef") => {
                        for k in ["Root", "Info", "Encrypt"] {
                            if let Some(v) = d.get(k) {
                                self.trailer.insert(k.into(), v.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for n in stm {
            if let Some(os) = self.objstm(n) {
                for (i, (on, _)) in os.index.iter().enumerate() {
                    self.xref.entry(*on).or_insert(Entry::InStm(n, i as u32));
                }
            }
        }
        self.cache.borrow_mut().clear();
        if !self.trailer.contains_key("Root") {
            for n in nums {
                let o = self.load(n);
                if o.dict().and_then(|d| d.get("Type")).and_then(|t| t.name()) == Some("Catalog") {
                    self.trailer.insert("Root".into(), Obj::Ref(n, 0));
                    break;
                }
            }
        }
    }

    // ---- objects ---------------------------------------------------------------------------------------------------------

    /// Parse `N G obj ... endobj` at `off`. `expect`: the object number it must carry.
    fn parse_indirect_at(&self, off: usize, expect: Option<u32>) -> Option<(u32, Obj)> {
        if off >= self.data.len() {
            return None;
        }
        let mut lx = Lexer::new(&self.data, off);
        let Tok::Obj(Obj::Int(n)) = lx.next() else { return None };
        let Tok::Obj(Obj::Int(_g)) = lx.next() else { return None };
        match lx.next() {
            Tok::Keyword(k) if k == "obj" => {}
            _ => return None,
        }
        if n < 0 || n > u32::MAX as i64 || expect.map(|e| e as i64 != n).unwrap_or(false) {
            return None;
        }
        let v = match parse_obj(&mut lx, true, 0) {
            Ok(Tok::Obj(o)) => o,
            Ok(Tok::Keyword(_)) | Ok(_) => Obj::Null,
            Err(_) => return None,
        };
        if let Obj::Dict(d) = &v {
            if let Tok::Keyword(k) = lx.next() {
                if k == "stream" {
                    let mut p = lx.pos;
                    if self.data.get(p) == Some(&b'\r') {
                        p += 1;
                    }
                    if self.data.get(p) == Some(&b'\n') {
                        p += 1;
                    }
                    let end = self.stream_end(d, p);
                    return Some((n as u32, Obj::Stream(d.clone(), p.min(self.data.len()), end)));
                }
            }
        }
        Some((n as u32, v))
    }

    fn stream_end(&self, d: &Dict, start: usize) -> usize {
        // Length, when direct (or an indirect object that is not being loaded), and followed by "endstream"
        let len = match d.get("Length") {
            Some(Obj::Int(n)) if *n >= 0 => Some(*n as usize),
            Some(Obj::Ref(r, _)) => match self.load(*r) {
                Obj::Int(n) if n >= 0 => Some(n as usize),
                _ => None,
            },
            _ => None,
        };
        if let Some(l) = len {
            if let Some(end) = start.checked_add(l).filter(|e| *e <= self.data.len()) {
                let mut lx = Lexer::new(&self.data, end);
                if matches!(lx.next(), Tok::Keyword(k) if k == "endstream") {
                    return end;
                }
            }
        }
        match find(&self.data, b"endstream", start) {
            Some(mut e) => {
                if e > start && self.data[e - 1] == b'\n' {
                    e -= 1;
                }
                if e > start && self.data[e - 1] == b'\r' {
                    e -= 1;
                }
                e
            }
            None => self.data.len(),
        }
    }

    /// The object with this number (Null when it does not exist or is damaged).
    pub fn load(&self, num: u32) -> Obj {
        if let Some(o) = self.cache.borrow().get(&num) {
            return o.clone();
        }
        if !self.loading.borrow_mut().insert(num) {
            return Obj::Null; // a cycle (a stream whose Length points at itself)
        }
        let o = match self.xref.get(&num).copied() {
            Some(Entry::At(off)) => self.parse_indirect_at(off, Some(num)).map(|(_, o)| o).unwrap_or(Obj::Null),
            Some(Entry::InStm(s, idx)) => self.from_objstm(s, idx, num).unwrap_or(Obj::Null),
            _ => Obj::Null,
        };
        self.loading.borrow_mut().remove(&num);
        let mut c = self.cache.borrow_mut();
        if c.len() > 400_000 {
            c.clear();
        }
        c.insert(num, o.clone());
        o
    }

    fn objstm(&self, n: u32) -> Option<Rc<ObjStm>> {
        if let Some(c) = self.objstm.borrow().get(&n) {
            return c.clone();
        }
        let built = (|| {
            let o = self.load(n);
            let Obj::Stream(d, _, _) = &o else { return None };
            let count = self.resolve(d.get("N")?).int()?.clamp(0, 1_000_000) as usize;
            let first = self.resolve(d.get("First")?).int()?.max(0) as usize;
            let dec = self.stream_data(&o).ok()?;
            let mut lx = Lexer::new(&dec.data, 0);
            let mut index = Vec::new();
            for _ in 0..count {
                let (Tok::Obj(Obj::Int(on)), Tok::Obj(Obj::Int(off))) = (lx.next(), lx.next()) else { break };
                if on >= 0 && on <= u32::MAX as i64 && off >= 0 {
                    index.push((on as u32, off as usize));
                }
            }
            Some(Rc::new(ObjStm { index, data: dec.data, first }))
        })();
        self.objstm.borrow_mut().insert(n, built.clone());
        built
    }

    fn from_objstm(&self, stm: u32, idx: u32, num: u32) -> Option<Obj> {
        let os = self.objstm(stm)?;
        let (on, off) = match os.index.get(idx as usize) {
            Some(&(on, off)) if on == num => (on, off),
            _ => os.index.iter().copied().find(|(on, _)| *on == num)?,
        };
        let _ = on;
        parse_value(&os.data, os.first.checked_add(off)?)
    }

    /// Follow references until a direct object (a chain or loop gives Null).
    pub fn resolve(&self, o: &Obj) -> Obj {
        let mut cur = o.clone();
        for _ in 0..32 {
            match cur {
                Obj::Ref(n, _) => cur = self.load(n),
                other => return other,
            }
        }
        Obj::Null
    }

    pub fn get(&self, d: &Dict, key: &str) -> Obj {
        d.get(key).map(|o| self.resolve(o)).unwrap_or(Obj::Null)
    }

    pub fn get_num(&self, d: &Dict, key: &str) -> Option<f64> {
        self.get(d, key).num()
    }

    pub fn object_count(&self) -> usize {
        self.xref.len()
    }

    pub fn all_numbers(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.xref.keys().copied().collect();
        v.sort_unstable();
        v
    }

    // ---- streams ---------------------------------------------------------------------------------------------------------

    /// Run a stream through its filters. Image filters (DCT, JPX, CCITT, JBIG2) are left to the caller.
    pub fn stream_data(&self, o: &Obj) -> Result<Decoded, PdfError> {
        let Obj::Stream(d, s, e) = o else { return Err(PdfError::Damaged("not a stream".into())) };
        let raw = self.data.get(*s..(*e).min(self.data.len())).unwrap_or(&[]);
        let names: Vec<Obj> = match self.get(d, "Filter") {
            Obj::Name(n) => vec![Obj::Name(n)],
            Obj::Arr(a) => a.iter().map(|x| self.resolve(x)).collect(),
            _ => vec![],
        };
        let parms: Vec<Obj> = match d.get("DecodeParms").or_else(|| d.get("DP")).map(|p| self.resolve(p)) {
            Some(Obj::Arr(a)) => a.iter().map(|x| self.resolve(x)).collect(),
            Some(p @ Obj::Dict(_)) => vec![p],
            _ => vec![],
        };
        let mut data = raw.to_vec();
        for (i, f) in names.iter().enumerate().take(8) {
            let name = f.name().unwrap_or("");
            let p = parms.get(i).cloned().unwrap_or(Obj::Null);
            let pd = p.dict().cloned().unwrap_or_default();
            let cap = filter::MAX_STREAM.min(self.budget.get());
            let geti = |k: &str, dflt: i64| self.get(&pd, k).int().unwrap_or(dflt);
            data = match name {
                "FlateDecode" | "Fl" | "LZWDecode" | "LZW" => {
                    let out = if name.starts_with('F') { filter::inflate(&data, cap) } else { filter::lzw(&data, geti("EarlyChange", 1) != 0, cap) };
                    let pr = geti("Predictor", 1);
                    if pr > 1 {
                        filter::predictor(out, pr, geti("Colors", 1) as usize, geti("BitsPerComponent", 8) as usize, geti("Columns", 1) as usize)
                    } else {
                        out
                    }
                }
                "ASCIIHexDecode" | "AHx" => filter::ascii_hex(&data),
                "ASCII85Decode" | "A85" => filter::ascii85(&data),
                "RunLengthDecode" | "RL" => filter::run_length(&data, cap),
                "Crypt" => data,
                "DCTDecode" | "DCT" | "JPXDecode" | "CCITTFaxDecode" | "CCF" | "JBIG2Decode" => {
                    if !self.take_budget(data.len()) {
                        return Err(PdfError::TooLarge);
                    }
                    return Ok(Decoded { data, image_filter: Some((name.to_string(), p)) });
                }
                _ => return Err(PdfError::Damaged(format!("unknown stream filter {name}"))),
            };
            if !self.take_budget(data.len()) {
                return Err(PdfError::TooLarge);
            }
        }
        Ok(Decoded { data, image_filter: None })
    }
}
