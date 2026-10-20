//! USB cameras that the stock `uvcvideo` driver only runs with a quirk.
//!
//! Some cheap UVC webcams answer the "probe control" request (how driver and camera agree on a format) wrongly, and
//! `uvcvideo` gives up with `Failed to query (129) UVC probe control : -32` or `Failed to set UVC probe control : -110`;
//! no `/dev/video*` appears. The driver has a `quirks` module parameter that changes how it talks to the camera, but it
//! is global: it applies to every camera probed while it is set. So for one camera we set it, bind that camera's video
//! control interface, and put the old value back at once (`uvcvideo` reads the parameter only while probing).
//!
//! **Who loads the drivers.** On a real Mac mini the camera enumerated while IYAKKI was starting, the kernel's hotplug
//! helper (`mdev`, `modprobe $MODALIAS`) loaded `uvcvideo` and `snd-usb-audio` at once, `uvcvideo` probed the camera
//! without the quirk at 3.7 s, and by the time a quirk was tried the camera no longer answered. So at the very start of
//! boot [`block_autoload`] writes a modprobe.d file that blacklists both drivers for alias (hotplug) loading, and IYAKKI
//! loads them itself by name ([`prepare_boot`], and [`handle_device`] for hot plugs) after a known camera's microphone
//! is off and its video interfaces are held back (`authorized=0`) until they can be bound with the quirk. Its first
//! probe then already has the quirk.
//!
//! Parts:
//! - [`CAMERA_QUIRKS`]: cameras known to need a quirk (vendor:product -> quirks value), and whether their built-in
//!   microphone (USB audio interfaces) is kept off.
//! - A generic fallback for any other UVC camera: no video node after it appeared, and the kernel log shows a probe
//!   control failure for it -> try once with [`UVC_QUIRK_PROBE_DEF`], then [`UVC_QUIRK_PROBE_MINMAX`].
//! - A camera that stopped answering (`-110`, timed out) is not retried for minutes: each try can block in the kernel for
//!   many 5 s USB timeouts. The log says to replug it.
//!
//! Everything goes through [`Sysfs`] (paths relative to the sysfs root) and [`Runner`], so tests run on a fake. Nothing
//! here sleeps or reads the kernel log by itself: the caller passes those in.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::run::Runner;

/// `UVC_QUIRK_PROBE_MINMAX`: ask the camera for its minimum and maximum before setting the probe control.
pub const UVC_QUIRK_PROBE_MINMAX: u32 = 0x2;
/// `UVC_QUIRK_PROBE_DEF`: use the camera's default probe values (GET_DEF) instead of querying the current ones.
pub const UVC_QUIRK_PROBE_DEF: u32 = 0x100;
/// What the generic fallback tries, in order.
pub const FALLBACK_QUIRKS: [u32; 2] = [UVC_QUIRK_PROBE_DEF, UVC_QUIRK_PROBE_MINMAX];

/// One camera known to need a `uvcvideo` quirk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraQuirk {
    pub vendor: u16,
    pub product: u16,
    /// The name the camera reports, for logs and docs.
    pub name: &'static str,
    /// The value written to `/sys/module/uvcvideo/parameters/quirks` while its video interface binds.
    pub uvc_quirks: u32,
    /// Keep its USB audio interfaces (the built-in microphone) unauthorized, so `snd-usb-audio` never talks to it.
    pub audio_off: bool,
    /// Why, and how sure we are.
    pub note: &'static str,
}

/// Cameras known to need a quirk. Keep this small and only add what was seen on real hardware. Every camera in the
/// table is also kept out of USB autosuspend (`power/control=on`), a precaution: an idle camera without a driver is
/// suspended after 2 s, and a cheap one may not wake up properly.
pub const CAMERA_QUIRKS: &[CameraQuirk] = &[CameraQuirk {
    vendor: 0x1d6c,
    product: 0x0103,
    name: "SHENZHEN Fullhan HD 4MP WEBCAM",
    uvc_quirks: UVC_QUIRK_PROBE_DEF,
    audio_off: true,
    note: "Seen on a Mac mini 2018 (kernel 6.12): without a quirk uvcvideo fails with 'Failed to query (129) UVC probe control : -32'; \
           with 0x100 /dev/video0 and /dev/video1 appear. The microphone is kept off as a precaution: once snd-usb-audio's \
           'cannot get freq at ep 0x89' came right before the camera stopped answering until replugged, but that the \
           microphone query caused it is not proven.",
}];

/// The known entry for this vendor:product.
pub fn known(vendor: u16, product: u16) -> Option<&'static CameraQuirk> {
    CAMERA_QUIRKS.iter().find(|q| q.vendor == vendor && q.product == product)
}

/// Sysfs access with paths relative to its root (`bus/usb/devices/1-8/idVendor`).
pub trait Sysfs {
    fn read(&self, rel: &str) -> Option<String>;
    fn write(&self, rel: &str, value: &str) -> Result<(), String>;
    /// Entry names in a folder, sorted; empty when it is missing.
    fn list(&self, rel: &str) -> Vec<String>;
    fn exists(&self, rel: &str) -> bool;
}

/// The real sysfs (root `/sys`), or a folder standing in for it.
pub struct RealSysfs {
    pub root: PathBuf,
}

impl RealSysfs {
    pub fn system() -> RealSysfs {
        RealSysfs { root: PathBuf::from("/sys") }
    }
}

impl Sysfs for RealSysfs {
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }
    fn write(&self, rel: &str, value: &str) -> Result<(), String> {
        std::fs::write(self.root.join(rel), value).map_err(|e| format!("{rel}: {e}"))
    }
    fn list(&self, rel: &str) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.root.join(rel)).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
}

pub const USB_DEVICES: &str = "bus/usb/devices";
pub const UVC_DRIVER: &str = "bus/usb/drivers/uvcvideo";
pub const UVC_QUIRKS_PARAM: &str = "module/uvcvideo/parameters/quirks";
/// Present while `snd-usb-audio` is loaded.
pub const SND_USB_AUDIO_MODULE: &str = "module/snd_usb_audio";

/// The modprobe.d file [`block_autoload`] writes.
pub const MODPROBE_CONF: &str = "iyakki-camera.conf";

/// Its text: both drivers blacklisted for alias loading (the hotplug helper's `modprobe $MODALIAS`, and IYAKKI's own
/// `modprobe -b` coldplug). Loading by name (`modprobe uvcvideo`, without `-b`) still works; that is how IYAKKI loads them.
pub fn modprobe_conf() -> String {
    "# Written by IYAKKI at every boot (crates/iyakki-drivers/src/camquirk.rs). IYAKKI loads these two itself, after\n\
     # cameras that need a uvcvideo quirk are held back, so their first probe already has the quirk.\n\
     blacklist uvcvideo\n\
     blacklist snd_usb_audio\n"
        .to_string()
}

/// Write [`modprobe_conf`] into `modprobe_d` (`/etc/modprobe.d`, in RAM on the device). Must run before the kernel's
/// hotplug helper is set and before any coldplug.
pub fn block_autoload(modprobe_d: &Path) -> Result<(), String> {
    std::fs::create_dir_all(modprobe_d).map_err(|e| format!("{}: {e}", modprobe_d.display()))?;
    let p = modprobe_d.join(MODPROBE_CONF);
    std::fs::write(&p, modprobe_conf()).map_err(|e| format!("{}: {e}", p.display()))
}

/// One interface of a USB device (`1-8:1.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub class: u8,
    pub subclass: u8,
}

impl Interface {
    /// UVC video control (class 0x0e, subclass 1): the interface `uvcvideo` binds to.
    pub fn is_video_control(&self) -> bool {
        self.class == 0x0e && self.subclass == 1
    }
    /// Any video class interface (control or streaming).
    pub fn is_video(&self) -> bool {
        self.class == 0x0e
    }
    pub fn is_audio(&self) -> bool {
        self.class == 0x01
    }
}

fn hex(s: Option<String>) -> Option<u32> {
    u32::from_str_radix(s?.trim().trim_start_matches("0x"), 16).ok()
}

/// vendor:product of a USB device (`1-8`).
pub fn ids(sys: &dyn Sysfs, dev: &str) -> Option<(u16, u16)> {
    let v = hex(sys.read(&format!("{USB_DEVICES}/{dev}/idVendor")))?;
    let p = hex(sys.read(&format!("{USB_DEVICES}/{dev}/idProduct")))?;
    Some((v as u16, p as u16))
}

/// `1d6c:0103`
pub fn id_text(ids: (u16, u16)) -> String {
    format!("{:04x}:{:04x}", ids.0, ids.1)
}

/// The interfaces of the active configuration of a device, sorted by name.
pub fn interfaces(sys: &dyn Sysfs, dev: &str) -> Vec<Interface> {
    let prefix = format!("{dev}:");
    sys.list(USB_DEVICES)
        .into_iter()
        .filter(|n| n.starts_with(&prefix))
        .filter_map(|n| {
            let class = hex(sys.read(&format!("{USB_DEVICES}/{n}/bInterfaceClass")))? as u8;
            let subclass = hex(sys.read(&format!("{USB_DEVICES}/{n}/bInterfaceSubClass"))).unwrap_or(0) as u8;
            Some(Interface { name: n, class, subclass })
        })
        .collect()
}

/// Are all the interfaces the device announces there yet (`bNumInterfaces`)? Right after a hotplug event they are not.
pub fn configured(sys: &dyn Sysfs, dev: &str) -> bool {
    let want = sys.read(&format!("{USB_DEVICES}/{dev}/bNumInterfaces")).and_then(|s| s.trim().parse::<usize>().ok());
    match want {
        Some(n) if n > 0 => interfaces(sys, dev).len() >= n,
        _ => false,
    }
}

/// The video nodes (`video0`, ...) of a device's interfaces.
pub fn video_nodes(sys: &dyn Sysfs, dev: &str) -> Vec<String> {
    interfaces(sys, dev).iter().flat_map(|i| sys.list(&format!("{USB_DEVICES}/{}/video4linux", i.name))).filter(|n| n.starts_with("video")).collect()
}

/// USB devices (`1-8`, `2-1.3`), root hubs left out.
pub fn usb_devices(sys: &dyn Sysfs) -> Vec<String> {
    sys.list(USB_DEVICES).into_iter().filter(|n| !n.contains(':') && !n.starts_with("usb")).collect()
}

/// Known cameras that are plugged in, with their table entry.
pub fn known_present(sys: &dyn Sysfs) -> Vec<(String, &'static CameraQuirk)> {
    usb_devices(sys).into_iter().filter_map(|d| ids(sys, &d).and_then(|id| known(id.0, id.1)).map(|q| (d, q))).collect()
}

fn authorized(sys: &dyn Sysfs, iface: &str) -> bool {
    sys.read(&format!("{USB_DEVICES}/{iface}/authorized")).is_none_or(|v| v.trim() != "0")
}

/// Set `authorized` on the interfaces `pick` selects, only where it changes. Returns the ones changed.
fn set_authorized(sys: &dyn Sysfs, dev: &str, pick: fn(&Interface) -> bool, on: bool) -> Result<Vec<String>, String> {
    let mut changed = Vec::new();
    for i in interfaces(sys, dev).into_iter().filter(|i| pick(i)) {
        if authorized(sys, &i.name) == on {
            continue;
        }
        sys.write(&format!("{USB_DEVICES}/{}/authorized", i.name), if on { "1" } else { "0" })?;
        changed.push(i.name);
    }
    Ok(changed)
}

/// Turn off the audio interfaces of a device (deauthorizing an interface also unbinds its driver). Returns what was
/// turned off; interfaces already off are left alone.
pub fn audio_off(sys: &dyn Sysfs, dev: &str) -> Result<Vec<String>, String> {
    set_authorized(sys, dev, Interface::is_audio, false)
}

/// Keep a known camera out of USB autosuspend (a precaution; see [`CAMERA_QUIRKS`]).
pub fn keep_awake(sys: &dyn Sysfs, dev: &str) {
    let p = format!("{USB_DEVICES}/{dev}/power/control");
    if sys.read(&p).is_some_and(|v| v.trim() != "on") {
        let _ = sys.write(&p, "on");
    }
}

pub fn uvcvideo_loaded(sys: &dyn Sysfs) -> bool {
    sys.exists(UVC_QUIRKS_PARAM)
}

/// Load `uvcvideo` by name if it is not loaded. Known cameras' video interfaces are held back (`authorized=0`) first,
/// so the driver's first look at them is the bind with the quirk, not a stock probe.
pub fn load_uvcvideo(sys: &dyn Sysfs, runner: &dyn Runner) -> Result<(), String> {
    if uvcvideo_loaded(sys) {
        return Ok(());
    }
    for (dev, _) in known_present(sys) {
        set_authorized(sys, &dev, Interface::is_video, false)?;
    }
    runner.run("modprobe", &["uvcvideo"])?;
    if uvcvideo_loaded(sys) {
        Ok(())
    } else {
        Err("uvcvideo is not loaded and has no quirks parameter".into())
    }
}

/// Load `snd-usb-audio` by name if it is not loaded.
pub fn load_snd_usb_audio(sys: &dyn Sysfs, runner: &dyn Runner) -> Result<(), String> {
    if sys.exists(SND_USB_AUDIO_MODULE) {
        return Ok(());
    }
    runner.run("modprobe", &["snd-usb-audio"])
}

/// Bind one video control interface with `quirks` set, then restore the previous global value whatever happened. Video
/// interfaces of the device that were held back are authorized again just before the bind.
pub fn bind_with_quirks(sys: &dyn Sysfs, runner: &dyn Runner, iface: &str, quirks: u32) -> Result<(), String> {
    load_uvcvideo(sys, runner)?;
    let dev = iface.split(':').next().unwrap_or(iface);
    let previous = sys.read(UVC_QUIRKS_PARAM).map(|s| s.trim().to_string()).ok_or("cannot read the uvcvideo quirks parameter")?;
    sys.write(UVC_QUIRKS_PARAM, &format!("0x{quirks:x}"))?;
    let bound = (|| {
        set_authorized(sys, dev, Interface::is_video, true)?;
        if sys.exists(&format!("{USB_DEVICES}/{iface}/driver")) {
            sys.write(&format!("{UVC_DRIVER}/unbind"), iface)?;
        }
        sys.write(&format!("{UVC_DRIVER}/bind"), iface)
    })();
    let restored = sys.write(UVC_QUIRKS_PARAM, &previous);
    bound?;
    restored.map_err(|e| format!("could not restore the uvcvideo quirks parameter to {previous}: {e}"))
}

/// The part of the kernel log since this device was last enumerated (`usb 1-8: new high-speed USB device number 5`), so
/// failures from before a replug do not count.
fn since_plugged<'a>(kernel_log: &'a str, dev: &str) -> &'a str {
    let tag = format!("usb {dev}: new ");
    kernel_log.rfind(&tag).map_or(kernel_log, |i| &kernel_log[i..])
}

/// The error of the latest UVC probe control failure for this device since it was plugged in (`uvcvideo 1-8:1.1: Failed
/// to query (129) UVC probe control : -32 (exp. 26).` -> -32). `can't set config #1, error -110` counts as -110.
pub fn last_probe_failure(kernel_log: &str, dev: &str) -> Option<i32> {
    let tag = format!(" {dev}:");
    let mut last = None;
    for l in since_plugged(kernel_log, dev).lines().filter(|l| l.contains(&tag)) {
        let code = if let Some((_, r)) = l.split_once("UVC probe control : ") {
            r.split_whitespace().next()
        } else if l.contains("can't set config") {
            l.rsplit_once("error ").map(|(_, r)| r.trim())
        } else {
            None
        };
        if let Some(c) = code.and_then(|c| c.trim_end_matches('.').parse::<i32>().ok()) {
            last = Some(c);
        }
    }
    last
}

/// Did uvcvideo report a probe control failure for this device since it was plugged in?
pub fn probe_control_failed(kernel_log: &str, dev: &str) -> bool {
    last_probe_failure(kernel_log, dev).is_some()
}

/// `-110`: the camera did not answer at all; more tries only block in the kernel.
fn stopped_answering(kernel_log: &str, dev: &str) -> bool {
    last_probe_failure(kernel_log, dev) == Some(-110)
}

/// How long an unknown camera gets for its stock probe before the fallback steps in (it steps in at once when the kernel
/// log already shows the failure).
pub const SETTLE: Duration = Duration::from_secs(4);
/// How long a bind that returned success gets to show its video nodes (they are normally there when the bind returns).
pub const NODE_LIMIT: Duration = Duration::from_secs(1);
/// Pause before the one retry of a known camera's bind that failed (`No such device`).
pub const RETRY_PAUSE: Duration = Duration::from_millis(500);
/// How long to wait for a just-plugged device to have all its interfaces.
pub const CONFIGURE_LIMIT: Duration = Duration::from_secs(3);
const STEP: Duration = Duration::from_millis(100);

/// Poll every 100 ms for up to `limit`, until the device has a video node or `stop` says so.
fn poll(sys: &dyn Sysfs, dev: &str, limit: Duration, wait: &mut dyn FnMut(Duration), stop: &dyn Fn() -> bool) -> Vec<String> {
    let mut spent = Duration::ZERO;
    loop {
        let n = video_nodes(sys, dev);
        if !n.is_empty() || spent >= limit || stop() {
            return n;
        }
        wait(STEP);
        spent += STEP;
    }
}

/// At boot, before the deferred drivers load and after [`block_autoload`] kept the hotplug helper from loading them:
/// known cameras are kept awake, lose their microphone and have their video held back; then `uvcvideo` (when any UVC
/// camera is present) and `snd-usb-audio` (when any USB audio interface is present) are loaded by name. The quirk binds
/// are left to [`handle_device`] in a background thread (a camera that does not answer can block a bind for minutes).
pub fn prepare_boot(sys: &dyn Sysfs, runner: &dyn Runner) -> Vec<String> {
    let mut out = Vec::new();
    for (dev, q) in known_present(sys) {
        keep_awake(sys, &dev);
        if !q.audio_off {
            continue;
        }
        let idt = ids(sys, &dev).map(id_text).unwrap_or_default();
        match audio_off(sys, &dev) {
            Ok(off) if !off.is_empty() => out.push(format!("camera: {idt} ({}) microphone kept off as a precaution ({})", q.name, off.join(", "))),
            Ok(_) => {}
            Err(e) => out.push(format!("camera: {idt}: could not turn its microphone off: {e}")),
        }
    }
    let all: Vec<Interface> = usb_devices(sys).iter().flat_map(|d| interfaces(sys, d)).collect();
    if all.iter().any(Interface::is_video_control) {
        if let Err(e) = load_uvcvideo(sys, runner) {
            out.push(format!("camera: loading uvcvideo failed: {e}"));
        }
    }
    if all.iter().any(Interface::is_audio) {
        if let Err(e) = load_snd_usb_audio(sys, runner) {
            out.push(format!("camera: loading snd-usb-audio failed: {e}"));
        }
    }
    out
}

/// Everything for one USB device that just appeared (boot or hotplug). A known camera is bound with its quirk at once
/// (one retry after [`RETRY_PAUSE`] if the bind fails); any other UVC camera gets the generic fallback. Devices with USB
/// audio get `snd-usb-audio` loaded (after a known camera's microphone is off). Devices without a UVC video control
/// interface give no log lines. `kernel_log` returns the current kernel log; `wait` sleeps.
pub fn handle_device(sys: &dyn Sysfs, runner: &dyn Runner, dev: &str, kernel_log: &dyn Fn() -> String, wait: &mut dyn FnMut(Duration)) -> Vec<String> {
    let mut out = Vec::new();
    let Some(id) = ids(sys, dev) else { return out };
    let mut spent = Duration::ZERO;
    while !configured(sys, dev) && spent < CONFIGURE_LIMIT {
        wait(STEP);
        spent += STEP;
    }
    let idt = id_text(id);
    let entry = known(id.0, id.1);
    if let Some(q) = entry {
        keep_awake(sys, dev);
        if q.audio_off {
            match audio_off(sys, dev) {
                Ok(off) if !off.is_empty() => out.push(format!("camera: {idt} microphone kept off as a precaution ({})", off.join(", "))),
                Ok(_) => {}
                Err(e) => out.push(format!("camera: {idt}: could not turn its microphone off: {e}")),
            }
        }
    }
    let ifaces = interfaces(sys, dev);
    if ifaces.iter().any(Interface::is_audio) {
        if let Err(e) = load_snd_usb_audio(sys, runner) {
            out.push(format!("camera: {idt}: loading snd-usb-audio failed: {e}"));
        }
    }
    let Some(vc) = ifaces.into_iter().find(Interface::is_video_control) else { return out };
    let wedged = |out: &mut Vec<String>| out.push(format!("camera: {idt} stopped answering (UVC probe control timed out, -110); unplug it and plug it in again"));
    let mut tried: Vec<u32> = Vec::new();
    if let Some(q) = entry.filter(|_| video_nodes(sys, dev).is_empty()) {
        tried.push(q.uvc_quirks);
        for attempt in 0..2 {
            match bind_with_quirks(sys, runner, &vc.name, q.uvc_quirks) {
                Ok(()) => {
                    let nodes = poll(sys, dev, NODE_LIMIT, wait, &|| false);
                    if !nodes.is_empty() {
                        out.push(format!("camera: {idt} needed uvcvideo quirk 0x{:x} ({})", q.uvc_quirks, nodes.join(", ")));
                        return out;
                    }
                    break;
                }
                Err(e) => {
                    out.push(format!("camera: {idt} ({}): binding with uvcvideo quirk 0x{:x} failed: {e}", q.name, q.uvc_quirks));
                    if stopped_answering(&kernel_log(), dev) {
                        wedged(&mut out);
                        return out;
                    }
                    if attempt == 0 {
                        wait(RETRY_PAUSE);
                    }
                }
            }
        }
    } else if !uvcvideo_loaded(sys) {
        // a hot plug with no camera at boot: the hotplug helper may not load uvcvideo (blacklisted), so load it here
        if let Err(e) = load_uvcvideo(sys, runner) {
            out.push(format!("camera: {idt}: loading uvcvideo failed: {e}"));
            return out;
        }
    }
    let nodes = poll(sys, dev, SETTLE, wait, &|| probe_control_failed(&kernel_log(), dev));
    if !nodes.is_empty() {
        if !tried.is_empty() {
            out.push(format!("camera: {idt} needed uvcvideo quirk 0x{:x} ({})", tried[0], nodes.join(", ")));
        }
        return out;
    }
    let log = kernel_log();
    if !probe_control_failed(&log, dev) {
        out.push(format!("camera: {idt} has no video device yet and uvcvideo reported no probe control failure; no quirk tried"));
        return out;
    }
    if stopped_answering(&log, dev) {
        wedged(&mut out);
        return out;
    }
    let todo: Vec<u32> = FALLBACK_QUIRKS.into_iter().filter(|q| !tried.contains(q)).collect();
    for q in todo {
        tried.push(q);
        if let Err(e) = bind_with_quirks(sys, runner, &vc.name, q) {
            out.push(format!("camera: {idt}: retry with uvcvideo quirk 0x{q:x} failed: {e}"));
            if stopped_answering(&kernel_log(), dev) {
                wedged(&mut out);
                return out;
            }
            continue;
        }
        let nodes = poll(sys, dev, NODE_LIMIT, wait, &|| false);
        if !nodes.is_empty() {
            out.push(format!("camera: {idt} needed uvcvideo quirk 0x{q:x} ({})", nodes.join(", ")));
            return out;
        }
    }
    let list: Vec<String> = tried.iter().map(|q| format!("0x{q:x}")).collect();
    out.push(format!("camera: {idt} still has no video device after uvcvideo quirks {}; unplug it and plug it in again", list.join(", ")));
    out
}

#[cfg(test)]
#[path = "camquirk_tests.rs"]
mod tests;
