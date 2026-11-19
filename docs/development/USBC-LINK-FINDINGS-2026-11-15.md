# One USB-C cable between two Macs: what was found on 2026-11-15

Scope: the developer-mode link over ONE cable (`iyakki dev usb`, `docs/DEVELOPER-MODE.md` section 10), tried for the first time on real
machines: a Mac mini 2018 (Macmini8,1) and a MacBook Pro 16,2, both on 0.1.0-alpha.4-dev with kernel 6.12.112, joined by the owner's USB-C
cable (a full-featured cable that also drives a monitor; whether it is rated for Thunderbolt is **not confirmed**).

**Result: no link. The cause is not proven.** What is proven is where the fault is not, and which single test separates the two causes left.

**State update (alpha.5 merge): the final form now compiles with zero warnings; the 36 directlink and usbports tests and the 7 Settings page tests pass, and the lockdown skip is covered by tests in `usbports` and `directlink`. Still nothing has run on a device (Test numbers stay at fakes level, Manual needed).**

**Earlier state of the code on branch `alpha.4/usbc-link` (work stopped on 2026-11-15 for the alpha.4 cut).** Section 4 describes what is
written. An earlier form of it passed its tests (30 direct-link tests, 7 Settings page tests). After that the lane read was made on-demand, and the
lockdown skip, the kernel-log read and the two-USB-hosts text were added; **that final form was never compiled or run.** First step in alpha.5:
`cargo test -p iyakki-agent --lib -- directlink usbports`, `cargo test -p iyakki-apps --lib usbc_tests`, the matrix, boundary and ratchet tests of
`iyakki-core`, `check-linux-cfg.sh` and `compat-check.sh`; fix what they show before merging. Nothing of it has run on a device.

**The three findings in one place.**
1. Between the Mac mini 2018 and the MacBook Pro 16,2, with the owner's cable, the USB-C ports did not enter Thunderbolt mode: both controllers awake, not one
   interrupt on a replug. Whether the cable or Apple's port controllers (two hosts, no macOS) is the reason is open (section 3).
2. Kernel lockdown (every Secure Boot image) refuses all debugfs reads, root included, and logs each try. IYAKKI's startup USB probe was doing that on every
   poll (the Lenovo's `Lockdown:` lines). Debugfs is therefore not a place IYAKKI can rely on for diagnostics; `/proc/interrupts` and sysfs are.
3. A USB-C cable between two computers whose ports stay in plain USB gives `usbN-portM: Cannot enable. Maybe the USB cable is bad?` (the Lenovo with a Mac):
   two USB hosts, which cannot work; it needs Thunderbolt/USB4 on both ends or a host-to-host bridge cable.

## 1. What was read on the machines (read-only, as the unprivileged developer-mode shell user)

| Fact | Mac mini 2018 | MacBook Pro 16,2 |
|---|---|---|
| Controllers | two Titan Ridge, `8086:15eb` at `07:00.0` and `7d:00.0` | two Ice Lake built-in, `8086:8a17` at `00:0d.2`, `8086:8a0d` at `00:0d.3` |
| Driver | `thunderbolt` and `thunderbolt_net` loaded; two domains, security `user` | the same |
| Devices on the Thunderbolt bus | only each controller's own router (`0-0`, `1-0`, "Macintosh") | the same |
| Controller power | `power/control=auto`, `runtime_status=active`, `D0`, never suspended since the start (`runtime_suspended_time` 0) | the same |
| Hotplug bridges below it | `06:01.0`, `06:04.0`, `7c:01.0`, `7c:04.0` suspended in D3hot (normal with nothing behind them); `pciehp` interrupts 0 | `00:07.0` to `00:07.3` suspended in D3hot; `pciehp` interrupts 0 |
| Thunderbolt interrupts since the start | 194, 194, 193, 193 | 170 on each of four lines |
| After the owner unplugged and plugged the cable at the Mac mini (13:30) | **unchanged**: 194, 194, 193, 193; no new device, no `thunderbolt0` | **unchanged**: 170 x 4; no new device |
| Kernel log | only `device links to tunneled native ports are missing!` per controller | the same, plus `can't derive routing for PCI INT A` |

## 2. What that rules out

* **Not a missing driver or module.** Both modules are loaded, both domains exist.
* **Not a sleeping controller.** The first theory (the controller is runtime-suspended and loses the plug, as the USB controller of F-NET-15 did)
  is wrong: both are awake in D0 and were never suspended.
* **Not an approval that is waiting.** No device appears at all, so there is nothing to approve; host-to-host links need none anyway.
* **Not the kernel's handling of the event.** On these Macs the kernel itself manages the ports (it uses its software connection manager on every Apple
  computer and leaves the controller firmware's manager alone). A cable that brings a Thunderbolt link up makes the controller send a plug event,
  which arrives as an interrupt. The counters did not move by one on either side. Nothing reached the driver, so nothing in the driver dropped it.
* **The warning in the kernel log is not the cause.** `device links to tunneled native ports are missing!` is about the order in which devices are
  woken after sleep (the kernel has a list of older Apple controllers for which it sets these links; Titan Ridge and Ice Lake are not on it). It does
  not stop a link from coming up.

So the port never switched to Thunderbolt mode. That switch is made below the kernel, by the USB-C port controller chips of the two Macs when they
talk to each other and to the cable's marker chip over the cable's configuration wire. The kernel has no part in it on these machines (this kernel
has no USB Type-C support built, `CONFIG_TYPEC` is not set, and on Intel Macs the port controller is not described to Linux in a way a driver binds to).

## 3. The two causes that are left

1. **The cable.** Thunderbolt mode is only entered when the cable says it can carry it. A charging cable, a USB-only cable and many cables sold
   for monitors cannot, although a monitor works with them. This is the more likely cause and the cheapest to test.
2. **Apple's port controllers between two hosts without macOS.** Each Mac's port controller runs Apple's firmware. With a dock or a drive it
   enters Thunderbolt mode by itself; whether it does so between two computers when neither runs macOS is not known to us. If this is the cause,
   IYAKKI cannot fix it with a kernel option; it would need a driver for that chip written here, and that is not planned work yet.

**The test that separates them** (no approval, a minute): run `iyakki dev usb watch` on the Mac mini and, while it counts,
plug in (a) a cable with the lightning mark (Thunderbolt 3 or 4) to the MacBook Pro, then (b) a Thunderbolt dock or drive if one is at hand.

| What the watch says | Meaning |
|---|---|
| (a) "The other computer appeared" | the first cable was the cause; done |
| (a) nothing, (b) "A Thunderbolt device is connected" | port and controller are fine with a device; host to host is refused by the port controllers: cause 2 |
| (a) nothing, (b) nothing | the port does not enter Thunderbolt mode at all under Linux on this Mac: cause 2, wider |
| "noticed the cable ... but no computer introduced itself" | the link came up; the fault moved up to the kernel or the other end, and the lane states in **Copy diagnostics** say where |

**The lane states** (does a port see a cable link at all) would be the direct proof, but they are only in the kernel's debug files. Those need root (the
developer-mode shell runs as an unprivileged user for an `admin` pairing, so they could not be read from the PC), and **under kernel lockdown, which every
Secure Boot image runs with, nobody can read them, root included**. `iyakki dev usb` reads them on the device where it can and puts them into
**Copy diagnostics**; where it cannot, the replug test (interrupt counts, readable everywhere) is the tool. They are expected to say "nothing plugged in" on
every lane; anything else changes the picture.

### A second observation the same day: the Lenovo and a Mac

The owner joined the Lenovo (lenovo-t7) and a Mac with the USB-C cable. The Lenovo's kernel log said
`usb usb2-port2: Cannot enable. Maybe the USB cable is bad? config error`. That is a SuperSpeed USB port trying plain USB and finding another USB host
instead of a device: two hosts on one cable, the case section 10.1 of `docs/DEVELOPER-MODE.md` says cannot work. It also shows that on that pair the
ports fell back to plain USB and did not enter Thunderbolt mode. The check list and the replug test now name this case when the line is in the kernel log.

The same kernel log showed many `Lockdown: iyakki-startup-: debugfs access is restricted` lines. They came from IYAKKI's own startup probe
(`iyakki_agent::usbports`, which reads the USB port states from debugfs on every poll). It now looks at `/sys/kernel/security/lockdown` first and leaves the
debug files alone when a lockdown mode is on, as does the lane-state read.

## 4. What changed in IYAKKI

* **A wrong green step fixed.** The computer's own router (`0-0`) was counted as "a Thunderbolt device is connected", so step 2 was green with nothing
  plugged in, on every Thunderbolt computer. It is now its own kind and never counts.
* **The cause is named.** Step 2 and **Copy diagnostics** now say: controller asleep; awake and no lane sees a cable (cable or other end); a cable is seen
  but the link is still being set up; the link is up but nobody answered as a computer. With the lane states unreadable it points to the replug test.
* **Replug test.** `iyakki dev usb watch [SECONDS]` and the **Replug test** button (key `w`) on Settings -> Developer mode -> Connect over USB-C watch the port
  for 30 seconds while the cable is plugged out and in, and say whether this computer noticed it. Nothing is changed on the machine.
* **Diagnostics text** lists each controller with its power state, the interrupt count and the lane states.
* **Two USB hosts named.** When the kernel log has `Cannot enable. Maybe the USB cable is bad?`, steps 1 and 2 and the replug test say that both ends
  are USB hosts and what cable or port is needed.
* **Quiet under lockdown.** The startup USB probe and the lane-state read no longer open debugfs files on a Secure Boot image (no more `Lockdown:` lines).
* No kernel option, boot parameter or power change was made: the evidence supports none.

## 5. What works today instead

A **USB-C (or USB-A) Ethernet adapter on each computer and one network cable between the two adapters.** The drivers are in every image (`r8152`,
`ax88179_178a`, `asix`, `cdc_ether`, `cdc_ncm`). With no router in between, each side waits 20 seconds for DHCP, then takes a link-local address, the
firewall treats the adapter as a direct link, and the check list goes on from step 3 as it would for Thunderbolt. Tested with made-up machines
(`two_usb_ethernet_adapters_and_a_network_cable_are_a_direct_link_on_a_thunderbolt_machine`); **not yet tried on real adapters.**

Limits, said plainly: the adapter must be a USB one on **both** ends. A cable from an adapter into a built-in Ethernet port gets no address on the
built-in side (the link manager does not take over built-in cards). And when both computers are on the same network anyway, `iyakki dev pair NAME.local`
needs no cable at all.

## 6. Hardware test recipe (about ten minutes, with the owner)

Needs the build with this change on both Macs, developer mode on, both signed in.

1. On the Mac mini: Settings -> Developer mode -> Connect over USB-C. Expect step 1 green, **step 2 red** ("Nothing is connected..."), and a **Replug test** button.
   Before this change step 2 was green; if it still is, the build is the old one.
2. Press **Copy diagnostics** and keep the text. Write down the `lane` lines, or "lane states: not readable" (expected on a Secure Boot image).
3. Press **Replug test**, unplug the cable at the Mac mini, wait five seconds, plug it in. Write down the sentence it ends with.
4. Repeat step 3 with a cable that has the lightning mark, and with a Thunderbolt dock or drive if there is one. Use the table in section 3.
5. Ethernet instead: put a USB-C Ethernet adapter on each Mac, join the adapters with a network cable, wait 30 seconds, press Check again. Expect steps 1 to 3 green
   and the adapter's name with a 169.254 address; then pair and run **Speed**.
6. On the Lenovo after a restart: `iyakki dev dmesg --grep Lockdown --device lenovo-t7` should show no new `debugfs access is restricted` lines from `iyakki-startup-`.
7. Record: which cable, which ports, each sentence. A link over Thunderbolt or over the adapters with steps 1 to 7 green is the first real pass for F-DEV-10.

### Hardware test recipe, extended (alpha.5; nothing below has been run yet; Manual stays "needed" until someone does)

A. **Lightning-marked cable test.** Use a cable printed with the lightning mark (Thunderbolt 3 or 4, a short passive one up to 0.8 m or an active one). Mac mini to MacBook Pro.
   Start `iyakki dev usb watch 60` on one side, plug the cable in at both ends, record the sentence it ends with and the interrupt counts before and after
   (`/proc/interrupts`, lines with `thunderbolt`). Repeat with each port pair and with the cable turned around. Pass: "The other computer appeared", or steps 1 to 4 green.
B. **Thunderbolt dock or drive test.** Plug a Thunderbolt dock or drive into each Mac in turn, with the watch running. Pass for the port: the watch says "A Thunderbolt device is connected"
   and a device (not only the router `N-0`) appears under `/sys/bus/thunderbolt/devices`. Read this together with A using the table in section 3 to tell cable from port controller.
C. **USB Ethernet adapter route on two machines.** One USB-C (or USB-A) Ethernet adapter per machine, a network cable between the adapters, no router. Wait 30 seconds, press Check again.
   Pass: steps 1 to 3 green on both, a 169.254 address on each adapter, then pair and run Speed. Try it on at least two different machine pairs (for example Mac mini to MacBook Pro, and a Lenovo or HP to a Mac),
   and note the adapter chip (`r8152`, `ax88179_178a`, `asix`, `cdc_ether`, `cdc_ncm`) from the kernel log.
D. **Lockdown noise.** After a restart of a signed image, `iyakki dev dmesg --grep Lockdown` shows no new `debugfs access is restricted` line from `iyakki-startup-`.
Record each result in the F-DEV-10 / F-DEV-18 rows (Manual `passed <date> <machine>` or `failed KI-nnn`); do not raise Test above 50 for anything that ran only on fakes.

## Update 2026-11-16 (owner)
The USB Ethernet adapter route was verified by the owner on real machines: it works. Only the USB-C to USB-C direct link (Thunderbolt/USB4, one cable between two computers) is still pending a cable rated for Thunderbolt or a Thunderbolt dock/drive.
