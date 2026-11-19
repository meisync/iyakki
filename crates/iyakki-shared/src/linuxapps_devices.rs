//! Sound and graphics hardware for a Linux sandbox (F-LINAPP-18): which device nodes of the computer a sandbox may be given, chosen by
//! two switches that are off by default and set by the person (`linux_devices_set`, a card every time). Nothing here opens a device.
//!
//! * **Sound**: the playback PCM nodes of the sound cards (`/dev/snd/pcmC<n>D<m>p`) and the control nodes (`controlC<n>`) that ALSA
//!   libraries open first. No capture node: a program that listens through a microphone is a different grant. A program that opens a card
//!   directly holds it for itself (the planned own mixer will share it); the person is told so on the card.
//! * **Graphics**: the render nodes (`/dev/dri/renderD<n>`), which allow drawing and compute without taking over the screen. Never the
//!   `card<n>` nodes (display control) and never a framebuffer.
//!
//! The list is read from the computer's `/dev` when the sandbox starts, so a card that was plugged in later is found; the list is
//! checked again by [`node_ok`] where the sandbox is built, so a hand-made command line cannot name another device.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePolicy {
    pub sound: bool,
    pub gpu: bool,
}

impl DevicePolicy {
    /// A damaged or missing file reads as everything off.
    pub fn load(file: &Path) -> DevicePolicy {
        std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> Result<(), String> {
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, file).map_err(|e| e.to_string())
    }

    /// A sentence for the page, the card and the listing.
    pub fn describe(&self) -> String {
        match (self.sound, self.gpu) {
            (false, false) => "No sound or graphics hardware".into(),
            (true, false) => "Sound output (a program that opens the card holds it for itself)".into(),
            (false, true) => "Graphics hardware (render nodes, no screen control)".into(),
            (true, true) => "Sound output and graphics hardware".into(),
        }
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit())
}

/// `pcmC0D0p` playback only, `controlC0`.
fn snd_name_ok(n: &str) -> bool {
    if let Some(c) = n.strip_prefix("controlC") {
        return all_digits(c);
    }
    let Some(r) = n.strip_prefix("pcmC") else { return false };
    let Some((card, rest)) = r.split_once('D') else { return false };
    let Some(dev) = rest.strip_suffix('p') else { return false };
    all_digits(card) && all_digits(dev)
}

fn render_name_ok(n: &str) -> bool {
    n.strip_prefix("renderD").is_some_and(all_digits)
}

/// May this host path be handed to a sandbox at all?
pub fn node_ok(path: &str) -> bool {
    match path.strip_prefix("/dev/snd/") {
        Some(n) => snd_name_ok(n),
        None => path.strip_prefix("/dev/dri/").is_some_and(render_name_ok),
    }
}

/// The nodes the policy gives, read from `dev_root` (the computer's `/dev`; tests give a folder of stand-ins). Only character devices
/// count when `need_char` (the real machine); the folder of a test holds plain files.
pub fn select(policy: &DevicePolicy, dev_root: &Path, need_char: bool) -> Vec<String> {
    use std::os::unix::fs::FileTypeExt;
    let mut out = Vec::new();
    for (on, sub, ok) in [(policy.sound, "snd", snd_name_ok as fn(&str) -> bool), (policy.gpu, "dri", render_name_ok as fn(&str) -> bool)] {
        if !on {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(dev_root.join(sub))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(ok))
            .filter(|e| !need_char || e.file_type().is_ok_and(|t| t.is_char_device()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        out.extend(names.into_iter().map(|n| format!("/dev/{sub}/{n}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-devsel-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (sub, names) in [("snd", vec!["controlC0", "pcmC0D0p", "pcmC0D0c", "pcmC0D1p", "timer", "seq", "pcmC1D0p", "controlC0x"]), ("dri", vec!["card0", "renderD128", "renderD129", "controlD64"])] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
            for n in names {
                std::fs::write(d.join(sub).join(n), b"").unwrap();
            }
        }
        d
    }

    #[test]
    fn everything_is_off_until_the_person_switches_it_on() {
        let d = dev("off");
        assert!(select(&DevicePolicy::default(), &d, false).is_empty());
        assert_eq!(DevicePolicy::default().describe(), "No sound or graphics hardware");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sound_gives_playback_and_control_nodes_and_never_capture_timer_or_sequencer() {
        let d = dev("snd");
        let got = select(&DevicePolicy { sound: true, gpu: false }, &d, false);
        assert_eq!(got, ["/dev/snd/controlC0", "/dev/snd/pcmC0D0p", "/dev/snd/pcmC0D1p", "/dev/snd/pcmC1D0p"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn graphics_gives_render_nodes_and_never_the_card_nodes() {
        let d = dev("gpu");
        assert_eq!(select(&DevicePolicy { sound: false, gpu: true }, &d, false), ["/dev/dri/renderD128", "/dev/dri/renderD129"]);
        assert_eq!(select(&DevicePolicy { sound: true, gpu: true }, &d, false).len(), 6);
        // plain files are not devices: the real machine's check
        assert!(select(&DevicePolicy { sound: true, gpu: true }, &d, true).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_node_is_ok_only_when_it_is_one_of_the_granted_kinds() {
        for ok in ["/dev/snd/pcmC0D0p", "/dev/snd/controlC12", "/dev/dri/renderD128"] {
            assert!(node_ok(ok), "{ok}");
        }
        for bad in ["/dev/snd/pcmC0D0c", "/dev/snd/timer", "/dev/dri/card0", "/dev/sda", "/dev/mem", "/dev/snd/../sda", "/dev/snd/pcmC0D0p/x", "/dev/snd/pcmCD0p", "/dev/snd/pcmC0Dp", "/dev/dri/renderD", "/dev/dri/renderD1234", "dev/snd/pcmC0D0p", ""] {
            assert!(!node_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_policy_round_trips_and_a_damaged_file_reads_as_off() {
        let d = dev("file");
        let f = d.join("devices.json");
        let p = DevicePolicy { sound: true, gpu: false };
        p.save(&f).unwrap();
        assert_eq!(DevicePolicy::load(&f), p);
        std::fs::write(&f, "{{{").unwrap();
        assert_eq!(DevicePolicy::load(&f), DevicePolicy::default());
        assert_eq!(DevicePolicy::load(&d.join("none.json")), DevicePolicy::default());
        let _ = std::fs::remove_dir_all(&d);
    }
}
