# Cloud mode

A machine-wide mode for machines with no local screen, speakers or camera: servers, cloud VMs, headless boxes, computers used only
through a remote desktop. It hides what needs local hardware and keeps everything that makes sense remotely. Code:
`crates/iyakki-agent/src/cloudmode.rs` (the decision and the tables), used by the shell (`shell.rs`, `shell_fnkeys.rs`,
`devpop.rs`, `brandmenu.rs`, `about.rs`, `apps/settings*.rs`).

## How the mode is chosen

| Setting (Settings, Cloud mode; administrators) | Result |
|---|---|
| **Automatic** (default) | On when the machine has no display (no DRM card, no framebuffer), no keyboard or pointer (a power button does not count), no sound card, or when the image is a cloud variant (`VARIANT=` in `/etc/iyakki-release` contains `cloud`). Otherwise off. |
| **On** | On, and everything that needs hardware is hidden whether the hardware exists or not. |
| **Off** | Off: nothing is hidden. |

The boot parameter `iyakki.mode=cloud` (same as **On**) or `iyakki.mode=desktop` (same as **Off**) beats the setting. No GRUB entry exists
for it yet; add the parameter to the kernel command line. Settings, Cloud mode and About (IYAKKI menu) say which mode is in force and why
("Automatic: this machine has no screen, no sound card.").

In Automatic, a thing is hidden only if its own hardware is missing: a headless machine with a sound card keeps RADIO and MEDIA; one with
a USB camera keeps CAMERA. **On** hides them all.

## What is hidden

One table in code (`cloudmode.rs`); a test fails when a new app has no class.

| Hidden when the hardware is missing (or always, with **On**) | Needs |
|---|---|
| Tiles MEDIA, RADIO, TALES (reads aloud), VOICE (reads aloud; dictation also needs a microphone, which the tile checks itself) | a sound output |
| Tile CAMERA | a camera |
| Settings pages Sound, System sounds, Display, Keyboard (Fn keys), Bluetooth | sound, a display, a keyboard or pointer, a Bluetooth adapter |
| Devices popover rows Sound, Display, Camera, Bluetooth, Keyboard and pointer, Battery | the same hardware (Battery: a battery) |
| IYAKKI menu entries Media player, Mac drivers | sound; Apple hardware or the Mac image |
| Volume, brightness, keyboard-light and media keys, and their on-screen bar (the power and sleep keys still work) | a keyboard or pointer |

Kept, because they work over a remote desktop: CHAT, RAG, MCP, CLI, SKILLS, CALENDAR, AI IDE, EDIT, DATA, DASH, FLOWS, RUNS, SRE PILOT,
SYSTEM, STORE, K8S (cluster tools need nothing local), USERS, USAGE, NET SCAN, MESH, BROWSER (the web engine needs no screen) and SETTINGS (without its hardware pages). Network
(wired settings) stays. Printers: IYAKKI has no printer page yet, so nothing is hidden for them.

Hidden is not just invisible: the number keys, the Enter on a tile, jumps from other apps and the letter keys of the IYAKKI menu do
nothing for a hidden app or entry. The number keys 1 to 9 open the Nth tile that is *shown*, so in cloud mode the numbers shift (with
MEDIA, RADIO, TALES, VOICE and CAMERA gone, the later tiles move up).

Services that stay off in cloud mode when their hardware is missing: the sound output probe and the soft-volume setup (not started at
boot), and the once-a-second camera look the CAMERA tile does. The mode is decided at boot for services and again at each sign-in for
the menus; changing the setting shows in the menus at the next sign-in and in the services at the next start.

The person's own layout (groups, moved tiles) is unaffected: a hidden tile moved to another group stays hidden, and a built-in group
with nothing left to show disappears as it always did. Settings, Home layout still lists every tile (known gap).

## With the image variants and the tier plan

Machines that run unattended (tiers 0 to 4 of the plan) have no one at a screen; the `web` images are the usual way to use such a
machine: open its address in a browser (the remote desktop, `iyakki-remote`) and work in the same shell, with cloud mode hiding the
tiles that would do nothing there. A future cloud image variant only needs `VARIANT=cloud-…` in its release file to turn the mode on in
Automatic. The local-hardware images (`libre`, `hw`, `mac`, `macweb`, `aarch64`) have a screen, input and sound, so they stay in desktop
mode unless asked.

## What is not tested

Everything is tested with made-up `/sys` trees and the shell's own render; no real server or cloud VM was booted. How a given cloud
hypervisor presents a virtual display or keyboard (a VM with a virtual GPU has a framebuffer, so Automatic reads it as a screen unless the
image variant says cloud) is to be confirmed.
