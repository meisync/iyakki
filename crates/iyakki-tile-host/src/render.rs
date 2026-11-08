//! Reference renderer: a display list to an RGBA image, with a tiny built-in 5x7 bitmap font
//! (upper case, digits and punctuation; lower case draws as upper case, anything else as `?`).
//! It exists so tests can assert pixels. It is not the production renderer (that is `iyakki-ui`).
use iyakki_tile_abi::{Color, DisplayList, Op, Rect};

/// Columns of the classic 5x7 font for 0x20..=0x5A; bit 0 is the top row.
const FONT: [[u8; 5]; 59] = [
    [0x00, 0x00, 0x00, 0x00, 0x00], [0x00, 0x00, 0x5F, 0x00, 0x00], [0x00, 0x07, 0x00, 0x07, 0x00],
    [0x14, 0x7F, 0x14, 0x7F, 0x14], [0x24, 0x2A, 0x7F, 0x2A, 0x12], [0x23, 0x13, 0x08, 0x64, 0x62],
    [0x36, 0x49, 0x55, 0x22, 0x50], [0x00, 0x05, 0x03, 0x00, 0x00], [0x00, 0x1C, 0x22, 0x41, 0x00],
    [0x00, 0x41, 0x22, 0x1C, 0x00], [0x14, 0x08, 0x3E, 0x08, 0x14], [0x08, 0x08, 0x3E, 0x08, 0x08],
    [0x00, 0x50, 0x30, 0x00, 0x00], [0x08, 0x08, 0x08, 0x08, 0x08], [0x00, 0x60, 0x60, 0x00, 0x00],
    [0x20, 0x10, 0x08, 0x04, 0x02], [0x3E, 0x51, 0x49, 0x45, 0x3E], [0x00, 0x42, 0x7F, 0x40, 0x00],
    [0x42, 0x61, 0x51, 0x49, 0x46], [0x21, 0x41, 0x45, 0x4B, 0x31], [0x18, 0x14, 0x12, 0x7F, 0x10],
    [0x27, 0x45, 0x45, 0x45, 0x39], [0x3C, 0x4A, 0x49, 0x49, 0x30], [0x01, 0x71, 0x09, 0x05, 0x03],
    [0x36, 0x49, 0x49, 0x49, 0x36], [0x06, 0x49, 0x49, 0x29, 0x1E], [0x00, 0x36, 0x36, 0x00, 0x00],
    [0x00, 0x56, 0x36, 0x00, 0x00], [0x08, 0x14, 0x22, 0x41, 0x00], [0x14, 0x14, 0x14, 0x14, 0x14],
    [0x00, 0x41, 0x22, 0x14, 0x08], [0x02, 0x01, 0x51, 0x09, 0x06], [0x32, 0x49, 0x79, 0x41, 0x3E],
    [0x7E, 0x11, 0x11, 0x11, 0x7E], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 0x09, 0x09, 0x09, 0x01],
    [0x3E, 0x41, 0x49, 0x49, 0x7A], [0x7F, 0x08, 0x08, 0x08, 0x7F], [0x00, 0x41, 0x7F, 0x41, 0x00],
    [0x20, 0x40, 0x41, 0x3F, 0x01], [0x7F, 0x08, 0x14, 0x22, 0x41], [0x7F, 0x40, 0x40, 0x40, 0x40],
    [0x7F, 0x02, 0x0C, 0x02, 0x7F], [0x7F, 0x04, 0x08, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 0x09, 0x19, 0x29, 0x46],
    [0x46, 0x49, 0x49, 0x49, 0x31], [0x01, 0x01, 0x7F, 0x01, 0x01], [0x3F, 0x40, 0x40, 0x40, 0x3F],
    [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x3F, 0x40, 0x38, 0x40, 0x3F], [0x63, 0x14, 0x08, 0x14, 0x63],
    [0x07, 0x08, 0x70, 0x08, 0x07], [0x61, 0x51, 0x49, 0x45, 0x43],
];

fn glyph(c: char) -> &'static [u8; 5] {
    let u = c.to_ascii_uppercase();
    let i = (u as u32).wrapping_sub(0x20) as usize;
    FONT.get(i).unwrap_or(&FONT[(b'?' - 0x20) as usize])
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

/// Draw `list` on a `w` x `h` surface cleared to `bg`.
pub fn render(list: &DisplayList, w: i32, h: i32, bg: Color) -> Image {
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
                for (n, ch) in text.chars().enumerate() {
                    let g = glyph(ch);
                    let gx = x + n as i32 * adv;
                    for dy in 0..sz {
                        let row = dy * 7 / sz;
                        for dx in 0..adv {
                            let col = dx * 6 / adv;
                            if col < 5 && (g[col as usize] >> row) & 1 == 1 {
                                pen.put(gx + dx, y + dy, *color);
                            }
                        }
                    }
                }
            }
            Op::Image { r, .. } => {
                pen.fill(*r, Color(128, 128, 128, 255));
            }
            Op::PushClip { r } => {
                let c = r.intersect(&clip);
                clips.push(c);
            }
            Op::PopClip => {
                if clips.len() > 1 {
                    clips.pop();
                }
            }
            Op::Hit { .. } => {}
        }
    }
    img
}
