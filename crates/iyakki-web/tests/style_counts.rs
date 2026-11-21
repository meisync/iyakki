//! Style speed measured in operations, not seconds (F-BROX-15, F-BROW-99 style): how many element styles are worked out in full and how
//! many are taken from a look-alike. The numbers do not move with the machine's load, so the budgets are plain assertions.

use iyakki_web::css::Viewport;
use iyakki_web::html;
use iyakki_web::style;
use iyakki_web::url::Url;

fn page(blocks: usize) -> String {
    let mut s = String::from("<html><head><style>body{margin:8px}.c0{color:#223}.c1{color:#322;background:#eef}.c2{margin:4px 0}.c3{border:1px solid #ccc}.c4 b{color:#a00}.c5 a{color:#06c}.c6{padding:4px}p{margin:2px 0}</style></head><body>");
    for i in 0..blocks {
        s.push_str(&format!("<div class=\"c{}\"><h3>Item {i}</h3><p>Some text with <b>bold</b> and <a href=\"/p/{i}\">a link</a>.</p></div>", i % 7));
    }
    s.push_str("</body></html>");
    s
}

fn counts(blocks: usize) -> ((u64, u64), usize) {
    let url = Url::parse("https://counts.test/").unwrap();
    let vp = Viewport::default();
    let tree = html::parse(&page(blocks));
    let sheets = style::collect_sheets(&tree, &url, &vp, &|_: &Url| None);
    let ss = style::StyleSheets::new(&sheets);
    style::reset_style_counts();
    let cs = style::compute(&tree, &ss, &vp);
    (style::style_counts(), cs.iter().flatten().count())
}

#[test]
fn the_styles_worked_out_in_full_stay_the_same_however_many_look_alike_blocks_the_page_has() {
    let ((full_small, shared_small), n_small) = counts(70);
    let ((full_big, shared_big), n_big) = counts(4200);
    assert_eq!(full_small as usize + shared_small as usize + 1, n_small, "every element is counted once (the root is neither)") ;
    assert_eq!(full_big as usize + shared_big as usize + 1, n_big);
    assert_eq!(full_small, full_big, "the work grows with the kinds of element, not with their number");
    // seven kinds of block, five elements in each, plus the page's own elements
    assert!(full_big <= 7 * 5 + 6, "{full_big} styles in full");
    assert!(shared_big > 20 * full_big, "{shared_big} copies against {full_big} full styles");
}

#[test]
fn an_element_with_its_own_style_is_never_shared_and_counts_as_full() {
    let url = Url::parse("https://counts.test/").unwrap();
    let vp = Viewport::default();
    let mut src = String::from("<html><body>");
    for i in 0..50 {
        src.push_str(&format!("<p style=\"color:#{:03x}\">x</p>", i));
    }
    src.push_str("</body></html>");
    let tree = html::parse(&src);
    let sheets = style::collect_sheets(&tree, &url, &vp, &|_: &Url| None);
    let ss = style::StyleSheets::new(&sheets);
    style::reset_style_counts();
    let _ = style::compute(&tree, &ss, &vp);
    let (full, shared) = style::style_counts();
    assert!(full >= 50 && shared == 0, "{full} {shared}");
}
