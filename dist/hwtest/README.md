# Hardware checks (dist/hwtest)

`hwcheck.py` drives a real IYAKKI device only through `iyakki dev` (state, screenshot, input, logs, dmesg, shell with an approval card, info, usb) and
writes honest verdicts: PASS (the condition was observed), FAIL (the row promises it and it is not there), NEEDS-HUMAN (a person must judge).
Evidence goes to `~/iyakki-work/hwtest/<date>/[<device>/]<ID>/`, outside the repo. Usage: see the top of `hwcheck.py`
(`list`, `run ID...`, `run all`, `explore`, `--device NAME`). Recipe ids are global.

| Files | What |
|---|---|
| `hwcheck.py` | the runner, the screen helpers and the safety rules (Esc and q refused on Home, text only into a focused field, no reboot or install) |
| `recipes.py` to `recipes6.py` | earlier recipes (one per row; some ids carry a suffix such as `-live` or `-mm14`) |
| `recipes_m14.py`, `recipes_m14_hw.py` | batch M14: 31 recipes, ids end in `-m14`; all read-only, none types a secret or changes a setting |
| `recipes_m14_parse.py` | pure parsers of command output used by batch M14 (no device, no hwcheck import) |
| `test_m14.py` | unit tests of the batch's logic with canned `iyakki dev` output; no device needed |

Run the tests: `python3 -I -m unittest discover -s dist/hwtest -p test_m14.py`.

A recipe written for specific hardware (Mac mini 2014 / 2018, an Apple Mac, a touch screen, Broadcom Wi-Fi, an NVIDIA-free PC) ends NOT-APPLICABLE (skipped, with the reason; `hwmatch.py`, `require_hardware`) on a device that does not match, never FAIL. Tests: `python3 -I -m unittest discover -s dist/hwtest -p test_hwmatch.py`.

A recipe that cannot tell (a format it did not recognise, a machine without the hardware) ends NEEDS-HUMAN, not FAIL.
A PASS always lists what it did NOT cover; do not flip a whole FEATURES.md row on a narrow observation. Run one device at a time.

## Batch M14

"Row" is the FEATURES.md row the recipe gives evidence for; PASS and FAIL conditions are in each recipe's docstring.

| Recipe | Row | Check |
|---|---|---|
| `F-BATT-04-m14` | F-BATT-04 | Settings -> Battery on a laptop shows the charge and the three switches (read-only; a desktop is NEEDS-HUMAN) |
| `F-BATT-05-m14` | F-BATT-05 | The running kernel has the battery options (ACPI_BATTERY, ACPI_AC, POWER_SUPPLY, ACPI_SBS, PM, ACPI_BUTTON, ACPI_VIDEO) (read-only) |
| `F-BOOT-06-m14` | F-BOOT-06 | Touch screens: a multi-touch-only panel is present (read-only; the finger test is NEEDS-HUMAN) |
| `F-CAM-06-m14` | F-CAM-06 | CAMERA tile is enabled exactly when the kernel has a video device (reads the tile's caption; does not open the camera) |
| `F-CLI-07-m14` | F-CLI-07 | CLI tile -> Connections tab opens (read-only; no host is added or connected) |
| `F-CLIENT-08-m14` | F-CLIENT-08 | Settings -> Remote access shows the Connect-a-client state (EXPERIMENTAL; off by default; read-only) |
| `F-CLOUD-01-m14` | F-CLOUD-01 | Cloud mode is off on a normal machine: no iyakki.mode=cloud on the kernel line and the Cloud page opens (read-only) |
| `F-CLOUD-20-m14` | F-CLOUD-20 | Settings -> Cloud access opens for an administrator (read-only) |
| `F-DEV-10-m14` | F-DEV-10 | `iyakki dev usb` prints its numbered check list (read-only; no authorize, pair or gadget change) |
| `F-DEVT-05-m14` | F-DEVT-05 | DEV TOOLS tile has the QR code tool and shows its description (read-only; no code is made) |
| `F-DISP-07-m14` | F-DISP-07 | Touch screens on several displays: a touch screen exists and the kernel log has its 'touch: ... -> <display>' mapping (read-only) |
| `F-DOMAIN-02-m14` | F-DOMAIN-02 | Domain listener: nothing listens on ports 80 or 443 on a non-loopback address unless approved (read-only) |
| `F-DOMAIN-06-m14` | F-DOMAIN-06 | Settings -> Network -> Domains opens with its blurb and its route-table buttons (read-only) |
| `F-DRIVERS-27-m14` | F-DRIVERS-27 | Macs with the Cirrus CS8409 codec: codec bound to the CS8409 driver and the Sound page not asking for it (read-only) |
| `F-K8S-18-m14` | F-K8S-18 | Settings -> Clusters has the Platform services button (read-only; not pressed) |
| `F-KBD-03-m14` | F-KBD-03 | Brightness and keyboard-light sources: which backlight the kernel registered and which the shell would rank first (read-only) |
| `F-LDR-09-m14` | F-LDR-09 | Boot log on the boot volume: EFI/iyakki-boot/last-boot.log exists and has lines (read-only; NEEDS-HUMAN if the boot volume is not mounted) |
| `F-MEDIA-07-m14` | F-MEDIA-07 | Sample media is on the image: a melody, a beat (mp3) and a sample WebM video (read-only) |
| `F-NET-06-m14` | F-NET-06 | Settings -> Network agrees with the adapters the kernel has: Ethernet and Wi-Fi rows, a missing one 'not found', a driverless Broadcom card named (read-only) |
| `F-NET-08-m14` | F-NET-08 | Settings -> Bluetooth agrees with the kernel: adapter present, or 'No Bluetooth adapter' (read-only) |
| `F-NET-11-m14` | F-NET-11 | Mac mini 2018 Bluetooth: BCM4364 on a UART, does hci0 get an address? (read-only; documented limit is FAIL) |
| `F-PWR-01-m14` | F-PWR-01 | Settings -> Power offers the three power profiles and the sleep-after-idle line (read-only) |
| `F-REL-01-m14` | F-REL-01 | Version everywhere: `dev info` and the About page of the IYAKKI menu name the same version (read-only) |
| `F-SHARE-01-m14` | F-SHARE-01 | Settings -> Shared folders opens; the listening sockets are logged as evidence (read-only) |
| `F-SND-02-m14` | F-SND-02 | Settings -> Sound lists where sound can play; the kernel has a playback device behind it (read-only) |
| `F-SND-04-m14` | F-SND-04 | Settings -> Sound shows a volume level and the mute state (read-only) |
| `F-SND-05-m14` | F-SND-05 | T2 Mac sound: Apple T2 Audio card exists and aaudio logged (read-only) |
| `F-SND-20-m14` | F-SND-20 | Built-in (T2) microphone switch is offered on a T2 Mac and only there (read-only) |
| `F-VOICE-17-m14` | F-VOICE-17 | VOICE tile opens with its READ ALOUD, DICTATE and RECORD tabs; nothing is spoken or recorded (read-only) |
| `F-VOICE-19-m14` | F-VOICE-19 | Settings -> System sounds opens (read-only; no cue is played) |
| `F-VPN-07-m14` | F-VPN-07 | Settings -> VPN opens with its import choices and no tunnel interface is up by default (read-only) |
