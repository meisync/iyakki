# Touch Bar (MacBook Pro 2016 to 2020)

Status: **written and tested with a fake device only. Nothing of the strip has run on a real Touch Bar yet.** The function-key
mode (Esc, F-keys or media keys drawn by the T2 chip) is separate, was measured on a MacBookPro16,2 and keeps working whatever
the strip does.

Contents: 1 What works, 2 What does not, 3 The two modes, 4 The strips, 5 Settings, 6 How the display is driven (and where the
protocol comes from), 7 The safe bring-up, 8 Never blocks, never hides the keys, 9 Troubleshooting, 10 Device test plan,
11 Licences.

## 1. What works

* **Keys (measured on a MacBookPro16,2):** after the USB retry the Touch Bar shows Esc and F-keys or media keys (drawn by the T2),
  and the keys send ordinary evdev codes. IYAKKI already acts on them: volume up, down and mute change the master volume with the
  bar at the bottom of the screen, brightness up and down change the panel (with the lowest step above black), play/pause, next and
  previous drive the radio and the media player. The Fn key switches between the two layers while held.
* **Strip (new, fakes only):** IYAKKI draws its own 2170 x 60 picture on the Touch Bar: volume and brightness sliders, the clock, the
  recent apps, the agent's state, shortcuts of the focused app. See section 4.

## 2. What does not

* **Touching the strip probably does nothing yet.** The touch panel of the bar reaches Linux through `hid-multitouch`, which learned the
  Touch Bar only after kernel 6.12 (the Apple Touch Bar class appears in 6.18, not in 6.12). Without it the strip is **display only**:
  it shows the volume, brightness, clock, recent apps and agent state, but its buttons and sliders cannot be pressed. Settings ->
  Keyboard says so. If the kernel does show a touch node named like the bar (`Touch Bar`), IYAKKI reads it and keeps it away from the
  mouse pointer of the main screen. Carrying the 6.18 patch would be a kernel change and an image build.
* **Touch ID** is impossible (it needs the Secure Enclave) and out of scope.
* **Holding Fn to cycle strips** is not wired (the keyboard driver consumes Fn); the chip next to Esc cycles them.
* **IDE Test and Deploy buttons** are not offered: the IDE has no keyboard shortcut for them. Run (F5) and Save (Ctrl+S) are.
* **Browser items** (back, forward, reload) send the browser's own keys (b, f, r), which work in its built-in reader view.
* There is no screen-blank or screen-lock feature in the shell yet, so "off with the screen" is: dark after five minutes without any
  key, click or touch, and on again at the next one; before sign-in only the name IYAKKI is drawn.

## 3. The two modes

`hid-appletb-kbd` has a `mode` per Touch Bar device (`/sys/bus/hid/drivers/hid-appletb-kbd/*/mode`):

| Mode | What the T2 draws |
|---|---|
| 0 | Esc only |
| 1 | F1 to F12 (Fn shows the media keys while held) |
| 2 | media, brightness and volume keys (Fn shows F1 to F12 while held); the kernel's default |
| 3 | nothing (off) |

The module parameter `fntoggle` (default on) is the Fn toggle between 1 and 2; it does nothing in modes 0 and 3. Settings ->
Keyboard -> *Function keys* (media keys first or F-keys first) writes mode 2 or 1, and the value is written again every ten
seconds and when the driver loads later.

While the IYAKKI strip is drawn the shell asks for mode 3, so the T2's own keys do not draw over the picture. **The moment the strip
is off, fails, or the display stops answering, the chosen mode (or 2) is written back** (`keyboard::restore_touchbar_mode`), so the Touch
Bar always ends up as a working key row. The choice *Touch Bar: off* keeps mode 3 (dark).

## 4. The strips

Esc is always at the left (it sends Esc to the screen in front). Next to it a chip shows the strip's name and four dots; tapping it
shows the next enabled strip. The strip last shown is remembered per account.

| Strip | Contents |
|---|---|
| System | volume slider (speaker icon, red cross when muted) and a Mute button; brightness slider; keyboard-light slider when the machine has one; Play/Pause for the radio or the player; the clock |
| Recent | the last six apps you opened, as tiles with their icon; a tile opens (or returns to) the app. From another app this is "Esc, then open": an app that keeps the Esc (a question about unsaved work) is never closed by the strip, a note says so |
| AI | Claude Code's state (ready, working, needs login, not installed, from the agent status cache), Chat, Ask (both open CHAT), Stop agent (enabled only while the chat in front is running), Approve and Deny while a tool call waits (the strip then comes up by itself), and Run and Save inside the IDE |
| App | up to six items the focused app offers; the system strip when it offers none (the launcher, apps without items) |

Apps register their items in `touchbar::Registry` (label, glyph, action; at most six, ids unique). Today: CHAT (new chat, model,
agent, approvals), IDE (run, save, open file, files, panel), TALES (play, save, model), RADIO and MEDIA (previous, play/pause,
next, stop), BROWSER (back, forward, reload, address). Every action is a key the app already handles (its F1 help lists it) or a
media key.

The picture is redrawn only when something it shows changed, at most ten times a second; only the changed rectangle is sent.
Nothing but the name is drawn before sign-in; after five idle minutes the bar goes black.

## 5. Settings

Settings -> Keyboard (administrators): **Touch Bar** (`u`): default (strip where the display works, keys otherwise), strip, keys, off.
**Touch Bar strips** (`e`): all four, System + Recent, System + AI, System + Recent + AI, System only, Recent only, AI only, App only (without touch the chip cannot be pressed, so a single strip is the way to look at the others; the first enabled strip is shown). **Touch Bar flip** (`l`): 0 to
3, for a bar that shows the picture mirrored (section 6). The page lists the keys mode of every Touch Bar device, what the strip is
doing (`drawing 2170 x 60`, `waiting: ...`, `not used: ...`) and says that it is display only.

## 6. How the display is driven, and where the protocol comes from

**Clean room.** Linux 6.15 added `drivers/gpu/drm/tiny/appletbdrm.c` (GPL-2.0, by Kerem Karabay). Kernel 6.12 does not have it, and
`tiny-dfr` is not shipped. The wire format was **re-derived from the description** of that file (field offsets, message tags, the
order of the start-up messages), which was read as a reference only. **No code was copied** (GPL-2.0 code cannot go into this
project); the structures, encoder, decoder and tests in `crates/iyakki-drivers/src/touchbar.rs` are written here. The reference is
named in the source and here: `drivers/gpu/drm/tiny/appletbdrm.c`, v6.15.

The device is USB `05ac:8302`. Besides its HID interfaces it has a vendor interface (class 0x10) with one bulk OUT and one bulk IN
endpoint. IYAKKI finds it by reading the device's descriptors from sysfs (so the interface number and endpoints are not guessed) and
talks to it through `/dev/bus/usb/BBB/DDD` with raw `ioctl`s (`USBDEVFS_CLAIMINTERFACE`, `_BULK`, `_CLEAR_HALT`, `_RELEASEINTERFACE`,
and `_RESET` in the bring-up; listed in `BOUNDARY.md`). Everything is little-endian:

* **Request** (32 bytes): u16 2, u16 0x1512, u32 0, u32 0, u32 16, then a 4-byte tag, 8 zero bytes, u32 16. Tags: `GINF` information,
  `REDY` ready, `CLRD` clear.
* **Start-up** (the kernel's order): send information, read the 65-byte answer (the device answers the first request after a
  configuration with a `REDY` signal first, which is read past once) with the panel size at bytes 32 and 36 (2170 x 60 on the 2020
  model), 24 bits per pixel at 40, pixel format "RGBA" at 53; send ready; send clear.
* **Frame update**: a 16-byte header (u16 2, u16 0x12, u32 9, u32 0, u32 size of the rest), u16 1, one byte frame id, 29 zero bytes;
  per rectangle u16 x, u16 y, u16 width, u16 height, u32 data size and the pixels (3 bytes each, blue, green, red); an 80-byte footer
  holding the constants 0xfffe, 0x80001, 0x80002, 0xffff and a u64 timestamp; zero padding to a multiple of 16. The answer (tag `UDCL`)
  repeats the timestamp at byte 32.
* **Orientation (not yet checked on a device).** The panel is built portrait; the kernel driver swaps the axes. In the coordinates
  used here (x along the bar) the rectangle header holds the rectangle as is, and the pixels run column by column, each column from
  the bottom row up. If the picture comes out mirrored, Settings -> Keyboard -> Touch Bar flip (0 to 3) changes the column order
  or the row order without a rebuild.
* The unknown header fields are sent with the values the kernel driver is described to send; what they mean is not public.

## 7. The safe bring-up

At boot the display's USB configuration times out (`usb 5-6: can't set config #1, error -110`). A measured recovery: writing 2 and
then 1 to `bConfigurationValue` later configured it once; further attempts timed out and the device stayed stuck until a restart.
So the boot code (`touchbar_retry` in `iyakki-core`, a background thread) does the following and nothing else:

1. A device that already has a configuration is left alone: no write at all.
2. Round 1, three seconds after the network is up: write 2, then 1 (the sequence that worked).
3. Round 2, 20 seconds later (if the kernel did not configure it meanwhile): **one** `USBDEVFS_RESET` of that device, then write 1.
   Before every write and the reset the code checks that the sysfs folder still has the same bus and device number and ids 05ac:8302,
   and the usbfs node's own device descriptor is read back before the ioctl. Nothing else on any bus is touched.
4. Round 3, 60 seconds later: write 1.
5. Then it gives up until a restart or a replug and says so in the log. At most three rounds and one reset.

### 7.1 The configuration that holds the display (one switch per boot)

Measured on a MacBookPro16,2: after the bring-up the Touch Bar (`05ac:8302`) is in **configuration 1**, which has **one interface, class
03 (HID), bound to `hid-appletb-kbd`**. The display (class 0x10 with a bulk IN and a bulk OUT endpoint) is not in it; the first version of
the strip tried to claim it there and the kernel answered `error 2` (no such interface). The device descriptors (`/sys/bus/usb/devices/5-6/descriptors`,
all configurations) say which configuration holds the display; the t2linux documentation and the earlier measurement say it should be configuration 2.

What the strip does when the display is in another configuration than the active one:

1. It only acts when the strip is enabled (not *off* or *keys*) on a T2 MacBook. It never looks at, writes to or resets any other device.
2. **One** `USBDEVFS_SETCONFIGURATION` to the configuration that holds the display, sent through the device's own usbfs node (same checks as
   the bring-up: still the same bus and device number, ids 05ac:8302, and the node's own device descriptor read back first). No retry, no
   reset, and at most one such switch per boot (a counter for the whole process; a new strip thread after a settings change does not get another).
3. The kernel then unbinds `hid-appletb-kbd` from the old interface and binds the drivers of the new configuration (the keyboard driver's
   `mode` file gets a new HID device number, for example `0003:05AC:8302.0007` instead of `.0005`; the shell looks the file up again each time
   and never remembers a path). IYAKKI waits 1.5 seconds, looks at the configuration, and the next look opens the display.
4. If the write fails or the device times out (-110) and the configuration did not change, or the display will not open afterwards, the device is put
   **back in the configuration it had, once** (`switch_configuration(old)`), the outcome is logged (`Touch Bar: configuration 1 -> 2: Switched`,
   `FellBack` or `Failed`), the strip stays off and the function keys keep working. Nothing else is ever tried that boot.
5. If no configuration holds a display interface the strip stays off and says `no USB configuration of the device has a display interface`.

Whether configuration 2 keeps the Esc/F-key HID interface is **not known** until it is tried on the device. If it does not, the keys disappear after the
switch: the strip draws its own Esc, and the firmware keys return only if the strip gives up (the way back above) or after a restart. Setting
Settings -> Keyboard -> *Touch Bar: keys* (or *off*) before the next start prevents the switch altogether.

Also: `hid-appletb-kbd` logged `error -ENODEV: Failed to get backlight device` at boot because the backlight device (`05ac:8102`, usb 5-7) appears about
0.1 s later (probe order). After a configuration switch the keyboard driver probes again with the backlight already there. The shell's ten-second sync
(and every strip status change) also puts a bar left in mode 3 with nothing chosen back to mode 2.

## 8. Never blocks, never hides the keys

* The display runs on its own thread (`iyakki-touchbar`), touches on another (`iyakki-touchbar-in`). The shell only swaps a picture
  into a mailbox, reads a queue and a status. A heartbeat watchdog marks the strip *stuck* after eight seconds without an answer and the
  shell stops sending.
* Opening failures back off 2, 5, 15, 30, 60 seconds, then the device is left alone for five minutes. Three failed transfers in a
  row close the session (a vanished device at once). Each transfer has the kernel's one-second timeout.
* Every failure ends in the plain key mode (section 3). A machine that is not a T2 MacBook starts none of this.
* No panic on a malformed answer: lengths are checked, positions are clamped with `clamp_safe`, a source scan test refuses
  `.clamp(`, `.unwrap()` and `.expect(` in these files.

## 9. Troubleshooting

| What you see | What it means |
|---|---|
| Keys on the bar, no strip, Settings says `waiting: the device has no USB configuration yet` | the boot bring-up has not (or could not) configure it; `dmesg | grep -i 'config #1'`; restart |
| `not used: another driver holds the interface` | something else claimed the display interface (a driver bound to it); the strip gives way |
| `waiting: switching the USB configuration 1 -> 2 (once)` | normal for a few seconds the first time; then `drawing 2170 x 60` |
| `not used: switching to USB configuration 2 failed; the device is back in 1` | the one allowed switch timed out; restart to try again; the keys work |
| `not used: no USB configuration of the device has a display interface` | the device offers no display; strip off, keys stay |
| `not used: ... one switch allowed this boot is used` | a switch (or its way back) already happened this boot |
| Strip shows but mirrored or reversed | Touch Bar flip (`l`) 1, 2 or 3 |
| Bar black after the strip stopped | `cat /sys/bus/hid/drivers/hid-appletb-kbd/*/mode` should say 2 (or your choice); the shell writes it back within a heartbeat, or `echo 2 > .../mode` |
| Buttons do nothing | expected without touch support in the kernel (section 2) |
| Pointer jumps on the main screen when the bar is touched | should not happen: the bar's touch node is excluded; report the output of `/proc/bus/input/devices` |

## 10. Device test plan (run over SSH on the Mac; reads first, no writes until step 6)

1. `lsusb | grep 05ac`; expect `05ac:8302` and `05ac:8102`.
2. `for d in /sys/bus/usb/devices/*; do [ "$(cat $d/idProduct 2>/dev/null)" = 8302 ] && echo $d $(cat $d/bConfigurationValue) bus $(cat $d/busnum) dev $(cat $d/devnum); done`
3. `lsusb -v -d 05ac:8302 2>/dev/null | grep -E 'bConfigurationValue|bInterfaceNumber|bInterfaceClass|bEndpointAddress|bmAttributes'`: find the class 16 (0x10) interface with two bulk endpoints and note which configuration holds it.
4. `grep -B2 -A10 -i 'touch bar' /proc/bus/input/devices`: is there a node with `ABS=` and a name like the bar (touch panel)? Note the `EV=` line.
5. `dmesg | grep -iE 'touch ?bar|appletb|5-6|bce'` and the log lines `Touch Bar:` from IYAKKI (`journal`/console): the status sequence `waiting`, `drawing 2170 x 60`.
6. Look at the bar: the System strip should show Esc, a chip, a volume slider, brightness, play and a clock. Mirrored? Settings -> Keyboard -> Touch Bar flip, look again.
7. Change the volume with the keyboard keys (or Fn row): the slider on the bar follows within a second. Change brightness: same.
7b. After the first strip start: `cat /sys/bus/usb/devices/5-6/bConfigurationValue` (expect 2 if that is the display's configuration), `ls /sys/bus/hid/drivers/hid-appletb-kbd/` (new device number after the switch), the log lines `Touch Bar: configuration 1 -> 2: Switched` and `drawing 2170 x 60`, and whether the keys (Esc, F-keys) are still on the bar and in `evtest`.
8. Settings -> Keyboard -> Touch Bar -> keys (`u` until "firmware's keys only"): within a heartbeat `cat /sys/bus/hid/drivers/hid-appletb-kbd/*/mode` is 2 (or 1/2 as chosen) and the T2's keys are back. Then *off*: mode 3, bar dark. Then default: the strip again.
9. Hold Fn with the keys mode: the layers switch (mode 1 and 2). Unchanged by the strip.
10. Look at the other strips: Settings -> Keyboard -> Touch Bar strips (`e`) until "recent", then "ai" (a single enabled strip is the one shown; an approval waiting in CHAT brings the AI strip up by itself). With touch the chip next to Esc cycles them.
11. If a touch node exists (step 4): tap the chip, drag the volume slider. Report what `evtest` or `/proc/bus/input/devices` shows.
12. Idle for five minutes: the bar goes black; press a key: it comes back.

## 11. Licences

All the code here is the project's own and permissively licensed with the rest of the project. The kernel driver `appletbdrm.c`
(GPL-2.0) was read as a reference for the wire format only; nothing from it is in the repository. `hid-appletb-kbd` and
`hid-appletb-bl` are GPL-2.0 kernel modules carried as patches in the Mac image variant (see `docs/MAC-HARDWARE.md`).
