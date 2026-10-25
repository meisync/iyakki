# Battery and charging on laptops

IYAKKI shows the battery in the top bar **only when the computer has one**. A desktop, a virtual machine and a Mac mini show
nothing. Everything is in plain words and nothing here starts a program: the numbers are read from the kernel's own files.

**Status: built and tried with made-up `/sys` folders only. Nothing was run on a real laptop after the kernel change. The kernel
change needs a rebuild of the PC, Mac and `hw` images (see "Kernel options").**

## What is shown

* **In the top bar** (launcher, apps, and the bar of the panes view), left of the clock: a small battery drawn by IYAKKI itself
  and the percentage beside it. The fill is green, amber under 20 % and red under 10 %. A white **bolt** means charging. A white
  **plug** means the power adapter is in and the battery is full, or is deliberately not charging (battery protection, which many
  laptops do at 80 %). A **question mark** means the level is not known (or the kernel has no battery driver, see below). When
  the bar is narrow the percentage is dropped first, then the icon; on a phone-sized screen the apps' header has no room, and the
  Devices popover (F8) and Settings, Battery have the same facts.
* **Click it, or press F4**, for the popup: a bar with the percentage, the status (*Charging*, *Discharging*, *Full*, *Plugged in
  but not charging (battery protection)*), the time left or until full, health, charge cycles, technology, maker and model when
  the battery says them, the number of batteries when there are two, and whether the power adapter is connected. *Battery
  settings* (or Enter) opens Settings, Battery. Esc, the x or a click outside closes it.
* **Settings, System, Battery** (laptops only): the same bar and facts and three switches: *Show the percentage in the top bar*
  (default on), *Low battery warnings* (default on), *Shut down at the critical level* (default on; administrators change this
  one; the two display switches are for everyone and are the machine's, not per person).

## What is not a battery

A battery is a power supply of type `Battery` that the kernel does not mark `scope: Device` and that is present. A Bluetooth mouse,
a keyboard, a game pad or a stylus reports a battery too; on kernels that do not set the scope, a name like `hid-...`, `hidpp_...`
or `...controller` is treated the same way. An empty second bay (`present` 0) is not counted. Two batteries (BAT0 and BAT1) are
added up, weighted by their full capacity (30 Wh at 100 % plus 10 Wh at 0 % is 75 %, not 50 %).

## Warnings and the shutdown

| Charge, on battery | What happens |
|---|---|
| 20 % | A short message once (and the notification sound). |
| 10 % | A message once. |
| 5 % | A **Plug in now** card at the top of the screen (OK closes it; plugging in closes it too). |
| 3 % or the firmware says *critical* | A card with a **30 second countdown** and *Cancel shutdown* (also Esc). At zero the computer shuts down cleanly, the same safe power-off the power menu uses, and the audit log records *battery.critical* and *power.off*. |

Each level speaks once; it speaks again only after the charge went at least 3 points above it, or after the adapter was plugged
in. If several levels are crossed between two readings, only the deepest one is said.

**The shutdown never happens** on a machine without battery data (no battery, no driver, status unknown), on a reading of 0 or a
negative or garbled number with nothing else behind it, while charging, while any power adapter is connected, when the person
pressed *Cancel* (until the charge recovers or a cable is plugged in), or when the switch is off. It needs two critical readings in
a row before the countdown starts, and the countdown is timed with a steady clock, so setting the time over the network during the
count cannot shorten it. A cable cancels it at once (the adapter's file is read every second while the card shows).

## Where the data comes from

`/sys/class/power_supply/*` (`type`, `status`, `capacity`, `capacity_level`, `present`, `scope`, `energy_now`, `energy_full`,
`energy_full_design`, `power_now` or the `charge_*` and `current_now` variants, `voltage_now`, `cycle_count`, `technology`,
`manufacturer`, `model_name`, and the adapter's `online`), `/sys/class/dmi/id/chassis_type` and `/sys/bus/acpi/devices/PNP0C0A:*`.
Negative, zero-capacity and absurd values are ignored. The reading is made every 10 seconds while the shell is on screen (60 s
when nobody is looking, or when there is no battery at all) and the adapter file every second; no program is started.

* **Time left** = energy now / power now (or charge now / current now), smoothed over the last readings (restarted when the status
  or the adapter changes). It is shown only when the draw is a believable one, the answer is between 1 minute and 72 hours (48
  hours until full) and the battery is discharging (or charging for the time until full).
* **Health** = the full capacity now against the design capacity, at most 100 %; not shown when either number is missing or odd.

### "No battery driver: this image needs a newer kernel"

When the ACPI battery device `PNP0C0A` exists (and says a battery is inserted, or the chassis is a laptop) but the kernel made no
`BAT*` for it, the bar shows a question mark and the popup and Settings say exactly that. That is what the MacBook Pro 2020 showed:
only `ADP1` (the adapter) existed, because the images' kernel had no ACPI battery driver.

## Kernel options (a kernel rebuild is needed)

In `dist/buildroot/board/iyakki/linux.fragment` (PC, `hw` and, because it includes it, the Mac image): `CONFIG_PM=y`,
`CONFIG_POWER_SUPPLY=y`, `CONFIG_POWER_SUPPLY_HWMON=y`, `CONFIG_ACPI_AC=y`, **`CONFIG_ACPI_BATTERY=y`**, `CONFIG_ACPI_SBS=y`,
`# CONFIG_ACPI_PROCFS_POWER is not set`, `CONFIG_ACPI_BUTTON=y`, `CONFIG_ACPI_VIDEO=y`.
In `dist/buildroot/board/iyakki-aarch64/linux.fragment` (generic arm64): the same ACPI ones, plus `CONFIG_BATTERY_SBS=y`,
`CONFIG_BATTERY_BQ27XXX=y` and `CONFIG_BATTERY_BQ27XXX_I2C=y` (the two fuel-gauge chips tablets and small ARM laptops use). Apple
Silicon (Asahi) already has `CONFIG_CHARGER_MACSMC=m` in its own fragment. Charger chips and SoC-specific gauges are board
specific and are not listed: a board that needs one adds it. `CONFIG_APPLE_GMUX` and `CONFIG_SENSORS_APPLESMC` do not provide the
battery on the Intel MacBooks (the ACPI battery does); `applesmc` stays for fans and temperatures.

Images built before this change have **no battery driver**: the indicator then shows a question mark with the note above on a
laptop, and nothing on a desktop. They need the next kernel build.

## After the rebuild: how to check on a laptop

```text
ls /sys/class/power_supply                     # expect ADP1 (or AC) and BAT0
cat /sys/class/power_supply/BAT0/{status,capacity,energy_now,energy_full,power_now}
cat /sys/bus/acpi/devices/PNP0C0A:00/status    # 31 = battery inserted; 15 = none inserted
dmesg | grep -i -E 'battery|ACPI: AC'
```

If `BAT0` is still missing with status 31, the battery's ACPI methods failed: send `dmesg | grep -i acpi` and the model
(`cat /sys/class/dmi/id/product_name`).

## Not supported yet

* Per-app power use, power profiles (performance / balanced / saver) and suspend or hibernate: none exist in IYAKKI yet. Low
  battery handling is a warning and a clean shutdown, nothing else.
* Charge limits (stop at 80 %), battery calibration and wear history.
* Phone-sized screens in an open app have no room for the chip (the Devices popover and Settings have the facts).
* Wireless devices' batteries (a mouse, headphones) are not shown.
* Battery data for machines whose battery reports nothing the kernel understands (some ARM boards): the question mark.

Code: `crates/iyakki-agent/src/battery.rs` (engine, tests in `battery_tests.rs`), `crates/iyakki-shell/src/batterybar.rs` (icon,
chip, popup, cards), `shell_battery.rs` (heartbeat and shutdown), `apps/settings_battery.rs` (the page). Rows F-BATT-01 to
F-BATT-05 in [FEATURES.md](../FEATURES.md).
