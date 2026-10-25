//! Kernel mode setting without a library: the DRM ioctls written from `include/uapi/drm/drm_mode.h` (Linux 6.12), behind a
//! [`DrmBackend`] trait so everything but the final syscalls can be tested with [`FakeDrm`].
//!
//! The real backend ([`RealDrm`]) runs only on a device: it refuses to open anything in tests, when the shell runs inside a
//! window (a desktop owns the screen), and when another program is the DRM master. Nothing here is ever started on the
//! development computer. See `docs/MULTI-DISPLAY.md` section 4.

use std::collections::HashMap;

pub const DISPLAY_MODE_LEN: usize = 32;
pub const MODE_TYPE_PREFERRED: u32 = 1 << 3;
pub const CONNECTED: u32 = 1;
/// `DRM_FORMAT_XRGB8888`: 'X' 'R' '2' '4'.
pub const FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub const CAP_DUMB_BUFFER: u64 = 1;

// ------------------------------------------------------------------------------------------------ the kernel's structs

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub typ: u32,
    pub name: [u8; DISPLAY_MODE_LEN],
}

impl Default for ModeInfo {
    fn default() -> Self {
        ModeInfo { clock: 0, hdisplay: 0, hsync_start: 0, hsync_end: 0, htotal: 0, hskew: 0, vdisplay: 0, vsync_start: 0, vsync_end: 0, vtotal: 0, vscan: 0, vrefresh: 0, flags: 0, typ: 0, name: [0; DISPLAY_MODE_LEN] }
    }
}

impl ModeInfo {
    pub fn size(&self) -> (u32, u32) {
        (self.hdisplay as u32, self.vdisplay as u32)
    }
    pub fn preferred(&self) -> bool {
        self.typ & MODE_TYPE_PREFERRED != 0
    }
    /// A made-up mode for tests and the fake backend.
    pub fn make(w: u16, h: u16, hz: u32, preferred: bool) -> ModeInfo {
        let mut m = ModeInfo { hdisplay: w, vdisplay: h, htotal: w + 160, vtotal: h + 40, hsync_start: w + 48, hsync_end: w + 80, vsync_start: h + 3, vsync_end: h + 8, vrefresh: hz, typ: if preferred { MODE_TYPE_PREFERRED } else { 0 }, ..Default::default() };
        m.clock = ((m.htotal as u64 * m.vtotal as u64 * hz as u64) / 1000) as u32;
        let name = format!("{w}x{h}");
        m.name[..name.len().min(DISPLAY_MODE_LEN - 1)].copy_from_slice(&name.as_bytes()[..name.len().min(DISPLAY_MODE_LEN - 1)]);
        m
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CardRes {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GetConnector {
    pub encoders_ptr: u64,
    pub modes_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_modes: u32,
    pub count_props: u32,
    pub count_encoders: u32,
    pub encoder_id: u32,
    pub connector_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GetEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CrtcCmd {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: ModeInfo,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CreateDumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DestroyDumb {
    pub handle: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbCmd2 {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub flags: u32,
    pub handles: [u32; 4],
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifier: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PageFlip {
    pub crtc_id: u32,
    pub fb_id: u32,
    pub flags: u32,
    pub reserved: u32,
    pub user_data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GetCap {
    pub capability: u64,
    pub value: u64,
}

// ioctl numbers: `_IOC(dir, 'd', nr, size)`; dir 3 = read+write, 0 = none
const fn iowr(nr: u64, size: usize) -> u64 {
    (3u64 << 30) | ((size as u64) << 16) | ((b'd' as u64) << 8) | nr
}
const fn io(nr: u64) -> u64 {
    ((b'd' as u64) << 8) | nr
}
pub const IOCTL_SET_MASTER: u64 = io(0x1e);
pub const IOCTL_DROP_MASTER: u64 = io(0x1f);
pub const IOCTL_GET_CAP: u64 = iowr(0x0c, std::mem::size_of::<GetCap>());
pub const IOCTL_GETRESOURCES: u64 = iowr(0xA0, std::mem::size_of::<CardRes>());
pub const IOCTL_GETCRTC: u64 = iowr(0xA1, std::mem::size_of::<CrtcCmd>());
pub const IOCTL_SETCRTC: u64 = iowr(0xA2, std::mem::size_of::<CrtcCmd>());
pub const IOCTL_GETENCODER: u64 = iowr(0xA6, std::mem::size_of::<GetEncoder>());
pub const IOCTL_GETCONNECTOR: u64 = iowr(0xA7, std::mem::size_of::<GetConnector>());
pub const IOCTL_RMFB: u64 = iowr(0xAF, 4);
pub const IOCTL_PAGE_FLIP: u64 = iowr(0xB0, std::mem::size_of::<PageFlip>());
pub const IOCTL_CREATE_DUMB: u64 = iowr(0xB2, std::mem::size_of::<CreateDumb>());
pub const IOCTL_MAP_DUMB: u64 = iowr(0xB3, std::mem::size_of::<MapDumb>());
pub const IOCTL_DESTROY_DUMB: u64 = iowr(0xB4, std::mem::size_of::<DestroyDumb>());
pub const IOCTL_ADDFB2: u64 = iowr(0xB8, std::mem::size_of::<FbCmd2>());

/// The name a connector has in sysfs (`card0-eDP-1`): the kernel's type table, then the per-type number.
pub fn connector_name(typ: u32, type_id: u32) -> String {
    let t = match typ {
        1 => "VGA",
        2 => "DVI-I",
        3 => "DVI-D",
        4 => "DVI-A",
        5 => "Composite",
        6 => "SVIDEO",
        7 => "LVDS",
        8 => "Component",
        9 => "DIN",
        10 => "DP",
        11 => "HDMI-A",
        12 => "HDMI-B",
        13 => "TV",
        14 => "eDP",
        15 => "Virtual",
        16 => "DSI",
        17 => "DPI",
        18 => "Writeback",
        19 => "SPI",
        20 => "USB",
        _ => "Unknown",
    };
    format!("{t}-{type_id}")
}

// ------------------------------------------------------------------------------------------------ the trait

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    pub id: u32,
    pub name: String,
    pub connected: bool,
    pub modes: Vec<ModeInfo>,
    /// The CRTCs (indexes into [`Resources::crtcs`]) its encoders can drive, as a bit mask over those indexes.
    pub possible_crtcs: u32,
    /// The CRTC it is on now, 0 for none.
    pub current_crtc: u32,
    pub mm: (u32, u32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    pub crtcs: Vec<u32>,
    pub connectors: Vec<u32>,
    pub max_size: (u32, u32),
}

/// A dumb buffer registered as a framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub handle: u32,
    pub fb_id: u32,
    pub w: u32,
    pub h: u32,
    pub pitch: u32,
    pub size: u64,
}

pub trait DrmBackend: Send {
    fn name(&self) -> String;
    fn resources(&mut self) -> Result<Resources, String>;
    fn connector(&mut self, id: u32) -> Result<Connector, String>;
    fn create_buffer(&mut self, w: u32, h: u32) -> Result<Buffer, String>;
    /// The buffer's pixels (XRGB8888, `pitch` bytes per row).
    fn pixels(&mut self, b: &Buffer) -> Result<&mut [u8], String>;
    fn destroy_buffer(&mut self, b: Buffer);
    /// Show `fb` on `crtc` from `connectors` in `mode` (`None` switches the CRTC off).
    fn set_crtc(&mut self, crtc: u32, fb: u32, connectors: &[u32], mode: Option<&ModeInfo>) -> Result<(), String>;
    fn page_flip(&mut self, crtc: u32, fb: u32) -> Result<(), String>;
}

// ------------------------------------------------------------------------------------------------ the fake

/// A made-up graphics card: the connectors you give it, buffers in ordinary memory, and a record of what was asked.
pub struct FakeDrm {
    pub crtcs: Vec<u32>,
    pub connectors: Vec<Connector>,
    next: u32,
    bufs: HashMap<u32, Vec<u8>>,
    pub fb_of: HashMap<u32, Buffer>,
    /// What each CRTC shows now: (fb, connectors, mode size).
    pub shown: HashMap<u32, (u32, Vec<u32>, (u32, u32))>,
    pub log: Vec<String>,
    /// Make the next call of this kind fail ("set_crtc", "create_buffer", "page_flip", "resources").
    pub fail: Option<&'static str>,
    pub flips: usize,
    pub pageflip_supported: bool,
}

impl FakeDrm {
    pub fn new(crtc_count: u32, connectors: Vec<Connector>) -> FakeDrm {
        FakeDrm { crtcs: (1..=crtc_count).map(|i| 40 + i).collect(), connectors, next: 100, bufs: HashMap::new(), fb_of: HashMap::new(), shown: HashMap::new(), log: Vec::new(), fail: None, flips: 0, pageflip_supported: true }
    }

    /// A connector with a list of (w, h, hz) modes, the first being preferred.
    pub fn conn(id: u32, name: &str, connected: bool, modes: &[(u16, u16, u32)], possible: u32) -> Connector {
        Connector { id, name: name.into(), connected, modes: modes.iter().enumerate().map(|(i, m)| ModeInfo::make(m.0, m.1, m.2, i == 0)).collect(), possible_crtcs: possible, current_crtc: 0, mm: (300, 200) }
    }

    fn check(&mut self, what: &'static str) -> Result<(), String> {
        if self.fail == Some(what) {
            self.fail = None;
            return Err(format!("{what} failed (made up)"));
        }
        Ok(())
    }
}

impl DrmBackend for FakeDrm {
    fn name(&self) -> String {
        "fake".into()
    }
    fn resources(&mut self) -> Result<Resources, String> {
        self.check("resources")?;
        Ok(Resources { crtcs: self.crtcs.clone(), connectors: self.connectors.iter().map(|c| c.id).collect(), max_size: (8192, 8192) })
    }
    fn connector(&mut self, id: u32) -> Result<Connector, String> {
        self.connectors.iter().find(|c| c.id == id).cloned().ok_or_else(|| format!("no connector {id}"))
    }
    fn create_buffer(&mut self, w: u32, h: u32) -> Result<Buffer, String> {
        self.check("create_buffer")?;
        self.next += 1;
        let pitch = (w * 4).div_ceil(64) * 64;
        let b = Buffer { handle: self.next, fb_id: self.next + 1000, w, h, pitch, size: pitch as u64 * h as u64 };
        self.bufs.insert(b.handle, vec![0; b.size as usize]);
        self.fb_of.insert(b.fb_id, b);
        self.log.push(format!("create_buffer {w}x{h} pitch {pitch}"));
        Ok(b)
    }
    fn pixels(&mut self, b: &Buffer) -> Result<&mut [u8], String> {
        self.bufs.get_mut(&b.handle).map(|v| v.as_mut_slice()).ok_or_else(|| "no such buffer".to_string())
    }
    fn destroy_buffer(&mut self, b: Buffer) {
        self.bufs.remove(&b.handle);
        self.fb_of.remove(&b.fb_id);
        self.log.push(format!("destroy_buffer {}", b.handle));
    }
    fn set_crtc(&mut self, crtc: u32, fb: u32, connectors: &[u32], mode: Option<&ModeInfo>) -> Result<(), String> {
        if mode.is_some() {
            self.check("set_crtc")?;
        }
        if !self.crtcs.contains(&crtc) {
            return Err(format!("no CRTC {crtc}"));
        }
        match mode {
            Some(m) => {
                let b = self.fb_of.get(&fb).ok_or("no such framebuffer")?;
                if (b.w, b.h) != m.size() && (b.w < m.size().0 || b.h < m.size().1) {
                    return Err("the buffer is smaller than the mode".into());
                }
                self.shown.insert(crtc, (fb, connectors.to_vec(), m.size()));
                self.log.push(format!("set_crtc {crtc} fb {fb} {}x{}", m.hdisplay, m.vdisplay));
            }
            None => {
                self.shown.remove(&crtc);
                self.log.push(format!("set_crtc {crtc} off"));
            }
        }
        Ok(())
    }
    fn page_flip(&mut self, crtc: u32, fb: u32) -> Result<(), String> {
        self.check("page_flip")?;
        if !self.pageflip_supported {
            return Err("page flips are not supported".into());
        }
        let e = self.shown.get_mut(&crtc).ok_or("that CRTC shows nothing")?;
        e.0 = fb;
        self.flips += 1;
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ the real thing

/// Why the real backend was not opened (the shell logs this one line and keeps the framebuffer path).
pub fn refuse_reason(windowed: bool) -> Option<&'static str> {
    if cfg!(test) {
        return Some("tests never open /dev/dri");
    }
    if windowed {
        return Some("the shell runs in a window (a desktop owns the screen)");
    }
    None
}

pub struct RealDrm {
    fd: i32,
    maps: HashMap<u32, (*mut u8, usize)>,
    master: bool,
    card: String,
}

// SAFETY: the mappings are owned by this struct and only reached through &mut self.
unsafe impl Send for RealDrm {}

fn ioc<T>(fd: i32, req: u64, arg: &mut T) -> Result<(), i32> {
    loop {
        // SAFETY: `arg` is a repr(C) struct that matches the kernel's layout for `req` (sizes are asserted in the tests).
        let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T) };
        if r >= 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if e != libc::EINTR && e != libc::EAGAIN {
            return Err(e);
        }
    }
}

impl RealDrm {
    /// Open the card and become its master. Fails (and the caller keeps the framebuffer) when not allowed to, when another
    /// program is the master, or when the card has no dumb buffers.
    pub fn open(path: &str, windowed: bool) -> Result<RealDrm, String> {
        if let Some(why) = refuse_reason(windowed) {
            return Err(why.to_string());
        }
        let c = std::ffi::CString::new(path).map_err(|_| "bad path".to_string())?;
        // SAFETY: plain open of a device node.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(format!("{path}: {}", std::io::Error::last_os_error()));
        }
        let mut d = RealDrm { fd, maps: HashMap::new(), master: false, card: path.to_string() };
        let mut cap = GetCap { capability: CAP_DUMB_BUFFER, value: 0 };
        if ioc(fd, IOCTL_GET_CAP, &mut cap).is_err() || cap.value == 0 {
            return Err(format!("{path}: no dumb buffers"));
        }
        // SAFETY: an ioctl without argument.
        if unsafe { libc::ioctl(fd, IOCTL_SET_MASTER as _) } < 0 {
            let e = std::io::Error::last_os_error();
            return Err(format!("{path}: cannot become the DRM master ({e}); another program may hold the screen"));
        }
        d.master = true;
        Ok(d)
    }
}

impl Drop for RealDrm {
    fn drop(&mut self) {
        for (_, (p, len)) in self.maps.drain() {
            // SAFETY: a mapping this struct made and has not unmapped.
            unsafe { libc::munmap(p.cast(), len) };
        }
        if self.master {
            // SAFETY: an ioctl without argument.
            unsafe { libc::ioctl(self.fd, IOCTL_DROP_MASTER as _) };
        }
        // SAFETY: our own descriptor.
        unsafe { libc::close(self.fd) };
    }
}

fn err(what: &str, e: i32) -> String {
    format!("{what}: {}", std::io::Error::from_raw_os_error(e))
}

impl DrmBackend for RealDrm {
    fn name(&self) -> String {
        self.card.clone()
    }

    fn resources(&mut self) -> Result<Resources, String> {
        let mut r = CardRes::default();
        ioc(self.fd, IOCTL_GETRESOURCES, &mut r).map_err(|e| err("GETRESOURCES", e))?;
        // the second call fills the arrays; counts may change between calls, so ask again with room to spare
        let (mut crtcs, mut conns, mut encs, mut fbs) = (vec![0u32; r.count_crtcs as usize + 4], vec![0u32; r.count_connectors as usize + 4], vec![0u32; r.count_encoders as usize + 4], vec![0u32; r.count_fbs as usize + 4]);
        let mut r2 = CardRes { crtc_id_ptr: crtcs.as_mut_ptr() as u64, connector_id_ptr: conns.as_mut_ptr() as u64, encoder_id_ptr: encs.as_mut_ptr() as u64, fb_id_ptr: fbs.as_mut_ptr() as u64, count_crtcs: crtcs.len() as u32, count_connectors: conns.len() as u32, count_encoders: encs.len() as u32, count_fbs: fbs.len() as u32, ..Default::default() };
        ioc(self.fd, IOCTL_GETRESOURCES, &mut r2).map_err(|e| err("GETRESOURCES", e))?;
        crtcs.truncate((r2.count_crtcs as usize).min(crtcs.len()));
        conns.truncate((r2.count_connectors as usize).min(conns.len()));
        Ok(Resources { crtcs, connectors: conns, max_size: (r2.max_width, r2.max_height) })
    }

    fn connector(&mut self, id: u32) -> Result<Connector, String> {
        let mut c = GetConnector { connector_id: id, ..Default::default() };
        ioc(self.fd, IOCTL_GETCONNECTOR, &mut c).map_err(|e| err("GETCONNECTOR", e))?;
        let mut modes = vec![ModeInfo::default(); c.count_modes as usize + 2];
        let mut encs = vec![0u32; c.count_encoders as usize + 2];
        let mut c2 = GetConnector { connector_id: id, modes_ptr: modes.as_mut_ptr() as u64, count_modes: modes.len() as u32, encoders_ptr: encs.as_mut_ptr() as u64, count_encoders: encs.len() as u32, ..Default::default() };
        ioc(self.fd, IOCTL_GETCONNECTOR, &mut c2).map_err(|e| err("GETCONNECTOR", e))?;
        modes.truncate((c2.count_modes as usize).min(modes.len()));
        encs.truncate((c2.count_encoders as usize).min(encs.len()));
        modes.retain(|m| m.hdisplay > 0 && m.vdisplay > 0);
        // which CRTCs can drive it: the union over its encoders; the mask is over the CRTC list
        let mut possible = 0;
        for e in encs {
            let mut en = GetEncoder { encoder_id: e, ..Default::default() };
            if ioc(self.fd, IOCTL_GETENCODER, &mut en).is_ok() {
                possible |= en.possible_crtcs;
            }
        }
        let mut current = 0;
        if c2.encoder_id != 0 {
            let mut en = GetEncoder { encoder_id: c2.encoder_id, ..Default::default() };
            if ioc(self.fd, IOCTL_GETENCODER, &mut en).is_ok() {
                current = en.crtc_id;
            }
        }
        Ok(Connector { id, name: connector_name(c2.connector_type, c2.connector_type_id), connected: c2.connection == CONNECTED, modes, possible_crtcs: possible, current_crtc: current, mm: (c2.mm_width, c2.mm_height) })
    }

    fn create_buffer(&mut self, w: u32, h: u32) -> Result<Buffer, String> {
        let mut c = CreateDumb { width: w, height: h, bpp: 32, ..Default::default() };
        ioc(self.fd, IOCTL_CREATE_DUMB, &mut c).map_err(|e| err("CREATE_DUMB", e))?;
        let fail = |this: &mut RealDrm, handle: u32, what: String| -> String {
            let mut d = DestroyDumb { handle };
            let _ = ioc(this.fd, IOCTL_DESTROY_DUMB, &mut d);
            what
        };
        let mut f = FbCmd2 { width: w, height: h, pixel_format: FORMAT_XRGB8888, ..Default::default() };
        f.handles[0] = c.handle;
        f.pitches[0] = c.pitch;
        if let Err(e) = ioc(self.fd, IOCTL_ADDFB2, &mut f) {
            return Err(fail(self, c.handle, err("ADDFB2", e)));
        }
        let mut m = MapDumb { handle: c.handle, ..Default::default() };
        if let Err(e) = ioc(self.fd, IOCTL_MAP_DUMB, &mut m) {
            let mut fb = f.fb_id;
            let _ = ioc(self.fd, IOCTL_RMFB, &mut fb);
            return Err(fail(self, c.handle, err("MAP_DUMB", e)));
        }
        // SAFETY: mapping the buffer object at the offset the kernel gave for it.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), c.size as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, self.fd, m.offset as _) };
        if p == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            let mut fb = f.fb_id;
            let _ = ioc(self.fd, IOCTL_RMFB, &mut fb);
            return Err(fail(self, c.handle, err("mmap of the buffer", e)));
        }
        self.maps.insert(c.handle, (p.cast(), c.size as usize));
        Ok(Buffer { handle: c.handle, fb_id: f.fb_id, w, h, pitch: c.pitch, size: c.size })
    }

    fn pixels(&mut self, b: &Buffer) -> Result<&mut [u8], String> {
        let (p, len) = *self.maps.get(&b.handle).ok_or("no such buffer")?;
        // SAFETY: the mapping lives as long as the buffer (until destroy_buffer) and is only reached through &mut self.
        Ok(unsafe { std::slice::from_raw_parts_mut(p, len) })
    }

    fn destroy_buffer(&mut self, b: Buffer) {
        if let Some((p, len)) = self.maps.remove(&b.handle) {
            // SAFETY: a mapping this struct made.
            unsafe { libc::munmap(p.cast(), len) };
        }
        let mut fb = b.fb_id;
        let _ = ioc(self.fd, IOCTL_RMFB, &mut fb);
        let mut d = DestroyDumb { handle: b.handle };
        let _ = ioc(self.fd, IOCTL_DESTROY_DUMB, &mut d);
    }

    fn set_crtc(&mut self, crtc: u32, fb: u32, connectors: &[u32], mode: Option<&ModeInfo>) -> Result<(), String> {
        let mut conns: Vec<u32> = connectors.to_vec();
        let mut c = CrtcCmd { crtc_id: crtc, fb_id: if mode.is_some() { fb } else { 0 }, set_connectors_ptr: conns.as_mut_ptr() as u64, count_connectors: if mode.is_some() { conns.len() as u32 } else { 0 }, ..Default::default() };
        if let Some(m) = mode {
            c.mode = *m;
            c.mode_valid = 1;
        }
        ioc(self.fd, IOCTL_SETCRTC, &mut c).map_err(|e| err("SETCRTC", e))
    }

    fn page_flip(&mut self, crtc: u32, fb: u32) -> Result<(), String> {
        let mut p = PageFlip { crtc_id: crtc, fb_id: fb, ..Default::default() };
        ioc(self.fd, IOCTL_PAGE_FLIP, &mut p).map_err(|e| err("PAGE_FLIP", e))
    }
}

#[cfg(test)]
#[path = "kms_tests.rs"]
mod tests;
