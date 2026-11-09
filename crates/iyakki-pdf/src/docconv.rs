//! A page of the reader browser (`iyakki_web::doc::Doc`) as blocks for the PDF writer: headings, paragraphs, list items, quotes, code,
//! tables as aligned monospace rows, rules and the pictures the caller can supply.

use crate::write::{Block, PdfImage};
use iyakki_web::doc as d;

fn spans_text(spans: &[d::Span]) -> String {
    let mut s = String::new();
    for sp in spans {
        s.push_str(&sp.text);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `image(i)`: the picture the page's `i`-th image reference stands for, when the caller has it decoded.
pub fn blocks_from_doc(doc: &d::Doc, image: &dyn Fn(usize) -> Option<PdfImage>) -> Vec<Block> {
    let mut out = Vec::new();
    if !doc.title.trim().is_empty() {
        let first_heading = doc.blocks.iter().find_map(|b| if let d::Block::Text(t) = b { if t.level > 0 { Some(spans_text(&t.spans)) } else { None } } else { None });
        if first_heading.as_deref() != Some(doc.title.trim()) {
            out.push(Block::Heading(1, doc.title.trim().to_string()));
        }
    }
    walk(&doc.blocks, image, &mut out, 0);
    out
}

fn walk(blocks: &[d::Block], image: &dyn Fn(usize) -> Option<PdfImage>, out: &mut Vec<Block>, depth: usize) {
    if depth > 30 || out.len() > 20_000 {
        return;
    }
    for b in blocks {
        match b {
            d::Block::Text(t) => {
                for sp in &t.spans {
                    if let Some(i) = sp.image {
                        if let Some(img) = image(i) {
                            out.push(Block::Image(img));
                        }
                    }
                }
                let text = spans_text(&t.spans);
                if text.is_empty() {
                    continue;
                }
                if t.level > 0 {
                    out.push(Block::Heading(t.level, text));
                } else if !t.marker.is_empty() {
                    out.push(Block::Bullet(t.indent.saturating_sub(1).min(4), t.marker.clone(), text));
                } else if t.quote {
                    out.push(Block::Quote(text));
                } else {
                    out.push(Block::Para(text));
                }
            }
            d::Block::Pre(p) => {
                if !p.trim().is_empty() {
                    out.push(Block::Code(p.trim_end().to_string()));
                }
            }
            d::Block::Rule => out.push(Block::Rule),
            d::Block::Box(bx) => walk(&bx.children, image, out, depth + 1),
            d::Block::Row(r) => {
                for c in &r.cols {
                    walk(&c.blocks, image, out, depth + 1);
                }
            }
            d::Block::Table(t) => {
                let rows: Vec<Vec<String>> = t.rows.iter().map(|r| r.iter().map(|c| c.text()).collect()).collect();
                let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if cols == 0 {
                    continue;
                }
                let widths: Vec<usize> = (0..cols).map(|i| rows.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0).min(28)).collect();
                let mut lines = Vec::new();
                for r in &rows {
                    let mut line = String::new();
                    for (i, w) in widths.iter().enumerate() {
                        let c: String = r.get(i).map(|c| c.chars().take(*w).collect()).unwrap_or_default();
                        line.push_str(&c);
                        line.push_str(&" ".repeat(w - c.chars().count() + 2));
                    }
                    lines.push(line.trim_end().to_string());
                }
                out.push(Block::Code(lines.join("\n")));
            }
        }
    }
}
