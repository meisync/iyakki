//! Linux framebuffer output (`/dev/fb0`). On modern kernels this is the fbdev emulation layer
//! over DRM/KMS (`CONFIG_DRM_FBDEV_EMULATION`), so it works with simpledrm/EFI-GOP, i915,
//! amdgpu, bochs, virtio-gpu and others without any userspace GPU stack.

use crate::safeclamp::SafeClamp;
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

/// Whether the pan in [`Framebuffer::scan_out`] is done (`always`, the default) or left out (`skip`): `iyakki.fbset=skip|always` on the
/// kernel command line, to compare on a monitor whose USB hub drops while the boot screen goes up (KI-077).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FbSet {
    Always,
    Skip,
}

impl FbSet {
    pub fn from_cmdline(cmdline: &str) -> FbSet {
        cmdline.split_whitespace().filter_map(|w| w.strip_prefix("iyakki.fbset=")).last().map_or(FbSet::Always, |v| if v == "skip" { FbSet::Skip } else { FbSet::Always })
    }

    pub fn word(self) -> &'static str {
        if self == FbSet::Skip { "skip" } else { "always" }
    }
}

static FBSET_SKIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read `iyakki.fbset=` once (the splash and the shell both call it; the later call wins and is the same text).
pub fn init_fbset_from_cmdline() -> FbSet {
    let m = FbSet::from_cmdline(&iyakki_storage::cmdline_text());
    FBSET_SKIP.store(m == FbSet::Skip, std::sync::atomic::Ordering::Relaxed);
    m
}

pub fn fbset() -> FbSet {
    if FBSET_SKIP.load(std::sync::atomic::Ordering::Relaxed) { FbSet::Skip } else { FbSet::Always }
}

/// Whether the console must be put into graphics mode: only when it is not in it already (`KDGETMODE` answered 1 = `KD_GRAPHICS`).
pub fn kd_set_needed(current: Option<i32>) -> bool {
    current != Some(1)
}

/// Whether two readings of the screen settings differ in anything that makes a GPU driver set a mode again: size, virtual size,
/// pixel format, timings. The pan offsets and the `activate`/`reserved` words do not count.
fn mode_differs(a: &VarInfo, b: &VarInfo) -> bool {
    let key = |v: &VarInfo| {
        (
            [v.xres, v.yres, v.xres_virtual, v.yres_virtual, v.bits_per_pixel, v.grayscale, v.nonstd, v.pixclock],
            [v.left_margin, v.right_margin, v.upper_margin, v.lower_margin, v.hsync_len, v.vsync_len, v.sync, v.vmode, v.rotate, v.colorspace],
            [v.red, v.green, v.blue, v.transp].map(|f| (f.offset, f.length, f.msb_right)),
        )
    };
    key(a) != key(b)
}

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;
/// `FBIOPAN_DISPLAY`, used only by [`Framebuffer::scan_out`], always with the offsets the screen already has.
const FBIOPAN_DISPLAY: libc::c_ulong = 0x4606;

/// How pixels are packed on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub bpp: u32,
    pub r: (u32, u32), // (offset, length)
    pub g: (u32, u32),
    pub b: (u32, u32),
}

/// One 8-bit channel placed in a field of `f.1` bits at bit `f.0` (fields wider than 8 bits, such as 10-bit
/// channels, are scaled up; a field of 0 bits is left out).
pub fn pack_channel(c: u8, f: (u32, u32)) -> u32 {
    if f.1 == 0 || f.0 >= 32 {
        return 0;
    }
    let v = if f.1 >= 8 { (c as u32) << (f.1 - 8).min(24) } else { (c as u32) >> (8 - f.1) };
    v << f.0
}

/// What the kernel says about a screen (`fb_var_screeninfo` and `fb_fix_screeninfo`, the parts that matter), so the
/// geometry rules can be tested with made-up screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenInfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bpp: u32,
    pub red: (u32, u32),
    pub green: (u32, u32),
    pub blue: (u32, u32),
    pub line_length: u32,
    pub smem_len: u32,
}

/// The usable geometry worked out from a [`ScreenInfo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub pitch: usize,
    pub format: PixelFormat,
    /// Bytes to map.
    pub len: usize,
    /// Something to mention in the boot log (a panned screen, a missing stride, a guessed colour layout).
    pub notes: Vec<String>,
}

/// Work out how to draw on a screen of any stride and pixel layout; refuses only what cannot be drawn on.
pub fn geometry(i: &ScreenInfo) -> Result<Geometry, String> {
    let bpp = match i.bpp {
        15 | 16 => 16,
        24 => 24,
        32 => 32,
        other => return Err(format!("unsupported framebuffer depth {other} bpp")),
    };
    let bytes = (bpp / 8) as usize;
    if i.xres == 0 || i.yres == 0 {
        return Err(format!("framebuffer reports {}x{}", i.xres, i.yres));
    }
    let mut notes = Vec::new();
    // a driver that does not report a stride: assume the rows are packed
    let mut pitch = i.line_length as usize;
    if pitch == 0 {
        pitch = i.xres.max(i.xres_virtual) as usize * bytes;
        notes.push("no stride reported, assuming packed rows".to_string());
    }
    let mut width = i.xres;
    if pitch < width as usize * bytes {
        width = (pitch / bytes) as u32;
        notes.push(format!("stride {pitch} is narrower than {} pixels, using {width}", i.xres));
    }
    let mut height = i.yres;
    let mut len = i.smem_len as usize;
    if len == 0 {
        len = pitch * i.yres_virtual.max(i.yres) as usize;
    }
    if len < pitch * height as usize {
        height = (len / pitch) as u32;
        notes.push(format!("memory holds only {height} rows of {}", i.yres));
    }
    if width == 0 || height == 0 {
        return Err("framebuffer memory too small to draw on".into());
    }
    if i.yoffset != 0 || i.xoffset != 0 {
        notes.push(format!("screen is panned to ({}, {}); drawing at the top-left of the memory", i.xoffset, i.yoffset));
    }
    // colour layout: take what the driver says; when it says nothing, use the usual one for the depth
    let have = |f: (u32, u32)| f.1 > 0;
    let (r, g, b) = if have(i.red) && have(i.green) && have(i.blue) {
        (i.red, i.green, i.blue)
    } else {
        notes.push("no colour layout reported, assuming the usual one".to_string());
        match bpp {
            16 => ((11, 5), (5, 6), (0, 5)),
            _ => ((16, 8), (8, 8), (0, 8)),
        }
    };
    if r.0 + r.1 > bpp || g.0 + g.1 > bpp || b.0 + b.1 > bpp {
        return Err(format!("colour fields do not fit in {bpp} bits"));
    }
    Ok(Geometry { width, height, pitch, format: PixelFormat { bpp, r, g, b }, len, notes })
}

/// Convert premultiplied-RGBA rows to the framebuffer layout. `src` is tightly packed RGBA,
/// `dst` rows are `pitch` bytes apart. 16, 24 and 32 bits per pixel are supported (15 counts as 16).
pub fn blit(src: &[u8], w: usize, h: usize, fmt: PixelFormat, dst: &mut [u8], pitch: usize) -> Result<(), String> {
    let bytes = (fmt.bpp / 8) as usize;
    if !matches!(bytes, 2..=4) {
        return Err(format!("unsupported framebuffer depth {} bpp", fmt.bpp));
    }
    if dst.len() < pitch * h || src.len() < w * h * 4 || pitch < w * bytes {
        return Err("framebuffer smaller than the frame".into());
    }
    let pack = |c: u8, f: (u32, u32)| -> u32 { pack_channel(c, f) };
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
    let pack = |c: u8, f: (u32, u32)| -> u32 { pack_channel(c, f) };
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
    if !matches!(bytes, 2..=4) {
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
    if !matches!(bytes, 2..=4) {
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
    /// The part of the buffer the monitor shows (top-left); the shell lays out and draws for this size.
    pub width: u32,
    pub height: u32,
    /// The whole buffer, which can be larger than the visible part (see `display`).
    pub buffer_width: u32,
    pub buffer_height: u32,
    /// Why `width` x `height` was chosen ("eDP-1 preferred mode").
    pub visible_source: String,
    pub pitch: usize,
    pub format: PixelFormat,
    /// What the kernel reported, for the boot log.
    pub info: ScreenInfo,
    /// Oddities found by [`geometry`].
    pub notes: Vec<String>,
    /// [`sysfs_signature`] of the device, read just before it was opened (`None` without sysfs).
    pub signature: Option<String>,
}

impl Framebuffer {
    /// The one display line for the log: "display: buffer 2880x1800, visible 2560x1600 (eDP-1 preferred mode)".
    pub fn display_line(&self) -> String {
        format!("display: buffer {}x{}, visible {}x{} ({})", self.buffer_width, self.buffer_height, self.width, self.height, self.visible_source)
    }

    /// One line for the boot log: "simpledrm 2560x1440 32 bpp stride 10240 (R16/8 G8/8 B0/8)".
    pub fn describe(&self) -> String {
        let f = self.format;
        format!("{} {}x{} {} bpp stride {} (R{}/{} G{}/{} B{}/{}){}", self.id, self.width, self.height, f.bpp, self.pitch, f.r.0, f.r.1, f.g.0, f.g.1, f.b.0, f.b.1, if self.notes.is_empty() { String::new() } else { format!("; {}", self.notes.join("; ")) })
    }
}

// SAFETY: the mapping is owned by this struct and only accessed through &mut self.
unsafe impl Send for Framebuffer {}

/// Which screen a framebuffer device is right now: the driver's name and the geometry. Changes when a GPU driver takes over
/// from the firmware's screen or a monitor's mode changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub pitch: usize,
}

/// Read the screen's settings without mapping it or changing anything (only the two read-only `FBIOGET_*` queries; IYAKKI
/// never sets a mode or switches consoles, and pans only to the offsets the screen already has, in [`Framebuffer::scan_out`]; so it never causes a mode change itself).
fn query(file: &std::fs::File, path: &str) -> Result<(ScreenInfo, String), String> {
    let fd = file.as_raw_fd();
    let (mut var, mut fix) = (VarInfo::default(), FixInfo::default());
    // SAFETY: the ioctls fill the repr(C) structs above, which match the kernel's layouts.
    unsafe {
        if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var as *mut VarInfo) < 0 || libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix as *mut FixInfo) < 0 {
            return Err(format!("{path}: not a framebuffer ({})", std::io::Error::last_os_error()));
        }
    }
    let info = ScreenInfo {
        xres: var.xres,
        yres: var.yres,
        xres_virtual: var.xres_virtual,
        yres_virtual: var.yres_virtual,
        xoffset: var.xoffset,
        yoffset: var.yoffset,
        bpp: var.bits_per_pixel,
        red: (var.red.offset, var.red.length),
        green: (var.green.offset, var.green.length),
        blue: (var.blue.offset, var.blue.length),
        line_length: fix.line_length,
        smem_len: fix.smem_len,
    };
    Ok((info, String::from_utf8_lossy(&fix.id).trim_end_matches('\0').to_string()))
}

/// Which screen `path` is now (cheap: no mapping), for noticing a GPU driver's takeover quickly.
pub fn identify(path: &str) -> Result<Identity, String> {
    let file = std::fs::OpenOptions::new().read(true).open(path).map_err(|e| format!("{path}: {e}"))?;
    let (info, id) = query(&file, path)?;
    let geo = geometry(&info).map_err(|e| format!("{path}: {e}"))?;
    let vis = crate::display::visible_now((geo.width, geo.height), (info.xres, info.yres));
    Ok(Identity { id, width: vis.width, height: vis.height, pitch: geo.pitch })
}

/// What sysfs says about the screen behind `path` (`/dev/fbN` -> `<sys_graphics>/fbN`): the driver's name, the size, the
/// stride and the depth, in one line. Reading it never opens the device. That matters: when a GPU driver takes over, the
/// firmware's screen (simpledrm) is unplugged, and the last close of its device frees its buffer, which on this kernel
/// (6.12, simpledrm's shmem fbdev) can spin forever in the kernel (`shmem_undo_range`), taking a CPU core and the thread
/// with it. Polling with open/close every 100 ms could be that last close.
pub fn sysfs_signature(sys_graphics: &std::path::Path, path: &str) -> Option<String> {
    let node = std::path::Path::new(path).file_name()?.to_str()?;
    let dir = sys_graphics.join(node);
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|v| v.trim().to_string());
    let name = read("name")?;
    Some(format!("{name} {} stride {} {} bpp", read("virtual_size").unwrap_or_default(), read("stride").unwrap_or_default(), read("bits_per_pixel").unwrap_or_default()))
}

impl Framebuffer {
    /// Has the screen behind `path` changed since this framebuffer was opened (a GPU driver replacing the firmware's
    /// screen, another mode)? Asks sysfs when it can (see [`sysfs_signature`]), else the device itself.
    pub fn screen_changed(&self, path: &str) -> bool {
        match (&self.signature, sysfs_signature(std::path::Path::new("/sys/class/graphics"), path)) {
            (Some(was), Some(now)) => *was != format!("{now} | {}", crate::display::signature(std::path::Path::new("/sys/class/drm"))),
            _ => identify(path).is_ok_and(|now| now != self.identity()),
        }
    }

    /// This screen's [`Identity`].
    pub fn identity(&self) -> Identity {
        Identity { id: self.id.clone(), width: self.width, height: self.height, pitch: self.pitch }
    }

    /// A screen in ordinary memory, for tests of what gets written.
    #[cfg(test)]
    pub(crate) fn in_memory(mem: &mut [u8], width: u32, height: u32) -> Framebuffer {
        Framebuffer::in_memory_as(mem, width, height, width as usize * 4, PixelFormat { bpp: 32, r: (16, 8), g: (8, 8), b: (0, 8) })
    }

    /// Like [`Framebuffer::in_memory`], with another stride and pixel layout.
    #[cfg(test)]
    pub(crate) fn in_memory_as(mem: &mut [u8], width: u32, height: u32, pitch: usize, format: PixelFormat) -> Framebuffer {
        Framebuffer { _file: std::fs::File::open("/dev/null").ok(), id: "test".into(), map: mem.as_mut_ptr(), len: mem.len(), width, height, pitch, format, info: ScreenInfo::default(), notes: vec![], signature: None, buffer_width: width, buffer_height: height, visible_source: "test".into() }
    }

    /// A buffer of `buf` pixels (32 bpp, packed rows) of which the monitor shows the top-left `vis`, in ordinary memory.
    #[cfg(test)]
    pub(crate) fn in_memory_visible(mem: &mut [u8], buf: (u32, u32), vis: (u32, u32)) -> Framebuffer {
        let mut fb = Framebuffer::in_memory_as(mem, buf.0, buf.1, buf.0 as usize * 4, PixelFormat { bpp: 32, r: (16, 8), g: (8, 8), b: (0, 8) });
        fb.width = vis.0;
        fb.height = vis.1;
        fb.visible_source = "test".into();
        fb
    }

    pub fn open(path: &str) -> Result<Framebuffer, String> {
        // read before opening: if the screen changes in between, the next [`Framebuffer::screen_changed`] notices
        let signature = sysfs_signature(std::path::Path::new("/sys/class/graphics"), path).map(|s| format!("{s} | {}", crate::display::signature(std::path::Path::new("/sys/class/drm"))));
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{path}: {e}"))?;
        let fd = file.as_raw_fd();
        let (info, id) = query(&file, path)?;
        let geo = geometry(&info).map_err(|e| format!("{path}: {e}"))?;
        let len = geo.len;
        // SAFETY: mapping the device file shared and writable for its reported length.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0) };
        if map == libc::MAP_FAILED {
            return Err(format!("{path}: mmap failed ({})", std::io::Error::last_os_error()));
        }
        let vis = crate::display::visible_now((geo.width, geo.height), (info.xres, info.yres));
        let mut notes = geo.notes;
        if vis.smaller_than((geo.width, geo.height)) {
            notes.push(format!("monitor shows {}x{} of the {}x{} buffer", vis.width, vis.height, geo.width, geo.height));
            // SAFETY: the mapping above is `len` bytes long and ours.
            let mem = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), len) };
            crate::display::clear_outside(mem, geo.pitch, geo.height as usize, vis.width as usize, vis.height as usize, (geo.format.bpp / 8) as usize);
        }
        Ok(Framebuffer {
            _file: Some(file),
            id,
            map: map.cast(),
            len,
            width: vis.width,
            height: vis.height,
            buffer_width: geo.width,
            buffer_height: geo.height,
            visible_source: vis.source,
            pitch: geo.pitch,
            format: geo.format,
            info,
            notes,
            signature,
        })
    }

    /// The open device's file descriptor (`None` after [`Framebuffer::release`]): for the blank call of `panel.rs`.
    pub fn raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        self._file.as_ref().map(|f| f.as_raw_fd())
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
        let h = self.height as usize;
        self.present_banded_rows(new, old, 0, h)
    }

    /// [`Framebuffer::present_banded`] for the rows `y0..y1` only (the rest of the frame is known to be unchanged, as when only
    /// the browser's page rectangle was painted): nothing outside them is compared or written, and a small band count uses fewer
    /// threads. With an `old` of another size every row is written. Returns the rows written.
    pub fn present_banded_rows(&mut self, new: &[u8], old: &mut Vec<u8>, y0: usize, y1: usize) -> Result<usize, String> {
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
        let (y0, y1) = if full { (0, h) } else { (y0.min(h), y1.min(h)) };
        if y1 <= y0 {
            return Ok(0);
        }
        let rows_all = y1 - y0;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8).min(rows_all.div_ceil(48)).max(1);
        let band = rows_all.div_ceil(threads);
        let (fmt, pitch) = (self.format, self.pitch);
        let Some(dst) = self.bytes().get_mut(y0 * pitch..) else { return Err("screen memory smaller than the frame".into()) };
        let jobs: Vec<_> = dst.chunks_mut(band * pitch).zip(old[y0 * row..y1 * row].chunks_mut(band * row)).zip(new[y0 * row..y1 * row].chunks(band * row)).collect();
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

    /// Write only what differs from `old` inside the given rectangles `(x, y, w, h)` of `new` (the rest of the frame is known
    /// to be unchanged, as when only the highlighted words of a story and a progress bar moved): each row of a rectangle is
    /// compared over the rectangle's columns alone and written over those columns alone, and `old` follows. With an `old` of
    /// another size everything is written. Returns the pixels written.
    pub fn present_rects(&mut self, new: &[u8], old: &mut Vec<u8>, rects: &[(i32, i32, i32, i32)]) -> Result<usize, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if new.len() < w * h * 4 || h == 0 {
            return Err("frame smaller than the screen".into());
        }
        if self.is_released() {
            return Err("the screen is not ours right now".into());
        }
        if old.len() != new.len() {
            return self.present_banded(new, old).map(|rows| rows * w);
        }
        let (fmt, pitch) = (self.format, self.pitch);
        let dst = self.bytes();
        let mut written = 0;
        for &(rx, ry, rw, rh) in rects {
            let (x0, x1) = (rx.clamp_safe(0, w as i32) as usize, (rx + rw).clamp_safe(0, w as i32) as usize);
            let (y0, y1) = (ry.clamp_safe(0, h as i32) as usize, (ry + rh).clamp_safe(0, h as i32) as usize);
            if x1 <= x0 {
                continue;
            }
            for y in y0..y1 {
                let (a, b) = ((y * w + x0) * 4, (y * w + x1) * 4);
                if old[a..b] == new[a..b] {
                    continue;
                }
                // the changed stretch of this row inside the rectangle, not all of it
                let first = (a..b).step_by(4).find(|&i| old[i..i + 4] != new[i..i + 4]).unwrap_or(a);
                let last = (a..b).step_by(4).rev().find(|&i| old[i..i + 4] != new[i..i + 4]).unwrap_or(a) + 4;
                let px = (last - first) / 4;
                let x = (first - y * w * 4) / 4;
                blit_rect(new, w, h, (x, y, px, 1), fmt, dst, pitch)?;
                old[first..last].copy_from_slice(&new[first..last]);
                written += px;
            }
        }
        Ok(written)
    }

    /// Make this framebuffer the picture the monitor shows, without changing its mode or offsets.
    ///
    /// The kernel puts a framebuffer on the monitor only when the text console sets it up, and the console skips that while
    /// it is in graphics mode. IYAKKI switches the console to graphics mode on the firmware's screen (simpledrm), so when a
    /// GPU driver (i915, bochs, amdgpu...) replaces that screen its framebuffer is never shown: the monitor keeps showing what
    /// the firmware and GRUB left in memory (the boot menu with GRUB's black terminal box) while everything drawn goes into
    /// a buffer nobody looks at, until a monitor hotplug happens to set it up. A pan to the current offsets is the fbdev
    /// call that makes the DRM framebuffer helper commit its configuration (same mode, same buffer): the drawn frame
    /// appears. On a screen that already shows this framebuffer it changes nothing. Call it after a whole frame has been
    /// written, so the monitor switches straight to a complete picture: first the written pages are flushed (`fsync`, which
    /// drivers that draw through a shadow buffer, such as bochs or simpledrm, implement by copying it to the screen at once
    /// instead of up to 50 ms later, piece by piece), then the pan.
    pub fn scan_out(&mut self) -> Result<(), String> {
        let Some(file) = self._file.as_ref() else { return Err("the screen is not ours right now".into()) };
        let fd = file.as_raw_fd();
        if fbset() == FbSet::Skip {
            log::info!("fb: FBIOPAN_DISPLAY skipped (iyakki.fbset=skip); the screen settings are not touched");
            // SAFETY: fsync on our own open descriptor
            unsafe { libc::fsync(fd) };
            return Ok(());
        }
        // SAFETY: fsync on our own open descriptor; drivers without a shadow buffer answer EINVAL, which is fine
        unsafe { libc::fsync(fd) };
        let mut var = VarInfo::default();
        // SAFETY: both ioctls take a pointer to a `fb_var_screeninfo`, which `VarInfo` matches; the pan writes back the
        // offsets it applied into the same struct.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var as *mut VarInfo) < 0 {
                return Err(format!("reading the screen settings failed ({})", std::io::Error::last_os_error()));
            }
            let before = var;
            if libc::ioctl(fd, FBIOPAN_DISPLAY as _, &mut var as *mut VarInfo) < 0 {
                return Err(format!("showing the framebuffer failed ({})", std::io::Error::last_os_error()));
            }
            let mut after = VarInfo::default();
            let reread = libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut after as *mut VarInfo) >= 0;
            log::info!("fb: FBIOPAN_DISPLAY done at offset {},{} (iyakki.fbset=always); mode fields {} by it", before.xoffset, before.yoffset, if reread && mode_differs(&before, &after) { "CHANGED" } else { "unchanged" });
        }
        Ok(())
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
    fn the_fbset_option_and_the_skip_decisions_are_pure() {
        assert_eq!(FbSet::from_cmdline("quiet iyakki.fbset=skip"), FbSet::Skip);
        assert_eq!(FbSet::from_cmdline("iyakki.fbset=skip iyakki.fbset=always"), FbSet::Always);
        assert_eq!(FbSet::from_cmdline("quiet"), FbSet::Always);
        assert_eq!(FbSet::Skip.word(), "skip");
        assert!(!kd_set_needed(Some(1)), "already graphics: no KDSETMODE");
        assert!(kd_set_needed(Some(0)) && kd_set_needed(None));
        let a = VarInfo { xres: 1920, yres: 1080, xres_virtual: 1920, yres_virtual: 1080, bits_per_pixel: 32, ..Default::default() };
        let mut b = a;
        b.xoffset = 0;
        b.yoffset = 7;
        b.activate = 128;
        assert!(!mode_differs(&a, &b), "offsets and activate flags are not a mode");
        b.bits_per_pixel = 16;
        assert!(mode_differs(&a, &b));
        let mut c = a;
        c.red.length = 8;
        assert!(mode_differs(&a, &c));
        c = a;
        c.pixclock = 6734;
        assert!(mode_differs(&a, &c));
    }

    #[test]
    fn a_released_screen_refuses_writes_and_can_be_released_twice() {
        let path = std::env::temp_dir().join(format!("iyakki-fb-test-{}", std::process::id()));
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        // SAFETY: mapping the 4096-byte test file shared and writable.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        assert_ne!(map, libc::MAP_FAILED);
        let mut fb = Framebuffer { _file: Some(file), id: "test".into(), map: map.cast(), len: 4096, width: 4, height: 4, buffer_width: 4, buffer_height: 4, visible_source: String::new(), pitch: 16, format: XRGB, info: ScreenInfo::default(), notes: vec![], signature: None };
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
        assert!(blit(&src, 2, 2, PixelFormat { bpp: 8, ..XRGB }, &mut vec![0u8; 100], 8).is_err());
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
        let fb_new = |mem: &mut Vec<u8>| Framebuffer { _file: Some(std::fs::File::open("/dev/null").unwrap()), id: String::new(), map: mem.as_mut_ptr(), len: mem.len(), width: w as u32, height: h as u32, buffer_width: w as u32, buffer_height: h as u32, visible_source: String::new(), pitch: w * 4, format: XRGB, info: ScreenInfo::default(), notes: vec![], signature: None };
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
    fn presenting_a_row_range_equals_presenting_the_whole_frame_and_touches_nothing_else() {
        // tall enough that several bands (threads) are used for the range
        let (w, h) = (6usize, 400usize);
        let mk = |mem: &mut Vec<u8>| Framebuffer { _file: Some(std::fs::File::open("/dev/null").unwrap()), id: String::new(), map: mem.as_mut_ptr(), len: mem.len(), width: w as u32, height: h as u32, buffer_width: w as u32, buffer_height: h as u32, visible_source: String::new(), pitch: w * 4, format: XRGB, info: ScreenInfo::default(), notes: vec![], signature: None };
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        };
        let first: Vec<u8> = (0..w * 4 * h).map(|_| rnd()).collect();
        let (mut mem_a, mut mem_b) = (vec![0u8; w * 4 * h], vec![0u8; w * 4 * h]);
        let (mut old_a, mut old_b) = (Vec::new(), Vec::new());
        mk(&mut mem_a).present_banded(&first, &mut old_a).unwrap();
        mk(&mut mem_b).present_banded(&first, &mut old_b).unwrap();
        for (y0, y1) in [(100usize, 300usize), (0, 1), (399, 400), (50, 50), (390, 900), (7, 130)] {
            let mut next = old_a.clone();
            for y in y0.min(h)..y1.min(h) {
                for b in &mut next[y * w * 4..(y + 1) * w * 4] {
                    *b = rnd();
                }
            }
            let rows = mk(&mut mem_a).present_banded_rows(&next, &mut old_a, y0, y1).unwrap();
            assert!(rows <= y1.min(h).saturating_sub(y0.min(h)), "{y0}..{y1}: only rows of the range are written, got {rows}");
            mk(&mut mem_b).present_banded(&next, &mut old_b).unwrap();
            assert!(mem_a == mem_b, "{y0}..{y1}: the screen is the same as after a whole-frame present");
            assert!(old_a == next && old_b == next, "{y0}..{y1}: the old frame follows");
        }
        // rows outside the range are not even looked at: a change there stays unwritten until a present that covers it
        let mut changed = old_a.clone();
        changed[0] ^= 0xFF;
        let before = mem_a.clone();
        assert_eq!(mk(&mut mem_a).present_banded_rows(&changed, &mut old_a, 100, 120).unwrap(), 0);
        assert!(mem_a == before && old_a[0] != changed[0]);
    }

    fn info(w: u32, h: u32, bpp: u32, stride: u32) -> ScreenInfo {
        let (r, g, b) = match bpp {
            16 => ((11, 5), (5, 6), (0, 5)),
            _ => ((16, 8), (8, 8), (0, 8)),
        };
        ScreenInfo { xres: w, yres: h, xres_virtual: w, yres_virtual: h, bpp, red: r, green: g, blue: b, line_length: stride, smem_len: stride * h, ..Default::default() }
    }

    #[test]
    fn odd_screens_get_a_geometry_or_a_clear_refusal() {
        // a Mac mini's native 2560x1440 with a padded stride, a plain 1024x768, a 16 bit panel
        let g = geometry(&info(2560, 1440, 32, 10240)).unwrap();
        assert_eq!((g.width, g.height, g.pitch, g.len), (2560, 1440, 10240, 10240 * 1440));
        assert!(g.notes.is_empty());
        let g = geometry(&info(1366, 768, 32, 5504)).unwrap();
        assert_eq!((g.width, g.pitch), (1366, 5504), "stride larger than the width is kept");
        assert_eq!(geometry(&info(800, 600, 16, 1600)).unwrap().format.bpp, 16);
        assert_eq!(geometry(&info(800, 600, 24, 2400)).unwrap().format.bpp, 24);
        // no stride reported: packed rows are assumed and said so
        let g = geometry(&info(1280, 720, 32, 0)).unwrap();
        assert_eq!(g.pitch, 1280 * 4);
        assert!(!g.notes.is_empty());
        // a stride narrower than the width shrinks the width instead of writing past the row
        let g = geometry(&info(1000, 100, 32, 3200)).unwrap();
        assert_eq!(g.width, 800);
        // memory shorter than the rows
        let mut i = info(100, 100, 32, 400);
        i.smem_len = 400 * 60;
        assert_eq!(geometry(&i).unwrap().height, 60);
        // no colour layout: the usual one for the depth
        let mut i = info(100, 100, 32, 400);
        i.red = (0, 0);
        assert_eq!(geometry(&i).unwrap().format.r, (16, 8));
        // refusals
        assert!(geometry(&info(0, 100, 32, 400)).is_err());
        assert!(geometry(&info(100, 100, 8, 100)).is_err());
        let mut i = info(100, 100, 32, 400);
        i.red = (28, 8);
        assert!(geometry(&i).is_err(), "field past the end of the pixel");
        let mut i = info(100, 100, 32, 400);
        i.yoffset = 100;
        assert!(geometry(&i).unwrap().notes.iter().any(|n| n.contains("panned")));
    }

    #[test]
    fn channels_are_scaled_to_the_field_width() {
        assert_eq!(pack_channel(255, (16, 8)), 0xFF0000);
        assert_eq!(pack_channel(255, (20, 10)), 0x3FC << 20, "a 10 bit field gets the 8 bit value scaled up");
        assert_eq!(pack_channel(255, (11, 5)), 0x1F << 11);
        assert_eq!(pack_channel(255, (0, 0)), 0, "a missing field is left out");
    }

    #[test]
    fn a_24_bit_and_a_bgr_screen_are_drawn_in_their_own_order() {
        let src = [255, 0, 0, 255, 0, 0, 255, 255]; // red, blue
        let mut dst = vec![0u8; 6];
        blit(&src, 2, 1, PixelFormat { bpp: 24, r: (16, 8), g: (8, 8), b: (0, 8) }, &mut dst, 6).unwrap();
        assert_eq!(dst, [0, 0, 255, 255, 0, 0], "BGR bytes, 3 per pixel");
        let mut d = vec![0u8; 8];
        let rgbx = PixelFormat { bpp: 32, r: (0, 8), g: (8, 8), b: (16, 8) };
        blit(&src, 2, 1, rgbx, &mut d, 8).unwrap();
        assert_eq!(&d[..4], &[255, 0, 0, 0], "RGBX");
        assert_eq!(&d[4..7], &[0, 0, 255]);
        // a stride bigger than the row leaves the padding alone, at every size the Mac may report
        for (w, stride) in [(1usize, 8usize), (2560, 10240 + 64), (1920, 7680)] {
            let src = vec![9u8; w * 4];
            let mut dst = vec![0xAAu8; stride];
            blit(&src, w, 1, XRGB, &mut dst, stride).unwrap();
            assert!(dst[w * 4..].iter().all(|b| *b == 0xAA));
        }
    }

    #[test]
    fn the_sysfs_signature_follows_a_gpu_driver_taking_over_without_opening_the_device() {
        let d = std::env::temp_dir().join(format!("iyakki-fb-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("fb0")).unwrap();
        assert_eq!(sysfs_signature(&d, "/dev/fb0"), None, "no name: no signature (the caller asks the device)");
        for (f, v) in [("name", "simpledrmdrmfb\n"), ("virtual_size", "1920,1080\n"), ("stride", "7680\n"), ("bits_per_pixel", "32\n")] {
            std::fs::write(d.join("fb0").join(f), v).unwrap();
        }
        let firmware = sysfs_signature(&d, "/dev/fb0").unwrap();
        assert_eq!(firmware, "simpledrmdrmfb 1920,1080 stride 7680 32 bpp");
        std::fs::write(d.join("fb0/name"), "i915drmfb\n").unwrap();
        assert_ne!(sysfs_signature(&d, "/dev/fb0").unwrap(), firmware, "same size, another driver");
        assert_eq!(sysfs_signature(&d, "/dev/fb1"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn opening_a_non_framebuffer_fails_cleanly() {
        assert!(Framebuffer::open("/dev/null").is_err());
        assert!(Framebuffer::open("/no/such/fb").is_err());
    }

    #[test]
    fn presenting_rectangles_writes_only_the_changed_stretch_of_each_row_inside_them() {
        let (w, h) = (16usize, 10usize);
        let mut mem = vec![0xEEu8; w * 4 * h];
        let mk = |mem: &mut Vec<u8>| Framebuffer { _file: Some(std::fs::File::open("/dev/null").unwrap()), id: String::new(), map: mem.as_mut_ptr(), len: mem.len(), width: w as u32, height: h as u32, buffer_width: w as u32, buffer_height: h as u32, visible_source: String::new(), pitch: w * 4, format: XRGB, info: ScreenInfo::default(), notes: vec![], signature: None };
        let mut old = vec![10u8; w * 4 * h];
        let mut new = old.clone();
        // pixels changed: (5,2), (7,2) inside the rectangle; (1,2) in the same row but outside it; (6,8) in another rectangle; (12,5) in none
        for (x, y) in [(5usize, 2usize), (7, 2), (1, 2), (6, 8), (12, 5)] {
            new[(y * w + x) * 4] = 99;
        }
        let rects = [(4, 1, 6, 3), (5, 7, 4, 2)];
        assert_eq!(mk(&mut mem).present_rects(&new, &mut old, &rects).unwrap(), 3 + 1, "pixels 5..=7 of row 2 (the stretch between the two changes) and one in the other rectangle");
        let at = |x: usize, y: usize| mem[(y * w + x) * 4..(y * w + x) * 4 + 4].to_vec();
        assert_eq!(at(5, 2)[2], 99, "BGRX: red is the third byte");
        assert_eq!(at(7, 2)[2], 99);
        assert_eq!(at(6, 8)[2], 99);
        assert_eq!(at(1, 2), vec![0xEE; 4], "outside the rectangle: not written");
        assert_eq!(at(12, 5), vec![0xEE; 4]);
        assert_eq!(old[(2 * w + 5) * 4], 99, "the old frame follows inside the rectangles");
        assert_eq!(old[(2 * w + 1) * 4], 10, "and nowhere else");
        // nothing changed in them any more: nothing is written
        assert_eq!(mk(&mut mem).present_rects(&new, &mut old, &rects).unwrap(), 0);
        // an old frame of another size: the whole frame is written
        let mut other = Vec::new();
        assert_eq!(mk(&mut mem).present_rects(&new, &mut other, &rects).unwrap(), h * w);
        assert_eq!(other, new);
    }
}
