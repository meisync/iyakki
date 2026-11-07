# Own boot loader: plan

Update 2026-11-04 (later): the menu under the logo with "More options", the shim chain with MOK and SBAT (QEMU Secure Boot test passes), the boot confirmation and the safe update are built; the plan to make the loader the default and remove GRUB from UEFI images is `docs/development/GRUB-REPLACEMENT-PLAN.md` in the IYAKKI repository (GRUB stays for BIOS until a BIOS stage exists).
Status: idea from the owner 2026-11-04 ("our own futuristic boot loader that supports the old stack as well"). Steps 1 to 3 of section 4 are built and tested in QEMU/OVMF (2026-11-04); not yet run on real firmware, with a trusted Secure Boot key, or on the Mac. See the status lines in section 4.
Today the chain is firmware, shim (Secure Boot), GRUB (menu and script in `dist/buildroot/board/iyakki/grub.cfg`), kernel, first stage
(`iyakki-stage1`). Read with `docs/development/SELF-PREPARING-BOOT.md` and `docs/BOOT-SPEED.md` in the IYAKKI repository.

## 0. Separate project (owner, 2026-11-04)

This folder (`iyakki-boot/`) is kept apart from the IYAKKI workspace so it can be checked in, pushed and open-sourced as its own
repository, under `MIT OR Apache-2.0` (owner decided 2026-11-04: more adoption; users pick either, GPLv2-only projects can use the MIT side).

* Its own Cargo workspace, excluded from the IYAKKI workspace; it depends on no IYAKKI crate and no IYAKKI crate depends on it.
* No IYAKKI names, paths or keys inside the code: the image id, volume labels (`IYAKKI-BOOT`, `IYAKKI-DATA`), signing key and entry
  names come from a config file, so any Linux distribution can use the loader.
* IYAKKI uses it only as a built, signed binary placed on the boot volume by `dist/buildroot` (a copy step, like the firmware pack).
* Split to its own repository later with `git subtree split --prefix=iyakki-boot` (keeps history).

## 1. Why

* One program we understand end to end, written in Rust like the rest, replacing GRUB's script language for the few things we use:
  pick a tree (`current`, `previous`, seed), pick the lean or full start, pass a command line, show a menu.
* Boot decisions that GRUB cannot make cleanly: count failed starts and roll back by itself, check the image id, pick a recovery entry
  when the keyboard or touch screen is held, show a touch-friendly menu.
* Control of the display hand-over (the Mac mini touch monitor problem starts at the splash to shell hand-over; a loader that sets one
  framebuffer and keeps it removes one modeset from the chain; to be measured, not assumed).
* Smaller: GRUB plus modules is several MB; a loader that only reads FAT, loads a kernel and an initrd can be well under 1 MB.

## 2. Shape

* **Two front ends, one core.** `iyakki-boot-core` (no_std Rust): config parser, entry choice, boot counting, command-line rules, menu
  model, tests on the host. The UEFI and BIOS programs are thin shells around it.
* **UEFI (the modern stack).** An `x86_64-unknown-uefi` application (also `aarch64`, same code). It reads the FAT boot volume through
  the firmware's file protocol, then starts the Linux EFI stub with `LoadImage` / `StartImage` and passes the initrd through the
  `LINUX_EFI_INITRD_MEDIA_GUID` protocol. No kernel boot-protocol code of our own, so the stub keeps working with Secure Boot.
* **Legacy BIOS (the old stack).** A boot sector plus a second stage (assembly start, then Rust): real mode with INT 13h extended reads,
  a small FAT reader, protected then long mode, kernel loaded by the Linux boot protocol (bzImage, initrd address, command line). Runs on
  old PCs and in QEMU without OVMF. Text menu first, VESA framebuffer later.
* **Raw bindings, no big dependency.** The UEFI tables we use are written in-house (a few hundred lines) so the loader stays Apache-2.0
  with nothing else to audit; the `uefi` crate is MPL-2.0 and is not used.
* **Secure Boot.** Same signing key and MOK path as now (`secureboot.sh`, `project-secure-boot-signing-key`): shim loads our signed
  loader instead of GRUB. The loader never loads an unsigned kernel when Secure Boot is on (it uses `LoadImage`, so firmware checks).
* **Config.** One small signed-or-hashed text file on the boot volume (`boot/iyakki.cfg`), written atomically by the updater as
  `grub.cfg` is today. A broken file falls back to a built-in entry (`current` then the seed), never to a prompt.

## 2a. Other systems on the machine (owner, 2026-11-04)

The loader scans the machine and offers every other operating system it finds, so IYAKKI can be the menu on a shared or dual-boot PC.

* **UEFI:** list the EFI system partitions on all disks (GPT type GUID) and the boot entries in firmware (`BootOrder`, `Boot####`). Offer
  `EFI/Microsoft/Boot/bootmgfw.efi`, other `EFI/*/*.efi` loaders and `EFI/BOOT/BOOTX64.EFI` by `LoadImage` / `StartImage` (chain load). Linux
  installs with `/boot/loader/entries/*.conf` (BLS) or their own `grub.cfg` can be started through their own loader the same way.
  macOS (Apple boot file) is listed and started through the firmware.
* **BIOS:** read the boot sector of each disk and active partition (`0x55AA`), offer it and chain load it (copy to 0000:7C00, jump); Windows
  and NTFS-based systems start through their own boot sector without us reading NTFS.
* **Names and order:** names come from the loader files and OS release files we can read (FAT only at first: no NTFS/ext4/btrfs reader
  in the loader at the start, so Linux on ext4 is found through its ESP entry rather than by reading its root). The owner can rename,
  hide, reorder and choose a default; the choice lives in `iyakki.cfg`. Entries are rescanned at every start (cheap) so a new disk shows up.
* **Safety:** scanning only reads; nothing on another disk is written. Chain loading under Secure Boot works only for signed loaders
  (Windows is; unsigned ones are shown greyed with the reason). BitLocker or other disk encryption is left to the other system's loader.
* **Boot once:** "boot this once" sets a one-shot variable, so a firmware menu round trip is not needed; the updater never edits another
  system's files.

## 3. Futuristic parts (after the basics work)

1. Touch and mouse menu with big targets (GOP framebuffer, built-in bitmap font, no theme files); keyboard stays complete.
2. Boot assessment: the loader writes `boot-tries`, the system clears it once the supervisor is up; three failures switch to `previous`
   and show why on the next good start (replaces the `iyakki-lean-failed` marker dance).
3. Measured boot: extend a TPM PCR with the kernel, initrd and command line when a TPM exists; the vault can later bind to it.
4. Hold-a-key recovery entries: network boot a recovery image, touch-screen probe (`iyakki.touchscreen=probe`), safe graphics, previous.
5. Fast start: read only what is needed (lean initrd about 1 MB), start the kernel from the loader without a second menu timeout.
6. Same loader on aarch64 UEFI (Pi 4/5 with UEFI firmware, Apple Silicon VM) so one menu and one update path serve every tier.

## 3a. Splash (2026-11-04)

The distribution supplies the picture, the loader ships none: config keys `splash`, `splash-bg`, `splash-ms`; an in-tree BMP decoder in
`boot-core`; drawn once before the menu on the existing GOP mode, kept on screen through a normal start, menu drawn over the same
background only when asked for. Done and tested in QEMU/OVMF (test h). Open: real firmware, the Mac mini, and whether the Linux
framebuffer driver keeps the picture across the hand-over on those machines (measure; see the README).

## 3b. Display hand-over (2026-11-04)

Why: on a Mac mini with a USB-C touch monitor the monitor's USB hub drops about 1 s after the kernel's GPU driver takes the screen. The
loader is not the proven cause (the same happens under GRUB), but it must not add to it. What the loader guarantees:

1. It never sets a GOP mode (`gfx.rs` only reads `Mode` and blits with `BufferToVideo`), never blanks, never resets the GPU.
2. It starts the kernel with `LoadImage`/`StartImage`, so the kernel's EFI stub reads the same GOP (base, size, stride, pixel format) and fills
   `screen_info`; efifb/simpledrm then show the firmware picture without a modeset, and i915 later inherits it (fastboot) when the hardware state matches.
3. Nothing is drawn after `StartImage` and no boot services call touches the display path in between (no `SetMode`, no `ConOut` mode query).
4. Open and testable only on the Mac: whether firmware (Apple) leaves the link trained for the same mode i915 asks for. If the drop remains
   with the loader, the loader cannot fix it: the retrain happens in i915's first commit, after the loader is gone. A loader-side change is
   therefore not justified by the evidence; none was made. QEMU (`tools/qemu-test.sh`) only proves points 1 to 3, not the monitor.

## 4. Order

1. `iyakki-boot-core` with tests (config, entry choice, counting, rollback). Done (`crates/boot-core`).
2. Done in QEMU (`crates/boot-uefi`, `tools/qemu-test.sh`; details in README): UEFI app that boots the existing lean and full entries in QEMU/OVMF next to GRUB (selectable as an extra entry first); then the scan
   of other EFI systems and chain loading (test with a second ESP holding a fake loader in QEMU: done; one real Windows or Linux disk: open).
3. Signing script and test done (2026-11-04 later: SBAT tool, shim chain, MOK-signed kernel through `Security2Arch` + `ShimLock`, QEMU Secure Boot test: see `docs/SIGNING.md`, sections 4 to 6) (`tools/sign.sh`, `docs/SIGNING.md`); Secure Boot with a trusted key and the Mac mini test are open: Secure Boot signing and the Mac mini test as an extra boot option on the T7 (GRUB stays the default until it passes).
4. Updater and installer write `boot.cfg`; GRUB kept as fallback entry for one release. Done in part: the updater saves `loader.prev.efi` and writes the loader folder by temp name + rename; the generated `boot.cfg` has the second level; IYAKKI confirms a good start. Open: the image wiring that makes the loader the default (GRUB as `grub-recovery.efi`).
5. BIOS path (boot sector, second stage), tested in plain QEMU and on one old PC.
6. Touch menu (done in layout and unit tests, rows >= 44 px at 1080p; the first real touch test is open), TPM, recovery entries (the second level has safe graphics, remote diagnostics, hardware check, previous loader, GRUB recovery, UEFI firmware setup).

## 5. Risks

* Firmware differences: Mac EFI, USB drivers and early display behave differently per machine. Mitigation: keep GRUB as a fallback
  entry until the loader has booted every machine in the test list.
* BIOS code is the hardest to debug and the least valuable per hour; do it after UEFI, and keep GRUB for BIOS if it slips.
* A boot loader bug makes a machine unbootable; every image is boot-tested in QEMU before it reaches a stick (existing rule).
* Signing: a second signed binary means the same key must be used or MOK must be enrolled again.

## 6. Open questions for the owner

* Should the scan of other systems be on by default, or only after the owner switches it on in Settings?
* Is BIOS support needed for the first release of the loader, or may it follow UEFI?
* Replace GRUB entirely, or keep it as a recovery entry on every image?
* Is a TPM-bound vault a goal, or only measured boot logging?
