# Update fault injection and the upgrade proof (beta gate 5, lane U1)

**For:** whoever runs the QEMU level of the autotest or files a report. **You will get:** the cases, how each one is made, what must happen after it, where the evidence goes, and the findings.

## The cases (rows F-UPD-27 to F-UPD-30)

| Fault | Unit (`tests_update_faults`, no boot) | QEMU, real GRUB (`dist/qemu/update-faults-test.py`) | Hardware (`checklists/F-UPD-30.json`) |
|---|---|---|---|
| Power cut while the pack is copied | yes | not (the stick path has no copy step) | step 1 |
| Power cut in the initramfs, the kernel, the menu write | every byte of the sweep | `cut-initrd`, `cut-kernel`, `cut-menu` | step 2 |
| Cut after the system image pointer switched, before it, inside a pointer write | yes | no (needs the data volume) | no |
| New version never reaches its first picture | yes | `done` (nobody marks it good) | step 3 |
| Cut before it is marked good | yes | `done` | step 4 |
| Rollback, and its own failed start | yes | no | no |
| Damaged component, before and after staging | yes | no | no |
| Signature from another key, or over a changed manifest | yes | no | no |
| Full data disk, full boot partition | yes | no | no |
| Older version without a yes | yes | no | no |

After every fault the machine must run within three starts and the second at the latest runs the saved version by itself. The QEMU script checks the entry GRUB chose (the kernel's own command line) and that the kernel started each time.

## Upgrade and rollback on real packs (`dist/qemu/upgrade-test.py`, F-UPD-29)

Old pack: the alpha.7 pack of the green run `run-20261121-123102-26faa589`. New pack: the newest `~/iyakki-work/green/run-*/pack`. The update centre installs the new one signed with a throw-away key, keeps the old kernel and initramfs whole, arms and marks the counter, then rolls back. Run it on the build host only, one at a time, never while the gate runs.

## Report format

Both scripts write `OUT/<ROW>/qemu/result.json` through `dist/autotest/results.py` (level `qemu`, status, evidence files beside it, `not_covered` list) with the serial logs and a text summary; `./run.sh autotest --profile qemu --image BUILD --rows F-UPD-28,F-UPD-29` runs them through the scenario files (`host-script:` header) and reads the records back. Default OUT: `~/iyakki-work/autotest/<date-time>/`. The hardware level is the checklist; its human records go to `OUT/human/`.

## Run results (Q6, 2026-11-23 and 24, the AMD host, evidence in `~/iyakki-work/autotest/q6-faults/` and `q6-after/`)

| Run | Result | What decided it |
|---|---|---|
| `update-faults-test.py` on the stick as it is (Q1) | control passes; done, cut-initrd, cut-kernel, cut-menu FAIL: counter 1, starts normal, normal, normal | the stick's BIOS GRUB has no `loadenv` module: its menu says "error: can't find command `load_env'" (screen dump of a QEMU start of the stick copy), so the guard never runs and `save_env` never writes |
| `boot-counter-test.sh` on the same stick (no update engine involved) | FAIL at start 1: counter stays 1, expected 2 | the same cause: the failure is in GRUB, not in `apply` |
| `update-faults-test.py --grub-core` (BIOS core made from the defconfig's list) | all five PASS: control normal/normal/normal; done, cut-initrd, cut-kernel, cut-menu normal/previous/normal, counter 0 at the end | the product code is right: arming before the first boot file changes works with a real GRUB that has `loadenv` |
| `upgrade-test.py` on the real packs | before: FAIL (Idle, expected Armed); after: PASS | two causes, both in the harness' view of a pack, see finding 4 and 5 |

## Findings

0. **The decision (Q6): the QEMU failures were not a bug of U1's `apply`; the cause is the GRUB inside the image.** `repack.sh` copies `grub.img`, `grub-eltorito.img` and the UEFI file from the kernel donor. The donors on the build host (`buildroot-libre`, `-mac`, `-web`) were built between 2026-11-04 and 2026-11-10, before `loadenv` was added to the module lists (commit of 2026-11-14), and the stick under test carries exactly the donor's `grub.img` (same bytes). A GRUB without `loadenv` cannot run `load_env` or `save_env`, so the counter never stepped and no recent image could fall back by itself in BIOS mode. Evidence: the screen of the stick copy (`error: can't find command load_env`), the same failure of `boot-counter-test.sh` (which does not touch the update engine), and the pass of all five cases when only the BIOS core is replaced by one made with the defconfig's list. Not a wrong assumption of the script: the script's check (`iyakki.entry=previous` on the kernel command line after a counter 1, 2, 0 sequence) is the one `boot-counter-test.sh` has used since M80 and it passes with a GRUB that has the module. The scripts' check stays as strict as before.
   Fix: `dist/buildroot/grub-refresh.sh` makes the BIOS images (and a plain UEFI file) again from the donor's GRUB build tree with the defconfig's module lists, the same `grub-mkimage` call Buildroot makes (checked: byte for byte equal to the donor's own image on the donor's own lists); `repack.sh` runs it on the staged images, so the age of the donor no longer decides. Refused when the defconfig's list has no `loadenv`. Test: `crates/iyakki-core/tests/grub_refresh.rs`. The scripts take `--grub-core FILE` (update-faults) or `GRUB_CORE=FILE` (boot-counter) to test a stick that was made before, and say in the failure line what a counter that never steps means. Still open: a full image built from a current defconfig run through `boot-counter-test.sh` on its own BIOS GRUB (KI-994), the Secure Boot chain (`secureboot.sh grub-image` reads the module list of the tree's own `.config`, which is stale in an old donor: refresh the tree before a signed build), and the ISO's `fat.efi`.

1. **Fixed.** The boot counter started only after the update finished. A power cut while the boot files were replaced left a default entry with a mismatched kernel and initramfs and nothing that chose the saved version except the person at the menu. `apply` now arms the counter before the first boot file changes (when it is idle) and restores the environment block when it finishes; the update centre arms it for the first start as before. Tests: `a_power_cut_at_any_byte_of_the_boot_file_replacement_still_reaches_a_start_that_runs_on_its_own`, `an_update_that_finishes_leaves_the_counter_idle_for_a_direct_apply_and_armed_for_the_update_centre`.
2. **KNOWN-ISSUES draft (not filed, owner triage).** After a fall-back the guard sets the counter to 0 and the next ordinary restart starts the failed set again (one failed start, then the saved version again). The shell tells the person on each fall-back. A fix needs the running saved version to rewrite the menu default; it touches the boot menu and needs the real GRUB run first.
3. **Observation.** A cut between the system image pointer switch and the kernel replacement starts the new root with the old kernel until the retry; the counter is armed, the shell says the update did not finish.
4. **Fixed (found by the upgrade run, Q6).** The packs of the green runs name the menu folder `boot/GRUB` and the file `GRUB.CFG` (the way a FAT volume lists them). On a case-sensitive folder the counter looked for `boot/grub/grubenv`, found no folder, and refused to arm, so the update centre's `arm_counter` said "will not fall back by itself" and the upgrade proof stopped at `Idle`. On the FAT volume of a real stick both spellings are the same folder, so the bug showed only with a folder laid out from a pack. `bootcount` now uses the spelling that is there (as it does for the menu file). Test: `tests_bootcount::a_folder_with_capital_letters_the_way_a_fat_volume_lists_a_pack_is_armed_and_read_in_the_folder_that_is_there`.
5. **Test assumption fixed (Q6).** The real packs of the 2026-11 builds carry no `boot/iyakki-release`, so after the update the boot partition still names the old version; the running system reads its release text from its own system image. The upgrade test now builds the new system's release text from the manifest when the partition has none of that version, so the first start of the new version marks itself good as it does on a machine. Nothing was loosened: the same assertions (new kernel stands, counter armed, old version kept whole, marked good, rollback puts the old pair back) run.
6. **Product question left for the owner.** An update pack without `boot/iyakki-release` leaves the partition naming the old version until the next start; `Running::installed` is empty for it. The update centre copes (the running version is read from the system image), but the person-facing text "version on disk" is then the old one. Not changed here: why these builds write no `boot/iyakki-release` (`post-image.sh` copies it only when `etc/iyakki-release` exists in the target tree) was not looked at.

## Loader layout (LDR2, row F-UPD-31, 2026-11-24)

The images are loader-default since 2026-11-07: `EFI/BOOT/BOOTX64.EFI` is `iyakki-boot`, GRUB is only `grub-recovery.efi` (the Recovery row), and the loader keeps the version an update replaces in `EFI/iyakki-boot/prev/` (`bzImage`, `initrd`, one marker `set-plain` or `set-oldroot` with `stamp N`). The loader counts starts that nobody confirmed (UEFI variable `BootTries`, `tries 9` in `boot.cfg`, refilled by `BootConfirm` or by a new stamp) and then starts the previous version by itself. GRUB's counter (the sections above) is not what decides here. `dist/qemu/update-faults-test.py --loader` tests this path.

How it works: the same real engine (`update_fault_state_on_a_folder_for_a_real_grub_run`) runs on a folder with the stick's boot files and `EFI/iyakki-boot/boot.cfg` (the room check uses the real 256 MB volume size), everything it leaves in the folder, half files of a cut included, is written back into the FAT volume of a stick copy, the copy's `boot.cfg` gets `loglevel=7` (the kernel prints its command line) and `iyakki.entry=previous` on the previous rows, then the real loader starts it under OVMF with a vars file of its own per case (the tries survive resets). The power-cut knob now also covers the loader's copy (kernel, initramfs, marker). Start by start the script kills QEMU when the kernel prints its command line (so nobody confirms a start) and keeps the loader's `last-boot.log` (F-LDR-09) and the serial log of every start. The script holds `~/iyakki-work/.qemu.lock` for the starts of one case.

| Case | Cut where | Files of the volume after it | Expected starts |
|---|---|---|---|
| control | no update | old, no `prev/` | current x3, no previous row |
| done | none; finished, nobody confirms | new set, `prev/` = old set, marker stamp 1 | current x9 (tries 8..0), previous at start 10 |
| cut-initrd | while the initramfs is replaced | old set (the new file is beside it), `prev/` whole | as done |
| cut-kernel | while the kernel is replaced | new initramfs + old kernel (a mixed set) | as done |
| cut-menu | while the menu is written | new set | as done |
| cut-prev-kernel | inside the copy of the old kernel | old set, `prev/bzImage.tmp` half, NO marker | current x3, no previous row |
| cut-prev-initrd | inside the copy of the old initramfs | old set, `prev/initrd.tmp` half, no marker | same |
| cut-prev-marker | at the marker | old set, `prev/` whole, no marker | same |

PASS needs, per case: the files named above (checked from the engine's own listing, the cut position is therefore a checked fact, not an assumption), `prev/` byte for byte the old set when a marker exists, the loader's tries counting down one by one, the fall-back at start tries+1 with `iyakki.entry=previous` on the kernel's command line, and a kernel command line printed at every start.

### Finds (found by the unit sweep `a_power_cut_at_any_point_of_the_loaders_copy_or_the_replacement_never_leaves_a_previous_row_over_half_a_copy_and_the_machine_starts`, fixed in `update.rs`)

1. **Fixed: a retried update kept the half-replaced files as the previous version.** After a cut between the replaced initramfs and the kernel, the next `iyakki dev update` (power is back) copied what stood on the boot volume (new initramfs + old kernel) as the "previous version", for the data volume's copy (GRUB's entry) and for the loader's row. The fall-back would then have started a set that never existed. Now an update that finds its own unfinished marker and the whole saved copy keeps that copy (and its version) and feeds the loader's row from it.
2. **Fixed: a retry after the system image pointer had switched wrote the loader's row (`set-plain`) and GRUB's entry for the NEW root.** The old kernel would have started on the new root. The retry now keeps `set-oldroot` and `iyakki.root=previous`. Test: `a_power_cut_after_the_system_image_switched_and_a_retry_still_keeps_the_old_set_for_the_old_root_in_the_loaders_row_and_in_grubs_entry` (fails without the fix, checked).
3. **Script assumptions fixed.** The first `cut-kernel` formula left out the menu write and the update marker, so the cut fell at the end of the initramfs and the run reported `files=old` (the script said so and the case FAILED; the check was not loosened, the cut was moved).
4. **Checked, nothing wrong:** a cut inside `prev/` leaves the old files standing, `bzImage.tmp` or `initrd.tmp` without a marker, so the loader's row (it `requires` the marker) is never offered over half a copy; the marker is written last through a rename; the next update removes the leftovers first.
5. **Observation, no bug.** With `tries 9` a new set that fails costs nine counted starts before the loader falls back (GRUB's counter costs one). That is the loader's design (tries survive a slow first start); a person at the menu can pick the previous row at once.

Not covered here: real hardware (checklist F-UPD-30), the shell writing `BootConfirm` in a started guest, shim and MOK, the cut between the system image pointer and the kernel in QEMU (the stick has no data volume; the unit test above and F-UPD-27 cover it), corrupt component and signature mismatch (identical on both layouts, F-UPD-27 unit tests: they refuse before anything is written).

### Run results (2026-11-24, this PC, evidence `~/iyakki-work/autotest/ldr2-full/` and `ldr2-kernel/`)

Stick: the 2026-11-10 `buildroot-libre` install image (loader-default, `tries 9`). The first full run: 7 of 8 PASS, `cut-kernel` FAIL (find 3 above, a script position). After the fix `cut-kernel` PASS (`ldr2-kernel`). Sequences (entry/tree per start, loader's `tries_left`): control 3 x current (8, 7, 6); done, cut-initrd, cut-kernel, cut-menu 9 x current (8 down to 0), start 10 `prev-plain` on the previous tree (tries 8), kernel command line with `iyakki.entry=previous`; cut-prev-* 3 x current, prev/ listing `bzImage.tmp:6291456`, `bzImage + initrd.tmp:35651584`, `bzImage + initrd` and no marker. The engine's own `FAULT-STATE` line for each case is in the report text `F-UPD-31/qemu/update-faults-loader.txt`.
