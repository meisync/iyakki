use crate::audioview::*;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::media::*;
use crate::size::Metrics;

fn port() -> MemMedia {
    let m = MemMedia::new();
    m.put_file("music/one.mp3", 60_000, false);
    m.put_file("music/two.ogg", 30_000, false);
    m.put_file("three.wav", 5_000, false);
    m.put_stream("https://radio.example.org/live", None);
    m.put_stream("https://example.org/talk.mp3", Some(120_000));
    m
}

fn view() -> (AudioView, MemMedia) {
    let m = port();
    let mut v = AudioView::new(Box::new(m.clone()), 900);
    v.tick();
    (v, m)
}

fn draw(v: &mut AudioView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn press(v: &mut AudioView, l: &DisplayList, name: &str) {
    let a = action_named(l, name);
    assert!(v.owns(a), "{name}");
    v.action(a);
}

#[test]
fn the_first_tick_lists_the_files_and_a_tap_on_one_plays_it() {
    let (mut v, m) = view();
    assert_eq!(v.names(), ["music/one.mp3", "music/two.ogg", "three.wav"]);
    assert_eq!(v.state(), PlayState::Idle);
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    press(&mut v, &l, "music/two.ogg");
    assert_eq!(m.calls()[0], "open home music/two.ogg");
    assert_eq!(v.state(), PlayState::Playing);
    assert_eq!(v.title(), Some("two.ogg"));
    assert_eq!(v.duration_ms(), Some(30_000));
    // a second tick does not list again (the selection stays)
    v.tick();
    assert_eq!(v.selected, 1);
}

#[test]
fn position_and_time_follow_the_core_and_the_bar_fills() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    m.advance(15_000);
    v.tick();
    assert_eq!(v.position_ms(), 15_000);
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    let t = texts(&l);
    assert!(t.contains(&"0:15".to_string()) && t.contains(&"1:00".to_string()), "{t:?}");
    assert!(t.contains(&"playing".to_string()) && t.contains(&"one".to_string().replace("one", "one.mp3")), "{t:?}");
    // the filled part of the bar is a quarter of the track
    let fills: Vec<f32> = l.ops.iter().filter_map(|o| if let Op::RoundRect(r, _, _) = o { (r.h == 8.0).then_some(r.w) } else { None }).collect();
    assert!(fills.len() >= 2 && (fills[1] / fills[0] - 0.25).abs() < 0.03, "{fills:?}");
    // it runs to the end: the next file starts by itself
    m.advance(60_000);
    v.tick();
    assert_eq!(v.title(), Some("two.ogg"));
    assert!(m.calls().contains(&"open home music/two.ogg".to_string()));
}

#[test]
fn pause_play_and_the_space_key_are_the_same_button() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    press(&mut v, &l, "Pause");
    assert_eq!(v.state(), PlayState::Paused);
    m.advance(5000);
    assert_eq!(m.position(), 0, "paused: the clock stands still");
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    assert!(texts(&l).contains(&"paused".to_string()));
    v.key(UiKey::Char(' '));
    assert_eq!(v.state(), PlayState::Playing);
    // after the end Play starts over
    m.advance(90_000);
    v.set_auto_next(false);
    v.tick();
    assert_eq!(v.state(), PlayState::Ended);
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    press(&mut v, &l, "Play");
    assert_eq!((v.state(), m.position()), (PlayState::Playing, 0));
}

#[test]
fn a_tap_on_the_bar_seeks_and_the_keys_and_buttons_move_by_seconds() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    press(&mut v, &l, "Seek to 50%");
    assert_eq!(m.position(), 30_000);
    assert_eq!(v.position_ms(), 30_000);
    v.key(UiKey::Left);
    assert_eq!(m.position(), 25_000);
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    press(&mut v, &l, "+10s");
    assert_eq!(m.position(), 35_000);
    press(&mut v, &l, "-10s");
    v.action(action_named(&l, "-10s"));
    v.action(action_named(&l, "-10s"));
    v.action(action_named(&l, "-10s"));
    assert_eq!(m.position(), 0, "never before the start");
    v.seek_by(1_000_000);
    assert_eq!(m.position(), 60_000, "never past the end");
}

#[test]
fn volume_mute_and_speed_are_asked_of_the_core_and_kept_for_the_next_file() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    press(&mut v, &l, "Vol-");
    press(&mut v, &l, "Vol-");
    assert_eq!(m.volume(), (80, false));
    assert!(texts(&draw(&mut v, 500.0, 700.0, false, 1.0)).contains(&"Volume 80%".to_string()));
    press(&mut v, &l, "Mute");
    assert_eq!(m.volume(), (80, true));
    assert!(texts(&draw(&mut v, 500.0, 700.0, false, 1.0)).contains(&"Muted".to_string()));
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    press(&mut v, &l, "1x");
    assert_eq!(m.rate(), 1250);
    v.key(UiKey::Char('n'));
    assert_eq!(v.title(), Some("two.ogg"));
    assert_eq!((m.volume(), m.rate()), ((80, true), 1250), "the person's choices go to the next file");
    v.key(UiKey::Char('+'));
    assert_eq!(m.volume(), (90, false), "a louder volume ends the mute");
    for _ in 0..20 {
        v.key(UiKey::Char('+'));
    }
    assert_eq!(m.volume().0, 100);
}

#[test]
fn a_remote_source_shows_the_card_and_nothing_plays_until_allow() {
    let (mut v, m) = view();
    v.open(Source::Stream("https://example.org/talk.mp3".into()), "A talk");
    assert_eq!(v.state(), PlayState::AwaitingApproval);
    assert_eq!(v.approval(), Some("Play this from the internet"));
    m.advance(5000);
    assert_eq!(m.position(), 0);
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t.contains("from the internet")));
    press(&mut v, &l, "Allow");
    assert_eq!((v.state(), v.approval()), (PlayState::Playing, None));
    assert_eq!(v.title(), Some("A talk"));
    // a no is told in a sentence
    v.open(Source::Station("Tone FM".into()), "");
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    press(&mut v, &l, "Deny");
    assert_eq!(v.state(), PlayState::Failed);
    assert_eq!(v.error(), Some("You did not allow that source."));
    assert!(texts(&draw(&mut v, 400.0, 700.0, false, 1.0)).iter().any(|t| t.contains("did not allow")));
}

#[test]
fn a_live_stream_shows_live_has_no_seek_bar_and_says_so_when_moved() {
    let (mut v, _m) = view();
    v.open(Source::Stream("https://radio.example.org/live".into()), "Radio");
    v.action(900 + 7); // Allow
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    assert!(texts(&l).contains(&"LIVE".to_string()), "{:?}", texts(&l));
    assert!(!l.a11y().iter().any(|n| n.name.starts_with("Seek to")));
    v.key(UiKey::Right);
    assert_eq!(v.error(), Some("A live stream cannot be moved."));
}

#[test]
fn a_refusal_and_a_core_without_a_player_are_told_in_a_sentence() {
    let m = MemMedia::unavailable();
    let mut v = AudioView::new(Box::new(m), 900);
    v.tick();
    assert_eq!(v.error(), Some("The media player is not available here."));
    let l = draw(&mut v, 400.0, 600.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t == "The media player is not available here."));
    let (mut v, m) = view();
    m.refuse_next("At most 4 apps may play at the same time.");
    v.key(UiKey::Enter);
    assert_eq!((v.state(), v.error()), (PlayState::Failed, Some("At most 4 apps may play at the same time.")));
    // Play on a failed player tries the chosen file again
    v.key(UiKey::Char(' '));
    assert_eq!(v.state(), PlayState::Playing);
    assert_eq!(v.error(), None);
}

#[test]
fn the_level_meter_follows_the_sound_and_falls_back() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    m.set_level(800);
    m.advance(250);
    v.tick();
    let bar = |l: &DisplayList| l.ops.iter().filter_map(|o| if let Op::RoundRect(r, _, _) = o { (r.h == 4.0).then_some(r.w) } else { None }).collect::<Vec<f32>>();
    let loud = bar(&draw(&mut v, 500.0, 700.0, false, 1.0));
    assert_eq!(loud.len(), 2);
    assert!((loud[1] / loud[0] - 0.8).abs() < 0.12, "{loud:?}");
    m.set_level(0);
    m.advance(250);
    for _ in 0..40 {
        v.tick();
    }
    let quiet = bar(&draw(&mut v, 500.0, 700.0, false, 1.0));
    assert_eq!(quiet.len(), 1, "no bar when there is no level: {quiet:?}");
}

#[test]
fn escape_stops_and_closes_the_session() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    assert!(m.is_open());
    assert!(v.key(UiKey::Esc));
    assert!(!m.is_open());
    assert_eq!(v.state(), PlayState::Idle);
    assert!(!v.key(UiKey::Esc), "nothing to stop: the key is the app's");
    let l = draw(&mut v, 400.0, 700.0, false, 1.0);
    assert!(texts(&l).contains(&"No sound chosen".to_string()));
}

#[test]
fn tracks_are_listed_when_there_is_a_choice_and_chosen_by_tap() {
    let (mut v, m) = view();
    v.key(UiKey::Enter);
    // one audio track: no list
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    assert!(!l.a11y().iter().any(|n| n.name.starts_with("Audio:")));
    // a second audio track and a caption track come with a file
    m.add_tracks_for_test();
    v.tick();
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    press(&mut v, &l, "Audio: Commentary (en)");
    assert!(m.calls().contains(&"track 1".to_string()));
    assert!(v.tracks().iter().any(|t| t.id == 1 && t.selected) && v.tracks().iter().any(|t| t.id == 0 && !t.selected));
}

#[test]
fn every_control_has_a_name_stays_inside_the_pane_and_is_big_enough_at_three_size_classes_and_three_times_the_text() {
    for (w, h, touch) in [(360.0f32, 740.0f32, true), (800.0, 480.0, false), (1280.0, 720.0, false)] {
        for scale in [1.0f32, 3.0] {
            let (mut v, m) = view();
            m.add_tracks_for_test();
            v.key(UiKey::Down);
            v.key(UiKey::Enter);
            m.add_tracks_for_test();
            v.tick();
            let mt = Metrics::new(w, h, 1.0, scale, touch);
            let l = draw(&mut v, w, h, touch, scale);
            let mut seen = 0;
            for o in &l.ops {
                if let Op::Hit(r, hit) = o {
                    seen += 1;
                    assert!(!hit.name.is_empty());
                    assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h} x{scale}: {r:?}", hit.name);
                    assert!(r.h >= mt.min_target() - 0.5, "{} too low at {w}x{h} x{scale}: {}", hit.name, r.h);
                    assert!(v.owns(hit.action), "{}", hit.name);
                }
                if let Op::Text { y, size, text, .. } = o {
                    assert!(*y >= -0.5 && *y + *size * 0.8 <= h + 0.5, "text {text:?} outside at {w}x{h} x{scale}: y={y}");
                }
            }
            assert!(seen >= 8, "at {w}x{h} x{scale} only {seen} controls");
            let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
            for must in ["Pause", "+10s", "-10s", "Mute"] {
                assert!(names.iter().any(|n| n == must), "{must} missing at {w}x{h} x{scale}: {names:?}");
            }
        }
    }
}

#[test]
fn the_compact_and_the_expanded_layouts_differ_and_the_wide_one_has_the_list_beside_the_player() {
    let (mut v, _m) = view();
    let c = draw(&mut v, 360.0, 740.0, true, 1.0);
    let e = draw(&mut v, 1280.0, 720.0, false, 1.0);
    assert_ne!(c, e);
    let row_x = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == "three.wav").then_some((r.x, r.y)) } else { None }).unwrap();
    let play_x = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == "Play").then_some((r.x, r.y)) } else { None }).unwrap();
    assert!(row_x(&e).0 < play_x(&e).0, "wide: the list is to the left of the buttons");
    assert!(row_x(&c).1 > play_x(&c).1, "compact: the list is under the buttons");
}

#[test]
fn the_skin_changes_the_corners_and_the_gaps_of_the_player() {
    use crate::look::Shape;
    use crate::tokens::Tokens;
    let radii = |shape: Shape| {
        let mut look = Look::default();
        look.surface.shape = shape;
        let (mut v, _m) = view();
        let m = Metrics::new(500.0, 700.0, 1.0, 1.0, false).with_tokens(Tokens::of(&look));
        let mut c = ListCanvas::new(500.0, 700.0);
        v.draw(&mut c, &look.theme, &m, Rect::new(0.0, 0.0, 500.0, 700.0));
        c.finish().ops.iter().filter_map(|o| if let Op::RoundRect(_, r, _) = o { Some(*r) } else { None }).collect::<Vec<f32>>()
    };
    let (soft, sharp, round) = (radii(Shape::Soft), radii(Shape::Sharp), radii(Shape::Round));
    assert_eq!(soft[0], 10.0, "the panel is a large corner of the plain skin");
    assert_eq!(sharp[0], 3.0);
    assert_eq!(round[0], 16.0);
    assert_ne!(soft, sharp);
}
