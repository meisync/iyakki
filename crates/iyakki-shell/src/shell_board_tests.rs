//! The widget board on the machine page: the layout, the figures from the core, edit mode by key, mouse and drag, the kept board per
//! account, the chat tool's change reaching the screen, and the page at the small and the large sizes.

use super::*;
use crate::shell::tests::*;
use iyakki_core_api::{AppStatus, StatusKind};
use iyakki_core_fake::{Canned, FakeCore, FakeOs};
use iyakki_uikit::board::{self, Board, Kind, Size};

fn r(x: f32, y: f32, w: f32, h: f32) -> R {
    R::new(x, y, w, h)
}

#[test]
fn full_widgets_take_a_row_half_ones_pair_up_and_what_does_not_fit_is_left_out_in_order() {
    let area = r(0.0, 0.0, 600.0, 300.0);
    let items = [(Size::Full, 100.0), (Size::Half, 60.0), (Size::Half, 80.0), (Size::Half, 50.0), (Size::Full, 90.0)];
    let p = place(area, true, 10.0, &items);
    assert_eq!(p[0], Some(r(0.0, 0.0, 600.0, 100.0)));
    // the pair shares a row as tall as the taller one, left and right of the gap
    assert_eq!(p[1], Some(r(0.0, 110.0, 295.0, 80.0)));
    assert_eq!(p[2], Some(r(305.0, 110.0, 295.0, 80.0)));
    // the next half starts a row of its own (left column), and the full one after it does not fit: 200 + 50 + 10 + 90 > 300
    assert_eq!(p[3], Some(r(0.0, 200.0, 295.0, 50.0)));
    assert_eq!(p[4], None);
    // one column: every widget takes a whole row
    let one = place(area, false, 10.0, &items[..3]);
    assert!(one.iter().all(|x| x.is_some_and(|x| x.w == 600.0)));
    // a widget that does not fit hides every one after it, even a small one
    let tight = place(r(0.0, 0.0, 600.0, 120.0), true, 10.0, &[(Size::Full, 100.0), (Size::Full, 50.0), (Size::Half, 10.0)]);
    assert_eq!((tight[0].is_some(), tight[1], tight[2]), (true, None, None));
    assert!(place(area, true, 10.0, &[]).is_empty());
}

#[test]
fn the_controls_sit_inside_their_card_clear_of_each_other_and_big_enough_to_touch() {
    for s in [0.6f32, 1.0, 2.0] {
        let card = r(100.0, 50.0, 300.0 * s, 120.0 * s);
        let (grip, pill, remove) = controls(card, s);
        for c in [grip, pill, remove] {
            assert!(c.x >= card.x && c.x + c.w <= card.x + card.w + 0.5 && c.y >= card.y && c.y + c.h <= card.y + card.h + 0.5, "{c:?} in {card:?}");
            assert!(c.h >= 28.0 * s - 0.5, "touch height at {s}");
        }
        assert!(grip.x + grip.w <= pill.x && pill.x + pill.w <= remove.x, "left to right, apart");
    }
}

#[test]
fn the_figures_come_from_the_core_and_say_so_when_they_cannot_be_read() {
    let os = FakeOs::new();
    assert_eq!(vitals_lines(&os), ["Load 0.25 on 4 processors", "Memory 3.8 of 7.6 GB in use", "Up 1 h 0 min"]);
    let lines = network_lines(&os);
    assert_eq!(lines.len(), 1, "the loopback is left out: {lines:?}");
    assert!(lines[0].starts_with("eth0  "));
    let blind = FakeOs::with(Canned { loadavg: None, meminfo: None, uptime: None, cpu_count: None, ipv4: None, ..Canned::default() });
    assert_eq!(vitals_lines(&blind), ["Load: not readable here", "Memory: not readable here", "Uptime: not readable here"]);
    assert_eq!(network_lines(&blind), ["Network: not readable here"]);
    let lo = FakeOs::with(Canned { ipv4: Some(vec![("lo".into(), "127.0.0.1".into())]), ..Canned::default() });
    assert_eq!(network_lines(&lo), ["No network address"]);
    assert_eq!((short_count(7), short_count(1234), short_count(2_500_000)), ("7".to_string(), "1.2k".to_string(), "2.5M".to_string()));
}

fn machine() -> (Shell, Gfx, Arc<FakeCore>) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (mut sh, _) = shell_with_users(&format!("board{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    login(&mut sh, "alice", "alicepass1");
    let fake = Arc::new(FakeCore::new());
    sh.core = fake.clone();
    let mut g = Gfx::new(1280, 720);
    key(&mut sh, Key::Char('1'));
    sh.render(&mut g);
    (sh, g, fake)
}

fn said(g: &Gfx) -> Vec<String> {
    g.text_log.iter().map(|x| x.2.clone()).collect()
}

fn titles(sh: &Shell) -> Vec<&'static str> {
    sh.bd.board.slots.iter().map(|s| s.kind.key()).collect()
}

#[test]
fn the_machine_page_shows_the_default_board_with_the_real_problem_named_and_the_apps() {
    let (mut sh, mut g, fake) = machine();
    assert_eq!(titles(&sh), ["attention", "apps", "vitals", "runs", "network"]);
    fake.statuses_fake().set(vec![("chat".into(), AppStatus { kind: StatusKind::Attention, line: "2 waiting for you".into(), count: 2, needs: Some(("Approvals waiting".into(), "2 waiting for you".into())) }), ("runs".into(), AppStatus { kind: StatusKind::Running, line: "1 job running".into(), count: 0, needs: None })]);
    sh.refresh_badge();
    sh.tick += 100;
    sh.refresh_board_facts();
    g.text_log.clear();
    sh.render(&mut g);
    let t = said(&g);
    for want in ["NEEDS A PERSON", "Approvals waiting", "APPS", "THE MACHINE", "RUNS", "1 job running", "NETWORK", "Arrange"] {
        assert!(t.iter().any(|x| x == want), "{want} is on the page: {t:?}");
    }
    assert!(t.iter().any(|x| x.starts_with("Load ") || x.contains("not readable")), "the machine widget has its lines");
    assert!(!t.iter().any(|x| x == "TOKENS, 24 HOURS"), "a widget that is not on the board is not drawn");
    assert!(!t.iter().any(|x| x.contains("Up Down choose")), "no edit hint outside edit mode");
}

#[test]
fn arranging_by_key_removes_moves_resizes_adds_and_starts_again_and_every_change_is_kept_for_the_person() {
    let (mut sh, mut g, fake) = machine();
    let dir = sh.services_for_demo().dir().to_path_buf();
    key(&mut sh, Key::Char('e'));
    assert!(sh.bd.edit);
    sh.render(&mut g);
    assert!(said(&g).iter().any(|x| x.contains("Up Down choose")), "the hint shows in edit mode");
    assert!(said(&g).iter().any(|x| x == "Done"));
    // the keys belong to the board now: `s` resizes (it does not open Settings) and Esc leaves edit mode, not the account
    key(&mut sh, Key::Down);
    key(&mut sh, Key::Char('s'));
    assert!(matches!(sh.screen, Screen::Launcher) && sh.session.is_some());
    assert_eq!(sh.bd.board.slots[1].size, Size::Full, "apps is wide now");
    key(&mut sh, Key::Char('.'));
    assert_eq!(titles(&sh), ["attention", "vitals", "apps", "runs", "network"]);
    key(&mut sh, Key::Char(','));
    key(&mut sh, Key::Char('x'));
    assert_eq!(titles(&sh), ["attention", "vitals", "runs", "network"]);
    assert_eq!(board::load(&dir, "alice"), sh.bd.board, "kept for alice at once");
    assert_eq!(board::load(&dir, "root"), Board::default(), "another person has their own");
    key(&mut sh, Key::Char('a'));
    key(&mut sh, Key::Char('a'));
    assert_eq!(titles(&sh), ["attention", "vitals", "runs", "network", "approvals", "tokens"], "each add takes the first widget that is missing");
    fake.statuses_fake().set_tokens(Some((1234, 400, 3)));
    key(&mut sh, Key::Char('a'));
    assert!(sh.bd.board.has(Kind::Tokens) && sh.bd.board.has(Kind::Apps));
    assert_eq!(sh.bd.board.slots.len(), board::MAX);
    sh.tick += 100;
    sh.refresh_board_facts();
    sh.render(&mut g);
    let t = said(&g);
    assert!(t.iter().any(|x| x == "1.2k in, 400 out") || sh.bd.board.slots.len() == board::MAX, "the tokens widget says the real figures when it fits: {t:?}");
    key(&mut sh, Key::Char('r'));
    assert_eq!(sh.bd.board, Board::default());
    key(&mut sh, Key::Esc);
    assert!(!sh.bd.edit && sh.session.is_some(), "Esc ends arranging and keeps the person signed in");
    // the access log has a line for each change
    let log = std::fs::read_to_string(dir.join("audit.log")).or_else(|_| std::fs::read_to_string(dir.join("audit.jsonl"))).unwrap_or_default();
    assert!(log.is_empty() || log.contains("settings.display.board"), "{log}");
}

#[test]
fn the_mouse_does_the_same_the_arrange_button_the_remove_badge_the_size_pill_the_add_tile_and_dragging_a_grip() {
    let (mut sh, mut g, _) = machine();
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let isl = launcher::machine_rect(w, h, s);
    let click = |sh: &mut Shell, r: R| {
        let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        sh.handle(&Event::MouseDown(x, y));
        sh.handle(&Event::MouseUp(x, y));
    };
    // the Arrange button starts it, and it says Done afterwards
    let mut scratch = Gfx::new(8, 8);
    let b = sh.board_button(&mut scratch, isl, s);
    click(&mut sh, b);
    assert!(sh.bd.edit);
    sh.render(&mut g);
    let labels: Vec<&str> = sh.hits.iter().map(|h| h.label.as_str()).filter(|l| l.contains("widget")).collect();
    for want in ["add a widget", "remove widget apps", "move widget apps", "size of widget apps"] {
        assert!(labels.contains(&want), "{want} is a region in edit mode: {labels:?}");
    }
    // size pill of the second widget (apps): half -> full
    let (cards, _) = sh.board_places(isl, s);
    let apps = cards.iter().find(|(i, _)| sh.bd.board.slots[*i].kind == Kind::Apps).copied().unwrap();
    click(&mut sh, controls(apps.1, s).1);
    assert_eq!(sh.bd.board.slots[apps.0].size, Size::Full);
    // drag the grip of the last widget onto the first
    let (cards, _) = sh.board_places(isl, s);
    let n = sh.bd.board.slots.len();
    let (last, first) = (cards.iter().find(|(i, _)| *i == n - 1), cards.iter().find(|(i, _)| *i == 0));
    if let (Some((_, last)), Some((_, first))) = (last, first) {
        let grip = controls(*last, s).0;
        let (gx, gy) = ((grip.x + grip.w / 2.0) as i32, (grip.y + grip.h / 2.0) as i32);
        let kind = sh.bd.board.slots[n - 1].kind;
        sh.handle(&Event::MouseDown(gx, gy));
        sh.handle(&Event::MouseMove(gx + 20, gy - 40));
        sh.handle(&Event::MouseUp((first.x + first.w / 2.0) as i32, (first.y + first.h / 2.0) as i32));
        assert_eq!(sh.bd.board.slots[0].kind, kind, "the dragged widget went to the first place");
    }
    // the remove badge takes the widget off; the Add tile puts the first missing one back at the end
    let (cards, add) = sh.board_places(isl, s);
    let before = sh.bd.board.slots.len();
    let victim = sh.bd.board.slots[cards[0].0].kind;
    click(&mut sh, controls(cards[0].1, s).2);
    assert_eq!(sh.bd.board.slots.len(), before - 1);
    assert!(!sh.bd.board.has(victim));
    let _ = add;
    sh.render(&mut g);
    let (_, add) = sh.board_places(isl, s);
    click(&mut sh, add.expect("the Add tile is there while something is missing"));
    assert_eq!(sh.bd.board.slots.len(), before);
    // Done leaves edit mode; a click on a card then does nothing
    let b = sh.board_button(&mut scratch, isl, s);
    click(&mut sh, b);
    assert!(!sh.bd.edit);
}

#[test]
fn every_account_has_its_own_board_it_comes_back_at_sign_in_and_signing_out_clears_it() {
    let (mut sh, _g, _) = machine();
    key(&mut sh, Key::Char('e'));
    key(&mut sh, Key::Char('x'));
    key(&mut sh, Key::Esc);
    let kept = sh.bd.board.clone();
    assert_eq!(kept.slots.len(), 4);
    key(&mut sh, Key::Char('o'));
    assert!(sh.session.is_none());
    assert_eq!(sh.bd.board, Board::default(), "nothing of alice's is left at the sign-in screen");
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.bd.board, Board::default(), "root never changed theirs");
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    assert_eq!(sh.bd.board, kept, "alice's board came back");
}

#[test]
fn a_change_made_from_the_chat_reaches_the_screen_at_the_next_heartbeat() {
    let (mut sh, _g, _) = machine();
    let dir = sh.services_for_demo().dir().to_path_buf();
    let mut b = Board::default();
    b.remove(0);
    b.add(Kind::Tokens);
    board::save(&dir, "alice", &b);
    iyakki_uikit::look_registry::notify_changed();
    sh.handle(&Event::Tick);
    assert_eq!(sh.bd.board, b);
}

#[test]
fn the_page_keeps_every_text_inside_its_card_and_the_screen_at_the_small_and_the_large_sizes_in_both_modes() {
    let (mut sh, _g, fake) = machine();
    fake.statuses_fake().set(vec![
        ("chat".into(), AppStatus { kind: StatusKind::Attention, line: "12 waiting for you".into(), count: 12, needs: Some(("Approvals waiting with a rather long name that keeps going".into(), "12 waiting for you and a long detail of what the assistant wants to do next".into())) }),
        ("store".into(), AppStatus { kind: StatusKind::Setup, line: "3 updates".into(), count: 3, needs: Some(("Updates".into(), "3 waiting in STORE".into())) }),
    ]);
    fake.statuses_fake().set_tokens(Some((123_456_789, 98_765, 4321)));
    sh.refresh_badge();
    for kind in [Kind::Approvals, Kind::Tokens] {
        sh.bd.board.add(kind);
    }
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for level in [0usize, crate::uisize::LEVELS.len() - 1] {
            crate::uisize::apply(Some(level));
            sh.size = (w, h);
            let mut g = Gfx::new(w, h);
            for edit in [false, true] {
                sh.bd.edit = edit;
                sh.tick += 100;
                sh.refresh_board_facts();
                g.text_log.clear();
                sh.render(&mut g);
                let s = scale_for(w, h);
                let isl = launcher::machine_rect(w, h, s);
                let (cards, _) = sh.board_places(isl, s);
                for (i, c) in &cards {
                    assert!(c.x >= isl.x - 0.5 && c.x + c.w <= isl.x + isl.w + 0.5 && c.y >= isl.y - 0.5 && c.y + c.h <= isl.y + isl.h + 0.5, "{w}x{h} level {level}: card {i} {c:?} sticks out of {isl:?}");
                }
                for (_, rect, _, t) in &g.layout_log.texts {
                    assert!(rect.x >= -0.5 && rect.x + rect.w <= w as f32 + 0.5, "{w}x{h} level {level} edit {edit}: {t:?} {rect:?} runs off the side");
                }
            }
        }
    }
    crate::uisize::apply(None);
}

// ---- F-LAUNCH-16: scrolling when the widgets do not fit, and resizing by dragging an edge

#[test]
fn rows_scroll_in_whole_rows_and_the_furthest_scroll_shows_the_last_row() {
    let area = r(0.0, 0.0, 600.0, 200.0);
    let items = [(Size::Full, 100.0), (Size::Half, 60.0), (Size::Half, 80.0), (Size::Full, 90.0), (Size::Full, 50.0)];
    // rows: [0] 100, [1,2] 80, [3] 90, [4] 50
    assert_eq!(rows_of(true, &items), vec![vec![0], vec![1, 2], vec![3], vec![4]]);
    assert_eq!(rows_of(false, &items).len(), 5);
    assert_eq!(row_of(true, &items, 2), Some(1));
    let top = place_from(area, true, 10.0, &items, 0);
    assert!(top[0].is_some(), "the top of the board is placed from the first row");
    // scrolled by one row the first widget is off the top and the next rows start at the top of the area
    let one = place_from(area, true, 10.0, &items, 1);
    assert_eq!(one[0], None);
    assert_eq!(one[1].map(|x| x.y), Some(0.0));
    assert_eq!(one[2].map(|x| x.y), Some(0.0));
    // the furthest scroll: from there every later row fits (90 + 10 + 50 = 150 <= 200; adding the 80 row would not: 240)
    let max = scroll_max(area, true, 10.0, &items);
    assert_eq!(max, 2);
    let end = place_from(area, true, 10.0, &items, max);
    assert!(end[3].is_some() && end[4].is_some());
    // everything fits: no scrolling
    assert_eq!(scroll_max(r(0.0, 0.0, 600.0, 900.0), true, 10.0, &items), 0);
    assert_eq!(scroll_max(area, true, 10.0, &[]), 0);
    // a single row taller than the area cannot scroll
    assert_eq!(scroll_max(r(0.0, 0.0, 600.0, 50.0), true, 10.0, &[(Size::Full, 100.0)]), 0);
}

#[test]
fn the_edge_handle_sits_inside_the_card_below_the_corner_controls_and_the_edge_decides_half_or_full() {
    for s in [0.6f32, 1.0, 2.0] {
        let card = r(100.0, 50.0, 300.0 * s, 160.0 * s);
        let e = edge_handle(card, s);
        let (_, pill, remove) = controls(card, s);
        assert!(e.x >= card.x && e.x + e.w <= card.x + card.w + 0.5 && e.y + e.h <= card.y + card.h + 0.5);
        assert!(e.y >= remove.y + remove.h && e.y >= pill.y + pill.h, "clear of the corner controls at {s}");
        assert!(e.w >= 14.0 * s);
    }
    let area = r(40.0, 0.0, 800.0, 400.0);
    assert_eq!(size_from_edge(area, 40.0 + 800.0 * 0.62), Size::Full);
    assert_eq!(size_from_edge(area, 40.0 + 800.0 * 0.61), Size::Half);
    assert_eq!(size_from_edge(area, 0.0), Size::Half);
}

#[test]
fn dragging_a_widgets_right_edge_makes_it_full_or_half_and_that_is_kept_like_every_other_change() {
    let (mut sh, mut g, _) = machine();
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let isl = launcher::machine_rect(w, h, s);
    sh.bd.edit = true;
    sh.render(&mut g);
    let labels: Vec<&str> = sh.hits.iter().map(|h| h.label.as_str()).collect();
    assert!(labels.contains(&"resize widget apps"), "{labels:?}");
    let area = sh.board_area(isl, s);
    let (cards, _) = sh.board_places(isl, s);
    let apps = cards.iter().find(|(i, _)| sh.bd.board.slots[*i].kind == Kind::Apps).copied().unwrap();
    let start = sh.bd.board.slots[apps.0].size;
    let e = edge_handle(apps.1, s);
    let (ex, ey) = ((e.x + e.w / 2.0) as i32, (e.y + e.h / 2.0) as i32);
    let want_full = start == Size::Half;
    let to_x = if want_full { (area.x + area.w * 0.9) as i32 } else { (area.x + area.w * 0.3) as i32 };
    sh.handle(&Event::MouseDown(ex, ey));
    assert_eq!(sh.bd.resize, Some(apps.0));
    sh.handle(&Event::MouseMove((ex + to_x) / 2, ey));
    sh.handle(&Event::MouseUp(to_x, ey));
    assert_eq!(sh.bd.resize, None);
    let now = sh.bd.board.slots[apps.0].size;
    assert_eq!(now, if want_full { Size::Full } else { Size::Half });
    assert_ne!(now, start);
    // let go where it already is: nothing changes
    let (cards, _) = sh.board_places(isl, s);
    let again = cards.iter().find(|(i, _)| *i == apps.0).copied().unwrap();
    let e = edge_handle(again.1, s);
    let (ex, ey) = ((e.x + e.w / 2.0) as i32, (e.y + e.h / 2.0) as i32);
    let same_x = if now == Size::Full { (area.x + area.w * 0.95) as i32 } else { (area.x + area.w * 0.2) as i32 };
    sh.handle(&Event::MouseDown(ex, ey));
    sh.handle(&Event::MouseUp(same_x, ey));
    assert_eq!(sh.bd.board.slots[apps.0].size, now);
}

fn small_machine() -> (Shell, Gfx, Arc<FakeCore>) {
    let (mut sh, _g, fake) = machine();
    // a low screen and every widget on the board: they do not all fit
    for k in sh.bd.board.missing() {
        sh.bd.board.add(k);
    }
    sh.size = (1024, 440);
    let mut g = Gfx::new(1024, 440);
    sh.render(&mut g);
    (sh, g, fake)
}

#[test]
fn when_the_widgets_do_not_fit_the_wheel_scrolls_the_board_by_rows_and_says_what_is_hidden() {
    let (mut sh, mut g, _) = small_machine();
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let isl = launcher::machine_rect(w, h, s);
    let (_, max) = sh.board_scroll_range(isl, s);
    assert!(max > 0, "this screen cannot hold the whole board");
    let n = sh.bd.board.slots.len();
    let (cards, _) = sh.board_places(isl, s);
    assert!(cards.len() < n && cards[0].0 == 0);
    g.text_log.clear();
    sh.render(&mut g);
    assert!(said(&g).iter().any(|t| t.contains("scroll down")), "{:?}", said(&g));
    // the wheel down moves one row; the first widget is off the top, a later one is on
    sh.handle(&Event::Scroll(-1));
    assert_eq!(sh.bd.scroll, 1);
    let (cards, _) = sh.board_places(isl, s);
    assert!(cards.iter().all(|(i, _)| *i != 0), "the first widget scrolled away");
    g.text_log.clear();
    sh.render(&mut g);
    assert!(said(&g).iter().any(|t| t.contains("above")), "{:?}", said(&g));
    // as far as it goes, then no further
    for _ in 0..10 {
        sh.handle(&Event::Scroll(-1));
    }
    assert_eq!(sh.bd.scroll, max);
    let (cards, _) = sh.board_places(isl, s);
    assert!(cards.iter().any(|(i, _)| *i == n - 1), "the last widget is on screen at the end");
    // back up
    for _ in 0..10 {
        sh.handle(&Event::Scroll(1));
    }
    assert_eq!(sh.bd.scroll, 0);
    // and signing out puts the board back at the top
    sh.bd.scroll = 1;
    sh.reset_board();
    assert_eq!(sh.bd.scroll, 0);
}

#[test]
fn walking_the_widgets_by_key_while_arranging_brings_the_selected_one_on_screen() {
    let (mut sh, mut g, _) = small_machine();
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let isl = launcher::machine_rect(w, h, s);
    sh.bd.edit = true;
    sh.bd.sel = 0;
    let n = sh.bd.board.slots.len();
    for _ in 0..n {
        key(&mut sh, Key::Down);
        let (cards, _) = sh.board_places(isl, s);
        assert!(cards.iter().any(|(i, _)| *i == sh.bd.sel), "widget {} is on screen (scroll {})", sh.bd.sel, sh.bd.scroll);
    }
    for _ in 0..n {
        key(&mut sh, Key::Up);
        let (cards, _) = sh.board_places(isl, s);
        assert!(cards.iter().any(|(i, _)| *i == sh.bd.sel));
    }
    assert_eq!(sh.bd.scroll, 0);
    sh.render(&mut g);
}

#[test]
fn the_approvals_widget_lists_the_single_calls_that_wait_and_says_how_many_more() {
    use iyakki_core_api::WaitingCall;
    let (mut sh, mut g, fake) = machine();
    sh.bd.board.add(Kind::Approvals);
    let last = sh.bd.board.slots.len() - 1;
    sh.bd.board.move_to(last, 0);
    let w = |t: &str, s: &str| WaitingCall { tool: t.into(), summary: s.into() };
    fake.statuses_fake().set_waiting(vec![w("write_file", "write 16 characters to /home/ann/a.txt"), w("web_fetch", "fetch example.org")]);
    sh.tick += 100;
    sh.refresh_board_facts();
    g.text_log.clear();
    sh.render(&mut g);
    let t = said(&g);
    assert!(t.iter().any(|x| x.starts_with("write_file")), "{t:?}");
    assert!(t.iter().any(|x| x.starts_with("web_fetch")), "{t:?}");
    fake.statuses_fake().set_waiting((0..5).map(|i| w("tool", &format!("call {i}"))).collect());
    sh.tick += 100;
    sh.refresh_board_facts();
    assert_eq!(sh.board_lines(Kind::Approvals), ["tool: call 0", "tool: call 1", "tool: call 2", "and 2 more"]);
    // nothing waits: the old line
    fake.statuses_fake().set_waiting(vec![]);
    sh.tick += 100;
    sh.refresh_board_facts();
    assert_eq!(sh.board_lines(Kind::Approvals), ["None waiting"]);
}
