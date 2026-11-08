//! The size of the installer's text on the console.
//!
//! The boot-time installer draws on the kernel's text console (`/dev/tty0`). Its built-in font is 8x16 pixels, which is tiny on
//! a high-resolution laptop screen (a 2560x1600 panel shows 320 columns). This module picks a scale from the screen's resolution
//! ([`scale_for_screen`], a pure function), enlarges the console font by whole pixels ([`scale_glyphs`]: every font pixel
//! becomes a block of `scale` x `scale` pixels, so no font file is needed) and loads it with the console's font ioctl
//! (`KDFONTOP`). The person can change it with `+` and `-` while the installer runs, or fix it from the boot menu with
//! `iyakki.ui_scale=N`. The kernel's console fonts are at most 32 pixels wide and tall, so the 8x16 font can be doubled and
//! no more; a larger scale would need the installer to be drawn on the framebuffer instead (see docs/INSTALLER.md).
//!
//! Everything that decides is pure and tested; only [`Zoom`] talks to the console, and it does nothing on a terminal that is
//! not a Linux virtual console (a desktop's terminal window has its own zoom).

/// The console's biggest glyph, in pixels, both ways (a kernel limit).
pub const MAX_GLYPH: u32 = 32;
/// The scale levels the person can choose with `+` and `-`.
pub const LEVELS: [u32; 4] = [1, 2, 3, 4];

/// The scale for a screen of `w` x `h` pixels: 1 below 1900 x 1150, 2 from there (a 1920x1080 panel already gets it: its 240
/// columns of 8 pixels are far too small to read across a room), 3 from 3600 x 2000 (a 4K panel; used only when the font allows).
/// An unknown size (0) leaves the console as it is.
pub fn scale_for_screen(w: u32, h: u32) -> u32 {
    if w == 0 || h == 0 {
        1
    } else if w >= 3600 || h >= 2000 {
        3
    } else if w >= 1900 || h >= 1150 {
        2
    } else {
        1
    }
}

/// The biggest scale a font of `gw` x `gh` pixels can have (at least 1).
pub fn max_scale(gw: u32, gh: u32) -> u32 {
    if gw == 0 || gh == 0 {
        return 1;
    }
    (MAX_GLYPH / gw).min(MAX_GLYPH / gh).clamp(1, LEVELS[LEVELS.len() - 1])
}

/// `iyakki.ui_scale=N` on the kernel command line (1 to 4); `None` when absent or not a number in range.
pub fn scale_from_cmdline(cmdline: &str) -> Option<u32> {
    cmdline.split_whitespace().find_map(|t| t.strip_prefix("iyakki.ui_scale=")).and_then(|v| v.parse::<u32>().ok()).filter(|v| LEVELS.contains(v))
}

/// The framebuffer size from `/sys/class/graphics/fbN/virtual_size` ("2560,1600").
pub fn parse_virtual_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.trim().split_once(',')?;
    let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The next scale after a key: `+` or `=` one level up, `-` or `_` one level down, kept between 1 and `max`. `None` for any other
/// character or when the scale would not change.
pub fn scale_after_key(c: char, current: u32, max: u32) -> Option<u32> {
    let next = match c {
        '+' | '=' => current + 1,
        '-' | '_' => current.saturating_sub(1),
        _ => return None,
    };
    let next = next.clamp(1, max.max(1));
    (next != current).then_some(next)
}

/// A console font: `charcount` glyphs of `width` x `height` pixels. Each glyph takes `32 * ((width + 7) / 8)` bytes (the console's
/// layout: 32 rows reserved, the first `height` used, rows padded to whole bytes, the leftmost pixel in the top bit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    pub width: u32,
    pub height: u32,
    pub charcount: u32,
    pub data: Vec<u8>,
}

fn row_bytes(width: u32) -> usize {
    width.div_ceil(8) as usize
}

impl Font {
    pub fn glyph_bytes(&self) -> usize {
        MAX_GLYPH as usize * row_bytes(self.width)
    }

    fn pixel(&self, glyph: usize, x: u32, y: u32) -> bool {
        let at = glyph * self.glyph_bytes() + y as usize * row_bytes(self.width) + (x / 8) as usize;
        self.data.get(at).is_some_and(|b| b & (0x80 >> (x % 8)) != 0)
    }
}

/// The font enlarged `scale` times in both directions. `None` when the result would not fit the console (wider or taller than 32
/// pixels) or the font data is shorter than its size says.
pub fn scale_glyphs(f: &Font, scale: u32) -> Option<Font> {
    if scale == 0 || f.width == 0 || f.height == 0 || f.width * scale > MAX_GLYPH || f.height * scale > MAX_GLYPH || f.data.len() < f.glyph_bytes() * f.charcount as usize {
        return None;
    }
    let (w, h) = (f.width * scale, f.height * scale);
    let mut out = Font { width: w, height: h, charcount: f.charcount, data: vec![0; MAX_GLYPH as usize * row_bytes(w) * f.charcount as usize] };
    let (stride, rb) = (out.glyph_bytes(), row_bytes(w));
    for g in 0..f.charcount as usize {
        for y in 0..h {
            for x in 0..w {
                if f.pixel(g, x / scale, y / scale) {
                    out.data[g * stride + y as usize * rb + (x / 8) as usize] |= 0x80 >> (x % 8);
                }
            }
        }
    }
    Some(out)
}

#[cfg(target_os = "linux")]
mod console {
    use super::Font;

    #[repr(C)]
    struct FontOp {
        op: u32,
        flags: u32,
        width: u32,
        height: u32,
        charcount: u32,
        data: *mut u8,
    }

    const KDFONTOP: libc::c_ulong = 0x4B72;
    const OP_SET: u32 = 0;
    const OP_GET: u32 = 1;

    pub fn get(fd: i32) -> Option<Font> {
        let mut buf = vec![0u8; 512 * 32 * 4];
        let mut op = FontOp { op: OP_GET, flags: 0, width: 32, height: 32, charcount: 512, data: buf.as_mut_ptr() };
        // SAFETY: KDFONTOP/GET fills `buf` (512 glyphs of up to 32x32 pixels = 64 KiB, exactly its size) and the three sizes.
        if unsafe { libc::ioctl(fd, KDFONTOP as _, &mut op) } != 0 || op.width == 0 || op.height == 0 {
            return None;
        }
        let f = Font { width: op.width, height: op.height, charcount: op.charcount, data: Vec::new() };
        let n = f.glyph_bytes() * op.charcount as usize;
        buf.truncate(n);
        Some(Font { data: buf, ..f })
    }

    pub fn set(fd: i32, f: &Font) -> bool {
        let mut data = f.data.clone();
        let mut op = FontOp { op: OP_SET, flags: 0, width: f.width, height: f.height, charcount: f.charcount, data: data.as_mut_ptr() };
        // SAFETY: KDFONTOP/SET reads `charcount` glyphs of 32 rows each from `data`, which holds exactly that many bytes.
        unsafe { libc::ioctl(fd, KDFONTOP as _, &mut op) == 0 }
    }
}

/// The installer's text size on the Linux console, restored when dropped. Does nothing where the console font cannot be read
/// (a terminal window, macOS, a console without a font ioctl): `scale` stays 1 and `max` 1.
pub struct Zoom {
    pub scale: u32,
    pub max: u32,
    #[cfg(target_os = "linux")]
    original: Option<Font>,
}

impl Zoom {
    /// Pick the scale (the command line's `iyakki.ui_scale`, else from the screen's size) and apply it.
    pub fn start() -> Zoom {
        #[cfg(target_os = "linux")]
        {
            let Some(original) = console::get(1) else { return Zoom { scale: 1, max: 1, original: None } };
            let max = max_scale(original.width, original.height);
            let size = std::fs::read_to_string("/sys/class/graphics/fb0/virtual_size").ok().and_then(|t| parse_virtual_size(&t)).unwrap_or((0, 0));
            let want = scale_from_cmdline(&iyakki_storage::cmdline_text()).unwrap_or_else(|| scale_for_screen(size.0, size.1));
            let mut z = Zoom { scale: 1, max, original: Some(original) };
            z.set(want.min(max));
            z
        }
        #[cfg(not(target_os = "linux"))]
        Zoom { scale: 1, max: 1 }
    }

    /// Change the scale to `want` (kept within 1 and `max`). Returns whether the console changed.
    pub fn set(&mut self, want: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            let want = want.clamp(1, self.max.max(1));
            let Some(orig) = &self.original else { return false };
            if want == self.scale {
                return false;
            }
            let font = if want == 1 { Some(orig.clone()) } else { scale_glyphs(orig, want) };
            if font.is_some_and(|f| console::set(1, &f)) {
                self.scale = want;
                return true;
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = want;
        false
    }

    /// A key press (`+`, `-`): true when the text size changed and the screen must be drawn again.
    pub fn key(&mut self, c: char) -> bool {
        match scale_after_key(c, self.scale, self.max) {
            Some(n) => self.set(n),
            None => false,
        }
    }
}

impl Drop for Zoom {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if self.scale != 1 {
            if let Some(o) = &self.original {
                console::set(1, o);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_follows_the_resolution() {
        // (width, height, scale)
        for (w, h, s) in [(0, 0, 1), (1024, 768, 1), (1280, 720, 1), (1366, 768, 1), (1600, 900, 1), (1920, 1080, 2), (1920, 1200, 2), (2560, 1440, 2), (2560, 1600, 2), (2880, 1800, 2), (3840, 2160, 3), (5120, 2880, 3)] {
            assert_eq!(scale_for_screen(w, h), s, "{w}x{h}");
        }
        // every screen of 2000 pixels or more across gets at least 2
        for w in (2000..=6000).step_by(100) {
            assert!(scale_for_screen(w, 1000) >= 2, "{w}");
        }
        // the scale never goes down as the screen grows
        let mut last = 0;
        for w in (640..=5200).step_by(40) {
            let s = scale_for_screen(w, w * 10 / 16);
            assert!(s >= last, "{w}");
            last = s;
        }
    }

    #[test]
    fn the_font_limits_the_scale() {
        assert_eq!(max_scale(8, 16), 2, "the console's 8x16 font can be doubled");
        assert_eq!(max_scale(8, 8), 4);
        assert_eq!(max_scale(16, 32), 1);
        assert_eq!(max_scale(0, 0), 1);
        assert_eq!(scale_from_cmdline("quiet iyakki.ui_scale=2 iyakki.install=1"), Some(2));
        for bad in ["", "iyakki.ui_scale=0", "iyakki.ui_scale=9", "iyakki.ui_scale=x", "ui_scale=2"] {
            assert_eq!(scale_from_cmdline(bad), None, "{bad:?}");
        }
        assert_eq!(parse_virtual_size("2560,1600\n"), Some((2560, 1600)));
        for bad in ["", "2560", "0,0", "a,b"] {
            assert_eq!(parse_virtual_size(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn plus_and_minus_move_between_the_levels_and_stop_at_the_ends() {
        assert_eq!(scale_after_key('+', 1, 2), Some(2));
        assert_eq!(scale_after_key('=', 1, 2), Some(2));
        assert_eq!(scale_after_key('+', 2, 2), None, "already the biggest the font allows");
        assert_eq!(scale_after_key('-', 2, 2), Some(1));
        assert_eq!(scale_after_key('-', 1, 2), None);
        assert_eq!(scale_after_key('x', 1, 2), None);
        assert_eq!(scale_after_key('+', 3, 4), Some(4));
    }

    fn sample() -> Font {
        // two glyphs of 8x16: glyph 0 has its top-left pixel and its bottom-right pixel set, glyph 1 a full row 3
        let mut data = vec![0u8; 2 * 32];
        data[0] = 0x80;
        data[15] = 0x01;
        data[32 + 3] = 0xFF;
        Font { width: 8, height: 16, charcount: 2, data }
    }

    #[test]
    fn every_font_pixel_becomes_a_block() {
        let f = sample();
        let d = scale_glyphs(&f, 2).unwrap();
        assert_eq!((d.width, d.height, d.charcount), (16, 32, 2));
        assert_eq!(d.glyph_bytes(), 64, "32 rows of 2 bytes");
        let bit = |g: usize, x: u32, y: u32| d.pixel(g, x, y);
        // the top-left pixel is a 2x2 block at the top-left, and nothing next to it
        assert!(bit(0, 0, 0) && bit(0, 1, 0) && bit(0, 0, 1) && bit(0, 1, 1));
        assert!(!bit(0, 2, 0) && !bit(0, 0, 2));
        // the bottom-right pixel (7, 15) is the block (14..16, 30..32)
        assert!(bit(0, 14, 30) && bit(0, 15, 31) && !bit(0, 13, 30) && !bit(0, 15, 29));
        // row 3 of glyph 1 is rows 6 and 7 of the big one, the whole width
        for x in 0..16 {
            assert!(bit(1, x, 6) && bit(1, x, 7) && !bit(1, x, 5) && !bit(1, x, 8), "{x}");
        }
        // the set pixels: 4 + 4 for glyph 0, 16 columns * 2 rows = 32 for glyph 1
        let count = |g: usize| (0..16).flat_map(|x| (0..32).map(move |y| (x, y))).filter(|(x, y)| d.pixel(g, *x, *y)).count();
        assert_eq!((count(0), count(1)), (8, 32));
        // scale 1 is the same font
        assert_eq!(scale_glyphs(&f, 1).unwrap(), f);
    }

    #[test]
    fn a_font_that_would_not_fit_the_console_is_refused() {
        let f = sample();
        assert!(scale_glyphs(&f, 3).is_none(), "8x16 tripled is 48 pixels tall");
        assert!(scale_glyphs(&f, 0).is_none());
        let short = Font { data: vec![0; 10], ..f };
        assert!(scale_glyphs(&short, 2).is_none(), "data shorter than the size says");
    }

    #[test]
    fn a_three_times_font_fits_when_the_glyph_is_small() {
        let f = Font { width: 8, height: 8, charcount: 1, data: { let mut d = vec![0u8; 32]; d[0] = 0x81; d } };
        let d = scale_glyphs(&f, 3).unwrap();
        assert_eq!((d.width, d.height), (24, 24));
        assert!(d.pixel(0, 0, 0) && d.pixel(0, 2, 2) && !d.pixel(0, 3, 0) && d.pixel(0, 21, 0) && d.pixel(0, 23, 2));
        assert_eq!(d.glyph_bytes(), 96, "24 pixels = 3 bytes per row, 32 rows");
    }
}
