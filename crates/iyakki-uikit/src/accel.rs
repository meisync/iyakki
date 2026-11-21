//! One place that knows what the graphics hardware can do for the expensive passes of the shell and the in-house browser engine
//! (F-BROX-23): blit and alpha blend of page layers, scaling of pictures, colour conversion of video frames, blur, and scrolling by
//! an offset. No app names a vendor: the apps ask an [`Accel`], which holds a fallback chain (graphics card, then the SIMD-style
//! processor path, then the plain processor path) and a capability probe ([`probe`]) that says in words what is there and why a
//! card is not used.
//!
//! Rules kept here:
//! * the plain path ([`PlainCpu`]) is the reference: every other path is tested against it with a stated tolerance ([`golden`]);
//!   the SIMD-style path is exact (tolerance 0), a card may differ by the tolerances in [`Tolerance`];
//! * a library or driver that is not installed is never assumed: the probe looks at the render node and at the libraries a card
//!   path would load, under an injectable root;
//! * a backend writes its destination only after it succeeded (a card works on its own texture and reads back at the end), so a lost
//!   device never leaves a half-drawn page: the chain drops the card for the rest of the session, reports it once
//!   ([`Accel::take_report`]) and runs the same call on the next path;
//! * the card path itself needs hardware: the harness here ([`golden::run_suite`]) is what a run on real hardware calls.

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};

/// A picture in premultiplied `0xAARRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Surface {
        Surface { w, h, px: vec![0; w * h] }
    }
    pub fn filled(w: usize, h: usize, c: u32) -> Surface {
        Surface { w, h, px: vec![c; w * h] }
    }
}

/// Which path ran a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Gpu,
    SimdCpu,
    PlainCpu,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Gpu => "graphics card",
            Tier::SimdCpu => "processor (wide)",
            Tier::PlainCpu => "processor (plain)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccelError {
    /// The device went away or the driver failed: the card is not used again this session.
    DeviceLost(String),
    /// This path does not do this pass (or this size): the next path does it, the card stays.
    Unsupported,
}

/// The passes a backend can run.
pub trait Backend {
    fn tier(&self) -> Tier;
    /// Source-over of `src` onto `dst` at `(dx, dy)`, clipped to `dst`.
    fn blit(&mut self, dst: &mut Surface, src: &Surface, dx: i32, dy: i32) -> Result<(), AccelError>;
    /// Bilinear scale to `dw` x `dh`.
    fn scale(&mut self, src: &Surface, dw: usize, dh: usize) -> Result<Surface, AccelError>;
    /// Video frame planar 4:2:0 (limited range, BT.601) to opaque pixels. `w` and `h` are even.
    fn yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Result<Surface, AccelError>;
    /// Box blur of the given radius (edges repeat).
    fn blur(&mut self, s: &mut Surface, radius: usize) -> Result<(), AccelError>;
    /// Move the picture up by `dy` rows (down when negative); the rows that come in are transparent.
    fn scroll(&mut self, s: &mut Surface, dy: i32) -> Result<(), AccelError>;
    /// Pixels the backend has produced so far (a count, never a time: tests budget on it).
    fn pixels_done(&self) -> u64;
}

// ---- shared integer maths (the reference formulas) ----

/// Exact rounded `x * a / 255` for `x`, `a` in `0..=255`.
#[inline]
pub fn mul255(x: u32, a: u32) -> u32 {
    let t = x * a + 128;
    (t + (t >> 8)) >> 8
}

#[inline]
fn over_px(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let inv = 255 - sa;
    let ch = |sh: u32| (((src >> sh) & 255) + mul255((dst >> sh) & 255, inv)).min(255);
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Two 8-bit lanes held in 16-bit lanes at once: `(x * a / 255)` rounded, exactly as [`mul255`].
#[inline]
fn mul255_pair(x: u32, a: u32) -> u32 {
    let t = x * a + 0x0080_0080;
    ((t + ((t >> 8) & 0x00ff_00ff)) >> 8) & 0x00ff_00ff
}

#[inline]
fn over_px_wide(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let inv = 255 - sa;
    let rb = mul255_pair(dst & 0x00ff_00ff, inv) + (src & 0x00ff_00ff);
    let ag = mul255_pair((dst >> 8) & 0x00ff_00ff, inv) + ((src >> 8) & 0x00ff_00ff);
    // a channel can only exceed 255 when the source was not premultiplied: clamp each lane like the reference does
    let sat = |v: u32| {
        let hi = (v >> 8) & 0x0001_0001;
        (v | (hi * 255)) & 0x00ff_00ff
    };
    sat(rb) | (sat(ag) << 8)
}

fn clip(dst: &Surface, src: &Surface, dx: i32, dy: i32) -> Option<(usize, usize, usize, usize, usize)> {
    let x0 = dx.max(0) as i64;
    let y0 = dy.max(0) as i64;
    let x1 = (dx as i64 + src.w as i64).min(dst.w as i64);
    let y1 = (dy as i64 + src.h as i64).min(dst.h as i64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, (x0 - dx as i64) as usize))
}

/// One entry of the coordinate table of a bilinear scale: the two source indexes and the weight of the second (0..=256).
fn axis_one(sn: usize, dn: usize, i: usize) -> (usize, usize, u32) {
    let pos = (((2 * i + 1) * sn) as i64 * 65536) / (2 * dn as i64) - 32768;
    let pos = pos.clamp_safe(0, ((sn - 1) as i64) * 65536);
    let a = (pos >> 16) as usize;
    (a, (a + 1).min(sn - 1), ((pos & 0xffff) >> 8) as u32)
}

fn axis(sn: usize, dn: usize) -> Vec<(usize, usize, u32)> {
    (0..dn).map(|i| axis_one(sn, dn, i)).collect()
}

#[inline]
fn lerp_px(p00: u32, p10: u32, p01: u32, p11: u32, wx: u32, wy: u32) -> u32 {
    let mut out = 0u32;
    for sh in [24u32, 16, 8, 0] {
        let c = |p: u32| (p >> sh) & 255;
        let top = c(p00) * (256 - wx) + c(p10) * wx;
        let bot = c(p01) * (256 - wx) + c(p11) * wx;
        let v = (top * (256 - wy) + bot * wy + 32768) >> 16;
        out |= v.min(255) << sh;
    }
    out
}

fn scale_with(src: &Surface, dw: usize, dh: usize, xs: &[(usize, usize, u32)], ys: &[(usize, usize, u32)]) -> Surface {
    let mut out = Surface::new(dw, dh);
    for (y, &(ya, yb, wy)) in ys.iter().enumerate() {
        let r0 = &src.px[ya * src.w..(ya + 1) * src.w];
        let r1 = &src.px[yb * src.w..(yb + 1) * src.w];
        let row = &mut out.px[y * dw..(y + 1) * dw];
        for (x, &(xa, xb, wx)) in xs.iter().enumerate() {
            row[x] = lerp_px(r0[xa], r0[xb], r1[xa], r1[xb], wx, wy);
        }
    }
    out
}

#[inline]
fn yuv_px(y: u8, u: u8, v: u8) -> u32 {
    let c = (y as i32 - 16).max(0) * 298;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u32;
    let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u32;
    let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u32;
    0xff00_0000 | (r << 16) | (g << 8) | b
}

fn check_yuv(y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> bool {
    w > 0 && h > 0 && w % 2 == 0 && h % 2 == 0 && y.len() >= w * h && u.len() >= w * h / 4 && v.len() >= w * h / 4
}

/// The edge-clamped box blur of one line of `n` values with stride `step`, naive: the reference.
fn blur_line_plain(buf: &mut [u32], start: usize, step: usize, n: usize, r: usize) {
    let src: Vec<u32> = (0..n).map(|i| buf[start + i * step]).collect();
    let win = (2 * r + 1) as u32;
    for i in 0..n {
        let mut sum = [0u32; 4];
        for k in 0..=2 * r {
            let j = (i + k).saturating_sub(r).min(n - 1);
            for (c, s) in sum.iter_mut().enumerate() {
                *s += (src[j] >> (24 - 8 * c as u32)) & 255;
            }
        }
        let mut px = 0u32;
        for (c, s) in sum.iter().enumerate() {
            px |= ((s + win / 2) / win) << (24 - 8 * c as u32);
        }
        buf[start + i * step] = px;
    }
}

/// The same result with a window that slides (one add and one subtract a value, not a sum of the whole window).
fn blur_line_sliding(buf: &mut [u32], start: usize, step: usize, n: usize, r: usize, scratch: &mut Vec<u32>) {
    scratch.clear();
    scratch.extend((0..n).map(|i| buf[start + i * step]));
    let src = &scratch[..];
    let win = (2 * r + 1) as u32;
    let at = |i: isize| src[i.clamp_safe(0, n as isize - 1) as usize];
    let mut sum = [0u32; 4];
    for k in -(r as isize)..=(r as isize) {
        let p = at(k);
        for (c, s) in sum.iter_mut().enumerate() {
            *s += (p >> (24 - 8 * c as u32)) & 255;
        }
    }
    for i in 0..n {
        let mut px = 0u32;
        for (c, s) in sum.iter().enumerate() {
            px |= ((s + win / 2) / win) << (24 - 8 * c as u32);
        }
        buf[start + i * step] = px;
        let out = at(i as isize - r as isize);
        let inn = at(i as isize + r as isize + 1);
        for (c, s) in sum.iter_mut().enumerate() {
            let sh = 24 - 8 * c as u32;
            *s = *s + ((inn >> sh) & 255) - ((out >> sh) & 255);
        }
    }
}

fn blur_with(s: &mut Surface, r: usize, done: &mut u64, mut line: impl FnMut(&mut [u32], usize, usize, usize, usize)) {
    if r == 0 || s.w == 0 || s.h == 0 {
        return;
    }
    for y in 0..s.h {
        line(&mut s.px, y * s.w, 1, s.w, r);
    }
    for x in 0..s.w {
        line(&mut s.px, x, s.w, s.h, r);
    }
    *done += (s.w * s.h) as u64 * 2;
}

fn scroll_rows(s: &mut Surface, dy: i32) {
    let h = s.h as i64;
    let d = dy as i64;
    if d == 0 || s.w == 0 {
        return;
    }
    if d.abs() >= h {
        s.px.iter_mut().for_each(|p| *p = 0);
        return;
    }
    let w = s.w;
    let keep = (h - d.abs()) as usize * w;
    if d > 0 {
        let off = d as usize * w;
        s.px.copy_within(off..off + keep, 0);
        s.px[keep..].iter_mut().for_each(|p| *p = 0);
    } else {
        let off = (-d) as usize * w;
        s.px.copy_within(0..keep, off);
        s.px[..off].iter_mut().for_each(|p| *p = 0);
    }
}

// ---- the plain processor path: the reference ----

#[derive(Default)]
pub struct PlainCpu {
    done: u64,
}

impl Backend for PlainCpu {
    fn tier(&self) -> Tier {
        Tier::PlainCpu
    }
    fn blit(&mut self, dst: &mut Surface, src: &Surface, dx: i32, dy: i32) -> Result<(), AccelError> {
        if let Some((x0, y0, w, h, sx0)) = clip(dst, src, dx, dy) {
            let sy0 = (y0 as i64 - dy as i64) as usize;
            for r in 0..h {
                for c in 0..w {
                    let d = &mut dst.px[(y0 + r) * dst.w + x0 + c];
                    *d = over_px(*d, src.px[(sy0 + r) * src.w + sx0 + c]);
                }
            }
            self.done += (w * h) as u64;
        }
        Ok(())
    }
    fn scale(&mut self, src: &Surface, dw: usize, dh: usize) -> Result<Surface, AccelError> {
        if src.w == 0 || src.h == 0 || dw == 0 || dh == 0 {
            return Ok(Surface::new(dw, dh));
        }
        // the plain path works the coordinates out again for every row
        let xs = axis(src.w, dw);
        let mut out = Surface::new(dw, dh);
        for y in 0..dh {
            let part = scale_with(src, dw, 1, &xs, &[axis_one(src.h, dh, y)]);
            out.px[y * dw..(y + 1) * dw].copy_from_slice(&part.px);
        }
        self.done += (dw * dh) as u64;
        Ok(out)
    }
    fn yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Result<Surface, AccelError> {
        if !check_yuv(y, u, v, w, h) {
            return Err(AccelError::Unsupported);
        }
        let mut out = Surface::new(w, h);
        for r in 0..h {
            for c in 0..w {
                let ci = (r / 2) * (w / 2) + c / 2;
                out.px[r * w + c] = yuv_px(y[r * w + c], u[ci], v[ci]);
            }
        }
        self.done += (w * h) as u64;
        Ok(out)
    }
    fn blur(&mut self, s: &mut Surface, radius: usize) -> Result<(), AccelError> {
        blur_with(s, radius, &mut self.done, blur_line_plain);
        Ok(())
    }
    fn scroll(&mut self, s: &mut Surface, dy: i32) -> Result<(), AccelError> {
        scroll_rows(s, dy);
        self.done += (s.w * s.h) as u64;
        Ok(())
    }
    fn pixels_done(&self) -> u64 {
        self.done
    }
}

// ---- the wide processor path: the same answers, less work per pixel ----

#[derive(Default)]
pub struct SimdCpu {
    done: u64,
    scratch: Vec<u32>,
}

impl Backend for SimdCpu {
    fn tier(&self) -> Tier {
        Tier::SimdCpu
    }
    fn blit(&mut self, dst: &mut Surface, src: &Surface, dx: i32, dy: i32) -> Result<(), AccelError> {
        if let Some((x0, y0, w, h, sx0)) = clip(dst, src, dx, dy) {
            let sy0 = (y0 as i64 - dy as i64) as usize;
            for r in 0..h {
                let s = &src.px[(sy0 + r) * src.w + sx0..(sy0 + r) * src.w + sx0 + w];
                let d = &mut dst.px[(y0 + r) * dst.w + x0..(y0 + r) * dst.w + x0 + w];
                // runs of opaque pixels are a copy, a transparent run is skipped
                for (dp, &sp) in d.iter_mut().zip(s) {
                    *dp = over_px_wide(*dp, sp);
                }
            }
            self.done += (w * h) as u64;
        }
        Ok(())
    }
    fn scale(&mut self, src: &Surface, dw: usize, dh: usize) -> Result<Surface, AccelError> {
        if src.w == 0 || src.h == 0 || dw == 0 || dh == 0 {
            return Ok(Surface::new(dw, dh));
        }
        // the coordinate tables are built once for the whole picture
        let out = scale_with(src, dw, dh, &axis(src.w, dw), &axis(src.h, dh));
        self.done += (dw * dh) as u64;
        Ok(out)
    }
    fn yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Result<Surface, AccelError> {
        if !check_yuv(y, u, v, w, h) {
            return Err(AccelError::Unsupported);
        }
        let mut out = Surface::new(w, h);
        for r in 0..h {
            let (cu, cv) = (&u[(r / 2) * (w / 2)..], &v[(r / 2) * (w / 2)..]);
            let yr = &y[r * w..(r + 1) * w];
            let o = &mut out.px[r * w..(r + 1) * w];
            // one chroma pair serves two pixels
            for c in (0..w).step_by(2) {
                o[c] = yuv_px(yr[c], cu[c / 2], cv[c / 2]);
                o[c + 1] = yuv_px(yr[c + 1], cu[c / 2], cv[c / 2]);
            }
        }
        self.done += (w * h) as u64;
        Ok(out)
    }
    fn blur(&mut self, s: &mut Surface, radius: usize) -> Result<(), AccelError> {
        let mut scratch = std::mem::take(&mut self.scratch);
        blur_with(s, radius, &mut self.done, |b, st, step, n, r| blur_line_sliding(b, st, step, n, r, &mut scratch));
        self.scratch = scratch;
        Ok(())
    }
    fn scroll(&mut self, s: &mut Surface, dy: i32) -> Result<(), AccelError> {
        scroll_rows(s, dy);
        self.done += (s.w * s.h) as u64;
        Ok(())
    }
    fn pixels_done(&self) -> u64 {
        self.done
    }
}

// ---- the capability probe ----

/// The graphics interface level a card path would use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiLevel {
    None,
    Gles2,
    Gles3,
    Vulkan,
}

impl ApiLevel {
    pub fn label(self) -> &'static str {
        match self {
            ApiLevel::None => "none",
            ApiLevel::Gles2 => "OpenGL ES 2",
            ApiLevel::Gles3 => "OpenGL ES 3",
            ApiLevel::Vulkan => "Vulkan",
        }
    }
}

/// One graphics device with a render node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub node: PathBuf,
    /// Only for the words of a report; no caller branches on it.
    pub vendor: &'static str,
    pub pci_device: u16,
    pub driver: Option<String>,
    /// Software or paravirtual: it draws on the processor anyway, so it is not a card to wait for.
    pub is_software: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub devices: Vec<Device>,
    /// Index into `devices` of the one that would be used.
    pub chosen: Option<usize>,
    pub api: ApiLevel,
    /// Why the card is not used, in a sentence; empty when it is.
    pub reason_off: String,
}

impl Caps {
    pub fn usable(&self) -> bool {
        self.chosen.is_some() && self.api != ApiLevel::None && self.reason_off.is_empty()
    }
    pub fn node(&self) -> Option<&Path> {
        self.chosen.map(|i| self.devices[i].node.as_path())
    }
    /// One line for Settings, the log and the chat tool.
    pub fn summary(&self) -> String {
        match (self.usable(), self.chosen) {
            (true, Some(i)) => {
                let d = &self.devices[i];
                format!("graphics card: {} on {}, driver {}, {} (fallback: processor)", d.vendor, d.node.display(), d.driver.as_deref().unwrap_or("unknown"), self.api.label())
            }
            _ => format!("no graphics card used: {} (the processor draws)", if self.reason_off.is_empty() { "no render node" } else { &self.reason_off }),
        }
    }
}

fn vendor_name(id: u32) -> &'static str {
    match id {
        0x1002 => "AMD",
        0x10de => "NVIDIA",
        0x8086 => "Intel",
        0x106b => "Apple",
        0x1af4 | 0x1234 | 0x15ad => "virtual",
        _ => "other",
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn lib_present(root: &Path, names: &[&str]) -> bool {
    ["usr/lib", "usr/lib64", "lib", "lib64", "usr/lib/x86_64-linux-gnu", "usr/lib/aarch64-linux-gnu", "lib/x86_64-linux-gnu", "lib/aarch64-linux-gnu"].iter().any(|d| names.iter().any(|n| root.join(d).join(n).exists()))
}

/// Looks at the render nodes and the libraries a card path would load, under `root` (`/` on a real machine). `pick` is the person's
/// choice of one node (`renderD129`), else the first real card is used. Nothing is opened or initialised here.
pub fn probe(root: &Path, pick: Option<&str>) -> Caps {
    let mut devices = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.join("sys/class/drm")) {
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("renderD")).collect();
        names.sort();
        for n in names {
            let dev = root.join("sys/class/drm").join(&n).join("device");
            let vid = read_trim(&dev.join("vendor")).and_then(|s| hex(&s)).unwrap_or(0);
            let did = read_trim(&dev.join("device")).and_then(|s| hex(&s)).unwrap_or(0) as u16;
            let driver = std::fs::read_link(dev.join("driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            let is_software = matches!(driver.as_deref(), Some("virtio-pci") | Some("virtio_gpu") | Some("bochs-drm") | Some("bochs") | Some("vmwgfx") | Some("qxl") | Some("simpledrm") | Some("cirrus")) || matches!(vid, 0x1af4 | 0x1234 | 0x15ad);
            devices.push(Device { node: root.join("dev/dri").join(&n), vendor: vendor_name(vid), pci_device: did, driver, is_software });
        }
    }
    let egl = lib_present(root, &["libEGL.so.1", "libEGL.so"]);
    let gles = lib_present(root, &["libGLESv2.so.2", "libGLESv2.so"]);
    let vulkan = lib_present(root, &["libvulkan.so.1"]);
    let chosen = pick
        .and_then(|p| devices.iter().position(|d| d.node.file_name().is_some_and(|f| f.to_string_lossy() == p) && !d.is_software))
        .or_else(|| devices.iter().position(|d| !d.is_software));
    let api = if egl && gles { if vulkan { ApiLevel::Vulkan } else { ApiLevel::Gles3 } } else { ApiLevel::None };
    let reason_off = if devices.is_empty() {
        "no render node (no graphics driver is loaded)".to_string()
    } else if chosen.is_none() {
        "the only graphics device is virtual or software".to_string()
    } else if !(egl && gles) {
        "the graphics libraries (EGL and OpenGL ES) are not installed".to_string()
    } else {
        String::new()
    };
    Caps { devices, chosen, api, reason_off }
}

// ---- the chain ----

/// Said once when the card is given up for the session (audit and notification text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostReport {
    pub reason: String,
    pub now_using: Tier,
}

pub struct Accel {
    chain: Vec<Box<dyn Backend>>,
    report: Option<LostReport>,
    /// Calls the card path declined or lost, for the numbers page.
    fell_through: u64,
}

impl Accel {
    /// The chain for a machine: the card path only when the caps allow it, a path was supplied, and `allowed` is on (the person's,
    /// the site's and the administrator's switch put together by the caller); the wide path next; the plain path always last.
    pub fn new(caps: &Caps, allowed: bool, gpu: Option<Box<dyn Backend>>) -> Accel {
        let mut chain: Vec<Box<dyn Backend>> = Vec::new();
        if let (true, true, Some(g)) = (allowed, caps.usable(), gpu) {
            chain.push(g);
        }
        chain.push(Box::new(SimdCpu::default()));
        chain.push(Box::new(PlainCpu::default()));
        Accel { chain, report: None, fell_through: 0 }
    }
    /// The processor only (no probe, no card).
    pub fn cpu_only() -> Accel {
        Accel::new(&Caps { devices: vec![], chosen: None, api: ApiLevel::None, reason_off: "off".into() }, false, None)
    }
    /// The tier a call would try first.
    pub fn tier(&self) -> Tier {
        self.chain[0].tier()
    }
    /// The loss report, once; `None` afterwards and when nothing was lost.
    pub fn take_report(&mut self) -> Option<LostReport> {
        self.report.take()
    }
    pub fn fell_through(&self) -> u64 {
        self.fell_through
    }
    /// Pixels done per tier (counts for budgets).
    pub fn pixels_by_tier(&self) -> Vec<(Tier, u64)> {
        self.chain.iter().map(|b| (b.tier(), b.pixels_done())).collect()
    }

    fn run<T>(&mut self, mut f: impl FnMut(&mut dyn Backend) -> Result<T, AccelError>) -> T {
        let mut i = 0;
        loop {
            let last = i + 1 == self.chain.len();
            match f(&mut *self.chain[i]) {
                Ok(v) => return v,
                Err(AccelError::DeviceLost(reason)) if self.chain[i].tier() == Tier::Gpu => {
                    let next = self.chain[i + 1].tier();
                    self.chain.remove(i);
                    if self.report.is_none() {
                        self.report = Some(LostReport { reason, now_using: next });
                    }
                    self.fell_through += 1;
                }
                Err(_) if !last => {
                    self.fell_through += 1;
                    i += 1;
                }
                // the plain path declined (a malformed input): the caller gets what the reference would give for nothing
                Err(_) => unreachable_input(),
            }
        }
    }

    pub fn blit(&mut self, dst: &mut Surface, src: &Surface, dx: i32, dy: i32) {
        self.run(|b| b.blit(dst, src, dx, dy))
    }
    pub fn scale(&mut self, src: &Surface, dw: usize, dh: usize) -> Surface {
        self.run(|b| b.scale(src, dw, dh))
    }
    /// `None` for planes that do not fit the size.
    pub fn yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Option<Surface> {
        if !check_yuv(y, u, v, w, h) {
            return None;
        }
        Some(self.run(|b| b.yuv420(y, u, v, w, h)))
    }
    pub fn blur(&mut self, s: &mut Surface, radius: usize) {
        self.run(|b| b.blur(s, radius))
    }
    pub fn scroll(&mut self, s: &mut Surface, dy: i32) {
        self.run(|b| b.scroll(s, dy))
    }
}

fn unreachable_input() -> ! {
    // the inputs are validated before the chain runs; the plain path only declines what [`check_yuv`] already refused
    panic!("the plain processor path declined a validated call")
}

// ---- the golden harness: every path against the plain reference ----

pub mod golden {
    use super::*;

    /// Largest allowed difference of one channel against the reference, per pass.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Tolerance {
        pub blit: u8,
        pub scale: u8,
        pub yuv: u8,
        pub blur: u8,
        pub scroll: u8,
    }

    /// What the wide path must match (exactly) and what a card may differ by (rounding in float maths and in the sampler).
    pub const EXACT: Tolerance = Tolerance { blit: 0, scale: 0, yuv: 0, blur: 0, scroll: 0 };
    pub const CARD: Tolerance = Tolerance { blit: 1, scale: 2, yuv: 2, blur: 2, scroll: 0 };

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Diff {
        pub max: u8,
        pub over: usize,
        pub size_matches: bool,
    }

    pub fn compare(reference: &Surface, got: &Surface, tol: u8) -> Diff {
        if reference.w != got.w || reference.h != got.h {
            return Diff { max: 255, over: reference.px.len().max(got.px.len()), size_matches: false };
        }
        let (mut max, mut over) = (0u8, 0usize);
        for (a, b) in reference.px.iter().zip(&got.px) {
            let d = (0..4).map(|k| ((a >> (8 * k)) & 255).abs_diff((b >> (8 * k)) & 255) as u8).max().unwrap_or(0);
            max = max.max(d);
            if d > tol {
                over += 1;
            }
        }
        Diff { max, over, size_matches: true }
    }

    /// A deterministic picture with soft and hard edges and every alpha value in it (no files, no random numbers from outside).
    pub fn fixture(w: usize, h: usize, seed: u32) -> Surface {
        let mut s = Surface::new(w, h);
        let mut x = seed.wrapping_mul(2654435761).wrapping_add(12345);
        for p in s.px.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let a = (x >> 24) as u32;
            // premultiplied: channels never above the alpha
            let c = |v: u32| mul255(v & 255, a);
            *p = (a << 24) | (c(x) << 16) | (c(x >> 8) << 8) | c(x >> 16);
        }
        s
    }

    pub fn fixture_yuv(w: usize, h: usize, seed: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let f = fixture(w, h, seed);
        let y: Vec<u8> = f.px.iter().map(|p| (p & 255) as u8).collect();
        let u: Vec<u8> = f.px.iter().take(w * h / 4).map(|p| ((p >> 8) & 255) as u8).collect();
        let v: Vec<u8> = f.px.iter().take(w * h / 4).map(|p| ((p >> 16) & 255) as u8).collect();
        (y, u, v)
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Line {
        pub pass: &'static str,
        pub diff: Diff,
        pub tol: u8,
    }

    impl Line {
        pub fn ok(&self) -> bool {
            self.diff.size_matches && self.diff.over == 0
        }
    }

    /// Runs every pass on fixtures through `candidate` and through the plain reference and lists the differences. On real hardware this
    /// is called with the card path; the result goes into the Manual column (vendor, driver, worst difference per pass).
    pub fn run_suite(candidate: &mut dyn Backend, tol: Tolerance) -> Vec<Line> {
        let mut r = PlainCpu::default();
        let mut out = Vec::new();
        let mut push = |pass: &'static str, tol: u8, a: &Surface, b: &Surface| out.push(Line { pass, diff: compare(a, b, tol), tol });
        let base = fixture(37, 29, 1);
        let layer = fixture(21, 17, 2);
        for (dx, dy) in [(3, 4), (-5, -6), (30, 20), (100, 100)] {
            let (mut a, mut b) = (base.clone(), base.clone());
            r.blit(&mut a, &layer, dx, dy).ok();
            if candidate.blit(&mut b, &layer, dx, dy).is_ok() {
                push("blit", tol.blit, &a, &b);
            }
        }
        for (dw, dh) in [(74, 58), (10, 7), (37, 29), (1, 1)] {
            let a = r.scale(&base, dw, dh).unwrap();
            if let Ok(b) = candidate.scale(&base, dw, dh) {
                push("scale", tol.scale, &a, &b);
            }
        }
        let (y, u, v) = fixture_yuv(32, 18, 3);
        let a = r.yuv420(&y, &u, &v, 32, 18).unwrap();
        if let Ok(b) = candidate.yuv420(&y, &u, &v, 32, 18) {
            push("yuv", tol.yuv, &a, &b);
        }
        for rad in [1, 3, 8] {
            let (mut a, mut b) = (base.clone(), base.clone());
            r.blur(&mut a, rad).ok();
            if candidate.blur(&mut b, rad).is_ok() {
                push("blur", tol.blur, &a, &b);
            }
        }
        for dy in [5, -7, 29, -40] {
            let (mut a, mut b) = (base.clone(), base.clone());
            r.scroll(&mut a, dy).ok();
            if candidate.scroll(&mut b, dy).is_ok() {
                push("scroll", tol.scroll, &a, &b);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "accel_tests.rs"]
mod tests;
