# A drive that is not seen after boot, and a stick that takes over the boot

Status: plan, 2026-11-05. Owner report from the Mac mini 2018 (T2, IYAKKI on the internal disk):

1. A Samsung T7 plugged in AFTER the system is up is not detected. A UVC camera plugged into the same kind of port works at once.
2. A T7 plugged in WHILE the Mac starts is booted instead of the internal disk. Owner's rule: **a newly attached device must not take over the boot.**

## What was observed (2026-11-05)

* After boot, with the T7 plugged straight into a USB-C port (owner, 2026-11-05: "camera is USB-A, T7 on USB-C"), the kernel log has no new USB line at
  all: no `new SuperSpeed` and no `new high-speed`. A camera on a USB-A port enumerated as `usb 1-14` on the main PCH controller (`00:14.0`) and its driver
  loaded. **The two were on different controllers**: the Mac mini's USB-C ports are Thunderbolt 3 ports behind the Thunderbolt-side xHCI (`08:00.0`,
  buses 3 and 4), which only sees a plain USB device when the Thunderbolt controller (`07:00.0`) has switched the port to USB. The Mac's kernel logs
  "device links to tunneled native ports are missing!" for both Thunderbolt controllers at boot, and no Thunderbolt event at the plug-in. Not yet
  tried: the T7 on a USB-A port (C-to-A cable). The same T7 is detected when hot-plugged into the build PC. At boot the firmware reads the T7
  without trouble (the firmware has set the port up itself).
* The keyboard and mouse hub (`1-8`) re-enumerated each time the T7 was plugged in (log times 239 s, 602 s, 606 s): the plug disturbs that hub.
* The Mac's kernel has two USB controllers: the PCH one (`0000:00:14.0`, buses 1 and 2) and the Thunderbolt-side one (`0000:08:00.0`, buses 3 and 4).
* The installer appends IYAKKI's firmware entry to `BootOrder` and never puts it first (`crates/iyakki-usb/src/efi.rs`: "the computer keeps
  starting the other system by default"). On the internal install nothing says "start the internal disk first".

## 1. Drive not seen after boot

Not a guess-and-patch job: the cause is not known. Order of work:

0. **Today, no code:** the T7 on a USB-A port with the C-to-A cable. If it is seen there, the defect is the Thunderbolt/USB-C path (hot-plug of a
   plain USB device behind the Thunderbolt controller), not storage or power, and the monitor below must cover the Thunderbolt controller too:
   `/sys/bus/thunderbolt/devices` (domain, `security` level, `authorized`), the Thunderbolt xHCI's ports and its runtime power state
   (`0000:08:00.0/power/runtime_status`; a controller asleep at plug-in time is the usual cause), plus `lspci`-level state of `07:00.0`.
1. **Port monitor (read-only, always on).** A small agent module reads each xHCI root port's state from sysfs / debugfs (`usbN/N-port*/state`,
   `.../ports/portNN/portsc` when debugfs is there) and logs a line on every change: port, connected or not, link state (U0, U3, Polling,
   Compliance, ...), speed, and whether an enumerated device follows within 3 s. It also goes into `iyakki dev diag` and the hardware check.
   Result: the next plug-in tells us whether the Mac sees a connection at all (power or cable), a link that does not train (USB 3 link,
   link power management) or a connection with no enumeration (controller routing).
2. **Then the fix that the evidence points to**, one of: a controller or port quirk set at start (link power management off for that controller),
   a hub or power workaround, or a documented "use this port / this cable" hint in the hardware check. Not before step 1 shows what it is.
3. A named test entry "USB storage troubleshooting" is added to the boot menus only if the evidence needs a kernel option; the existing
   "Compatible USB mode (no UAS)" entry stays as it is.

Exit: the T7 is detected after boot on the Mac mini, and the port monitor test (fake sysfs tree) is in the matrix.

## 1b. Is the touch problem (KI-077) the same cause? (owner hypothesis, 2026-11-05)

Both involve devices on the Mac mini's USB-C ports whose state is set up by the firmware and then disturbed or ignored once Linux runs.

| | For a shared cause | Against |
| --- | --- | --- |
| Touch | The monitor is on a USB-C (Thunderbolt 3) port; its hub (`usb 1-5`) drops 0.57-0.8 s after the splash's whole frame and 88 ms after the boot screen thread ends, and stays silent until the cable is replugged. | Blacklisting `thunderbolt` and `apple_bce` did not move the drop (4.637 s, 4.538 s): the Thunderbolt driver is not the trigger. |
| T7 | On USB-C it is not seen after boot; the firmware boots it. KNOWN-ISSUES line 49: the T7 on USB-C also "vanished once for 0.6 s" during use. | A monitor plugged into USB-C after boot DOES work (KI-077 "what worked"), so USB-C hot-plug as such is not dead. The monitor's hub shows up as a USB 2 device on the PCH controller; the T7 shows nothing at all, not even USB 2. |

Reading: the common element could be the Type-C port controller (power-role and data-role negotiation, DisplayPort alternate mode) that the Apple firmware
runs and Linux has no driver for. That would explain "works if set up by the firmware before Linux, breaks when something changes later" for both. It is
not proven, and nothing above separates it from two unrelated defects. The port monitor of section 1 is the test: it logs the port state on every
change, so the touch drop (hub port goes to a disconnected or reset state at 4.5 s) and the T7 plug-in (does the port register a connection) are seen
at the same level. If both show the port itself changing state without a kernel-side cause, the shared cause is real and the fix is on the
firmware / Type-C side (for example keeping the firmware's port configuration untouched, or sequencing what we do around the display hand-over).

## 2. A newly attached device must not take over the boot

Two places decide what starts: the firmware (which disk it picks) and, once a stick has started, the loader or GRUB on it.

* **Firmware side (the real fix).** On the installed system, IYAKKI makes its own internal disk the first start entry:
  `BootOrder` gets the IYAKKI entry first (it already creates the entry, `efi::name_running_disk`), and on a T2 Mac the startup disk the Apple
  firmware reads (`efi-boot-device` and `efi-boot-next` variables, as `bless` writes them) is set to the internal volume. Settings -> Startup:
  "Start this computer's own disk first" (default ON for an internal install; OFF for sticks). Reading the variables first, writing only the entry
  we created, keeping the old order in a file for undo (`iyakki startup-disk revert`).
  **Needs the Mac:** which variables the T2 firmware honours with an external disk attached. Step A: read-only listing of `BootOrder`, `Boot####`
  and the Apple variables from the running Mac (the developer view or a new `iyakki dev startup-disk status`); step B: the write; step C: the
  owner restarts with the stick attached.
* **Stick side (a safety net).** When a stick's loader starts and it finds an installed IYAKKI on another disk of this computer (the loader's
  "scan other systems" is on now), its default entry is that installed system and the stick's own entries sit under "More options"; install media keep
  "Install to a disk" on the first level. The Option-key picker still lets the owner start the stick on purpose. Applies to sticks only (an
  `install-media` marker, see the loader menu change of 2026-11-05), never to the installed disk.

Exit: with the T7 plugged in during a restart, the Mac mini starts the internal disk; holding Option still offers the stick.

## Risks

* Writing firmware variables on a T2 Mac can change what the owner sees at the Apple picker. Reads first, a revert command, only our entry written,
  and the owner present for the first restart.
* The stick-side default means a stick no longer starts by itself on a computer that has an install. Owners who want the old behaviour get
  "Start from this stick" as an entry; the decision is theirs per stick (open question: switch on the stick or in Settings?).
