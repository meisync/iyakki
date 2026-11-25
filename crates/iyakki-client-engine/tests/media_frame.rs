//! F-SDK-147 (UT2): the media view's picture is drawn by the client engine through the display list's image operation, and a newer picture
//! changes the pixels. The view runs over the in-memory media port (a video whose colour follows the clock); the list goes through its text
//! form, as it does to the page, into the engine's own raster.

use iyakki_client_engine::uidraw::UiSurface;
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::{ListCanvas, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::media::MemMedia;
use iyakki_ui_sdk::mediaview::MediaView;
use iyakki_ui_sdk::size::Metrics;

fn shown(v: &mut MediaView, w: f32, h: f32) -> (String, Rect) {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, 1.0, false);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    let list = c.finish();
    let at = list.ops.iter().find_map(|o| if let Op::Image(r, _) = o { Some(*r) } else { None }).expect("the video picture is an image line");
    (list.to_text(), at)
}

fn centre_pixel(text: &str, at: Rect, w: usize) -> [u8; 4] {
    let mut s = UiSurface::default();
    s.load(text, 1.0).unwrap();
    let (px, fw, _, _) = s.framebuffer();
    assert_eq!(fw, w);
    let (x, y) = ((at.x + at.w / 2.0) as usize, (at.y + at.h / 2.0) as usize);
    let i = (y * fw + x) * 4;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn the_engine_draws_the_video_picture_of_the_media_view_and_a_newer_picture_changes_the_pixels() {
    let m = MemMedia::new();
    m.put_video("clips/one.avi", 10_000, 10, 64, 48);
    let mut v = MediaView::new(Box::new(m.clone()), 1200);
    v.tick();
    v.audio_mut().play_index(0);
    v.tick();
    let (text, at) = shown(&mut v, 500.0, 700.0);
    assert!(text.contains("\nimage "), "the media view uses the image operation and no operation of its own");
    assert_eq!(centre_pixel(&text, at, 500), [0, 40, 80, 255], "the first picture of the clip");
    m.advance(1000);
    v.tick();
    let (text, at) = shown(&mut v, 500.0, 700.0);
    assert_eq!(centre_pixel(&text, at, 500), [10, 40, 80, 255], "the picture of one second on");
}
