//! The `Canvas` trait: every call an app makes to draw. Two implementations here: [`crate::raster::RasterCanvas`] (pixels, through the
//! toolkit's `Gfx`: the device and the desktop window) and [`crate::list::ListCanvas`] (records a [`crate::list::DisplayList`]: a browser,
//! a client, a test, a WASM tile). An app written against this trait runs on both without a change.

use crate::geom::{Color, Rect};
use crate::path::{self, Seg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    Regular,
    Bold,
    Mono,
}

/// What a clickable region is, for the screen reader and for a client that exposes ARIA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Button,
    Link,
    ListItem,
    Tab,
    Field,
    Text,
    Image,
}

impl Role {
    pub const ALL: [Role; 7] = [Role::Button, Role::Link, Role::ListItem, Role::Tab, Role::Field, Role::Text, Role::Image];

    pub fn key(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Link => "link",
            Role::ListItem => "item",
            Role::Tab => "tab",
            Role::Field => "field",
            Role::Text => "text",
            Role::Image => "image",
        }
    }

    pub fn from_key(k: &str) -> Option<Role> {
        Role::ALL.iter().copied().find(|r| r.key() == k)
    }
}

/// A clickable region: what the app is told when it is used (`UiEvent::Action(action)`), its name and its role.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub action: u32,
    pub name: String,
    pub role: Role,
    /// What the region is now for a screen reader: the `Hit::SELECTED` ... `Hit::SECRET` bits (same values as the tile ABI's access flags).
    /// Not part of the text form of a display list; it travels with the list in the process and into the tile ABI's `Access` operation.
    pub state: u8,
    /// The text a field holds (cut by the tile ABI); empty for anything else. A `Hit::SECRET` field has none, only a length is said.
    pub value: String,
}

impl Default for Hit {
    fn default() -> Hit {
        Hit { action: 0, name: String::new(), role: Role::Text, state: 0, value: String::new() }
    }
}

impl Hit {
    pub const SELECTED: u8 = 1;
    pub const CHECKED: u8 = 2;
    pub const DISABLED: u8 = 4;
    pub const EXPANDED: u8 = 8;
    pub const SECRET: u8 = 16;

    /// A region with no state: what every `Hit` was before state existed.
    pub fn new(action: u32, name: impl Into<String>, role: Role) -> Hit {
        Hit { action, name: name.into(), role, state: 0, value: String::new() }
    }

    pub fn with_state(mut self, state: u8) -> Hit {
        self.state = state & 31;
        self
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Hit {
        self.value = value.into();
        self
    }
}

/// How the bytes of an [`Image`] are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// 8-bit red, green, blue, alpha, row after row (`width * height * 4` bytes).
    Rgba,
    /// One complete JPEG file.
    Jpeg,
}

impl ImageFormat {
    pub fn key(self) -> &'static str {
        match self {
            ImageFormat::Rgba => "rgba",
            ImageFormat::Jpeg => "jpeg",
        }
    }
    pub fn from_key(k: &str) -> Option<ImageFormat> {
        [ImageFormat::Rgba, ImageFormat::Jpeg].into_iter().find(|f| f.key() == k)
    }
}

/// The most bytes one image carries, and the largest picture in pixels each way. A larger one is refused by the list reader.
pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 4096;

/// A picture an app got from the core (a camera frame, a video frame). `seq` grows with every new picture of the same source, so a
/// renderer that keeps the last decoded one can tell a new picture from the same one drawn again.
#[derive(Clone, PartialEq)]
pub struct Image {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub data: std::sync::Arc<Vec<u8>>,
}

impl std::fmt::Debug for Image {
    // short on purpose: a picture's bytes never go into a log or a test failure (and the engine module stays small)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Image")
    }
}

impl Image {
    /// Whether the picture is well formed and within the limits (the reader and the canvases refuse what is not).
    pub fn check(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > MAX_IMAGE_SIDE || self.height > MAX_IMAGE_SIDE {
            return Err("an image is 1 to 4096 pixels each way".into());
        }
        if self.data.len() > MAX_IMAGE_BYTES {
            return Err("an image is at most 1024 KB".into());
        }
        match self.format {
            ImageFormat::Rgba if self.data.len() != self.width as usize * self.height as usize * 4 => Err("an RGBA image has width x height x 4 bytes".into()),
            ImageFormat::Jpeg if !self.data.starts_with(&[0xFF, 0xD8]) => Err("a JPEG image starts with FF D8".into()),
            _ => Ok(()),
        }
    }
}

pub trait Canvas {
    /// The drawing area in logical units. Lay the screen out from this, never from a constant.
    fn size(&self) -> (f32, f32);
    fn rect(&mut self, r: Rect, c: Color);
    fn rrect(&mut self, r: Rect, radius: f32, c: Color);
    fn outline(&mut self, r: Rect, radius: f32, width: f32, c: Color);
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color);
    /// A vertical gradient from `top` to `bottom`.
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color);
    /// A soft glow around a rounded rectangle (a shadow when the colour is dark).
    fn shadow(&mut self, r: Rect, radius: f32, spread: f32, c: Color);
    /// Text with its top-left at (x, y); returns the advance width.
    fn text(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle) -> f32;
    fn measure(&mut self, size: f32, s: &str, style: TextStyle) -> f32;
    /// Everything `f` draws or registers is cut to `r`.
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn Canvas));
    /// A picture stretched to `r` (keep the aspect yourself: see [`contain`]). A canvas that cannot draw it draws nothing; the default does.
    fn image(&mut self, _r: Rect, _img: &Image) {}
    /// How text is set from now on (family, letter spacing, tabular figures). The host sets it once a frame from the person's choices;
    /// a canvas that cannot honour part of it (the pixel canvas draws the person's family by itself) ignores that part.
    fn set_type(&mut self, _o: crate::typeface::TextOpts) {}
    /// The type options now in force (so a replayed run can put them back).
    fn type_opts(&self) -> crate::typeface::TextOpts {
        crate::typeface::TextOpts::default()
    }
    /// Blur what is already drawn inside `r` (the back of a glass surface). A canvas without blur ignores it.
    fn blur(&mut self, _r: Rect, _radius: f32) {}
    /// A shape of straight and curved segments, filled (non-zero winding) and/or stroked. The default draws the outline with `line` and
    /// the fill as thin horizontal bands, so a canvas that does not know paths still shows the shape; real canvases override it.
    fn path(&mut self, fill: Option<Color>, stroke: Option<(Color, f32)>, segs: &[Seg]) {
        let polys = path::flatten(segs, 0.25);
        if let (Some(c), Some((x0, y0, x1, y1))) = (fill, path::bounds(&polys, 0.0)) {
            let (px, py) = (x0.floor() as i32, y0.floor() as i32);
            let (w, h) = (((x1.ceil() as i32 - px).max(0) as usize).min(4096), ((y1.ceil() as i32 - py).max(0) as usize).min(4096));
            let mask = path::fill_mask(&polys, px, py, w, h);
            for row in 0..h {
                let mut x = 0;
                while x < w {
                    if mask[row * w + x] >= 0.5 {
                        let s = x;
                        while x < w && mask[row * w + x] >= 0.5 {
                            x += 1;
                        }
                        self.rect(Rect::new((px + s as i32) as f32, (py + row as i32) as f32, (x - s) as f32, 1.0), c);
                    } else {
                        x += 1;
                    }
                }
            }
        }
        if let Some((c, wd)) = stroke {
            for p in &polys {
                let n = p.pts.len();
                for i in 0..if p.closed { n } else { n.saturating_sub(1) } {
                    let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
                    self.line(a.0, a.1, b.0, b.1, wd, c);
                }
            }
        }
    }
    /// Everything `f` draws is drawn at `alpha` (0 to 1) of its own opacity. The pixel canvas and the recording canvas scale every
    /// colour's alpha as it is drawn ([`AlphaCanvas`]); overlapping shapes of one group each fade, as a browser's `globalAlpha` does.
    fn layer(&mut self, alpha: f32, f: &mut dyn FnMut(&mut dyn Canvas));
    /// A run whose clusters have the given advances (one per cluster, see [`path::clusters`]). A canvas that shapes text itself draws the
    /// run whole (the default); a recording canvas keeps the advances so a renderer without the font can place each cluster.
    #[allow(clippy::too_many_arguments)]
    fn text_clusters(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle, _advances: &[f32]) -> f32 {
        self.text(x, y, size, c, s, style)
    }
    /// A clickable region (also the accessibility tree: every control has a name and a role).
    fn hit(&mut self, r: Rect, hit: Hit);
}

/// The largest rectangle of the aspect `w` by `h` that fits in `r`, centred (letterboxed).
pub fn contain(r: Rect, w: u32, h: u32) -> Rect {
    if w == 0 || h == 0 || r.w <= 0.0 || r.h <= 0.0 {
        return Rect::new(r.x, r.y, 0.0, 0.0);
    }
    let k = (r.w / w as f32).min(r.h / h as f32);
    let (iw, ih) = (w as f32 * k, h as f32 * k);
    Rect::new(r.x + (r.w - iw) / 2.0, r.y + (r.h - ih) / 2.0, iw, ih)
}

/// `s` shortened with an ellipsis so that it is at most `max_w` wide.
pub fn fit(c: &mut dyn Canvas, size: f32, s: &str, style: TextStyle, max_w: f32) -> String {
    if c.measure(size, s, style) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let t: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
        if c.measure(size, &t, style) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "\u{2026}"
}

fn faded(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], (c[3] as f32 * a).round() as u8]
}

/// A canvas that draws on another with every colour's alpha scaled: what [`Canvas::layer`] uses by default.
pub struct AlphaCanvas<'a> {
    pub inner: &'a mut dyn Canvas,
    pub alpha: f32,
}

impl Canvas for AlphaCanvas<'_> {
    fn size(&self) -> (f32, f32) {
        self.inner.size()
    }
    fn rect(&mut self, r: Rect, c: Color) {
        self.inner.rect(r, faded(c, self.alpha));
    }
    fn rrect(&mut self, r: Rect, radius: f32, c: Color) {
        self.inner.rrect(r, radius, faded(c, self.alpha));
    }
    fn outline(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        self.inner.outline(r, radius, width, faded(c, self.alpha));
    }
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        self.inner.line(x0, y0, x1, y1, width, faded(c, self.alpha));
    }
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.inner.gradient(r, faded(top, self.alpha), faded(bottom, self.alpha));
    }
    fn shadow(&mut self, r: Rect, radius: f32, spread: f32, c: Color) {
        self.inner.shadow(r, radius, spread, faded(c, self.alpha));
    }
    fn text(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle) -> f32 {
        self.inner.text(x, y, size, faded(c, self.alpha), s, style)
    }
    fn measure(&mut self, size: f32, s: &str, style: TextStyle) -> f32 {
        self.inner.measure(size, s, style)
    }
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn Canvas)) {
        let a = self.alpha;
        self.inner.clipped(r, &mut |cc| f(&mut AlphaCanvas { inner: cc, alpha: a }));
    }
    fn image(&mut self, r: Rect, img: &Image) {
        // a picture is drawn whole: the canvas trait has no picture opacity
        self.inner.image(r, img);
    }
    fn set_type(&mut self, o: crate::typeface::TextOpts) {
        self.inner.set_type(o);
    }
    fn type_opts(&self) -> crate::typeface::TextOpts {
        self.inner.type_opts()
    }
    fn blur(&mut self, r: Rect, radius: f32) {
        self.inner.blur(r, radius);
    }
    fn path(&mut self, fill: Option<Color>, stroke: Option<(Color, f32)>, segs: &[Seg]) {
        self.inner.path(fill.map(|c| faded(c, self.alpha)), stroke.map(|(c, w)| (faded(c, self.alpha), w)), segs);
    }
    fn layer(&mut self, alpha: f32, f: &mut dyn FnMut(&mut dyn Canvas)) {
        let a = self.alpha * alpha.clamp(0.0, 1.0);
        self.inner.layer(a, f);
    }
    fn text_clusters(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle, advances: &[f32]) -> f32 {
        self.inner.text_clusters(x, y, size, faded(c, self.alpha), s, style, advances)
    }
    fn hit(&mut self, r: Rect, hit: Hit) {
        self.inner.hit(r, hit);
    }
}
