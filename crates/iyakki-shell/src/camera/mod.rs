//! USB cameras for the CAMERA tile, all in-house on top of libc: finding them ([`detect`]), talking V4L2 ([`v4l2`]),
//! JPEG frames ([`jpeg`]), pixel work and PNG print sizes ([`pixels`]), MJPEG/AVI recording ([`avi`]), passport photos
//! and print sheets ([`passport`]), and the threads that keep a live picture coming while the app draws ([`live`]).

use crate::safeclamp::SafeClamp;

pub mod avi;
pub mod detect;
pub mod jpeg;
pub mod live;
pub mod passport;
pub mod probe;
pub mod pixels;
pub mod specs;
pub mod v4l2;
pub mod agentcam;
pub mod own;

pub use detect::{CamDevice, Presence, Roots};

/// `dir/stem.ext`, or `dir/stem-2.ext` and so on: nothing is ever overwritten.
pub fn unique(dir: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem}-{n}.{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

static ZONE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The time zone the on-screen clock uses (Settings -> Time; empty: the machine's own). The shell passes it on every frame so
/// file names carry the same local time as the clock.
pub fn set_zone(name: Option<String>) {
    if let Ok(mut z) = ZONE.lock() {
        *z = name.filter(|n| !n.is_empty());
    }
}

/// The time stamp in file names (`20261018-142233`): the same zone as the clock in the top bar, at `unix`.
pub fn stamp_at(zone: Option<String>, unix: i64) -> String {
    let z = iyakki_agent::clock::Zone::system_with(zone);
    iyakki_agent::clock::format(&z, unix, "%Y%m%d-%H%M%S")
}

/// The time stamp in file names, now.
pub fn stamp() -> String {
    stamp_at(ZONE.lock().ok().and_then(|z| z.clone()), iyakki_agent::clock::now_unix())
}

/// How the camera sends pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pix {
    /// Each frame a JPEG (usually without Huffman tables, see [`jpeg::with_huffman`]).
    Mjpeg,
    /// Uncompressed YUV 4:2:2.
    Yuyv,
}

impl Pix {
    pub fn name(self) -> &'static str {
        match self {
            Pix::Mjpeg => "MJPEG",
            Pix::Yuyv => "YUYV",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mode {
    pub pix: Pix,
    pub width: u32,
    pub height: u32,
}

impl Mode {
    pub fn label(&self) -> String {
        format!("{}×{} {}", self.width, self.height, self.pix.name())
    }
}

/// One picture as the camera sent it.
#[derive(Clone, Debug)]
pub struct RawFrame {
    pub pix: Pix,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RawFrame {
    /// The picture as straight RGBA.
    pub fn to_rgba(&self) -> Result<(u32, u32, Vec<u8>), String> {
        match self.pix {
            Pix::Mjpeg => jpeg::decode(&self.data).map(|b| (b.width, b.height, b.rgba)),
            Pix::Yuyv => {
                if self.data.len() < (self.width * self.height * 2) as usize {
                    return Err("a short frame".into());
                }
                Ok((self.width, self.height, pixels::yuyv_to_rgba(self.width, self.height, &self.data)))
            }
        }
    }

    /// The picture as a JPEG file: an MJPEG frame as it is (tables put back), a YUYV frame encoded.
    pub fn to_jpeg(&self) -> Result<Vec<u8>, String> {
        match self.pix {
            Pix::Mjpeg => jpeg::with_huffman(&self.data).map(|c| c.into_owned()).ok_or_else(|| "not a JPEG frame".to_string()),
            Pix::Yuyv => {
                let (w, h, rgba) = self.to_rgba()?;
                Ok(jpeg::encode_rgba(w, h, &rgba, 90))
            }
        }
    }
}

/// A brightness or contrast control of the camera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub id: u32,
    pub name: String,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub default: i32,
    pub value: i32,
}

impl Control {
    /// The value one step (a twentieth of the range, at least the camera's step) up or down.
    pub fn stepped(&self, up: bool) -> i32 {
        let span = (self.max - self.min).max(1);
        let s = ((span / 20) / self.step.max(1)).max(1) * self.step.max(1);
        (if up { self.value + s } else { self.value - s }).clamp_safe(self.min, self.max)
    }
    /// Where the value sits, 0..=1.
    pub fn fraction(&self) -> f32 {
        (self.value - self.min) as f32 / (self.max - self.min).max(1) as f32
    }
}

/// A streaming camera.
pub trait Capture: Send {
    fn mode(&self) -> Mode;
    /// The next frame, waiting at most `timeout_ms`; `Ok(None)` when none came in time.
    fn next(&mut self, timeout_ms: u32) -> Result<Option<RawFrame>, String>;
    fn set_control(&mut self, id: u32, value: i32) -> Result<(), String>;
}

/// A camera opened and streaming, with what it offers.
pub struct Opened {
    pub cap: Box<dyn Capture>,
    pub modes: Vec<Mode>,
    pub controls: Vec<Control>,
}

/// Opens a camera in a mode (`None`: its best default).
pub trait Opener: Send + Sync {
    fn open(&self, dev: &CamDevice, want: Option<Mode>) -> Result<Opened, String>;
}

/// A camera made up in software: a moving test picture with a face-shaped oval, sent as MJPEG or YUYV. For the tests and
/// the screenshots; never shown to a person as a real camera.
pub mod fake {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub struct FakeCapture {
        pub mode: Mode,
        pub n: u64,
        pub controls: Arc<Mutex<Vec<(u32, i32)>>>,
        /// Fail after this many frames (a camera pulled out).
        pub fail_after: Option<u64>,
    }

    /// The test picture: a blue-grey wall, a skin-toned oval in the middle and a bar that moves with `n`.
    pub fn picture(w: u32, h: u32, n: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        let (cx, cy) = (w as f32 / 2.0, h as f32 * 0.45);
        let (rx, ry) = (h as f32 * 0.17, h as f32 * 0.24);
        let bar = (n * 7 % w.max(1) as u64) as u32;
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - cx) / rx).powi(2) + ((y as f32 - cy) / ry).powi(2);
                let px = if d < 1.0 {
                    let eye = (y as f32 - (cy - ry * 0.15)).abs() < ry * 0.06 && ((x as f32 - (cx - rx * 0.4)).abs() < rx * 0.12 || (x as f32 - (cx + rx * 0.4)).abs() < rx * 0.12);
                    if eye { [40, 40, 50] } else { [228, 186, 160] }
                } else if y > (h as f32 * 0.72) as u32 && (x as f32 - cx).abs() < rx * 2.2 {
                    [52, 70, 120]
                } else if x.abs_diff(bar) < 3 {
                    [255, 120, 80]
                } else {
                    let g = 200 - (y * 40 / h.max(1)) as u8;
                    [g - 10, g, g + 12]
                };
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        out
    }

    impl Capture for FakeCapture {
        fn mode(&self) -> Mode {
            self.mode
        }
        fn next(&mut self, timeout_ms: u32) -> Result<Option<RawFrame>, String> {
            if self.fail_after.is_some_and(|k| self.n >= k) {
                return Err("the camera was disconnected".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms.min(20) as u64));
            self.n += 1;
            let rgba = picture(self.mode.width, self.mode.height, self.n);
            let data = match self.mode.pix {
                Pix::Mjpeg => jpeg::strip_huffman(&jpeg::encode_rgba(self.mode.width, self.mode.height, &rgba, 75)),
                Pix::Yuyv => pixels::rgba_to_yuyv(self.mode.width, self.mode.height, &rgba),
            };
            Ok(Some(RawFrame { pix: self.mode.pix, width: self.mode.width, height: self.mode.height, data }))
        }
        fn set_control(&mut self, id: u32, value: i32) -> Result<(), String> {
            self.controls.lock().unwrap().push((id, value));
            Ok(())
        }
    }

    /// Opens [`FakeCapture`]s; remembers what was opened.
    #[derive(Default)]
    pub struct FakeOpener {
        pub opened: Arc<Mutex<Vec<(String, Mode)>>>,
        pub controls: Arc<Mutex<Vec<(u32, i32)>>>,
        pub yuyv_only: bool,
        pub fail_after: Option<u64>,
    }

    impl FakeOpener {
        pub fn modes(&self) -> Vec<Mode> {
            let m = |pix, width, height| Mode { pix, width, height };
            if self.yuyv_only {
                vec![m(Pix::Yuyv, 320, 240), m(Pix::Yuyv, 640, 480)]
            } else {
                vec![m(Pix::Yuyv, 640, 480), m(Pix::Mjpeg, 320, 240), m(Pix::Mjpeg, 640, 360), m(Pix::Mjpeg, 1280, 720)]
            }
        }
    }

    impl Opener for FakeOpener {
        fn open(&self, dev: &CamDevice, want: Option<Mode>) -> Result<Opened, String> {
            let modes = self.modes();
            // the fake's default stays small so tests are quick
            let mode = want.filter(|m| modes.contains(m)).unwrap_or(if self.yuyv_only { modes[0] } else { modes[2] });
            self.opened.lock().unwrap().push((dev.node.clone(), mode));
            let controls = vec![
                Control { id: v4l2::CID_BRIGHTNESS, name: "Brightness".into(), min: -64, max: 64, step: 1, default: 0, value: 0 },
                Control { id: v4l2::CID_CONTRAST, name: "Contrast".into(), min: 0, max: 100, step: 1, default: 50, value: 50 },
            ];
            Ok(Opened { cap: Box::new(FakeCapture { mode, n: 0, controls: self.controls.clone(), fail_after: self.fail_after }), modes, controls })
        }
    }

    /// A camera device entry for the fake.
    pub fn device(node: &str, name: &str) -> CamDevice {
        CamDevice { node: node.into(), path: format!("/fake/{node}").into(), name: name.into(), caps: detect::Caps { driver: "fake".into(), card: name.into(), bus: "fake".into(), caps: v4l2::CAP_VIDEO_CAPTURE | v4l2::CAP_STREAMING }, builtin: false, physical: node.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_become_pictures_and_jpeg_files_and_controls_step_within_range() {
        let rgba = fake::picture(32, 16, 1);
        let y = RawFrame { pix: Pix::Yuyv, width: 32, height: 16, data: pixels::rgba_to_yuyv(32, 16, &rgba) };
        let (w, h, back) = y.to_rgba().unwrap();
        assert_eq!((w, h, back.len()), (32, 16, 32 * 16 * 4));
        let j = y.to_jpeg().unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        let m = RawFrame { pix: Pix::Mjpeg, width: 32, height: 16, data: jpeg::strip_huffman(&jpeg::encode_rgba(32, 16, &rgba, 80)) };
        assert!(jpeg::has_huffman(&m.to_jpeg().unwrap()), "a saved photo is a complete JPEG");
        assert_eq!(m.to_rgba().unwrap().0, 32);
        assert!(RawFrame { pix: Pix::Yuyv, width: 32, height: 16, data: vec![0; 10] }.to_rgba().is_err());
        let c = Control { id: 1, name: "B".into(), min: -64, max: 64, step: 1, default: 0, value: 60 };
        assert_eq!(c.stepped(true), 64);
        assert_eq!(c.stepped(false), 54);
        assert!((c.fraction() - 124.0 / 128.0).abs() < 1e-4);
        let c = Control { id: 1, name: "C".into(), min: 0, max: 10, step: 5, default: 5, value: 5 };
        assert_eq!((c.stepped(true), c.stepped(false)), (10, 0));
        assert_eq!(Mode { pix: Pix::Mjpeg, width: 1280, height: 720 }.label(), "1280×720 MJPEG");
    }
}
