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

/// Is the Broadcom FaceTime HD camera (PCI 14e4:1570, a MacBook's built-in camera) among the PCI devices?
pub fn has_facetime_pci(sys: &Path) -> bool {
    let rd = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten().any(|e| rd(&e.path(), "vendor") == "0x14e4" && rd(&e.path(), "device") == "0x1570")
}

/// A camera found on the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamDevice {
    /// `video0`
    pub node: String,
    pub path: PathBuf,
    /// The name from sysfs (`/sys/class/video4linux/videoN/name`), cleaned up; the card name when sysfs has none.
    pub name: String,
    pub caps: Caps,
    /// The built-in camera of the machine (FaceTime HD, an integrated webcam), not a plugged-in one.
    pub builtin: bool,
    /// Which physical camera this node belongs to: the nodes of one device (pictures, metadata) share it.
    pub physical: String,
}

/// A sysfs camera name without the trailing colon the UVC driver leaves (`FaceTime HD Camera (Built-in):`) and without the
/// doubled form (`HD 4MP WEBCAM: HD 4MP WEBCAM`).
pub fn clean_name(raw: &str) -> String {
    let t = raw.trim();
    if let Some((a, b)) = t.split_once(": ") {
        if a.trim() == b.trim() {
            return a.trim().to_string();
        }
    }
    t.trim_end_matches(':').trim().to_string()
}

/// Is this the machine's own camera? By its driver or name, or because the USB port says the device is fixed.
pub fn is_builtin(name: &str, driver: &str, removable: &str) -> bool {
    let n = name.to_ascii_lowercase();
    driver == "facetimehd" || n.contains("built-in") || n.contains("builtin") || n.contains("facetime") || n.contains("integrated") || n.contains("internal") || removable.trim() == "fixed"
}

pub type Query = Box<dyn Fn(&Path) -> Option<Caps> + Send + Sync>;

/// The cameras on the machine, kept up to date by [`Presence::scan`].
pub struct Presence {
    roots: Roots,
    query: Query,
    /// node -> (its `dev` number, the camera when it is one)
    known: BTreeMap<String, (String, Option<CamDevice>)>,
    apple: bool,
    /// The Broadcom FaceTime HD camera (PCI 14e4:1570) of a MacBook is on this machine.
    facetime_pci: bool,
    scanned: bool,
    /// Lower-case name parts of cameras the person may not use; they are left out of the list.
    hidden: Vec<String>,
    /// A camera that is not a device node of this machine: the web browser's camera of a remote session (`browsercam`).
    browser: Option<CamDevice>,
}

fn node_number(n: &str) -> u32 {
    n.trim_start_matches("video").parse().unwrap_or(u32::MAX)
}

impl Presence {
    /// The real machine, asking devices through V4L2.
    pub fn system() -> Presence {
        Presence::with(Roots::system(), Box::new(|p: &Path| super::v4l2::query_caps(p)))
    }

    /// Does this one look at the real `/sys` and `/dev` (not the empty placeholder of [`Presence::none`] or a test's folders)?
    pub fn looks_at_the_machine(&self) -> bool {
        self.roots == Roots::system()
    }

    pub fn with(roots: Roots, query: Query) -> Presence {
        let apple = std::fs::read_to_string(roots.sys.join("class/dmi/id/sys_vendor")).is_ok_and(|v| v.contains("Apple"));
        let facetime_pci = has_facetime_pci(&roots.sys);
        Presence { roots, query, known: BTreeMap::new(), apple, facetime_pci, scanned: false, hidden: Vec::new(), browser: None }
    }

    /// Nothing at all: no camera, never looks (the tests' default).
    pub fn none() -> Presence {
        Presence::with(Roots { sys: "/nonexistent-iyakki-sys".into(), dev: "/nonexistent-iyakki-dev".into() }, Box::new(|_| None))
    }

    /// The only camera is the web browser's (a remote session has no camera of its own to look at).
    pub fn browser() -> Presence {
        let mut p = Presence::none();
        p.browser = Some(CamDevice { node: "browser".into(), path: PathBuf::from("browser"), name: "Browser camera".into(), caps: Caps { driver: "browser".into(), card: "Browser camera".into(), bus: "web".into(), caps: CAP_VIDEO_CAPTURE | CAP_STREAMING }, builtin: true, physical: "browser".into() });
        p
    }

    /// Look again; true when the list of cameras changed.
    pub fn scan(&mut self) -> bool {
        let before = self.cameras();
        let class = self.roots.sys.join("class/video4linux");
        let mut seen = Vec::new();
        if !self.scanned {
            let n = std::fs::read_dir(&class).map(|r| r.flatten().count());
            log::info!("camera: first look at {}: {}", class.display(), match n { Ok(n) => format!("{n} video node(s)"), Err(e) => format!("cannot be read ({e})") });
        }
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
                let answer = (self.query)(&path);
                // one line per node when it is first asked: the log says why a camera that the kernel found is not in the list
                match &answer {
                    Some(c) => log::info!("camera: {node} ({devno}) \"{sysname}\": driver {}, caps {:#x}, {}", c.driver, c.caps, if c.is_camera() { "takes pictures" } else { "does not take pictures (no capture, or no streaming)" }),
                    None => log::info!("camera: {node} ({devno}) \"{sysname}\": the device did not answer VIDIOC_QUERYCAP (could not be opened, or is not a V4L2 device)"),
                }
                let cam = answer.filter(Caps::is_camera).map(|caps| {
                    let cleaned = clean_name(&sysname);
                    let name = if !cleaned.is_empty() { cleaned } else if !caps.card.is_empty() { clean_name(&caps.card) } else { node.clone() };
                    // the USB device above the interface: its path tells the nodes of one camera apart from another camera's
                    let usb = std::fs::canonicalize(class.join(&node).join("device")).ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
                    let removable = usb.as_ref().map(|d| std::fs::read_to_string(d.join("removable")).unwrap_or_default()).unwrap_or_default();
                    let physical = usb.map(|d| d.display().to_string()).unwrap_or_else(|| format!("{}|{}", caps.bus, caps.card));
                    let builtin = is_builtin(&name, &caps.driver, &removable);
                    CamDevice { node: node.clone(), path: path.clone(), name, caps, builtin, physical }
                });
                self.known.insert(node, (devno, cam));
            }
        }
        self.known.retain(|n, _| seen.contains(n));
        self.scanned = true;
        self.cameras() != before
    }

    /// Cameras the person may not use are left out of [`Presence::cameras`].
    pub fn set_hidden(&mut self, names: Vec<String>) {
        self.hidden = names.into_iter().map(|n| n.to_ascii_lowercase()).filter(|n| !n.is_empty()).collect();
    }

    /// One entry per physical camera, in node order (video0, video2, video10): the picture node of each (the lowest when
    /// a device shows several, so the metadata node of a webcam never counts), without the cameras that are hidden.
    pub fn cameras(&self) -> Vec<CamDevice> {
        let mut all: Vec<CamDevice> = self.known.values().filter_map(|(_, c)| c.clone()).collect();
        all.sort_by_key(|c| node_number(&c.node));
        let mut v: Vec<CamDevice> = Vec::new();
        for c in all {
            if v.iter().any(|o| o.physical == c.physical) {
                continue;
            }
            let n = c.name.to_ascii_lowercase();
            if self.hidden.iter().any(|h| n.contains(h.as_str())) {
                continue;
            }
            v.push(c);
        }
        if let Some(b) = &self.browser {
            v.insert(0, b.clone());
        }
        v
    }

    pub fn available(&self) -> bool {
        !self.cameras().is_empty()
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
        if self.apple && self.facetime_pci {
            "No camera found. The built-in FaceTime HD camera needs Apple's firmware, which IYAKKI never ships: open STORE, Setup, Hardware and press Install camera driver and firmware (the driver is in this image; the firmware is downloaded from Apple for this Mac). A USB camera works at once."
        } else if self.apple {
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
    fn the_placeholder_scanner_is_told_apart_from_the_real_one() {
        // the shell swaps the empty placeholder for the real scanner when the machine turns out to be a desktop
        assert!(!Presence::none().looks_at_the_machine());
        assert!(Presence::system().looks_at_the_machine());
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
    fn a_facetime_hd_camera_is_a_normal_video_node_and_without_its_firmware_the_hint_points_to_the_camera_firmware() {
        let m = FakeMachine::new("facetime");
        let w = |rel: &str, s: &str| {
            let p = m.roots.sys.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        w("class/dmi/id/sys_vendor", "Apple Inc.\n");
        w("bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n");
        w("bus/pci/devices/0000:03:00.0/device", "0x1570\n");
        let mut p = m.presence();
        p.scan();
        assert!(!p.available() && p.hint().contains("FaceTime HD") && p.hint().contains("camera driver and firmware") && p.hint().contains("USB camera"));
        // the driver's node: capture + read/write + streaming, device_caps given (what fthd_v4l2_ioctl_querycap sets), card "Apple Facetime HD"
        let caps = CAP_VIDEO_CAPTURE | CAP_STREAMING | 0x0100_0000 /* READWRITE */;
        m.plug("video0", "81:0", "Apple Facetime HD", Some(raw_caps("facetimehd", "Apple Facetime HD", caps | CAP_DEVICE_CAPS, caps)));
        assert!(p.scan() && p.available());
        let c = &p.cameras()[0];
        assert_eq!((c.node.as_str(), c.name.as_str(), c.caps.driver.as_str()), ("video0", "Apple Facetime HD", "facetimehd"));
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
