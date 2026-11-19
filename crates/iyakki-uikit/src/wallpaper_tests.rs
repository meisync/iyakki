use super::*;
use crate::gfx::Gfx;

/// Every test leaves the wallpaper as it found it: the choice is thread-local and the tests share a thread.
struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        set_current(Id::Theme);
    }
}

fn drawn(id: Id) -> Gfx {
    set_current(id);
    let mut g = Gfx::new(96, 54);
    g.clear(rgb(0, 0, 0));
    draw(&mut g);
    g
}

/// Average relative luminance of the middle of the picture (WCAG), for judging "is this wallpaper light?".
/// The top and bottom bands are chrome ground for the bar and the footer hints, not the wallpaper, so they are left out.
fn luminance(g: &Gfx) -> f64 {
    let f = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (w, h) = (g.width() as usize, g.height() as usize);
    let (from, to) = (h * 20 / 100, h * 80 / 100);
    let px = g.rgba();
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in from..to {
        for x in 0..w {
            let i = (y * w + x) * 4;
            sum += 0.2126 * f(px[i]) + 0.7152 * f(px[i + 1]) + 0.0722 * f(px[i + 2]);
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn keys_survive_a_round_trip_and_anything_unknown_falls_back_to_the_theme() {
    for id in ALL {
        assert_eq!(Id::from_key(id.key()), id, "{}", id.name());
    }
    // a file written by a newer release must never leave a person with no background
    assert_eq!(Id::from_key("aurora-from-the-future"), Id::Theme);
    assert_eq!(Id::from_key(""), Id::Theme);
    assert_eq!(Id::default(), Id::Theme);
}

#[test]
fn the_choice_is_kept_per_person_beside_the_theme_and_the_ui_size() {
    let dir = std::env::temp_dir().join(format!("iyakki-wallpaper-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    assert_eq!(load(&dir, "amy"), Id::Theme, "nothing saved means the theme's own background");

    // a value already in the file must survive: the wallpaper is written beside the others, not over them
    let p = dir.join("ui-amy.json");
    std::fs::write(&p, r#"{"theme":"light","size":"Large"}"#).unwrap();
    save(&dir, "amy", Id::Daylight);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["wallpaper"], "daylight");
    assert_eq!(v["theme"], "light", "the theme is left alone");
    assert_eq!(v["size"], "Large", "the UI size is left alone");

    assert_eq!(load(&dir, "amy"), Id::Daylight);
    assert_eq!(load(&dir, "bea"), Id::Theme, "one person's choice is not another's");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn choosing_another_wallpaper_bumps_the_generation_so_the_cache_rebuilds() {
    let _r = Reset;
    set_current(Id::Theme);
    let g0 = generation();
    set_current(Id::Dusk);
    assert_ne!(generation(), g0, "a change has to invalidate the background cache");
    let g1 = generation();
    set_current(Id::Dusk);
    assert_eq!(generation(), g1, "choosing the same one again rebuilds nothing");
}

#[test]
fn the_theme_wallpaper_draws_nothing_and_every_other_one_paints_the_whole_screen() {
    let _r = Reset;
    set_current(Id::Theme);
    let mut g = Gfx::new(32, 18);
    g.clear(rgb(0, 0, 0));
    assert!(!draw(&mut g), "Theme hands the drawing back to fx::background");
    assert!(g.rgba().chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0), "Theme left the picture untouched");

    for id in ALL.into_iter().filter(|w| *w != Id::Theme) {
        let g = drawn(id);
        assert!(g.rgba().chunks(4).all(|p| p[3] == 255), "{}: every pixel is painted", id.name());
        assert!(!g.rgba().chunks(4).all(|p| p[..3] == [0, 0, 0]), "{}: it painted something", id.name());
    }
}

#[test]
fn each_wallpaper_looks_different_from_the_others() {
    let _r = Reset;
    let pics: Vec<(Id, Vec<u8>)> = ALL.into_iter().filter(|w| *w != Id::Theme).map(|id| (id, drawn(id).rgba().to_vec())).collect();
    for (i, (a, pa)) in pics.iter().enumerate() {
        for (b, pb) in pics.iter().skip(i + 1) {
            assert_ne!(pa, pb, "{} and {} draw the same picture", a.name(), b.name());
        }
    }
}

#[test]
fn what_each_wallpaper_claims_about_its_lightness_is_what_it_draws() {
    let _r = Reset;
    // Settings tells a person whether a wallpaper is light, so the claim has to match the pixels, not the intention.
    for id in ALL.into_iter().filter(|w| *w != Id::Theme) {
        let l = luminance(&drawn(id));
        if id.is_light() {
            assert!(l > 0.55, "{} says it is light but measures {l:.2}", id.name());
        } else {
            assert!(l < 0.35, "{} says it is dark but measures {l:.2}", id.name());
        }
    }
}

#[test]
fn a_wallpaper_is_drawn_the_same_at_every_size_it_is_asked_for() {
    let _r = Reset;
    // the corner a glow starts from must not jump about between 720p and 4K: compare the same relative point
    set_current(Id::Daylight);
    let at = |w: u32, h: u32| {
        let mut g = Gfx::new(w, h);
        draw(&mut g);
        let (x, y) = ((w as f32 * 0.25) as u32, (h as f32 * 0.45) as u32);
        let i = ((y * w + x) * 4) as usize;
        let p = g.rgba();
        [p[i], p[i + 1], p[i + 2]]
    };
    let small = at(320, 180);
    let big = at(1280, 720);
    let near = small.iter().zip(big.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 24);
    assert!(near, "the same relative point differs between sizes: {small:?} vs {big:?}");
}

/// The ink the shell writes on the top bar and the footer reads over every wallpaper in every theme (4.5:1), measured on the drawn picture.
#[test]
fn the_top_bar_and_the_footer_ink_reads_over_every_wallpaper_in_every_theme() {
    use crate::fx::contrast;
    use crate::theme::ThemeId;
    let (w, h) = (1920u32, 1080u32);
    for theme in crate::theme::ALL {
        crate::theme::set_current(theme);
        let t = crate::theme::current();
        for id in ALL {
            crate::wallpaper::set_current(id);
            let mut g = Gfx::new(w, h);
            crate::fx::background(&mut g);
            let px = |x: u32, y: u32| {
                let i = ((y * w + x) * 4) as usize;
                [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2], 255]
            };
            // where the bar's text sits: the title and chips on the left, the clock and buttons on the right, and the footer line
            let bands: [(&str, u32, u32, [[u8; 4]; 2]); 3] = [("top bar", 14, 76, [t.text, t.muted]), ("chip row", 84, 122, [t.text, t.muted]), ("footer", h - 62, h - 24, [t.dim, t.dim])];
            for (name, y0, y1, inks) in bands {
                for y in (y0..y1).step_by(6) {
                    for x in (30..w - 30).step_by(97) {
                        for ink in inks {
                            let c = contrast(ink, px(x, y));
                            assert!(c >= 4.5, "{theme:?} on {id:?}: the {name} ink is {c:.2}:1 at ({x},{y})");
                        }
                    }
                }
            }
        }
    }
    crate::wallpaper::set_current(Id::Theme);
    crate::theme::set_current(ThemeId::Dark);
}

fn solid(w: u32, h: u32, c: [u8; 3]) -> (u32, u32, Vec<u8>) {
    (w, h, (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect())
}

#[test]
fn a_picture_wallpaper_covers_the_screen_keeps_its_colours_and_falls_back_to_the_theme_without_one() {
    let _r = Reset;
    set_picture(None);
    assert_eq!(Id::from_key("picture"), Id::Picture);
    assert!(!ALL.contains(&Id::Picture), "no button offers it");
    // no picture held: draw says "the caller draws the theme's own background"
    set_current(Id::Picture);
    let mut g = Gfx::new(96, 54);
    assert!(!draw(&mut g));
    // a red picture of another shape covers the whole screen
    assert!(set_picture(Some(solid(40, 40, [200, 20, 20]))));
    let g = drawn(Id::Picture);
    let mid = |g: &Gfx, x: usize, y: usize| { let o = (y * 96 + x) * 4; g.rgba()[o..o + 3].to_vec() };
    assert_eq!(mid(&g, 48, 27), vec![200, 20, 20]);
    assert_eq!(mid(&g, 1, 27), vec![200, 20, 20]);
    assert_eq!(mid(&g, 94, 27), vec![200, 20, 20]);
    assert!(!Id::Picture.is_light());
    assert!(set_picture(Some(solid(8, 8, [240, 240, 240]))));
    assert!(Id::Picture.is_light());
    // a half-and-half picture is cropped to the middle, not squeezed: wide picture 200x20, left black, right white
    let mut rgba = Vec::new();
    for _ in 0..20 {
        for x in 0..200 {
            let v = if x < 100 { 0 } else { 255 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    set_picture(Some((200, 20, rgba)));
    let g = drawn(Id::Picture);
    assert!(mid(&g, 20, 27)[0] < 20 && mid(&g, 76, 27)[0] > 235, "left black, right white");
}

#[test]
fn a_bad_picture_is_refused_and_a_big_one_is_shrunk() {
    let _r = Reset;
    assert!(!set_picture(Some((0, 5, vec![]))));
    assert!(!set_picture(Some((4, 4, vec![0; 10]))), "bytes do not match the size");
    assert!(!set_picture(Some((PICTURE_MAX_SIDE + 1, 1, vec![0; (PICTURE_MAX_SIDE as usize + 1) * 4]))));
    assert!(!has_picture());
    // 6000 x 4000 = 24 M pixels > the cap: halved once (3000 x 2000 = 6 M < 9.4 M)
    assert!(set_picture(Some(solid(6000, 4000, [10, 20, 30]))));
    PICTURE.with(|p| {
        let p = p.borrow();
        let p = p.as_ref().unwrap();
        assert_eq!((p.w, p.h), (3000, 2000));
        assert_eq!(&p.rgba[..4], &[10, 20, 30, 255]);
    });
    assert!(set_picture(None) == false && !has_picture());
}
