# Hardware matrix: machine by capability, status, evidence, owner steps

**Written:** 2026-10-27 (owner request: re-review the hardware matrix, v1.0.0 gate item 4 in [BACKLOG.md](BACKLOG.md)). **For:** the owner.
**Docs only.** Nothing was built, booted or written to a drive for this page, and no machine was reached. Every cell below comes from a file in
the repository or from the coordinator's observations of 2026-10-26/27 on the T2 MacBook Pro through developer mode, which are marked as such.
Where no evidence exists the cell says `untested` and the evidence column says `none`.

Related: [../HARDWARE.md](../HARDWARE.md) (what is supported, prose), [../../KNOWN-ISSUES.md](../../KNOWN-ISSUES.md),
RELEASE-READINESS.md (gap table, section 5.3 owner checklist), BETA-CHECKLIST.md (gates and the proposed
hardware waivers), [PC-TEST-SHEET.md](PC-TEST-SHEET.md) (tonight's two PCs), ../BETA-PLAN.md (lab H1 to H9).

## 0. How to read this page

One status word per cell:

| Word | Meaning here |
|---|---|
| **verified** | Seen working on that machine, with a dated pointer to a file or to the 2026-10-26/27 session. A repeat on the release image is still part of the gate. |
| **partly** | Some of it works, or it works with a named limit. The note says what is missing. |
| **failing** | Tried and does not work, and no fix exists that could be tried. |
| **untested** | Not tried, or tried before and a fix has been **built but not verified**. If something was seen failing, the note starts with `Last seen: FAILING`. Built-but-unverified is never counted as verified. |
| **not supported** | The machine has no such part (marked in the notes as `no such part`), or the product does not support it there by decision. |
| **waived** | The owner waived it in writing. **No cell is waived today**: section 5 only proposes waivers. |

Evidence rules: `owner report` means the owner said so and no file holds more; the coordinator session of 2026-10-26/27 is an observation, and no saved
`iyakki dev diag` file from it is named anywhere in the repository (the owner should attach one, section 8). Dates are the day of the observation
or `date not recorded`.

## 1. Machines

| Id | Machine | Image and medium so far | Role for 1.0.0 (proposed) |
|---|---|---|---|
| A | Mac T2 MacBook Pro 2020 (MacBookPro16,2) | macweb image from a USB SSD, then installed to the internal disk | Gate machine |
| B | Mac mini 2018 (Macmini8,1, T2) | macweb install image on a USB SSD (Samsung T7), 2026-10-18/19 | Gate machine |
| C | Mac mini 2014 (Macmini7,1), "the older Mac mini" | none | Gate machine only if the owner means this one by "the older Mac mini" (decision 1, section 5) |
| D | AMD CPU + NVIDIA GPU PC | PC image 2026-10-26 not yet run on it; owner reports an AMD PC booted earlier | Gate machine (the AMD PC) |
| E | Intel CPU + NVIDIA GPU PC: HP Pavilion Laptop 16-af0xxx (Core Ultra, Meteor Lake; NVIDIA 10de:25aa; Insyde F.11) | PC lean image 1107g (kernel with x2APIC) from a Samsung T7, 2026-11-07; the 1107e kernel stopped before stage 1 on it | Gate machine (the Intel PC) |
| F | Generic Intel iGPU PC | none | Not a gate machine; a result is a bonus |
| G | AMD iGPU PC | none | Not a gate machine; a result is a bonus |
| H | QEMU x86_64 | all automated boot, install, persistence tests | Automated gate, not hardware |
| I | QEMU aarch64 | UEFI boot self-test | Automated, not hardware |
| J | Apple Silicon, VM path only | none (recipe never run on a Mac) | Owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal is out of scope |
| K | Raspberry Pi 5 | none | Planned; out of 1.0.0 (proposal) |
| L | Raspberry Pi 3 (IYAKKI lite) | none | Planned; out of 1.0.0 (proposal) |
| M | NVIDIA Jetson | none | Not planned |

Note on D: [../HARDWARE.md](../HARDWARE.md) said "one AMD PC booted, model and GPU not written down". The owner now says the AMD + NVIDIA PC booted. This page
treats them as the same machine; that is an assumption, and the boot cell is only `partly` because nothing but the owner's word exists.

## 2. The matrix (one word per cell)

Column letters are the ids of section 1.


| Capability | A | B | C | D | E | F | G | H | I | J | K | L | M |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Boot from USB | verified | verified | untested | partly | untested | verified | untested | verified | verified | not supported | not supported | not supported | not supported |
| Install to internal disk | partly | untested | untested | untested | untested | untested | untested | verified | not supported | not supported | not supported | not supported | not supported |
| Install to USB drive | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported |
| Upgrade from previous release | untested | untested | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Secure Boot | untested | untested | not supported | untested | untested | verified | untested | verified | not supported | not supported | not supported | not supported | not supported |
| Display, internal | verified | not supported | not supported | untested | untested | untested | untested | verified | verified | untested | not supported | not supported | not supported |
| Display, external monitor | verified | verified | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Display, hot-plug | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Touch screen mapping | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Keyboard (and pointer) | verified | verified | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Touch Bar | partly | not supported | not supported | not supported | not supported | not supported | not supported | not supported | not supported | not supported | not supported | not supported | not supported |
| Fn keys | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Sound out | verified | verified | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Sound in (built-in) | not supported | not supported | not supported | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| USB camera | partly | verified | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| USB microphone | verified | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Built-in camera | verified | not supported | not supported | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Wi-Fi | verified | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Ethernet | partly | verified | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Bluetooth | mixed | 2026-11-10 | owner on a MacBook Pro 14,1 (2026-11-08): pair, phone sound and tether with internet; the Mac mini 2018 (BCM4364, no patch file in macOS images): the page scans and lists devices, pairing untried | In-house stack. MacBook Pro 14,1 works; on the Mac mini 2018 the controller runs in its ROM state, so a USB dongle is the fallback. |
| USB-C link and internet sharing | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| k3s start | verified | verified | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| GPU for k3s | untested | untested | untested | untested | untested | untested | partly | not supported | not supported | untested | not supported | not supported | not supported |
| Browser, software drawing | verified | verified | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Browser, hardware drawing | partly | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Video decode | partly | partly | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Battery and power source | untested | not supported | not supported | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Sleep (idle, lid) | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Suspend to RAM | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported | not supported | not supported | not supported |
| Shutdown | untested | untested | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Developer mode pairing | partly | untested | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |
| Firewall | partly | untested | untested | untested | untested | untested | untested | untested | untested | untested | not supported | not supported | not supported |

## 3. Counts

416 cells = 32 capabilities x 13 machines.

| Status | Cells |
|---|---|
| verified | 26 |
| partly | 11 |
| failing | 2 |
| untested | 208 |
| not supported | 169 |
| waived | 0 |
| **total** | **416** |

Of the 169 `not supported` cells, 50 are `no such part` (a Mac mini has no battery, a VM has no Touch Bar) and the rest are decisions or plans (Apple Silicon bare metal, Raspberry Pi, Jetson, the aarch64 installer, Secure Boot on a Mac mini 2014).
3 of the `untested` cells were last seen failing and have a built, unverified fix.

Per machine:

| Id | verified | partly | failing | untested | not supported | waived |
|---|---|---|---|---|---|---|
| A | 10 | 8 | 1 | 12 | 1 | 0 |
| B | 8 | 1 | 1 | 17 | 5 | 0 |
| C | 0 | 0 | 0 | 26 | 6 | 0 |
| D | 0 | 1 | 0 | 30 | 1 | 0 |
| E | 2 | 0 | 0 | 29 | 1 | 0 |
| F | 0 | 0 | 0 | 31 | 1 | 0 |
| G | 0 | 1 | 0 | 30 | 1 | 0 |
| H | 4 | 0 | 0 | 11 | 17 | 0 |
| I | 2 | 0 | 0 | 10 | 20 | 0 |
| J | 0 | 0 | 0 | 12 | 20 | 0 |
| K | 0 | 0 | 0 | 0 | 32 | 0 |
| L | 0 | 0 | 0 | 0 | 32 | 0 |
| M | 0 | 0 | 0 | 0 | 32 | 0 |


The machines that count for the gate are A to E. Their 146 cells that apply (no `no such part`) are: 20 verified, 10 partly, 2 failing, 114 untested.

## 4. Evidence per machine

Status, date and evidence for every cell. The test step for each cell that is not `verified` is in the machine's checklist (section 7).

### A. Mac T2 MacBook Pro 2020 (MacBookPro16,2)

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | verified | 2026-10-23 | docs/MAC-HARDWARE.md 'MacBookPro16,2: measured 2026-10-23'; KNOWN-ISSUES.md 'Machine 2' (owner report) | Booted from a Samsung T7 USB SSD; Option key at the chime. |
| Install to internal disk | partly | 2026-10-24 and 2026-10-26/27 | KNOWN-ISSUES.md 'Machine 2' (owner, first internal boot 2026-10-24); coordinator session via developer mode, 2026-10-26/27 (macweb image booted from the internal disk) | Not yet seen: macOS still boots afterwards, the drive-lost fix (F-LOSS), upgrade of that install. F-INST-03 is partial 80%. |
| Install to USB drive | untested | — | none |  |
| Upgrade from previous release | untested | — | none | iyakki-usb --update is not proven on a stick (RELEASE-READINESS rows 16, 17). |
| Secure Boot | untested | — | none | Not tried on a T2 Mac; needs the Startup Security Utility policy first (docs/SECURE-BOOT.md). |
| Display, internal | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | eDP-1 2560x1600. |
| Display, external monitor | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27; owner: 'external monitor features are good' | DP-4 1920x1080 over DisplayPort; two displays driven. |
| Display, hot-plug | untested | — | none | Detected and driven; unplug and replug while running was not recorded. |
| Touch screen mapping | untested | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | Last seen: FAILING. External touch panel wch.cn 27c0:0859 reacts on the wrong display. Fix built, unverified. |
| Keyboard (and pointer) | verified | 2026-10-23 | docs/MAC-HARDWARE.md 'MacBookPro16,2' (keyboard, trackpad pointer); owner report on the internal-disk boot | Tap, two-finger scroll, palm rejection not checked (MAC-HARDWARE). |
| Touch Bar | partly | 2026-10-23 | docs/MAC-HARDWARE.md 'MacBookPro16,2' (Touch Bar keys after a retry; backlight) | Keys row and backlight work; the IYAKKI-drawn strip never opens (USB configuration switch falls back). |
| Fn keys | untested | — | none | F-KBD-06 and docs/KEYBOARD-AND-FN-KEYS.md: built against made-up machines, not tried on a real Mac. |
| Sound out | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 (AppleT2x4 card); docs/MAC-HARDWARE.md 'T2 sound' (2026-10-23) | One program at a time (shared mixer is off by default, F-SND-10). |
| Sound in (built-in) | not supported | 2026-10-23 | docs/MAC-HARDWARE.md 'Never capture from this card' | Built-in microphone is off on purpose: capture hung the machine. Use a USB microphone. |
| USB camera | partly | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | EMEET SmartCam Nova 4K (UVC 328f:00af) detected. The camera picker was broken; fix built, unverified. Photo and video from it not recorded. |
| USB microphone | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 (USB-Audio of the EMEET camera; owner confirmed it works) |  |
| Built-in camera | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | Works at 640x480 YUYV (Apple camera driver downloaded by the person, F-DRIVERS-29). |
| Wi-Fi | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 (addresses 192.168.50.67 and .66 by DHCP) | BCM4364 B3 with Apple firmware installed by the person; the firmware job ran on 2026-10-23 (MAC-HARDWARE). |
| Ethernet | partly | 2026-10-23 | docs/MAC-HARDWARE.md 'Ethernet adapter (USB, 0bda:8153)' | Worked when present at boot; hot-plug fix is in source, not re-run. |
| Bluetooth | failing | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27; docs/MAC-HARDWARE.md (hci0 exists, no address) | BCM4364, no patch file exists in macOS images. USB dongle is the fallback (waiver proposed). |
| USB-C link and internet sharing | untested | — | none | F-NET-14 direct link: fakes only (partial 60%). |
| k3s start | verified | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 (node Ready, v1.37.1; started again at boot) |  |
| GPU for k3s | untested | — | none | Intel Iris Plus only (no discrete GPU in the 13-inch); stage 1 never run on it. |
| Browser, software drawing | verified | 2026-10-24 | KNOWN-ISSUES.md 'Machine 2' (owner: YouTube autoplay and sound, page fullscreen) |  |
| Browser, hardware drawing | partly | 2026-10-24 | KNOWN-ISSUES.md 'Machine 2' (owner: GPU browser helper worked) | No hardware video decode. |
| Video decode | partly | 2026-10-24 | docs/project/BACKLOG.md (YouTube smooth at 720p with CPU decode) | Software decode only. H.264/AAC add-on not run on a device. |
| Battery and power source | untested | — | none | Last seen: FAILING (only the adapter showed, docs/MAC-HARDWARE.md batteries). Kernel options written (F-BATT-05), no kernel built. |
| Sleep (idle, lid) | untested | — | none | F-PWR-01 is planned; nothing built. |
| Suspend to RAM | untested | — | none | Unreliable on T2 Macs; F-PWR-01 plans it as experimental. |
| Shutdown | untested | — | none | Clean power-off not recorded on this machine. |
| Developer mode pairing | partly | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | Works over Wi-Fi. Port 7979 is blocked by the device firewall when it is on; fix built, unverified. |
| Firewall | partly | 2026-10-26/27 | coordinator session via developer mode, 2026-10-26/27 | Firewall blocks as designed, but it also blocked developer mode (7979); fix built, unverified. |

### B. Mac mini 2018 (Macmini8,1, T2)

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 46) | GRUB waits 20 to 30 s at the logo (gate: boot budget not met). |
| Install to internal disk | untested | — | none | Internal-disk install never tried on this machine. |
| Install to USB drive | untested | — | none |  |
| Upgrade from previous release | untested | — | none |  |
| Secure Boot | untested | — | none | T2 Startup Security policy; never tried. |
| Display, internal | not supported | — | none | No internal display. |
| Display, external monitor | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 48, HDMI monitor) | Second monitor on DisplayPort not tried. |
| Display, hot-plug | untested | — | none | HDMI once reported disconnected until re-plugged; re-probe fix F-BOOT-21 is unverified on the Mac. |
| Touch screen mapping | untested | — | none |  |
| Keyboard (and pointer) | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 48, USB keyboard and mouse) |  |
| Touch Bar | not supported | — | none | No Touch Bar. |
| Fn keys | untested | — | none |  |
| Sound out | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 49: HDMI, built-in speaker, headphone jack) | Radio plus YouTube together fixed in a newer image, not re-tested on the Mac. |
| Sound in (built-in) | not supported | — | none | No built-in microphone. |
| USB camera | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 50, Fullhan 4MP: photo, video, passport picture) |  |
| USB microphone | untested | — | none |  |
| Built-in camera | not supported | — | none | No built-in camera. |
| Wi-Fi | untested | 2026-10-18/19 | KNOWN-ISSUES.md 'Hardware: what was tested' | Last seen: FAILING (never associated). The firmware route has since worked on the MacBook (A); re-run here. |
| Ethernet | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (items 46, 51: Ethernet used for Quick install downloads) |  |
| Bluetooth | failing | 2026-10-21 | KNOWN-ISSUES.md 'Bluetooth does not work'; docs/project/RELEASE-READINESS.md row 8 | BCM4364 has no patch file anywhere (measured). USB dongle is the fallback (waiver proposed). |
| USB-C link and internet sharing | untested | — | none |  |
| k3s start | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 51, node Ready) |  |
| GPU for k3s | untested | — | none |  |
| Browser, software drawing | verified | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 53, Google and YouTube VP9) |  |
| Browser, hardware drawing | untested | — | none | Playback was software-rendered (KNOWN-ISSUES). |
| Video decode | partly | 2026-10-18/19 | ALPHA-GATE.md items 46 to 54 (item 53) | VP9 plays; 720p dropped about 57 of 628 frames. H.264 does not play (policy). |
| Battery and power source | not supported | — | none | Desktop, no battery. |
| Sleep (idle, lid) | untested | — | none | F-PWR-01 planned. |
| Suspend to RAM | untested | — | none |  |
| Shutdown | untested | — | none |  |
| Developer mode pairing | untested | — | none | Developer mode never run on this machine. |
| Firewall | untested | — | none |  |

### C. Mac mini 2014 (Macmini7,1), the older mini

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Install to internal disk | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Install to USB drive | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Upgrade from previous release | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Secure Boot | not supported | — | none | No T2 chip and no Secure Boot with Linux on this Mac. |
| Display, internal | not supported | — | none | No internal display. |
| Display, external monitor | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Display, hot-plug | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Touch screen mapping | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Keyboard (and pointer) | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Touch Bar | not supported | — | none | No Touch Bar. |
| Fn keys | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Sound out | untested | — | none | HDMI audio expected; analog needs the Mac variant (CS4206). |
| Sound in (built-in) | not supported | — | none | No built-in microphone. |
| USB camera | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| USB microphone | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Built-in camera | not supported | — | none | No built-in camera. |
| Wi-Fi | untested | — | none | BCM4360 needs Broadcom's wl built by the owner (F-DRIVERS-10, F-DRIVERS-16); never loaded on a Macmini7,1. |
| Ethernet | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Bluetooth | untested | — | none | btusb binds with built-in firmware; limited (MAC-HARDWARE section 4). |
| USB-C link and internet sharing | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| k3s start | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| GPU for k3s | untested | — | none | Haswell HD 5000. |
| Browser, software drawing | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Browser, hardware drawing | untested | — | none | Haswell is not supported by Mesa iris (F-DRIVERS-15). |
| Video decode | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Battery and power source | not supported | — | none | Desktop, no battery. |
| Sleep (idle, lid) | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Suspend to RAM | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Shutdown | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Developer mode pairing | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |
| Firewall | untested | — | none | Never booted; docs/MAC-HARDWARE.md section 10 is the test list. |

### D. AMD CPU + NVIDIA GPU PC

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | partly | 2026-10 (week of 2026-10-20, date not recorded) | docs/HARDWARE.md 'An AMD PC' (owner report only; no log, model or image recorded) | Owner reports it booted; the 2026-10-26 PC image has not been run on it. Test tonight. |
| Install to internal disk | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Install to USB drive | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Upgrade from previous release | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Secure Boot | untested | — | none | Image is unsigned: Secure Boot off for the test. |
| Display, internal | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Display, external monitor | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Display, hot-plug | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Touch screen mapping | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Keyboard (and pointer) | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Touch Bar | not supported | — | none | Apple hardware only. |
| Fn keys | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sound out | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sound in (built-in) | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| USB camera | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| USB microphone | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Built-in camera | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Wi-Fi | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Ethernet | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Bluetooth | untested | — | none | Qualcomm and Realtek Bluetooth chips need firmware the person installs (docs/RESTRICTED-FIRMWARE.md). |
| USB-C link and internet sharing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| k3s start | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| GPU for k3s | untested | — | none | NVIDIA on nouveau (Turing and newer) unless the vendor driver is installed (docs/GPU-K3S.md; stage 3 never run on a real card). |
| Browser, software drawing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Browser, hardware drawing | untested | — | none | Expected software drawing on an NVIDIA-driven screen (llvmpipe; PC-TEST-SHEET section 4). |
| Video decode | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Battery and power source | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sleep (idle, lid) | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Suspend to RAM | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Shutdown | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Developer mode pairing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Firewall | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |

### E. Intel CPU + NVIDIA GPU PC: HP Pavilion Laptop 16-af0xxx

HP Pavilion Laptop 16-af0xxx, board 8CB6, Insyde BIOS F.11 (2024-09-26), Intel Meteor Lake (host bridge 8086:7d02, Arc iGPU 8086:7d45 on a
1920x1200 panel), NVIDIA 10de:25aa as a second GPU without display output, WD NVMe 15b7:5036 (not behind VMD), Realtek RTL8852BE Wi-Fi 10ec:b852
with Bluetooth on USB 0bda:b85c, camera 05c8:0445, 32 GB; two touch devices; the stick on the USB4 xHCI 8086:7ec0. All from the loader's boot log
on the stick (`EFI/iyakki-boot/last-boot.log`, docs/BOOT-LOADER.md), 2026-11-07. History: the 1107e kernel (no `CONFIG_X86_X2APIC`) died before
stage 1 on this machine; the rebuilt kernel boots it (docs/development/HP-LAPTOP-PLAN.md).

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | verified | 2026-11-07 | owner's report ("it booted on hp") after the kernel was rebuilt with x2APIC and interrupt remapping; the loader's log of the failing boot is under the owner's work folder (hp-patch/hp-run-1) | Samsung T7, lean image 1107g, the loader as shim's second stage, default entry with `iyakki.root=boot`. The stick's logs of the successful boot are not yet read back. |
| Install to internal disk | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Install to USB drive | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Upgrade from previous release | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Secure Boot | verified | 2026-11-07 | the loader's log on the stick: "secure boot on", shim and MOK accepted the signed loader and the signed kernel; the same chain booted after the kernel rebuild | The IYAKKI key enrolled in MOK by the owner. |
| Display, internal | untested | — | none | The boot reached IYAKKI (owner's report); resolution, scale and the Arc iGPU driver not yet checked (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Display, external monitor | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Display, hot-plug | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Touch screen mapping | untested | — | none | The firmware lists two absolute-pointer devices; not tried in IYAKKI yet. |
| Keyboard (and pointer) | untested | — | none | The loader's menu took the keyboard (an entry was chosen by hand on the first run); IYAKKI itself not yet checked. |
| Touch Bar | not supported | — | none | Apple hardware only. |
| Fn keys | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sound out | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sound in (built-in) | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| USB camera | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| USB microphone | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Built-in camera | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Wi-Fi | untested | — | none | Realtek RTL8852BE (10ec:b852): needs the Realtek firmware the person installs from Store -> Setup (docs/FIRMWARE-LICENCES.md); not tried yet. |
| Ethernet | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Bluetooth | untested | — | none | Qualcomm and Realtek Bluetooth chips need firmware the person installs (docs/RESTRICTED-FIRMWARE.md). |
| USB-C link and internet sharing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| k3s start | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| GPU for k3s | untested | — | none | NVIDIA on nouveau (Turing and newer) unless the vendor driver is installed (docs/GPU-K3S.md; stage 3 never run on a real card). |
| Browser, software drawing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Browser, hardware drawing | untested | — | none | Expected software drawing on an NVIDIA-driven screen (llvmpipe; PC-TEST-SHEET section 4). |
| Video decode | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Battery and power source | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Sleep (idle, lid) | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Suspend to RAM | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Shutdown | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Developer mode pairing | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |
| Firewall | untested | — | none | Image built 2026-10-26; booted only in QEMU/UEFI, never on this hardware (docs/project/PC-TEST-SHEET.md sections 3 and 8). |

### F. Generic Intel iGPU PC

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | untested | — | none | No such PC has been booted; QEMU only. |
| Install to internal disk | untested | — | none | No such PC has been booted; QEMU only. |
| Install to USB drive | untested | — | none | No such PC has been booted; QEMU only. |
| Upgrade from previous release | untested | — | none | No such PC has been booted; QEMU only. |
| Secure Boot | untested | — | none | No such PC has been booted; QEMU only. |
| Display, internal | untested | — | none | No such PC has been booted; QEMU only. |
| Display, external monitor | untested | — | none | No such PC has been booted; QEMU only. |
| Display, hot-plug | untested | — | none | No such PC has been booted; QEMU only. |
| Touch screen mapping | untested | — | none | No such PC has been booted; QEMU only. |
| Keyboard (and pointer) | untested | — | none | No such PC has been booted; QEMU only. |
| Touch Bar | not supported | — | none | Apple hardware only. |
| Fn keys | untested | — | none | No such PC has been booted; QEMU only. |
| Sound out | untested | — | none | No such PC has been booted; QEMU only. |
| Sound in (built-in) | untested | — | none | No such PC has been booted; QEMU only. |
| USB camera | untested | — | none | No such PC has been booted; QEMU only. |
| USB microphone | untested | — | none | No such PC has been booted; QEMU only. |
| Built-in camera | untested | — | none | No such PC has been booted; QEMU only. |
| Wi-Fi | untested | — | none | No such PC has been booted; QEMU only. |
| Ethernet | untested | — | none | No such PC has been booted; QEMU only. |
| Bluetooth | untested | — | none | No such PC has been booted; QEMU only. |
| USB-C link and internet sharing | untested | — | none | No such PC has been booted; QEMU only. |
| k3s start | untested | — | none | No such PC has been booted; QEMU only. |
| GPU for k3s | untested | — | none | Vulkan stage 1 not tried on an Intel iGPU. |
| Browser, software drawing | untested | — | none | No such PC has been booted; QEMU only. |
| Browser, hardware drawing | untested | — | none | i915 or xe plus Mesa iris are in the image; not run on hardware (PC-TEST-SHEET section 4). |
| Video decode | untested | — | none | No such PC has been booted; QEMU only. |
| Battery and power source | untested | — | none | No such PC has been booted; QEMU only. |
| Sleep (idle, lid) | untested | — | none | No such PC has been booted; QEMU only. |
| Suspend to RAM | untested | — | none | No such PC has been booted; QEMU only. |
| Shutdown | untested | — | none | No such PC has been booted; QEMU only. |
| Developer mode pairing | untested | — | none | No such PC has been booted; QEMU only. |
| Firewall | untested | — | none | No such PC has been booted; QEMU only. |

### G. AMD iGPU PC

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | untested | — | none | No such PC has been booted; QEMU only. |
| Install to internal disk | untested | — | none | No such PC has been booted; QEMU only. |
| Install to USB drive | untested | — | none | No such PC has been booted; QEMU only. |
| Upgrade from previous release | untested | — | none | No such PC has been booted; QEMU only. |
| Secure Boot | untested | — | none | No such PC has been booted; QEMU only. |
| Display, internal | untested | — | none | No such PC has been booted; QEMU only. |
| Display, external monitor | untested | — | none | No such PC has been booted; QEMU only. |
| Display, hot-plug | untested | — | none | No such PC has been booted; QEMU only. |
| Touch screen mapping | untested | — | none | No such PC has been booted; QEMU only. |
| Keyboard (and pointer) | untested | — | none | No such PC has been booted; QEMU only. |
| Touch Bar | not supported | — | none | Apple hardware only. |
| Fn keys | untested | — | none | No such PC has been booted; QEMU only. |
| Sound out | untested | — | none | No such PC has been booted; QEMU only. |
| Sound in (built-in) | untested | — | none | No such PC has been booted; QEMU only. |
| USB camera | untested | — | none | No such PC has been booted; QEMU only. |
| USB microphone | untested | — | none | No such PC has been booted; QEMU only. |
| Built-in camera | untested | — | none | No such PC has been booted; QEMU only. |
| Wi-Fi | untested | — | none | No such PC has been booted; QEMU only. |
| Ethernet | untested | — | none | No such PC has been booted; QEMU only. |
| Bluetooth | untested | — | none | No such PC has been booted; QEMU only. |
| USB-C link and internet sharing | untested | — | none | No such PC has been booted; QEMU only. |
| k3s start | untested | — | none | No such PC has been booted; QEMU only. |
| GPU for k3s | partly | date not recorded | docs/GPU-K3S.md section 7 (Radeon 780M on the development PC, stage 1 in k3s under the PC's own OS) | Not IYAKKI booted on it; stage 1 proven on that GPU only. |
| Browser, software drawing | untested | — | none | No such PC has been booted; QEMU only. |
| Browser, hardware drawing | untested | — | none | amdgpu plus Mesa radeonsi are in the image; not run on hardware. |
| Video decode | untested | — | none | No such PC has been booted; QEMU only. |
| Battery and power source | untested | — | none | No such PC has been booted; QEMU only. |
| Sleep (idle, lid) | untested | — | none | No such PC has been booted; QEMU only. |
| Suspend to RAM | untested | — | none | No such PC has been booted; QEMU only. |
| Shutdown | untested | — | none | No such PC has been booted; QEMU only. |
| Developer mode pairing | untested | — | none | No such PC has been booted; QEMU only. |
| Firewall | untested | — | none | No such PC has been booted; QEMU only. |

### H. QEMU x86_64

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | verified | 2026-10-26 | docs/project/PC-TEST-SHEET.md section 8 (qemu-test.sh PASS with 4 GB; install image boots under OVMF) | BIOS and UEFI scripts in dist/buildroot/. |
| Install to internal disk | verified | date not recorded | dist/buildroot/qemu-install-test.sh; docs/HARDWARE.md 'QEMU / KVM' | BIOS, UEFI, GPT, 4Kn virtual disks. |
| Install to USB drive | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Upgrade from previous release | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Secure Boot | verified | 2026-10-18 | docs/SECURE-BOOT.md 'Verified in QEMU' (OVMF, 17 checks) | QEMU only; not any real PC. |
| Display, internal | verified | date not recorded | F-BOOT-17 (boot screen test with screenshots) |  |
| Display, external monitor | not supported | — | none | No such part in the virtual machine. |
| Display, hot-plug | not supported | — | none | No such part in the virtual machine. |
| Touch screen mapping | not supported | — | none | No such part in the virtual machine. |
| Keyboard (and pointer) | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Touch Bar | not supported | — | none | No such part in the virtual machine. |
| Fn keys | not supported | — | none | No such part in the virtual machine. |
| Sound out | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Sound in (built-in) | not supported | — | none | No such part in the virtual machine. |
| USB camera | not supported | — | none | No such part in the virtual machine. |
| USB microphone | not supported | — | none | No such part in the virtual machine. |
| Built-in camera | not supported | — | none | No such part in the virtual machine. |
| Wi-Fi | not supported | — | none | No such part in the virtual machine. |
| Ethernet | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Bluetooth | not supported | — | none | No such part in the virtual machine. |
| USB-C link and internet sharing | not supported | — | none | No such part in the virtual machine. |
| k3s start | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| GPU for k3s | not supported | — | none | No such part in the virtual machine. |
| Browser, software drawing | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Browser, hardware drawing | not supported | — | none | No such part in the virtual machine. |
| Video decode | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Battery and power source | not supported | — | none | No such part in the virtual machine. |
| Sleep (idle, lid) | not supported | — | none | No such part in the virtual machine. |
| Suspend to RAM | not supported | — | none | No such part in the virtual machine. |
| Shutdown | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Developer mode pairing | untested | — | none | Not a target of the gate; only what a script checks is marked. |
| Firewall | untested | — | none | Not a target of the gate; only what a script checks is marked. |

### I. QEMU aarch64

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | verified | date not recorded | dist/buildroot/qemu-uefi-aarch64-test.sh; F-BOOT-08 (self-test passes) | Software emulation; slow. Boots from a disk image, not a stick. |
| Install to internal disk | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Install to USB drive | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Upgrade from previous release | untested | — | none | Not a target of the gate. |
| Secure Boot | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Display, internal | verified | date not recorded | F-BOOT-08 (framebuffer found) |  |
| Display, external monitor | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Display, hot-plug | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Touch screen mapping | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Keyboard (and pointer) | untested | — | none | Not a target of the gate. |
| Touch Bar | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Fn keys | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Sound out | untested | — | none | Not a target of the gate. |
| Sound in (built-in) | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| USB camera | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| USB microphone | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Built-in camera | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Wi-Fi | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Ethernet | untested | — | none | Not a target of the gate. |
| Bluetooth | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| USB-C link and internet sharing | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| k3s start | untested | — | none | Not a target of the gate. |
| GPU for k3s | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Browser, software drawing | untested | — | none | Not a target of the gate. |
| Browser, hardware drawing | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Video decode | untested | — | none | Not a target of the gate. |
| Battery and power source | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Sleep (idle, lid) | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Suspend to RAM | not supported | — | none | No such part, or no installer for the aarch64 image (docs/HARDWARE.md). |
| Shutdown | untested | — | none | Not a target of the gate. |
| Developer mode pairing | untested | — | none | Not a target of the gate. |
| Firewall | untested | — | none | Not a target of the gate. |

### J. Apple Silicon, VM path only

| Capability | Status | Date | Evidence | Note |
|---|---|---|---|---|
| Boot from USB | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Install to internal disk | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Install to USB drive | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Upgrade from previous release | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Secure Boot | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Display, internal | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Display, external monitor | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Display, hot-plug | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Touch screen mapping | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Keyboard (and pointer) | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Touch Bar | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Fn keys | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Sound out | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Sound in (built-in) | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| USB camera | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| USB microphone | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Built-in camera | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Wi-Fi | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Ethernet | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Bluetooth | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| USB-C link and internet sharing | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| k3s start | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| GPU for k3s | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Browser, software drawing | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Browser, hardware drawing | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Video decode | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Battery and power source | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Sleep (idle, lid) | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Suspend to RAM | not supported | 2026-10-27 | FEATURES.md F-BOOT-09 (owner decision 2026-10-27: lightweight VM + k3s + model bridge; bare metal out of scope) | Bare metal is not promised; a VM has no such part or passes it through the host. |
| Shutdown | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Developer mode pairing | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |
| Firewall | untested | — | none | VM path: the recipe was never run on a Mac (F-BOOT-08 'Never run on a Mac'). |

### K. Raspberry Pi 5

Planned, see docs/development/RASPBERRY-PI-PLAN.md; nothing built or booted. Every cell is `not supported` (2026-10-26, [../HARDWARE.md](../HARDWARE.md) and [../development/RASPBERRY-PI-PLAN.md](../development/RASPBERRY-PI-PLAN.md)). Nothing to test.

### L. Raspberry Pi 3 (IYAKKI lite)

Planned as IYAKKI lite (headless), docs/development/RASPBERRY-PI-PLAN.md section 5; nothing built. Every cell is `not supported` (2026-10-26, [../HARDWARE.md](../HARDWARE.md) and [../development/RASPBERRY-PI-PLAN.md](../development/RASPBERRY-PI-PLAN.md)). Nothing to test.

### M. NVIDIA Jetson

Not planned (RASPBERRY-PI-PLAN.md note on Jetson). Every cell is `not supported` (2026-10-26, [../HARDWARE.md](../HARDWARE.md) and [../development/RASPBERRY-PI-PLAN.md](../development/RASPBERRY-PI-PLAN.md)). Nothing to test.

## 5. Re-rank: release-blocking for 1.0.0, or waivable in writing

The rule is BETA-CHECKLIST section 3: a waiver names the row, the reason and the date, and the owner decides. **What cannot be waived by a note:** upgrade from the previous release (data safety), a clean shutdown that does not corrupt the data volume, no unexplained
kernel `WARN`/`ERROR` or false alarm on a gate machine (gate item 2), and a repeat of every `verified` cell on the release image.

### 5.1 Release-blocking (the gate machines A to E)

| Capability | Blocks 1.0.0 on | Why |
|---|---|---|
| Boot from USB | A, B, C (if listed), D, E | The first thing a user does. |
| Install to internal disk | A, D, E (B and C if listed) | Gate item 4 names internal and USB installs. Only A has a partial result. |
| Install to USB drive | One gate machine | Gate item 4. |
| Upgrade from previous release | A and one PC | Gate item 5; cannot be waived. Needs `iyakki-usb --update` to exist first. |
| Display, internal and external | A, D, E (and the PCs' monitor) | Without a picture nothing else matters. A is verified. |
| Keyboard and pointer | all gate machines | A, B verified. |
| Sound out | A, B, D, E | A, B verified. |
| Ethernet | B, D, E (A has a USB adapter) | Needed for Quick install. B verified. |
| Wi-Fi | A (verified), B, D, E where a Wi-Fi chip exists | Gate item 4 lists Wi-Fi. B needs a re-run. |
| k3s start | all gate machines | A, B verified. |
| Browser, software drawing, and video decode (software, limits stated) | all gate machines | A, B verified or partly. |
| Developer mode pairing and Firewall together | A, one PC | The firewall blocked developer mode on A. Both are shipping features; fix and verify. |
| Battery and power source (laptops) | A | The low-battery path can power the machine off; it has never been seen on a real battery. |
| Shutdown | all gate machines | Clean power-off, no data loss. |
| USB camera, USB microphone | A (camera picker fix) | Camera picker was broken; mic is verified. |

### 5.2 May be waived in writing (recommendations for the owner)

| # | Cells | Recommendation | Reason and condition |
|---|---|---|---|
| W1 | Bluetooth on A, B, C | **Waive.** Document a USB Bluetooth dongle. | BCM4364 has no patch file in any macOS image, no legal source (RELEASE-READINESS row 8). Condition: one dongle test on a Mac (step in the checklists) and an honest Settings page (F-NET-13, built). |
| W2 | Secure Boot on all real hardware | **Declare "Secure Boot off only" for 1.0.0**, or do it. My recommendation: declare. | Only QEMU/OVMF was verified (17 checks); the initial RAM disk is unsigned, GRUB 2.12 lacks 2025 fixes, key custody is open. Doing it is about 5 person-days plus owner time on one PC and one T2 Mac. If declared, HARDWARE.md and SECURITY.md say it plainly. |
| W3 | Apple Silicon bare metal | **Out of 1.0.0 (decided 2026-10-27).** F-BOOT-09, F-DRIVERS-12, F-DRIVERS-19 are `deprecated`. | The VM path (lightweight VM + k3s + model bridge) is the promise; it needs its own run on the owner's Mac before it is claimed (column J is all `untested`). |
| W4 | Raspberry Pi 5 and Pi 3 lite | **Out of 1.0.0.** | Planned only; nothing built (RASPBERRY-PI-PLAN). |
| W5 | NVIDIA Jetson | Nothing to waive: not planned. | |
| W6 | Built-in microphone of T2 Macs | **Waive.** USB microphone is verified. | Capture on the T2 card hung the machine; it stays off on purpose. |
| W7 | IYAKKI-drawn Touch Bar strip | **Waive the strip.** Keys row and backlight work. | USB configuration switch falls back; a separate bring-up (BACKLOG). |
| W8 | Sleep and suspend, all machines | **Declare "no sleep in 1.0.0"; keep screen blank and power saver.** | F-PWR-01 is planned and nothing is built; T2 suspend is unreliable. If the owner wants sleep, it becomes blocking on A and one PC. |
| W9 | Hardware video decode | **Waive; state the software limit.** | CPU decode: 720p smooth on A; on B 57 of 628 frames dropped at 720p. H.264/AAC only through the user-downloaded add-on. |
| W10 | NVIDIA vendor driver and GPU for k3s stage 3 | **Waive as a 1.0.0 promise unless tonight's run passes.** Stage 1 (Vulkan) only where verified. | Never run on a real NVIDIA card (docs/GPU-K3S.md section 7). |
| W11 | Wi-Fi of the Mac mini 2014 (wl self-built) | **Waive** (F-DRIVERS-10 is class C). | Needs the owner's own Broadcom build; never loaded on that Mac. |
| W12 | USB-C link and internet sharing | **Waive unless one pair of machines passes.** | Developer convenience; fakes only (F-NET-14). |
| W13 | Legacy BIOS PCs, 4Kn disks | **Declare "UEFI only, 512-byte disks" for 1.0.0.** | Tested in QEMU only; no old PC in the lab. |
| W14 | Touch screen mapping | **Verify once; waive on failure with a line in HARDWARE.md.** | Only a USB touch panel on A was observed; fix is built. |
| W15 | Fn keys | **Verify on A; waive elsewhere.** | Built against made-up machines (KEYBOARD-AND-FN-KEYS). |
| W16 | Mac mini 2014 as a whole | **Decision 1:** if "the older Mac mini" is the 2014, then boot, Ethernet, sound out and display are blocking for it, and Wi-Fi, Bluetooth, fan sensors are waived. If it is the 2018 mini, remove C from the gate list. | Gate item 4 says "the older Mac mini". The lab has both. |

Decisions for the owner, in short: **1** which mini is "the older Mac mini"; **2** Secure Boot do or declare; **3** sleep build or declare; **4** accept W1, W6, W7, W9, W12, W13.


## 6. The ten owner test steps that unlock the most cells

| # | Step | Machine | Cells it can move |
|---|---|---|---|
| 1 | Tonight: hardware check entry, `iyakki hwreport` saved, three cold boots timed, administrator created, reboot, `iyakki dev diag -o FILE` (PC-TEST-SHEET sections 2, 3, 6) | D and E | boot, display, keyboard, Ethernet, sound, shutdown, devmode, about 8 cells each |
| 2 | Install to a spare SSD with `iyakki install --dry-run` first, boot from it twice | D or E | install to internal (gate), data persistence |
| 3 | Wi-Fi join and a web page on each PC with a Wi-Fi chip, then Ethernet | D, E | Wi-Fi, Ethernet |
| 4 | One session on the MacBook with the image that carries the five built fixes: touch mapping on the monitor hub, camera picker, firewall ON with developer mode on port 7979, TALES drift, dev cluster after an address change | A | 5 cells from `untested` or `partly` to a result |
| 5 | Mac mini 2018: Mac drivers page, Download from Apple, join Wi-Fi; then three cold boots and an HDMI replug for the flicker fix | B | Wi-Fi, hot-plug, boot budget |
| 6 | Browser on both PCs: simple page, modern site, YouTube; note renderer (expect llvmpipe on NVIDIA) | D, E | browser, video |
| 7 | GPU page, Install stage 1 on nouveau; then, only if that passes, the NVIDIA vendor driver steps of PC-TEST-SHEET section 5 and the 4090 list of docs/GPU-K3S.md section 8 | D, E | GPU for k3s, W10 |
| 8 | Battery on the MacBook with the new kernel image: `ls /sys/class/power_supply`, Settings, System, Battery, unplug the adapter | A | battery (blocking on a laptop) |
| 9 | Power off from the menu on every machine you touch and note the next boot's messages | A, B, D, E | shutdown (not waivable) |
| 10 | Decide section 5 decisions 1 to 4; then the Secure Boot enrol test on one PC only if you chose to do it | owner | W2, W8, W16 |

Not an owner step yet: the upgrade test. It waits for `iyakki-usb --update`; when that exists, it is the next one to run on A and one PC.

## 7. Printable checklists (one page each)

Use one page per machine. Tick the box, write PASS / FAIL / a short note in the last column. "Evidence" says what to save: **D** = `iyakki dev diag -o NAME-DATE-diag.txt`,
**H** = `iyakki hwreport` (or press S in the hardware check; it saves `/var/lib/iyakki/hwreport.txt`), **S** = `iyakki dev screenshot -o NAME-DATE-WHAT.png`
(or a phone photo), **L** = the `dmesg` lines asked for, **note** = a line is enough. Rows marked `repeat` are `verified` already and are repeated on the release image.
Never write a drive without checking its name, size and serial; install tests only on a disk you can lose. File names: `LETTER-DATE-what`, for example `A-2026-11-02-diag.txt`.

<div style="page-break-before: always"></div>

### Checklist A. Mac T2 MacBook Pro 2020 (MacBookPro16,2)

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine A of this page, and the evidence files named `A-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | verified | repeat: Hold Option at the chime, pick EFI Boot; on the internal disk: boot twice from it. | H | |
| [ ] | Install to internal disk | partly | Boot the internal disk a third time after a cold start; check macOS still boots (Option picker). | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Only if Secure Boot stays in 1.0.0: set Startup Security Utility to allow it, then enrol the key (docs/SECURE-BOOT.md). | S L | |
| [ ] | Display, internal | verified | repeat: Hardware check lists the panel and its native size. iyakki dev screenshot of the sign-in screen. | H S | |
| [ ] | Display, external monitor | verified | repeat: Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | Touch panel on the monitor hub: touch the corners of the external screen; the pointer must follow on that screen. | note | |
| [ ] | Keyboard (and pointer) | verified | repeat: Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Touch Bar | partly | Touch Bar: Esc, F-keys and media keys respond; Settings, Keyboard shows the strip opened or the reason it did not. | S | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | verified | repeat: Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | USB camera | partly | CAMERA tile, Change: the EMEET is listed; photo and 10 s video from it. | S | |
| [ ] | USB microphone | verified | repeat: Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Built-in camera | verified | repeat: CAMERA tile on the built-in camera: photo and 10 s video; note the size and format. | S | |
| [ ] | Wi-Fi | verified | repeat: Settings, Network: scan, join your access point, open a web page, reboot, it joins again. Note chip and driver. | D | |
| [ ] | Ethernet | partly | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | failing | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | verified | repeat: Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | Store, Setup, GPU card, Open its page, Install, Run test prompt (docs/GPU-K3S.md section 8). Send the job log whatever happens. | D L | |
| [ ] | Browser, software drawing | verified | repeat: BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | partly | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | partly | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Battery and power source | untested | On the NEW image: ls /sys/class/power_supply shows BAT0; unplug the adapter and watch Settings, System, Battery. | S | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | partly | Developer mode with the firewall ON: iyakki dev pair works over Wi-Fi. | D | |
| [ ] | Firewall | partly | Firewall ON: developer mode reachable, a closed port blocked, counter rises. | D | |

Rows left out are `not supported` on this machine (1). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist B. Mac mini 2018 (Macmini8,1, T2)

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine B of this page, and the evidence files named `B-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | verified | repeat: Hold Option at the chime, pick EFI Boot. Time three cold boots (GRUB, first picture, sign-in). | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Decision first (section 5). If tested: sign with the owner key (docs/SECURE-BOOT.md), enable Secure Boot, enrol the key in MokManager, boot; write down the exact refusal if it fails. | S L | |
| [ ] | Display, external monitor | verified | repeat: Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | Unplug and replug HDMI, then three cold boots: no flicker (F-BOOT-21, F-BOOT-23). | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | verified | repeat: Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | verified | repeat: Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | USB camera | verified | repeat: Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Wi-Fi | untested | Run the Mac drivers page, Download from Apple (docs/MAC-WIFI-BLUETOOTH-SETUP.md), then join your access point; write the exact outcome. | D | |
| [ ] | Ethernet | verified | repeat: Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | failing | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | verified | repeat: Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | Store, Setup, GPU card, Open its page, Install, Run test prompt (docs/GPU-K3S.md section 8). Send the job log whatever happens. | D L | |
| [ ] | Browser, software drawing | verified | repeat: BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | partly | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (5). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist C. Mac mini 2014 (Macmini7,1), the older mini

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine C of this page, and the evidence files named `C-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | untested | Hold Option at the chime. Follow docs/MAC-HARDWARE.md section 10, row 1 and 2. | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Display, external monitor | untested | Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | untested | Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | untested | Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | USB camera | untested | Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Wi-Fi | untested | Only if you build the wl pack (dist/drivers/build-wl-pack.sh, section 10 rows 4 and 4b); otherwise write 'waived'. | D | |
| [ ] | Ethernet | untested | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | untested | docs/MAC-HARDWARE.md section 10, rows 5 and 6. | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | Store, Setup, GPU card, Open its page, Install, Run test prompt (docs/GPU-K3S.md section 8). Send the job log whatever happens. | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | untested | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (6). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist D. AMD CPU + NVIDIA GPU PC

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine D of this page, and the evidence files named `D-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | partly | Flash the stick with iyakki-usb (check name and size, read-back verified). Secure Boot off. One-time boot menu, pick the stick, boot entry hardware check first, press S to save, then boot IYAKKI to the administrator screen. | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Decision first (section 5). If tested: sign with the owner key (docs/SECURE-BOOT.md), enable Secure Boot, enrol the key in MokManager, boot; write down the exact refusal if it fails. | S L | |
| [ ] | Display, internal | untested | Hardware check lists the panel and its native size. iyakki dev screenshot of the sign-in screen. | H S | |
| [ ] | Display, external monitor | untested | Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | untested | Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | untested | Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | Sound in (built-in) | untested | Voice tile with the built-in microphone. Do not capture from the T2 card; just confirm the tile says so. | note | |
| [ ] | USB camera | untested | Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Built-in camera | untested | CAMERA tile on the built-in camera: photo and 10 s video; note the size and format. | S | |
| [ ] | Wi-Fi | untested | Settings, Network: scan, join your access point, open a web page, reboot, it joins again. Note chip and driver. | D | |
| [ ] | Ethernet | untested | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | untested | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | GPU page on nouveau; Install stage 1; then NVIDIA vendor driver only if the basic test works (PC-TEST-SHEET section 5). | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | untested | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Battery and power source | untested | ls /sys/class/power_supply (a BAT entry); Settings, System, Battery; unplug the adapter, the percentage falls and the top bar shows it. Do not drain to empty. | S | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (1). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist E. Intel CPU + NVIDIA GPU PC

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine E of this page, and the evidence files named `E-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | untested | Flash the stick with iyakki-usb (check name and size, read-back verified). Secure Boot off. One-time boot menu, pick the stick, boot entry hardware check first, press S to save, then boot IYAKKI to the administrator screen. | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Decision first (section 5). If tested: sign with the owner key (docs/SECURE-BOOT.md), enable Secure Boot, enrol the key in MokManager, boot; write down the exact refusal if it fails. | S L | |
| [ ] | Display, internal | untested | Hardware check lists the panel and its native size. iyakki dev screenshot of the sign-in screen. | H S | |
| [ ] | Display, external monitor | untested | Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | untested | Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | untested | Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | Sound in (built-in) | untested | Voice tile with the built-in microphone. Do not capture from the T2 card; just confirm the tile says so. | note | |
| [ ] | USB camera | untested | Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Built-in camera | untested | CAMERA tile on the built-in camera: photo and 10 s video; note the size and format. | S | |
| [ ] | Wi-Fi | untested | Settings, Network: scan, join your access point, open a web page, reboot, it joins again. Note chip and driver. | D | |
| [ ] | Ethernet | untested | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | untested | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | GPU page on nouveau; Install stage 1; then NVIDIA vendor driver only if the basic test works (PC-TEST-SHEET section 5). | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | untested | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Battery and power source | untested | ls /sys/class/power_supply (a BAT entry); Settings, System, Battery; unplug the adapter, the percentage falls and the top bar shows it. Do not drain to empty. | S | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (1). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist F. Generic Intel iGPU PC

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine F of this page, and the evidence files named `F-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | untested | Flash the stick with iyakki-usb (check name and size, read-back verified). Secure Boot off. One-time boot menu, pick the stick, boot entry hardware check first, press S to save, then boot IYAKKI to the administrator screen. | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Decision first (section 5). If tested: sign with the owner key (docs/SECURE-BOOT.md), enable Secure Boot, enrol the key in MokManager, boot; write down the exact refusal if it fails. | S L | |
| [ ] | Display, internal | untested | Hardware check lists the panel and its native size. iyakki dev screenshot of the sign-in screen. | H S | |
| [ ] | Display, external monitor | untested | Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | untested | Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | untested | Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | Sound in (built-in) | untested | Voice tile with the built-in microphone. Do not capture from the T2 card; just confirm the tile says so. | note | |
| [ ] | USB camera | untested | Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Built-in camera | untested | CAMERA tile on the built-in camera: photo and 10 s video; note the size and format. | S | |
| [ ] | Wi-Fi | untested | Settings, Network: scan, join your access point, open a web page, reboot, it joins again. Note chip and driver. | D | |
| [ ] | Ethernet | untested | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | untested | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | untested | Store, Setup, GPU card, Open its page, Install, Run test prompt (docs/GPU-K3S.md section 8). Send the job log whatever happens. | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | untested | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Battery and power source | untested | ls /sys/class/power_supply (a BAT entry); Settings, System, Battery; unplug the adapter, the percentage falls and the top bar shows it. Do not drain to empty. | S | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (1). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist G. AMD iGPU PC

Date ____ / ____ / ______  Image sha-256 (first 12) ______________  Tester ____________  Results go to: section 4, machine G of this page, and the evidence files named `G-DATE-what` kept with the lab sheet.

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Boot from USB | untested | Flash the stick with iyakki-usb (check name and size, read-back verified). Secure Boot off. One-time boot menu, pick the stick, boot entry hardware check first, press S to save, then boot IYAKKI to the administrator screen. | H | |
| [ ] | Install to internal disk | untested | Boot the stick. Settings, System, Install to disk (or iyakki install --list, then --dry-run --disk NAME, then install) onto a disk you can lose. Remove the stick, boot from the disk twice. Data survives. | D S | |
| [ ] | Install to USB drive | untested | Boot the stick, iyakki install --list, install onto a SECOND USB drive (never the running stick), boot from that drive. | D | |
| [ ] | Upgrade from previous release | untested | Blocked until iyakki-usb --update exists. Then: stick from the previous release with data on it, update in place, boot, data and settings kept. Interrupt power once on a spare stick. | D | |
| [ ] | Secure Boot | untested | Decision first (section 5). If tested: sign with the owner key (docs/SECURE-BOOT.md), enable Secure Boot, enrol the key in MokManager, boot; write down the exact refusal if it fails. | S L | |
| [ ] | Display, internal | untested | Hardware check lists the panel and its native size. iyakki dev screenshot of the sign-in screen. | H S | |
| [ ] | Display, external monitor | untested | Plug the monitor before boot. Settings, Displays lists both; iyakki dev screenshot; note resolution. | D S | |
| [ ] | Display, hot-plug | untested | While running, unplug and replug the monitor cable three times. The picture returns without a reboot. | D | |
| [ ] | Touch screen mapping | untested | With a touch screen on one display, touch a corner of each display. The pointer lands on the touched screen (Settings, Displays). | note | |
| [ ] | Keyboard (and pointer) | untested | Hardware check live tester (keys turn green); type in the administrator form; move and click the pointer. | H | |
| [ ] | Fn keys | untested | Settings, Keyboard, Last special keys: press F1 to F12 with and without Fn; flip Function keys mode; restart; it holds. | S | |
| [ ] | Sound out | untested | Settings, Sound, Test sound on every output listed (speaker, jack, HDMI or DP); play the radio tile; volume and mute keys. | D | |
| [ ] | Sound in (built-in) | untested | Voice tile with the built-in microphone. Do not capture from the T2 card; just confirm the tile says so. | note | |
| [ ] | USB camera | untested | Plug a USB camera; CAMERA tile: the picker lists it; take a photo and a 10 s video. | S | |
| [ ] | USB microphone | untested | Plug a USB microphone or headset; Voice tile: the level moves; record 5 s and play it back. | D | |
| [ ] | Built-in camera | untested | CAMERA tile on the built-in camera: photo and 10 s video; note the size and format. | S | |
| [ ] | Wi-Fi | untested | Settings, Network: scan, join your access point, open a web page, reboot, it joins again. Note chip and driver. | D | |
| [ ] | Ethernet | untested | Plug Ethernet before boot (and once while running): top bar says connected, an address, a web page loads. | D | |
| [ ] | Bluetooth | untested | Settings, Bluetooth: note the controller state. Then plug a USB Bluetooth dongle, pair one device (this proves the waiver fallback). | D S | |
| [ ] | USB-C link and internet sharing | untested | Cable between this computer and a second one: direct link appears (Settings, Network), iyakki dev pair over it, then internet sharing from the other side. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s (needs internet); kubectl get nodes shows Ready; reboot: it starts again. | D | |
| [ ] | GPU for k3s | partly | Store, Setup, GPU card, Open its page, Install, Run test prompt (docs/GPU-K3S.md section 8). Send the job log whatever happens. | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a simple page, a modern site, a YouTube video (F for the full engine); one long scroll. | S | |
| [ ] | Browser, hardware drawing | untested | Same pages; note which renderer the engine uses (GPU page, dmesg) and the dropped-frame reading (docs/BROWSER-SPEED.md). | L | |
| [ ] | Video decode | untested | YouTube 720p and 1080p: smooth or not; note dropped frames. VP9 only (H.264 needs the add-on). | note | |
| [ ] | Battery and power source | untested | ls /sys/class/power_supply (a BAT entry); Settings, System, Battery; unplug the adapter, the percentage falls and the top bar shows it. Do not drain to empty. | S | |
| [ ] | Sleep (idle, lid) | untested | Not built (F-PWR-01 planned). Nothing to test; write 'planned' on the sheet. After it is built: sleep after 1 minute, wake by key. | note | |
| [ ] | Suspend to RAM | untested | Not built. After F-PWR-01: suspend and resume once; note resume time and what broke. T2 Macs: experimental. | note | |
| [ ] | Shutdown | untested | Power off from the menu: screen goes dark, fans stop, no panic lines; next boot shows no 'unexpected shutdown' note. | D | |
| [ ] | Developer mode pairing | untested | Settings, System, Developer mode: e, then p; on your computer iyakki dev pair ADDRESS; iyakki dev whoami; G; iyakki dev diag -o FILE. Do it once with the firewall ON. | D | |
| [ ] | Firewall | untested | Settings, Firewall ON: from your computer reach 7979 with developer mode on (must work) and a closed port (must not); the blocked counter rises. | D | |

Rows left out are `not supported` on this machine (1). Send back: the `hwreport`, the `diag` bundle, the screenshots and any `dmesg` lines asked for.

<div style="page-break-before: always"></div>

### Checklist J. Apple Silicon, VM path only (short)

Owner decision 2026-10-27: lightweight VM + k3s + model bridge. Bare metal is not promised. Recipe: [../APPLE-SILICON.md](../APPLE-SILICON.md) section 2 and [../MACOS.md](../MACOS.md).

| Done | Capability | Now | Step | Evid. | Result |
|---|---|---|---|---|---|
| [ ] | Upgrade from previous release | untested | Replace the image in the VM with the next one; the data disk is kept. | D | |
| [ ] | Display, internal | untested | VM window shows the sign-in screen at the window size. | H S | |
| [ ] | Keyboard (and pointer) | untested | Type and click in the VM window. | H | |
| [ ] | Sound out | untested | Test sound inside the VM; note the host audio device. | D | |
| [ ] | Ethernet | untested | The VM's virtual network: top bar says connected. | D | |
| [ ] | k3s start | untested | Store, Setup, Quick install, k3s; kubectl get nodes Ready. | D | |
| [ ] | GPU for k3s | untested | Model bridge: the VM reaches a model runner on the Mac (docs/MAC-MODEL-HOST.md); one answer comes back. | D L | |
| [ ] | Browser, software drawing | untested | BROWSER tile: a page and a YouTube video; note dropped frames. | S | |
| [ ] | Video decode | untested | YouTube 720p in the VM; note dropped frames. | note | |
| [ ] | Shutdown | untested | Shut the VM down from IYAKKI; the data disk is clean on the next start. | D | |
| [ ] | Developer mode pairing | untested | Developer mode pairing from the Mac host to the VM. | D | |
| [ ] | Firewall | untested | Firewall ON: developer mode reachable, a closed port blocked. | D | |

### Checklists H and I. QEMU

Scripts, not a sheet: `dist/buildroot/qemu-test.sh`, `qemu-install-test.sh`, `qemu-persist-test.sh`, `qemu-uefi-aarch64-test.sh`, `secureboot-testkit.sh` (see [../BOOT-TEST.md](../BOOT-TEST.md)). Record the date and the pass line in section 4 of this page. K, L and M: nothing to test.

## 8. Where to write results and how to collect evidence

1. **Collect** (from the second computer, developer mode on and paired, scopes granted with `G`; [../DEVELOPER-MODE.md](../DEVELOPER-MODE.md)): `iyakki dev diag -o NAME-DATE-diag.txt` (version, uptime, memory, disks, network, sound, services, cluster, recent errors, audit tail; no secrets), `iyakki dev screenshot -o NAME-DATE-WHAT.png`, `iyakki dev state`. On the device: `iyakki hwreport`, and `dmesg | grep -i -E "nouveau|nvrm|firmware|gsp|i915|amdgpu|brcm|bluetooth"` for graphics, Wi-Fi or Bluetooth rows.
2. **Write** the result on the printed sheet, then tell an agent: "machine X, capability Y, PASS, date, files". The agent changes the cell in section 2 and section 4 of this page, then [../HARDWARE.md](../HARDWARE.md) and [../../KNOWN-ISSUES.md](../../KNOWN-ISSUES.md) from it, and the FEATURES.md note of the row, in one pass.
3. **Keep** the files together under one folder of your choice named after the machine and date (the repository has no such folder yet; do not commit images or logs that hold addresses or serials without looking). Cite the file name in the evidence column. A cell without a file name stays `owner report`.
4. After the gate run, the table in section 3 must show no `untested` for A to E on a capability that section 5.1 marks as blocking, or a written waiver for it.

## 9. What this page could not verify

* Nothing was run: no machine, no drive, no build, no test of the product. The coordinator's observations of 2026-10-26/27 on machine A are taken as told; no `diag` file of that session is named in the repository.
* The dates of the Mac mini 2018 results are the span of ALPHA-GATE items 46 to 54. The MacBook's USB-boot date (2026-10-23) is the day the `MAC-HARDWARE.md` measurement was made; the owner's own report has no day.
* Whether the earlier "AMD PC" and the AMD + NVIDIA PC of tonight are the same machine; which image booted on it; what worked on it.
* Whether "the older Mac mini" in the gate text means the 2014 mini (decision 1).
* The QEMU dates for install, display and aarch64 boot (`date not recorded`): the scripts exist and HARDWARE.md says they pass; no run log is cited here.
* Which exact built fixes are in which image: 'built, unverified' comes from the coordinator and is not checked against an image.
* Whether the 13-inch MacBookPro16,2 has a usable Vulkan path on its Intel GPU, whether any gate PC has a Wi-Fi chip, a battery or a touch screen.

