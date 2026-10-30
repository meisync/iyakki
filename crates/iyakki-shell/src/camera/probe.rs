//! One picture from the camera for the CHECK-UP tile: open the camera the CAMERA tile would use, wait for a frame that is no longer the
//! camera's first dark one, hand the picture over and let go of the camera. Nothing is written to disk; the caller shows the picture
//! once and drops it.

use std::time::{Duration, Instant};

use iyakki_agent::checkup::{picture_spread, CamHeard};

use super::{CamDevice, Opener};

/// What came back: the verdict input and, when a picture arrived, the picture (RGBA).
pub struct Shot {
    pub heard: CamHeard,
    pub picture: Option<(u32, u32, Vec<u8>)>,
}

/// Frames to let go by before the one that is kept (a webcam's first frames are dark while it settles its exposure).
const SETTLE_FRAMES: usize = 3;

/// Take one picture from the camera `saved` names (the CAMERA tile's choice), else the built-in one, else the first of `cams`.
/// `no_camera` is what to say when there is none. Gives up after `wait`.
pub fn grab(cams: &[CamDevice], saved: &str, opener: &dyn Opener, no_camera: &str, wait: Duration) -> Shot {
    if cams.is_empty() {
        return Shot { heard: CamHeard::NoCamera(no_camera.to_string()), picture: None };
    }
    let dev = &cams[crate::apps::camera::preferred(cams, saved)];
    let name = dev.name.clone();
    let fail = |why: String| Shot { heard: CamHeard::Failed { name: name.clone(), why }, picture: None };
    let mut opened = match opener.open(dev, None) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let start = Instant::now();
    let mut seen = 0usize;
    let mut last = None;
    while start.elapsed() < wait {
        match opened.cap.next(200) {
            Ok(Some(frame)) => {
                seen += 1;
                last = Some(frame);
                if seen > SETTLE_FRAMES {
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => return fail(e),
        }
    }
    // the camera is let go of here, before the picture is judged
    drop(opened);
    let Some(frame) = last else { return fail("no picture arrived".into()) };
    match frame.to_rgba() {
        Ok((w, h, rgba)) => Shot { heard: CamHeard::Picture { name, width: w, height: h, spread: picture_spread(&rgba) }, picture: Some((w, h, rgba)) },
        Err(e) => fail(e),
    }
}

/// The real machine: the cameras V4L2 lists, the CAMERA tile's saved choice (`home` is the person's home folder).
pub fn system_grab(home: &std::path::Path, wait: Duration) -> Shot {
    let mut presence = super::Presence::system();
    presence.scan();
    let cams = presence.cameras();
    let saved = crate::apps::camera::load_choice(&home.join(crate::apps::camera::CHOICE_FILE));
    grab(&cams, &saved, &super::v4l2::V4l2Opener, presence.hint(), wait)
}

#[cfg(test)]
mod tests {
    use super::super::fake::{device, FakeOpener};
    use super::*;

    #[test]
    fn a_picture_is_taken_after_the_settling_frames_and_the_camera_is_named() {
        let op = FakeOpener::default();
        let s = grab(&[device("video0", "USB Camera")], "", &op, "none", Duration::from_secs(3));
        let CamHeard::Picture { name, width, height, spread } = s.heard else { panic!("expected a picture") };
        assert_eq!((name.as_str(), width, height), ("USB Camera", 640, 360));
        assert!(spread > 20, "the fake picture has a face and a wall");
        let (w, h, rgba) = s.picture.unwrap();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert_eq!(op.opened.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_cameras_tile_choice_wins_and_no_camera_says_why() {
        let op = FakeOpener::default();
        let cams = [device("video0", "First"), device("video2", "Second")];
        let s = grab(&cams, "Second", &op, "none", Duration::from_secs(3));
        assert!(matches!(s.heard, CamHeard::Picture { ref name, .. } if name == "Second"));
        assert_eq!(op.opened.lock().unwrap()[0].0, "video2");
        let none = grab(&[], "", &op, "No camera found. Connect a USB camera.", Duration::from_secs(1));
        assert_eq!(none.heard, CamHeard::NoCamera("No camera found. Connect a USB camera.".into()));
        assert!(none.picture.is_none());
    }

    #[test]
    fn a_camera_that_drops_out_or_cannot_be_opened_is_a_failure_not_a_hang() {
        let op = FakeOpener { fail_after: Some(1), ..FakeOpener::default() };
        let s = grab(&[device("video0", "USB Camera")], "", &op, "", Duration::from_secs(3));
        assert!(matches!(s.heard, CamHeard::Failed { ref why, .. } if why.contains("disconnected")), "{:?}", s.heard);
        struct Busy;
        impl Opener for Busy {
            fn open(&self, _: &CamDevice, _: Option<super::super::Mode>) -> Result<super::super::Opened, String> {
                Err("device or resource busy".into())
            }
        }
        let b = grab(&[device("video0", "USB Camera")], "", &Busy, "", Duration::from_secs(1));
        assert!(matches!(b.heard, CamHeard::Failed { ref why, .. } if why.contains("busy")));
    }
}
