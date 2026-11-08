# Touch recovery (software replug)

Problem: a touch screen is listed by the kernel but sends no event until the USB cable is unplugged and plugged in again (KI-077, Mac mini with
a USB-C touch monitor). Tablets cannot be replugged, so IYAKKI recovers in software. Code: `crates/iyakki-agent/src/touchrecover.rs`
(tests: `touchrecover_tests.rs`), the input re-open in `crates/iyakki-shell/src/input.rs` (`apply_reopen`), the button in
`crates/iyakki-shell/src/apps/settings_keyboard.rs`.

## What it does

Detection. A touch screen is an input device with ABS_MT_POSITION_X that is INPUT_PROP_DIRECT (or at least not called a pad), sits on USB HID
(`0003:...`) or I2C HID (`0018:...`) and has no keyboard keys. It is "silent" when it produced no event at all since it appeared for 90 s.
A screen nobody has touched looks the same, so the automatic run is limited to steps that disturb only that one device.

Ladder (least disruptive first). After each step the program waits 6 s for any event from a touch screen:

1. `Reopen`: the shell opens the evdev node again.
2. `HidRebind`: unbind and bind the HID device from `hid-multitouch`.
3. USB: `UsbReset`, `authorized` 0 then 1 of that one device (fallback: unbind/bind in the `usb` driver). I2C: `I2cRebind`, unbind and bind
   the client from its `i2c_hid*` driver.
4. `ModuleReload` (manual button only): `modprobe -r hid_multitouch`, `modprobe hid_multitouch`, only when the module exists and every device
   bound to it is a touch screen.

5. `LinkRetrain` (manual button only, USB panels only): the connected DisplayPort connectors' sysfs `status` is written `off`, 1.5 s later `on`,
   then `detect` (always written last, so a connector is never left forced); the screen blinks for 2 to 4 s. This is the software version of a
   cable replug, the one thing that woke the Mac's panel after a cold boot (KI-077). The automatic run never does this.

It stops at the first step after which an event arrives.

Refused, with a sentence in the log and the status line, and nothing written: a USB hub or root hub or port; a USB device with any interface
that is not HID, or with a keyboard interface (protocol 1), or with another input node that types; a driver other than `hid-multitouch`
(HID) or `i2c_hid*` (I2C); any path outside `/sys/devices` or a name of the wrong shape. The boot disk's USB port is never touched (only a
leaf device, never a port or hub).

Rate limits. Automatic: 3 tries per boot, 0 s / 180 s / 900 s apart; after a try the silence is counted again. Manual: 20 s apart, 8 per
hour. `iyakki.touchrecover=off` on the kernel command line turns the automatic run off. Audit: log lines `audit: touch.recover ...`; the
button also writes `settings.touch.reset` to the audit log.

Button. Settings -> Keyboard -> "Reset touch screen" (mouse, touch and keyboard `z`), next to "Probe touch screen" (the older single re-attach).
The row "Touch recovery" shows the status line. Touch the screen now and then while it runs: the program can only see that events arrive.

It sits beside the older KI-077 steps (`touchprobe.rs`, `touchfix.rs`: re-attach at 10 s, hub at 30 s, DP link and xHCI rungs, opt-in). It does
not replace them.

## Measured on the Mac mini (2026-11-06, image 1106)

After a cold boot the automatic run (tries at 96 s and 280 s) ran Reopen, HidRebind and UsbReset and saw no event after any of them; the older
hub re-attach also failed. `LinkRetrain` was added after that and has NOT been tried on hardware: press Settings -> Keyboard -> "Reset touch screen"
after a cold boot and read the `touch-recover:` lines.

RESULT 2026-11-07 (image 1107a, owner pressed Reset touch screen on the Mac mini after the cold boot): every step ran, including `LinkRetrain`
(log: "1 DisplayPort connector(s) switched off and on"; the display probe then saw `DP-1=connected` again about 1.3 s later), and NO touch event
followed any step. `ModuleReload` is skipped there (`hid_multitouch` is built into the kernel). So retraining the DisplayPort link in software does
not wake the panel either, and the physical cable replug still does something none of the software steps do (candidates: the USB-C connection
losing and regaining its data path or power-delivery contract, which resets the monitor's hub and touch controller together). Next experiment
(needs the owner at the Mac): follow the kernel log while replugging the cable once (`iyakki dev dmesg --follow --grep usb`) and compare what the kernel
sees with what the ladder does; then try a combined step (hub port power off and on plus the link retrain within one second).

## RESULT 2026-11-07 evening: the cause, measured over SSH on the Mac mini (image 1107h)

Root shell (`iyakki.ssh=1` entry), a 450 KB static helper (`hidfeat`: HIDIOCGFEATURE/HIDIOCSFEATURE and a timed read of `/dev/hidrawN`;
source kept with the owner's logs, not in the repository) and the owner touching the screen:

* The panel wch.cn 27c0:0859 has three HID interfaces, **all subclass 00 protocol 00** (`bInterfaceSubClass` read from sysfs and from the
  raw configuration descriptor): there is no boot-protocol interface, so step 0 (`ReportProtocol`) never applied; that theory is withdrawn.
* Interface `:1.0` (hid-multitouch, 704-byte report descriptor: Digitizer, report 13 with up to 10 contacts, X 0..16383, Y 0..9599;
  features report 10 Contact Count Maximum = 15 and report 33 **Device Mode** + Device Identifier) sent **0 reports** in 20 s of touching.
  `urbnum` of the device did not move while the agent's own ladder reported "no event".
* Interface `:1.2` (hid-generic, 80-byte descriptor: Mouse, report 7 = button, X 0..16383, Y 0..9599) sent **1081 reports** in the same
  20 s (`07 01 ce 0d 40 0b 00` ...), and its evdev node delivered BTN_LEFT presses and ABS_X/ABS_Y positions. So the controller is in
  **mouse mode**: it reports every finger, only through the mouse-like interface.
* Setting the Device Mode feature to 2 (touchscreen) by hand (`21 02 00`, the same request the kernel driver makes at every probe) changed
  nothing: the multi-touch interface stayed silent and the mouse-like one kept reporting. GET_REPORT of that feature returns the SETUP packet
  (`a1 01 21 03 00 00 08 00`), a firmware quirk; the controller's mode cannot be read or set over the class pipe. Only the power loss of a
  replug puts it back into multi-touch mode, which is why the replug "fixes" it.
* The owner saw touch work "for a bit" while the ladder's `HidRebind`/`UsbReset` had the multi-touch device unbound: in those seconds the
  shell had no touch device and let the mouse-like interface through.

**Why it looked dead**: `iyakki-shell::input::poll_devices` muted the mouse-like interface of a touch monitor whenever a multi-touch device
was present ("it repeats what the multi-touch interface says"). In mouse mode the only speaking interface was the muted one.

**Fix (shell, same day)**: `mouse_like_muted(finger, is_touch, touch_spoke)`: the mouse-like interface is an echo only once a multi-touch
device has actually spoken; a multi-touch device that never delivered an event leaves the word to it (logged once: "the multi-touch
interface has not spoken since it was opened; this mouse-like interface speaks for the touch screen (controller in mouse mode)"). Its finger
events are also counted for this module (`note_event`), so the automatic ladder no longer resets a panel that works through that interface.
Test `iyakki-shell::input::tests::mouse_like_interface_speaks_until_the_touch_device_does`. Gestures: the mouse-like interface carries one
finger and a button, so after a cold boot the panel behaves like a single-touch screen until a replug; that is the remaining limit of KI-077.

**VERIFIED on the Mac mini 2026-11-07 (image 1107i, update pack, full power off, cold boot, owner touching)**: "touch worked". The log
(`~/iyakki-work/mac/syslog-1107i.txt` with the owner): at 14.8 s "input: /dev/input/event6: the multi-touch interface has not spoken since it
was opened; this mouse-like interface speaks for the touch screen (controller in mouse mode)", then "finger down on /dev/input/event6" for
every press; the automatic ladder did not run (its events are counted now). KI-077 is closed for taps and drags; the open remainder is
multi-finger gestures until a replug.

## NOT proven

Nothing here ran on hardware. Tests use a fake sysfs tree in a temp directory and a fake Env that records writes. Not known:

* whether any step wakes the Mac monitor's panel (earlier software re-attach, hub reset, port and xHCI steps did NOT, see KI-077; the likely
  reason is that only the DisplayPort/Type-C replug retrains the link, which these steps do not do). This module may fail on the Mac for that
  reason and still help on a tablet whose panel is merely wedged.
* the real sysfs layout of the tablet touch panels (I2C client names, the driver names `i2c_hid_acpi` / `i2c_hid_of`, whether the panel is
  on `hid-multitouch` or `hid-generic`; a panel on another driver is refused);
* whether `hid_multitouch` is a module or built in on the image kernel (built in: that step says so and is skipped);
* whether a rebind hands the shell a new event node under the same name (the shell scan opens new inodes by itself; `Reopen` covers the same node);
* whether an untouched screen causes needless resets in the field (they are limited, a second of disturbance).

## Test on the Mac after a cold boot

1. Cold boot with the touch monitor connected; do not touch the screen. Sign in with keyboard or mouse.
2. After 90 s uptime the log should show (`iyakki dev` log or the journal):
   `audit: touch.recover automatic try 1: a touch screen is listed and sent no event for 90 s`, then per step
   `touch-recover: <name>: step Reopen: re-open the input node; touch the screen now`, `step HidRebind`, `step UsbReset`, each followed by
   `no event after <step>` or `an event arrived after <step>: touch works`, and `audit: touch.recover automatic try 1 ended: ...`.
   Touch the screen during those lines (about 6 s each) so the verdict means something.
3. If touch works, note the step name in the "arrived after" line. If not, the next tries come 3 and 15 minutes later (max 3).
4. Open Settings -> Keyboard: the "Touch recovery" row shows the last status. Press "Reset touch screen" (also with the mouse, the finger on a
   working part, or `z`) and touch the panel while it runs; the line ends "Touch events arrived after: ..." or "Tried: ... No touch event was seen".
5. Compare with the old behaviour: boot with `iyakki.touchrecover=off`, confirm touch stays silent, and replug the cable to confirm the panel is fine.
6. Record on the Mac: `ls /sys/class/input`, `cat /sys/class/input/input*/name`, the HID names under `/sys/bus/hid/devices`, and which step (if
   any) helped. Send these so KI-077 can be closed or the next rung chosen.

## 2026-11-07: the boot-protocol theory and step 0

Reading the Mac mini's saved kernel logs again: the panel's controller is a wch.cn 27c0:0859 with three HID interfaces, `:1.0` multitouch
(hid-multitouch), `:1.1` a vendor interface and **`:1.2` a boot-protocol mouse** (subclass 01, protocol 02, hid-generic "Mouse"). Firmware
built on EDK2, Apple's included, binds every boot mouse for its own menus and sends the HID request SET_PROTOCOL(boot). The kernel's
`usbhid` never sends SET_PROTOCOL(report) (`drivers/hid/usbhid/hid-core.c` only sets idle), and a cheap controller keeps the protocol
across bus resets, re-enumeration and driver rebinds: only a power loss puts it back, which is what the cable replug does and what none
of the refuted software steps did. In boot protocol the multitouch interface is silent. This explains every fact in KI-077: the device
enumerates and opens normally, every reset and rebind changes nothing, the physical replug fixes it, and the replug before the firmware
stage fixed it too (the firmware had already finished binding mice).

Step 0, `ReportProtocol` (`crates/iyakki-drivers/src/usbfs.rs`, `hid_set_report_protocol`): to each boot-protocol HID interface of the
one device (descriptor read back, vendor and product checked): DISCONNECT_CLAIM, the control request `21 0b 0001 <iface> 0000`, release,
and IOCTL(CONNECT) so usbhid takes the interface back. It runs first in the ladder for a USB panel that has such an interface, and once
at the first sight of the panel (`prime`, from the startup poll, before any silence is counted; it costs nothing when the protocol is
already right). Tests: `touchrecover_tests` (the step first and only on interface 2, the node from busnum/devnum, nothing else touched,
prime once, the safety check), `usbfs::protocol_tests` (layouts and request numbers against the kernel header, the request bytes, the
descriptor check). NOT yet seen on the Mac mini: the next pack carries it; after a cold boot the log should show
`touch-recover: prime: wch.cn TouchScreen: boot-mouse interface switched to report protocol at first sight` and a finger should work
without a replug. If touch still stays silent, the theory is wrong for this controller and the next experiment is the one above (follow
the kernel log through a physical replug).
