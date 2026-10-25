# Self-preparing boot: one entry, a full first boot, a fast start after it

**Status: DESIGN ONLY (2026-10-24, owner request). Nothing in this file is built.** Every statement is tagged **EXISTS** (read in the
code or docs, path given), **GAP** (missing today) or **PROPOSED** (a decision for the owner). Numbers are tagged measured (and
where) or estimated. No Rust, shell or build file was changed to write this; `FEATURES.md` is not touched.

## 1. What the owner asked for

1. The GRUB menu has **one** start entry, `IYAKKI`. A GRUB script decides between the fast start (lean first stage, `boot/initrd-lean`,
   tree on the data volume) and the full start (`boot/initrd`, 173 MB).
2. The first boot after an install or an update is a full boot. During or after it the running system builds its own fast-start tree
   on the data volume and then sets a "fast start ready" marker. Every later boot is the fast start, by itself.
3. The special entries move under "More options..." and follow the same rule, or are forced full.

## 2. What exists today (the starting point)

| Piece | State | Where |
|---|---|---|
| First stage `/init`: finds `IYAKKI-BOOT` (FAT) and `IYAKKI-DATA` (ext4), reads `iyakki/root/current`, then `previous`, then seed `boot/root.sfs`; tmpfs overlay; patch layers; moves the volumes into the new root | EXISTS | `crates/iyakki-stage1/src/main.rs` (`run`, `present`, `patch_layers`), `lib.rs` (`candidates`, `Pointer`, `plan_patches`) |
| Failure: writes `boot/grub/iyakki-lean-failed`, shows text, restarts after 30 s; a lean start that reaches "switching root" removes it | EXISTS | `main.rs` `fail`, `lean_marker` |
| Tree copy with manifest, hard links to the `current` tree, `.<id>.tmp`, verify, rename, `previous` then `current` last, prune | EXISTS (Python, build-machine only) | `dist/buildroot/lean-sync.py` (`sync`, `build`, `verify`, `prune`, `write_atomic`) |
| `initrd-lean` (about 1.2 MB), tree, manifest, GRUB block are made from a finished build | EXISTS, separate from the image build | `dist/buildroot/lean-image.sh` (`--root export` or `--root stick`) |
| Lean entries are inserted in front of `IYAKKI` only when `/boot/initrd-lean` exists; failed marker makes `IYAKKI` the default | EXISTS | `dist/buildroot/board/iyakki/grub.cfg` (block `# BEGIN lean-boot` ... `# END lean-boot`) |
| `post-image.sh` writes the same `grub.cfg` into the ISO and the install template; `boot/initrd` is `rootfs.cpio.zst`; `boot/iyakki-release` is copied | EXISTS | `dist/buildroot/board/iyakki/post-image.sh` |
| `mount_data_volume`: skips its own search when the first stage mounted the data volume; otherwise waits, **formats the `pending` partition** (marker `IYAKKI-FORMAT-ME-v1`), mounts at `/var/lib/iyakki`; `iyakki.data=ram` keeps data in RAM | EXISTS | `crates/iyakki-core/src/boot.rs` (`mount_data_volume`), `crates/iyakki-storage/src/lib.rs` (`find_data`, `find_pending`, `format_pending`) |
| System update: refuses RAM-only data, checks boot-volume growth, stages to `boot-next/`, saves `boot-prev/<version>/`, replaces initrd, kernel, rest, `grub.cfg` last; `boot/iyakki-release` says what is installed | EXISTS | `docs/DEVELOPER-MODE.md` section 7, `crates/iyakki-agent/src/devmode/update.rs` (`plan`, `running_from`), `bootmount.rs` |
| Installer clones the stick's FAT volume; `leanstrip.rs` removes the lean block and makes `IYAKKI` the default, because a new install has no tree | EXISTS (another agent is changing it) | `crates/iyakki-usb/src/leanstrip.rs`, `fatclone.rs`, `docs/INSTALLER.md` |
| QEMU harness through UEFI, GRUB, kernel, throttled USB, `snapshot=on` | EXISTS | `dist/buildroot/qemu-lean-test.py`, `qemu-grub-menu-test.py` |
| The image id that GRUB can compare with a tree id | **GAP** (`lean-image.sh` uses the time it ran as id, `ID=${IYAKKI_ROOT_ID:-date}`) | `lean-image.sh` line 43 |
| `initrd-lean` in the normal image build, or on a fresh install | **GAP** (BOOT-SPEED.md: "call the split from `repack.sh`" is a later step) | `docs/BOOT-SPEED.md` "How to build and test" |
| A tree builder that runs on the device | **GAP** (`lean-sync.py` needs Python on a build machine) | no `fastprep` module exists |

## 3. The design in one picture

```
 install / update / new image   ->  boot/initrd, boot/initrd-lean, boot/image-id (= BUILD_ID)       [no iyakki-fast.cfg]
 GRUB:  marker missing or ids differ or failed marker  ->  FULL start (initrd)
 FULL start:  format pending data (if any), setup, first-run, sign-in
              -> later, a low-priority job "fast start prep" copies / to <data>/iyakki/root/<tree>/ (hidden .tmp first)
              -> verify -> rename -> previous, current -> LAST: boot/iyakki-fast.cfg  (set iyakki_fast_id=BUILD_ID)
 next boot:   marker present, id equal, no failed marker  ->  FAST start (initrd-lean + tree)
```

Rule of the whole design: **invalidate first, validate last.** Anything that changes the boot files removes `iyakki-fast.cfg` before it
touches a byte; only the prep job, after everything is verified, writes it. So a power cut anywhere leaves a full boot, never a fast
start with a tree that does not match its kernel (PROPOSED).

## 4. The GRUB script (PROPOSED)

The decision runs once at the top of `grub.cfg`, before the menu. GRUB reads only the boot volume (`$prefix`), no search on the data
volume, so the decision costs nothing on a slow firmware USB driver.

```
set iyakki_fast=0
set iyakki_boot_id=""
set iyakki_fast_id=""
if [ -f $prefix/image-id.cfg ]; then source $prefix/image-id.cfg; fi          # set iyakki_boot_id="BUILD_ID"
if [ -f /boot/initrd-lean -a -f $prefix/iyakki-fast.cfg -a ! -f $prefix/iyakki-lean-failed ]; then
  source $prefix/iyakki-fast.cfg                                              # set iyakki_fast_id="BUILD_ID"
  if [ -n "$iyakki_boot_id" -a "$iyakki_fast_id" = "$iyakki_boot_id" ]; then set iyakki_fast=1; fi
fi
function iyakki_start {   # args: extra kernel words; $iyakki_force_full=1 skips the fast path
  linux __KERNEL_PATH__ $iyakki_usbq console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 $*
  if [ "$iyakki_fast" = 1 -a -z "$iyakki_force_full" ]; then initrd __INITRD_PATH__-lean; else initrd __INITRD_PATH__; fi
}
menuentry "IYAKKI" --id iyakki { iyakki_loading; iyakki_start iyakki.boot=auto }
```

* `iyakki.boot=fast|full` is also passed (the function sets it from the same variables) so the running system knows which path it took
  (the status line in section 9 uses it). Today `/boot/initrd-lean` is tested with `[ -f __INITRD_PATH__-lean ]` (`grub.cfg` line 59);
  `test`'s `-a`/`!` need `insmod test`, which the file already does (line 16 and 58).
* `image-id.cfg` and `iyakki-fast.cfg` are tiny GRUB-syntax files so `source` can read them (a plain pointer file cannot be read into
  a variable by GRUB). They live on the boot volume next to `grub.cfg`, like the existing `iyakki-lean-failed` and `iyakki-usb-compat.cfg`.
* Keep the managed-block markers (`# BEGIN ...` / `# END ...`) around the new decision code under **new names** (for example
  `fast-start`), so the old `lean-sync.py grubstrip` and `leanstrip.rs` still recognise old experiment sticks and do **not** strip the
  new block (conflict with the installer work, section 11).
* Verify with `grub-script-check` on the build machine (tool of the GRUB package; not run here, so unverified) plus the existing
  `qemu-grub-menu-test.py` and `boot_menu.rs` text checks.

### Menu after the change

| Entry | Path | Why |
|---|---|---|
| `IYAKKI` (default, 3 s) | decided by the rule | the only normal entry |
| `IYAKKI - safe graphics` | rule | same system, other kernel words |
| `IYAKKI - Install to a disk` | **forced full** | install mode needs the live payload of the boot volume and must not depend on a tree |
| More options: remote diagnostics, compatible USB mode, show boot messages, NVIDIA (nouveau off), also in a browser | rule | fast and full variants chosen by the rule; the compatible-USB entry keeps its fixed quirk words |
| More options: hardware check, verbose self-test | **forced full** | they test the full initrd's behaviour |
| More options: **IYAKKI - full start (no fast start)** | forced full | the owner's escape hatch; also what a person picks to rebuild |
| `IYAKKI (previous version)` written by `iyakki dev update` | forced full | it loads `boot-prev/<version>/` kernel and initrd (DEVELOPER-MODE section 7) |

The two fast-start entries and "fast start, compatible USB mode" of today disappear as separate rows (`grub.cfg` lines 59 to 73, 159 to 165).

## 5. Where `initrd-lean`, `image-id` come from on a fresh install (PROPOSED)

| Step | Change | Size |
|---|---|---|
| Build | A new small script (`mk-initrd-lean.sh`, the cpio part of `lean-image.sh` lines 46 to 64: stage1 + BusyBox + `libc.so`) is called by `post-image.sh` after `rootfs.cpio.zst` exists and by `repack.sh` after it rebuilds the rootfs. It writes `$BINARIES_DIR/initrd-lean`. stage1 is built with `cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1` (BOOT-SPEED.md) | S |
| Build id | A `BUILD_ID` (build time plus short hash of the rootfs) is written to `/etc/iyakki-release` (already has `VERSION`, `BUILD_DATE`, `VARIANT`; `crates/iyakki-agent/src/relinfo.rs`) and to `install-root/boot/image-id.cfg` and `boot/iyakki-release` | S |
| Template | `post-image.sh` copies `initrd-lean` and `image-id.cfg` into `install-root/boot/` (lines 52 to 56 of `post-image.sh` today copy `bzImage`, `initrd`, `iyakki-release`). `genimage-install.cfg` already stages `install-root/boot` into the 256 MB `iyakki-boot.vfat` | S |
| ISO | The ISO tree gets the same `grub.cfg` but no `initrd-lean`/marker, so it always takes the full path (the live ISO has no data disk: BOOT-SPEED.md "Fallbacks") | none |
| Installer | `iyakki-usb` clones the FAT volume (`fatclone`), so `initrd-lean` and `image-id.cfg` arrive without new code; only the state files must be left out (section 11) | see 11 |
| Update | `iyakki dev update-pack` reads the image's boot partition, so `initrd-lean` and `image-id.cfg` are in the payload by themselves; the update code must add the "invalidate first" step and write `image-id.cfg` with the initrd | M |
| Experiment sticks | `lean-install.sh` / `lean-remote-refresh.sh` keep working but become the maintenance route; on a stick with a different tree id the new rule falls back to full and the job rebuilds, so they migrate by themselves | none |

**Boot-volume space.** The owner's figure: about 19 MB free on the 256 MB stick volume (not re-measured here); 320 MiB internal
(`docs/INSTALLER.md` rule 9 and 10). Added: `initrd-lean` about 1.2 MB (measured, `BOOT-SPEED.md`), `image-id.cfg`, `iyakki-fast.cfg` and
`iyakki-lean-failed` a few bytes each (one 4 KiB FAT cluster or one 32 KiB cluster each, estimated), `stage1.log` at most 64 KB
(`LOG_MAX`). Total about 1.5 MB: fits, but the **update growth rule** (`new total - old total <= free`) must count `initrd-lean` and
write it by temp file plus rename (1.2 MB of temporary room). Nothing else may grow the boot volume; a `root.sfs` seed (220 MB) does
**not** fit and is not part of this design.

## 6. What the "root" is in a full boot, and the two ways to copy it

In a full boot the kernel unpacks `boot/initrd` into the RAM file system: `/` is that RAM disk (BOOT-SPEED.md: 663 MB unpacked, 3400 files).
`iyakki-core` is PID 1 on it. Mounted on top of it: `/proc`, `/sys`, `/dev`, `/run`, `/tmp`, cgroup2, the data volume at `/var/lib/iyakki`,
`/home` as a bind of `<data>/home` (`docs/ADMIN-GUIDE.md` data table, `boot.rs`).

| | **L: copy the live root** (the owner's wording) | **P: unpack the image on the boot volume** (PROPOSED alternative) |
|---|---|---|
| Source | `/` of the running system | `boot/initrd` (zstd cpio) read from the boot volume, streamed |
| Excludes | by device: skip every directory whose `st_dev` differs from `/` (this drops `/proc`, `/sys`, `/dev`, `/run`, `/tmp`, the data volume, `/home`, `/.stage1`); plus a short name list for runtime files that live on the RAM disk itself | nothing to exclude: it is the pristine image |
| Content | includes what the running system changed (machine id, host name, resolv.conf, generated host keys, driver-pack links in `/lib/firmware`, `iyakki dev push` files): **must be diffed against the initrd before it is trusted** | exactly what `lean-image.sh` makes today, so the tree equals the image bit for bit |
| Id proof | none; the tree is only "the root as it was when copied" | `BUILD_ID` of the initrd proves it |
| New code | manifest walk + copy + verify (a Rust port of `lean-sync.py` `make_manifest`/`build`/`verify`) | the same port, plus a cpio reader (the repo already has `ruzstd` in `crates/iyakki-agent/Cargo.toml`; no cpio reader exists: GAP) |
| Cost | reads 663 MB from RAM; no extra USB read | reads 173 MB from the USB boot volume, decodes 663 MB (estimated well under the write time) |
| Risk | a polluted tree boots with the runtime files of one earlier session baked in; hard to notice | none of that; a bigger new module |

Recommendation: **P**, because the whole point of the id rule is "the tree is the image". If the owner wants L, add a mandatory phase 0
test (section 13, phase 0 in section 14) that boots a QEMU full start, runs the job, and compares the tree to `lean-image.sh`'s tree with `lean-sync.py verify`;
every difference must be named in the exclude list. The job's other parts (guards, atomic switch, marker) are identical in both.

Both write the same on-disk layout the first stage already reads: `iyakki/root/<tree>/tree/`, `.lean-complete` (manifest hash),
pointers `current` and `previous` (`lib.rs` `Pointer::parse`: id plus optional `size=`).

## 7. The prep job: when, how much, how safely (PROPOSED)

**Where it lives.** A detached job like the other long jobs (`crates/iyakki-agent/src/jobs.rs`, `quickinstall.rs`; the firmware jobs are the
pattern, `RESTRICTED-FIRMWARE.md` 3a), new module `fastprep.rs` in `iyakki-agent`, started by the supervisor from `late_boot` (the
background half of boot, `boot.rs`; it must stay out of the critical path, BOOT-SPEED.md "Where the time goes").

**When it starts** (all must hold, otherwise it records the reason and does nothing):

| Check | Reason |
|---|---|
| Booted with `iyakki.boot=full` and `/proc/mounts` shows a disk-backed `/var/lib/iyakki` (`storage::data_location` is `Disk`) | RAM-only or no data volume stays full (section 10) |
| First-run finished (an account exists) and the sign-in screen has been up 60 s | do not compete with format, setup, first-run for the USB bus |
| `/etc/iyakki-release` `BUILD_ID` equals `boot/image-id.cfg` (the boot volume is found like `bootmount.rs` / `update.rs` `running_from` do) | an update staged but not rebooted must not build the old tree |
| No `boot/UPDATING` marker, no `boot-next/UPDATING.json` | an interrupted update owns the boot files |
| Setting `fast_start` is not `off` (section 9) | the owner's switch |
| AC power, or battery above a set level; not in a measured memory squeeze | PROPOSED thresholds; battery state exists (`docs/BATTERY.md`), the memory gate exists (`BOOT-SPEED.md`) |
| Fewer than 3 failed prep attempts for this `BUILD_ID` (`iyakki/root/fast.attempts`) | no endless rebuilds of a tree that cannot be written |

**Guards (space and time).**

* Space: need free >= tree size x 1.2 (the rule `lean-install.sh` line 100 already uses) + 1 GiB left for the person. The first tree is
  about 610 MB (BOOT-SPEED.md, extracted size); with `previous` kept, a worst case is two full trees, about 1.3 GB. The
  update path already needs room for `boot-next/` and two `boot-prev/` kernel+initrd sets (about 190 MB each, estimated), so a data
  volume under about 4 GiB gets no fast start unless the owner picks another limit (decision D5).
* Time (estimated, not measured on a stick): 610 MB at 40 MB/s is about 15 s plus hashing both sides; at 10 MB/s about a minute; a later
  tree copies only changed files (hard links for the rest, as `lean-sync.py build` does). Run at idle I/O priority, one file at a time,
  fsync per batch, yield when the foreground reads. Hard stop after 30 minutes: the job ends as "failed: too slow" and retries next boot.
* The job reads nothing outside `/`, writes nothing outside `<data>/iyakki/root/` and the three small files on the boot volume.

**Order of writes (reuse of `lean-sync.py sync`, same rules):**

1. `iyakki/root/.<tree>.tmp/tree` built; files unchanged against `current` are hard links; owners, modes, devices and links from the manifest.
2. fsync, then **verify from fresh reads** (`posix_fadvise DONTNEED`, as `sha_file(drop=True)` does) against the manifest.
3. write `.lean-complete` (manifest hash), fsync directory, `rename` to `iyakki/root/<tree>`, fsync `iyakki/root`.
4. verify again from the final path; only then write `previous` (copy of `current`) and then `current` through a temp file and rename (`write_atomic`).
5. prune: keep `current` and `previous`; remove other trees that carry the marker or are `.tmp` (`prune`).
6. remount the boot volume read-write (as `update.rs` does), write `iyakki-fast.cfg.tmp`, fsync, rename; **delete `iyakki-lean-failed` first**,
   remount read-only. This is the only step that makes the next boot fast.

A cut at step 1 to 5 leaves an old pointer and no marker (full boot; the job resumes and skips what verifies). A cut inside 6 leaves either
the old marker state or the new file complete (FAT rename is a directory-entry write; the file is a few bytes, and GRUB treats a damaged
`source` as "not fast"). **Open check:** FAT rename atomicity on power loss is not proven; the fallback is that a half-written marker fails
the id comparison and gives a full boot (listed in the tests).

## 8. Stage1 changes (kept minimal)

| # | Change | Where | Size |
|---|---|---|---|
| 1 | None needed for the first version of the rule: GRUB decides, stage1 keeps trusting `current`. | | 0 |
| 2 | **Belt and braces (recommended):** when `boot/image-id.cfg` exists, read it and the chosen tree's `build-id` file; on a mismatch call `fail("tree does not belong to this image")`, which writes the existing failed marker. Catches a tree swapped by hand or a GRUB variable gone wrong | `main.rs` after `present()` | S |
| 3 | `lean_marker(None)` currently deletes `iyakki-lean-failed` on the first good fast start. Under the new rule the **job** owns the marker (it deletes it after a rebuilt tree). Remove the delete from stage1, or the first retry loops back into a tree that failed | `main.rs` `lean_marker` | S |
| 4 | Pass `iyakki.boot=fast` information: none; GRUB passes it | | 0 |
| 5 | Log line `stage1: tree <id> build <id>` to `stage1.log` | `main.rs` | XS |

**Conflicts with the other agent's patch-overlay work** (already visible in the working tree: `patch_layers`, `plan_patches`,
`overlay_options_patched`, `PATCHES_DIR`, `parse_layer_info` in `lib.rs`/`main.rs`):

* A patch layer is bound to a **root id** (`layer.txt` `base=`; `plan_patches` skips a layer "made for root X, not this one (obsolete)"). The
  tree directory name is that root id. With this design the id changes on every image; patches built for the old tree are skipped on the new
  one, which is the behaviour they already want, but the patch tool must read the id from `iyakki/root/current`, not from `BUILD_ID`.
  Decision: **the tree directory name stays `<BUILD_ID>`** (and `<BUILD_ID>-r2` for a rebuild); `build-id` inside the tree holds the pure id.
* `main.rs` is edited in the same function (`run`) by both changes; my items 2, 3, 5 touch `lean_marker` and the block after `present()`;
  the patch work touches the block after the tmpfs mount. Disjoint hunks.
* `boot-count` guard of patches (`PATCH_GUARD_MAX`): a fast start with patches that never reaches the system also needs a way back to a full
  start. The failed marker already provides it; the patch guard and the failed marker must not both rebuild the tree.
* **Order:** (1) patch-overlay lands first; (2) this design's items 2, 3 (and 5) as one small commit; (3) GRUB script and build steps (no
  stage1 change); (4) the job. Stage1 is built into `initrd-lean` only, so a stage1 change never needs a new tree.

## 9. What the owner sees, and how to turn it off (PROPOSED)

| Where | Text |
|---|---|
| Boot screen (full start) | unchanged. The existing stage text for the first format stays: "Formatting the data partition (first start only)…" (`boot.rs` `FIRST_START_FORMAT_STAGE`) |
| Jobs list beside the clock | "Preparing the fast start: 43 %" with the bytes and a Cancel button (same job machinery as the firmware packs) |
| Settings -> Storage (and the System tile's About) | one line: `Fast start: ready (build 20261024...)`, `preparing 43 %`, `will be prepared after the next full start`, `off`, `not available: no data volume`, `failed: <reason>`; and the numbers: tree size, free space |
| Settings -> Storage | switch **Fast start** (admin only): On (default) / Off. Off removes `iyakki-fast.cfg` at once (GRUB goes full next time) and offers "Free 610 MB" which deletes `iyakki/root/` after a typed confirm. Audit lines `faststart.prepare`, `faststart.ready`, `faststart.off`, `faststart.failed` |
| GRUB | "More options... -> IYAKKI - full start (no fast start)" always works, whatever the files say |
| Kernel command line | `iyakki.fast=off` (press `e`) makes the supervisor skip the job and treat the marker as off for that boot |

## 10. Cases

| Case | What happens |
|---|---|
| 466 GiB data volume | normal; a tree is a tiny fraction |
| Small data volume (under the limit of D5) | no tree; status "not available: needs N MB free, has M"; always full |
| RAM-only (`iyakki.data=ram`, `data_location` = `Memory`, live ISO, cloud and VM images) | no job; `initrd-lean` is not even on the ISO; always full. Update already refuses RAM-only data (DEVELOPER-MODE section 7) |
| keep-ESP install (`EFI/IYAKKI/INITRD` only, no GRUB) | no GRUB, so no rule: always full (BOOT-SPEED.md "Fallbacks"). The job does not run (no `boot/image-id.cfg`) |
| aarch64 / cloud | unchanged; `lean-image.sh` is x86-64 only |
| Internal disk install, 320 MiB boot volume | same design, more room |
| Second IYAKKI drive attached | the job and stage1 select by label **on the same disk**; stage1's `scan` takes the first match (`main.rs` `scan`): two sticks with the same labels are an existing, unsolved ambiguity, not made worse |
| Install onto the same data volume twice | `iyakki/root/` of an older install is not a valid tree for the new `BUILD_ID`; the rule falls to full, the job writes a new tree and prunes |

## 11. The installer (the other agent is fixing this; this is the contract)

* Today `leanstrip.rs` (`strip_lean_block`, `clean_grub_cfg`) removes the lean block and makes `IYAKKI` the default, because a copied block
  would make the fast start first while no tree exists (its header says so). With the decision script the copied menu is **safe as it is**:
  no `iyakki-fast.cfg` means full.
* What the clone **must not copy**: `boot/grub/iyakki-fast.cfg`, `boot/grub/iyakki-lean-failed`, `boot/stage1.log`. What it **must copy**:
  `boot/initrd-lean`, `boot/grub/image-id.cfg`, `boot/iyakki-release`, the new `grub.cfg` unchanged.
* It must keep stripping the **old** experiment block (markers `# BEGIN lean-boot`) so that a stick that had `lean-install.sh` applied
  installs cleanly; it must **not** strip the new block (different markers, section 4). This is the one place the two agents' work collides;
  agree on the marker names before either lands (decision D8).
* The new disk's `DATA` is the marker block; the first start formats it (`format_pending`), which is exactly the full-boot-first rule.
  Nothing for the installer to do about the tree.
* An installer **update** of a stick or disk (reinstall of the boot volume only) must delete `iyakki-fast.cfg` first.

## 12. First-boot failure paths

| Failure | Result |
|---|---|
| Format of the pending partition fails (`format_pending` error, logged `data: could not format`) | no data volume, RAM session, no job, full every time; the existing error path |
| Data volume found but full or read-only | job records "not available", full |
| Power cut during the job | next boot full; job resumes |
| Job finishes but the first fast start fails (stage1 `fail`) | failed marker written, restart into full, job sees the marker, re-verifies the tree; if verify fails, builds `<BUILD_ID>-r2`; after 3 attempts stops and tells the owner |
| Drive slow or missing in 60 s on a fast start | same as above: stage1 `WAIT_FOR_DEVICES` 60 s, failed marker, restart (existing behaviour, `BOOT-SPEED.md` "No restart loop") |
| `grub.cfg` damaged | `grub.cfg.before-lean` is the old fallback only on experiment sticks; the update code keeps its own copy path (DEVELOPER-MODE section 7); not changed here |
| Update interrupted | `boot/UPDATING` set; marker was removed first; full boot; the shell's red top-bar message stays |

## 13. Tests

**Unit (Rust, no QEMU):** the GRUB decision as a pure function over (`boot_id`, `fast_id`, flags) mirrored by a text test on `grub.cfg`
(pattern of `boot_menu.rs`); exclude rules by device; manifest port against golden output of `lean-sync.py manifest` for a small fixture
tree; hard-link reuse; `verify` detects one flipped byte; pointer switch order with a fake file system that fails at each step (as the
`fatwrite` and `apply` tests do); guard decisions (space, attempts, battery, RAM-only); `write_atomic` leaves no partial file; cpio reader
(only if P) against `cpio -o -H newc` output; `fastprep` refuses names with `..`, absolute links escaping the tree are copied as links
(never followed).

**Script tests:** `lean-install-test.sh` style with fake mount points for the new marker writer; update-pack payload contains
`initrd-lean` and `image-id.cfg`.

**QEMU (`qemu-lean-test.py` + `qemu-boot-speed.py`, image `snapshot=on` except the persistence case which uses a private copy):**

| Scenario | Expect |
|---|---|
| Fresh template (initrd-lean, image-id, no tree, pending data) | boot 1 `iyakki.boot=full`; formats; job; marker; boot 2 `iyakki.boot=fast`, Enter to SSH near the measured 4.7 s (BOOT-SPEED.md) |
| Change `image-id.cfg` on a copy (simulated update) | full again; new tree; old tree kept as `previous` |
| `iyakki-lean-failed` present | full; marker removed by the job |
| Kill QEMU at each step of section 7 (hook `IYAKKI_FASTPREP_PAUSE_AFTER=step`) | next boot full, never fast on a bad tree |
| `iyakki.data=ram`, tiny data volume, full volume | no tree, full, status line says why |
| Break the tree (`--break-tree` exists) | fast start fails, restart, full, rebuilt `-r2` |
| Patch layer present (after the other agent's work) | skipped as obsolete after an image change |

Not testable without hardware: the Mac firmware's USB speed for GRUB reads and the first reads of the tree (BOOT-SPEED.md open risks);
FAT rename atomicity; real-power-cut behaviour of a USB SSD. These go on the Mac checklist.

## 14. Phased plan

| Phase | Work | Effort | Risk |
|---|---|---|---|
| 0 | Measure: diff a booted full root against the unpacked initrd (decides L or P); measure copy time on the real stick; measure free boot-volume space on a current stick | S | L may need a long exclude list |
| 1 | Build: `BUILD_ID`, `image-id.cfg`, `initrd-lean` in `post-image.sh`/`repack.sh`, `genimage` unchanged | S-M | build-time cost of the stage1 cargo build in `repack.sh` |
| 2 | GRUB decision script, menu rework, `boot_menu.rs` and QEMU menu tests | M | menu regressions on a Mac; the compatible-USB entries |
| 3 | Stage1 items 2, 3, 5 (after the patch-overlay work) | S | collision with patch work |
| 4 | `fastprep.rs` job: port of manifest/build/verify, guards, marker writer, audit, jobs list, Settings line and switch | L | the biggest piece; hash/copy bugs; I/O load on a slow stick |
| 5 | Update path: invalidate first, write `image-id.cfg` and `initrd-lean`, growth rule | M | the 19 MB boot volume |
| 6 | Installer contract (section 11), with the other agent | S | marker-name collision |
| 7 | QEMU scenarios above; then a Mac run with the stopwatch table in BOOT-SPEED.md | M | Mac results unknown |

## 15. Biggest risks

1. GRUB must see the same ids the job wrote; one wrong comparison gives either a useless full start every time (safe) or a fast start on
   a mismatched tree (unsafe, guarded by stage1 item 2 and "invalidate first").
2. The 19 MB boot-volume margin on 256 MB sticks.
3. A polluted tree if option L is chosen without the diff.
4. Writing the boot volume from the running system (remount read-write) on a stick that can drop off the bus (`STORAGE-LOSS.md`).
5. Unmeasured: the fast start on the real Mac (BOOT-SPEED.md); it is the reason for the whole feature.

## OPEN DECISIONS (owner)

* **D1** Option L (copy the live root, as asked) or P (unpack the image on the boot volume, recommended)?
* **D2** The tree directory name = `BUILD_ID` (recommended, patches bind to it) or a timestamp as today?
* **D3** Start the job 60 s after the sign-in screen, or only on AC power, or only when idle?
* **D4** Keep `previous` (second tree, up to 610 MB more) or only `current`? Recommended: keep both, as `lean-sync.py` does.
* **D5** Smallest data volume that gets a fast start (suggested: 4 GiB free-after-tree 1 GiB).
* **D6** Default of the Settings switch: On (recommended) or ask once at first-run?
* **D7** Which special entries are forced full (suggested: install, hardware check, self-test, previous version, full start).
* **D8** New marker names for the decision block, agreed with the installer agent before either lands.
* **D9** Remove the stage1 failed-marker delete (item 3) so the job owns the marker (recommended).
