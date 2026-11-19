use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

use super::*;

const DOC: &str = "# Title here\n\nSome *bold* and `code` words with a [safe link](https://example.org/a) and a [bad one](javascript:alert(1)).\nA second line of the same paragraph.\n\n- first item\n- second item\n1. numbered\n\n> a quote\n\n```\nlet x = 1;\n```\n\n---\n![a picture](pic.png)\n[x](  DATA:text/html,hi)\n";

fn host(w: u32, h: u32, text: &str) -> Host<MarkdownApp> {
    Host::new(MarkdownApp::new("notes.md", text), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn center_of(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}

#[test]
fn blocks_are_read_marks_are_dropped_and_unsafe_links_are_counted_not_listed() {
    let d = parse(DOC);
    assert_eq!(d.blocks[0], Block::Heading(1, "Title here".into()));
    assert_eq!(d.blocks[1], Block::Para("Some bold and code words with a safe link and a bad one. A second line of the same paragraph.".into()));
    assert!(d.blocks.contains(&Block::Item("\u{2022} first item".into())) && d.blocks.contains(&Block::Item("1. numbered".into())));
    assert!(d.blocks.contains(&Block::Quote("a quote".into())) && d.blocks.contains(&Block::Code(vec!["let x = 1;".into()])) && d.blocks.contains(&Block::Rule));
    assert_eq!(d.links, vec![("safe link".to_string(), "https://example.org/a".to_string())]);
    assert_eq!(d.unsafe_links, 2, "javascript: and a shouting DATA: with spaces are both left out");
    assert_eq!(d.images, 1);
    assert!(d.blocks.iter().any(|b| *b == Block::Para("[image: a picture]".into())) || d.blocks.iter().any(|b| matches!(b, Block::Para(t) if t.contains("[image: a picture]"))));
}

#[test]
fn hostile_and_huge_texts_are_bounded_and_never_panic() {
    assert_eq!(parse("").blocks.len(), 0);
    let big = "word ".repeat(400_000);
    let d = parse(&big);
    assert!(d.blocks.len() == 1);
    let many: String = (0..2000).map(|i| format!("[l{i}](https://e.org/{i}) ")).collect();
    assert_eq!(parse(&many).links.len(), MAX_LINKS);
    for t in ["[", "![", "[a](", "[a](b", "\\", "```", "#", "# ", "> ", "1. ", "[[[]]]((()))", "é\u{0}\u{301}[é](é)"] {
        let _ = parse(t);
    }
    let multi = "é".repeat(MAX_TEXT); // cut on a character boundary
    let _ = parse(&multi);
}

#[test]
fn the_document_is_drawn_wrapped_to_the_pane_and_a_phone_gets_more_lines_than_a_desktop() {
    let mut phone = host(360, 740, DOC);
    let l = phone.record();
    assert!(l.texts().contains(&"Title here") && l.texts().contains(&"notes.md"), "{:?}", l.texts());
    let phone_lines = phone.app.lines;
    let mut wide = host(1280, 720, DOC);
    wide.record();
    assert!(phone_lines > wide.app.lines, "narrower pane, more lines ({phone_lines} vs {})", wide.app.lines);
    assert!(l.a11y().iter().any(|n| n.name == "Top") && l.a11y().iter().any(|n| n.name == "Links"));
    assert!(l.a11y().iter().any(|n| n.role == Role::Text && n.name.starts_with("notes.md: ")));
}

#[test]
fn scrolling_by_keys_wheel_and_the_top_button_and_the_end_is_the_end() {
    let long: String = (0..200).map(|i| format!("Line number {i} of a long document.\n\n")).collect();
    let mut h = host(800, 480, &long);
    h.record();
    h.event(&UiEvent::Key(UiKey::Down));
    assert_eq!(h.app.top, 1);
    h.event(&UiEvent::Key(UiKey::Char(' ')));
    assert!(h.app.top > 5, "a page down moves more than a few lines: {}", h.app.top);
    h.event(&UiEvent::Wheel(1));
    let t = h.app.top;
    h.event(&UiEvent::Wheel(-1));
    assert_eq!(h.app.top, t + 3);
    for _ in 0..2000 {
        h.event(&UiEvent::Key(UiKey::Down));
    }
    assert_eq!(h.app.top, h.app.lines - 1, "stops at the last line");
    let l = h.record();
    let (x, y) = center_of(&l, "Top");
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
    assert_eq!(h.app.top, 0);
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn the_links_page_lists_the_safe_links_and_says_what_it_left_out() {
    let mut h = host(800, 480, DOC);
    let l = h.record();
    let (x, y) = center_of(&l, "Links");
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
    let l = h.record();
    let texts = l.texts().join("\n");
    assert!(texts.contains("Links (1)") && texts.contains("safe link -> https://example.org/a"), "{texts}");
    assert!(texts.contains("2 link(s) left out"), "{texts}");
    assert!(!texts.contains("javascript") && !texts.to_lowercase().contains("data:text"), "an unsafe address is never shown");
    assert!(l.a11y().iter().any(|n| n.name == "Text"));
}

#[test]
fn opening_another_document_replaces_the_first_and_an_empty_one_says_so() {
    let mut h = host(800, 480, DOC);
    h.record();
    h.event(&UiEvent::Key(UiKey::Down));
    h.app.open("other.md\u{7}", "");
    assert_eq!(h.app.title, "other.md");
    assert_eq!(h.app.top, 0);
    assert!(h.record().texts().contains(&"This document is empty."));
    assert_eq!(h.app.title(), "MARKDOWN");
}

#[test]
fn every_size_and_text_size_draws_inside_the_pane_with_named_controls() {
    for (w, h, touch) in [(360u32, 740u32, true), (800, 480, false), (1280, 720, false)] {
        for text_scale in [1.0f32, 3.0] {
            let mut host = host(w, h, DOC);
            host.touch = touch;
            host.text_scale = text_scale;
            let l = host.record();
            for op in &l.ops {
                if let Op::Hit(r, hit) = op {
                    assert!(!hit.name.is_empty(), "an unnamed control");
                    assert!(r.x >= -0.01 && r.right() <= w as f32 + 0.01, "{} sticks out sideways at {w}x{h} x{text_scale}: {r:?}", hit.name);
                    if touch && hit.role == Role::Button {
                        assert!(r.h >= 44.0, "{} is too small to touch ({r:?})", hit.name);
                    }
                }
            }
        }
    }
}

/// The SDK rule for new apps: colours come from the Look. No colour literal, no machine access, and no IYAKKI crate but the SDK.
#[test]
fn the_app_uses_only_the_sdk_and_no_colour_of_its_own() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let code = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    let code = code.split("#[cfg(test)]").next().unwrap();
    for bad in ["rgb(", "rgba(", "std::fs", "std::net", "std::process", "iyakki_uikit", "iyakki_agent", "iyakki_shell", "\"/proc", "\"/sys", "\"/dev"] {
        assert!(!code.contains(bad), "lib.rs contains {bad}");
    }
    let m = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let mut in_deps = false;
    for l in m.lines() {
        if l.starts_with('[') {
            in_deps = l.trim() == "[dependencies]";
        } else if in_deps && l.starts_with("iyakki-") {
            assert!(l.starts_with("iyakki-ui-sdk"), "a dependency besides the SDK: {l}");
        }
    }
}

#[test]
fn the_file_list_is_chosen_by_keys_and_by_tap_never_shows_a_path_and_esc_closes_it_before_the_viewer() {
    let names: Vec<String> = (0..30).map(|i| format!("notes/day-{i}.md")).collect();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let mut hst = host(w, h, "");
        hst.app.offer(names.clone());
        assert!(hst.app.choosing());
        let l = hst.record();
        assert!(l.ops.iter().any(|o| matches!(o, Op::Hit(_, hit) if hit.name == "notes/day-0.md")), "{w}x{h}: the first row is there");
        for o in &l.ops {
            if let Op::Hit(r, _) = o {
                assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
            }
        }
        // keys: down twice, Enter chooses the third
        hst.event(&UiEvent::Key(UiKey::Down));
        hst.event(&UiEvent::Key(UiKey::Down));
        hst.event(&UiEvent::Key(UiKey::Enter));
        assert_eq!(hst.app.take_choice(), Some(2));
        assert!(!hst.app.choosing());
        assert_eq!(hst.app.take_choice(), None, "once");
    }
    // a tap on a row; the list scrolls to keep the focused row in view
    let mut hst = host(360, 400, "");
    hst.app.offer(names.clone());
    for _ in 0..25 {
        hst.event(&UiEvent::Key(UiKey::Down));
    }
    let l = hst.record();
    assert!(l.ops.iter().any(|o| matches!(o, Op::Hit(_, hit) if hit.name == "notes/day-25.md")), "the focused row is on the screen");
    let (x, y) = center_of(&l, "notes/day-25.md");
    hst.mouse(x, y, Phase::Down, 0);
    hst.mouse(x, y, Phase::Up, 40);
    assert_eq!(hst.app.take_choice(), Some(25));
    // Esc closes the list and does not leave; a second Esc leaves
    hst.app.offer(names);
    assert_eq!(hst.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(!hst.app.choosing());
    assert_eq!(hst.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
    // an empty list offers nothing; control characters in a name are dropped
    hst.app.offer(vec![]);
    assert!(!hst.app.choosing());
    hst.app.offer(vec!["a\u{7}b.md".into()]);
    assert!(hst.record().ops.iter().any(|o| matches!(o, Op::Hit(_, hit) if hit.name == "ab.md")));
}

/// F-SDK-121: an SDK app writes no styling numbers either: gaps, sizes and radii come from the skin's tokens.
#[test]
fn the_app_has_no_styling_numbers_of_its_own() {
    let bad = iyakki_ui_sdk::lint::style_literals(include_str!("lib.rs"));
    assert!(bad.is_empty(), "use m.gap, m.size, m.radius, m.line:\n{}", bad.join("\n"));
}
