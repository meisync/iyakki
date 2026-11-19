//! Tiny PDFs built in the tests (no files committed).

use crate::filter::zlib_stored;

/// A classic-xref PDF from `(number, body)` pairs; `root` is the catalog's number.
pub fn assemble(objects: &[(u32, Vec<u8>)], root: u32, extra_trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let max = objects.iter().map(|o| o.0).max().unwrap_or(0);
    let mut offs = vec![0usize; max as usize + 1];
    for (n, body) in objects {
        offs[*n as usize] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", max + 1).as_bytes());
    for n in 1..=max {
        if offs[n as usize] == 0 {
            out.extend_from_slice(b"0000000000 65535 f \n");
        } else {
            out.extend_from_slice(format!("{:010} 00000 n \n", offs[n as usize]).as_bytes());
        }
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root {root} 0 R {extra_trailer} >>\nstartxref\n{xref}\n%%EOF\n", max + 1).as_bytes());
    out
}

pub fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

/// One page per content string, all with Helvetica as /F1 and a 200 x 100 page.
pub fn simple(pages: &[&str]) -> Vec<u8> {
    let mut objs: Vec<(u32, Vec<u8>)> = vec![(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec())];
    let kids: String = (0..pages.len()).map(|i| format!("{} 0 R ", 4 + 2 * i)).collect();
    objs.push((2, format!("<< /Type /Pages /Kids [{kids}] /Count {} /MediaBox [0 0 200 100] >>", pages.len()).into_bytes()));
    objs.push((3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()));
    objs.push((100, b"<< /Title (Test \\(doc\\)) /Author (Someone) >>".to_vec()));
    for (i, c) in pages.iter().enumerate() {
        objs.push((4 + 2 * i as u32, format!("<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 5 + 2 * i).into_bytes()));
        objs.push((5 + 2 * i as u32, stream("", c.as_bytes())));
    }
    assemble(&objs, 1, "/Info 100 0 R")
}

/// A PDF 1.5 file: the catalog, page tree, page and font live in an object stream; the cross-reference is a stream.
pub fn objstm_file(content: &str) -> Vec<u8> {
    let bodies = [
        (1u32, "<< /Type /Catalog /Pages 2 0 R >>"),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>"),
        (3, "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"),
        (4, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
    ];
    let mut header = String::new();
    let mut data = String::new();
    for (n, b) in &bodies {
        header.push_str(&format!("{n} {} ", data.len()));
        data.push_str(b);
        data.push('\n');
    }
    let first = header.len();
    let all = format!("{header}{data}");
    let z = zlib_stored(all.as_bytes());
    let mut out = b"%PDF-1.5\n".to_vec();
    let o6 = out.len();
    out.extend_from_slice(b"6 0 obj\n");
    out.extend_from_slice(&stream(&format!("/Type /ObjStm /N {} /First {first} /Filter /FlateDecode", bodies.len()), &z));
    out.extend_from_slice(b"\nendobj\n");
    let o5 = out.len();
    out.extend_from_slice(b"5 0 obj\n");
    out.extend_from_slice(&stream("", content.as_bytes()));
    out.extend_from_slice(b"\nendobj\n");
    let o7 = out.len();
    // xref stream, W [1 2 1]: type, field2, field3
    let mut x: Vec<u8> = Vec::new();
    let mut rec = |t: u8, a: usize, b: u8| {
        x.push(t);
        x.extend_from_slice(&(a as u16).to_be_bytes());
        x.push(b);
    };
    rec(0, 0, 255);
    for i in 0..4u8 {
        rec(2, 6, i);
    }
    rec(1, o5, 0);
    rec(1, o6, 0);
    rec(1, o7, 0);
    out.extend_from_slice(b"7 0 obj\n");
    out.extend_from_slice(&stream("/Type /XRef /Size 8 /W [1 2 1] /Root 1 0 R", &x));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{o7}\n%%EOF\n").as_bytes());
    out
}

/// A cheap deterministic generator for the mutation tests.
pub struct Lcg(pub u64);
impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

pub fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}
