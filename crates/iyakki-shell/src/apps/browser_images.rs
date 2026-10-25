//! Pictures for the browser: fetched in the background after a page is shown (a few at a time, with the browser's cookies
//! and the account's host policy), decoded, shrunk if huge, and handed back as ready-to-draw pixmaps.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use iyakki_web::image::{self, Kind};
use iyakki_web::{CookieStore, Fetch, Jar, Request, Url};
use tiny_skia::Pixmap;

/// What the browser knows about one picture.
#[derive(Clone)]
pub enum ImgState {
    /// Not asked for yet.
    Todo,
    Loading,
    Ready(Arc<Pixmap>),
    /// An animated picture (GIF with several frames): the frame shown depends on the clock.
    Animated(Arc<Anim>),
    Failed(String),
}

impl ImgState {
    pub fn settled(&self) -> bool {
        matches!(self, ImgState::Ready(_) | ImgState::Animated(_) | ImgState::Failed(_))
    }
}

/// Pictures larger than this are shrunk when they arrive.
const MAX_SIDE: u32 = 2048;
/// At most this many pictures per page, and this many bytes in all.
pub const MAX_IMAGES: usize = 60;
const MAX_BYTES: usize = 14 << 20;

pub type Msg = (u64, usize, Result<Pic, String>);

/// Frames of all of one animation together may have at most this many pixels (later frames are dropped).
const MAX_ANIM_PIXELS: u64 = 8_000_000;

/// An animation: ready-to-draw frames and how long each one stays.
pub struct Anim {
    frames: Vec<(Pixmap, u32)>,
    total_ms: u64,
}

impl Anim {
    /// The frame to show `ms` milliseconds after the animation started (it loops).
    pub fn frame_at(&self, ms: u64) -> &Pixmap {
        let mut t = ms % self.total_ms.max(1);
        for (pm, d) in &self.frames {
            if t < *d as u64 {
                return pm;
            }
            t -= *d as u64;
        }
        &self.frames[self.frames.len() - 1].0
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// What a finished load hands back.
pub enum Pic {
    Still(Arc<Pixmap>),
    Anim(Arc<Anim>),
}

impl Pic {
    pub fn size(&self) -> (u32, u32) {
        match self {
            Pic::Still(p) => (p.width(), p.height()),
            Pic::Anim(a) => (a.frames[0].0.width(), a.frames[0].0.height()),
        }
    }
}

/// Straight RGBA to the premultiplied pixmap the renderer wants.
pub(crate) fn bitmap_pixmap(w: u32, h: u32, rgba: &[u8]) -> Result<Pixmap, String> {
    let mut pm = Pixmap::new(w, h).ok_or("the picture has no size")?;
    for (dst, src) in pm.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = src[3] as u32;
        dst[0] = (src[0] as u32 * a / 255) as u8;
        dst[1] = (src[1] as u32 * a / 255) as u8;
        dst[2] = (src[2] as u32 * a / 255) as u8;
        dst[3] = src[3];
    }
    Ok(pm)
}

/// Decode a picture file; a GIF with several frames becomes an animation. `content_type` is the server's word, used only
/// when the bytes alone do not say what the file is (an SVG that starts with a long comment, say).
pub fn to_pic(bytes: &[u8], content_type: &str) -> Result<Pic, String> {
    if image::sniff(bytes) == Kind::Gif {
        if let Ok(a) = iyakki_web::gif::decode_animated(bytes) {
            if a.frames.len() > 1 {
                let mut frames = Vec::new();
                let (mut total, mut pixels) = (0u64, 0u64);
                for f in &a.frames {
                    pixels += a.width as u64 * a.height as u64;
                    if !frames.is_empty() && pixels > MAX_ANIM_PIXELS {
                        break;
                    }
                    let pm = shrink(bitmap_pixmap(a.width, a.height, &f.rgba)?);
                    total += f.delay_ms as u64;
                    frames.push((pm, f.delay_ms));
                }
                return Ok(Pic::Anim(Arc::new(Anim { frames, total_ms: total })));
            }
        }
    }
    if image::sniff(bytes) == Kind::Unknown && content_type.to_ascii_lowercase().contains("svg") {
        let b = iyakki_web::svg::decode(bytes)?;
        return Ok(Pic::Still(Arc::new(shrink(bitmap_pixmap(b.width, b.height, &b.rgba)?))));
    }
    to_pixmap(bytes).map(|p| Pic::Still(Arc::new(p)))
}

/// Decode a picture file into a pixmap (premultiplied, as the renderer wants it).
pub fn to_pixmap(bytes: &[u8]) -> Result<Pixmap, String> {
    let pm = match image::sniff(bytes) {
        Kind::Png => Pixmap::decode_png(bytes).map_err(|e| format!("the PNG could not be read ({e})"))?,
        Kind::Jpeg | Kind::Gif | Kind::Webp | Kind::Svg => {
            let b = image::decode(bytes)?;
            bitmap_pixmap(b.width, b.height, &b.rgba)?
        }
        _ => return image::decode(bytes).map(|_| unreachable!()),
    };
    Ok(shrink(pm))
}

/// Average blocks of pixels so that neither side is over [`MAX_SIDE`].
fn shrink(pm: Pixmap) -> Pixmap {
    let (w, h) = (pm.width(), pm.height());
    let f = w.max(h).div_ceil(MAX_SIDE);
    if f <= 1 {
        return pm;
    }
    let (nw, nh) = ((w / f).max(1), (h / f).max(1));
    let Some(mut out) = Pixmap::new(nw, nh) else { return pm };
    let src = pm.data();
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..f {
                for dx in 0..f {
                    let (sx, sy) = ((x * f + dx).min(w - 1), (y * f + dy).min(h - 1));
                    let o = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += src[o + c] as u32;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out.data_mut()[o + c] = (acc[c] / (f * f)) as u8;
            }
        }
    }
    out
}

/// Fetch and decode every picture in `jobs` (`index`, `src as written`), reporting each as it finishes.
#[allow(clippy::too_many_arguments)]
pub fn spawn_loader(page: Url, jobs: Vec<(usize, String)>, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Sync + Clone + 'static, cookies: Arc<Mutex<CookieStore>>, serial: u64, tx: Sender<Msg>) {
    let queue = Arc::new(Mutex::new(jobs.into_iter().take(MAX_IMAGES).collect::<VecDeque<_>>()));
    let budget = Arc::new(AtomicUsize::new(0));
    let workers = super::browser_speed::image_workers(&super::browser_speed::machine(std::path::Path::new("/")));
    for _ in 0..workers {
        let (queue, budget, page, web, allow, cookies, tx) = (queue.clone(), budget.clone(), page.clone(), web.clone(), allow.clone(), cookies.clone(), tx.clone());
        let _ = std::thread::Builder::new().name("iyakki-browser-image".into()).stack_size(16 << 20).spawn(move || loop {
            let Some((idx, src)) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { return };
            let result = fetch_one(&page, &src, web.as_ref(), &allow, &cookies, &budget);
            if tx.send((serial, idx, result)).is_err() {
                return;
            }
        });
    }
}

fn fetch_one(page: &Url, src: &str, web: &dyn Fetch, allow: &dyn Fn(&str) -> bool, cookies: &Mutex<CookieStore>, budget: &AtomicUsize) -> Result<Pic, String> {
    if src.is_empty() {
        return Err("the picture has no address".into());
    }
    if let Some((mime, bytes)) = image::parse_data_url(src) {
        return to_pic(&bytes, &mime);
    }
    let url = page.join(src)?;
    // a protected page does not load unprotected pictures: anyone on the network could swap them
    if page.https && !url.https {
        return Err("blocked: this picture would load without protection (http) on a secure page".into());
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut req = Request::get(&url);
    req.referer = page.to_string();
    req.priority = iyakki_web::net::IMAGE;
    let (_, r) = iyakki_web::fetch_following(web, req, allow, Some(Jar { store: cookies, now }))?;
    if r.status >= 400 {
        return Err(format!("the server answered {}", r.status));
    }
    if budget.fetch_add(r.body.len(), Ordering::Relaxed) + r.body.len() > MAX_BYTES {
        return Err("this page's pictures are over the size limit".into());
    }
    to_pic(&r.body, &r.content_type)
}

/// A 2x2 animated GIF whose frames are single colours, each shown `delay_cs` hundredths of a second (for tests).
#[cfg(test)]
pub(crate) fn test_gif(colors: &[[u8; 3]], delay_cs: u16) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&[2, 0, 2, 0, 0x80, 0, 0]); // 2x2, a global palette of 2
    g.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
    for c in colors {
        // a local palette per frame, index 0 = the colour
        g.extend_from_slice(&[0x21, 0xF9, 4, 4, delay_cs as u8, (delay_cs >> 8) as u8, 0, 0]);
        g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0x80]);
        g.extend_from_slice(&[c[0], c[1], c[2], 0, 0, 0]);
        g.push(2);
        // clear (4), four literal 0 codes, end (5): 3 bits each, least significant bit first
        let mut bits: Vec<u8> = Vec::new();
        for code in [4u8, 0, 0, 0, 0, 5] {
            for i in 0..3 {
                bits.push((code >> i) & 1);
            }
        }
        let bytes: Vec<u8> = bits.chunks(8).map(|ch| ch.iter().enumerate().fold(0u8, |a, (i, b)| a | (b << i))).collect();
        g.push(bytes.len() as u8);
        g.extend_from_slice(&bytes);
        g.push(0);
    }
    g.push(0x3B);
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_web::Response;
    use std::collections::HashMap;

    fn file(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/../iyakki-web/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn png_jpeg_and_gif_become_pixmaps_and_other_things_say_why_not() {
        for (name, w, h) in [("rgb.png", 67, 45), ("base420.jpg", 67, 45), ("prog444.jpg", 67, 45), ("pal.gif", 67, 45)] {
            let pm = to_pixmap(&file(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((pm.width(), pm.height()), (w, h), "{name}");
        }
        assert!(to_pixmap(b"RIFF\0\0\0\0WEBPVP8 ").unwrap_err().contains("WebP"));
        let svg = to_pixmap(b"<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'><rect width='20' height='10' fill='red'/></svg>").unwrap();
        assert_eq!((svg.width(), svg.height()), (20, 10));
        assert_eq!(svg.pixels()[0].red(), 255);
        assert!(to_pixmap(b"not a picture").is_err());
        assert!(to_pixmap(&[]).is_err());
    }

    #[test]
    fn a_multi_frame_gif_becomes_an_animation_that_loops_on_the_clock() {
        let g = test_gif(&[[255, 0, 0], [0, 255, 0], [0, 0, 255]], 10);
        let Ok(Pic::Anim(a)) = to_pic(&g, "image/gif") else { panic!("expected an animation") };
        assert_eq!(a.len(), 3);
        let col = |ms: u64| { let p = a.frame_at(ms).pixels()[0]; (p.red(), p.green(), p.blue(), p.alpha()) };
        assert_eq!(col(0), (255, 0, 0, 255));
        assert_eq!(col(99), (255, 0, 0, 255));
        assert_eq!(col(100), (0, 255, 0, 255));
        assert_eq!(col(250), (0, 0, 255, 255));
        assert_eq!(col(300), (255, 0, 0, 255), "loops");
        assert_eq!(col(3_000_000_100), (0, 255, 0, 255));
        // one frame is a still picture
        assert!(matches!(to_pic(&test_gif(&[[1, 2, 3]], 10), ""), Ok(Pic::Still(_))));
        assert!(matches!(to_pic(&file("pal.gif"), ""), Ok(Pic::Still(_))));
    }

    #[test]
    fn svg_arrives_by_content_type_or_content_and_through_data_addresses() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='8' height='6'><circle cx='4' cy='3' r='3' fill='blue'/></svg>";
        let Ok(Pic::Still(p)) = to_pic(svg, "image/svg+xml") else { panic!() };
        assert_eq!((p.width(), p.height()), (8, 6));
        assert!(p.pixels()[3 * 8 + 4].blue() == 255);
        // a long comment first: only the server's word says it is an SVG
        let mut late = format!("<!--{}-->", "x".repeat(5000)).into_bytes();
        late.extend_from_slice(svg);
        assert!(to_pic(&late, "text/plain").is_err());
        assert!(matches!(to_pic(&late, "image/svg+xml;charset=utf-8"), Ok(Pic::Still(_))));
        let url = format!("data:image/svg+xml;utf8,{}", String::from_utf8_lossy(svg).replace('<', "%3C").replace('>', "%3E").replace(' ', "%20"));
        let (mime, bytes) = image::parse_data_url(&url).unwrap();
        assert_eq!(mime, "image/svg+xml");
        assert!(matches!(to_pic(&bytes, &mime), Ok(Pic::Still(_))));
        assert!(to_pic(b"<svg><", "image/svg+xml").is_ok(), "a broken SVG is a picture of what could be drawn");
        assert!(to_pic(b"<html>", "image/svg+xml").is_err());
    }

    #[test]
    fn gif_transparency_stays_transparent_after_premultiplying() {
        let pm = to_pixmap(&file("pal.gif")).unwrap();
        assert!(pm.pixels().iter().any(|p| p.alpha() == 0), "the transparent index became transparent");
        assert!(pm.pixels().iter().filter(|p| p.alpha() == 255).count() > 100);
    }

    #[test]
    fn huge_pictures_are_shrunk() {
        let pm = Pixmap::new(5000, 3000).unwrap();
        let s = shrink(pm);
        assert!(s.width() <= MAX_SIDE && s.height() <= MAX_SIDE && s.width() >= 1500, "{}x{}", s.width(), s.height());
        let small = shrink(Pixmap::new(100, 50).unwrap());
        assert_eq!((small.width(), small.height()), (100, 50));
    }

    struct Site(HashMap<String, Response>);
    impl Fetch for Site {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.0.get(&url.to_string()).cloned().ok_or_else(|| "not found".into())
        }
    }

    #[test]
    fn the_loader_fetches_with_policy_decodes_and_reports_each_picture() {
        let png = file("rgb.png");
        let mut pages = HashMap::new();
        pages.insert("https://a.org/img/p.png".to_string(), Response { status: 200, content_type: "image/png".into(), body: png.clone(), ..Response::default() });
        pages.insert("https://a.org/missing.png".to_string(), Response { status: 404, ..Response::default() });
        pages.insert("https://a.org/text.png".to_string(), Response { status: 200, content_type: "text/plain".into(), body: b"hello".to_vec(), ..Response::default() });
        pages.insert("https://evil.net/x.png".to_string(), Response { status: 200, content_type: "image/png".into(), body: png, ..Response::default() });
        let web: Arc<dyn Fetch> = Arc::new(Site(pages));
        let (tx, rx) = std::sync::mpsc::channel();
        let jobs = vec![(0, "/img/p.png".to_string()), (1, "/missing.png".to_string()), (2, "text.png".to_string()), (3, "https://evil.net/x.png".to_string()), (4, "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7".to_string()), (5, String::new())];
        spawn_loader(Url::parse("https://a.org/dir/page").unwrap(), jobs, web, |h| h == "a.org", Arc::new(Mutex::new(CookieStore::new())), 7, tx);
        let mut got: Vec<(usize, Result<(u32, u32), String>)> = Vec::new();
        for _ in 0..6 {
            let (serial, idx, r) = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("every job reports");
            assert_eq!(serial, 7);
            got.push((idx, r.map(|p| p.size())));
        }
        got.sort_by_key(|g| g.0);
        assert_eq!(got[0].1, Ok((67, 45)));
        assert!(got[1].1.as_ref().unwrap_err().contains("404"));
        assert!(got[2].1.is_err(), "text is not a picture");
        assert!(got[3].1.as_ref().unwrap_err().contains("may not open pages from evil.net"), "{:?}", got[3]);
        assert_eq!(got[4].1, Ok((1, 1)), "data: addresses are decoded without the network");
        assert!(got[5].1.is_err());
    }
}
