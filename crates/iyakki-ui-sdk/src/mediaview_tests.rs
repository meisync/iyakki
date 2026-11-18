use crate::audioview;
use crate::canvas::ImageFormat;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::media::*;
use crate::mediaview::*;
use crate::size::Metrics;

fn port() -> MemMedia {
    let m = MemMedia::new();
    m.put_video("clips/one.avi", 10_000, 10, 64, 48);
    m.put_file("song.mp3", 5_000, false);
    m
}

fn view() -> (MediaView, MemMedia) {
    let m = port();
    let mut v = MediaView::new(Box::new(m.clone()), 1200);
    v.tick();
    (v, m)
}

fn draw(v: &mut MediaView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn images(l: &DisplayList) -> Vec<(Rect, u64, u32, u32)> {
    l.ops.iter().filter_map(|o| if let Op::Image(r, i) = o { Some((*r, i.seq, i.width, i.height)) } else { None }).collect()
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn play_video(v: &mut MediaView) {
    v.audio_mut().play_index(0);
    v.tick();
}

#[test]
fn a_video_shows_the_newest_picture_letterboxed_in_the_surface_and_follows_the_clock() {
    let (mut v, m) = view();
    assert_eq!(v.audio().names(), ["clips/one.avi", "song.mp3"]);
    play_video(&mut v);
    assert!(v.audio().has_video());
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    let im = images(&l);
    assert_eq!(im.len(), 1);
    let (r, seq, w, h) = im[0];
    assert_eq!((seq, w, h), (1, 64, 48));
    assert!((r.w / r.h - 64.0 / 48.0).abs() < 0.01, "the picture keeps its shape: {r:?}");
    assert!(v.audio().picture().is_some_and(|p| p.format == ImageFormat::Rgba));
    // one second on: the picture of that time
    m.advance(1000);
    v.tick();
    assert_eq!(images(&draw(&mut v, 500.0, 700.0, false, 1.0))[0].1, 11);
    // a slow screen: the clock runs five seconds before the next look, the pictures between are never shown
    m.advance(5000);
    v.tick();
    assert_eq!(images(&draw(&mut v, 500.0, 700.0, false, 1.0))[0].1, 61);
    // the port was asked for what is new, not for everything
    assert!(m.calls().iter().filter(|c| *c == "frame").count() >= 3);
}

#[test]
fn a_sound_has_no_picture_surface_no_full_screen_and_the_audio_layout() {
    let (mut v, _m) = view();
    v.audio_mut().play_index(1);
    v.tick();
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    assert!(images(&l).is_empty() && !l.a11y().iter().any(|n| n.name == "Full screen" || n.name == "Picture"));
    assert!(!v.key(UiKey::Char('f')) || !v.fullscreen);
    assert!(!v.fullscreen);
    assert!(l.a11y().iter().any(|n| n.name == "Pause"));
}

#[test]
fn the_picture_waits_for_the_first_frame_and_a_tap_on_it_plays_or_pauses() {
    let (mut v, _m) = view();
    v.audio_mut().play_index(0);
    // opened, no tick yet: no picture
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    assert!(images(&l).is_empty() && texts(&l).iter().any(|t| t == "Waiting for the first picture"), "{:?}", texts(&l));
    v.tick();
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    assert_eq!(v.audio().state(), PlayState::Playing);
    let a = action_named(&l, "Picture");
    assert!(v.owns(a) && v.action(a));
    assert_eq!(v.audio().state(), PlayState::Paused);
    let l = draw(&mut v, 500.0, 700.0, false, 1.0);
    v.action(action_named(&l, "Picture"));
    assert_eq!(v.audio().state(), PlayState::Playing);
}

#[test]
fn full_screen_shows_only_the_picture_and_a_bar_and_escape_leaves_it_before_it_stops() {
    let (mut v, _m) = view();
    play_video(&mut v);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Full screen"));
    assert!(v.fullscreen);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
    assert!(names.contains(&"Exit full screen".to_string()) && names.contains(&"Pause".to_string()) && !names.contains(&"song.mp3".to_string()), "{names:?}");
    let im = images(&l);
    assert_eq!(im.len(), 1);
    assert!(im[0].0.h > 480.0 * 0.6, "the picture takes most of the pane: {:?}", im[0].0);
    // Pause in the bar is the player's own button
    v.action(action_named(&l, "Pause"));
    assert_eq!(v.audio().state(), PlayState::Paused);
    // Esc leaves full screen first; the player keeps its state
    assert!(v.key(UiKey::Esc));
    assert!(!v.fullscreen && v.audio().state() == PlayState::Paused);
    // f toggles; Esc then stops
    v.key(UiKey::Char('f'));
    assert!(v.fullscreen);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Exit full screen"));
    assert!(!v.fullscreen);
    assert!(v.key(UiKey::Esc));
    assert_eq!(v.audio().state(), PlayState::Idle);
}

#[test]
fn every_control_has_a_name_stays_inside_the_pane_and_is_big_enough_at_three_size_classes_and_three_times_the_text() {
    for (w, h, touch) in [(360.0f32, 740.0f32, true), (800.0, 480.0, false), (1280.0, 720.0, false)] {
        for scale in [1.0f32, 3.0] {
            for full in [false, true] {
                let (mut v, _m) = view();
                play_video(&mut v);
                v.fullscreen = full;
                let mt = Metrics::new(w, h, 1.0, scale, touch);
                let l = draw(&mut v, w, h, touch, scale);
                let mut seen = 0;
                for o in &l.ops {
                    if let Op::Hit(r, hit) = o {
                        seen += 1;
                        assert!(!hit.name.is_empty());
                        assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h} x{scale} full={full}: {r:?}", hit.name);
                        assert!(r.h >= mt.min_target() - 0.5, "{} too low at {w}x{h} x{scale} full={full}: {}", hit.name, r.h);
                        assert!(v.owns(hit.action), "{}", hit.name);
                    }
                    if let Op::Image(r, _) = o {
                        assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "picture outside at {w}x{h} x{scale}: {r:?}");
                    }
                }
                assert!(seen >= 3, "{seen} controls at {w}x{h} x{scale} full={full}");
                assert_eq!(images(&l).len(), 1, "{w}x{h} x{scale} full={full}");
            }
        }
    }
}

#[test]
fn the_skin_changes_the_corners_of_the_surface() {
    use crate::look::Shape;
    use crate::tokens::Tokens;
    let radii = |shape: Shape| {
        let mut look = Look::default();
        look.surface.shape = shape;
        let (mut v, _m) = view();
        play_video(&mut v);
        let m = Metrics::new(500.0, 700.0, 1.0, 1.0, false).with_tokens(Tokens::of(&look));
        let mut c = ListCanvas::new(500.0, 700.0);
        v.draw(&mut c, &look.theme, &m, Rect::new(0.0, 0.0, 500.0, 700.0));
        c.finish().ops.iter().filter_map(|o| if let Op::RoundRect(_, r, _) = o { Some(*r) } else { None }).collect::<Vec<f32>>()
    };
    assert_ne!(radii(Shape::Soft), radii(Shape::Sharp));
    assert!(audioview::ACTIONS < ACTIONS);
}
