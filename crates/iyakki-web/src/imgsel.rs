//! Which picture an `<img>` shows: the `srcset` choice (`1x`/`2x` and `w` descriptors, with `sizes`), the first usable
//! `<source>` of a `<picture>` (by `type` and a simple `media` test), and whether the picture is `loading=lazy`.
//!
//! The choice is made for the window the page is built for ([`with_window`]: the reader tells it, and builds the page again when the window
//! changes size); outside that, for a nominal window of [`NOMINAL_VIEWPORT`] CSS pixels. One device pixel per CSS pixel is assumed unless the reader says otherwise ([`with_window_dppx`]). Media conditions understood: `min-width`, `max-width`,
//! `screen`, `all`, joined with `and`; anything else (colour scheme, orientation, print, ...) does not match.

use crate::html::Tree;

/// The window width, in CSS pixels, that `sizes` and `media` are judged against.
pub const NOMINAL_VIEWPORT: f32 = 960.0;

thread_local! {
    static WINDOW: std::cell::Cell<f32> = const { std::cell::Cell::new(NOMINAL_VIEWPORT) };
    static DPPX: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// Device pixels per CSS pixel the choices on this thread are made for now.
fn dppx() -> f32 {
    DPPX.with(std::cell::Cell::get)
}

/// The window width the choices on this thread are made for now.
fn window() -> f32 {
    WINDOW.with(std::cell::Cell::get)
}

/// Runs `f` with picture choices (`srcset` widths, `sizes`, `media`) made for a window `width` CSS pixels wide (kept between 200 and 8000).
pub fn with_window<T>(width: f32, f: impl FnOnce() -> T) -> T {
    with_window_dppx(width, 1.0, f)
}

/// [`with_window`] for a screen that has `dppx` device pixels per CSS pixel (kept between 1 and 4): a `srcset` takes the smallest picture that is
/// sharp at that density, and `min-resolution` and `-webkit-min-device-pixel-ratio` are judged against it.
pub fn with_window_dppx<T>(width: f32, dppx: f32, f: impl FnOnce() -> T) -> T {
    let w = if width.is_finite() { width.clamp(200.0, 8000.0) } else { NOMINAL_VIEWPORT };
    let d = if dppx.is_finite() { dppx.clamp(1.0, 4.0) } else { 1.0 };
    let old = WINDOW.with(|c| c.replace(w));
    let old_d = DPPX.with(|c| c.replace(d));
    let out = f();
    WINDOW.with(|c| c.set(old));
    DPPX.with(|c| c.set(old_d));
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Desc {
    X(f32),
    W(f32),
}

/// `url descriptor, url descriptor` (a descriptor is optional: `1x`). A `data:` address may hold commas.
fn candidates(v: &str) -> Vec<(String, Desc)> {
    let b: Vec<char> = v.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        while i < b.len() && (b[i].is_whitespace() || b[i] == ',') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        let mut url: String = b[start..i].iter().collect();
        let mut desc = String::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',').to_string();
        } else {
            while i < b.len() && b[i] != ',' {
                desc.push(b[i]);
                i += 1;
            }
        }
        let d = desc.trim().to_ascii_lowercase();
        let parsed = if let Some(n) = d.strip_suffix('x') {
            n.trim().parse::<f32>().ok().filter(|n| *n > 0.0).map(Desc::X)
        } else if let Some(n) = d.strip_suffix('w') {
            n.trim().parse::<f32>().ok().filter(|n| *n > 0.0).map(Desc::W)
        } else if d.is_empty() {
            Some(Desc::X(1.0))
        } else {
            None
        };
        if let (false, Some(p)) = (url.is_empty(), parsed) {
            out.push((url, p));
        }
    }
    out
}

/// One `(min-width: 600px)` style test, or a list joined with `and`.
pub fn media_matches(q: &str) -> bool {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() || q == "all" || q == "screen" {
        return true;
    }
    let q = q.strip_prefix("screen and ").or_else(|| q.strip_prefix("all and ")).unwrap_or(&q);
    q.split(" and ").all(|part| {
        let p = part.trim().trim_start_matches('(').trim_end_matches(')');
        let Some((k, val)) = p.split_once(':') else { return false };
        let k = k.trim();
        // resolution: `2dppx`, `2x`, `192dpi`, and the older plain ratio of `-webkit-min-device-pixel-ratio: 2`
        if matches!(k, "min-resolution" | "max-resolution" | "resolution" | "-webkit-min-device-pixel-ratio" | "-webkit-max-device-pixel-ratio" | "min-device-pixel-ratio" | "max-device-pixel-ratio" | "-webkit-device-pixel-ratio") {
            let v = val.trim();
            let ratio = if let Some(n) = v.strip_suffix("dppx").or_else(|| v.strip_suffix('x')) {
                n.trim().parse::<f32>().ok()
            } else if let Some(n) = v.strip_suffix("dpi") {
                n.trim().parse::<f32>().ok().map(|n| n / 96.0)
            } else if k.contains("device-pixel-ratio") {
                v.parse::<f32>().ok()
            } else {
                None
            };
            let Some(r) = ratio else { return false };
            return if k.contains("min") { dppx() >= r } else if k.contains("max") { dppx() <= r } else { (dppx() - r).abs() < 0.01 };
        }
        let Some(px) = css_px(val.trim()) else { return false };
        match k {
            "min-width" => window() >= px,
            "max-width" => window() <= px,
            _ => false,
        }
    })
}

/// `600px`, `50vw`, `40em` (16 px per em) or a bare `0`.
fn css_px(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(n) = v.strip_suffix("px") {
        return n.trim().parse().ok();
    }
    if let Some(n) = v.strip_suffix("vw") {
        return n.trim().parse::<f32>().ok().map(|n| n * window() / 100.0);
    }
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    (v == "0").then_some(0.0)
}

/// Split at commas outside brackets.
fn top_level_commas(v: &str) -> Vec<&str> {
    let (mut depth, mut last, mut out) = (0i32, 0, Vec::new());
    for (i, c) in v.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&v[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    out.push(&v[last..]);
    out
}

/// The width the picture will have: the first `sizes` entry whose condition holds, else the window.
fn slot_width(sizes: &str) -> f32 {
    for e in top_level_commas(sizes) {
        let e = e.trim();
        if e.is_empty() {
            continue;
        }
        let (cond, len) = match e.strip_prefix('(') {
            Some(_) => {
                let mut depth = 0;
                let mut cut = e.len();
                for (i, c) in e.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                cut = i + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                (&e[..cut], e[cut..].trim())
            }
            None => ("", e),
        };
        if cond.is_empty() || media_matches(cond) {
            if let Some(px) = css_px(len).filter(|p| *p > 0.0) {
                return px;
            }
            // a length we cannot work out (`calc(...)`): fall back to the window
            return window();
        }
    }
    window()
}

/// The address a `srcset` (with its `sizes`) asks for on a one-to-one screen: the smallest picture that is at least as sharp as
/// the screen, else the sharpest there is.
pub fn pick_srcset(srcset: &str, sizes: &str) -> Option<String> {
    let slot = slot_width(sizes);
    let cands: Vec<(String, f32)> = candidates(srcset)
        .into_iter()
        .map(|(u, d)| {
            let density = match d {
                Desc::X(x) => x,
                Desc::W(w) => w / slot,
            };
            (u, density)
        })
        .collect();
    let enough = cands.iter().filter(|c| c.1 >= dppx()).min_by(|a, b| a.1.total_cmp(&b.1));
    let best = enough.or_else(|| cands.iter().max_by(|a, b| a.1.total_cmp(&b.1)));
    best.map(|c| c.0.clone())
}

/// A `type` the reader can draw (an empty or missing type is accepted).
pub fn type_supported(t: &str) -> bool {
    let t = t.trim().to_ascii_lowercase();
    t.is_empty() || matches!(t.split(';').next().unwrap_or("").trim(), "image/png" | "image/apng" | "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/gif" | "image/webp" | "image/svg+xml")
}

/// `loading=lazy` on the element.
pub fn is_lazy(tree: &Tree, n: usize) -> bool {
    tree.attr(n, "loading").is_some_and(|l| l.trim().eq_ignore_ascii_case("lazy"))
}

/// The address of the picture the `<img>` node `n` shows, or an empty string.
pub fn image_source(tree: &Tree, n: usize) -> String {
    // inside a <picture>: the first <source> before the <img> that the reader can use
    if let Some(p) = tree.parent_of(n).filter(|&p| tree.tag(p) == Some("picture")) {
        for &c in &tree.nodes[p].children {
            if c == n {
                break;
            }
            if tree.tag(c) != Some("source") {
                continue;
            }
            if !type_supported(tree.attr(c, "type").unwrap_or("")) || !media_matches(tree.attr(c, "media").unwrap_or("")) {
                continue;
            }
            if let Some(u) = tree.attr(c, "srcset").and_then(|s| pick_srcset(s, tree.attr(c, "sizes").unwrap_or(""))) {
                return u;
            }
        }
    }
    if let Some(u) = tree.attr(n, "srcset").and_then(|s| pick_srcset(s, tree.attr(n, "sizes").unwrap_or(""))) {
        return u;
    }
    let attr = |k: &str| tree.attr(n, k).map(str::trim).filter(|v| !v.is_empty());
    attr("src").or_else(|| attr("data-src")).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    fn src_of(page: &str) -> Vec<String> {
        let t = html::parse(page);
        (0..t.nodes.len()).filter(|&n| t.tag(n) == Some("img")).map(|n| image_source(&t, n)).collect()
    }

    #[test]
    fn density_descriptors_pick_the_smallest_picture_that_is_sharp_enough() {
        assert_eq!(pick_srcset("a.png 1x, b.png 2x", "").as_deref(), Some("a.png"));
        assert_eq!(pick_srcset("b.png 2x, a.png", "").as_deref(), Some("a.png"), "no descriptor means 1x");
        assert_eq!(pick_srcset("b.png 2x, c.png 3x", "").as_deref(), Some("b.png"));
        assert_eq!(pick_srcset("a.png 0.5x, b.png 0.75x", "").as_deref(), Some("b.png"), "else the sharpest there is");
        assert_eq!(pick_srcset("", ""), None);
        assert_eq!(pick_srcset("a.png 7q", ""), None, "a descriptor that is not understood drops the candidate");
    }

    #[test]
    fn a_dense_screen_takes_the_picture_sharp_at_its_density_and_a_plain_one_is_unchanged() {
        let set = "a.png 1x, b.png 2x, c.png 3x";
        assert_eq!(pick_srcset(set, "").as_deref(), Some("a.png"));
        assert_eq!(with_window_dppx(960.0, 2.0, || pick_srcset(set, "")).as_deref(), Some("b.png"));
        assert_eq!(with_window_dppx(960.0, 3.0, || pick_srcset(set, "")).as_deref(), Some("c.png"));
        assert_eq!(with_window_dppx(960.0, 2.5, || pick_srcset(set, "")).as_deref(), Some("c.png"), "the next sharper one");
        assert_eq!(with_window_dppx(960.0, 4.0, || pick_srcset("a.png 1x, b.png 2x", "")).as_deref(), Some("b.png"), "else the sharpest there is");
        // width descriptors: a 400 px slot on a 2x screen wants 800 pixels
        let w = "s.jpg 400w, m.jpg 800w, l.jpg 1600w";
        assert_eq!(pick_srcset(w, "400px").as_deref(), Some("s.jpg"));
        assert_eq!(with_window_dppx(960.0, 2.0, || pick_srcset(w, "400px")).as_deref(), Some("m.jpg"));
        // the old value comes back after the call, and a bad one means 1x
        assert_eq!(pick_srcset(set, "").as_deref(), Some("a.png"));
        assert_eq!(with_window_dppx(960.0, f32::NAN, || pick_srcset(set, "")).as_deref(), Some("a.png"));
        assert_eq!(with_window_dppx(960.0, 0.2, || pick_srcset(set, "")).as_deref(), Some("a.png"));
    }

    #[test]
    fn resolution_conditions_follow_the_density() {
        let at = |d: f32, q: &str| with_window_dppx(960.0, d, || media_matches(q));
        assert!(!at(1.0, "(min-resolution: 2dppx)") && at(2.0, "(min-resolution: 2dppx)") && at(2.0, "(min-resolution: 192dpi)") && !at(1.0, "(min-resolution: 192dpi)"));
        assert!(at(2.0, "(-webkit-min-device-pixel-ratio: 2)") && !at(1.0, "(-webkit-min-device-pixel-ratio: 1.5)") && at(1.0, "(max-resolution: 1.5dppx)") && !at(2.0, "(max-resolution: 1.5dppx)"));
        assert!(at(2.0, "screen and (min-width: 600px) and (min-resolution: 2x)") && !at(1.0, "(min-resolution: 2x) and (min-width: 600px)"));
        assert!(!at(2.0, "(min-resolution: fast)") && !at(2.0, "(min-resolution: 2furlongs)"));
        let p = "<picture><source media='(min-resolution: 2dppx)' srcset='dense.png'><img src='plain.png'></picture>";
        let t = html::parse(p);
        let n = (0..t.nodes.len()).find(|&n| t.tag(n) == Some("img")).unwrap();
        assert_eq!(image_source(&t, n), "plain.png");
        assert_eq!(with_window_dppx(960.0, 2.0, || image_source(&t, n)), "dense.png");
    }

    #[test]
    fn width_descriptors_use_sizes_or_the_window() {
        let set = "s.jpg 400w, m.jpg 800w, l.jpg 1600w";
        assert_eq!(pick_srcset(set, "").as_deref(), Some("l.jpg"), "960 px window: 1600w is the first at least 1x");
        assert_eq!(pick_srcset(set, "100vw").as_deref(), Some("l.jpg"));
        assert_eq!(pick_srcset(set, "(max-width: 1200px) 400px, 800px").as_deref(), Some("s.jpg"));
        assert_eq!(pick_srcset(set, "(max-width: 600px) 400px, 800px").as_deref(), Some("m.jpg"), "first entry fails, the default 800px holds");
        assert_eq!(pick_srcset(set, "calc(100vw - 2em)").as_deref(), Some("l.jpg"));
        assert_eq!(pick_srcset("only.jpg 100w", "800px").as_deref(), Some("only.jpg"));
    }

    #[test]
    fn data_addresses_with_commas_stay_in_one_piece() {
        let c = candidates("data:image/png;base64,AAAA 1x, b.png 2x");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "data:image/png;base64,AAAA");
        assert_eq!(pick_srcset("a.png,b.png 2x", "").as_deref(), Some("a.png,b.png"), "a comma inside a word is part of the address");
        assert_eq!(pick_srcset("a.png, b.png 2x", "").as_deref(), Some("a.png"), "a comma after the address ends the candidate");
    }

    #[test]
    fn media_conditions() {
        assert!(media_matches("") && media_matches("screen") && media_matches("(min-width: 600px)") && media_matches("(max-width: 1200px)"));
        assert!(!media_matches("(min-width: 1200px)") && !media_matches("(max-width: 600px)") && !media_matches("print"));
        assert!(media_matches("(min-width: 600px) and (max-width: 1200px)") && !media_matches("(min-width: 600px) and (max-width: 800px)"));
        assert!(!media_matches("(prefers-color-scheme: dark)") && !media_matches("(orientation: portrait)"));
        assert!(media_matches("screen and (min-width: 40em)"));
    }

    #[test]
    fn a_picture_takes_the_first_source_the_reader_can_draw() {
        let p = "<picture><source type='image/avif' srcset='a.avif'><source media='(max-width: 500px)' srcset='small.png'><source type='image/webp' srcset='x.webp 1x, y.webp 2x'><img src='fallback.jpg'></picture>";
        assert_eq!(src_of(p), vec!["x.webp"], "AVIF is not drawn, the narrow window does not match, WebP does");
        let p = "<picture><source type='image/avif' srcset='a.avif'><img src='fallback.jpg' srcset='f1.jpg 1x, f2.jpg 2x'></picture>";
        assert_eq!(src_of(p), vec!["f1.jpg"], "no usable source: the img's own srcset, then src");
        let p = "<picture><img src='only.png'></picture><picture><source srcset='after.png'><img src='before.png'></picture>";
        assert_eq!(src_of(p), vec!["only.png", "after.png"]);
        let p = "<div><source srcset='stray.png'><img src='plain.png'></div>";
        assert_eq!(src_of(p), vec!["plain.png"], "a <source> outside a <picture> means nothing");
    }

    #[test]
    fn src_data_src_and_srcset_fall_back_in_order() {
        assert_eq!(src_of("<img src=a.png><img data-src=b.png><img src='' data-src=c.png><img srcset='d.png 2x'><img>"), vec!["a.png", "b.png", "c.png", "d.png", ""]);
        assert_eq!(src_of("<img src=a.png srcset='b1.png 1x, b2.png 2x'>"), vec!["b1.png"], "srcset wins over src");
    }

    #[test]
    fn lazy_is_read_from_loading() {
        let t = html::parse("<img src=a loading=lazy><img src=b loading=EAGER><img src=c LOADING=' Lazy '><img src=d>");
        let v: Vec<bool> = (0..t.nodes.len()).filter(|&n| t.tag(n) == Some("img")).map(|n| is_lazy(&t, n)).collect();
        assert_eq!(v, vec![true, false, true, false]);
    }

    #[test]
    fn the_choice_follows_the_window_it_is_made_for_and_goes_back_after() {
        let set = "s.jpg 400w, m.jpg 800w, l.jpg 1600w";
        assert_eq!(with_window(1280.0, || pick_srcset(set, "100vw")).as_deref(), Some("l.jpg"));
        assert_eq!(with_window(360.0, || pick_srcset(set, "100vw")).as_deref(), Some("s.jpg"), "a phone-sized window takes the small one");
        assert_eq!(pick_srcset(set, "100vw"), pick_srcset(set, ""), "outside the call the nominal window is back");
        assert!(with_window(600.0, || media_matches("(max-width: 700px)")) && !with_window(1000.0, || media_matches("(max-width: 700px)")));
        assert!(with_window(f32::NAN, || media_matches("(min-width: 900px)")), "a bad width means the nominal one");
        // nested calls restore what was there
        with_window(500.0, || {
            with_window(900.0, || ());
            assert!(media_matches("(max-width: 520px)"));
        });
    }
}
