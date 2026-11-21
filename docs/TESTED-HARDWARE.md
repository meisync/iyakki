# Tested hardware

**For:** anyone deciding whether to try IYAKKI on a machine they own.
**Status:** alpha (0.1.0-alpha.6). This page is a summary written from the project's own notes ([`HARDWARE.md`](HARDWARE.md), [`MAC-HARDWARE.md`](MAC-HARDWARE.md), [`../KNOWN-ISSUES.md`](../KNOWN-ISSUES.md), [`../FEATURES.md`](../FEATURES.md)); where they disagree, they win. Most real-hardware testing was done by the project's owner on a handful of machines, most of it on the Mac mini 2018. **A machine that is not listed here is untested**, not unsupported: run the *Hardware check* boot entry first ([`BOOT-TEST.md`](BOOT-TEST.md)), then tell us what you saw (the *Hardware report* issue form).

## Machines that ran IYAKKI

"Booted" means the stick or disk started IYAKKI to the sign-in screen and a person used it. Everything not named in the last column was **not tried**.

| Machine | How it was run | What worked | What did not, or is open | Not tried |
|---|---|---|---|---|
| Mac mini 2018 (Macmini8,1, Apple T2, Intel UHD 630) | USB SSD, later the internal disk | Boots through GRUB (UEFI); data volume and `/home` persist; HDMI audio, built-in speaker and headphone jack; USB webcam; local k3s dev cluster; the full web engine (Google, YouTube with VP9 video); developer mode, update and restart | Bluetooth: the chip has no firmware file in macOS, so it does not work (KI-006); Wi-Fi needs Apple firmware the person installs and was never associated here; a USB-C touch monitor was dead after a cold boot (the shell-side fix was verified on this machine, multi-finger gestures are still open, KI-077); a Debian "Linux apps" sandbox install failed once (KI-075); the first USB-C plug of a 4K monitor offered only 1080p, a different cable gave 4K (KI-160) | Apple's own keyboard, suspend, Secure Boot |
| Mac mini 2014 (Macmini7,1, no T2, Intel HD 5000) | lean install image over a cable | Boots (UEFI); the owner reports that everything works except Wi-Fi, and that sound and Bluetooth work | Wi-Fi: the Broadcom BCM4360 card has no working driver (KI-094); use the cable or a USB Wi-Fi adapter | Nearly everything else was not recorded in detail |
| MacBook Pro 14,1 (13-inch 2017, no T2) | USB SSD, lean image | Boots; Bluetooth pairing, phone sound (A2DP) and tethering with internet worked (owner); the quiet speakers (Cirrus CS8409 codec) were measured and handled in code, microphone path fixed | The Bluetooth patch pack is not needed here and the installer leaves it out | Camera, battery, suspend |
| MacBook Pro 16,2 (2020, Apple T2) | USB SSD, then the internal disk | Boots; keyboard, trackpad, speakers, GPU browser helper, YouTube with sound, internal panel and an external DisplayPort monitor, Wi-Fi with Apple firmware, built-in and USB cameras, developer mode over Wi-Fi | The Touch Bar strip drawn by IYAKKI never opens; the built-in microphone is switched off on purpose | Battery, suspend, Secure Boot |
| HP Pavilion Laptop 16-af0xxx (Intel Meteor Lake, NVIDIA dGPU, Realtek RTL8852BE Wi-Fi) | USB SSD, Secure Boot on with the project's key enrolled | Boots after the kernel got x2APIC support (the first stick stopped before stage 1); the signed boot chain was accepted by the firmware with the key enrolled; the camera works | A closed privacy shutter gave a black picture without a message (KI-082, fixed in code) | Wi-Fi (Realtek firmware pack), sound, touch, battery, suspend, internal-disk install, NVIDIA beyond the open driver |
| Lenovo Legion Pro 5 16ARX8 (82WM; AMD Ryzen with Radeon, NVIDIA dGPU, MediaTek Wi-Fi and Bluetooth) | USB SSD and a flash drive written with `iyakki-usb`, Secure Boot on with the key enrolled | Boots; the USB writer and the lock keys passed a manual check; alpha.6 candidates were updated and restarted through developer mode | The camera light staying on and a slow driver download with no network were found and fixed in code (KI-080, KI-083, KI-093); the camera-light fix is not confirmed on it; Wi-Fi and Bluetooth firmware is installed by the person with a button; the mixer starts low | Everything not named |
| An AMD PC (model not recorded) | not recorded | Booted (owner report only) | n/a | Everything: treat as untested |
| QEMU/KVM (x86-64, UEFI and BIOS) | the project's automated tests | Boot, persistence, installer, sign-in, the session tests of alpha.6 | n/a | Real hardware drivers, Wi-Fi, Bluetooth, sound, GPU |

Other machine classes: other Intel/AMD PCs are expected to boot (the loader and kernel are generic) but only the ones above did; legacy BIOS boot was tested in QEMU only; **Apple Silicon Macs are not supported on bare metal** (a VM recipe ran once on an Apple Silicon Mac mini, 2026-11-19, and reached a Ready k3s node); Raspberry Pi and other ARM boards are not supported; VirtualBox, VMware and Hyper-V are untested. Details and the class table: [`HARDWARE.md`](HARDWARE.md).

## Things that do not work or are limited on any machine

| What | Why | Where it is written down |
|---|---|---|
| Netflix and Prime Video do not play | they need a Widevine module that is not shipped | F-BROW-101, release notes |
| H.264 and AAC video, so TikTok's video does not play | not shipped on purpose (patent policy); an optional audio and video add-on is the way to get it, built on the machine | KI-017 |
| Wi-Fi and Bluetooth on Macs | need Apple's firmware, installed by the person on their own machine; the 2013 to 2015 Macs with a Broadcom BCM4360 have no Wi-Fi driver | KI-006, KI-094 |
| Direct USB-C to USB-C link between two computers | built, but the cable-only link between two computers is not working yet (still pending); a USB Ethernet adapter was verified | F-DEV-10, F-DEV-11 in FEATURES.md |
| Touch on a Mac mini 2018 after a cold boot | the controller starts in mouse mode; fixed in the shell and verified on the Mac mini; gestures with several fingers are still open | KI-077 |
| Linux apps (a Debian sandbox) on an installed image | the install failed in the one image run on a Mac mini | KI-075 |
| Wake phrase for the voice assistant | experimental, off by default | FEATURES.md |
| The built-in browser engine | no video decoding, no IndexedDB, Chinese/Japanese/Korean and emoji shown as boxes; the full web engine plays YouTube | README "Known gaps" |
| Secure Boot | verified in QEMU, and on the HP and Lenovo laptops with the project's key enrolled; never walked through by a newcomer from a public release | [`SECURE-BOOT.md`](SECURE-BOOT.md) |
| Suspend, battery reporting, hibernation | not recorded as tried on any machine | [`BATTERY.md`](BATTERY.md) |

## Add your machine

Open a **Hardware report** issue: the model, the firmware settings you changed, what you saw at boot, what worked and what failed, and the output of `iyakki hwreport` or a *Share diagnostics* file ([`DOCTOR.md`](DOCTOR.md)). Please do not post serial numbers or network addresses.
