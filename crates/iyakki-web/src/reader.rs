//! Reader view: the article of a page without the menus, boxes, colours and forms around it. Works on the finished
//! [`Doc`]: it keeps headings, long paragraphs that are not mostly links, list items with sentences in them, preformatted
//! text and pictures that sit in text, and drops the rest.

use crate::doc::{Block, Doc, Span, Style, TextBlock};

/// A paragraph shorter than this (in characters) is probably a menu entry, a button or a caption.
const MIN_PARAGRAPH: usize = 70;

fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// The share of the words that are inside a link, 0 to 1.
fn link_share(spans: &[Span]) -> f32 {
    let total: usize = spans.iter().map(|s| s.text.chars().count()).sum();
    if total == 0 {
        return 1.0;
    }
    let linked: usize = spans.iter().filter(|s| s.link.is_some()).map(|s| s.text.chars().count()).sum();
    linked as f32 / total as f32
}

fn collect<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
    for b in blocks {
        match b {
            Block::Box(x) => collect(&x.children, out),
            Block::Row(r) => {
                for c in &r.cols {
                    collect(&c.blocks, out);
                }
            }
            other => out.push(other),
        }
    }
}

fn calm(spans: &[Span]) -> Vec<Span> {
    spans
        .iter()
          .filter(|s| s.control.is_none() && s.inline.is_none() && s.style.gap == 0)
        .map(|s| Span { style: Style { bold: s.style.bold, italic: s.style.italic, code: s.style.code, dim: s.style.dim, ..Style::default() }, ..s.clone() })
        .collect()
}

/// All the words of a page in reading order, one paragraph per line, cut at `max` characters (for handing to a model or a
/// search index).
pub fn plain_text(doc: &Doc, max: usize) -> String {
    let mut flat = Vec::new();
    collect(&doc.blocks, &mut flat);
    let mut out = String::new();
    for b in flat {
        let line = match b {
            Block::Text(t) => {
                let text = plain(&t.spans);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                format!("{}{}{}", if t.level > 0 { "# " } else { "" }, if t.marker.is_empty() { String::new() } else { format!("{} ", t.marker) }, text)
            }
            Block::Pre(p) => p.trim_end().to_string(),
            Block::Table(t) => t.rows.iter().map(|r| r.iter().map(|c| c.text().trim().to_string()).collect::<Vec<_>>().join(" | ")).collect::<Vec<_>>().join("\n"),
            _ => continue,
        };
        if out.chars().count() + line.chars().count() + 1 > max {
            let room = max.saturating_sub(out.chars().count());
            out.extend(line.chars().take(room));
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The reader version of a page, or `None` when no article text could be found.
pub fn simplify(doc: &Doc) -> Option<Doc> {
    let mut flat = Vec::new();
    collect(&doc.blocks, &mut flat);
    // which blocks are the article
    let keep: Vec<bool> = flat
        .iter()
        .map(|b| match b {
            Block::Text(t) if t.level == 0 => {
                let n = plain(&t.spans).trim().chars().count();
                let has_image = t.spans.iter().any(|s| s.image.is_some());
                (n >= MIN_PARAGRAPH && link_share(&t.spans) < 0.5) || (has_image && n < 400 && link_share(&t.spans) < 0.9 && n > 0) || (has_image && n == 0)
            }
            Block::Pre(p) => p.trim().len() >= 20,
            _ => false,
        })
        .collect();
    if !keep.iter().any(|k| *k) {
        return None;
    }
    // the article starts at the first kept paragraph and ends at the last, headings between them stay
    let first = keep.iter().position(|k| *k)?;
    let last = keep.iter().rposition(|k| *k)?;
    // plain pictures only count inside the article, not before its first sentence
    let mut blocks = Vec::new();
    let mut has_h1 = false;
    for (i, b) in flat.iter().enumerate() {
        match b {
            Block::Text(t) if t.level > 0 => {
                let text = plain(&t.spans);
                // a heading just before the article belongs to it
                let near = i + 3 >= first && i <= last;
                if near && !text.trim().is_empty() && link_share(&t.spans) < 0.8 {
                    has_h1 |= t.level == 1;
                    blocks.push(Block::Text(TextBlock { level: t.level, spans: calm(&t.spans), ..TextBlock::default() }));
                }
            }
            Block::Text(t) if keep[i] && i >= first => {
                blocks.push(Block::Text(TextBlock { level: 0, indent: t.indent, quote: t.quote, marker: t.marker.clone(), spans: calm(&t.spans), ..TextBlock::default() }));
            }
            Block::Pre(p) if keep[i] => blocks.push(Block::Pre(p.clone())),
            _ => {}
        }
    }
    if !has_h1 && !doc.title.trim().is_empty() {
        let title = Span { text: doc.title.trim().to_string(), style: Style { bold: true, ..Style::default() }, link: None, control: None, image: None, inline: None };
        blocks.insert(0, Block::Text(TextBlock { level: 1, spans: vec![title], ..TextBlock::default() }));
    }
    Some(Doc { title: doc.title.clone(), canvas: None, blocks, links: doc.links.clone(), controls: Vec::new(), forms: Vec::new(), images: doc.images.clone(), inlines: Vec::new(), truncated: doc.truncated, access: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::from_html as render;

    #[test]
    fn keeps_the_article_and_drops_menus_boxes_and_forms() {
        let long = "This is a long sentence that belongs to the article and goes on for a while so that it is clearly text. ";
        let html = format!(
            "<html><head><title>My Story</title></head><body><nav><a href=a>Home</a> <a href=b>News</a> <a href=c>About us and more things</a></nav>\
             <div style='background:red'><h2>Chapter</h2><p style='color:blue'>{long}{long}</p><p>{long}</p></div>\
             <form><input name=q><button>Go</button></form><footer><a href=x>Privacy</a></footer></body></html>"
        );
        let doc = render(&html);
        let r = simplify(&doc).expect("an article");
        let texts: Vec<String> = r.blocks.iter().map(|b| match b { Block::Text(t) => plain(&t.spans), _ => String::new() }).collect();
        assert_eq!(texts[0], "My Story", "{texts:?}");
        assert!(texts.iter().any(|t| t == "Chapter"));
        assert_eq!(texts.iter().filter(|t| t.starts_with("This is a long")).count(), 2);
        assert!(!texts.iter().any(|t| t.contains("Privacy") || t.contains("Home")));
        assert!(r.controls.is_empty());
        for b in &r.blocks {
            if let Block::Text(t) = b {
                assert!(t.spans.iter().all(|s| s.style.color.is_none() && s.style.bg.is_none()));
            }
        }
    }

    #[test]
    fn plain_text_reads_in_order_and_stops_at_the_limit() {
        let doc = render("<h1>Title</h1><p>First paragraph.</p><ul><li>one</li></ul><p>Last paragraph here.</p>");
        let t = plain_text(&doc, 10_000);
        assert!(t.starts_with("# Title\nFirst paragraph.\n"), "{t:?}");
        assert!(t.contains("one") && t.trim_end().ends_with("Last paragraph here."));
        assert_eq!(plain_text(&doc, 12).chars().count(), 12);
    }

    #[test]
    fn a_page_of_links_has_no_article() {
        let doc = render("<body><p><a href=a>one</a></p><p><a href=b>two</a></p></body>");
        assert!(simplify(&doc).is_none());
    }
}
