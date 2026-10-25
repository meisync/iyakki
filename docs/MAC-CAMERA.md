# The built-in camera of a MacBook (FaceTime HD)

Status: written and tested against fakes and against Apple's real bytes for the unpacking; **nothing has run on a real
MacBook yet**. Read [section 5 of MAC-HARDWARE.md](MAC-HARDWARE.md#5-macbook-with-a-facetime-hd-camera) for the background.

## What was measured

On a MacBookPro14,1 (13-inch 2017, no T2 chip) the camera is the PCI device `0000:03:00.0`, vendor `14e4`, device `1570`,
class `0x0480` ("multimedia controller", Broadcom 720p FaceTime HD). No driver was bound, there was no `/dev/video*` and no
camera module was loaded: the kernel has no driver for it.

## Why the driver is in the image (and the install is one click)

The device has no compiler, so "install the driver" cannot mean building it there. The driver is GPL-2.0 open source, and
shipping a GPL module inside the image is allowed as long as we offer its source: the pinned commit, its hash and our
build recipe are in this repository (see LICENSING.md, item 17), the same as for the kernel and the cs8409 sound module. So
the module ships in the Mac image, and only Apple's firmware, which we may not redistribute, is downloaded. The card
"Install camera driver and firmware" therefore does two things in order: it downloads and checks Apple's firmware on this
machine, then loads the module (`modprobe`, no restart) and waits for `/dev/video*`. A separate downloadable module pack was
considered (signed, hashed, fetched from a release URL); it would only add a hosting dependency, so it is not built. If it
is ever needed it fits the same card: fetch the pack, check the hash, then the same load step.

To get the module into an image without a kernel rebuild, run `dist/buildroot/build-facetimehd.sh` once (it fetches the
pinned tarball into `dl/facetimehd`, checks its sha256, builds `facetimehd.ko` and `videobuf2-dma-sg.ko` against
`target/buildroot-mac/build/linux-*` with that build's compiler and copies them to
`target/buildroot-mac/target/lib/modules/<ver>/updates/`). `./run.sh build-macweb-layered` then adds both files to the
rootfs and runs depmod (`repack.sh`, list `OOT_MODULES`). `dist/buildroot/build-facetimehd.sh --check` verifies the built
files (vermagic, PCI alias, dependencies).

## How it works

1. **Driver.** The Mac image builds `facetimehd` (patjak/facetimehd, GPL-2.0, out of tree, pinned commit
   `6dc8cd85e8`, master of 2026-10-14) with the kernel (package `dist/buildroot/package/facetimehd`). It compiled without a
   patch against the 6.12.112 kernel tree. It needs `videobuf2-dma-sg`, which the IYAKKI kernel does not build (the option
   has no prompt and only PCI media drivers select it), so the package also builds the kernel's own `videobuf2-dma-sg.c` as
   a module: **no kernel rebuild is needed**. The module has the alias `pci:v000014E4d00001570*`, so it loads only on this
   camera. The kernel options it needs (`VIDEO_DEV`, `VIDEOBUF2_V4L2`, `VIDEOBUF2_MEMOPS`, `MEDIA_SUPPORT`) are set already.
2. **Firmware (Apple's, never shipped).** The camera runs a small processor program that Apple ships inside macOS. The
   job "Camera firmware" fetches it from Apple's update server on this machine: three byte ranges (17.6 MB) of the macOS
   10.12.6 combo update (the same addresses as facetimehd-firmware's Makefile), decodes them, assembles
   `AppleCameraInterface`, unpacks `facetimehd/firmware.bin` (version 5.60.0, 1,417,220 bytes, sha256
   `240ef2e991f1d089d8228ce11d92b66bfa4b3d7289ec4fee228b64a713024330`) and cuts the eleven sensor files
   `facetimehd/<id>_01XX.dat` out of `AppleCameraAssistant`. Every piece, the driver file, the firmware and each sensor
   file are checked against sha256 values that were verified against Apple's real bytes on 2026-10-22. Only `https` addresses
   on `cdn-apple.com` and `apple.com` are used; a server that answers the whole 1.9 GB image instead of the range is refused.
3. **Install.** The files become the driver pack `mac-facetimehd` on the data volume, linked into `/lib/firmware` at every
   start; the driver is loaded again and the job looks for the camera device.
4. **Use.** The CAMERA tile finds the driver's video node like any other: capture with streaming, card name
   "Apple Facetime HD". The driver offers **YUYV only** (its YVYU and NV16 are switched off upstream), sizes in steps of 8
   up to the sensor (1280x720), 30 frames per second (the driver's only frame interval). The tile opens 640x480 YUYV by default and offers the larger sizes.

## What is missing or worse than a webcam

* **Sensor calibration.** The `*_01XX.dat` files hold the factory calibration of each sensor model (colour, lens shading).
  They are optional for the driver: it asks for the one that fits the sensor it finds and, without it, starts without
  calibration. The picture is then expected to have wrong colours or shading. The job installs all eleven, so this should
  not apply; which one the camera uses is in the kernel log.
* **Quality.** The driver is community work, experimental: exposure and white balance are the ISP firmware's work. The
  driver registers a few V4L2 controls (brightness and contrast among them, from its source); whether the CAMERA tile's
  sliders work with them is unverified.
* **Microphone.** The camera's microphone is not part of this; the microphone of the MacBookPro14,1 does not work yet
  (see MAC-HARDWARE.md, sound).
* The 12-inch MacBook (848x588 sensor) is covered by the same firmware (5.60.0) but was never tried.

## Scanning a QR code with the camera (the Claude Code sign-in)

Besides the CAMERA tile, the camera has one more user: the Claude Code sign-in dialog. Its **Scan with camera** button (Ctrl+K)
reads a QR code that the person holds up, shown on their phone, and fills the sign-in code into the dialog's field.

* **Asked first, visibly on.** The camera is opened only after the person says yes to a card that says what it is for. While it
  runs, the card shows a blinking **CAMERA IS ON** mark, a live picture (from the same capture path as the tile: first camera,
  default mode, 640x480 YUYV on the FaceTime HD) with a frame to hold the code in, and the seconds left.
* **Stopped fast.** The camera is closed when a code is read, on Esc or **Stop camera**, when the dialog closes or hides, when
  the sign-in ends, when the camera stops sending, and after 60 seconds. No camera, a camera in use (the CAMERA tile is open)
  and a refused open are each reported in plain words and leave the camera off.
* **Frames never stored.** Frames are decoded in memory by our own reader (`iyakki_agent::qr_decode`: versions 1 to 10, levels
  L/M/Q/H with Reed-Solomon repair, numeric, alphanumeric and byte modes, rotation, moderate tilt, uneven light, and the mirror
  image tried too), at most about every 120 ms, and dropped. Nothing is written to disk, sent anywhere or put in the audit
  log; the scanned text is never repeated in a message.
* **Only a code goes in.** The text must have the shape of a sign-in code (no spaces, the characters a code has, at least 20
  of them, and Claude's `#`). A web address, even one with a code in it, is refused: it is never put in the field and never
  opened. Nothing is sent until the person presses Enter.

### The clipboard's Scan QR

The clipboard history (Ctrl+Shift+V) has a **Scan QR** button (Ctrl+K) that uses the same camera code (`scan_core.rs`: the same
question first, **CAMERA IS ON** card, frame guide, stops after a read, Esc, closing or 60 seconds). Any text is allowed (a secret,
an address, a key; at most 64 KB, far more than a QR code holds). It is shown before it is copied, and nothing is opened. The
audit records only "clipboard scan: n characters, secret yes/no".

Not verified on a real MacBook: the FaceTime HD's picture is soft and its exposure is the firmware's; a phone screen at an angle
or with glare can fail to read. If it does not read, move the phone a little farther away (about 20 cm) or turn its brightness up;
pasting the code always works.

## Real-device checklist (MacBookPro14,1, over SSH)

Needs an image repacked after `dist/buildroot/build-facetimehd.sh` (or built with `BR2_PACKAGE_FACETIMEHD=y`).

1. Check the hardware and that nothing is bound: `lspci -nn | grep -i 14e4:1570` and `ls /dev/video* ; ls /sys/class/video4linux`.
2. Check the modules are in the image: `modinfo facetimehd | grep -E 'alias|depends|vermagic'` (alias `pci:v000014E4d00001570*`,
   depends `videobuf2-v4l2,videobuf2-common,videobuf2-dma-sg`) and `modinfo videobuf2_dma_sg | grep vermagic`.
3. Run the job as root: `iyakki quick-install mac-camera --data /var/lib/iyakki`. It prints five numbered steps and ends with
   `done: ...`. Send back the whole output.
4. Check the files: `ls -l /var/lib/iyakki/firmware/mac-facetimehd/files/facetimehd/ /lib/firmware/facetimehd/` and
   `sha256sum /lib/firmware/facetimehd/firmware.bin` (expect `240ef2e9...4330`).
5. Send back `dmesg | grep -iE 'facetimehd|fthd|videobuf'` (the driver prints the firmware it loaded, the sensor it found and
   "sensor count"; a line saying the set file is missing means the calibration did not load).
6. `ls -l /dev/video*` and `cat /sys/class/video4linux/video*/name` (expect "Apple Facetime HD").
7. If the driver loaded but no device appeared, reboot once (the firmware is linked at every start) and repeat 5 and 6.
8. Formats: `v4l2-ctl -d /dev/video0 --list-formats-ext` if `v4l2-utils` is there (expect YUYV only), otherwise say so.
9. Open the CAMERA tile: the tile should be enabled, the preview should show a picture at 640x480, and the Devices popover
   should list "Apple Facetime HD" as OK. Report the colours, the frame rate and anything that crashes.
10. Before the job ran, the tile should have been greyed with the hint about Camera firmware and the Devices row a warning.
11. To start over: delete the pack in STORE, Drivers, or `rm -r /var/lib/iyakki/firmware/mac-facetimehd` and restart.

## Where it stands

**Store -> Setup -> Hardware** has the card "MacBook FaceTime camera" (`Item::MacCamera`). It is shown only when
`camfw::camera_in("/sys")` finds the 14e4:1570 camera and never in cloud mode. Its one button is "Install camera driver and firmware"
(Retry after a failure, "Open camera" when done); while the job runs the card has a progress bar and the button opens the
log. Details say what it is, the licence (driver GPL-2.0; Apple's firmware under Apple's licence, fetched from Apple's CDN on
your click and never shipped) and the size (about 17.6 MB). The job's result line (`camfw::read_outcome`) is shown on the
card, and "Delete download" removes the kept pieces (`camfw::delete_leftovers`). Only administrators can install or delete.
The CLI is unchanged: `iyakki quick-install mac-camera --data /var/lib/iyakki`. The Mac drivers page (IYAKKI menu) names the
card in its USB webcam row. Tested with fakes only (a stand-in program starts the job, made-up result files); nothing ran
on a MacBook. The card starts the job with `iyakki_agent::quickinstall::start_camera_firmware` (same "already running" guard
as the Wi-Fi job).
