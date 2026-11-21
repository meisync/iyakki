//! Graphics acceleration of the full web engine on this machine (F-BROX-24, F-BROX-25): the machine's facts for
//! [`iyakki_shared::webaccel::plan`] and the engine's extra environment. The person's, the site's and the administrator's switches are in
//! `<data>/web-profile/web-accel.json` ([`iyakki_shared::webaccel`]).

use std::path::Path;

use iyakki_shared::gpu;
use iyakki_shared::webaccel::{self as acc, Machine, Plan};

fn has_lib(root: &Path, names: &[&str]) -> bool {
    ["usr/lib", "usr/lib64", "lib", "lib64", "usr/lib/x86_64-linux-gnu", "usr/lib/aarch64-linux-gnu"].iter().any(|d| names.iter().any(|n| root.join(d).join(n).exists()))
}

fn has_va(root: &Path) -> bool {
    let dri = ["usr/lib/dri", "usr/lib64/dri", "usr/lib/x86_64-linux-gnu/dri", "usr/lib/aarch64-linux-gnu/dri"].iter().any(|d| {
        std::fs::read_dir(root.join(d)).map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().ends_with("_drv_video.so"))).unwrap_or(false)
    });
    let plugin = ["usr/lib/gstreamer-1.0", "usr/lib64/gstreamer-1.0", "usr/lib/x86_64-linux-gnu/gstreamer-1.0", "usr/lib/aarch64-linux-gnu/gstreamer-1.0"].iter().any(|d| root.join(d).join("libgstva.so").exists());
    dri && plugin
}

/// What the machine under `root` offers: a real (not virtual) graphics device with a render node and a bound driver, the EGL and OpenGL ES
/// libraries, and for video the VA-API driver and GStreamer's VA plug-in. Nothing is opened.
pub fn machine_from(root: &Path, software_mode: bool) -> Machine {
    let gpus = gpu::detect(root);
    let real: Vec<_> = gpus.iter().filter(|g| !g.is_virtual()).collect();
    let with_node = real.iter().any(|g| g.render.is_some() && g.driver.is_some());
    let libs = has_lib(root, &["libEGL.so.1", "libEGL.so"]) && has_lib(root, &["libGLESv2.so.2", "libGLESv2.so"]);
    let reason = if gpus.is_empty() || real.is_empty() {
        "no graphics card found (a virtual display draws on the processor)"
    } else if !with_node {
        "no render node: the graphics driver is not loaded"
    } else if !libs {
        "the graphics libraries (EGL and OpenGL ES) are not installed"
    } else {
        ""
    };
    Machine { node_usable: reason.is_empty(), node_reason: reason.to_string(), va_driver: has_va(root), software_mode }
}

/// The plan for one person (the profile folder's name) and one site; `host` empty gives the person-wide plan.
pub fn plan_for(data: &Path, root: &Path, user: &str, host: &str, software_mode: bool) -> Plan {
    let (pol, _) = acc::load(&data.join("web-profile"));
    acc::plan(&machine_from(root, software_mode), &pol.verdict(user, host))
}

/// The user a profile folder (`<data>/web-profile/<user>`) belongs to.
pub fn user_of_profile(profile: &Path) -> String {
    profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
#[path = "webaccel_tests.rs"]
mod tests;
