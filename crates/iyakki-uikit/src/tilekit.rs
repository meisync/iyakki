//! Small pieces the list-and-detail tiles (K8S, GIT) share: a job on a thread whose answer is taken once ([`Job`]), a scrolling text ([`TextView`] of
//! [`Line`]s) and the selection and scroll state of a list ([`ListState`]). Moved out of the K8S tile (alpha.8 N1) so a tile crate can use them.

use std::sync::{Arc, Mutex};

use crate::safeclamp::SafeClamp;
use crate::ui::R;

pub use tiny_skia::Pixmap;

// ------------------------------------------------------------------------------------------------ jobs

pub struct Job<T>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Job<T> {
    pub fn spawn(f: impl FnOnce() -> T + Send + 'static) -> Job<T> {
        let slot = Arc::new(Mutex::new(None));
        let s2 = slot.clone();
        std::thread::spawn(move || {
            let v = f();
            *s2.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        });
        Job(slot)
    }
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

// ------------------------------------------------------------------------------------------------ text view

/// One line of a scrolling text: the text, the colour of its value part, and how many characters at the start are a key.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub color: [u8; 4],
    pub key_len: usize,
}

impl Line {
    pub fn plain(text: impl Into<String>, color: [u8; 4]) -> Line {
        Line { text: text.into(), color, key_len: 0 }
    }
}

#[derive(Default)]
pub struct TextView {
    pub lines: Vec<Line>,
    /// The first line in view.
    pub top: usize,
    pub wrap: bool,
    /// Keep the end in view (follow mode).
    pub stick: bool,
    /// Lines that fit, as drawn last.
    pub fit: usize,
    /// The area as drawn last.
    pub rect: R,
}

impl TextView {
    pub fn set(&mut self, lines: Vec<Line>, keep_top: bool) {
        if !keep_top {
            self.top = 0;
        }
        self.lines = lines;
        self.top = self.top.min(self.lines.len().saturating_sub(1));
    }
    pub fn scroll(&mut self, rows: i32) {
        let max = self.lines.len().saturating_sub(self.fit.max(1)) as i32;
        let cur = if self.stick { max } else { self.top as i32 };
        let now = (cur + rows).clamp_safe(0, max.max(0));
        self.stick = false;
        self.top = now as usize;
        if now >= max && max > 0 && rows > 0 {
            // scrolling down to the end re-attaches follow mode (the log view uses this)
            self.stick = true;
        }
    }
    pub fn home(&mut self) {
        self.stick = false;
        self.top = 0;
    }
    pub fn end(&mut self) {
        self.stick = true;
        self.top = self.lines.len().saturating_sub(self.fit.max(1));
    }
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

// ------------------------------------------------------------------------------------------------ lists

#[derive(Clone, Copy, Debug, Default)]
pub struct ListState {
    pub top: usize,
    pub sel: usize,
    /// The selection moved by key: show it at the next draw.
    pub reveal: bool,
    /// Rows that fit, as drawn last.
    pub fit: usize,
    pub rect: R,
}

impl ListState {
    pub fn up(&mut self, n: usize, by: usize) {
        self.sel = self.sel.saturating_sub(by).min(n.saturating_sub(1));
        self.reveal = true;
    }
    pub fn down(&mut self, n: usize, by: usize) {
        self.sel = (self.sel + by).min(n.saturating_sub(1));
        self.reveal = true;
    }
    pub fn page(&self) -> usize {
        self.fit.saturating_sub(1).max(1)
    }
    pub fn scroll(&mut self, n: usize, rows: i32) {
        let max = n.saturating_sub(self.fit.max(1)) as i32;
        self.top = (self.top as i32 + rows).clamp_safe(0, max.max(0)) as usize;
        self.reveal = false;
    }
    pub fn reset(&mut self) {
        self.top = 0;
        self.sel = 0;
        self.reveal = true;
    }
}


/// Draw `pm` inside `r`, centred, as large as fits with the proportions kept. Returns the rectangle it took.
pub fn draw_fit(g: &mut crate::gfx::Gfx, pm: &Pixmap, r: R) -> R {
    let k = (r.w / pm.width() as f32).min(r.h / pm.height() as f32).max(0.01);
    let (dw, dh) = (pm.width() as f32 * k, pm.height() as f32 * k);
    let (dx, dy) = (r.x + (r.w - dw) / 2.0, r.y + (r.h - dh) / 2.0);
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    g.pm_mut().draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_translate(dx, dy).pre_scale(k, k), None);
    R::new(dx, dy, dw, dh)
}

// ------------------------------------------------------------------------------------------------ pictures

/// Averages blocks of pixels down to `nw` x `nh` (the pixmap is premultiplied, so averaging is right).
pub fn resize_box(pm: &Pixmap, nw: u32, nh: u32) -> Option<Pixmap> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut out = Pixmap::new(nw, nh)?;
    let src = pm.data();
    let (sx, sy) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let dst = out.data_mut();
    for y in 0..nh as usize {
        let (y0, y1) = ((y as f32 * sy) as usize, (((y + 1) as f32 * sy).ceil() as usize).clamp_safe(1, h));
        for x in 0..nw as usize {
            let (x0, x1) = ((x as f32 * sx) as usize, (((x + 1) as f32 * sx).ceil() as usize).clamp_safe(1, w));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for yy in y0..y1.max(y0 + 1).min(h) {
                for xx in x0..x1.max(x0 + 1).min(w) {
                    let o = (yy * w + xx) * 4;
                    for c in 0..4 {
                        acc[c] += src[o + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = (y * nw as usize + x) * 4;
            for c in 0..4 {
                dst[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    Some(out)
}

/// A file name that is safe to create: one path component, no leading dot, with the right extension.
pub fn clean_name(name: &str, ext: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = base.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '(' | ')')).collect();
    n = n.trim().trim_start_matches('.').to_string();
    n.truncate(100);
    let stem_ok = !n.is_empty();
    let has_ext = n.to_ascii_lowercase().ends_with(&format!(".{ext}")) || (ext == "jpg" && n.to_ascii_lowercase().ends_with(".jpeg"));
    if !stem_ok {
        return format!("picture.{ext}");
    }
    if has_ext {
        n
    } else {
        format!("{n}.{ext}")
    }
}

/// `950`, `12.3k`, `4.5M`.
pub fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        if k < 10.0 { format!("{k:.1}k") } else { format!("{:.0}k", k) }
    } else if n < 1_000_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else {
        format!("{:.1}B", n as f64 / 1e9)
    }
}
