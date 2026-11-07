//! The CAMERA tile's camera in a browser session: the picture comes from the person's web browser (JPEG frames over the
//! session's WebSocket, `iyakki_agent::browsermedia`), not from a device of this machine.
//!
//! Which camera a tile uses is decided in one place, `iyakki_agent::browsersource::browser_camera_for`. Opening asks the page to
//! show its Allow button when the camera is off (the same permission prompt as the media policy; a denied policy is refused and
//! audited) and then waits for the person's own click; nothing here switches the camera on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::browsermedia::{BrowserMedia, Kind, Requested};

use super::{CamDevice, Capture, Mode, Opened, Opener, Pix, RawFrame};

/// How long opening waits for the person to click Allow and the first picture to arrive.
pub const WAIT: Duration = Duration::from_secs(60);

pub struct BrowserOpener {
    media: Arc<BrowserMedia>,
    wait: Duration,
}

impl BrowserOpener {
    pub fn new(media: Arc<BrowserMedia>) -> BrowserOpener {
        BrowserOpener { media, wait: WAIT }
    }
    pub fn with_wait(mut self, wait: Duration) -> BrowserOpener {
        self.wait = wait;
        self
    }
}

/// Width and height from the start-of-frame marker of a JPEG.
pub fn jpeg_size(j: &[u8]) -> Option<(u32, u32)> {
    if j.len() < 4 || j[0] != 0xFF || j[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= j.len() {
        if j[i] != 0xFF {
            return None;
        }
        let m = j[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            if i + 9 > j.len() {
                return None;
            }
            let h = u16::from_be_bytes([j[i + 5], j[i + 6]]) as u32;
            let w = u16::from_be_bytes([j[i + 7], j[i + 8]]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        i += 2 + len;
    }
    None
}

impl Opener for BrowserOpener {
    fn open(&self, _dev: &CamDevice, _want: Option<Mode>) -> Result<Opened, String> {
        let asked = self.media.request(Kind::Camera);
        if asked == Requested::Denied {
            return Err(iyakki_agent::browsersource::ask_words(Kind::Camera, asked).unwrap_or_default());
        }
        let end = Instant::now() + self.wait;
        let first = loop {
            if self.media.is_on(Kind::Camera) {
                if let Some(f) = self.media.camera() {
                    break f;
                }
            }
            if Instant::now() >= end {
                return Err("The browser camera was not allowed in time. Press the red button at the top of the page, then choose the camera again.".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let (width, height) = jpeg_size(&first).unwrap_or((640, 480));
        let mode = Mode { pix: Pix::Mjpeg, width, height };
        Ok(Opened { cap: Box::new(BrowserCapture { media: self.media.clone(), mode, seen: self.media.camera_frames().saturating_sub(1) }), modes: vec![mode], controls: Vec::new() })
    }
}

pub struct BrowserCapture {
    media: Arc<BrowserMedia>,
    mode: Mode,
    seen: u64,
}

impl Capture for BrowserCapture {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn next(&mut self, timeout_ms: u32) -> Result<Option<RawFrame>, String> {
        let end = Instant::now() + Duration::from_millis(timeout_ms as u64);
        loop {
            if !self.media.is_on(Kind::Camera) {
                return Err("The browser camera was switched off.".into());
            }
            let n = self.media.camera_frames();
            if n != self.seen {
                if let Some(j) = self.media.camera() {
                    self.seen = n;
                    let (width, height) = jpeg_size(&j).unwrap_or((self.mode.width, self.mode.height));
                    return Ok(Some(RawFrame { pix: Pix::Mjpeg, width, height, data: j }));
                }
            }
            if Instant::now() >= end {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn set_control(&mut self, _id: u32, _value: i32) -> Result<(), String> {
        Err("The browser's camera has no controls here.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::browsermedia::{Policy, CH_CAMERA};
    use std::sync::Mutex;

    /// A minimal JPEG: SOI, SOF0 of 320x240, then the end marker (enough for the size and the framing checks).
    fn jpeg(w: u16, h: u16, tag: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8];
        v.extend(h.to_be_bytes());
        v.extend(w.to_be_bytes());
        v.extend([1, 1, 0x11, 0, tag, 0xFF, 0xD9]);
        v
    }

    fn media(p: Policy) -> (Arc<BrowserMedia>, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        (BrowserMedia::new(p, Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))), None), log)
    }

    fn feed(m: &BrowserMedia, j: &[u8]) {
        let mut msg = vec![CH_CAMERA];
        msg.extend_from_slice(j);
        m.binary(&msg);
    }

    fn dev() -> CamDevice {
        super::super::Presence::browser().cameras().remove(0)
    }

    #[test]
    fn the_size_comes_from_the_start_of_frame_marker() {
        assert_eq!(jpeg_size(&jpeg(320, 240, 0)), Some((320, 240)));
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xD9]), None);
        assert_eq!(jpeg_size(b"GIF89a"), None);
        assert_eq!(jpeg_size(&[]), None);
    }

    #[test]
    fn a_browser_session_has_exactly_one_camera_the_browsers() {
        let cams = super::super::Presence::browser().cameras();
        assert_eq!(cams.len(), 1);
        assert_eq!(cams[0].name, "Browser camera");
        assert!(cams[0].caps.is_camera());
        assert!(super::super::Presence::none().cameras().is_empty(), "a local session without a camera still has none");
    }

    #[test]
    fn opening_asks_the_page_and_waits_for_the_persons_click_and_the_first_picture() {
        let (m, log) = media(Policy::allow_all());
        let op = BrowserOpener::new(m.clone()).with_wait(Duration::from_secs(5));
        let m2 = m.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            m2.set(Kind::Camera, true).unwrap(); // the person clicks Allow in the page
            feed(&m2, &jpeg(320, 240, 1));
        });
        let o = op.open(&dev(), None).unwrap();
        t.join().unwrap();
        assert_eq!(o.modes, vec![Mode { pix: Pix::Mjpeg, width: 320, height: 240 }]);
        assert!(m.take_messages().iter().any(|s| s.contains("media_ask") && s.contains("camera")));
        assert!(log.lock().unwrap().contains(&"browser.media.asked camera".to_string()));
    }

    #[test]
    fn frames_arrive_in_order_once_each_and_the_stream_ends_with_the_camera() {
        let (m, _) = media(Policy::allow_all());
        m.set(Kind::Camera, true).unwrap();
        feed(&m, &jpeg(160, 120, 1));
        let mut o = BrowserOpener::new(m.clone()).with_wait(Duration::from_secs(2)).open(&dev(), None).unwrap();
        let f = o.cap.next(500).unwrap().expect("the frame that opened it");
        assert_eq!((f.pix, f.width, f.height), (Pix::Mjpeg, 160, 120));
        assert!(o.cap.next(50).unwrap().is_none(), "no new picture, no frame");
        std::thread::sleep(iyakki_agent::browsermedia::MIN_CAMERA_GAP + Duration::from_millis(30));
        feed(&m, &jpeg(160, 120, 2));
        assert_eq!(o.cap.next(500).unwrap().unwrap().data[15], 2);
        m.set(Kind::Camera, false).unwrap();
        assert!(o.cap.next(50).is_err(), "the tile sees that the camera was switched off");
        assert!(o.cap.set_control(1, 1).is_err());
    }

    #[test]
    fn a_denied_policy_is_refused_at_once_and_audited() {
        let (m, log) = media(Policy::deny_all());
        let t0 = Instant::now();
        let e = BrowserOpener::new(m.clone()).with_wait(Duration::from_secs(5)).open(&dev(), None).err().unwrap();
        assert!(e.contains("may not use the camera") && t0.elapsed() < Duration::from_secs(2), "{e}");
        assert!(log.lock().unwrap().contains(&"browser.media.denied camera".to_string()));
    }

    #[test]
    fn nobody_clicking_ends_in_a_plain_message() {
        let (m, _) = media(Policy::allow_all());
        let e = BrowserOpener::new(m).with_wait(Duration::from_millis(200)).open(&dev(), None).err().unwrap();
        assert!(e.contains("not allowed in time"), "{e}");
    }
}
