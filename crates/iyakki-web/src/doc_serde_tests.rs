//! The page model crosses the core socket as JSON (F-SPLIT-03, D-SPLIT-03-TILES): a styled page with boxes, a gradient, a shadow, lengths of every
//! kind and the reading tree comes back equal, and a damaged answer is refused rather than half read.

use crate::css::{parse_sheet, Viewport};
use crate::doc::{build_styled, Doc};
use crate::html::parse;
use crate::style::StyleSheets;

fn styled() -> Doc {
    let css = "body{background:#102030} .card{margin:4px 8px;padding:6px;border:2px solid #ff0000;border-radius:5px;box-shadow:1px 2px 3px 1px #00000080;background:linear-gradient(90deg,#fff,#000);width:50%;max-width:calc(100% - 20px);opacity:.5} .row{display:flex;gap:4px;justify-content:space-between} .row>div{flex:1 1 30%}";
    let html = "<body><div class=card><h2>Title</h2><p>text <b>bold</b> <i>it</i> <a href='/x'>link</a></p></div><div class=row><div>a</div><div>b</div></div><ul><li>one</li></ul><input name=n value=v><textarea name=t>words</textarea><img src='p.png' alt='pic' width=4 height=5></body>";
    let vp = Viewport::default();
    let mut order = 0;
    let sheet = parse_sheet(css, &vp, &mut order);
    build_styled(&parse(html), &StyleSheets::new(&[sheet]), &vp)
}

#[test]
fn a_styled_page_comes_back_equal_through_json() {
    let doc = styled();
    assert!(doc.blocks.len() >= 3 && !doc.controls.is_empty() && !doc.images.is_empty() && doc.canvas.is_some(), "the page has boxes, controls, a picture and a canvas colour");
    let text = serde_json::to_string(&doc).unwrap();
    let back: Doc = serde_json::from_str(&text).unwrap();
    assert_eq!(back, doc);
    // the shadow and the gradient are in the model, so the round trip carried them
    assert!(text.contains("\"shadow\":{") && text.contains("\"gradient\":{"), "{}", &text[..text.len().min(400)]);
}

#[test]
fn a_page_without_a_reading_tree_comes_back_without_one_and_a_damaged_answer_is_refused() {
    let mut doc = Doc::default();
    doc.title = "t".into();
    let back: Doc = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    assert_eq!((back.title.as_str(), back.access.is_none()), ("t", true));
    let text = serde_json::to_string(&styled()).unwrap();
    assert!(serde_json::from_str::<Doc>(&text[..text.len() / 2]).is_err());
    assert!(serde_json::from_str::<Doc>(&text.replace("\"blocks\"", "\"bloks\"")).is_err());
}
