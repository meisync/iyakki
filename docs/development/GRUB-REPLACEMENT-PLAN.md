# Making the own boot loader the default, and removing GRUB from UEFI images

Status: plan and first implementation, 2026-11-04. Owner request the same day, after IYAKKI was installed on the Mac mini's internal disk and the
loader had booted that Mac once as a test entry: "We can plan to remove GRUB if all set? Provide a GRUB-like More option under the logo. With MOK
support." Read with `docs/BOOT-LOADER.md` (what is on the image), `iyakki-boot/docs/PLAN.md` (the loader's own plan), `iyakki-boot/docs/SIGNING.md`
(signing, shim, MOK) and `docs/SECURE-BOOT.md` (the image side).

## 0. Decisions in short

1. GRUB is removed from UEFI images only, and only after the exit criteria in section 1 are met on the listed machines. Not before.
2. Legacy BIOS keeps GRUB. The loader has no BIOS stage yet (`iyakki-boot/docs/PLAN.md` step 5). Until that exists, every image that must start on
   a BIOS PC (the live ISO, the install stick) carries GRUB for BIOS. State it plainly: **no BIOS stage, no GRUB removal for BIOS.**
3. The step from "test entry" to "default" is done by file names, not by new boot mechanisms: shim keeps loading a second stage called `grubx64.efi`
   (the name is only a name); on a loader-default image that file is the loader, and GRUB is kept under another name, `grub-recovery.efi`, as a
   recovery entry reachable from the loader's "More options". GRUB leaves the image in step 3, after one release with the recovery entry.
4. The loader opens a menu by itself when starts are not confirmed. IYAKKI now confirms a good start (section 4), so this protection works.
5. The owner's key stays the only key: it signs shim's second stage (the loader), the kernel and the kernel modules. MOK enrolment is unchanged
   (section 6).

## 1. Exit criteria: when the loader becomes the default, and when GRUB goes

The loader may become the default entry of an image when ALL of these are true; GRUB may be removed from a UEFI image one release later, when
all are still true and nobody needed the recovery entry.

| Where | What must work | Status 2026-11-04 |
| --- | --- | --- |
| Mac mini 2018 (T2), internal disk, Secure Boot "Full Security" | firmware picker shows the entry, shim, loader, kernel, sign-in screen; Option-key picker lists it | not tried; the loader booted this Mac once as a test entry (Secure Boot off) |
| Mac mini 2018 (T2), "Medium Security" and "No Security" | same chain; with No Security the loader started directly as `EFI/BOOT/BOOTX64.EFI` also works | not tried |
| A T2 MacBook | same as the Mac mini, with the built-in keyboard (T2 keyboard through the loader's text input) and touch bar absent | not tried |
| A plain UEFI PC, Secure Boot off | `EFI/BOOT/BOOTX64.EFI` is the loader; keyboard, USB keyboard, mouse | QEMU only |
| A plain UEFI PC, Secure Boot on, key enrolled in MOK | shim, signed loader with SBAT, MOK-signed kernel, sign-in screen | QEMU with Secure Boot firmware: passes (section 7); real PC not tried |
| A plain UEFI PC, Secure Boot on, key NOT enrolled | shim's blue MokManager screen, enrol, next start works | QEMU: shim stops, MokManager follows (existing test 6 of `qemu-secureboot-test.py` covers the key presses for GRUB) |
| A PC with a USB-C or HDMI touch monitor | menu by touch, rows at least 44 px at 1080p | layout tested; touch not tried |
| Legacy BIOS PC | GRUB stays | no loader BIOS stage: **GRUB stays for BIOS** |

Other conditions:

* The loader's menu has every start GRUB's menu has (a test compares them: `crates/iyakki-core/tests/boot_menu.rs`), plus previous loader,
  recovery GRUB and UEFI firmware setup.
* Boot counting works end to end: a start that reaches the sign-in screen is confirmed, a start that never does is counted, and after `tries` starts
  without confirmation the menu opens and waits (section 3).
* Update and install write the loader safely (sections 4 and 5), and `iyakki dev update` of a loader-default image was done once on the Mac mini
  with the loader replaced and the previous one still selectable.
* The loader's startup time on the Mac (USB boot and internal disk) is not worse than GRUB's. Measure with `docs/BOOT-SPEED.md`'s method.
* One full release (alpha or beta) on the owner's machines with the recovery entry present and no recovery needed.

## 2. Transition steps

| Step | What | State |
| --- | --- | --- |
| 1 | The loader is a GRUB test entry ("IYAKKI - new boot loader (test)") that chain-loads `EFI/iyakki-boot/loader.efi` | done; booted the Mac mini 2018 on 2026-11-04 |
| 1b | The loader is complete enough: menu under the logo with "More options", same starts as GRUB, SBAT, shim chain, boot confirmation, safe update | done in code and QEMU, 2026-11-04 (this plan) |
| 2 | The loader becomes the default: shim's second stage (`EFI/BOOT/grubx64.efi`) or, without shim, `EFI/BOOT/BOOTX64.EFI` is the loader; GRUB stays as `EFI/BOOT/grub-recovery.efi`, hidden from the firmware, reachable from "More options" of the loader ("Recovery (GRUB menu)") | image wiring DONE 2026-11-04 as the build option `IYAKKI_LOADER_DEFAULT=1` (default 0: GRUB stays the default of the alpha images): `dist/buildroot/loader-default.sh`, called by `post-image.sh` and `lean-image.sh`; update path checked by tests; QEMU runs below passed; NOT on any real machine yet: the machines in section 1 are still to do |
| 3 | GRUB is removed from UEFI images (no `grub-recovery.efi`, no GRUB modules and theme on the ESP; the loader's "Recovery" row disappears because its file is missing) | after one release of step 2 |
| 4 | BIOS decision: (a) keep GRUB for BIOS for good (the cheap answer; a BIOS image is a separate small GRUB-only boot sector set), or (b) write the BIOS stage (boot sector + real-mode stage, `PLAN.md` step 5) and then remove GRUB everywhere | open; owner decides after step 3 |

What step 2 changes on the image, exactly:

```
shim path (Secure Boot capable images; this is the Mac mini and most PCs):
  EFI/BOOT/BOOTX64.EFI        shim (Microsoft-signed), unchanged
  EFI/BOOT/mmx64.efi          MokManager, unchanged
  EFI/BOOT/grubx64.efi        the LOADER: SBAT section added, signed with the IYAKKI key   (was: GRUB)
  EFI/BOOT/grub-recovery.efi  GRUB (signed, as today)                                       (new name)
  EFI/IYAKKI/iyakki-mok.cer   the certificate MokManager enrols, unchanged
  EFI/iyakki-boot/loader.efi  the same signed loader (the updater's copy; "previous version" is loader.prev.efi beside it)
  EFI/iyakki-boot/boot.cfg, splash.bmp
plain path (no shim, Secure Boot off; internal disks without shim):
  EFI/BOOT/BOOTX64.EFI        the loader (unsigned is fine with Secure Boot off)
  EFI/BOOT/grub-recovery.efi  GRUB
```

The loader reads its config from `/EFI/iyakki-boot/boot.cfg` (compiled in, `BOOT_CONFIG_PATH`), so it behaves the same whatever its own file name
is. GRUB's own files (`boot/grub/grub.cfg`, theme, fonts) stay where they are in step 2, because `grub-recovery.efi` needs them.

Image tooling for this: `dist/buildroot/loader-default.sh esp ROOT` (the file names above; idempotent; signs with `secureboot.sh loader-image`
when `IYAKKI_SB_KEY` / `IYAKKI_SB_CERT` are set, else leaves the loader UNSIGNED and says so: Secure Boot off only) and `loader-default.sh grubcfg FILE`
(drops GRUB's "new boot loader (test)" entry, which a GRUB-default build keeps). `iyakki-boot/tools/add-sbat.sh`, `sign.sh` are unchanged. The
exact owner steps for the Mac mini's internal disk, the rollback and the T2 / MokManager notes are in `docs/BOOT-LOADER.md`
("The loader as the default").

What was run on 2026-11-04 without hardware (copy of `iyakki-lean-x86_64.img`, QEMU + OVMF, driver `iyakki-boot/tools/qemu_default_test.py`,
screenshots looked at; the original image's SHA-256 was checked unchanged afterwards):

| Run | Result |
| --- | --- |
| Secure Boot OFF, plain path (loader as `BOOTX64.EFI`, GRUB as `grub-recovery.efi`, no shim) | loader started, kernel started from `/boot/initrd-lean`, IYAKKI's first-run "create the administrator account" screen |
| Secure Boot OFF, shim path (shim, unsigned loader as `grubx64.efi`) | same, to the same screen |
| Recovery row, both layouts, Secure Boot OFF | GRUB's themed menu ("IYAKKI / More options...") appears. FOUND AND FIXED on the way: the loader refused `kind chain` rows of its own config unless the scanner had found them ("this kind of system cannot be started by the UEFI loader"); `boot-uefi/src/main.rs` now starts a chain row from the loader's own volume. This also fixes "Previous version of this boot loader" |
| Secure Boot ON, `qemu-secureboot-test.sh` (throwaway keys, shim -> signed loader as `grubx64.efi` -> signed kernel) | 10 of 10 checks passed (the loader and kernel run; unsigned or wrong-key loader, wrong-key or unsigned kernel, nothing enrolled, and a loader without `.sbat` are refused) |
| Secure Boot ON, the real loader-default layout built by `loader-default.sh` with a throwaway key (loader, GRUB and kernel signed by it, key in MOK) | "boot: loader started, secure boot on", kernel started, sign-in screen reached; Recovery row started the signed GRUB (GRUB 2.12 menu on screen). The kernel's owner-signed modules are refused under the throwaway key, so one service ("Firewall") reported "did not start": an effect of the test key, not of the layout |

The owner's key was not used in any of these. Still only the Mac can show: the firmware picker, T2 security levels, real Microsoft-signed shim on that
firmware, MokManager's key screen with the loader, boot time against GRUB, the T2 keyboard in the loader menu.

## 3. Rollback and recovery

*What remains before GRUB can be dropped (step 3), 2026-11-07:* (a) one release on the owner's machines with the loader-default image: the
previous-version row started from the boot volume copy by a real kernel (Mac mini, HP, Lenovo), and an unconfirmed update falling back by itself;
(b) `BootConfirm` written by IYAKKI proven on real hardware (F-LDR-06), because a start that is never confirmed rolls back after 9 tries;
(c) the Install row started through the loader on real install media; (d) the Secure Boot case: the copy is booted by the signed loader, kernel
module signing unchanged; (e) the full-RAM-disk install (no room for a copy) decided: shrink, or accept no loader row; (f) GRUB-only features that
are not carried (Compatible USB mode variable, lean-failed fallback, the `iyakki_trial` counter, which the loader's tries replace) accepted;
(g) the update's `boot/grub/*` handling made optional when GRUB is gone. BIOS (step 4) is separate.


Layers, from the cheapest to the last resort:

1. **Previous loader.** Every update that replaces `EFI/iyakki-boot/loader.efi` first saves the old one as `loader.prev.efi` (temp name, rename; section
   5). The loader's "More options" lists "Previous version of this boot loader" only when that file exists. It chain-loads the old loader, which reads
   the same `boot.cfg` and offers the same starts.
2. **Boot counting.** The loader writes a counter (`BootTries`, default 9) before it starts an entry by itself (default, countdown). IYAKKI confirms
   a good start (section 4); the loader reads the confirmation at the next start and refills the counter. A start that never reaches the
   sign-in screen is not confirmed. With the paths in today's `boot.cfg` there is no `{tree}`, so "current" and "previous" start the same files:
   9 unconfirmed starts switch to "previous" (same files), 9 more stop the loader from starting anything by itself, and it opens its menu and waits with
   the reason on screen ("the newest and the previous system both failed to start several times; choose what to start"). That is 18 starts.
   Choosing an entry by hand never spends a try.
   **Update 2026-11-07:** with the keyed rows below the loader now reaches the previous KERNEL itself and falls back to it by itself (see
   `docs/BOOT-LOADER.md`, "Rows that GRUB's recovery job needs"): the update copies it to `EFI/iyakki-boot/prev/` on the FAT volume (the
   loader reads FAT only; an ext4 reader was rejected as too big and risky for the boot path) and the default row's `previous` line names the rows.
   What GRUB's recovery still does that the loader does not (open): the proof on a real machine and the full-RAM-disk case (too big for the
   volume, then no row), see section "What remains before step 3".
3. **Recovery (GRUB menu).** On a loader-default image `EFI/BOOT/grub-recovery.efi` is the old GRUB with its whole menu, including the previous
   kernel and RAM disk that `iyakki dev update` keeps on the data partition ("IYAKKI - previous version ..."). The loader reads FAT only, and the
   data partition is not FAT, so the previous KERNEL is reached through GRUB, not through the loader. This is the reason GRUB stays one release.
4. **Safe graphics, remote diagnostics, hardware check, compatible USB mode** and all other starts are in "More options" and work as before.
5. **The firmware.** "UEFI firmware setup" in "More options" restarts into the firmware's own setup (`OsIndications`). On a Mac, hold Option at power
   on: the Apple picker lists every EFI entry and any USB stick; Internet Recovery (Cmd-Opt-R) is the last resort. A second stick with the previous
   image always works.
6. A loader bug that makes the machine unable to start the loader at all is the real risk; that is what the QEMU tests, the signing test and the
   one-release rule are for, and why step 3 waits.

## 4. IYAKKI confirms a good start

Code: `crates/iyakki-core/src/bootconfirm.rs`. Called from `crates/iyakki-core/src/main.rs` in the shell thread right after the boot screen is
handed over to the shell (`finish_boot_screen()`); a background thread waits 2 seconds (the sign-in screen is the first frame the shell draws) and
then calls `confirm_good_start(&Efivarfs::system())`.

* What is written: the UEFI variable `BootConfirm` (value `ok`, attributes non-volatile + boot services + runtime = 7) under the loader's vendor GUID
  `7b5a3c1e-2d84-4f6b-9a0e-5c1f8d3b2a47`, through `/sys/firmware/efi/efivars/BootConfirm-<guid>`.
* What the loader does with it: at its next start, `BootConfirm` present means `BootState::mark_good` (tries refilled, the reason of an earlier
  rollback is told once), then it deletes the variable.
* Trait: `VariableSink { available, write }`; tests use a fake sink and a temp directory standing in for efivarfs. Not a UEFI start (BIOS,
  container, test) gives `NotUefi` and writes nothing. A failed write is logged, never fatal.
* Not done: the confirmation does not wait for the person to type a password (it is the sign-in screen being drawn, not a sign-in), which matches
  what the loader can know: "the system came up far enough to draw".

## 5. Update path

`iyakki dev update` and update packs write files under `boot/` and `EFI/` of the boot partition (`crates/iyakki-agent/src/devmode/update.rs`).

* The pack builder (`crates/iyakki-usb/src/bootpack.rs`, `iyakki dev update-pack IMAGE`) copies every file under `boot/` and `EFI/` of the image's boot
  volume, so `EFI/iyakki-boot/{loader.efi,boot.cfg,splash.bmp}` are in a pack as soon as the image has them; and the updater accepts those paths
  (`valid_path`). Nothing in the builder had to change. (Not covered by a unit test of its own: the in-tree FAT fixture has no sub-folders.)
* New in `update.rs`: files in `EFI/iyakki-boot/` are never written in place. The old `loader.efi` is copied to `loader.prev.efi` first (temp name,
  rename); every file there is written under a `.upd` name and renamed; with too little room the update stops with a clear error instead of
  overwriting. Tests: `an_update_keeps_the_previous_boot_loader_beside_the_new_one`, `a_power_cut_while_the_loader_is_written_leaves_the_old_loader_whole`
  (the cut is swept over the whole write), `the_loader_folder_is_recognised`.
* On a loader-default image the loader also exists as `EFI/BOOT/grubx64.efi` / `BOOTX64.EFI`. These are replaced by the same rename rule by the
  existing code (`replace_file` renames beside the old file when there is room). Each path holds either the old or the new COMPLETE signed loader,
  and both read the same `boot.cfg`; so any power cut leaves a bootable machine. Order in a pack: `EFI/iyakki-boot/*` before `EFI/BOOT/*`.
* Secure Boot: the new loader in a pack is already signed by the image build (`loader-image`), so no signing happens on the machine.

## 6. Install path and the T2 specifics

* Install: the installer (`crates/iyakki-usb`, `install.rs`, `gpt_install.rs`, `fatclone.rs`) clones the boot volume of the image, so the loader,
  `boot.cfg` and the splash arrive with the rest of `EFI/` (nothing to add). Section 2 decides which file names they carry. The installer also
  names the volume for the Apple picker (`efi::name_running_disk`).
* Apple firmware (T2): the boot picker lists an EFI entry by the label of its volume, and starts `EFI/BOOT/BOOTX64.EFI` (or the file of a firmware
  boot entry). So the volume label stays `IYAKKI-BOOT`, and the loader must work as `EFI/BOOT/BOOTX64.EFI` directly (no shim; Secure Boot "No
  Security", or "Medium" with the Apple/Microsoft database) AND as shim's second stage (Full Security with the Microsoft CA enabled in the T2
  startup security utility). Both are the same signed binary under two names.
* The T2 keyboard and trackpad are not USB devices the firmware can always read; the loader reads keys through `ConIn` (what the firmware
  exposes) and the pointer through `SimplePointer`. On the Mac mini with an external USB keyboard this works; on a T2 MacBook the built-in keyboard
  reaches `ConIn` only in the firmware's own menus. If a MacBook shows no key response in the loader, the countdown still starts the default, and
  an external USB keyboard works. To be measured on the real machine (section 1).

## 7. MOK: how the loader works in the shim chain (done and tested in QEMU)

* shim loads its second stage by NAME (`grubx64.efi`) and verifies it against the Microsoft database, its own built-in certificate and the MOK list.
  The loader is that file: signed with the IYAKKI key (`tools/sign.sh`, the key is never printed or copied) and carrying an SBAT section
  (`iyakki.loader,1,...`; only our own component name, the same rule as for GRUB in `secureboot.sh`). shim 15.3+ refuses a second stage without
  `.sbat`; the QEMU test shows this (case 7).
* The kernel: the loader starts it with `LoadImage`/`StartImage` and the initrd through the `LINUX_EFI_INITRD_MEDIA_GUID` `LoadFile2` handle (the
  initrd is not signed, exactly as with GRUB). **Finding of the QEMU test:** shim's security override is NOT active for an image the second stage
  loads. A MOK-signed kernel was refused with `access denied` at first. So the loader wraps the firmware's `EFI_SECURITY2_ARCH_PROTOCOL.
  FileAuthentication` for the time it starts something (`crates/boot-uefi/src/shim.rs`, the same method as systemd-boot): the firmware's own check
  runs first and its "yes" stands; on "no", shim's lock protocol (`ShimLock.Verify`) judges the bytes (MOK list, shim certificate, SBAT) and its "yes"
  lets the image load. Nothing is allowed that neither the firmware nor shim trusts. Without shim, or with Secure Boot off, nothing is installed.
* MOK enrolment is unchanged: `EFI/IYAKKI/iyakki-mok.cer` is enrolled once through MokManager (`mmx64.efi`, "Enroll key from disk"; the image's
  `README-SECURE-BOOT.txt` lists the steps). The same key signs shim's second stage (the loader), the kernel and the kernel modules. A person who
  already enrolled the key for the GRUB images does nothing new. A changed key means enrolling again.
* Tests: `iyakki-boot/tools/qemu-secureboot-test.sh --kernel BZIMAGE` (QEMU, OVMF Secure Boot firmware, throwaway keys, `virt-fw-vars` puts the
  test key in the MOK list as MokManager would). Verified in QEMU: signed loader with SBAT plus MOK-signed kernel boots to the initramfs with
  "Secure boot enabled"; unsigned loader refused; loader signed by an un-enrolled key refused; kernel signed by another key refused; unsigned
  kernel refused; nothing enrolled stops at shim; a signed loader without `.sbat` is refused by the shim used. Not verified (needs hardware): the
  Microsoft-signed shim of the Mac's and PCs' real firmware, T2 startup security levels, the key presses of MokManager with the loader (the
  existing GRUB test drives them; the loader changes nothing about them).

## 8. Open questions for the owner

* Step 2 default: are the first-level rows "IYAKKI" and "More options" enough, or should "Safe graphics" be on the first level too (it is one tap
  away today, in "More options")?
* "Scan other systems" is off in the generated config (`IYAKKI_LOADER_SCAN=yes` turns it on at image build). Turn it on once it has been tried on the
  Mac mini's internal disk with macOS or a Windows partition present?
* BIOS: keep GRUB for BIOS for good (step 4a), or write the loader's BIOS stage (step 4b)? Step 3 does not depend on the answer.
