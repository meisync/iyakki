//! Reference renderer: a display list to an RGBA image, with the tile font table
//! (real DejaVu shapes at the ABI's fixed advance, see `font.rs`; a character outside ASCII and Latin-1 draws as `?`).
//! It exists so tests can assert pixels. It is not the production renderer (that is `iyakki-ui`).
use iyakki_tile_abi::{Color, DisplayList, Op, Rect};


/// Most pictures, most bytes of decoded pixels and the longest side of one picture a tile's table holds.
pub const TABLE_PICTURES: usize = 64;
pub const TABLE_BYTES: usize = 4 << 20;
pub const TABLE_SIDE: i32 = 1024;

/// A decoded picture a tile may draw by handle: `w` x `h` pixels, 4 bytes each (red, green, blue, alpha; straight alpha).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub w: i32,
    pub h: i32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    BadSize,
    TooBig,
    TableFull,
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TableError::BadSize => write!(f, "the picture's size and its pixels do not match"),
            TableError::TooBig => write!(f, "the picture is larger than {TABLE_SIDE} x {TABLE_SIDE} pixels"),
            TableError::TableFull => write!(f, "the tile's picture table is full ({TABLE_PICTURES} pictures or {} MB)", TABLE_BYTES >> 20),
        }
    }
}

/// The pictures one tile may draw, by handle. The host fills it (from the pictures the tile's bundle declares, decoded when the tile
/// is loaded); a tile only names a handle in an `Image` operation. Size-capped per tile; a handle that is not in the table draws as a
/// grey box, as before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageTable {
    pics: std::collections::BTreeMap<u32, Picture>,
    bytes: usize,
}

impl ImageTable {
    pub fn new() -> ImageTable {
        ImageTable::default()
    }

    /// Puts a picture under `handle` (replacing what was there).
    pub fn put(&mut self, handle: u32, w: i32, h: i32, rgba: Vec<u8>) -> Result<(), TableError> {
        if w <= 0 || h <= 0 || rgba.len() != (w as usize) * (h as usize) * 4 {
            return Err(TableError::BadSize);
        }
        if w > TABLE_SIDE || h > TABLE_SIDE {
            return Err(TableError::TooBig);
        }
        let old = self.pics.get(&handle).map_or(0, |p| p.rgba.len());
        if (self.pics.len() >= TABLE_PICTURES && !self.pics.contains_key(&handle)) || self.bytes - old + rgba.len() > TABLE_BYTES {
            return Err(TableError::TableFull);
        }
        self.bytes = self.bytes - old + rgba.len();
        self.pics.insert(handle, Picture { w, h, rgba });
        Ok(())
    }

    pub fn get(&self, handle: u32) -> Option<&Picture> {
        self.pics.get(&handle)
    }

    pub fn remove(&mut self, handle: u32) -> bool {
        match self.pics.remove(&handle) {
            Some(p) => {
                self.bytes -= p.rgba.len();
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.pics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pics.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Picture)> {
        self.pics.iter().map(|(h, p)| (*h, p))
    }
}


/// An RGBA image, row by row, 4 bytes per pixel, fully opaque after drawing on an opaque background.
pub struct Image {
    pub w: i32,
    pub h: i32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(w: i32, h: i32, bg: Color) -> Image {
        let n = (w.max(0) as usize) * (h.max(0) as usize);
        let mut rgba = Vec::with_capacity(n * 4);
        for _ in 0..n {
            rgba.extend_from_slice(&[bg.0, bg.1, bg.2, bg.3]);
        }
        Image { w, h, rgba }
    }
    pub fn pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return Color(0, 0, 0, 0);
        }
        let i = ((y * self.w + x) * 4) as usize;
        Color(self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3])
    }
    fn blend(&mut self, x: i32, y: i32, c: Color) {
        let i = ((y * self.w + x) * 4) as usize;
        let a = c.3 as u32;
        for (k, s) in [c.0, c.1, c.2].into_iter().enumerate() {
            self.rgba[i + k] = ((s as u32 * a + self.rgba[i + k] as u32 * (255 - a) + 127) / 255) as u8;
        }
        self.rgba[i + 3] = 255;
    }
}

struct Pen<'a> {
    img: &'a mut Image,
    clip: Rect,
}

impl Pen<'_> {
    fn put(&mut self, x: i32, y: i32, c: Color) {
        if self.clip.contains(x, y) {
            self.img.blend(x, y, c);
        }
    }
    fn fill(&mut self, r: Rect, c: Color) {
        let v = r.intersect(&self.clip);
        for y in v.y..v.y + v.h {
            for x in v.x..v.x + v.w {
                self.img.blend(x, y, c);
            }
        }
    }
}

/// Draw `list` on a `w` x `h` surface cleared to `bg`; pictures are grey boxes (no table).
pub fn render(list: &DisplayList, w: i32, h: i32, bg: Color) -> Image {
    render_with(list, w, h, bg, &ImageTable::default())
}

/// Like [`render`], with the pictures a tile may draw by handle. A picture is scaled to its rectangle by nearest neighbour in whole
/// numbers (the browser renderer does the same sums, so both draw the same pixels) and laid over what is there by its alpha.
pub fn render_with(list: &DisplayList, w: i32, h: i32, bg: Color, images: &ImageTable) -> Image {
    let mut img = Image::new(w, h, bg);
    let surface = Rect::new(0, 0, w, h);
    let mut clips = vec![surface];
    for op in &list.ops {
        let clip = *clips.last().unwrap_or(&surface);
        let mut pen = Pen { img: &mut img, clip };
        match op {
            Op::Rect { r, color } => pen.fill(*r, *color),
            Op::RoundRect { r, radius, color } => {
                let rad = (*radius).min(r.w / 2).min(r.h / 2).max(0);
                for ly in 0..r.h {
                    for lx in 0..r.w {
                        // doubled coordinates of the pixel centre relative to the nearest corner centre
                        let cx = if lx < rad { 2 * rad - (2 * lx + 1) } else if lx >= r.w - rad { (2 * lx + 1) - 2 * (r.w - rad) } else { 0 };
                        let cy = if ly < rad { 2 * rad - (2 * ly + 1) } else if ly >= r.h - rad { (2 * ly + 1) - 2 * (r.h - rad) } else { 0 };
                        if (cx as i64).pow(2) + (cy as i64).pow(2) <= 4 * (rad as i64).pow(2) {
                            pen.put(r.x + lx, r.y + ly, *color);
                        }
                    }
                }
            }
            Op::Outline { r, thickness, color } => {
                let t = (*thickness).clamp(0, r.w.min(r.h));
                pen.fill(Rect::new(r.x, r.y, r.w, t), *color);
                pen.fill(Rect::new(r.x, r.y + r.h - t, r.w, t), *color);
                pen.fill(Rect::new(r.x, r.y + t, t, r.h - 2 * t), *color);
                pen.fill(Rect::new(r.x + r.w - t, r.y + t, t, r.h - 2 * t), *color);
            }
            Op::Line { x1, y1, x2, y2, thickness, color } => {
                let t = (*thickness).max(1);
                let (dx, dy) = (x2 - x1, y2 - y1);
                let n = dx.abs().max(dy.abs()).max(1);
                for i in 0..=n {
                    let x = x1 + (dx as i64 * i as i64 / n as i64) as i32;
                    let y = y1 + (dy as i64 * i as i64 / n as i64) as i32;
                    pen.fill(Rect::new(x - t / 2, y - t / 2, t, t), *color);
                }
            }
            Op::Text { x, y, face, size, color, text } => {
                let adv = (iyakki_tile_abi::measure_text(*face, *size, "a")).max(1);
                let sz = *size as i32;
                if let Some(cell) = crate::font::cell(*face, *size) {
                    for (n, ch) in text.chars().enumerate() {
                        let gx = x + n as i32 * adv;
                        for dy in 0..sz {
                            for dx in 0..adv {
                                if crate::font::pixel(&cell, ch, dx, dy, adv, sz) {
                                    pen.put(gx + dx, y + dy, *color);
                                }
                            }
                        }
                    }
                }
            }
            Op::Image { handle, r } => match images.get(*handle) {
                Some(p) if r.w > 0 && r.h > 0 => {
                    let v = r.intersect(&clip);
                    for y in v.y..v.y + v.h {
                        let sy = ((y - r.y) as i64 * p.h as i64 / r.h as i64) as usize;
                        for x in v.x..v.x + v.w {
                            let sx = ((x - r.x) as i64 * p.w as i64 / r.w as i64) as usize;
                            let i = (sy * p.w as usize + sx) * 4;
                            if p.rgba[i + 3] > 0 {
                                pen.img.blend(x, y, Color(p.rgba[i], p.rgba[i + 1], p.rgba[i + 2], p.rgba[i + 3]));
                            }
                        }
                    }
                }
                _ => pen.fill(*r, Color(128, 128, 128, 255)),
            },
            Op::PushClip { r } => {
                let c = r.intersect(&clip);
                clips.push(c);
            }
            Op::PopClip => {
                if clips.len() > 1 {
                    clips.pop();
                }
            }
            Op::Hit { .. } | Op::Access { .. } => {}
        }
    }
    img
}
