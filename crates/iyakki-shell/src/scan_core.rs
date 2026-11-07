//! What every camera QR scan shares: where cameras come from ([`CamSource`]), the session that asks first, opens the camera, reads
//! frames in memory and closes it again ([`Session`]), the frame to grayscale step ([`decode_frame`]) and the "camera is on" card
//! with the live picture and a frame guide ([`draw_card`]).
//!
//! The sign-in dialog (agent_login_scan.rs) and the clipboard's "Scan QR" (clip_scan.rs) sit on top and only decide what a read
//! text may be used for. The camera is opened only after the person says yes, pictures are never stored or sent, and it is closed
//! as soon as something is read, on Esc, when the owner goes away and after [`SCAN_SECS`] seconds. This file never logs the text.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::qr_decode;
use tiny_skia::{FilterQuality, PixmapPaint, Transform};

use crate::camera::live::{Live, Preview};
use crate::camera::{CamDevice, Opener, Pix, Presence, RawFrame};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

/// The camera stops by itself after this long.
pub const SCAN_SECS: u64 = 60;
/// A new frame is looked at no more often than this.
const DECODE_EVERY: Duration = Duration::from_millis(120);
const ACCENT: [u8; 4] = rgb(255, 184, 77);

/// Where cameras come from: the machine's own, or made up for the tests.
pub struct CamSource {
    presence: Mutex<Presence>,
    opener: Arc<dyn Opener>,
}

impl CamSource {
    pub fn system() -> Arc<CamSource> {
        CamSource::with(Presence::system(), Arc::new(crate::camera::v4l2::V4l2Opener))
    }

    /// The camera of the desktop's own scans (the clipboard's "Scan QR", the sign-in dialog): the web browser's camera when a browser session
    /// is connected (the CAMERA app already does this; a scan that opened the machine's camera instead found nothing on a Mac mini without
    /// a camera, or lit the wrong one), the machine's otherwise.
    pub fn desktop() -> Arc<CamSource> {
        match iyakki_agent::browsersource::browser_camera_for_desktop() {
            Some(m) => CamSource::with(Presence::browser(), Arc::new(crate::camera::browsercam::BrowserOpener::new(m))),
            None => CamSource::system(),
        }
    }

    pub fn with(presence: Presence, opener: Arc<dyn Opener>) -> Arc<CamSource> {
        Arc::new(CamSource { presence: Mutex::new(presence), opener })
    }

    /// The first camera, if there is one.
    fn first(&self) -> Option<CamDevice> {
        let mut p = self.presence.lock().unwrap_or_else(|e| e.into_inner());
        p.scan();
        p.cameras().into_iter().next()
    }

    fn open(&self, dev: &CamDevice) -> Result<Live, String> {
        match self.opener.open(dev, None) {
            Ok(o) => Ok(Live::start(o.cap)),
            Err(e) if e.contains("busy") || e.contains("EBUSY") => Err("The camera is in use by another program (is the CAMERA app open?). Close it and try again.".to_string()),
            Err(e) if e.contains("ermission") || e.contains("EACCES") => Err("The camera was not allowed to open. It stays off.".to_string()),
            Err(_) => Err("The camera could not be opened. It stays off.".to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Off,
    /// The person is being asked whether the camera may turn on.
    Asking,
    /// The camera is on and frames are being read.
    On,
}

pub struct Session {
    src: Arc<CamSource>,
    step: Step,
    cam_name: String,
    live: Option<Live>,
    started: Option<Instant>,
    limit: Duration,
    seen: u64,
    last: Option<Instant>,
    /// What to tell the person (plain words), and whether it is good news.
    said: Option<(String, bool)>,
    /// What to say when the machine has no camera.
    no_camera: &'static str,
    /// A time budget per frame for the decoder; `None` is the decoder's own (30 ms, in proportion above 640x480). Only tests set one.
    budget: Option<Duration>,
}

impl Session {
    pub fn new(src: Arc<CamSource>, no_camera: &'static str) -> Session {
        Session { src, step: Step::Off, cam_name: String::new(), live: None, started: None, limit: Duration::from_secs(SCAN_SECS), seen: 0, last: None, said: None, no_camera, budget: None }
    }

    /// A shorter time limit (tests).
    pub fn with_limit(mut self, limit: Duration) -> Session {
        self.limit = limit;
        self
    }

    /// A fixed, generous decoder budget per frame (tests: a busy machine must not make the decoder give up).
    pub fn with_budget(mut self, budget: Duration) -> Session {
        self.budget = Some(budget);
        self
    }

    pub fn step(&self) -> Step {
        self.step
    }

    /// The camera is on right now (drives the "camera is on" indicator).
    pub fn on(&self) -> bool {
        self.live.is_some()
    }

    pub fn said(&self) -> Option<&(String, bool)> {
        self.said.as_ref()
    }

    pub fn camera_name(&self) -> &str {
        &self.cam_name
    }

    /// Whole seconds left before the camera stops by itself.
    pub fn secs_left(&self) -> u64 {
        self.started.map_or(0, |t| self.limit.saturating_sub(t.elapsed()).as_secs_f64().ceil() as u64)
    }

    /// The newest picture for the small preview.
    pub fn preview(&self) -> Option<Arc<Preview>> {
        self.live.as_ref().and_then(|l| l.preview()).map(|(_, p)| p)
    }

    /// Look for a camera and ask whether it may turn on. Nothing is opened yet.
    pub fn ask(&mut self) {
        if self.step != Step::Off {
            return;
        }
        match self.src.first() {
            Some(d) => {
                self.cam_name = d.name;
                self.said = None;
                self.step = Step::Asking;
            }
            None => self.said = Some((self.no_camera.to_string(), false)),
        }
    }

    /// The person said yes: open the camera.
    pub fn allow(&mut self) {
        if self.step != Step::Asking {
            return;
        }
        self.step = Step::Off;
        let Some(dev) = self.src.first() else {
            self.said = Some((self.no_camera.to_string(), false));
            return;
        };
        match self.src.open(&dev) {
            Ok(live) => {
                self.live = Some(live);
                self.started = Some(Instant::now());
                self.seen = 0;
                self.last = None;
                self.said = None;
                self.step = Step::On;
            }
            Err(e) => self.said = Some((e, false)),
        }
    }

    /// The person said no.
    pub fn deny(&mut self) {
        if self.step == Step::Asking {
            self.step = Step::Off;
            self.said = Some(("The camera stays off.".to_string(), true));
        }
    }

    /// Turn the camera off (Esc, the Stop button, the dialog closing or hiding, the program ending). The pictures go with it.
    pub fn stop(&mut self, why: Option<&str>) {
        self.live = None;
        self.started = None;
        self.step = Step::Off;
        if let Some(w) = why {
            self.said = Some((w.to_string(), true));
        }
    }

    /// Say something (plain words) without changing the camera.
    pub fn say(&mut self, msg: String, good: bool) {
        self.said = Some((msg, good));
    }

    /// Look at the camera. Called on every heartbeat. When a QR code is read, `accept` decides: `Ok((value, message))` turns the
    /// camera off, shows the message and returns the value; `Err(message)` shows the message and scanning goes on. `accept` must
    /// never put the read text in its messages.
    pub fn poll<T>(&mut self, accept: impl FnOnce(&[u8]) -> Result<(T, String), String>) -> Option<T> {
        if self.step != Step::On {
            return None;
        }
        if self.started.is_some_and(|t| t.elapsed() >= self.limit) {
            self.stop(None);
            self.said = Some((format!("No code was read in {} seconds. The camera is off.", self.limit.as_secs()), false));
            return None;
        }
        let live = self.live.as_ref()?;
        if live.error().is_some() {
            self.stop(None);
            self.said = Some(("The camera stopped sending pictures. It is off.".to_string(), false));
            return None;
        }
        let n = live.frames();
        if n == self.seen || self.last.is_some_and(|t| t.elapsed() < DECODE_EVERY) {
            return None;
        }
        self.seen = n;
        self.last = Some(Instant::now());
        let frame = live.latest()?;
        let bytes = decode_frame_with(&frame, self.budget)?;
        match accept(&bytes) {
            Ok((v, msg)) => {
                self.stop(Some(&msg));
                Some(v)
            }
            Err(m) => {
                self.said = Some((m, false));
                None
            }
        }
    }
}

/// The text of the QR code in a camera frame, if there is one (the decoder's own time budget).
pub fn decode_frame(f: &RawFrame) -> Option<Vec<u8>> {
    decode_frame_with(f, None)
}

/// The same with a chosen time budget per frame (`None`: the decoder's own). A grayscale picture is made from the luma; the decoder
/// tries the picture and its mirror image.
pub fn decode_frame_with(f: &RawFrame, budget: Option<Duration>) -> Option<Vec<u8>> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (w, h, gray): (usize, usize, Vec<u8>) = match f.pix {
        Pix::Yuyv => {
            if f.data.len() < w.checked_mul(h)?.checked_mul(2)? {
                return None;
            }
            (w, h, f.data.iter().step_by(2).take(w * h).copied().collect())
        }
        Pix::Mjpeg => {
            let (w, h, rgba) = f.to_rgba().ok()?;
            let gray = rgba.chunks_exact(4).map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
            (w as usize, h as usize, gray)
        }
    };
    match budget {
        None => qr_decode::decode_gray(w, h, &gray),
        Some(b) => qr_decode::decode_with(w, h, &gray, b),
    }
}

/// What the card says and which buttons it has.
pub struct Card<'a> {
    /// The question's title and body.
    pub ask_title: &'a str,
    pub ask_body: &'a str,
    /// The hint under the live picture.
    pub hold_note: &'a str,
    pub allow: usize,
    pub deny: usize,
    pub stop: usize,
}

/// The camera question and the live camera, as a card over what is behind: what the camera is for, a clear "camera is on" mark, the
/// small live picture with a frame to hold the code in, the time left and a Stop button. Only this card is clickable while it is up.
pub fn draw_card(g: &mut Gfx, p: R, s: f32, tick: u64, ss: &Session, c: &Card) {
    g.rect(p.x, p.y, p.w, p.h, [3, 5, 14, 200]);
    g.clear_hits();
    g.hit(p, Event::Tick, "backdrop");
    let asking = ss.step() == Step::Asking;
    let (cw, ch) = (540.0 * s, if asking { 320.0 * s } else { 508.0 * s });
    let r = R::new(p.x + (p.w - cw) / 2.0, p.y + ((p.h - ch) / 2.0).max(6.0 * s), cw, ch);
    fx::modal_panel(g, r, s, if asking { ACCENT } else { ERR }, !asking);
    let (x, w) = (r.x + 26.0 * s, r.w - 52.0 * s);
    if asking {
        g.text(x, r.y + 22.0 * s, 21.0 * s, TEXT, c.ask_title, Face::Bold);
        let mut y = r.y + 64.0 * s;
        for l in g.wrap(14.0 * s, c.ask_body, Face::Sans, w) {
            g.text(x, y, 14.0 * s, MUTED, &l, Face::Sans);
            y += 19.0 * s;
        }
        let cam = g.fit(13.0 * s, &format!("Camera: {}", ss.camera_name()), Face::Sans, w);
        g.text(x, y + 8.0 * s, 13.0 * s, DIM, &cam, Face::Sans);
        let by = r.y + r.h - 60.0 * s;
        let b1 = g.button(x, by, 15.0 * s, "Turn camera on", OK, Event::Tab(c.allow));
        g.button(x + b1 + 12.0 * s, by, 15.0 * s, "No", MUTED, Event::Tab(c.deny));
        g.text(x, by + 38.0 * s, 12.0 * s, DIM, "Enter turns it on · Esc leaves it off", Face::Sans);
        return;
    }
    // the indicator: a red dot that blinks and the words, always on screen while the camera runs
    let on = (tick / 15) % 2 == 0;
    g.rrect(x, r.y + 16.0 * s, 196.0 * s, 30.0 * s, 15.0 * s, rgb(70, 10, 18));
    g.circle(x + 17.0 * s, r.y + 31.0 * s, 6.0 * s, if on { rgb(255, 60, 70) } else { rgb(150, 30, 40) });
    g.text(x + 32.0 * s, r.y + 22.0 * s, 15.0 * s, TEXT, "CAMERA IS ON", Face::Bold);
    let left = format!("stops in {} s", ss.secs_left());
    let lw = g.text_width(13.0 * s, &left, Face::Sans);
    g.text(x + w - lw, r.y + 24.0 * s, 13.0 * s, MUTED, &left, Face::Sans);
    // the live picture, 4:3, with a frame guide
    let area = R::new(x, r.y + 60.0 * s, w, 300.0 * s);
    g.rrect(area.x, area.y, area.w, area.h, 10.0 * s, rgb(8, 10, 20));
    match ss.preview() {
        Some(pv) => {
            let (fw, fh) = (pv.width as f32, pv.height as f32);
            let k = (area.w / fw).min(area.h / fh);
            let (dw, dh) = ((fw * k).floor(), (fh * k).floor());
            let d = R::new((area.x + (area.w - dw) / 2.0).round(), (area.y + (area.h - dh) / 2.0).round(), dw, dh);
            let (pw, ph) = (pv.pm.width() as f32, pv.pm.height() as f32);
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
            g.pm.draw_pixmap(0, 0, pv.pm.as_ref(), &paint, Transform::from_row(dw / pw, 0.0, 0.0, dh / ph, d.x, d.y), None);
            let side = d.w.min(d.h) * 0.72;
            let (gx, gy) = (d.x + (d.w - side) / 2.0, d.y + (d.h - side) / 2.0);
            let arm = side * 0.2;
            for (px, py, sx, sy) in [(gx, gy, 1.0, 1.0), (gx + side, gy, -1.0, 1.0), (gx, gy + side, 1.0, -1.0), (gx + side, gy + side, -1.0, -1.0)] {
                g.line(px, py, px + sx * arm, py, 3.0 * s, ACCENT);
                g.line(px, py, px, py + sy * arm, 3.0 * s, ACCENT);
            }
        }
        None => {
            g.text(area.x + 20.0 * s, area.y + area.h / 2.0 - 8.0 * s, 15.0 * s, DIM, "Waiting for the first picture…", Face::Sans);
        }
    }
    let mut y = area.y + area.h + 10.0 * s;
    for l in g.wrap(13.0 * s, c.hold_note, Face::Sans, w).into_iter().take(2) {
        g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
        y += 17.0 * s;
    }
    if let Some((m, ok)) = ss.said() {
        for l in g.wrap(13.0 * s, m, Face::Sans, w).into_iter().take(2) {
            g.text(x, y + 2.0 * s, 13.0 * s, if *ok { OK } else { ERR }, &l, Face::Sans);
            y += 17.0 * s;
        }
    }
    let by = r.y + r.h - 54.0 * s;
    let b1 = g.button(x, by, 15.0 * s, "Stop camera", ERR, Event::Tab(c.stop));
    g.text(x + b1 + 14.0 * s, by + 8.0 * s, 12.0 * s, DIM, "Esc stops it too", Face::Sans);
}

#[cfg(test)]
mod desktop_tests {
    use super::*;
    use iyakki_agent::browsermedia::{self, BrowserMedia, Policy};

    #[test]
    fn a_scan_in_a_browser_session_uses_the_browsers_camera() {
        let m = BrowserMedia::silent(Policy::allow_all());
        browsermedia::register("scan-desktop-test-user", m);
        let dev = CamSource::desktop().first();
        browsermedia::unregister("scan-desktop-test-user");
        assert_eq!(dev.expect("the browser's camera is offered").name, "Browser camera");
    }
}
