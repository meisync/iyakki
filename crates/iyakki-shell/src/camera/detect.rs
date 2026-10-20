//! Is there a camera? The video nodes come from `/sys/class/video4linux`; each new node is asked what it is
//! (`VIDIOC_QUERYCAP`, through an injectable function so tests need no device) and only nodes that capture video and can
//! stream count. A UVC webcam makes two nodes (pictures and metadata); only the first is a camera. Nodes are asked once
//! and again only when they change, so the launcher can look every couple of seconds for a camera plugged in later.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::v4l2::{CAP_DEVICE_CAPS, CAP_STREAMING, CAP_VIDEO_CAPTURE};

/// Where to look (`/sys` and `/dev` on a real machine; temporary folders in tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    pub sys: PathBuf,
    pub dev: PathBuf,
}

impl Roots {
    pub fn system() -> Roots {
        Roots { sys: "/sys".into(), dev: "/dev".into() }
    }
}

/// What `VIDIOC_QUERYCAP` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub driver: String,
    pub card: String,
    pub bus: String,
    /// The capabilities of this node (`device_caps` when the driver gives them, else the device's).
    pub caps: u32,
}

impl Caps {
    /// A node a camera app can use: single-plane video capture with streaming I/O.
    pub fn is_camera(&self) -> bool {
        self.caps & CAP_VIDEO_CAPTURE != 0 && self.caps & CAP_STREAMING != 0
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// `struct v4l2_capability` (104 bytes): driver[16], card[32], bus_info[32], version, capabilities, device_caps,
/// reserved[3].
pub fn parse_caps(b: &[u8]) -> Option<Caps> {
    if b.len() < 104 {
        return None;
    }
    let u = |at: usize| u32::from_ne_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let all = u(84);
    let dev = u(88);
    Some(Caps { driver: cstr(&b[..16]), card: cstr(&b[16..48]), bus: cstr(&b[48..80]), caps: if all & CAP_DEVICE_CAPS != 0 { dev } else { all } })
}

/// A camera found on the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamDevice {
    /// `video0`
    pub node: String,
    pub path: PathBuf,
    pub name: String,
    pub caps: Caps,
}

pub type Query = Box<dyn Fn(&Path) -> Option<Caps> + Send + Sync>;

/// The cameras on the machine, kept up to date by [`Presence::scan`].
pub struct Presence {
    roots: Roots,
    query: Query,
    /// node -> (its `dev` number, the camera when it is one)
    known: BTreeMap<String, (String, Option<CamDevice>)>,
    apple: bool,
    scanned: bool,
}

fn node_number(n: &str) -> u32 {
    n.trim_start_matches("video").parse().unwrap_or(u32::MAX)
}

impl Presence {
    /// The real machine, asking devices through V4L2.
    pub fn system() -> Presence {
        Presence::with(Roots::system(), Box::new(|p: &Path| super::v4l2::query_caps(p)))
    }

    pub fn with(roots: Roots, query: Query) -> Presence {
        let apple = std::fs::read_to_string(roots.sys.join("class/dmi/id/sys_vendor")).is_ok_and(|v| v.contains("Apple"));
        Presence { roots, query, known: BTreeMap::new(), apple, scanned: false }
    }

    /// Nothing at all: no camera, never looks (the tests' default).
    pub fn none() -> Presence {
        Presence::with(Roots { sys: "/nonexistent-iyakki-sys".into(), dev: "/nonexistent-iyakki-dev".into() }, Box::new(|_| None))
    }

    /// Look again; true when the list of cameras changed.
    pub fn scan(&mut self) -> bool {
        let before = self.cameras();
        let class = self.roots.sys.join("class/video4linux");
        let mut seen = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&class) {
            for e in rd.flatten() {
                let node = e.file_name().to_string_lossy().into_owned();
                if !node.starts_with("video") {
                    continue;
                }
                let path = self.roots.dev.join(&node);
                if !path.exists() {
                    continue;
                }
                let devno = std::fs::read_to_string(class.join(&node).join("dev")).unwrap_or_default().trim().to_string();
                seen.push(node.clone());
                if self.known.get(&node).is_some_and(|(d, _)| *d == devno) {
                    continue;
                }
                let sysname = std::fs::read_to_string(class.join(&node).join("name")).unwrap_or_default().trim().to_string();
                let cam = (self.query)(&path).filter(Caps::is_camera).map(|caps| {
                    let name = if caps.card.is_empty() { if sysname.is_empty() { node.clone() } else { sysname.clone() } } else { caps.card.clone() };
                    CamDevice { node: node.clone(), path: path.clone(), name, caps }
                });
                self.known.insert(node, (devno, cam));
            }
        }
        self.known.retain(|n, _| seen.contains(n));
        self.scanned = true;
        self.cameras() != before
    }

    /// The cameras, in node order (video0, video2, video10).
    pub fn cameras(&self) -> Vec<CamDevice> {
        let mut v: Vec<CamDevice> = self.known.values().filter_map(|(_, c)| c.clone()).collect();
        v.sort_by_key(|c| node_number(&c.node));
        v
    }

    pub fn available(&self) -> bool {
        self.known.values().any(|(_, c)| c.is_some())
    }

    pub fn scanned(&self) -> bool {
        self.scanned
    }

    /// The short line on a greyed tile.
    pub fn tile_hint(&self) -> &'static str {
        "Connect a camera"
    }

    /// What to say when the greyed tile is chosen.
    pub fn hint(&self) -> &'static str {
        if self.apple {
            "No camera found. Connect a USB camera: a Mac mini has none built in, and the built-in camera of a Mac with a T2 chip is not reachable from IYAKKI."
        } else if !self.known.is_empty() {
            "No camera found: the video devices here do not take pictures. Connect a USB camera."
        } else {
            "No camera found. Connect a USB camera; it is picked up within a few seconds."
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The 104 bytes `VIDIOC_QUERYCAP` fills in.
    pub fn raw_caps(driver: &str, card: &str, caps: u32, device_caps: u32) -> Vec<u8> {
        let mut b = vec![0u8; 104];
        b[..driver.len()].copy_from_slice(driver.as_bytes());
        b[16..16 + card.len()].copy_from_slice(card.as_bytes());
        b[48..56].copy_from_slice(b"usb-0000");
        b[84..88].copy_from_slice(&caps.to_ne_bytes());
        b[88..92].copy_from_slice(&device_caps.to_ne_bytes());
        b
    }

    /// A fake /sys and /dev with video nodes, and the answers their QUERYCAP gives.
    pub struct FakeMachine {
        pub roots: Roots,
        pub answers: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        pub asked: Arc<Mutex<Vec<String>>>,
    }

    impl FakeMachine {
        pub fn new(tag: &str) -> FakeMachine {
            let d = std::env::temp_dir().join(format!("iyakki-cam-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("sys/class/video4linux")).unwrap();
            std::fs::create_dir_all(d.join("dev")).unwrap();
            FakeMachine { roots: Roots { sys: d.join("sys"), dev: d.join("dev") }, answers: Default::default(), asked: Default::default() }
        }
        /// A node with a QUERYCAP answer (`None`: the device cannot be opened).
        pub fn plug(&self, node: &str, devno: &str, name: &str, caps: Option<Vec<u8>>) {
            let c = self.roots.sys.join("class/video4linux").join(node);
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("dev"), format!("{devno}\n")).unwrap();
            std::fs::write(c.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(self.roots.dev.join(node), b"").unwrap();
            let p = self.roots.dev.join(node).display().to_string();
            match caps {
                Some(c) => self.answers.lock().unwrap().insert(p, c),
                None => self.answers.lock().unwrap().remove(&p),
            };
        }
        pub fn unplug(&self, node: &str) {
            let _ = std::fs::remove_dir_all(self.roots.sys.join("class/video4linux").join(node));
            let _ = std::fs::remove_file(self.roots.dev.join(node));
        }
        /// A UVC webcam: a picture node and a metadata node.
        pub fn plug_webcam(&self, first: u32, card: &str) {
            let cap = CAP_VIDEO_CAPTURE | CAP_STREAMING | CAP_DEVICE_CAPS | super::super::v4l2::CAP_META_CAPTURE;
            self.plug(&format!("video{first}"), &format!("81:{first}"), card, Some(raw_caps("uvcvideo", card, cap, CAP_VIDEO_CAPTURE | CAP_STREAMING)));
            self.plug(&format!("video{}", first + 1), &format!("81:{}", first + 1), card, Some(raw_caps("uvcvideo", card, cap, super::super::v4l2::CAP_META_CAPTURE | CAP_STREAMING)));
        }
        pub fn presence(&self) -> Presence {
            let (answers, asked) = (self.answers.clone(), self.asked.clone());
            Presence::with(
                self.roots.clone(),
                Box::new(move |p: &Path| {
                    let k = p.display().to_string();
                    asked.lock().unwrap().push(k.clone());
                    answers.lock().unwrap().get(&k).and_then(|b| parse_caps(b))
                }),
            )
        }
        pub fn dir(&self) -> PathBuf {
            self.roots.sys.parent().unwrap().to_path_buf()
        }
    }

    impl Drop for FakeMachine {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.dir());
        }
    }

    #[test]
    fn capability_bytes_are_read_like_the_kernel_writes_them() {
        let all = CAP_VIDEO_CAPTURE | CAP_STREAMING | CAP_DEVICE_CAPS | super::super::v4l2::CAP_META_CAPTURE;
        let c = parse_caps(&raw_caps("uvcvideo", "HD 4MP WEBCAM: HD 4MP WEBCAM", all, super::super::v4l2::CAP_META_CAPTURE | CAP_STREAMING)).unwrap();
        assert_eq!((c.driver.as_str(), c.card.as_str(), c.bus.as_str()), ("uvcvideo", "HD 4MP WEBCAM: HD 4MP WEBCAM", "usb-0000"));
        assert!(!c.is_camera(), "the metadata node of a webcam: device_caps win over the device's capabilities");
        let c = parse_caps(&raw_caps("uvcvideo", "cam", all, CAP_VIDEO_CAPTURE | CAP_STREAMING)).unwrap();
        assert!(c.is_camera());
        // an old driver without device_caps: the device capabilities count
        let c = parse_caps(&raw_caps("gspca", "old", CAP_VIDEO_CAPTURE | CAP_STREAMING, 0)).unwrap();
        assert!(c.is_camera());
        // capture without streaming (read() only), output-only, multi-plane: not for this app
        assert!(!parse_caps(&raw_caps("x", "x", CAP_VIDEO_CAPTURE, 0)).unwrap().is_camera());
        assert!(!parse_caps(&raw_caps("x", "x", 0x2 | CAP_STREAMING, 0)).unwrap().is_camera());
        assert!(!parse_caps(&raw_caps("x", "x", super::super::v4l2::CAP_VIDEO_CAPTURE_MPLANE | CAP_STREAMING, 0)).unwrap().is_camera());
        assert!(parse_caps(&[0u8; 50]).is_none());
    }

    #[test]
    fn webcams_are_found_in_fake_sysfs_and_hotplug_is_noticed_without_asking_twice() {
        let m = FakeMachine::new("detect");
        let mut p = m.presence();
        assert!(!p.scan() && !p.available() && p.scanned());
        assert!(p.hint().contains("Connect a USB camera"));
        // the Fullhan webcam of the Mac mini: video0 pictures, video1 metadata
        m.plug_webcam(0, "HD 4MP WEBCAM");
        assert!(p.scan(), "a camera plugged in later is noticed on the next look");
        let cams = p.cameras();
        assert_eq!(cams.len(), 1);
        assert_eq!((cams[0].node.as_str(), cams[0].name.as_str()), ("video0", "HD 4MP WEBCAM"));
        assert_eq!(cams[0].path, m.roots.dev.join("video0"));
        assert_eq!(m.asked.lock().unwrap().len(), 2);
        // nothing changed: no device is asked again
        assert!(!p.scan());
        assert_eq!(m.asked.lock().unwrap().len(), 2);
        // a second camera, numbered past ten, sorts by number
        m.plug_webcam(10, "C270");
        m.plug("video2", "81:2", "loopback", Some(raw_caps("v4l2 loopback", "", CAP_VIDEO_CAPTURE | CAP_STREAMING, 0)));
        assert!(p.scan());
        assert_eq!(p.cameras().iter().map(|c| c.node.as_str()).collect::<Vec<_>>(), vec!["video0", "video2", "video10"]);
        assert_eq!(p.cameras()[1].name, "loopback", "no card name: the sysfs name");
        // unplugged: gone; the same node coming back with a new number is asked again
        m.unplug("video10");
        m.unplug("video11");
        m.unplug("video2");
        assert!(p.scan());
        assert_eq!(p.cameras().len(), 1);
        m.unplug("video0");
        m.unplug("video1");
        assert!(p.scan() && !p.available());
        let n = m.asked.lock().unwrap().len();
        m.plug_webcam(0, "HD 4MP WEBCAM");
        std::fs::write(m.roots.sys.join("class/video4linux/video0/dev"), "81:4\n").unwrap();
        assert!(p.scan() && p.available());
        assert!(m.asked.lock().unwrap().len() > n);
        // a node without its /dev entry or one that cannot be opened is no camera
        m.plug("video5", "81:5", "ghost", None);
        std::fs::remove_file(m.roots.dev.join("video0")).unwrap();
        p.scan();
        assert!(!p.available());
    }

    #[test]
    fn the_hint_says_why_there_is_no_camera() {
        let m = FakeMachine::new("hint");
        std::fs::create_dir_all(m.roots.sys.join("class/dmi/id")).unwrap();
        std::fs::write(m.roots.sys.join("class/dmi/id/sys_vendor"), "Apple Inc.\n").unwrap();
        let mut p = m.presence();
        p.scan();
        assert!(p.hint().contains("Mac mini has none built in") && p.hint().contains("T2"));
        assert_eq!(p.tile_hint(), "Connect a camera");
        let m2 = FakeMachine::new("hint2");
        m2.plug("video0", "81:0", "meta only", Some(raw_caps("uvcvideo", "x", super::super::v4l2::CAP_META_CAPTURE | CAP_STREAMING, 0)));
        let mut p2 = m2.presence();
        p2.scan();
        assert!(!p2.available() && p2.hint().contains("do not take pictures"));
        assert!(!Presence::none().available());
    }
}
