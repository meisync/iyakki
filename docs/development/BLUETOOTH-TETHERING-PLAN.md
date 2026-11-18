# Bluetooth tethering (use a phone's internet, share ours)

Status: plan, 2026-11-05; client side (use a phone's internet) built 2026-11-08, not yet tried with a real phone (see Status below). Owner request: "also wifi and bluetooth tether".

## Today

* Settings -> Network -> Share internet lists "Bluetooth (use a phone's tethering)" as a source and "Bluetooth tethering (phones connect to this
  computer)" as a way to share. Both say "not available on this build".
* Why: the kernel has no BNEP (`CONFIG_BT_BNEP` is unset), the Bluetooth network encapsulation protocol that phones use for tethering (the PAN profile).
  The Mac mini's Bluetooth controller is there (`hci0` in `/sys/class/bluetooth`, checked 2026-11-05); whether it finds devices is not tested yet.

## Approach: BNEP in our own code, no kernel option

Building BNEP into the kernel is one config line, but it means a new kernel for every variant and the Mac kernel is already the long pole. BNEP is a small
protocol (Ethernet frames over an L2CAP channel on PSM 0x000F with a short setup exchange), so we write it in userspace:

1. **L2CAP socket** (AF_BLUETOOTH, BTPROTO_L2CAP, SOCK_SEQPACKET) to the phone, PSM 15. Plain `libc` calls, as the rest of the agent does.
2. **TAP interface** (`iyakbt0`) through `/dev/net/tun`. Frames read from the TAP are wrapped in BNEP and written to the L2CAP socket, and the other way round.
3. **BNEP setup**: `SETUP_CONNECTION_REQUEST` with the service UUIDs (PANU = 0x1115, NAP = 0x1116, GN = 0x1117), the response, and the filter messages
   (accept everything; answer the phone's filter requests with success).
4. **Pairing and connecting** through BlueZ-free means: the kernel's mgmt interface (HCI mgmt socket) for pairing with a numeric comparison that the person
   confirms on the screen. Check first whether the build has the kernel's `CONFIG_BT_HCIBTUSB`, `CONFIG_BT_RFCOMM` is not needed.

## What the person sees

* **Use a phone's tethering (PANU)**, first. Settings -> Network -> Share internet -> Internet comes from -> Bluetooth: pair the phone, turn on "Bluetooth
  tethering" on the phone, press Connect. The TAP gets an address by DHCP (our own DHCP client already exists for Ethernet) and becomes an uplink like
  any other, so the existing share manager can pass it on to the hotspot, a USB cable or an Ethernet port.
* **Share ours (NAP)**, second. Phones connect to the computer, which answers DHCP on `iyakbt0` and does the same NAT and firewall as for Wi-Fi. Same
  client isolation default as the hotspot.
* One card for pairing (code shown on both sides, press Yes), one switch per direction, a plain message for every failure (no controller, no firmware,
  phone refused, tethering is off on the phone).

## Tests

* BNEP framing and the setup exchange against a fake phone (pure functions, no radio).
* The L2CAP and TAP calls behind a trait with a fake for the manager tests; the manager treats `iyakbt0` like any other uplink and ignores it in the
  network set-up (as for `iyakap0`).
* On hardware: pair the owner's phone, connect, ping through it. The Mac mini's firmware is the first thing to check.

## Order

1. Check that the Mac's `hci0` scans and pairs (read through the mgmt socket) and write down what is missing.
2. BNEP codec and PANU over L2CAP + TAP, tests with a fake phone.
3. Pairing card, uplink entry, hardware test.
4. NAP.
5. FEATURES.md rows, KNOWN-ISSUES line for what the hardware cannot do.

## Open questions

* Does the Mac mini's Bluetooth need the Apple firmware blob? (Firmware by button, the same way as for Wi-Fi.)
* Do we accept phones that only offer NAP without a PIN, or always require the numeric confirmation? (Recommendation: always confirm.)

## Status 2026-11-08: client side built (use a phone's internet), unverified on a real phone

Owner decisions applied: BNEP in our own userspace code (no kernel `bnep`), the numeric confirmation is always asked for a phone, NAP (sharing ours)
is **not** in this slice.

### What is built

* `iyakki-devctl::bnep`: pure BNEP codec (setup request and response with PANU 0x1115, NAP 0x1116, GN 0x1117 in the 2-, 4- and 16-byte forms, general and the
  three compressed Ethernet forms, extension headers skipped, filter requests answered with success, unknown control messages answered with
  "not understood", a setup request from the phone refused with code 4). Tests include every prefix of valid packets, every type/length byte pair and a
  pseudo-random flood: no panic.
* `iyakki-devctl::bt_pan`: the PANU session `run()` over the trait `PanLink` (fake phone in the tests: set-up, frames both ways, refusal, silence, hang-up,
  hostile packets), the real link (the L2CAP socket of `bthid_sys::L2Sock`, asked for an MTU of 1691 before connecting, plus a TAP `iyakbt0` from
  `/dev/net/tun`, one `poll` on both), the manager `tether()` (one connection, state Idle / Connecting / Connected / Failed(reason)), the DHCP client of
  `Control::dhcp` on `iyakbt0` once the phone said yes, and the plain-language reasons (tethering off on the phone, the phone refused, the phone did not
  answer, no controller, controller off, radio blocked by On/off, no valid address, low-energy-only controller).
* Pairing a phone (`bthid_sys`): a scanned device of class phone is now pairable; the person always confirms (the number to compare, or "it shows no
  number, pair anyway?" for a phone without display); the pairing is stored with kind `phone` and never starts an input-device session. Headphones and
  speakers still say "not supported yet (no audio)".
* Settings -> Bluetooth: a "<phone> · Tether" row for each paired phone (Enter connects or disconnects, `u` forgets, the failure reason on the row), a
  **Tether** button, the **On/off**, **Power**, **Scan** and **Tether** buttons find their own row whatever is selected, a failed Power says why ("the radio is
  blocked by On/off", "the controller gave no reason ..."), the stale "Pairing is not available yet" scan message is gone, and the scan asks the kernel to look
  up the names of classic devices (the "confirm name" flag) and waits up to 6 s past its time for them, so the first scan shows names.
* Settings -> Network -> Share internet -> Internet comes from: `iyakbt0` is listed as "Bluetooth (use a phone's tethering) iyakbt0 - has internet" like any
  uplink once it has a default route; until then the "Bluetooth" entry says what is missing. The network set-up ignores `iyakbt0` like `iyakap0`
  (`iyakki_devctl::BT_PAN_IFACE`, `iyakki-core::net::list_ifaces`), and sharing never offers it as the cable to share on.
* Chat: `bluetooth_tether_status` (free), `bluetooth_tether_connect` and `bluetooth_tether_disconnect` (each asks; the card says the traffic counts against
  the phone's data plan).

### Not built

NAP (phones using this computer), GN, the SDP record for NAP, automatic reconnect of the tether when the phone comes back, a status chip in the top bar.

### Not verified (no real phone, no radio in the tests)

The tests use a fake phone and a fake link. Nothing in this slice has been run against a real phone, a real `hci0`, `/dev/net/tun` or `udhcpc`. In particular
unproven: that the L2CAP options call really gives MTU 1691 on the Lenovo's controller; that Android and iOS answer the PANU request with the NAP
destination UUID as written (Android does in BlueZ's own client); the numeric comparison on the screen with a real phone; the name look-up of the scan on real
controllers; the DHCP answer arriving through `iyakbt0`; the route of `iyakbt0` winning over other uplinks (it has no metric of its own; unplug the cable when
testing).

### Steps for the owner (Lenovo with a phone, as administrator on the machine)

1. Update the Lenovo with this build. Settings -> Bluetooth: if the radio row says off, press **On/off**, then **Power**. Press **Scan (10 s)**; your phone should be
   in the list **with its name** after the first scan (the phone must be on, unlocked, Bluetooth on, and "visible" in its Bluetooth screen).
2. Select the phone and press Enter: "Does <phone> show NNNNNN?" appears; compare with the number on the phone, press Yes on both sides.
3. Switch **Bluetooth tethering** on in the phone (Settings -> Network -> Hotspot and tethering).
4. Settings -> Bluetooth: press **Tether** (or select "<phone> · Tether" and press Enter). The row goes connecting, then connected with an address
   (`192.168.44.x` is what Android usually hands out). If it fails the row says why in plain words.
5. Unplug the cable and the Wi-Fi to prove it: open Settings -> Network -> Share internet: "Internet comes from" lists `iyakbt0` as "has internet". Open a
   web page; the phone's data counter moves.
6. Chat: ask "connect to my phone's internet" (a card appears), "is the phone tethering on" (status), "disconnect the phone".
7. If something fails, send the words the row shows plus `ls /sys/class/net`, `ip addr show iyakbt0` and `cat /run/udhcpc-iyakbt0.pid` (the session itself
   writes no log line yet).
