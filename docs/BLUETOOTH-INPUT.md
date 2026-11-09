# Bluetooth keyboards, mice and game pads

F-NET-18, F-NET-19, F-NET-22 to F-NET-25. IYAKKI's own Bluetooth code (no BlueZ) pairs a Bluetooth keyboard, mouse or game pad (classic or low energy), keeps it
connected and hands its reports to the kernel, so the shell sees an ordinary input device.

## The Bluetooth page

Settings, Bluetooth (administrators). Top to bottom:

1. **One switch**, "Bluetooth", with the state in words: On, Off, Blocked by the hardware switch, No Bluetooth adapter, Adapter
   found (waiting for its driver), Driver not loaded, Needs firmware: Install, Controller not set up, Controller not answering.
   Pressing it frees the radio and powers the controller together (and, to switch off, powers down and blocks the radio). When it
   cannot turn on, the card says why and carries the fix as a button (Install firmware opens Store, Setup, Hardware; Load the
   driver anyway when the drivers are held). A hardware key or switch on the machine is reported, never worked around. The status
   line is one fixed place at the top of the page and goes by itself after a few seconds.
2. **Paired devices**, first. Each row: an icon (keyboard, mouse, game pad, phone, headphones, other), the name, a state chip
   (Connected, Connecting..., Not connected, Tethering, Playing) and its own buttons: Connect or Disconnect for an input device;
   Tether (Stop tether) and Audio (Stop audio) for a phone; Forget (asks on a card first). The line under the name is the last
   outcome (for example "Tether failed: tethering is off on the phone (attempt 2)" or "Tether connected 192.168.x.x"), so a retry
   loop is visible. Clicking the row or Enter does its first button. A second click while a connection is starting does nothing
   (the row says Connecting...); a new try is allowed when the attempt has ended or after 25 seconds. "Audio from a phone" below
   them lets a phone pair with this computer for two minutes.
3. **Nearby devices**: Scan shows "Scanning... N s" with a Stop button and ends by itself after 15 seconds or when the pairing
   popup opens. At most 8 rows are shown: named and pairable devices (keyboard, mouse, game pad, phone) first, strongest signal
   first, then other named devices; devices with no name and no useful type are behind "Show N more devices", which opens a
   scroll area of 8 rows with its own scroll bar and a search field (name or address). A device seen once with a name keeps it in
   later scans. Headphones and speakers are listed but say they cannot be used yet (this computer plays sound from a phone, it
   cannot send sound).
4. **Advanced** (closed by default): the radio switch and the controller power separately, Load driver (when held), Look again.

**Pairing is a popup.** Pair (or Enter) on a nearby device opens a centred dialog "Pair with <name>": Connecting...; the number to
compare in big digits with Yes and No; a PIN or a passkey to type on the other device, big, with Cancel; "Accept pairing with
<phone>? It will not send audio or files" for a phone that shows no number; then "Paired. Connect now?" (a keyboard connects, a
phone starts tethering) or the plain reason with Try again and Close. Enter is the default button (Yes when you started the
pairing, No when the other device did), Esc cancels. The popup traps the keys and clicks, announces its title and the number to a
screen reader, and also opens when the other device starts the pairing; when Settings is not open the shell shows a notice naming
the device. When the OTHER device shows the number (a phone or a car kit shows a passkey or a PIN, the kernel's User Passkey Request or PIN Code Request), the popup has a typing stage: "Type the passkey shown on <device>" (six boxes, digits only, paste keeps only the digits) or "Type the PIN shown on <device>" (1 to 16 characters; for a mouse or game pad that has none printed, Use 0000 and Use 1234 buttons fill the entry, never for a phone). OK or Enter sends it, Cancel or Esc sends the negative reply, a wrong length is refused in plain words and the popup stays open. The device is watched while you type: if it gives up the popup shows why, and after 45 seconds without an answer the negative reply is sent. A keyboard is the other way round: its PIN or passkey is typed on the keyboard (the page shows it). What you type is wiped when sent, cancelled or closed, is not kept in the pairing state and is never logged (the log only says that an entry was sent or cancelled); the screen reader hears "Passkey, edit, 3 of 6 digits", never the digits. A pairing that a phone starts toward this computer ("Let a phone pair") still answers only the number comparison.

Keys: Up and Down choose a row, Enter its main action, `e` the switch, `s` scan or stop, `/` search, `m` more devices, `t` the
first phone's tethering, `u` forget the chosen paired device, `a` Advanced, `r` look again. Every button is also reachable with
the keyboard walk and has a name, role and state for the screen reader. The page scrolls by whole blocks and fits 360x740.

**System log.** Settings writes outcomes as well as requests (`settings.bluetooth` lines): the controller (address, power, radio,
firmware) when the page opens, pairing paired or failed with the reason, tether attempt N connected (with the `iyakbt0` address) or
failed with the reason, tether disconnected, audio connected with the codec, playing from <phone>, stopped, failed, the HID
session started or stopped. No PIN, passkey or number is ever logged. These are seen while Settings is open.

## What it does

* **Pair**: Settings, Bluetooth, Scan, then Pair (or Enter) on a keyboard, mouse or game pad. A "just works" device pairs
  at once; a device with a number to compare opens the popup with the number and Yes and No; an older keyboard gets a
  PIN or passkey that you type on that keyboard, then Enter.
* **Use**: after pairing the device connects by itself within a few seconds. The code reads the device's HID report
  descriptor over SDP, opens the HID control (PSM 0x11) and interrupt (PSM 0x13) channels and creates the input device
  through `/dev/uhid`.
* **Reconnect**: the pairing and its keys are kept in `/var/lib/iyakki/bluetooth.json` (mode 0600, administrator only) and
  given to the kernel again after a restart. A bonded device that you switch on connects back by itself. If the link is
  lost IYAKKI tries again after 3, 10, 30 and 60 seconds (twice the last), then waits for the device.
* **Settings, Bluetooth**: each paired device shows Connected, Connecting... or Not connected, with the reason in plain
  words, and buttons Connect or Disconnect and Forget (removes the keys); see "The Bluetooth page" above.
* **Chat**: `bluetooth_list` (free), `bluetooth_connect`, `bluetooth_disconnect`, `bluetooth_forget` (each asks; forgetting
  asks every time). Pairing needs you at the device, so it is only in Settings.

## Phones (tethering)

A scanned phone is also pairable (same pairing, the person always confirms; a phone that shows no number is still asked). It is
stored as kind `phone`, never opens input channels, and gets Tether and Audio buttons on its paired row: Tether uses its Bluetooth tethering as this
computer's internet (BNEP over L2CAP PSM 15 into the TAP `iyakbt0`, own code, see docs/development/BLUETOOTH-TETHERING-PLAN.md).
Chat: `bluetooth_tether_status`, `bluetooth_tether_connect`, `bluetooth_tether_disconnect`. A phone can also play its audio through this computer (below).

## Low-energy keyboards, mice and game pads (HID over GATT, F-NET-22)

Many newer keyboards and mice are low energy only (or use it on a second slot). IYAKKI has its own ATT/GATT client for them
(`iyakki-devctl::att`, no BlueZ):

* **Pair**: Settings, Bluetooth, `s`, Enter on a low-energy keyboard, mouse or game pad ("low energy (HID over GATT) · Enter
  pairs"). The kernel does the LE security (SMP) through the management interface; a keyboard that wants a passkey shows it on the
  page for you to type on that keyboard, a "just works" device pairs at once. The long term key is kept in the same file as the
  classic keys.
* **Use**: the hub opens the LE attribute channel (L2CAP fixed channel 4), asks for a larger message size, finds the HID service
  (0x1812): report map, input/output reports with their Report Reference, protocol mode, HID information, the PnP id for the
  vendor and product number. It selects report protocol, switches notifications on (CCCD) for every input report and creates the
  input device through `/dev/uhid` exactly like the classic path. Notifications become `UHID_INPUT2` (with the report id first when
  the report map uses ids); LED changes go back as ATT write commands.
* **Reconnect**: after pairing, the controller is told to connect in the background whenever the device advertises (management
  command Add Device, auto-connect); the connection event starts the session. The same is done again for every bonded LE device after
  a restart.
* The ATT parser rejects short, odd or oversized messages, discovery loops are bounded, lists that go backwards are refused, long
  reads are capped at 4096 bytes.

## Bluetooth audio from a phone or laptop (A2DP sink, F-NET-23 to F-NET-25)

The phone plays, this computer's speakers sound. All in-house: no BlueZ, no codec library.

* **SBC codec** (`iyakki-devctl::sbc`): encoder and decoder written from the A2DP description: 4 or 8 subbands, 4 to 16 blocks,
  mono, dual, stereo and joint stereo, loudness or SNR allocation, any bitpool, CRC-8, hostile frames refused. The window tables
  were measured from a second implementation (see `sbc_tables.rs`).
* **AVDTP** (`avdtp`): the phone connects to PSM 0x19: Discover, Get(All)Capabilities, Set Configuration, Open, Start, Suspend,
  Close, Abort, with the specification's error codes, fragment reassembly and bounded input. A second connection from the same
  phone is the media channel (`a2dp_sys`).
* **SDP record**: a small SDP server on PSM 1 answers for one record, the A2DP sink (0x110B, AVDTP 1.3, profile 0x110D), so phones
  list "IYAKKI Audio". It answers ServiceSearch, ServiceAttribute and ServiceSearchAttribute with continuation, and the SDP error
  response.
* **Media**: RTP header, SBC payload header, reorder window, loss turned into silence of the right length, fragmented frames, damaged
  frames as one frame of silence.
* **Sound path**: the decoded sound goes to the speakers as a mixer source named after the phone (falls back to the play command
  when the mixer is off), so Settings -> Sound's per-source volume, master volume, mute and ducking apply. Suspend or stop frees the
  speaker at once.
* **Pairing**: a phone pairs with this computer. Settings, Bluetooth, the card "Audio from a phone", "Let a phone pair": this computer is
  discoverable for two minutes (class of device: audio loudspeaker). On the phone choose this computer; the number is shown on
  the page and you confirm it (always asked, even for a phone that shows no number). Afterwards nobody else can pair.
* **Settings**: the card says "Playing from <phone>" while sound arrives, "connected" when idle, with a Disconnect button; each
  paired phone row has an Audio button (connects this computer to the phone for audio) and the log gets the codec and start and stop.
* **Chat**: `bluetooth_audio_status` (free), `bluetooth_audio_connect`, `bluetooth_audio_disconnect`, `bluetooth_audio_pairing`
  (each asks).

## What it does not do

* **No sending sound to headphones** (A2DP source) and no AVRCP (buttons, volume sync), no hands-free profile. The scan says
  so next to headphones and speakers.
* **No AAC, aptX or LDAC**: only SBC (every A2DP device must support it).
* One phone at a time for audio.
* The kernel needs `CONFIG_UHID` (module `uhid`, added to `linux.fragment`; takes effect with the next kernel build) and the
  Bluetooth modules. IYAKKI runs `modprobe uhid` once if `/dev/uhid` is missing.

## Tested and not tested

Unit tests with fakes cover the SDP client (fragments, errors, hostile data), the UHID event layout, report and LED
translation, link loss, unplug, key storage and reload, the retry schedule, the Settings rows and the chat tools. No real
keyboard, mouse or game pad has been used yet: the sockets, `/dev/uhid`, the background threads, the key reload after a
reboot and a device-initiated reconnect are unverified.

Added 2026-11-08, all tested only against fakes and scripted peers (no radio traffic anywhere):

* ATT/GATT: every message round trip and hostile bytes, a scripted GATT server (service, characteristic and descriptor discovery over
  several rounds, long reads, notifications during requests, indications confirmed, an encryption error retried once), the HOGP
  session against a fake uhid (report ids, LEDs, link loss).
* SBC: round trips in every mode with an error bound, a larger bitpool being better, malformed frames, bit allocation limits, and
  (when ffmpeg is installed, as a development oracle that is never shipped) our frames decoded by an independent decoder and its
  frames decoded by ours.
* AVDTP with a scripted peer (every command, wrong states, wrong values, fragments, hostile bytes), SDP requests and continuation,
  RTP loss, reorder, wrap-around, the audio path with a fake output, the Settings rows and the chat tools.

NOT verified: typing a passkey or PIN that a real phone, mouse or game pad shows (only the scripted mgmt socket and the fake machine were used), the real LE connection, SMP pairing and passkey entry, the Add Device auto-connect after a restart, a real keyboard's
report map; a real phone finding "IYAKKI Audio", pairing with it, choosing SBC settings, streaming, suspend and resume, the sound on
the speakers, latency, and a mixer source opened from a thread of the hub. The SBC codec has been checked against an independent
implementation on this development PC only; bit-exactness with every phone's encoder is not proven.

## Steps for the owner: low-energy keyboard (Lenovo, MediaTek controller)

1. Settings, Bluetooth: the switch says On. Put the LE keyboard in pairing mode, choose Scan, then Pair on its row. In the popup type
   the passkey on the keyboard if asked.
2. Expect "Paired. Connect now?", then Connected within seconds; typing works. Report the row text if not. `dev` diagnostics: `grep -A4 "Bus=0005"
   /proc/bus/input/devices`.
3. Switch the keyboard off and on: it should reconnect by itself (Add Device auto-connect).

## Steps for the owner: phone audio

1. Settings, Bluetooth: the card "Audio from a phone" should say "ready" (if it is missing the switch is off or the audio service
   says why). Choose "Let a phone pair": "Waiting for a phone to pair".
2. On the phone: Bluetooth settings, choose "IYAKKI Audio" (the name is the computer's). Confirm the number here.
3. Play music on the phone, choose the computer as the output. The row should say "Playing from <phone>" with "SBC ..."; sound on the
   speakers. Change the volume in Settings -> Sound (the source is named after the phone).
4. Pause on the phone: the row goes back to "connected". Ask the assistant "is a phone playing?" (bluetooth_audio_status).
Report: the row text, the SBC line, whether sound is clean, and any "problem:" text.



## Test on the HP laptop (once booted)

You need a classic Bluetooth keyboard or mouse (many cheap ones; a "dual mode" keyboard such as a K380 is classic on its
first slot) and the USB stick or disk install signed in as an administrator.

1. Open Settings, Bluetooth. Expect the Bluetooth card with "Controller hci0: ...". If it says Off, press the switch (or `e`).
   If it says No Bluetooth adapter or Needs firmware, stop: that is the Wi-Fi/Bluetooth firmware problem (docs/MAC-WIFI-BLUETOOTH-SETUP.md is for
   Macs; on the HP the Realtek file comes with the image).
2. Check the device file: in a terminal tile run `ls -l /dev/uhid`. If it is missing, `modprobe uhid`; if that says the module
   does not exist, this image was built before `CONFIG_UHID=m` and needs the next kernel build.
3. Put the keyboard in pairing mode (hold its Bluetooth key until the light blinks). In Settings, Bluetooth press `s`, wait for the
   scan (it stops by itself), choose Pair on the keyboard row.
4. Answer the popup: compare the number and choose Yes, or type the PIN it shows on the keyboard and press Enter there.
   Expected: "Paired. Connect now?" then within about five seconds the device row says Connected.
5. Type on the Bluetooth keyboard in a text field. Also check `grep -A4 "Bus=0005" /proc/bus/input/devices` shows the keyboard.
6. Switch the keyboard off and on. Expected: it says connected again within seconds, without pairing.
7. Restart the HP. Expected: after sign-in the keyboard (switched on) connects without pairing. If it does not, look at
   `ls -l /var/lib/iyakki/bluetooth.json` (should exist, mode `-rw-------`) and report what the row says.
8. Ask the assistant "disconnect my keyboard", approve the card, then "connect my keyboard". Then choose Forget on its row (it asks first) and pair again.

What to report when something fails: the row's text (it carries the reason), the output of `ls -l /dev/uhid`, and whether
step 5 shows the device in `/proc/bus/input/devices`.
