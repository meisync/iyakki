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
    /// How many times the whole animation plays; `None` is forever. After the last play the last frame stays.
    plays: Option<u32>,
}

impl Anim {
    /// The frame to show `ms` milliseconds after the animation started (it loops).
    pub fn frame_at(&self, ms: u64) -> &Pixmap {
        if let Some(n) = self.plays {
            if ms >= self.total_ms.saturating_mul(n as u64) {
                return &self.frames[self.frames.len() - 1].0;
            }
        }
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
#[derive(Clone)]
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

/// Decode a picture file; a GIF or WebP with several frames becomes an animation. `content_type` is the server's word, used only
/// when the bytes alone do not say what the file is (an SVG that starts with a long comment, say).
pub fn to_pic(bytes: &[u8], content_type: &str) -> Result<Pic, String> {
    let moving = match image::sniff(bytes) {
        Kind::Gif => iyakki_web::gif::decode_animated(bytes).ok(),
        Kind::Webp => iyakki_web::webp::decode_animated(bytes).ok(),
        _ => None,
    };
    {
        if let Some(a) = moving {
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
                return Ok(Pic::Anim(Arc::new(Anim { frames, total_ms: total, plays: a.plays() })));
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

// ------------------------------------------------------------------------------------------------ the decoded-picture cache (F-BROX-13)

/// A picture that was decoded once is kept by what it is made of, so the same file used again (a logo on every page of a site, a picture
/// after going back) is not decoded again. At most this many decoded bytes and this many pictures are kept; the one used longest ago goes first.
const CACHE_BYTES: usize = 48 << 20;
const CACHE_ITEMS: usize = 128;

fn pic_bytes(p: &Pic) -> usize {
    match p {
        Pic::Still(pm) => pm.data().len(),
        Pic::Anim(a) => a.frames.iter().map(|(pm, _)| pm.data().len()).sum(),
    }
}

/// What a file is made of: its length and two independent 64-bit sums of its bytes, plus the declared kind for an SVG that the bytes do not give away.
fn fingerprint(bytes: &[u8], svg_hint: bool) -> (usize, u64, u64, bool) {
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x9e3779b97f4a7c15u64);
    for &x in bytes {
        a = (a ^ x as u64).wrapping_mul(0x100000001b3);
        b = (b.rotate_left(5) ^ x as u64).wrapping_mul(0x9e3779b97f4a7c15);
    }
    (bytes.len(), a, b, svg_hint)
}

#[derive(Default)]
pub struct PicCache {
    map: std::collections::HashMap<(usize, u64, u64, bool), (Pic, usize, u64)>,
    bytes: usize,
    tick: u64,
    pub hits: u64,
    pub misses: u64,
}

impl PicCache {
    /// The picture for these bytes: from the cache (`true`) or decoded now with `decode` and kept (`false`). A failure is not kept.
    pub fn get_or_decode(&mut self, bytes: &[u8], svg_hint: bool, decode: impl FnOnce() -> Result<Pic, String>) -> Result<(Pic, bool), String> {
        let key = fingerprint(bytes, svg_hint);
        self.tick += 1;
        if let Some(e) = self.map.get_mut(&key) {
            e.2 = self.tick;
            self.hits += 1;
            return Ok((e.0.clone(), true));
        }
        self.misses += 1;
        let pic = decode()?;
        let size = pic_bytes(&pic);
        if size <= CACHE_BYTES / 4 {
            while (self.bytes + size > CACHE_BYTES || self.map.len() >= CACHE_ITEMS) && !self.map.is_empty() {
                let oldest = self.map.iter().min_by_key(|(_, v)| v.2).map(|(k, _)| *k);
                if let Some(k) = oldest {
                    if let Some(v) = self.map.remove(&k) {
                        self.bytes -= v.1;
                    }
                }
            }
            self.map.insert(key, (pic.clone(), size, self.tick));
            self.bytes += size;
        }
        Ok((pic, false))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

static PIC_CACHE: Mutex<Option<PicCache>> = Mutex::new(None);

/// [`to_pic`] through the shared cache; the flag says it came from there.
pub fn to_pic_cached(bytes: &[u8], content_type: &str) -> Result<(Pic, bool), String> {
    let svg_hint = content_type.to_ascii_lowercase().contains("svg");
    let mut g = PIC_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // the decode runs while the lock is held: a second worker that wants the same file waits for it instead of decoding it again
    g.get_or_insert_with(PicCache::default).get_or_decode(bytes, svg_hint, || to_pic(bytes, content_type))
}

/// How the shared cache did so far: (hits, misses, pictures kept).
pub fn pic_cache_stats() -> (u64, u64, usize) {
    let g = PIC_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    g.as_ref().map_or((0, 0, 0), |c| (c.hits, c.misses, c.len()))
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
        return to_pic_cached(&bytes, &mime).map(|p| p.0);
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
    to_pic_cached(&r.body, &r.content_type).map(|p| p.0)
}

/// A 2x2 animated GIF whose frames are single colours, each shown `delay_cs` hundredths of a second (for tests).
#[cfg(any(test, feature = "testing"))]
pub(crate) fn test_gif(colors: &[[u8; 3]], delay_cs: u16) -> Vec<u8> {
    test_gif_loops(colors, delay_cs, Some(0))
}

/// Like `test_gif` with the NETSCAPE loop count chosen (`None`: no block, so one play; `Some(0)`: forever).
#[cfg(any(test, feature = "testing"))]
pub(crate) fn test_gif_loops(colors: &[[u8; 3]], delay_cs: u16, loops: Option<u16>) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&[2, 0, 2, 0, 0x80, 0, 0]); // 2x2, a global palette of 2
    g.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
    if let Some(n) = loops {
        g.extend_from_slice(&[0x21, 0xFF, 11]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[3, 1, n as u8, (n >> 8) as u8, 0]);
    }
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
    fn a_loop_count_limits_the_plays_and_the_last_frame_stays() {
        let cols = [[255, 0, 0], [0, 255, 0]];
        let col = |a: &Anim, ms: u64| { let p = a.frame_at(ms).pixels()[0]; (p.red(), p.green(), p.blue()) };
        // no block: plays once, then stays on the last frame
        let Ok(Pic::Anim(a)) = to_pic(&test_gif_loops(&cols, 10, None), "") else { panic!() };
        assert_eq!((col(&a, 0), col(&a, 150), col(&a, 200), col(&a, 5_000)), ((255, 0, 0), (0, 255, 0), (0, 255, 0), (0, 255, 0)));
        // a count of 1: two plays in all (200 ms each pass of two frames at 100 ms)
        let Ok(Pic::Anim(a)) = to_pic(&test_gif_loops(&cols, 10, Some(1)), "") else { panic!() };
        assert_eq!((col(&a, 0), col(&a, 250), col(&a, 399), col(&a, 400), col(&a, 9_000)), ((255, 0, 0), (255, 0, 0), (0, 255, 0), (0, 255, 0), (0, 255, 0)));
        // forever
        let Ok(Pic::Anim(a)) = to_pic(&test_gif_loops(&cols, 10, Some(0)), "") else { panic!() };
        assert_eq!(col(&a, 1_000_000), (255, 0, 0));
    }

    #[test]
    fn an_animated_webp_becomes_an_animation_and_a_still_one_stays_a_still() {
        let Ok(Pic::Anim(a)) = to_pic(&file("webp_anim.webp"), "image/webp") else { panic!("expected an animation") };
        assert_eq!(a.len(), 2);
        assert_eq!((a.frame_at(0).width(), a.frame_at(0).height()), (40, 30));
        assert!(matches!(to_pic(&file("webp_lossless.webp"), "image/webp"), Ok(Pic::Still(_))));
        assert!(matches!(to_pic(&file("webp_lossy.webp"), ""), Ok(Pic::Still(_))));
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

    #[test]
    fn a_picture_decoded_once_is_not_decoded_again_and_a_different_file_is_its_own() {
        let mut c = PicCache::default();
        let a = test_gif(&[[250, 1, 2], [3, 4, 250]], 10);
        let mut decodes = 0;
        for want_hit in [false, true, true] {
            let (p, hit) = c.get_or_decode(&a, false, || {
                decodes += 1;
                to_pic(&a, "image/gif")
            })
            .unwrap();
            assert_eq!(hit, want_hit);
            assert!(matches!(p, Pic::Anim(_)));
        }
        assert_eq!(decodes, 1, "decoded once");
        let b = test_gif(&[[9, 9, 9], [8, 8, 8]], 10);
        let (_, hit) = c.get_or_decode(&b, false, || to_pic(&b, "image/gif")).unwrap();
        assert!(!hit, "other bytes, other picture");
        assert_eq!((c.hits, c.misses, c.len()), (2, 2, 2));
        // a failure is not kept: the next try decodes (and fails) again
        let bad = b"not a picture".to_vec();
        let mut tries = 0;
        for _ in 0..2 {
            assert!(c.get_or_decode(&bad, false, || { tries += 1; to_pic(&bad, "") }).is_err());
        }
        assert_eq!(tries, 2);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn the_cache_forgets_the_picture_used_longest_ago_when_it_is_full_by_count() {
        let mut c = PicCache::default();
        let gifs: Vec<Vec<u8>> = (0..CACHE_ITEMS as u32 + 3).map(|i| test_gif(&[[(i % 250) as u8, (i / 250) as u8, 7], [1, 2, 3]], 10 + (i % 5) as u16 + 20 * (i / 5) as u16)).collect();
        for g in &gifs {
            c.get_or_decode(g, false, || to_pic(g, "image/gif")).unwrap();
        }
        assert_eq!(c.len(), CACHE_ITEMS, "never more than the limit");
        let (_, newest) = c.get_or_decode(gifs.last().unwrap(), false, || to_pic(gifs.last().unwrap(), "image/gif")).unwrap();
        assert!(newest, "the newest is still there");
        let (_, oldest) = c.get_or_decode(&gifs[0], false, || to_pic(&gifs[0], "image/gif")).unwrap();
        assert!(!oldest, "the oldest went first");
    }

    #[test]
    fn the_shared_cache_serves_the_second_use_of_the_same_file_and_an_animated_webp_keeps_its_frames_and_delays() {
        let webp = file("webp_anim.webp");
        let (before_hits, _, _) = pic_cache_stats();
        let (first, hit1) = to_pic_cached(&webp, "image/webp").unwrap();
        let (second, hit2) = to_pic_cached(&webp, "image/webp").unwrap();
        assert!(!hit1 || before_hits > 0, "the first use may already be known to an earlier test");
        assert!(hit2, "the second use comes from the cache");
        for p in [first, second] {
            let Pic::Anim(a) = p else { panic!("an animated WebP is an animation") };
            assert_eq!(a.frames.len(), 2, "all frames, not only the first");
            assert!(a.frames.iter().all(|f| f.1 == 100), "with their delays");
        }
        assert!(pic_cache_stats().0 > before_hits);
    }
}
