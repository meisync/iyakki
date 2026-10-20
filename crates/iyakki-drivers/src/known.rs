//! The drivers IYAKKI knows about for Macs (metadata and instructions, no blobs), and the per-machine status that
//! detection gives each one. The text here is what the Store shows under "Why".
//!
//! Honesty rule: every entry says which of four things is true. Facts that were not verified against a primary source
//! are said so in docs/MAC-HARDWARE.md ("Unverified"), not here.

use std::collections::BTreeSet;

use crate::detect::Machine;
use crate::pack::Match;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Works with the standard image (the driver is in the kernel, no extra file needed).
    Standard,
    /// Works after installing the pack (the driver is in the kernel; a firmware file or option is missing).
    AfterPack,
    /// The kernel lacks the driver: needs the Mac image variant (`./run.sh build-mac`).
    NeedsMacImage,
    /// Works only with a non-free driver the owner builds on a Linux PC and installs as a signed pack (never shipped by us).
    LocalBuild,
    /// Cannot be done on this system (proprietary or out-of-tree, or a different kernel).
    NotSupported,
}

impl Availability {
    pub fn label(self) -> &'static str {
        match self {
            Availability::Standard => "works with the standard image",
            Availability::AfterPack => "works after installing the pack",
            Availability::NeedsMacImage => "needs the Mac image variant",
            Availability::LocalBuild => "works with a locally built non-free driver pack",
            Availability::NotSupported => "not supported",
        }
    }
}

/// How the pack's files get onto the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The pack itself is built into the catalog (options only, nothing to supply).
    Builtin,
    /// The person supplies firmware files from a folder or a USB stick.
    Folder,
    /// The person supplies Apple's driver file and the device extracts the firmware (`facetime`).
    Extract,
    /// Nothing to install here (information only).
    Info,
}

#[derive(Clone, Debug)]
pub struct Known {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub availability: Availability,
    pub delivery: Delivery,
    /// `(dmi_product, dmi_vendor, pci, usb)`; any one of these applying recommends the pack.
    pub matches: &'static [(&'static str, &'static str, &'static str, &'static str)],
    /// Modules that, when loaded or bound to the matching device, mean it works.
    pub drivers: &'static [&'static str],
    /// A firmware file path prefix (under the firmware folder) whose presence means the firmware is there.
    pub firmware_probe: &'static [&'static str],
    /// File name patterns accepted from a folder, and the firmware subfolder they go to.
    pub accept: &'static [(&'static str, &'static str)],
    pub license: &'static str,
    pub why: &'static str,
    /// What the person must supply, and from where.
    pub supply: &'static str,
    /// Numbered steps in plain words (shown under "HOW", one line per `\n`); empty when there is nothing to do.
    pub guide: &'static str,
}

impl Known {
    pub fn match_rules(&self) -> Vec<Match> {
        self.matches.iter().map(|(p, v, pci, usb)| Match { dmi_product: (*p).into(), dmi_vendor: (*v).into(), pci: (*pci).into(), usb: (*usb).into() }).collect()
    }
}


pub const KNOWN: &[Known] = &[
    Known {
        id: "mac-gpu-intel",
        title: "Intel graphics (Mac)",
        summary: "i915 drives the built-in Intel graphics; Haswell (HD 5000) needs no firmware file.",
        availability: Availability::Standard,
        delivery: Delivery::Info,
        matches: &[("", "Apple*", "8086:*", "")],
        drivers: &["i915", "xe"],
        firmware_probe: &[],
        accept: &[],
        license: "GPL-2.0 (driver in the kernel)",
        why: "The Intel graphics driver is part of the standard kernel image. Haswell-generation graphics (Mac mini 2014, HD 5000) load without a firmware blob; later generations use the DMC/GuC files that the hardware-firmware image variant carries. Nothing to install.",
        supply: "Nothing.",
        guide: "",
    },
    Known {
        id: "mac-wifi-brcmfmac",
        title: "Broadcom Wi-Fi firmware (Mac, brcmfmac)",
        summary: "Macs whose Wi-Fi chip brcmfmac supports (BCM43602; T2 Macs: BCM4364, BCM4377): the driver is in the kernel, the firmware is not.",
        availability: Availability::AfterPack,
        delivery: Delivery::Folder,
        matches: &[("", "Apple*", "14e4:43ba", ""), ("", "Apple*", "14e4:43bb", ""), ("", "Apple*", "14e4:43bc", ""), ("", "Apple*", "14e4:aa52", ""), ("", "Apple*", "14e4:4464", ""), ("", "Apple*", "14e4:4488", ""), ("", "Apple*", "14e4:43dc", "")],
        drivers: &["brcmfmac"],
        firmware_probe: &["brcm/brcmfmac"],
        accept: &[("brcmfmac*.bin", "brcm"), ("brcmfmac*.txt", "brcm"), ("brcmfmac*.clm_blob", "brcm"), ("brcmfmac*.txcap_blob", "brcm")],
        license: "Broadcom firmware: redistributable only as part of linux-firmware under its licence; Apple board files (.txt) come from your own Mac",
        why: "brcmfmac is the open Broadcom driver in the Linux kernel. It needs a firmware file for your exact chip (brcmfmac43602-pcie.bin for BCM43602) and, on Apple boards, a board file. The `hw` image variant carries the linux-firmware files; the standard `libre` image carries none. IYAKKI does not ship them in the pack: copy them from the linux-firmware repository (git.kernel.org) or from another Linux installation to a USB stick and install from the stick. T2 Macs (BCM4364 in the Mac mini 2018 and most 2018-2019 MacBook Pros, BCM4377 in the MacBookPro15,4, MacBookPro16,3 and MacBookAir9,1) use Apple's own firmware, which only macOS has: run dist/drivers/mac-wifi-firmware-from-macos.sh in macOS on the same Mac; it writes the files under the names brcmfmac asks for onto a USB stick.",
        supply: "Firmware files named brcmfmac<chip>-pcie.bin (and .txt / .clm_blob / .txcap_blob if present) from linux-firmware, or for a T2 Mac the folder iyakki-wifi-firmware that dist/drivers/mac-wifi-firmware-from-macos.sh wrote in macOS, in a folder or on a USB stick.",
        guide: "",
    },
    Known {
        id: "mac-wifi-bcm4360",
        title: "Broadcom BCM4360 Wi-Fi (Mac mini 2014 and others)",
        summary: "Works with a non-free driver you build yourself (never shipped), or with a USB Wi-Fi adapter; Ethernet always works.",
        availability: Availability::LocalBuild,
        delivery: Delivery::Folder,
        matches: &[("", "Apple*", "14e4:43a0", ""), ("", "Apple*", "14e4:4360", ""), ("", "Apple*", "14e4:43a1", ""), ("", "Apple*", "14e4:43a2", ""), ("", "Apple*", "14e4:43b1", "")],
        drivers: &["wl", "brcmfmac"],
        firmware_probe: &[],
        accept: &[],
        license: "NON-FREE. Broadcom's 802.11 STA driver (`wl`) is under Broadcom's own licence and has a binary-only part. IYAKKI does not ship it and never will: you download it from Broadcom and build it yourself, for your own machine. Loading it marks the kernel as tainted.",
        why: "The Mac mini 2014 has a Broadcom BCM4360 Wi-Fi card (PCI 14e4:43a0). No open driver in the kernel supports it: b43 and brcmfmac do not. The one Linux driver that does is Broadcom's own `wl` (the broadcom-sta driver). Its licence does not let IYAKKI include it, so it is not on the image and not in the Store. You can build it once, on a Linux PC, for exactly the kernel of this image, and install it here as a signed driver pack. It is non-free software that runs inside the kernel with full power: the review says so, and the kernel is marked tainted while it is loaded. It does WPA2 (not WPA3-only networks). The pack is checked again at every boot and stops loading, with a message, if the kernel is ever replaced. Do not want that? Use a USB Wi-Fi adapter or the Ethernet port.",
        supply: "Option A: a Linux PC with this source tree (dist/drivers/build-wl-pack.sh), the Mac image's kernel build folder and internet for the one download from Broadcom; you get a signed .iyp file for a USB stick. Option B: a USB Wi-Fi adapter. Option C: an Ethernet cable.",
        guide: "OPTION A: build the Broadcom driver yourself (about 15 minutes, on a Linux PC, not on the Mac)\n1. In the IYAKKI source folder run: sh dist/drivers/build-wl-pack.sh --accept-broadcom-licence\n   It shows Broadcom's licence, downloads the driver from Broadcom, builds it for this exact kernel (6.12.112), checks it and signs the pack with your own key. Everything lands in dist/drivers/out/.\n2. Copy the folder dist/drivers/out/ to a USB stick.\n3. First time only: STORE, Settings, Trust a publisher key. Paste the public key line the script printed and give it a name.\n4. Plug the stick into this Mac, press From USB... here and give the folder (for example /mnt/usb/out).\n5. Read the review (it names the kernel module, says non-free and tainted) and accept. The Wi-Fi network list then shows up in Settings, Network.\n6. After an image update with a new kernel, run the script again: an old pack is refused, not loaded.\nOPTION B: a USB Wi-Fi adapter with an open driver (nothing to build, nothing non-free)\nIn this image: MediaTek MT7612U and MT7921AU, MediaTek MT7601U, Atheros AR9271 (ath9k_htc). Their firmware is in the Mac image. Not usable: Realtek RTL8812AU/RTL8821AU (the driver is not in the kernel); MediaTek MT7610U needs a firmware file the image does not carry.\nOPTION C: the Ethernet port, or a Wi-Fi router or extender in client mode next to the Mac. Always works.",
    },
    Known {
        id: "mac-bt-broadcom",
        title: "Broadcom Bluetooth firmware (Mac)",
        summary: "btusb is in the kernel; the Broadcom patch-RAM file (.hcd) for your Bluetooth chip must be supplied.",
        availability: Availability::AfterPack,
        delivery: Delivery::Folder,
        matches: &[("", "Apple*", "", "0a5c:*"), ("", "Apple*", "", "05ac:82*"), ("", "Apple*", "14e4:5fa0", "")],
        drivers: &["btusb", "btbcm", "hci_bcm4377", "hci_uart"],
        firmware_probe: &["brcm/BCM20702", "brcm/BCM2045", "brcm/BCM43", "brcm/BCM4", "brcm/brcmbt"],
        accept: &[("*.hcd", "brcm"), ("brcmbt*.bin", "brcm"), ("brcmbt*.ptb", "brcm")],
        license: "Broadcom firmware, no redistribution grant: supply your own",
        why: "The BCM20702 Bluetooth chip in the Mac mini 2014 starts from a small built-in firmware and the kernel can load a newer patch-RAM file (.hcd) over it. The kernel asks for brcm/BCM20702A1-<usb vendor>-<usb product>.hcd; the file is Broadcom's and is not in linux-firmware. Without it the adapter may still work in a limited way. IYAKKI cannot ship it: take it from a Boot Camp / Windows driver package you own (converted from .hex to .hcd) or a community collection you trust, put it on a USB stick and install it here. The BCM4377 of the MacBookPro15,4, MacBookPro16,3 and MacBookAir9,1 is a PCIe device (driver hci_bcm4377, Mac image) that needs brcm/brcmbt4377b3-apple,formosa-<vendor>.bin and .ptb from macOS Monterey or later: dist/drivers/mac-wifi-firmware-from-macos.sh copies them together with the Wi-Fi files. The BCM4364 of the Mac mini 2018 and the 2018/2019 MacBook Pros is not USB or PCIe: its Bluetooth is on a UART (driver hci_uart with hci_bcm, Mac image) and runs from its built-in ROM without any file; if macOS has a BCM4364 patch file, the same script copies it as BCM.hcd (the name the kernel asks for).",
        supply: "A BCM20702A1-<vid>-<pid>.hcd file (the exact name is in the kernel log: `dmesg | grep -i firmware`), or for a BCM4377 Mac the brcmbt*.bin and .ptb files the macOS script wrote.",
        guide: "",
    },
    Known {
        id: "mac-facetimehd",
        title: "FaceTime HD camera (facetimehd)",
        summary: "Out-of-tree driver plus firmware extracted from Apple's driver: needs the Mac image variant and your own Apple file.",
        availability: Availability::NeedsMacImage,
        delivery: Delivery::Extract,
        matches: &[("", "Apple*", "14e4:1570", "")],
        drivers: &["facetimehd"],
        firmware_probe: &["facetimehd/firmware.bin"],
        accept: &[],
        license: "Driver bcwc_pcie: GPL-2.0 (out of tree). Firmware: Apple's, extracted on your machine, never redistributed by IYAKKI",
        why: "The camera of most MacBooks since about 2013 is a PCIe device (Broadcom 720p FaceTime HD) that the kernel does not support. The community driver facetimehd (bcwc_pcie) is out of tree: the Mac image variant would have to build it with the kernel. It also needs a firmware file that is part of Apple's macOS driver. Apple's licence does not allow us to ship it, so you supply Apple's AppleCameraInterface file and IYAKKI extracts the firmware with checksums, in this tile, on the device. The Mac mini 2014 has no camera.",
        supply: "The file AppleCameraInterface from a macOS install (from /System/Library/Extensions/AppleCameraInterface.kext/Contents/MacOS/) on a USB stick.",
        guide: "",
    },
    Known {
        id: "mac-t2",
        title: "Apple T2 Macs (keyboard, trackpad, audio, SSD, Touch Bar)",
        summary: "The Mac image carries apple-bce (sound tried on a Mac mini 2018) and the MacBook keyboard, trackpad and Touch Bar keys (not tried).",
        availability: Availability::NeedsMacImage,
        delivery: Delivery::Info,
        matches: &[("MacBookPro15,*", "", "", ""), ("MacBookPro16,*", "", "", ""), ("MacBookAir8,*", "", "", ""), ("MacBookAir9,*", "", "", ""), ("iMac20,*", "", "", ""), ("iMacPro1,1", "", "", ""), ("Macmini8,1", "", "", ""), ("MacPro7,1", "", "", "")],
        drivers: &["apple_bce"],
        firmware_probe: &[],
        accept: &[],
        license: "apple-bce (t2linux): GPL-2.0, out of tree, built with the Mac image's kernel; Wi-Fi and Bluetooth firmware comes from macOS",
        why: "Macs with the T2 security chip hide the keyboard, trackpad, audio and more behind the T2. The t2linux project's apple-bce module reaches them: it talks to the T2 over PCIe, offers its internal USB devices and its sound card (Apple T2 Audio: Speaker and the headphone jack). The Mac image variant builds apple-bce for its exact kernel; a driver pack cannot add it to the standard image (a module for one exact kernel, plus kernel options). Tried on a Mac mini 2018: a tone played on the built-in speaker. For T2 MacBooks the Mac image also builds the internal keyboard (hid-apple), the trackpad (bcm5974 with t2linux's T2 patch) and the Touch Bar in its keyboard mode (hid-appletb-kbd and hid-appletb-bl, backported from Linux 6.15+): Esc and F1-F12 or the media keys, Fn switches. These were built but not yet tried on a MacBook. Custom Touch Bar drawing (tiny-dfr) is not included.",
        supply: "Nothing: use the Mac image. Wi-Fi and Bluetooth firmware come from macOS (see the Wi-Fi and Bluetooth entries).",
        guide: "",
    },
    Known {
        id: "mac-fan-sensors",
        title: "Fan and temperature sensors (applesmc)",
        summary: "applesmc and coretemp are not in the standard kernel; the Mac image variant adds them. No fan daemon ships yet.",
        availability: Availability::NeedsMacImage,
        delivery: Delivery::Info,
        matches: &[("", "Apple*", "", "")],
        drivers: &["applesmc"],
        firmware_probe: &[],
        accept: &[],
        license: "GPL-2.0 (drivers in the kernel)",
        why: "Apple's fans and many temperature sensors are reached through the System Management Controller, driver applesmc (CONFIG_SENSORS_APPLESMC), plus coretemp for the CPU. Without applesmc the firmware's own fan curve still runs, which is usually enough; with it you can read temperatures and, with a small userspace controller (like mbpfan), set fan speed. The standard image has neither driver; build the Mac variant.",
        supply: "Nothing (a kernel option).",
        guide: "",
    },
    Known {
        id: "mac-audio-cirrus",
        title: "Built-in audio (Cirrus Logic CS4206 and similar)",
        summary: "snd-hda-intel is in the kernel; the Cirrus codec driver is not in the standard image: the Mac image variant adds it.",
        availability: Availability::NeedsMacImage,
        delivery: Delivery::Info,
        matches: &[("", "Apple*", "", "")],
        drivers: &["snd_hda_codec_cirrus", "snd_hda_codec_cs420x"],
        firmware_probe: &[],
        accept: &[],
        license: "GPL-2.0 (drivers in the kernel)",
        why: "The Mac mini 2014 uses a Cirrus Logic CS4206 HD-audio codec. The kernel driver is snd_hda_codec_cirrus (CONFIG_SND_HDA_CODEC_CIRRUS; newer kernels split it into CS420X and CS421X options). The standard image carries Realtek, Conexant, Sigmatel and Analog codecs but not Cirrus. HDMI audio works without it.",
        supply: "Nothing (a kernel option).",
        guide: "",
    },
    Known {
        id: "mac-keyboard-options",
        title: "Apple keyboard options (function keys)",
        summary: "A ready pack: makes the F-keys act as F-keys by default (hid_apple fnmode=2). Needs the hid_apple driver of the Mac variant.",
        availability: Availability::NeedsMacImage,
        delivery: Delivery::Builtin,
        matches: &[("", "Apple*", "", "")],
        drivers: &["hid_apple"],
        firmware_probe: &[],
        accept: &[],
        license: "Apache-2.0 (the pack); the driver is GPL-2.0 in the kernel",
        why: "Apple keyboards act as media keys by default. hid_apple's fnmode=2 makes F1-F12 plain function keys and Fn the media key. USB Apple keyboards work with the generic HID driver in the standard image; this pack only has an effect when hid_apple is present (Mac image variant).",
        supply: "Nothing.",
        guide: "",
    },
];

pub fn by_id(id: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The driver is loaded and nothing is missing.
    Working,
    /// The driver is loaded, the firmware file is not found.
    FirmwareMissing,
    /// A pack is installed but the driver has not picked it up yet (reload or restart).
    NeedsReload,
    /// No driver for it on this system.
    NoDriver,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Working => "driver loaded",
            Status::FirmwareMissing => "firmware missing",
            Status::NeedsReload => "installed, reload or restart",
            Status::NoDriver => "no driver",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub known: &'static Known,
    pub status: Status,
    pub devices: Vec<String>,
    pub installed: bool,
    pub detail: String,
}

/// The "Drivers for this machine" list: every known entry whose rules match, with its status. A generic PC gets an empty list.
/// `fw_present(prefix)` says whether a firmware file with that path prefix exists; `installed` holds the ids of installed packs.
pub fn for_machine(m: &Machine, fw_present: &dyn Fn(&str) -> bool, installed: &BTreeSet<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    for k in KNOWN {
        let rules = k.match_rules();
        let Some(devices) = m.first_match(&rules) else { continue };
        let bound = m.bound_drivers(&rules);
        let working = k.drivers.iter().any(|d| m.modules.contains(*d)) || bound.iter().any(|d| k.drivers.contains(&d.as_str()));
        let fw_ok = k.firmware_probe.is_empty() || k.firmware_probe.iter().any(|p| fw_present(p));
        let is_installed = installed.contains(k.id);
        let (status, detail) = if k.availability == Availability::NotSupported && !working {
            (Status::NoDriver, "not supported on this system".to_string())
        } else if k.availability == Availability::LocalBuild && !working && !is_installed {
            (Status::NoDriver, "no driver yet: build the pack on a Linux PC and install it from a USB stick (Why shows how), or use a USB Wi-Fi adapter or Ethernet".to_string())
        } else if working && fw_ok {
            (Status::Working, format!("driver {} is loaded", k.drivers.iter().find(|d| m.modules.contains(**d) || bound.iter().any(|b| b == **d)).copied().unwrap_or("?")))
        } else if working {
            (Status::FirmwareMissing, "the driver is loaded but its firmware file was not found: install the pack".to_string())
        } else if is_installed {
            (Status::NeedsReload, "the pack is installed: reload the driver or restart".to_string())
        } else if k.availability == Availability::NeedsMacImage {
            (Status::NoDriver, "this kernel has no driver for it: it needs the Mac image variant".to_string())
        } else if !fw_ok {
            (Status::FirmwareMissing, "firmware file not found: install the pack".to_string())
        } else {
            (Status::NoDriver, "no driver is bound to it".to_string())
        };
        rows.push(Row { known: k, status, devices, installed: is_installed, detail });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fake::*;

    fn rows(tag: &str, build: impl Fn(&std::path::Path), fw: &[&str], installed: &[&str]) -> Vec<Row> {
        let (r, root) = tree(tag);
        build(&root);
        let m = Machine::collect(&r);
        let fw: Vec<String> = fw.iter().map(|s| s.to_string()).collect();
        let inst: BTreeSet<String> = installed.iter().map(|s| s.to_string()).collect();
        for_machine(&m, &|p| fw.iter().any(|f| f.starts_with(p)), &inst)
    }

    fn ids(r: &[Row]) -> Vec<&'static str> {
        r.iter().map(|x| x.known.id).collect()
    }

    fn mini(root: &std::path::Path) {
        dmi(root, "Apple Inc.", "Macmini7,1");
        pci(root, "0000:00:02.0", "8086", "0a26", Some("i915"));
        pci(root, "0000:03:00.0", "14e4", "43a0", None);
        usb(root, "1-4", "05ac", "8289", "Bluetooth USB Host Controller", Some("btusb"));
        put(root, "proc/modules", "i915 1 0 - Live 0x0\nbtusb 1 0 - Live 0x0\n");
    }

    #[test]
    fn mac_mini_2014_gets_gpu_wifi_bluetooth_fan_audio_and_keyboard_but_no_camera_or_t2() {
        let r = rows("mini", mini, &[], &[]);
        let got = ids(&r);
        for want in ["mac-gpu-intel", "mac-wifi-bcm4360", "mac-bt-broadcom", "mac-fan-sensors", "mac-audio-cirrus", "mac-keyboard-options"] {
            assert!(got.contains(&want), "{want} in {got:?}");
        }
        for not in ["mac-facetimehd", "mac-t2", "mac-wifi-brcmfmac"] {
            assert!(!got.contains(&not), "{not} must not be recommended for a Mac mini 2014");
        }
        let st = |id: &str| r.iter().find(|x| x.known.id == id).unwrap().status;
        assert_eq!(st("mac-gpu-intel"), Status::Working);
        assert_eq!(st("mac-wifi-bcm4360"), Status::NoDriver);
        let wifi = r.iter().find(|x| x.known.id == "mac-wifi-bcm4360").unwrap();
        assert_eq!((wifi.known.availability, wifi.known.delivery), (Availability::LocalBuild, Delivery::Folder));
        assert!(wifi.detail.contains("build the pack") && wifi.detail.contains("USB Wi-Fi adapter"), "{}", wifi.detail);
        assert_eq!(st("mac-bt-broadcom"), Status::FirmwareMissing, "btusb is bound but no .hcd is present");
        assert_eq!(st("mac-fan-sensors"), Status::NoDriver);
        assert_eq!(st("mac-audio-cirrus"), Status::NoDriver);
        let bt = r.iter().find(|x| x.known.id == "mac-bt-broadcom").unwrap();
        assert!(bt.devices[0].contains("05ac:8289") && bt.devices[0].contains("btusb"));
    }

    #[test]
    fn firmware_present_or_pack_installed_changes_the_status() {
        let r = rows("mini-fw", mini, &["brcm/BCM20702A1-05ac-8289.hcd"], &[]);
        assert_eq!(r.iter().find(|x| x.known.id == "mac-bt-broadcom").unwrap().status, Status::Working);
        let r = rows("mini-inst", |root| {
            dmi(root, "Apple Inc.", "Macmini7,1");
            usb(root, "1-4", "0a5c", "21e6", "BCM20702A0", None);
        }, &[], &["mac-bt-broadcom"]);
        let bt = r.iter().find(|x| x.known.id == "mac-bt-broadcom").unwrap();
        assert_eq!((bt.status, bt.installed), (Status::NeedsReload, true));
    }

    #[test]
    fn macbook_with_a_facetime_camera_gets_the_camera_entry() {
        let r = rows("mbp", |root| {
            dmi(root, "Apple Inc.", "MacBookPro11,1");
            pci(root, "0000:02:00.0", "14e4", "1570", None);
        }, &[], &[]);
        let cam = r.iter().find(|x| x.known.id == "mac-facetimehd").expect("camera recommended");
        assert_eq!((cam.known.availability, cam.known.delivery, cam.status), (Availability::NeedsMacImage, Delivery::Extract, Status::NoDriver));
        assert!(!ids(&r).contains(&"mac-t2"), "MacBookPro11,1 has no T2 chip");
    }

    #[test]
    fn t2_macs_are_recognised_by_product_and_need_the_mac_image() {
        for p in ["MacBookPro15,2", "MacBookPro16,1", "MacBookAir8,1", "iMac20,1", "Macmini8,1", "MacPro7,1", "iMacPro1,1"] {
            let r = rows("t2", |root| dmi(root, "Apple Inc.", p), &[], &[]);
            let t2 = r.iter().find(|x| x.known.id == "mac-t2").unwrap_or_else(|| panic!("{p}"));
            assert_eq!((t2.known.availability, t2.known.delivery, t2.status), (Availability::NeedsMacImage, Delivery::Info, Status::NoDriver));
            assert!(t2.detail.contains("Mac image"), "{}", t2.detail);
        }
        // the Mac image on a Mac mini 2018: apple-bce is loaded
        let r = rows("t2-loaded", |root| {
            dmi(root, "Apple Inc.", "Macmini8,1");
            put(root, "proc/modules", "apple_bce 102400 0 - Live 0x0 (O)\nsnd_pcm 172032 5 apple_bce, Live 0x0\n");
        }, &[], &[]);
        let t2 = r.iter().find(|x| x.known.id == "mac-t2").unwrap();
        assert_eq!(t2.status, Status::Working, "{}", t2.detail);
        assert!(t2.detail.contains("apple_bce"));
    }

    #[test]
    fn a_2019_macbook_pro_gets_t2_wifi_and_pcie_bluetooth_and_takes_the_macos_script_files() {
        // MacBookPro15,4 (13", two ports): BCM4377 Wi-Fi 14e4:4488 and its PCIe Bluetooth 14e4:5fa0
        let r = rows("mbp154", |root| {
            dmi(root, "Apple Inc.", "MacBookPro15,4");
            pci(root, "0000:02:00.0", "14e4", "4488", Some("brcmfmac"));
            pci(root, "0000:02:00.1", "14e4", "5fa0", Some("hci_bcm4377"));
            put(root, "proc/modules", "brcmfmac 1 0 - Live 0x0\nhci_bcm4377 1 0 - Live 0x0\napple_bce 1 0 - Live 0x0 (O)\n");
        }, &[], &[]);
        let st = |id: &str| r.iter().find(|x| x.known.id == id).unwrap_or_else(|| panic!("{id} in {:?}", ids(&r))).status;
        assert_eq!(st("mac-wifi-brcmfmac"), Status::FirmwareMissing);
        assert_eq!(st("mac-bt-broadcom"), Status::FirmwareMissing);
        assert_eq!(st("mac-t2"), Status::Working);
        let r = rows("mbp154-fw", |root| {
            dmi(root, "Apple Inc.", "MacBookPro15,4");
            pci(root, "0000:02:00.1", "14e4", "5fa0", Some("hci_bcm4377"));
        }, &["brcm/brcmbt4377b3-apple,formosa-u.bin"], &[]);
        assert_eq!(r.iter().find(|x| x.known.id == "mac-bt-broadcom").unwrap().status, Status::Working);
        // the names dist/drivers/mac-wifi-firmware-from-macos.sh writes are accepted from the stick
        let takes = |id: &str, name: &str| by_id(id).unwrap().accept.iter().any(|(p, _)| crate::pack::glob(p, name));
        for n in ["brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin", "brcmfmac4377b3-pcie.apple,formosa-SPPR-m-3.1-X3.txt",
                  "brcmfmac4364b3-pcie.apple,borneo-HRPN-m-7.9-X0.clm_blob", "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.txcap_blob"] {
            assert!(takes("mac-wifi-brcmfmac", n), "{n}");
        }
        assert!(takes("mac-bt-broadcom", "brcmbt4377b3-apple,formosa-u.bin") && takes("mac-bt-broadcom", "brcmbt4377b3-apple,formosa-u.ptb"));
        assert!(!takes("mac-wifi-brcmfmac", "evil.sh") && !takes("mac-bt-broadcom", "brcmbt.sh"));
        let t2 = by_id("mac-t2").unwrap();
        assert!(t2.why.contains("Touch Bar") && t2.why.contains("not yet tried"), "{}", t2.why);
    }

    #[test]
    fn a_generic_pc_gets_no_recommendations_at_all() {
        let r = rows("pc", |root| {
            dmi(root, "Dell Inc.", "OptiPlex 7090");
            pci(root, "0000:00:02.0", "8086", "4c8a", Some("i915"));
            pci(root, "0000:00:14.3", "8086", "43f0", Some("iwlwifi"));
            usb(root, "1-1", "0a5c", "21e6", "Broadcom BT dongle", Some("btusb"));
        }, &[], &[]);
        assert!(r.is_empty(), "{:?}", ids(&r));
    }

    #[test]
    fn the_catalogue_is_well_formed() {
        let mut seen = BTreeSet::new();
        for k in KNOWN {
            assert!(seen.insert(k.id) && k.id.starts_with("mac-") && crate::pack::valid_module_name(k.id), "{}", k.id);
            assert!(!k.title.is_empty() && !k.summary.is_empty() && k.summary.len() <= 140 && k.title.len() <= 60, "{}", k.id);
            assert!(!k.why.is_empty() && !k.license.is_empty() && !k.supply.is_empty() && !k.match_rules().is_empty());
            assert!(k.match_rules().iter().all(|m| !m.is_empty()));
            if k.delivery == Delivery::Folder {
                assert!(!k.accept.is_empty() || !k.guide.is_empty(), "{}: a folder pack names what it accepts, or explains how to make the signed pack it takes", k.id);
            }
            for (_, dir) in k.accept {
                assert!(crate::pack::safe_rel_path(dir));
            }
        }
        assert_eq!(by_id("mac-t2").unwrap().availability, Availability::NeedsMacImage);
        let w = by_id("mac-wifi-bcm4360").unwrap();
        for need in ["non-free", "tainted", "build-wl-pack.sh", "--accept-broadcom-licence", "USB Wi-Fi adapter", "Ethernet", "MT7612U", "AR9271", "Trust a publisher key"] {
            assert!(format!("{} {} {} {}", w.why, w.guide, w.license, w.supply).to_lowercase().contains(&need.to_lowercase()), "the Wi-Fi card says {need:?}");
        }
        assert!(w.license.contains("does not ship it") && w.license.contains("build it yourself"));
        assert_eq!(by_id("mac-gpu-intel").unwrap().availability, Availability::Standard);
        // NotSupported has no entry since the Mac image carries apple-bce (mac-t2); the status stays for hardware with no driver
        for a in [Availability::Standard, Availability::AfterPack, Availability::NeedsMacImage, Availability::LocalBuild] {
            assert!(KNOWN.iter().any(|k| k.availability == a), "{a:?} is used");
        }
    }

    /// The macOS helper script, run on a fake `/usr/share/firmware`: Wi-Fi names, the UART Bluetooth patch file
    /// (`BCM.hcd`, only when exactly one) and the PCIe Bluetooth names.
    #[test]
    fn the_macos_script_renames_wifi_and_bluetooth_files_in_a_fake_firmware_tree() {
        use std::fs;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/drivers/mac-wifi-firmware-from-macos.sh");
        let run = |tag: &str, hcds: &[&str]| -> (std::path::PathBuf, String) {
            let root = std::env::temp_dir().join(format!("iyakki-fwscript-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let chip = root.join("src/wifi/C-4364__s-B2");
            fs::create_dir_all(&chip).unwrap();
            fs::write(chip.join("P-lanai-X0_M-HRPN_V-u__m-7.5.txt"), "boardtype = 0x1\n").unwrap();
            fs::write(chip.join("lanai.trx"), "fw").unwrap();
            let bt = root.join("src/bluetooth");
            fs::create_dir_all(&bt).unwrap();
            for h in hcds {
                fs::write(bt.join(h), "hcd").unwrap();
            }
            fs::write(bt.join("BCM4377B3_21.2.1_PCIE_macOS_formosaE_MUR_x.bin"), "bt").unwrap();
            let out = std::process::Command::new("bash").arg(&script).arg(root.join("out")).env("IYAKKI_FW_SRC", root.join("src")).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            (root.clone(), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
        };
        let (root, log) = run("one", &["BCM4364B3_001.hcd"]);
        let dest = root.join("out/iyakki-wifi-firmware");
        assert!(dest.join("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin").exists());
        assert_eq!(fs::read_to_string(dest.join("BCM.hcd")).unwrap(), "hcd", "one BCM4364 patch file becomes brcm/BCM.hcd");
        assert!(dest.join("brcmbt4377b3-apple,formosa-m.bin").exists(), "PCIe Bluetooth keeps its names: {log}");
        let _ = fs::remove_dir_all(&root);
        let (root, log) = run("two", &["BCM4364B3_001.hcd", "BCM4364B3_002.hcd"]);
        assert!(!root.join("out/iyakki-wifi-firmware/BCM.hcd").exists(), "two candidates: nothing is guessed");
        assert!(log.contains("not guessing"));
        let _ = fs::remove_dir_all(&root);
        let (root, log) = run("none", &[]);
        assert!(log.contains("starts from its ROM"));
        let _ = fs::remove_dir_all(&root);
    }
}
