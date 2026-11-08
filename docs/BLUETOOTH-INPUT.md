# Bluetooth keyboards, mice and game pads

F-NET-18 and F-NET-19. IYAKKI's own Bluetooth code (no BlueZ) pairs a classic Bluetooth keyboard, mouse or game pad, keeps it
connected and hands its reports to the kernel, so the shell sees an ordinary input device.

## What it does

* **Pair**: Settings, Bluetooth, `s` scans, Enter on a keyboard, mouse or game pad marked "pair". A "just works" device pairs
  at once; a device with a number to compare shows "Does X show 123456?" with a Yes and a No row; an older keyboard gets a
  PIN or passkey that you type on that keyboard, then Enter.
* **Use**: after pairing the device connects by itself within a few seconds. The code reads the device's HID report
  descriptor over SDP, opens the HID control (PSM 0x11) and interrupt (PSM 0x13) channels and creates the input device
  through `/dev/uhid`.
* **Reconnect**: the pairing and its keys are kept in `/var/lib/iyakki/bluetooth.json` (mode 0600, administrator only) and
  given to the kernel again after a restart. A bonded device that you switch on connects back by itself. If the link is
  lost IYAKKI tries again after 3, 10, 30 and 60 seconds (twice the last), then waits for the device.
* **Settings, Bluetooth**: each paired device shows connected, connecting, not connected or failed, with the reason in
  plain words. Enter connects or disconnects, `u` forgets (removes the keys).
* **Chat**: `bluetooth_list` (free), `bluetooth_connect`, `bluetooth_disconnect`, `bluetooth_forget` (each asks; forgetting
  asks every time). Pairing needs you at the device, so it is only in Settings.

## What it does not do

* **No audio** (A2DP, headphones, speakers): the page says so next to such devices.
* **No low-energy keyboards or mice** (HID over GATT, HOGP). They need an ATT/GATT client, a separate step; the long term
  keys are already stored for it. The scan marks them "low energy: not supported yet".
* The kernel needs `CONFIG_UHID` (module `uhid`, added to `linux.fragment`; takes effect with the next kernel build) and the
  Bluetooth modules. IYAKKI runs `modprobe uhid` once if `/dev/uhid` is missing.

## Tested and not tested

Unit tests with fakes cover the SDP client (fragments, errors, hostile data), the UHID event layout, report and LED
translation, link loss, unplug, key storage and reload, the retry schedule, the Settings rows and the chat tools. No real
keyboard, mouse or game pad has been used yet: the sockets, `/dev/uhid`, the background threads, the key reload after a
reboot and a device-initiated reconnect are unverified.

## Test on the HP laptop (once booted)

You need a classic Bluetooth keyboard or mouse (many cheap ones; a "dual mode" keyboard such as a K380 is classic on its
first slot) and the USB stick or disk install signed in as an administrator.

1. Open Settings, Bluetooth. Expect a controller row ("Controller hci0: ..."). If it is off, select it and press Enter (or `p`).
   If there is no controller row, stop: that is the Wi-Fi/Bluetooth firmware problem (docs/MAC-WIFI-BLUETOOTH-SETUP.md is for
   Macs; on the HP the Realtek file comes with the image).
2. Check the device file: in a terminal tile run `ls -l /dev/uhid`. If it is missing, `modprobe uhid`; if that says the module
   does not exist, this image was built before `CONFIG_UHID=m` and needs the next kernel build.
3. Put the keyboard in pairing mode (hold its Bluetooth key until the light blinks). In Settings, Bluetooth press `s`, wait ten
   seconds, select the keyboard row marked "pair" and press Enter.
4. Answer what the page asks: compare the number and choose Yes, or type the PIN it shows on the keyboard and press Enter there.
   Expected: a green "paired" row, then within about five seconds the device row says "connected".
5. Type on the Bluetooth keyboard in a text field. Also check `grep -A4 "Bus=0005" /proc/bus/input/devices` shows the keyboard.
6. Switch the keyboard off and on. Expected: it says connected again within seconds, without pairing.
7. Restart the HP. Expected: after sign-in the keyboard (switched on) connects without pairing. If it does not, look at
   `ls -l /var/lib/iyakki/bluetooth.json` (should exist, mode `-rw-------`) and report what the row says.
8. Ask the assistant "disconnect my keyboard", approve the card, then "connect my keyboard". Then press `u` on the row to forget
   it and pair again.

What to report when something fails: the row's text (it carries the reason), the output of `ls -l /dev/uhid`, and whether
step 5 shows the device in `/proc/bus/input/devices`.
