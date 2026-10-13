//! Linux framebuffer output (`/dev/fb0`). On modern kernels this is the fbdev emulation layer
//! over DRM/KMS (`CONFIG_DRM_FBDEV_EMULATION`), so it works with simpledrm/EFI-GOP, i915,
//! amdgpu, bochs, virtio-gpu and others without any userspace GPU stack.

use std::os::fd::AsRawFd;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Bitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct VarInfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: Bitfield,
    green: Bitfield,
    blue: Bitfield,
    transp: Bitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FixInfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    typ: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

/// How pixels are packed on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub bpp: u32,
    pub r: (u32, u32), // (offset, length)
    pub g: (u32, u32),
    pub b: (u32, u32),
}

/// Convert premultiplied-RGBA rows to the framebuffer layout. `src` is tightly packed RGBA,
/// `dst` rows are `pitch` bytes apart. Only 16 and 32 bits per pixel are supported.
pub fn blit(src: &[u8], w: usize, h: usize, fmt: PixelFormat, dst: &mut [u8], pitch: usize) -> Result<(), String> {
    let bytes = (fmt.bpp / 8) as usize;
    if bytes != 2 && bytes != 4 {
        return Err(format!("unsupported framebuffer depth {} bpp", fmt.bpp));
    }
    if dst.len() < pitch * h || src.len() < w * h * 4 || pitch < w * bytes {
        return Err("framebuffer smaller than the frame".into());
    }
    let pack = |c: u8, f: (u32, u32)| -> u32 { ((c as u32) >> 8u32.saturating_sub(f.1)) << f.0 };
    for y in 0..h {
        let s = &src[y * w * 4..(y + 1) * w * 4];
        let d = &mut dst[y * pitch..y * pitch + w * bytes];
        if bytes == 4 && fmt.r == (16, 8) && fmt.g == (8, 8) && fmt.b == (0, 8) {
            for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
                dp.copy_from_slice(&[sp[2], sp[1], sp[0], 0xff]); // BGRX
            }
        } else {
            for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(bytes)) {
                let v = pack(sp[0], fmt.r) | pack(sp[1], fmt.g) | pack(sp[2], fmt.b);
                dp.copy_from_slice(&v.to_le_bytes()[..bytes]);
            }
        }
    }
    Ok(())
}

/// Convert one row of RGBA pixels into the framebuffer's layout.
fn convert_row(s: &[u8], d: &mut [u8], bytes: usize, fmt: PixelFormat) {
    let pack = |c: u8, f: (u32, u32)| -> u32 { ((c as u32) >> 8u32.saturating_sub(f.1)) << f.0 };
    if bytes == 4 && fmt.r == (16, 8) && fmt.g == (8, 8) && fmt.b == (0, 8) {
        for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
            dp.copy_from_slice(&[sp[2], sp[1], sp[0], 0xff]); // BGRX
        }
    } else {
        for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(bytes)) {
            let v = pack(sp[0], fmt.r) | pack(sp[1], fmt.g) | pack(sp[2], fmt.b);
            dp.copy_from_slice(&v.to_le_bytes()[..bytes]);
        }
    }
}

/// Write only the rectangle (`x`, `y`, `rw`, `rh`) of `src` (a full `w`-wide RGBA frame) into `dst`.
#[allow(clippy::too_many_arguments)]
pub fn blit_rect(src: &[u8], w: usize, h: usize, rect: (usize, usize, usize, usize), fmt: PixelFormat, dst: &mut [u8], pitch: usize) -> Result<(), String> {
    let bytes = (fmt.bpp / 8) as usize;
    let (x, y, rw, rh) = rect;
    if bytes != 2 && bytes != 4 {
        return Err(format!("unsupported framebuffer depth {} bpp", fmt.bpp));
    }
    if x + rw > w || y + rh > h || src.len() < w * h * 4 || dst.len() < pitch * h || pitch < w * bytes {
        return Err("rectangle outside the frame".into());
    }
    for row in y..y + rh {
        let s = &src[(row * w + x) * 4..(row * w + x + rw) * 4];
        let d = &mut dst[row * pitch + x * bytes..row * pitch + (x + rw) * bytes];
        convert_row(s, d, bytes, fmt);
    }
    Ok(())
}

/// Write a small RGBA `patch` (`rw` x `rh` pixels) at (`x`, `y`) of the framebuffer.
pub fn blit_patch(patch: &[u8], rect: (usize, usize, usize, usize), fmt: PixelFormat, dst: &mut [u8], pitch: usize) -> Result<(), String> {
    let bytes = (fmt.bpp / 8) as usize;
    let (x, y, rw, rh) = rect;
    if bytes != 2 && bytes != 4 {
        return Err(format!("unsupported framebuffer depth {} bpp", fmt.bpp));
    }
    if patch.len() < rw * rh * 4 || dst.len() < (y + rh) * pitch || pitch < (x + rw) * bytes {
        return Err("patch outside the framebuffer".into());
    }
    for row in 0..rh {
        let s = &patch[row * rw * 4..(row + 1) * rw * 4];
        let d = &mut dst[(y + row) * pitch + x * bytes..(y + row) * pitch + (x + rw) * bytes];
        convert_row(s, d, bytes, fmt);
    }
    Ok(())
}

/// Runs of consecutive rows (`start..end`) that differ between two frames of the same size.
pub fn changed_rows(old: &[u8], new: &[u8], row_bytes: usize) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    if row_bytes == 0 || old.len() != new.len() {
        return runs;
    }
    for (y, (a, b)) in old.chunks_exact(row_bytes).zip(new.chunks_exact(row_bytes)).enumerate() {
        if a != b {
            match runs.last_mut() {
                Some(r) if r.1 == y => r.1 = y + 1,
                _ => runs.push((y, y + 1)),
            }
        }
    }
    runs
}

pub struct Framebuffer {
    /// `None` after [`Framebuffer::release`].
    _file: Option<std::fs::File>,
    /// The driver's name for the screen ("simpledrm", "i915drmfb"...): changes when a GPU driver takes over.
    pub id: String,
    map: *mut u8,
    len: usize,
    pub width: u32,
    pub height: u32,
    pub pitch: usize,
    pub format: PixelFormat,
}

// SAFETY: the mapping is owned by this struct and only accessed through &mut self.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub fn open(path: &str) -> Result<Framebuffer, String> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{path}: {e}"))?;
        let fd = file.as_raw_fd();
        let (mut var, mut fix) = (VarInfo::default(), FixInfo::default());
        // SAFETY: the ioctls fill the repr(C) structs above, which match the kernel's layouts.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var as *mut VarInfo) < 0 || libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix as *mut FixInfo) < 0 {
                return Err(format!("{path}: not a framebuffer ({})", std::io::Error::last_os_error()));
            }
        }
        let len = fix.smem_len as usize;
        // SAFETY: mapping the device file shared and writable for its reported length.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0) };
        if map == libc::MAP_FAILED {
            return Err(format!("{path}: mmap failed ({})", std::io::Error::last_os_error()));
        }
        Ok(Framebuffer {
            _file: Some(file),
            id: String::from_utf8_lossy(&fix.id).trim_end_matches('\0').to_string(),
            map: map.cast(),
            len,
            width: var.xres,
            height: var.yres,
            pitch: fix.line_length as usize,
            format: PixelFormat { bpp: var.bits_per_pixel, r: (var.red.offset, var.red.length), g: (var.green.offset, var.green.length), b: (var.blue.offset, var.blue.length) },
        })
    }

    /// The mapped screen memory (empty after [`Framebuffer::release`], so a write then fails instead of crashing).
    fn bytes(&mut self) -> &mut [u8] {
        if self.map.is_null() {
            return &mut [];
        }
        // SAFETY: `map`/`len` describe the live mapping created in `open`; it is unmapped only by `release` and drop.
        unsafe { std::slice::from_raw_parts_mut(self.map, self.len) }
    }

    /// Let go of the display: unmap and close it, so another program (the full web engine's compositor) can take it
    /// over. Every later write fails with an error until a new [`Framebuffer`] is opened. Harmless to call twice.
    pub fn release(&mut self) {
        if !self.map.is_null() {
            // SAFETY: unmapping the region mapped in `open`; the pointer is cleared so it is never used again.
            unsafe { libc::munmap(self.map.cast(), self.len) };
            self.map = std::ptr::null_mut();
            self.len = 0;
        }
        self._file = None;
    }

    pub fn is_released(&self) -> bool {
        self._file.is_none()
    }

    pub fn present(&mut self, rgba: &[u8]) -> Result<(), String> {
        let (w, h, fmt, pitch) = (self.width as usize, self.height as usize, self.format, self.pitch);
        blit(rgba, w, h, fmt, self.bytes(), pitch)
    }

    /// Write only the rows that differ from `old` (the frame already on screen). Returns the rows written.
    /// A 4K frame is 33 MB: rewriting it for a blinking caret would cost more than the rest of the UI.
    pub fn present_changed(&mut self, new: &[u8], old: Option<&[u8]>) -> Result<usize, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let Some(old) = old.filter(|o| o.len() == new.len()) else {
            self.present(new)?;
            return Ok(h);
        };
        let (fmt, pitch) = (self.format, self.pitch);
        let dst = self.bytes();
        let mut rows = 0;
        for (a, b) in changed_rows(old, new, w * 4) {
            blit(&new[a * w * 4..b * w * 4], w, b - a, fmt, &mut dst[a * pitch..], pitch)?;
            rows += b - a;
        }
        Ok(rows)
    }

    /// Like [`Framebuffer::present_changed`], but the frame is cut into bands that are compared and written by one
    /// thread each (a 4K frame is 33 MB, and writing video memory is slow), and `old` is brought up to date with
    /// the rows that were written. With an `old` of another size every row is written. Returns the rows written.
    pub fn present_banded(&mut self, new: &[u8], old: &mut Vec<u8>) -> Result<usize, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let row = w * 4;
        if new.len() < row * h || h == 0 {
            return Err("frame smaller than the screen".into());
        }
        if self.is_released() {
            return Err("the screen is not ours right now".into());
        }
        let full = old.len() != new.len();
        if full {
            *old = new.to_vec();
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
        let band = h.div_ceil(threads);
        let (fmt, pitch) = (self.format, self.pitch);
        let dst = self.bytes();
        let jobs: Vec<_> = dst.chunks_mut(band * pitch).zip(old.chunks_mut(band * row)).zip(new.chunks(band * row)).collect();
        let results: Vec<Result<usize, String>> = std::thread::scope(|sc| {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|((d, o), n)| {
                    sc.spawn(move || {
                        let runs = if full { vec![(0, n.len() / row)] } else { changed_rows(o, n, row) };
                        let mut rows = 0;
                        for (a, b) in runs {
                            blit(&n[a * row..b * row], w, b - a, fmt, &mut d[a * pitch..], pitch)?;
                            if !full {
                                o[a * row..b * row].copy_from_slice(&n[a * row..b * row]);
                            }
                            rows += b - a;
                        }
                        Ok(rows)
                    })
                })
                .collect();
            handles.into_iter().map(|j| j.join().unwrap_or_else(|_| Err("band thread panicked".into()))).collect()
        });
        let mut rows = 0;
        for r in results {
            rows += r?;
        }
        Ok(rows)
    }

    /// Write a small RGBA patch at `rect` (x, y, w, h).
    pub fn present_patch(&mut self, patch: &[u8], rect: (usize, usize, usize, usize)) -> Result<(), String> {
        let (fmt, pitch) = (self.format, self.pitch);
        blit_patch(patch, rect, fmt, self.bytes(), pitch)
    }

    /// Write a rectangle of a full RGBA frame (used to move the pointer without redrawing the screen).
    pub fn present_rect(&mut self, frame: &[u8], rect: (usize, usize, usize, usize)) -> Result<(), String> {
        let (w, h, fmt, pitch) = (self.width as usize, self.height as usize, self.format, self.pitch);
        blit_rect(frame, w, h, rect, fmt, self.bytes(), pitch)
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XRGB: PixelFormat = PixelFormat { bpp: 32, r: (16, 8), g: (8, 8), b: (0, 8) };
    const RGB565: PixelFormat = PixelFormat { bpp: 16, r: (11, 5), g: (5, 6), b: (0, 5) };

    #[test]
    fn a_released_screen_refuses_writes_and_can_be_released_twice() {
        let path = std::env::temp_dir().join(format!("iyakki-fb-test-{}", std::process::id()));
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        // SAFETY: mapping the 4096-byte test file shared and writable.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        assert_ne!(map, libc::MAP_FAILED);
        let mut fb = Framebuffer { _file: Some(file), id: "test".into(), map: map.cast(), len: 4096, width: 4, height: 4, pitch: 16, format: XRGB };
        let frame = vec![200u8; 4 * 4 * 4];
        assert!(fb.present(&frame).is_ok());
        assert!(!fb.is_released());
        fb.release();
        assert!(fb.is_released());
        assert!(fb.present(&frame).is_err(), "no write after release");
        assert!(fb.present_patch(&frame, (0, 0, 4, 4)).is_err());
        assert!(fb.present_rect(&frame, (0, 0, 4, 4)).is_err());
        assert!(fb.present_banded(&frame, &mut Vec::new()).is_err());
        fb.release(); // twice is fine
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn blits_xrgb8888_with_row_padding() {
        let src = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 10, 20, 30, 255]; // 2x2: R G / B misc
        let mut dst = vec![0xAAu8; 12 * 2]; // pitch 12 bytes: 8 pixels + 4 padding
        blit(&src, 2, 2, XRGB, &mut dst, 12).unwrap();
        assert_eq!(&dst[0..4], &[0, 0, 255, 255], "red in BGRX byte order");
        assert_eq!(&dst[4..8], &[0, 255, 0, 255]);
        assert_eq!(&dst[8..12], &[0xAA; 4], "padding untouched");
        assert_eq!(&dst[12..16], &[255, 0, 0, 255], "blue");
    }

    #[test]
    fn blits_rgb565_and_rgb_order() {
        let src = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255];
        let mut dst = vec![0u8; 6];
        blit(&src, 3, 1, RGB565, &mut dst, 6).unwrap();
        assert_eq!(u16::from_le_bytes([dst[0], dst[1]]), 0xF800);
        assert_eq!(u16::from_le_bytes([dst[2], dst[3]]), 0x07E0);
        assert_eq!(u16::from_le_bytes([dst[4], dst[5]]), 0x001F);
        // 32bpp with RGB (not BGR) byte order takes the generic path
        let rgbx = PixelFormat { bpp: 32, r: (0, 8), g: (8, 8), b: (16, 8) };
        let mut d2 = vec![0u8; 4];
        blit(&src[..4], 1, 1, rgbx, &mut d2, 4).unwrap();
        assert_eq!(&d2[..3], &[255, 0, 0]);
    }

    #[test]
    fn rejects_bad_geometry_and_depths() {
        let src = vec![0u8; 16];
        assert!(blit(&src, 2, 2, PixelFormat { bpp: 24, ..XRGB }, &mut vec![0u8; 100], 8).is_err());
        assert!(blit(&src, 2, 2, XRGB, &mut vec![0u8; 8], 8).is_err(), "buffer too small");
        assert!(blit(&src, 2, 2, XRGB, &mut vec![0u8; 100], 4).is_err(), "pitch too small");
    }

    #[test]
    fn blit_rect_writes_only_its_rectangle() {
        let mut src = vec![0u8; 4 * 4 * 4];
        for px in src.chunks_exact_mut(4) {
            px.copy_from_slice(&[10, 20, 30, 255]);
        }
        let mut dst = vec![0xEEu8; 4 * 4 * 4];
        blit_rect(&src, 4, 4, (1, 1, 2, 2), XRGB, &mut dst, 16).unwrap();
        let at = |x: usize, y: usize| &dst[y * 16 + x * 4..y * 16 + x * 4 + 4];
        assert_eq!(at(1, 1), &[30, 20, 10, 255]);
        assert_eq!(at(2, 2), &[30, 20, 10, 255]);
        assert_eq!(at(0, 0), &[0xEE; 4], "outside untouched");
        assert_eq!(at(3, 1), &[0xEE; 4]);
        assert!(blit_rect(&src, 4, 4, (3, 3, 2, 2), XRGB, &mut dst, 16).is_err(), "off the edge");
    }

    #[test]
    fn only_changed_rows_are_found_and_adjacent_ones_merge() {
        let a = vec![0u8; 8 * 6];
        let mut b = a.clone();
        assert!(changed_rows(&a, &b, 8).is_empty());
        b[8 * 1] = 1; // row 1
        b[8 * 2 + 3] = 1; // row 2
        b[8 * 5 + 7] = 1; // row 5
        assert_eq!(changed_rows(&a, &b, 8), vec![(1, 3), (5, 6)]);
        assert!(changed_rows(&a, &b[..40], 8).is_empty(), "different sizes: caller repaints everything");
        assert!(changed_rows(&a, &b, 0).is_empty());
    }

    #[test]
    fn banded_presenting_writes_the_changed_rows_and_updates_the_old_frame() {
        let (w, h) = (4usize, 10usize);
        let mut fbmem = vec![0xEEu8; w * 4 * h];
        let fb_new = |mem: &mut Vec<u8>| Framebuffer { _file: Some(std::fs::File::open("/dev/null").unwrap()), id: String::new(), map: mem.as_mut_ptr(), len: mem.len(), width: w as u32, height: h as u32, pitch: w * 4, format: XRGB };
        let mut new = vec![10u8; w * 4 * h];
        let mut old: Vec<u8> = Vec::new();
        assert_eq!(fb_new(&mut fbmem).present_banded(&new, &mut old).unwrap(), h, "no old frame: everything is written");
        assert_eq!(old, new);
        assert_eq!(&fbmem[0..4], &[10, 10, 10, 0xFF].map(|v| if v == 0xFF { fbmem[3] } else { v }));
        new[w * 4 * 7 + 5] = 99; // one pixel in row 7
        for b in fbmem.iter_mut() { *b = 0xEE; }
        assert_eq!(fb_new(&mut fbmem).present_banded(&new, &mut old).unwrap(), 1, "only row 7 differs");
        assert_eq!(old, new, "the old frame follows");
        assert_eq!(fbmem[0], 0xEE, "other rows are untouched");
        assert_ne!(fbmem[w * 4 * 7], 0xEE);
    }

    #[test]
    fn opening_a_non_framebuffer_fails_cleanly() {
        assert!(Framebuffer::open("/dev/null").is_err());
        assert!(Framebuffer::open("/no/such/fb").is_err());
    }
}
