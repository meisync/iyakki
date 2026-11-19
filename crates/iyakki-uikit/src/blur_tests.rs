use super::*;
use crate::gfx::Gfx;
use crate::theme::ThemeId;

fn hard_edge(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            let i = ((y * w + x) * 4) as usize;
            v[i..i + 3].copy_from_slice(&[0, 0, 0]);
        }
    }
    v
}

#[test]
fn a_box_blur_spreads_a_hard_edge_and_keeps_flat_areas() {
    let (w, h) = (64usize, 8usize);
    let mut px = hard_edge(w as u32, h as u32);
    box_blur(&mut px, w, h, 4);
    let at = |x: usize| px[(3 * w + x) * 4];
    assert_eq!(at(2), 0, "far left stays black");
    assert_eq!(at(61), 255, "far right stays white");
    assert!(at(31) > 0 && at(31) < 128 && at(32) > 128 && at(32) < 255, "the edge is soft: {} {}", at(31), at(32));
    assert!(at(28) < at(32) && at(32) < at(36), "and rises without a step back");
}

#[test]
fn a_flat_picture_stays_exactly_flat() {
    let mut px = vec![90u8; 40 * 30 * 4];
    for _ in 0..3 {
        box_blur(&mut px, 40, 30, 5);
    }
    assert!(px.iter().all(|&v| v == 90));
}

#[test]
fn the_frost_is_a_quarter_size_and_too_small_screens_get_none() {
    let f = build(&hard_edge(320, 200), 320, 200).expect("frost");
    assert_eq!((f.pm.width(), f.pm.height(), f.factor), (80, 50, SHRINK));
    assert!(build(&hard_edge(16, 16), 16, 16).is_none());
    assert!(build(&[0; 4], 320, 200).is_none(), "a wrong sized buffer is refused, not read");
}

#[test]
fn the_frost_is_built_once_per_background_and_again_only_when_the_background_changes() {
    crate::theme::set_current(ThemeId::Dark);
    let mut g = Gfx::new(320, 200);
    crate::fx::background(&mut g);
    let b0 = builds();
    let a = current().expect("frost");
    assert_eq!(builds(), b0 + 1);
    for _ in 0..5 {
        crate::fx::background(&mut g);
        let again = current().expect("frost");
        assert!(Rc::ptr_eq(&a, &again), "the same picture is lent again");
    }
    assert_eq!(builds(), b0 + 1, "five more frames built nothing");
    crate::wallpaper::set_current(crate::wallpaper::Id::Dusk);
    crate::fx::background(&mut g);
    let _ = current().expect("frost");
    assert_eq!(builds(), b0 + 2, "a new wallpaper builds it once");
    let mut g2 = Gfx::new(400, 240);
    crate::fx::background(&mut g2);
    let _ = current().expect("frost");
    assert_eq!(builds(), b0 + 3, "a new size builds it once");
    crate::wallpaper::set_current(crate::wallpaper::Id::Theme);
}

#[test]
fn the_island_over_the_frost_is_softer_than_the_wallpaper_and_the_ink_still_reads() {
    crate::theme::set_current(ThemeId::Dark);
    // a wallpaper with a sharp black and white split: the frame behind the island must not show the split sharply
    let mut g = Gfx::new(640, 360);
    crate::fx::background(&mut g);
    let frost = current().expect("frost");
    let (w, h) = (g.width(), g.height());
    // paint the split into the cached picture's source by building a frost of our own and drawing it through the island shape
    let split = hard_edge(w, h);
    let own = build(&split, w, h).expect("own frost");
    let mut plain = Gfx::new(w, h);
    plain.pm.data_mut().copy_from_slice(&split);
    let mut framed = Gfx::new(w, h);
    framed.pm.data_mut().copy_from_slice(&split);
    framed.rrect_picture(100.0, 60.0, 440.0, 240.0, 30.0, &own.pm, own.factor);
    let row = 180usize;
    let px = |g: &Gfx, x: usize| g.rgba()[(row * w as usize + x) * 4];
    // across the island's middle the sharp step (0 -> 255 in one pixel) became a ramp
    let mid = w as usize / 2;
    assert_eq!((px(&plain, mid - 1), px(&plain, mid)), (0, 255));
    let ramp = px(&framed, mid + 6) as i32 - px(&framed, mid - 6) as i32;
    assert!(ramp > 20 && px(&framed, mid) > 0 && px(&framed, mid) < 255, "the split is soft inside the island: {ramp}");
    assert_eq!(px(&framed, 50), 0, "outside the island nothing changed");
    drop(frost);
}
