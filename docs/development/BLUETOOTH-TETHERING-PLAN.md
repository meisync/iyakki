# Bluetooth tethering (use a phone's internet, share ours)

Status: plan, 2026-11-05. Owner request: "also wifi and bluetooth tether".

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
