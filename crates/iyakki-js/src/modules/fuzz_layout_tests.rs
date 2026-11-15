//! Fuzz test (deterministic, seeded) for the box estimate scripts ask sizes from: random DOM trees with random styles go through the real
//! parser, style computation and `boxes::layout`. A panic, a non-finite or negative size, a case slower than the guard or a case over the
//! heap bound fails the test and prints the seed. `IYAKKI_FUZZ_CASES=5000 cargo test -p iyakki-js fuzz` runs a larger set.

use super::fuzzkit::{cases, reset_peak, peak_above_start, live, Rng};
use iyakki_web::css::Viewport;
use iyakki_web::html;
use iyakki_web::style::{self, StyleSheets};
use iyakki_web::Url;
use std::time::{Duration, Instant};

const TAGS: [&str; 22] = ["div", "span", "p", "ul", "li", "table", "tr", "td", "img", "input", "button", "a", "section", "h1", "b", "canvas", "iframe", "textarea", "select", "form", "svg", "pre"];
const LEN: [&str; 24] = ["0", "1px", "10px", "-5px", "50%", "-20%", "2em", "1.5rem", "10vw", "50vh", "auto", "calc(100% - 10px)", "calc(1px + 2em)", "99999999px", "1e9px", "1e30px", "0.0001px", "inherit", "min-content", "fit-content", "100%", "300px", "1e-30px", "-1e9px"];
const PROPS: [&str; 30] = [
    "width", "height", "min-width", "max-width", "min-height", "max-height", "margin", "margin-left", "margin-top", "padding", "padding-left", "border-width", "top", "left", "right", "bottom", "gap", "flex-basis", "font-size", "line-height", "text-indent", "letter-spacing", "border", "outline-width", "margin-right", "margin-bottom", "padding-top", "padding-bottom", "column-gap", "row-gap",
];
const KW: [(&str, &[&str]); 12] = [
    ("display", &["block", "inline", "inline-block", "flex", "inline-flex", "grid", "none", "table", "table-row", "table-cell", "contents", "list-item"]),
    ("position", &["static", "relative", "absolute", "fixed", "sticky"]),
    ("float", &["left", "right", "none"]),
    ("clear", &["both", "left", "right", "none"]),
    ("flex-direction", &["row", "column", "row-reverse", "column-reverse"]),
    ("flex-wrap", &["wrap", "nowrap", "wrap-reverse"]),
    ("justify-content", &["center", "space-between", "space-around", "flex-end", "flex-start"]),
    ("box-sizing", &["border-box", "content-box"]),
    ("grid-template-columns", &["repeat(3, 1fr)", "repeat(99999, 1fr)", "1fr 2fr", "100px auto", "repeat(0, 1fr)", "repeat(-1, 1fr)", "none"]),
    ("flex", &["1 1 0", "0 0 auto", "99999 99999 1px", "-1 -1 -1px", "1", "none", "auto"]),
    ("overflow", &["hidden", "auto", "scroll", "visible"]),
    ("white-space", &["nowrap", "pre", "normal", "pre-wrap"]),
];

fn decl(r: &mut Rng) -> String {
    if r.chance(3) {
        let (k, vs) = KW[r.below(KW.len())];
        format!("{k}:{};", vs[r.below(vs.len())])
    } else {
        let mut v = LEN[r.below(LEN.len())].to_string();
        if r.chance(4) {
            v = format!("{} {}", v, LEN[r.below(LEN.len())]);
        }
        format!("{}:{v};", PROPS[r.below(PROPS.len())])
    }
}

fn gen_node(r: &mut Rng, out: &mut String, depth: usize, max_depth: usize, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    if r.chance(5) {
        out.push_str(["text ", "a longer piece of text that has to wrap at some point ", "", "\u{3b1}\u{3b2} ", "x".repeat(r.below(400)).as_str()][r.below(5)]);
        return;
    }
    let tag = TAGS[r.below(TAGS.len())];
    out.push('<');
    out.push_str(tag);
    if r.chance(2) {
        out.push_str(" style=\"");
        for _ in 0..1 + r.below(5) {
            out.push_str(&decl(r));
        }
        out.push('"');
    }
    if r.chance(3) {
        out.push_str(&format!(" class=\"c{} c{}\"", r.below(6), r.below(6)));
    }
    if r.chance(6) {
        out.push_str(" hidden");
    }
    out.push('>');
    if depth < max_depth {
        for _ in 0..r.below(5) {
            gen_node(r, out, depth + 1, max_depth, budget);
        }
    }
    if r.chance(6) {
        return; // left open on purpose
    }
    out.push_str(&format!("</{tag}>"));
}

fn gen_page(r: &mut Rng) -> String {
    let mut css = String::new();
    for k in 0..r.below(8) {
        css.push_str(&format!(".c{k}{{"));
        for _ in 0..1 + r.below(5) {
            css.push_str(&decl(r));
        }
        css.push('}');
        if r.chance(4) {
            css.push_str(&format!("@media (max-width:{}px){{.c{k}{{{}}}}}", r.below(3000), decl(r)));
        }
    }
    let mut body = String::new();
    // mostly broad and shallow, sometimes narrow and deep (nesting that tries the recursion caps)
    let (max_depth, mut budget) = match r.below(6) {
        0 => (400 + r.below(3000), 3000),
        1 => (30, 400),
        _ => (6 + r.below(8), 250),
    };
    while budget > 0 {
        let before = budget;
        gen_node(r, &mut body, 0, max_depth, &mut budget);
        if budget == before {
            break;
        }
    }
    format!("<!doctype html><html><head><style>{css}</style></head><body>{body}</body></html>")
}

#[test]
fn fuzz_layout_estimate_on_random_pages() {
    let mut r = Rng::new(0xB0C5);
    let base = Url::parse("https://a.org/").unwrap();
    let mut laid_boxes = 0usize;
    for case in 0..cases(250) {
        let seed_state = r.0;
        let page = gen_page(&mut r);
        let vp = match r.below(4) {
            0 => Viewport { width: 320.0, height: 480.0, ..Viewport::default() },
            1 => Viewport { width: 1.0, height: 1.0, ..Viewport::default() },
            2 => Viewport { width: 8000.0, height: 6000.0, ..Viewport::default() },
            _ => Viewport::default(),
        };
        let _g = Guard(format!("layout fuzz: case {case} rng state {seed_state:#x}, page of {} bytes starts {:?}", page.len(), page.chars().take(300).collect::<String>()));
        let start = live();
        reset_peak();
        let t = Instant::now();
        let tree = html::parse(&page);
        let none = |_: &Url| -> Option<String> { None };
        let sheets = style::collect_sheets(&tree, &base, &vp, &none);
        let ss = StyleSheets::new(&sheets);
        let styles = style::compute(&tree, &ss, &vp);
        let b = crate::modules::boxes::layout(&tree, &styles, &vp, ss.is_empty());
        let took = t.elapsed();
        assert!(took < Duration::from_secs(10), "case {case} took {took:?} for {} nodes", tree.nodes.len());
        let peak = peak_above_start(start);
        assert!(peak < 1 << 30, "case {case} used {peak} bytes");
        assert_eq!(b.rects.len(), tree.nodes.len());
        assert!(b.height.is_finite(), "case {case}: document height {}", b.height);
        for (n, rect) in b.rects.iter().enumerate() {
            if let Some(q) = rect {
                laid_boxes += 1;
                assert!([q.x, q.y, q.w, q.h].iter().all(|v| v.is_finite()), "case {case}: node {n} ({:?}) has a non-finite box {q:?}", tree.tag(n));
                assert!(q.w >= 0.0 && q.h >= 0.0, "case {case}: node {n} ({:?}) has a negative size {q:?}", tree.tag(n));
            }
        }
    }
    assert!(laid_boxes > 1000, "the pages must produce boxes ({laid_boxes})");
}

struct Guard(String);
impl Drop for Guard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("FUZZ FINDING: {}", self.0);
        }
    }
}
