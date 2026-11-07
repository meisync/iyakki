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
