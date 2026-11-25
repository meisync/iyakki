# Plan: IYAKKI boot menu on the Kria KV260 and Raspberry Pi 5, and robotics tools (CAN FD, GPIO, stereo camera)

**For:** the owner and the model that builds this next.
**You will get:** what we build, in what order, what each step needs, and how each part is reached from the chat assistant.
Date: 2026-11-24. Status: plan only. Decisions are marked D-ARM-nn and also go into `ALPHA-8-DECISIONS.md` when the work starts.

Out of scope for now (owner, 2026-11-24): Apple Silicon Macs through m1n1 (see `APPLE-SILICON-VM-PLAN.md` for the VM route that exists).

## Part A: the same boot experience on the two ARM boards

**Correction (2026-11-24, found after the first draft):** `iyakki-boot` already has most of this. `board-cfg` generates the KV260's own `extlinux` menu (the "skip UEFI" route: the board's U-Boot reads it) and the Pi 5 files; the loader builds for `aarch64-unknown-uefi` and boots in QEMU (`qemu-test.sh --arch aarch64`); `make-arm-variant.sh` makes a loader card from a baseline card, and the Pi loader variant starts U-Boot (`kernel=u-boot.bin`) which starts the loader. What its README says is **not proven** is U-Boot's EFI path on the real boards. So the work is proof and the pieces the first boots showed were missing, not new loaders.

| Step | What | Needs | Proof |
|---|---|---|---|
| A1 | **KV260 baseline menu on the real board** (the extlinux entries already generated): normal, SSH diagnostics, recovery, display-tree choice; plus the permanent card fixes (empty data partition, both device trees). | KI-1036..1040 fixes. | Card booted on the real board (done by hand on 2026-11-24 with the card tree; the generated menu is not yet the one that booted). |
| A2 | **Pi 5 baseline lines from the table**: the `-pi5` display overlay and the memory-cgroup words now come from `board-cfg` (branch `alpha.10/ARM-LINES` in iyakki-boot, golden files updated); the D0 device-tree name is still open (the table names the C0 file). | Push of that branch, then the pointer bump in the main repo, then `iyakki-usb` reads the table (branch `arm/pi5-generator-lines`). | Next Pi card boots k3s and the display with no hand edits. |
| A3 | **Loader on the real Pi 5 through U-Boot** (`kernel=u-boot.bin`, then the loader), the variant `make-arm-variant.sh` makes. | `u-boot.bin` by the person's click or an in-house build (GPL: not shipped in the image, D-ARM-01). | Real Pi 5, serial and screen. |
| A4 | **Loader on the real KV260** through the board's U-Boot EFI path (a card without `extlinux.conf`). | A3 results. | Real KV260. |

Decisions (recommended, owner may overrule):
* **D-ARM-01** The U-Boot for the Pi 5 is not part of the image: the person gets it by a click (or builds it), because it is GPL (the permissive-licence rule). The baseline Pi card needs no U-Boot at all.
* **D-ARM-02** The KV260 keeps its own U-Boot; we only write what it reads.
* **D-ARM-03** The first proof of any loader change is QEMU, then the board; boot-test before pushing to a card.

**Universal card (F-BOOT-47, owner question 2026-11-24):** one card can carry both `extlinux/extlinux.conf` and `EFI/BOOT/BOOT*.EFI`. UEFI firmware ignores extlinux and starts the loader; a U-Boot board normally tries extlinux first, so the loader starts there only if extlinux is absent or a `boot.scr`/U-Boot setting chooses it. First step: read the real boot order on the KV260 and the Pi 5.

**One aarch64 image (F-BOOT-48):** the userland root is the same on every ARM board; the kernel, modules, device trees and command line differ. One image can carry a shared root and one set per board, chosen by the loader's board table (cost: about one extra kernel and module set per board). x86_64 stays its own image; a cross-architecture fat image is not chosen (it doubles the size and breaks the one-x86_64-image decision).

## Part B: robotics tools, each reachable from the chat assistant

Every tool follows the rule *everything reachable by chat*: a chat tool per switch, reading is free, changing the machine asks, and everything goes through the core with the audit log (`project-everything-through-core-rule`). Nothing here touches Linux directly from the app layer.

### B1 CAN and CAN FD (CANivore and other adapters)
* The adapter appears as a SocketCAN interface (`can0`). **To verify on real hardware:** that the CANivore's USB firmware shows up through an in-tree kernel driver (I believe a `gs_usb`-style one, not confirmed) and which kernel options the images need; if not, the work stops there and is parked.
* Core facet `CanFacet` with a fake: list interfaces, set bitrate and the CAN FD data bitrate, bring up and down, send a frame, receive and log frames, filter by id.
* Tool page "CAN": interface list, a live frame view, a send form, a log that can be saved as a file. Opening an interface and sending frames ask (a frame can move a motor).
* Chat tools: `can_interfaces` (free), `can_read` (free, bounded), `can_configure` (asks), `can_send` (asks, names the id and bytes).

### B2 GPIO configuration, with a quick install
* Core facet `GpioFacet` over the kernel's character device (`/dev/gpiochipN`): list chips and lines, read a line, set direction, set output, set pull and edge events where the board supports it. The Pi 5 (RP1) and the KV260 (PMOD, header) have different line names, so each board gets a name table.
* **Digital on/off:** read a pin, drive it high or low, set pull-up or pull-down, and watch for edges.
* **Analog read:** neither board has analog inputs on its header. Analog goes through an external ADC chip on I2C or SPI that the kernel exposes as an IIO device (`/dev/iio:deviceN`); the page lists any ADC it finds and shows volts. (The KV260 chip's own system monitor is not on the header.)
* **PWM:** the Pi 5 has hardware PWM through an overlay line (`dtoverlay=pwm-...`, exposed under `/sys/class/pwm`); the KV260 has no PWM on its PS pins, so PWM there needs a design in the programmable logic or an external PWM chip, and the page says so instead of offering a dead control. Frequency and duty cycle are set with an approval and a safe limit.
* Quick install: one button, "Prepare this board for GPIO", that turns on what is missing (kernel module or device-tree overlay lines in `config-user.txt` / the extlinux entry), then asks to restart. It writes the lines the same way the boot menu does (A1/A3), so it needs those first.
* Chat tools: `gpio_list` (free), `gpio_read` (free), `gpio_set` (asks, names the pin and level), `gpio_analog_read` (free), `gpio_pwm` (asks, names pin, frequency and duty), `gpio_prepare` (asks).
* Safety: a line is shown as "in use" if another program holds it; an output pin is never set without the person's approval; the page shows the board's voltage warning.

### B3 Stereo camera tools
* Builds on the camera facet already in the core (V4L2; the Pi webcam case is KI-1040). Adds: two cameras (or one side-by-side stereo device) shown together, a capture that saves a left/right pair with a timestamp, and a calibration helper (chessboard capture, then a saved file).
* Depth and rectification run as an isolated k3s job like the other data-science tools (`project-datasci-toolkit-on-k3s`), not in the shell; a frame is handed over by a flow.
* Chat tools: `stereo_devices` (free), `stereo_capture` (asks: it uses the cameras), `stereo_calibrate` (asks), `stereo_depth_job` (asks, starts the isolated job).
* Evidence: the fake camera (`fakecam.py`) gives two sources for automated tests; real stereo hardware is a Manual item.

## Order and rows
1. A1 and A2 (card fixes, then the table lines reach the generators).
2. B2 GPIO (smallest, board-specific tables), then B1 CAN.
3. A3 (the loader on the real Pi 5), then A4.
4. B3 stereo tools.

FEATURES rows: `F-BOOT-43..46` (A1..A4) and `F-CAN-01`, `F-GPIO-01`, `F-CAM-22`. All start `planned` 0/0 (no partial ceiling change). Each gets a chat-tool note.

## Parked: KV260 vision acceleration (F-KV260-05)
The KV260 GPU (Mali-400 MP2) is graphics only (no compute), so inference would use the DPU in the programmable logic. Its design and firmware files come from the vendor by the person's click, are loaded at run time and are never shipped. Models must be compiled for it; they run as an isolated k3s job. Parked until the boot menu (F-BOOT-43) and the camera tools (F-CAM-22) exist.
