//! Video4Linux2 capture through its ioctls (libc only): what a device is, its formats and sizes, brightness and contrast,
//! and streaming with memory-mapped kernel buffers. The structures are laid out as in `<linux/videodev2.h>`; the request
//! numbers are computed from their sizes the way the kernel's `_IOR`/`_IOWR` macros do (x86 and Arm encoding).

use super::{Capture, Control, Mode, Opened, Opener, Pix, RawFrame};
use super::detect::{parse_caps, CamDevice, Caps};
use std::path::Path;

pub const CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
pub const CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x0000_1000;
pub const CAP_META_CAPTURE: u32 = 0x0080_0000;
pub const CAP_STREAMING: u32 = 0x0400_0000;
pub const CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub const CID_BRIGHTNESS: u32 = 0x0098_0900;
pub const CID_CONTRAST: u32 = 0x0098_0901;

const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const MEMORY_MMAP: u32 = 1;
const FIELD_ANY: u32 = 0;
const FRMSIZE_TYPE_DISCRETE: u32 = 1;
const CTRL_FLAG_DISABLED: u32 = 0x0001;

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}
pub const FMT_MJPEG: u32 = fourcc(b"MJPG");
pub const FMT_JPEG: u32 = fourcc(b"JPEG");
pub const FMT_YUYV: u32 = fourcc(b"YUYV");

pub fn pix_of(f: u32) -> Option<Pix> {
    match f {
        FMT_MJPEG | FMT_JPEG => Some(Pix::Mjpeg),
        FMT_YUYV => Some(Pix::Yuyv),
        _ => None,
    }
}

const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr
}
const fn ior<T>(nr: u32) -> u32 {
    ioc(2, nr, std::mem::size_of::<T>())
}
const fn iow<T>(nr: u32) -> u32 {
    ioc(1, nr, std::mem::size_of::<T>())
}
const fn iowr<T>(nr: u32) -> u32 {
    ioc(3, nr, std::mem::size_of::<T>())
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawCaps([u8; 104]);

#[repr(C)]
#[derive(Clone, Copy)]
struct FmtDesc {
    index: u32,
    type_: u32,
    flags: u32,
    description: [u8; 32],
    pixelformat: u32,
    mbus_code: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union FmtUnion {
    pix: PixFormat,
    raw: [u8; 200],
    _align: *mut libc::c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Format {
    type_: u32,
    fmt: FmtUnion,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FrmSizeEnum {
    index: u32,
    pixel_format: u32,
    type_: u32,
    /// discrete: width, height; stepwise: min_w, max_w, step_w, min_h, max_h, step_h
    size: [u32; 6],
    reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timecode {
    type_: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
union BufM {
    offset: u32,
    userptr: libc::c_ulong,
    planes: *mut libc::c_void,
    fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: Timecode,
    sequence: u32,
    memory: u32,
    m: BufM,
    length: u32,
    reserved2: u32,
    request_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QueryCtrl {
    id: u32,
    type_: u32,
    name: [u8; 32],
    minimum: i32,
    maximum: i32,
    step: i32,
    default_value: i32,
    flags: u32,
    reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CtrlValue {
    id: u32,
    value: i32,
}

const VIDIOC_QUERYCAP: u32 = ior::<RawCaps>(0);
const VIDIOC_ENUM_FMT: u32 = iowr::<FmtDesc>(2);
const VIDIOC_S_FMT: u32 = iowr::<Format>(5);
const VIDIOC_REQBUFS: u32 = iowr::<RequestBuffers>(8);
const VIDIOC_QUERYBUF: u32 = iowr::<Buffer>(9);
const VIDIOC_QBUF: u32 = iowr::<Buffer>(15);
const VIDIOC_DQBUF: u32 = iowr::<Buffer>(17);
const VIDIOC_STREAMON: u32 = iow::<libc::c_int>(18);
const VIDIOC_STREAMOFF: u32 = iow::<libc::c_int>(19);
const VIDIOC_G_CTRL: u32 = iowr::<CtrlValue>(27);
const VIDIOC_S_CTRL: u32 = iowr::<CtrlValue>(28);
const VIDIOC_QUERYCTRL: u32 = iowr::<QueryCtrl>(36);
const VIDIOC_ENUM_FRAMESIZES: u32 = iowr::<FrmSizeEnum>(74);

fn zeroed<T: Copy>() -> T {
    // SAFETY: only used for the plain-data V4L2 structures above, for which all-zero bytes are a valid value.
    unsafe { std::mem::zeroed() }
}

struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the descriptor was opened by us and is closed once.
        unsafe { libc::close(self.0) };
    }
}

impl Fd {
    fn open(path: &Path) -> Result<Fd, String> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| "bad device path".to_string())?;
        // SAFETY: `c` is a valid NUL-terminated path.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(format!("could not open {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        Ok(Fd(fd))
    }

    /// One ioctl, retried when interrupted.
    fn ioctl<T>(&self, req: u32, arg: &mut T) -> std::io::Result<()> {
        loop {
            // SAFETY: `arg` points at a structure of the size encoded in `req`, valid for reading and writing.
            let r = unsafe { libc::ioctl(self.0, req as _, arg as *mut T) };
            if r == 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(e);
            }
        }
    }
}

/// What the device says it is (`VIDIOC_QUERYCAP`), or `None` when it cannot be opened or is not a V4L2 device.
pub fn query_caps(path: &Path) -> Option<Caps> {
    let fd = Fd::open(path).ok()?;
    let mut raw = RawCaps([0; 104]);
    fd.ioctl(VIDIOC_QUERYCAP, &mut raw).ok()?;
    parse_caps(&raw.0)
}

fn formats(fd: &Fd) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..64 {
        let mut d: FmtDesc = zeroed();
        d.index = i;
        d.type_ = BUF_TYPE_VIDEO_CAPTURE;
        if fd.ioctl(VIDIOC_ENUM_FMT, &mut d).is_err() {
            break;
        }
        out.push(d.pixelformat);
    }
    out
}

/// Sizes the camera offers for a format; a stepwise range is offered as the usual sizes inside it.
/// The usual picture sizes inside a stepwise range (the FaceTime HD camera of a MacBook answers one: widths in steps of 8, up to its
/// sensor, 1280x720 on a MacBookPro14,1). A size counts when it is in the range and on the steps.
pub fn stepwise_sizes(minw: u32, maxw: u32, stepw: u32, minh: u32, maxh: u32, steph: u32) -> Vec<(u32, u32)> {
    let on = |v: u32, min: u32, step: u32| step <= 1 || (v - min) % step == 0;
    [(320, 240), (640, 480), (800, 600), (1280, 720), (1920, 1080), (2560, 1440)]
        .into_iter()
        .filter(|(w, h)| (minw..=maxw).contains(w) && (minh..=maxh).contains(h) && on(*w, minw, stepw) && on(*h, minh, steph))
        .collect()
}

fn sizes(fd: &Fd, format: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for i in 0..128 {
        let mut f: FrmSizeEnum = zeroed();
        f.index = i;
        f.pixel_format = format;
        if fd.ioctl(VIDIOC_ENUM_FRAMESIZES, &mut f).is_err() {
            break;
        }
        if f.type_ == FRMSIZE_TYPE_DISCRETE {
            out.push((f.size[0], f.size[1]));
        } else {
            out.extend(stepwise_sizes(f.size[0], f.size[1], f.size[2], f.size[3], f.size[4], f.size[5]));
            break;
        }
    }
    out.sort_by_key(|(w, h)| (*w as u64) * (*h as u64));
    out.dedup();
    out
}

fn control(fd: &Fd, id: u32, name: &str) -> Option<Control> {
    let mut q: QueryCtrl = zeroed();
    q.id = id;
    fd.ioctl(VIDIOC_QUERYCTRL, &mut q).ok()?;
    if q.flags & CTRL_FLAG_DISABLED != 0 || q.maximum <= q.minimum {
        return None;
    }
    let mut v = CtrlValue { id, value: 0 };
    fd.ioctl(VIDIOC_G_CTRL, &mut v).ok()?;
    Some(Control { id, name: name.into(), min: q.minimum, max: q.maximum, step: q.step.max(1), default: q.default_value, value: v.value })
}

struct MappedBuf {
    ptr: *mut libc::c_void,
    len: usize,
}

/// An open, streaming device.
pub struct V4l2Capture {
    fd: Fd,
    bufs: Vec<MappedBuf>,
    mode: Mode,
    streaming: bool,
}

// SAFETY: the mapped buffers belong to this capture alone and are only touched through `&mut self`.
unsafe impl Send for V4l2Capture {}

impl Drop for V4l2Capture {
    fn drop(&mut self) {
        if self.streaming {
            let mut t: libc::c_int = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
            let _ = self.fd.ioctl(VIDIOC_STREAMOFF, &mut t);
        }
        for b in &self.bufs {
            // SAFETY: each region was mapped by us with this length and is unmapped once.
            unsafe { libc::munmap(b.ptr, b.len) };
        }
    }
}

impl V4l2Capture {
    fn start(fd: Fd, mode: Mode) -> Result<V4l2Capture, String> {
        let mut rb = RequestBuffers { count: 4, type_: BUF_TYPE_VIDEO_CAPTURE, memory: MEMORY_MMAP, capabilities: 0, flags: 0, reserved: [0; 3] };
        fd.ioctl(VIDIOC_REQBUFS, &mut rb).map_err(|e| format!("the camera gave no buffers: {e}"))?;
        if rb.count == 0 {
            return Err("the camera gave no buffers".into());
        }
        let mut cap = V4l2Capture { fd, bufs: Vec::new(), mode, streaming: false };
        for i in 0..rb.count {
            let mut b: Buffer = zeroed();
            b.index = i;
            b.type_ = BUF_TYPE_VIDEO_CAPTURE;
            b.memory = MEMORY_MMAP;
            cap.fd.ioctl(VIDIOC_QUERYBUF, &mut b).map_err(|e| format!("buffer {i}: {e}"))?;
            // SAFETY: `offset` is the union member the kernel fills for MMAP buffers.
            let off = unsafe { b.m.offset };
            // SAFETY: mapping a region the driver described (length and offset from QUERYBUF) on our own descriptor.
            let ptr = unsafe { libc::mmap(std::ptr::null_mut(), b.length as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, cap.fd.0, off as libc::off_t) };
            if ptr == libc::MAP_FAILED {
                return Err(format!("could not map buffer {i}: {}", std::io::Error::last_os_error()));
            }
            cap.bufs.push(MappedBuf { ptr, len: b.length as usize });
            cap.fd.ioctl(VIDIOC_QBUF, &mut b).map_err(|e| format!("buffer {i}: {e}"))?;
        }
        let mut t: libc::c_int = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        cap.fd.ioctl(VIDIOC_STREAMON, &mut t).map_err(|e| format!("the camera did not start: {e}"))?;
        cap.streaming = true;
        Ok(cap)
    }
}

impl Capture for V4l2Capture {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn next(&mut self, timeout_ms: u32) -> Result<Option<RawFrame>, String> {
        let mut p = libc::pollfd { fd: self.fd.0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut p, 1, timeout_ms as libc::c_int) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EINTR) { Ok(None) } else { Err(e.to_string()) };
        }
        if r == 0 {
            return Ok(None);
        }
        if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 && p.revents & libc::POLLIN == 0 {
            return Err("the camera was disconnected".into());
        }
        let mut b: Buffer = zeroed();
        b.type_ = BUF_TYPE_VIDEO_CAPTURE;
        b.memory = MEMORY_MMAP;
        match self.fd.ioctl(VIDIOC_DQBUF, &mut b) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => return Ok(None),
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => return Err("the camera was disconnected".into()),
            Err(e) => return Err(format!("reading a frame: {e}")),
        }
        let data = self.bufs.get(b.index as usize).map(|m| {
            let n = (b.bytesused as usize).min(m.len);
            // SAFETY: the buffer is dequeued (ours until queued again) and `n` is within its mapped length.
            unsafe { std::slice::from_raw_parts(m.ptr as *const u8, n) }.to_vec()
        });
        // V4L2_BUF_FLAG_ERROR: a damaged frame; give the buffer back and skip it
        let damaged = b.flags & 0x40 != 0;
        let _ = self.fd.ioctl(VIDIOC_QBUF, &mut b);
        match data {
            Some(d) if !damaged && !d.is_empty() => Ok(Some(RawFrame { pix: self.mode.pix, width: self.mode.width, height: self.mode.height, data: d })),
            _ => Ok(None),
        }
    }

    fn set_control(&mut self, id: u32, value: i32) -> Result<(), String> {
        let mut v = CtrlValue { id, value };
        self.fd.ioctl(VIDIOC_S_CTRL, &mut v).map_err(|e| e.to_string())
    }
}

/// Opens real devices.
pub struct V4l2Opener;

/// The mode to use when none is asked for: MJPEG at 1280×720 if offered, else the largest MJPEG size up to 1920 wide,
/// else YUYV at 640×480 or the nearest smaller.
pub fn default_mode(modes: &[Mode]) -> Option<Mode> {
    let mj: Vec<&Mode> = modes.iter().filter(|m| m.pix == Pix::Mjpeg).collect();
    if let Some(m) = mj.iter().find(|m| (m.width, m.height) == (1280, 720)) {
        return Some(**m);
    }
    if let Some(m) = mj.iter().filter(|m| m.width <= 1920).max_by_key(|m| m.width * m.height) {
        return Some(**m);
    }
    let yu: Vec<&Mode> = modes.iter().filter(|m| m.pix == Pix::Yuyv).collect();
    yu.iter().filter(|m| m.width <= 640).max_by_key(|m| m.width * m.height).or_else(|| yu.iter().min_by_key(|m| m.width * m.height)).map(|m| **m).or_else(|| mj.first().map(|m| **m))
}

impl Opener for V4l2Opener {
    fn open(&self, dev: &CamDevice, want: Option<Mode>) -> Result<Opened, String> {
        let fd = Fd::open(&dev.path)?;
        let mut modes = Vec::new();
        for f in formats(&fd) {
            if let Some(p) = pix_of(f) {
                for (w, h) in sizes(&fd, f) {
                    let m = Mode { pix: p, width: w, height: h };
                    if !modes.contains(&m) {
                        modes.push(m);
                    }
                }
            }
        }
        let mode = want.filter(|m| modes.contains(m)).or_else(|| default_mode(&modes)).ok_or("the camera offers neither MJPEG nor YUYV pictures")?;
        let mut f: Format = zeroed();
        f.type_ = BUF_TYPE_VIDEO_CAPTURE;
        f.fmt.pix = PixFormat { width: mode.width, height: mode.height, pixelformat: if mode.pix == Pix::Mjpeg { FMT_MJPEG } else { FMT_YUYV }, field: FIELD_ANY, ..zeroed() };
        fd.ioctl(VIDIOC_S_FMT, &mut f).map_err(|e| format!("the camera refused {}x{}: {e}", mode.width, mode.height))?;
        // SAFETY: `pix` is the member the kernel fills for a capture format.
        let got = unsafe { f.fmt.pix };
        let mode = Mode { pix: pix_of(got.pixelformat).unwrap_or(mode.pix), width: got.width, height: got.height };
        let controls = [(CID_BRIGHTNESS, "Brightness"), (CID_CONTRAST, "Contrast")].iter().filter_map(|(id, n)| control(&fd, *id, n)).collect();
        let cap = V4l2Capture::start(fd, mode)?;
        Ok(Opened { cap: Box::new(cap), modes, controls })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the facetimehd driver of the MacBook's built-in camera reports (read from its source): one pixel format, YUYV, with a stepwise
    /// size range; no MJPEG. Checks the camera picks 640x480 YUYV by default and offers the sizes the sensor can do.
    #[test]
    fn the_facetime_hd_camera_offers_yuyv_only_with_a_stepwise_range_and_gets_vga_by_default() {
        let sizes = stepwise_sizes(320, 1280, 8, 240, 720, 1);
        assert_eq!(sizes, [(320, 240), (640, 480), (800, 600), (1280, 720)]);
        let modes: Vec<Mode> = sizes.iter().map(|(w, h)| Mode { pix: pix_of(FMT_YUYV).unwrap(), width: *w, height: *h }).collect();
        assert_eq!(default_mode(&modes), Some(Mode { pix: Pix::Yuyv, width: 640, height: 480 }));
        // the 12-inch MacBook's sensor is smaller: nothing past 848x588 (800x600 is too tall)
        assert_eq!(stepwise_sizes(320, 848, 8, 240, 588, 1), [(320, 240), (640, 480)]);
        // sizes that are off the steps are not offered
        assert!(stepwise_sizes(321, 1280, 8, 240, 720, 1).is_empty());
        // the driver's other formats (NV16, YVYU) are not ones this camera app reads
        assert_eq!(pix_of(fourcc(b"NV16")), None);
        assert_eq!(pix_of(fourcc(b"YVYU")), None);
    }

    #[test]
    fn structures_and_request_numbers_match_the_kernel_headers() {
        // the numbers the C macros give on 64-bit Linux
        if std::mem::size_of::<usize>() == 8 {
            assert_eq!(std::mem::size_of::<Format>(), 208);
            assert_eq!(std::mem::size_of::<Buffer>(), 88);
            assert_eq!(VIDIOC_S_FMT, 0xC0D0_5605);
            assert_eq!(VIDIOC_QBUF, 0xC058_560F);
            assert_eq!(VIDIOC_DQBUF, 0xC058_5611);
        }
        assert_eq!(std::mem::size_of::<FmtDesc>(), 64);
        assert_eq!(std::mem::size_of::<FrmSizeEnum>(), 44);
        assert_eq!(std::mem::size_of::<RequestBuffers>(), 20);
        assert_eq!(std::mem::size_of::<QueryCtrl>(), 68);
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xC040_5602);
        assert_eq!(VIDIOC_REQBUFS, 0xC014_5608);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(VIDIOC_STREAMOFF, 0x4004_5613);
        assert_eq!(VIDIOC_G_CTRL, 0xC008_561B);
        assert_eq!(VIDIOC_S_CTRL, 0xC008_561C);
        assert_eq!(VIDIOC_QUERYCTRL, 0xC044_5624);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xC02C_564A);
        assert_eq!(FMT_MJPEG, 0x4750_4A4D);
        assert_eq!(FMT_YUYV, 0x5659_5559);
    }

    #[test]
    fn the_default_mode_prefers_mjpeg_720p() {
        let m = |pix, width, height| Mode { pix, width, height };
        let all = [m(Pix::Yuyv, 640, 480), m(Pix::Yuyv, 1280, 720), m(Pix::Mjpeg, 640, 480), m(Pix::Mjpeg, 1280, 720), m(Pix::Mjpeg, 2560, 1440)];
        assert_eq!(default_mode(&all), Some(m(Pix::Mjpeg, 1280, 720)));
        assert_eq!(default_mode(&[m(Pix::Mjpeg, 640, 480), m(Pix::Mjpeg, 1920, 1080), m(Pix::Mjpeg, 2592, 1944)]), Some(m(Pix::Mjpeg, 1920, 1080)));
        assert_eq!(default_mode(&[m(Pix::Yuyv, 320, 240), m(Pix::Yuyv, 640, 480), m(Pix::Yuyv, 1280, 720)]), Some(m(Pix::Yuyv, 640, 480)));
        assert_eq!(default_mode(&[m(Pix::Yuyv, 1920, 1080)]), Some(m(Pix::Yuyv, 1920, 1080)));
        assert_eq!(default_mode(&[]), None);
        // opening something that is not a camera fails with a message, never a panic
        assert!(query_caps(Path::new("/nonexistent/video0")).is_none());
        let dev = CamDevice { node: "video9".into(), path: "/nonexistent/video9".into(), name: "x".into(), caps: parse_caps(&[0; 104]).unwrap() };
        assert!(V4l2Opener.open(&dev, None).is_err());
    }
}
