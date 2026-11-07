//! Tests of the "Mark done" button of a card in `review`: the person's OK sets the row done and records it as verified (a person's
//! decision), a read-only workspace shows no button, and the button sits inside its card at the four sizes. A child of the board's module.

use iyakki_agent::boardtrack::Tracker;

use super::*;

const SIZES: [(f32, f32, f32); 4] = [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 1.5), (1024.0, 600.0, 0.8), (800.0, 480.0, 0.62)];

fn review_world(tag: &str) -> (BoardView, PathBuf, PathBuf) {
    let (root, f) = super::tests::world(tag);
    let text = std::fs::read_to_string(&f).unwrap().replace("| F-B-01 | Gamma | P3 | planned | P2 | 0% | — |  |", "| F-B-01 | Gamma | P3 | review | P2 | 80% | — | work claimed; no automatic check applies, so it needs your OK |");
    assert!(text.contains("| review |"));
    std::fs::write(&f, text).unwrap();
    let mut v = BoardView::new(&root, &f);
    v.autosave_after = Duration::ZERO;
    (v, root, f)
}

fn draw_at(v: &mut BoardView, w: f32, h: f32, s: f32) -> Gfx {
    let mut g = Gfx::new(w as u32, h as u32);
    g.clear(rgb(8, 11, 26));
    v.draw(&mut g, R::new(0.0, 0.0, w, h), s, true, CYAN, 7);
    g
}

fn status_of(f: &Path, id: &str) -> String {
    iyakki_agent::featureboard::Board::parse(&std::fs::read_to_string(f).unwrap()).get(id).unwrap().status
}

#[test]
fn clicking_mark_done_on_a_review_card_sets_it_done_verified_and_saved() {
    let (mut v, root, f) = review_world("review-done");
    assert_eq!(status_of(&f, "F-B-01"), "review");
    let tracker = Tracker::new(&root, &f);
    assert!(!tracker.is_verified("F-B-01"));
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    let hits: Vec<_> = g.hits.iter().filter(|h| h.label == "Mark done").collect();
    assert_eq!(hits.len(), 1, "one review row, one button: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    let ev = hits[0].ev.clone();
    assert!(matches!(v.handle(&ev), BoardAct::Used));
    assert_eq!(v.file.as_ref().unwrap().board.get("F-B-01").unwrap().status, "done");
    assert!(tracker.is_verified("F-B-01"), "recorded as verified by the person");
    // the edit is written by the idle save; then the file on disk has it and the other rows are untouched
    assert!(v.is_dirty());
    v.tick();
    assert!(!v.is_dirty());
    assert_eq!(status_of(&f, "F-B-01"), "done", "saved");
    assert_eq!(status_of(&f, "F-A-02"), "partial");
    assert_eq!(status_of(&f, "F-B-02"), "planned");
    // the button is gone once the row is done
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert!(!g.hits.iter().any(|h| h.label == "Mark done"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_read_only_workspace_shows_no_mark_done_button_and_changes_nothing() {
    let (mut v, root, f) = review_world("review-ro");
    v.read_only = true;
    let before = std::fs::read_to_string(&f).unwrap();
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert!(!g.hits.iter().any(|h| h.label == "Mark done"));
    // the event itself (a stale button, a key) changes nothing either
    assert!(matches!(v.mark_reviewed_done("F-B-01"), BoardAct::Used));
    assert_eq!(v.file.as_ref().unwrap().board.get("F-B-01").unwrap().status, "review");
    assert!(!Tracker::new(&root, &f).is_verified("F-B-01"));
    assert_eq!(std::fs::read_to_string(&f).unwrap(), before, "the file is untouched");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_mark_done_button_sits_inside_its_card_at_the_four_sizes() {
    let (mut v, root, _f) = review_world("review-sizes");
    for (w, h, s) in SIZES {
        let g = draw_at(&mut v, w, h, s);
        let b = g.hits.iter().find(|x| x.label == "Mark done").unwrap_or_else(|| panic!("{w}x{h}: no Mark done button"));
        assert!(b.rect.w > 0.0 && b.rect.h > 0.0, "{w}x{h}: {:?}", b.rect);
        let card = v.card_rects.iter().find(|(id, _)| id == "F-B-01").unwrap_or_else(|| panic!("{w}x{h}: no card rect")).1;
        let (bx, by) = (b.rect.x, b.rect.y);
        assert!(bx >= card.x - 0.5 && by >= card.y - 0.5 && bx + b.rect.w <= card.x + card.w + 0.5 && by + b.rect.h <= card.y + card.h + 0.5, "{w}x{h} s{s}: button {:?} leaves its card {:?}", b.rect, card);
        assert!(b.rect.x + b.rect.w <= w + 0.5 && b.rect.y + b.rect.h <= h + 0.5, "{w}x{h}: leaves the screen {:?}", b.rect);
    }
    let _ = std::fs::remove_dir_all(&root);
}
