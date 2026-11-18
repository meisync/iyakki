# Opt-in QEMU proofs for the Linux-only pieces

**For:** whoever has a Linux PC with a built IYAKKI image and wants to see the pieces that unit tests on a Mac cannot run actually run.
**You will get:** five scripts, what each needs, the exact command, and what PASS means.

These scripts are **not** part of `./run.sh alpha-check`, `cargo test` or any default gate. Nothing calls them unless you run them. Each one boots the
built image in QEMU (a guest, never your real disks and network settings) and decides from the guest's serial output. They were written on a
Mac that could not run them. **Run for real on a Linux build host (Ubuntu 26.04, KVM) on 2026-11-14 (M80): all four PASS**, which raised F-LINAPP-13, F-INST-13, F-BOOT-30 and
F-BOOT-31 to Test 70 (`firewall-boot-test.sh` and `install-entry-test.sh` ran the same day: F-FW-09 and F-INST-10) (the runs and what they found are in each row's Notes: a missing `CONFIG_FANOTIFY`, a selftest that could never print `MemoryShort`, a GRUB menu
without its RAM disk after the installer's copy, one wrong script expectation).

## What every script needs

* a Linux PC with `qemu-system-x86_64`; KVM (`/dev/kvm` writable) is strongly recommended, without it raise `TIMEOUT` (seconds per guest run, default 240);
* `python3`;
* a built image: the build output folder with `images/bzImage` and `images/rootfs.cpio.zst` (the scripts take the folder as their first argument,
  default `target/buildroot-libre`). The scripts for compressed memory and the BIOS disk also need `images/iyakki-install-x86_64.img` and
  `target/debug/iyakki-usb` (`cargo build -p iyakki-usb`).

## The session checks (one boot, many rows)

`./run.sh test-qemu` (`dist/qemu/session-checks.py`, list in `session-checks.list`) boots the image once, pairs through developer mode and runs named checks over the serial log, the dev API and the guest's shell, with PASS or FAIL per feature row. See `docs/development/QEMU-TESTS.md`.

## The scripts

| Script | Proves | Command | Takes |
|---|---|---|---|
| `sandbox-net-test.sh` | F-LINAPP-13: the outbound-only network on a real kernel | `dist/qemu/sandbox-net-test.sh target/buildroot-libre` | about 1 minute |
| `boot-prefetch-test.sh` | F-BOOT-30: boot-order recording and the prefetch | `dist/qemu/boot-prefetch-test.sh target/buildroot-libre` | about 2 to 3 minutes |
| `compressed-memory-test.sh` | F-BOOT-31: the experimental compressed-memory plan at boot | `env IYAKKI_USB=target/debug/iyakki-usb dist/qemu/compressed-memory-test.sh target/buildroot-libre` | about 5 minutes, 6 GB of temporary space |
| `firewall-boot-test.sh` | F-FW-09: the firewall is applied at boot, before listeners, and `iyakki.firewall=off` skips it for one boot | `dist/qemu/firewall-boot-test.sh target/buildroot-libre` | about 3 minutes |
| `bios-shared-disk-test.sh` | F-INST-13: `--bios` on a shared disk, a BIOS boot, the restore | `env IYAKKI_USB=target/debug/iyakki-usb dist/qemu/bios-shared-disk-test.sh target/buildroot-libre` | about 5 minutes |
| `firewall-boot-test.sh` | F-FW-09: the firewall is applied before the network at boot, `iyakki.firewall=off` | `dist/qemu/firewall-boot-test.sh target/buildroot-libre` | about 3 minutes |
| `install-entry-test.sh` | F-INST-10: the install stick's "Install to a disk" entry shows the installer on the text console (needs `tesseract`) | `dist/qemu/install-entry-test.sh target/buildroot-libre` | about 30 seconds |
| `homebind-test.sh` | F-STORE-09: /home is a bind mount of the data volume and a file in it survives a power-off | `dist/qemu/homebind-test.sh target/buildroot-libre` | about 15 seconds (needs an image with the `homebind` boot stage) |
| `drive-loss-test.sh` | F-LOSS-01/02/04: a USB data disk is pulled (banner on the real screen) and plugged in again (reattach with real mount calls) | `dist/qemu/drive-loss-test.sh target/buildroot-libre` | about 1 minute (needs `tesseract` and an image with the `driveloss` boot stage) |
| `boot-counter-test.sh` | F-UPD-02: the update boot counter read and written by a real GRUB (guard, arm, fall back to the previous version, mark good) on a copy of the install stick; shows whether the built image's GRUB has `loadenv` | `dist/qemu/boot-counter-test.sh target/buildroot-libre` (`GRUB_FROM_DEFCONFIG=1` for a UEFI GRUB made from the defconfig's module list) | about 2 minutes (needs mtools, an install stick image, cargo; OVMF for the defconfig mode) |
| `startup-disk-test.sh` | F-NET-17: `iyakki startup-disk` status, register, first and revert against OVMF's real UEFI variables (efivarfs) in a guest started through OVMF with the kernel as an EFI stub | `dist/qemu/startup-disk-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `startupdisk` stage) | about 1 minute (needs OVMF, iyakki-usb, zstd, cpio) |
| `sound-output-test.sh` | F-SND-02: the sound output list and the automatic choice on a real kernel's /proc/asound with emulated HDA and USB audio cards, jacks read through the image's amixer | `dist/qemu/sound-output-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `sound` stage) | about 1 minute (needs zstd and cpio only for IYAKKI_BIN) |
| `computer-profile-test.sh` | F-HW-04: one install on two computers: the computer is recognised from real DMI strings (QEMU -smbios), its settings kept apart, a new computer starts from the defaults and the old one gets its values back | `dist/qemu/computer-profile-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `computer` stage) | about 1 minute (needs iyakki-usb) |
| `usb-awake-test.sh` | F-NET-15 (mechanism only): PCI USB controllers put in `auto` are switched to `on` by the usbawake step on a real kernel's sysfs | `dist/qemu/usb-awake-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `usbawake` stage) | about 30 seconds |
| `fnkeys-test.sh` | F-KBD-01: media keys pressed through the QEMU monitor arrive as the Fn keys of the table through the shell's own evdev reading and key state | `dist/qemu/fnkeys-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `fnkeys` stage) | about 1 minute |
| `pointer-speed-test.sh` | F-KBD-20: a QEMU mouse moved through the monitor, scaled by the pointer speed (100, 50 and 200 percent) through the shell's evdev reading | `dist/qemu/pointer-speed-test.sh target/buildroot-libre` (`IYAKKI_BIN=...` for an image without the `pointer` stage) | about 1.5 minutes |
| `unclean-shutdown-test.py` | F-SYS-05: a power cut (SIGKILL of QEMU) is noticed at the next boot (Settings -> Overview note, the kept kernel log, I have seen it), a clean power-off from the power menu is not; found that the session record was not synced | `UNCLEAN_WAIT=8 IYAKKI_BIN=PATH-OF-A-MUSL-IYAKKI python3 dist/qemu/unclean-shutdown-test.py target/buildroot-libre --out DIR` | about 9 minutes (needs iyakki-usb, Pillow, tesseract) |
| `dev-pair-test.py` | F-DEVMODE-15, F-DEV-16, F-DEV-04, F-DEVMODE-16 (and pairing of F-DEVMODE-02, F-DEVMODE-05; dev screenshot, files ls, shell --tty after an on-screen grant and approval): developer mode switched on in the real shell by keyboard, the pairing code read by OCR, the firewall allow pressed, then the built `iyakki dev pair` from the host (a wrong code refused, the right one paired) and `iyakki dev whoami` through a forwarded port | `IYAKKI_BIN=PATH-OF-A-MUSL-IYAKKI python3 dist/qemu/dev-pair-test.py target/buildroot-libre --out DIR` | about 3 minutes (needs iyakki-usb, Pillow, tesseract) |
| `shell-pages-test.py` | F-DISP-06, F-SND-04, F-KBD-02, F-CLOUD-01, F-DISKS-01, F-DISKS-06, F-DOCTOR-02, F-LOCK-01 and pages of F-HW-04, F-FW-06, F-SYS-01, F-CHECKUP-01: the real shell on a virtual screen, driven by keyboard (account, Settings pages, volume keys) and read by OCR; two boots (with and without a sound card) | `python3 dist/qemu/shell-pages-test.py target/buildroot-libre --out DIR` | about 7 minutes (needs iyakki-usb, Pillow, tesseract) |

Use `sudo` only if your QEMU needs it (`QEMU="sudo qemu-system-x86_64"`); the scripts never need root themselves.

## What PASS means

Each script prints `ok: ...` for every check and ends with a line `PASS (<name>)` and exit code 0. Any failed check prints `FAIL: ...`, keeps the work
directory and its logs (the path is printed) and exits 1. Exit 2 means a missing tool or image (nothing was tested).

* **sandbox-net-test.sh**: the guest runs `iyakki.selftest=sandboxnet`. It makes a throw-away process in a new network namespace and joins it to the guest
  with the same code a sandbox uses (`net_up`): the table `inet iyakki_sbx_0007` loaded, the `ivh...` end of the pair present, IPv4 forwarding on.
  Then from inside: the allowed destination (a listener on your PC, seen from the guest as `10.0.2.2:PORT_A`) **connects**; a second listener that is not on
  the allow-list **is refused**; the guest's own address on the pair **is refused**. Then teardown: no table, no `ivh` interface, forwarding back to what it
  was. The guest prints `IYAKKI-SELFTEST sandboxnet check <name> PASS|FAIL <detail>` lines and `... RESULT PASS n/n`; `selftest-result.py` reads them.
  Not covered: the sandbox's file-system side (`pivot_root`), which the `#[ignore]` tests in `linuxapps_tests.rs` cover on a Linux PC with a static busybox
  (`cargo test -p iyakki-agent --lib linuxapps_tests::a_real_sandbox -- --ignored --test-threads=1`, as root).
* **boot-prefetch-test.sh**: `iyakki.selftest=bootlist iyakki.bootrecord=1`. After 90 seconds the list `/var/lib/iyakki/boot-order.list` exists and holds at least
  20 boot files, none under `/proc /sys /dev /run /tmp`; a prefetch of that list ends with `ended Done`; with 100 MiB reported free it ends at once with
  `MemoryShort` and 0 batches. Not covered: that `iyakki.prefetch=on` starts the prefetch by itself on a second boot, and whether it makes a real boot faster.
* **compressed-memory-test.sh**: A and B (no data disk): zram is the swap, zswap is off, no swap file, with and without `iyakki.zram=exp`. C (an installed disk,
  `iyakki.zram=exp`): zswap is on, `/var/lib/iyakki/swapfile` (536870912 bytes, a quarter of the guest's 2 GiB) is in `/proc/swaps`, and zram is not. D (same
  disk, setting off): zram, zswap off and the swap file run C made is not in use (turning the setting off does not delete it). Not covered: the Settings page and the chat tool (unit-tested), and whether zswap helps on a stick.
* **bios-shared-disk-test.sh**: a 4 GiB MBR disk file with a foreign partition and foreign boot code does not start IYAKKI (control). After
  `iyakki-usb --plan --mode free-space --bios --bios-save S --bios-replace-boot-code` the foreign partition's first MiB is byte for byte unchanged, a save file
  exists, sector 0 has new boot code, and under QEMU's **BIOS** (SeaBIOS) the disk starts IYAKKI (`supervisor up`, the data volume mounted). The restore
  (`--bios-restore S --target-file DISK`) puts the old 440 boot-code bytes back and empties the gap. Not covered: a real legacy-BIOS PC, GPT disks, the foreign
  system's own menu entry (GRUB does not chainload it).

## The result lines

`selftest-result.py LOG MODE` reads the guest's lines (format in `crates/iyakki-core/src/selfcheck.rs`; both ends are tested together by
`cargo test -p iyakki-core --test qemu_scripts`): it prints each check, then `PASS`, or `FAIL: <why>` when a check failed, the RESULT line is missing (the guest
did not finish, for example a timeout) or the RESULT disagrees with the checks. Exit codes 0, 1, 2 as above.

## Tell us what happened

When you have run them, say which passed on which machine; the rows in `FEATURES.md` get `passed <date> <machine>` in Manual and Test can then rise.
A FAIL with its log is just as useful: each of these code paths has never run on Linux before.
