//! A story as a PDF: text, headings and pictures in place, read back with the repo's own reader; other scripts are refused.

use crate::story::{parse_story, StoredPic, StoryOptions};
use crate::storypdf::*;
use iyakki_pdf::{BoxGlyphs, Document, PageSize};

const TEXT: &str = "# The Brave Little Fox\n\n## Chapter one\n\nOnce upon a time a small fox lived near the river.\n\nShe was afraid of the dark.\n\n## Chapter two\n\nOne night an owl said do not worry.\n";

fn svg(colour: &str) -> Vec<u8> {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"200\" viewBox=\"0 0 200 200\"><rect width=\"200\" height=\"200\" fill=\"{colour}\"/></svg>").into_bytes()
}

fn pic(unit: usize, ext: &str, bytes: Vec<u8>) -> StoredPic {
    StoredPic { scene_text: String::new(), unit, title: format!("scene {unit}"), prompt: String::new(), style: "watercolor".into(), mode: "svg".into(), ext: ext.into(), bytes, source: String::new(), size: String::new(), variants: vec![], look: String::new(), seed: None, provider: String::new(), model: String::new(), ms: 0 }
}

fn render(d: &Document, page: usize) -> Vec<[u8; 3]> {
    d.render(page, 1.0, &BoxGlyphs).unwrap().rgba.chunks(4).map(|p| [p[0], p[1], p[2]]).collect()
}

#[test]
fn a_story_becomes_a_pdf_with_its_text_and_pictures_in_place() {
    let story = parse_story(TEXT, true);
    let mut o = StoryOptions::default();
    o.topic = "a fox".into();
    let base = story.unit_base();
    let pics = vec![pic(base, "svg", svg("#ff0000")), pic(base + 2, "svg", svg("#0000ff")), pic(base + 3, "mp4", vec![1, 2, 3])];
    let made = story_pdf(&story, &o, &pics, PageSize::A4, Some("D:20261112000000".into())).unwrap();
    assert_eq!((made.pictures, made.left_out), (2, 1), "the video is left out and said so");
    let d = Document::open(made.bytes.clone()).unwrap();
    assert_eq!(d.page_count(), made.pages);
    assert_eq!(d.info().title, "The Brave Little Fox");
    let text: String = (0..d.page_count()).map(|i| d.page_text(i).unwrap()).collect::<Vec<_>>().join("\n");
    for want in ["The Brave Little Fox", "Chapter one", "Once upon a time a small fox lived near the river.", "Chapter two", "an owl said do not worry"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(text.find("Chapter one").unwrap() < text.find("Chapter two").unwrap());
    let px = render(&d, 0);
    assert!(px.iter().any(|p| p[0] > 240 && p[1] < 20 && p[2] < 20), "the cover picture (red) is on the first page");
    let all: Vec<[u8; 3]> = (0..d.page_count()).flat_map(|i| render(&d, i)).collect();
    assert!(all.iter().any(|p| p[2] > 240 && p[0] < 20 && p[1] < 20), "the second picture (blue) is in the file");
    // the cover comes before the first paragraph, the second picture after it
    let (blocks, _, _) = story_blocks(&story, &o, &pics);
    let first_img = blocks.iter().position(|b| matches!(b, iyakki_pdf::Block::Image(_))).unwrap();
    let first_para = blocks.iter().position(|b| matches!(b, iyakki_pdf::Block::Para(p) if p.starts_with("Once upon"))).unwrap();
    assert!(first_img < first_para);
}

#[test]
fn a_story_without_pictures_or_in_another_script_is_handled_in_plain_words() {
    let story = parse_story(TEXT, true);
    let made = story_pdf(&story, &StoryOptions::default(), &[], PageSize::Letter, None).unwrap();
    assert_eq!((made.pictures, made.left_out), (0, 0));
    assert_eq!(Document::open(made.bytes).unwrap().page_size(0).unwrap().0, 612.0);
    // Tamil is shaped now (F-PDF-16); a script with no bundled font is still refused in plain words
    let tamil = parse_story("# \u{0b95}\u{0ba4}\u{0bc8}\n\n\u{0b92}\u{0bb0}\u{0bc1} \u{0b95}\u{0bbe}\u{0bb2}\u{0ba4}\u{0bcd}\u{0ba4}\u{0bbf}\u{0bb2}\u{0bcd}\n", true);
    assert!(story_pdf(&tamil, &StoryOptions::default(), &[], PageSize::A4, None).is_ok(), "Tamil is written with the shaped layout");
    // Armenian, Georgian and Ethiopic are drawn now; Chinese, Japanese and Korean still have no glyphs
    let ethiopic = parse_story("# \u{1230}\u{120b}\u{121d}\n\n\u{1230}\u{120b}\u{121d} \u{12d3}\u{1208}\u{121d}\n", true);
    assert!(story_pdf(&ethiopic, &StoryOptions::default(), &[], PageSize::A4, None).is_ok(), "Ethiopic is written now");
    let chinese = parse_story("# \u{4e2d}\u{6587}\n\n\u{4e2d}\u{6587} \u{6545}\u{4e8b}\n", true);
    let e = story_pdf(&chinese, &StoryOptions::default(), &[], PageSize::A4, None).err().expect("refused");
    assert!(e.contains("cannot be written in a PDF yet") && e.contains("Nothing was saved"), "{e}");
    assert!(story_pdf(&parse_story("", true), &StoryOptions::default(), &[], PageSize::A4, None).err().unwrap().contains("no story"));
}

#[test]
fn the_store_keeps_story_pdf_beside_story_md_and_replaces_only_its_own_copy() {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-storypdf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut policy = iyakki_sec::policy::Role::User.template();
    policy.fs_read = vec![dir.display().to_string()];
    policy.fs_write = vec![dir.display().to_string()];
    let store = crate::storykit::open_at(&policy, "tester", &dir.join("svc"), dir.clone());
    assert!(store.save_pdf("nothing", b"%PDF").is_err(), "a story that is not saved has no folder to hold a PDF");
    let story = parse_story(TEXT, true);
    let e = store.save(&story, &StoryOptions::default(), 1_700_000_000).unwrap();
    assert!(store.pdf_path(&e.id).is_none());
    let (p, replaced) = store.save_pdf(&e.id, b"%PDF-first").unwrap();
    assert!(!replaced && p.ends_with(format!("Stories/{}/story.pdf", e.id)) && p.parent().unwrap().join("story.md").is_file());
    let (_, replaced) = store.save_pdf(&e.id, b"%PDF-second").unwrap();
    assert!(replaced);
    assert_eq!(std::fs::read(store.pdf_path(&e.id).unwrap()).unwrap(), b"%PDF-second");
    assert!(!p.with_extension("pdf.part").exists());
    store.delete(&e.id).unwrap();
    assert!(!dir.join(format!("Stories/{}", e.id)).exists(), "the PDF goes with the story");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_request_for_the_file_counts() {
    for yes in ["make it a PDF", "Save this as a pdf please", "export to PDF", "can you give me a PDF of the story?", "story to pdf"] {
        assert!(asks_for_pdf(yes), "{yes}");
    }
    for no in ["make it shorter", "the fox reads a pdf in the library", "change the fox's name to Beeru", "pdfs are fun"] {
        assert!(!asks_for_pdf(no), "{no}");
    }
}

#[test]
fn a_greek_story_is_written_with_the_embedded_font_and_reads_back() {
    let story = parse_story("# \u{397} \u{3a0}\u{3b1}\u{3c1}\u{3b1}\u{3bc}\u{3c5}\u{3b8}\u{3b9}\u{3ac}\n\n\u{39c}\u{3b9}\u{3b1} \u{3c6}\u{3bf}\u{3c1}\u{3ac} \u{3ae}\u{3c4}\u{3b1}\u{3bd} \u{3ad}\u{3bd}\u{3b1}\u{3c2} \u{3bc}\u{3b9}\u{3ba}\u{3c1}\u{3cc}\u{3c2} \u{3b1}\u{3bb}\u{3b5}\u{3c0}\u{3bf}\u{3cd}.\n", true);
    let made = story_pdf(&story, &StoryOptions::default(), &[pic(story.unit_base(), "svg", svg("#ff0000"))], PageSize::A4, None).unwrap();
    let d = Document::open(made.bytes).unwrap();
    let t = d.page_text(0).unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(t.contains("\u{397} \u{3a0}\u{3b1}\u{3c1}\u{3b1}\u{3bc}\u{3c5}\u{3b8}\u{3b9}\u{3ac}") && t.contains("\u{3ad}\u{3bd}\u{3b1}\u{3c2} \u{3bc}\u{3b9}\u{3ba}\u{3c1}\u{3cc}\u{3c2} \u{3b1}\u{3bb}\u{3b5}\u{3c0}\u{3bf}\u{3cd}."), "{t}");
    assert_eq!(made.pictures, 1);
}
