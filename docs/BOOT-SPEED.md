# Boot speed

Goal: the splash within about 3 seconds of choosing the GRUB entry and the sign-in screen within about 8 seconds on a USB 3 SSD.
Status: **first stage, measurement kit and an experimental extra boot entry built; tried in QEMU with the real Mac root file system; not yet in the normal image build and not tried on a Mac.**
The owner decided: "yes lean boot with remote debug for now", then (2026-10-24) "switch to lean mode as normal": the **fast start (lean boot) is now the first and default menu entry** on a stick that has it, with the full boot as the automatic fallback (see "The fast start is the default" below). The sections below keep their original wording where it still holds; where they say "never the default" or "experimental", this paragraph wins.
Every number below says whether it was measured (and where) or estimated.

## Where the time goes

| Part | Who | Seen on the MacBookPro14,1 (owner's measurement) | Can we change it? |
|---|---|---|---|
| Firmware, GRUB menu | Mac EFI, GRUB | not measured; the menu waits 3 s (`set timeout="3"` in `grub.cfg`, kept as is) | the 3 s is a choice, not a limit |
| GRUB reads kernel + initrd (194 MB) from the FAT volume through the firmware's USB driver | firmware | **not measured, the biggest suspect** (the menu text already says "30 seconds to 1 minute") | yes: read less (below) |
| Kernel unpacks the initrd (663 MB) | kernel | 1.15 s (0.26 s to 1.41 s) | yes: nothing to unpack |
| `/init` to splash | IYAKKI | 0.36 s (1.49 s to 1.85 s) | already small |
| Real panel mode switch | GPU driver | 2.4 s kernel clock | no (driver) |
| After the desktop: sound (about 11 s), NTP (33 s), k3s | IYAKKI | background already (`late_boot`) | must stay out of the critical path |

Floor that no change here can remove: the firmware's own start (Mac: several seconds, not ours), reading the kernel (12.7 MB), the kernel's
hardware start, and the panel mode switch.

## Measuring

* **On the lean experiment's entries:** `ssh root@HOST 'sh -s' < dist/buildroot/lean-check.sh` (see "The experiment on a real stick"); `dist/buildroot/qemu-lean-test.py` drives the same through QEMU.
* **On the device** (boot entry "remote diagnostics", `iyakki.ssh=1`): `ssh admin@HOST 'sh -s' < dist/buildroot/boot-timeline.sh`.
  Prints the kernel-clock timeline (kernel start, initramfs unpack, PID 1, splash, sign-in screen, supervisor, sound, network, NTP, k3s),
  the initrd size, IYAKKI's own summary lines, and every gap over 1 s. It cannot see the firmware and GRUB part: **use a stopwatch from
  pressing Enter on the menu entry to the first moving picture** and write both numbers into the table below.
* **In QEMU through the real chain** (UEFI, GRUB, kernel, throttled virtual USB stick, KVM; the image is opened `snapshot=on`, nothing is written):
  `python3 dist/buildroot/qemu-boot-speed.py [IMAGE.img] --mbps 40,120,0 --runs 3`. "grub" is when the firmware starts GRUB, "output" is the
  first line the system prints (kernel loaded and unpacked, IYAKKI running), "super" is `supervisor up`. The span grub to output includes the
  3 s menu wait. The throttle is on the virtual disk's bytes per second; it does not copy the Mac firmware's USB driver (which may be slower
  than the stick, or issue small reads).
* Without GRUB: `python3 dist/buildroot/boot-time.py target/buildroot-macweb` (kernel and initrd handed to QEMU directly).

## Results

QEMU, KVM, 2 vCPU, 3 GB, this development computer (busy with other work, so +-1 s noise), image `macweb` of 2026-10-22, 2 runs, median.
Seconds from the firmware starting GRUB. **Measured.**

| Layout | Disk speed | GRUB to first output | to `supervisor up` |
|---|---|---|---|
| full initrd (today: 173 MB zstd, 663 MB unpacked) | 40 MB/s | 11.4 s | 11.5 s |
| full initrd | 120 MB/s | 9.1 s | 9.4 s |
| full initrd | unthrottled | 8.6 s | 8.7 s |
| lean (first stage, extracted tree on the data volume) | 40 MB/s | 5.5 s | 6.2 s |
| lean | 120 MB/s | 5.5 s | 6.2 s |
| lean | unthrottled | 5.5 s | 6.1 s |

Subtract the 3 s menu wait: the lean layout is about 2.5 s from the entry to the first output in this setup, the full one 5.6 to 8.4 s.
The lean figure does not depend on the stick's speed because GRUB reads about 1 MB of initrd (plus the 12.7 MB kernel) instead of 173 MB.
On a real Mac the firmware reads at an unknown speed: **the saving there is estimated, not measured** (173 MB less to read; at 10 MB/s that
would be 17 s, at 40 MB/s 4 s). Direct kernel boot (no GRUB), kernel clock: the full image reached "Run /init" at 6.6 s in the QEMU run of
2026-10-23 (the owner saw 1.49 s on the MacBook: a faster CPU and no emulated disk); the lean first stage starts at 0.77 s and the splash
is drawn at 2.65 s (1.5 s of that is the emulated USB controller appearing; **measured**).

Other measured sizes (unpacked root 663 MB): `root.sfs` zstd level 1 / 3 / 9 = 235 / 220 / 208 MB (initrd cpio at zstd -19: 173 MB); the first
stage initrd is about 1.2 MB; unpacking the whole squashfs on one core took 0.7 s on this computer (a laptop CPU will be slower;
only a fraction of the files is ever read). The old initrd also carried a 12.7 MB copy of the kernel (`/boot/bzImage`) that nothing in the
running system uses (the installer reads the kernel from the boot volume); the lean root drops it.

## The design (primary): lean initrd, real root read lazily

```
FAT boot volume (IYAKKI-BOOT)             data volume (IYAKKI-DATA, ext4)
  EFI/BOOT, boot/grub, boot/bzImage         iyakki/root/current        "ID" (+ "size=BYTES")
  boot/initrd   (lean: ~1 MB)               iyakki/root/previous       rollback
  boot/root.sfs (seed image, optional)      iyakki/root/ID/root.sfs    or  iyakki/root/ID/tree/  (extracted files)
```

1. GRUB loads the kernel and the lean initrd (unchanged menu entries: `linux ... / initrd ...`, so safe graphics, verbose, hardware check,
   remote diagnostics, install and the others all work the same; the command line reaches the real system).
2. `/init` is `iyakki-stage1` (`crates/iyakki-stage1`, about 450 KB static, no process spawned). It mounts /dev, /proc, /sys, looks (up to 30 s)
   for a FAT volume labelled IYAKKI-BOOT and an ext4 volume labelled IYAKKI-DATA by reading their first sectors, then tries in order: the data
   volume's `current` root, its `previous` root, the seed `boot/root.sfs`. A root image is attached to a loop device and mounted as squashfs;
   a tree is bind-mounted read-only. `iyakki.root=boot` ignores the data volume (last-resort rollback), `iyakki.stage1=shell` stops in a BusyBox shell.
3. A tmpfs holds the overlay's upper layer (`overlay` mount): writes behave as in today's initramfs, but tmpfs pages can be swapped out
   (zram), unlike the old ramfs. The first stage deletes its own files after the switch, so nothing big stays in non-swappable memory.
4. The volume the root lives on stays mounted: the data volume at `/var/lib/iyakki` (the system's own search sees this and skips,
   `boot::dir_is_mount_point`), the boot volume at `/.stage1/boot` (read-only; the in-place update remounts it as before).
5. Then `execv /sbin/init`: the unchanged IYAKKI as PID 1.

**Extracted tree or squashfs?** Both are supported by the first stage; the build makes `root.sfs`.
* Extracted ext4 tree: no decompression, works with today's kernel (**tried in QEMU**), file-level updates are possible, but 3400 files
  means many small reads on USB, a first boot has to extract it (the data partition does not exist before the first start), and the installer
  would have to write ext4 trees from macOS/Windows.
* Squashfs (zstd): one file, so the installer and `iyakki-usb` copy it with the rest of the FAT volume unchanged; 3x smaller to read;
  sequential 256 KB blocks suit USB; decompression is cheap (estimated well under a second of CPU for everything touched, measured 0.7 s for all
  of it on a fast CPU). **Chosen as the default; needs `CONFIG_SQUASHFS`/`_ZSTD` (added to `linux.fragment`, kernel rebuild, not yet verified with
  `verify-kernel-config.sh`).** Read speed on the Mac is **not measured**: that is the first thing to run once the kernel has it.

### Fallbacks and variants
* **Live ISO** (no data disk): keeps the full initrd (the ISO is not changed). A stick that has the lean layout but no root is a hard error
  with a shell (never silent).
* **keep-ESP install** (`EFI/IYAKKI/INITRD` only): takes the full initrd; the lean layout is for the whole-stick layout.
* **Cloud and VM images, aarch64**: unchanged (full initrd); `lean-image.sh` is x86-64 only for now.
* **First boot after an install** (the data partition is an unformatted marker): the seed `boot/root.sfs` is used; the system formats the data
  partition as today.

### In-place update (design; the update code is not changed yet)
The existing update (`iyakki-agent` `devmode/update.rs`, docs/INSTALLER.md) replaces `boot/bzImage` and `boot/initrd` on the FAT volume (about 64 MB free).
With the lean layout the kernel and the 1 MB initrd still go there; the system image goes to the **data volume**, never to the FAT:
1. write `iyakki/root/ID.tmp/root.sfs`, fsync, compare its size and SHA-256 with the update manifest;
2. rename to `iyakki/root/ID/`, write `previous` (copy of `current`), then `current` through a temporary file and rename (the atomic switch);
3. only then replace `boot/bzImage`/`boot/initrd` if the kernel changed (the kernel and its modules must match the root: the update ships both, and
   a root whose `/lib/modules/<version>` differs from the running kernel is not made `current`);
4. keep the previous ID for rollback (the existing `prev_root` keeps the previous kernel and initrd); remove older IDs after a successful boot;
5. a power cut leaves either the old pointer (old root boots) or the new one (fully written, size checked by the first stage); the user's
   files (`home`, vault, audit log) are on the same volume but outside `iyakki/root/` and are never touched. Updates of single files by checksum
   (an rsync-like delta against the tree) fit the extracted-tree variant.
The seed `boot/root.sfs` stays as the last-resort root (`iyakki.root=boot`); with 64 MB free it cannot be replaced in place, only by re-running the installer.
**Not implemented:** the update code, the installer's knowledge of `root.sfs`, GRUB entry for `iyakki.root=boot`.

### Installer
`iyakki-usb --install` clones the whole FAT volume of the image (`fatclone`), so `boot/root.sfs` and the lean initrd arrive without code
changes, provided the image's boot volume is large enough (`lean-image.sh` sizes it from the files; the normal template is 256 MB). The size
limit and the in-memory `read_file` of the clone code for a 220 MB file were **not checked**.

## How to build and test the lean layout (needs a finished build)
```
cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1
TMPDIR=/tmp/iyakki-test/rt-boot dist/buildroot/lean-image.sh target/buildroot-macweb/images OUT --root tree   # extracted tree on a data volume: works with today's kernel
dist/buildroot/lean-image.sh target/buildroot-macweb/images OUT --root sfs --level 3          # root.sfs on the boot volume: needs the new kernel
python3 dist/buildroot/qemu-boot-speed.py OUT/iyakki-lean-x86_64.img --mbps 40,120,0 --runs 3
```
For a stick: write `OUT/iyakki-lean-x86_64.img` to a USB drive with the normal writer (`iyakki-usb`, never `dd` by hand), pick the normal entry, stopwatch.
Next steps for the build: call the split from `repack.sh` (flag `--lean`) and size the boot volume in `genimage-install.cfg`; rebuild the kernel (Mac and PC) with the new fragment.

## Not done in this round (stated plainly)
* **Prefetch** of the boot-order file list after the splash (`readahead`/`posix_fadvise`, bounded by the memory gate, paused while the foreground
  reads): designed only. With a lazily read root it matters more, so it comes after the Mac measurement; record the order with a QEMU run using `fanotify`.
* **Experimental compressed memory** (owner decisions: zswap plus a small swap file when there is a data disk, zram otherwise; one Off/On setting
  in Settings, admin only, off by default, `iyakki.zram=exp`; the memory gate counts zram only when on): not implemented. Facts gathered:
  the real root compresses 663 MB to 220 MB (zstd -3, 3.0x) and 173 MB (zstd -19, 3.8x); lz4 was not available to measure. A root on zram for the
  no-disk live case would hold about that. Defaults are untouched (zram 50 percent, swappiness 100).
* GRUB: `set timeout="3"` kept; no GRUB setting was changed (the graphics modules load once; `linuxefi` does not exist in this GRUB build); no
  bootloader replacement (no measured win to justify one). FAT cluster size cannot be raised on a 100 to 300 MB volume (FAT32 needs 65525 clusters).
* Kernel command line options (`tsc=reliable`, `nowatchdog`, `raid=noautodetect`) were not changed: nothing measured shows them on the critical path;
  the kernel's own time before PID 1 is under 1 s on the MacBook.
* Critical path between `/init` and the sign-in screen: already split into `early_boot_critical` and a background `late_boot` (network, sound,
  deferred coldplug); tests in `boot.rs` pin it. The QEMU log shows storage search and coldplug are the only waits left (the data search
  waited 4 s in one broken test layout, fixed by the first stage's hand-over).


## The experiment on a real stick (experimental, next to the normal boot)

What it is: two extra GRUB entries, **IYAKKI - fast start (lean boot)** and **IYAKKI - fast start + remote diagnostics (SSH, keys only)** (the second
has `iyakki.ssh=1`, keys only, like the normal diagnostics entry). They sit in front of the normal entry and (since 2026-10-24) are the first entries and the default: the old first
entry, `IYAKKI`, is untouched and stays what the menu starts after 3 seconds. The lean entries appear only when `boot/initrd-lean` exists on the
boot volume (`if [ -f /boot/initrd-lean ]` in `grub.cfg`), so a stick without the files has exactly the menu it always had. Both use the normal
entry's command line (`$iyakki_usbq`, `quiet`, ...); only the initrd differs (`/boot/initrd-lean`, 1 MB, instead of `/boot/initrd`, 173 MB).
Nothing about the normal boot changes: `/boot/initrd`, the kernel, the data volume's other files are not touched.

What goes where (nothing else on the stick is written):

| Where | What | Written by |
|---|---|---|
| boot volume (`IYAKKI-BOOT`, FAT, about 64 MB free) | `boot/initrd-lean` (about 1 MB), `boot/grub/grub.cfg` gets the lean block inserted in front of the normal entry (the old file is kept as `boot/grub/grub.cfg.before-lean`) | `lean-install.sh` |
| data volume (`IYAKKI-DATA`, ext4) | `iyakki/root/<id>/tree/` (the whole system, about 610 MB, extracted), `iyakki/root/current` (the id), `iyakki/root/previous` | `lean-install.sh` |

The first stage tries the data volume's `current`, then `previous`, then a seed image on the boot volume (none in this experiment). It does not
modify the tree: the writes of the running system go to a tmpfs overlay and are gone at power-off, as with the full initrd.

### Commands (the orchestrator)
```
# 1. build the lean assets from a finished image (reads only IMAGES, writes only OUT; no drive involved; the keys that may sign in over SSH
#    are the ones baked into the image at /etc/iyakki/authorized_keys; IYAKKI_AUTH_KEYS=FILE replaces that file in the lean tree)
cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1
IYAKKI_AUTH_KEYS=~/.ssh/id_ed25519.pub dist/buildroot/lean-image.sh target/buildroot-macweb/images /tmp/lean-out --root export
# 2. mount the stick's two volumes (the owner confirms WHICH drive first), then look, then write
sudo dist/buildroot/lean-install.sh --boot /mnt/iyakki-boot --data /mnt/iyakki-data --lean-out /tmp/lean-out          # prints drive, size, serial; writes nothing
sudo dist/buildroot/lean-install.sh --boot /mnt/iyakki-boot --data /mnt/iyakki-data --lean-out /tmp/lean-out --yes    # copies, then re-reads and checks everything
# 3. unmount, put the stick in the Mac, hold Option, choose the stick, choose "IYAKKI - fast start + remote diagnostics (SSH, keys only)"
# 4. from the other computer (the Mac's address is on the screen's bottom right: "Remote diagnostics ON - ssh root@ADDRESS")
ssh root@ADDRESS 'sh -s' < dist/buildroot/lean-check.sh
```
`lean-install.sh` refuses unless both folders are mount points whose devices are labelled `IYAKKI-BOOT` and `IYAKKI-DATA` on the same drive, the drive
is not an NVMe disk and not the drive the running system's root is on; it prints the drive, size and serial and needs `--yes`. A newer tree is put
on in the same way (new id): only changed files are copied (the rest are hard links to the files of the tree that is `current`, which is never
modified), the new tree is written as `.<id>.tmp`, read back, renamed, and `previous`, then `current`, are switched last (atomic renames); the
previous tree is kept and older trees made by the script are removed. Interrupted? Run it again.
`lean-check.sh` is read-only. `lean-install-test.sh` tests the refusals and the copy with made-up mount points and drives.

### Owner instructions (stopwatch)
Put the stick in the MacBook, start it holding Option, choose the stick. In the menu choose **IYAKKI - lean boot + remote diagnostics
(experimental)** and start a stopwatch the moment you press Enter; stop it when the first moving picture (the boot animation) appears, then
again when the sign-in screen is shown. Do the same once with the normal **IYAKKI** entry (stopwatch from Enter to the boot animation and to the
sign-in screen). Send back: the model (MacBookPro14,1 or 16,2), the four stopwatch times (lean: animation, sign-in; normal: animation, sign-in), what the
screen showed between Enter and the animation (a black screen, the logo, text), and the bottom-right line of the lean sign-in screen so the
remote check can connect. If the lean entry shows text that says it cannot start, send a photo; the Mac restarts by itself after 30 seconds
(the first entry is the normal boot).

### Going back, removing it, and what happens when something is wrong
* **Back to the normal boot:** choose the first entry, `IYAKKI`. The lean files are inert for it: it reads `boot/initrd` and mounts the data volume as before,
  and never looks at `iyakki/root/`. Nothing needs to be undone to use the normal boot.
* **A lean boot that fails does not hang.** If no volume appears within 30 seconds, or the data volume has no usable `current` or `previous` tree (missing,
  damaged pointer, a folder that is not there), the first stage prints on the screen and the serial console (kernel log level 2, so even `quiet` shows it):
  what is wrong, "Only a small log (boot/stage1.log) was written to the drive. Restart and choose the first entry, IYAKKI, in the boot menu", and that the machine restarts in
  30 seconds. It then restarts into the GRUB menu, whose default is the normal entry. `iyakki.stage1=shell` on the entry's command line (press `e` in the
  menu) gives a BusyBox shell on the screen instead. `iyakki.root=boot` ignores the data volume's trees (there is no seed in this experiment, so it ends in
  the same message).
* **Removing it:** `sudo dist/buildroot/lean-install.sh --boot ... --data ... --remove --yes` (same checks) deletes `boot/initrd-lean`, takes the lean block
  out of `grub.cfg` (the result is byte-identical to the file before the install; the test checks this) and removes `iyakki/root/`. By hand: delete
  `boot/initrd-lean` (the two entries then disappear from the menu by themselves) and the folder `iyakki/root/` on the data volume; the user's files (`home`,
  vault, audit log) are never touched. If `grub.cfg` itself is ever damaged, `boot/grub/grub.cfg.before-lean` is the file as it was.
* Space: the tree takes about 610 MB of the data volume (a second tree only for the files that changed, through hard links), shown under Settings,
  Storage as used space.

### What the lean root needs and how it was checked (QEMU, real Mac root file system)
Image `target/buildroot-macweb/images` of 2026-10-23, `lean-image.sh --root stick` (the normal install layout plus the lean files), UEFI (OVMF), GRUB, the real
kernel (Linux 6.12.112, Mac variant), throttled virtual USB disk (40 MB/s), entry chosen with key presses, remote diagnostics entry with a test key,
KVM, 2 vCPU, 3 GB, `dist/buildroot/qemu-lean-test.py`. **Measured in QEMU only; not a Mac.**

| Check on the lean root | Result |
|---|---|
| reaches the sign-in screen (account creation page, "Ethernet connected", the remote diagnostics line) | yes; kernel clock 2.7 s |
| PID 1 is `/sbin/init` (iyakki-core); `/dev`, `/proc`, `/sys`, `/run`, `/tmp`, cgroup2, efivarfs mounted | yes (`/proc`, `/run`, `/tmp`, devpts show twice in `/proc/mounts`: the system mounts them again after the first stage; the same code runs in the normal boot) |
| `/` is an overlay, upper layer on tmpfs, lower layer the tree on the data volume | yes (`lowerdir=/lower,upperdir=/upper/u`); 2.88 GB free of 3 GB against 2.25 GB with the full initrd |
| the data volume handed over (`/var/lib/iyakki`, the system's own search skipped), `/home` on it | yes (log line "mounted by the first stage"); the directory mode is set to 0700 as the normal mount does |
| clock floor early and after the data mount, zram swap (1.5 GiB, zstd), the storage watcher (link and power management of the USB port) | all ran (log lines) |
| hotplug (`/proc/sys/kernel/hotplug` is `/sbin/mdev`), module loading from `/lib/modules` of the extracted root (`modprobe hid_apple` loads) | yes |
| `/lib/firmware`: a new symlink into `/var/lib/iyakki` (what a driver pack does) | works (the write goes to the overlay) |
| k3s autostart | **not checked: this image has no k3s** (it is downloaded at the owner's request); nothing in the lean layout changes where it lives (the data volume) |
| apple-bce | **not checked: the module is not in the QEMU image's kernel tree and the T2 hardware is absent**; it loads from `/lib/modules` like the other modules |
| does the tree route need squashfs? | no, only what the kernel has today. The squashfs route needs the option added to `linux.fragment` (the running rebuild) |

Found and fixed during this check:
* The first stage had no console device (the kernel cannot open `/dev/console` for an initrd without the node) and `/dev/console` is only the **last** `console=`
  word, the serial port, which a Mac does not have. Messages now go to the kernel log (level 2 for failures, so the screen shows them) and the stage mounts
  devtmpfs, then opens the console before it prints.
* The kernel limits what programs write to `/dev/kmsg` to about 10 lines per 5 seconds (`printk.devkmsg=ratelimit`): most boot lines, "supervisor up" and
  "boot (to login)" among them, were lost in every boot, so no remote check could read the timeline. The first stage and `iyakki-core` now set
  `/proc/sys/kernel/printk_devkmsg` to `on`. (This is also why the full-initrd boot below shows no timeline rows: the image used here was built before the change.)
* A failed lean boot used to end in a shell on a console the Mac does not show; it now prints the message and restarts (above).
* The wait for the volumes: a bounded loop (50 ms steps, up to 30 s), the second volume gets 3 s after the first one appears; the decision is in
  `iyakki_stage1::wait_decision` (tested). In QEMU the virtual USB disk appeared 1.2 to 1.4 s after the kernel started (the emulated controller).

### Time, lean against full, same image and same QEMU setup
Throttled to 40 MB/s, KVM, 2 runs each; Enter on the entry (QEMU wall clock) to the moment SSH answers (the screen is up, the supervisor is running). **Measured in QEMU.**

| Entry | Enter to SSH | Kernel clock: first stage starts looking / root mounted / splash / supervisor up / sign-in picture | Free memory after boot |
|---|---|---|---|
| normal, remote diagnostics (173 MB initrd) | 12.7 s, 12.7 s | /init at 3.7 to 4.8 s | 2.25 GB of 3 GB |
| lean, remote diagnostics (1 MB initrd, tree on the data volume) | 4.7 s, 4.8 s | 0.30 / 1.67 / 1.81 / 2.46 / 2.69 s (second run 0.28 / 1.77 / 1.94 / 2.55 / 2.87) | 2.88 GB of 3 GB |

The 1.4 s from the first stage to the volumes is the emulated xHCI controller; on a Mac it is the firmware-to-kernel USB re-enumeration and is unknown.
The lean boot's cost on a real stick is the **first read of the tree's files over USB** (610 MB of 3400 files, only a part of them used at boot):
this is the number the experiment exists to measure.

### Kernel facts for the T7 on a Mac (checked against the kernel fragments and the built kernel's `modules.builtin`)
The first stage runs before any module loads. Built in (`=y`) in `linux.fragment`, and listed in the built Mac kernel's `modules.builtin`: `USB_XHCI_HCD`,
`USB_XHCI_PCI`, `USB_STORAGE`, `USB_UAS`, `SCSI`, `BLK_DEV_SD`, `ATA`, `SATA_AHCI`, `BLK_DEV_NVME` (not used on a T2 Mac: the internal NVMe is behind the T2
and is not offered), `EXT4_FS`, `VFAT_FS`/`FAT_FS`/`MSDOS_FS`, `NLS_CODEPAGE_437`, `TMPFS`, `DEVTMPFS`, `OVERLAY_FS`, `BLK_DEV_LOOP`, `ZRAM`, `EFI_STUB`,
`HOTPLUG_PCI_PCIE` (the Thunderbolt-3 ports of both MacBooks reach their xHCI controller through a PCIe hot-plug bridge). Partition tables are the
defaults (MSDOS and EFI GPT). **Nothing is missing for the tree route.** Missing for the squashfs route only: `CONFIG_SQUASHFS` (+ `_ZSTD`), added to
`linux.fragment`, not in the kernel the QEMU image has (the running rebuild will have it; run `verify-kernel-config.sh` on it). On a Thunderbolt port the drive can
appear later than on a plain USB port: the first stage waits up to 30 s and prints which block devices it did see when it gives up.


### First real-Mac attempt (MacBookPro16,2, T2): what happened and what changed
The owner chose "lean boot + remote diagnostics": GRUB logo and "Loading IYAKKI..." text for up to about a minute, then a blank screen that stayed blank;
no network, no audit-log line from that boot (so `iyakki-core` never got as far as the firewall step). The stick's labels were checked afterwards and are fine
(`mkfs.fat` boot volume with the label in the boot sector, ext4 `IYAKKI-DATA`, GPT with sda1 1 MB, sda2 vfat, sda3 ext4), and a GPT stick made the installer's way
(`gpt_install` on a file standing in for the disk, the real install template) with the lean files boots to the sign-in screen in QEMU. **The cause is not proven.**
The most likely one found by reading the code, fixed now: the first stage read the first sectors of **every** block device in name order, so `nvme0n1` (the
internal disk behind the T2 chip, which the normal boot only reads later and can wait on) came before `sda`, and a read that stalls there blocks the whole stage
silently (nothing was printed on the screen) and, once started, also an exec. Other candidates still open: a stall later (GPU driver load from the slow stick),
the console (`/dev/console` is the last `console=` word, the serial port; the first stage used `eprintln!`, which panics when that write fails, killing PID 1 and
leaving a blank screen; not reproducible in QEMU, where the write succeeds, but removed).

Changes (all seen working in QEMU, GPT stick, delayed and slow disk):
* `nvme*` and `mmcblk*` are never read; every device read has a 2.5 s timeout in a helper thread; a stage that panics prints the message and restarts instead of dying.
* Labels: boot sector label (FAT32 at 71, FAT12/16 at 43, empty type text accepted), else the volume-label entry of the root directory; case-insensitive; if no label
  matches in 10 s, a layout rule (an `sd` FAT volume that holds `boot/initrd-lean`, followed by the next partition as an ext2/3/4 data volume; the FAT volume is only looked
  into read-only).
* The wait for the stick is 60 s (it was 30), with a status line every 5 s on the screen.
* On-screen status (written to the text console, so it shows even with `quiet`): `IYAKKI fast start: looking for the IYAKKI drive...`, `found the drive`,
  `starting IYAKKI...`. If you see the first line and nothing else, the stage is waiting for the drive; if you see the last and then a blank screen, the system started.
* **`boot/stage1.log`** on the boot volume (appended, at most 64 KB, fsynced, written as soon as the boot volume is mounted and again before the switch or on a failure): timestamps,
  command line, every block device with size and what it holds, each decision, mount results, the last 120 kernel log lines. If the drive is never found the log cannot be written (look at the screen).
* **`stage1-kmsg.log`** in the data volume's top folder (`/var/lib/iyakki/stage1-kmsg.log`, rewritten every 2 s for 3 minutes by a helper that outlives the exec): the whole kernel log, including
  IYAKKI's own lines, so the place where a start stopped can be read on another computer. (`iyakki-core` writes `boot-lean.log` the same way once the image is rebuilt with `bootlog.rs`.)
* A third lean entry, **IYAKKI - fast start, compatible USB mode (no UAS)** (in "More options..."), with the T7's USB id on the plain `usb-storage` driver.

Lazy reads over a slow stick (QEMU, 40 MB/s): 400 random reads/s: sign-in picture at 3.4 s kernel clock; 100 reads/s (10 ms each): 7.0 s. A disk plugged in 5 s or 20 s after power-on: found and booted
(Enter-to-SSH 14.5 s and 25.4 s). No data volume (label changed): message and restart after 30 s. So slow reads alone do not explain a minute of blank screen.

### Refreshing the lean files over SSH (stick in a machine you can reach, tree unchanged)
```
dist/buildroot/lean-remote-refresh.sh --ssh root@ADDRESS [-i KEY] [-p PORT] --lean-out /tmp/iyakki-test/lean-out2          # shows the boot partition it found, changes nothing
dist/buildroot/lean-remote-refresh.sh --ssh root@ADDRESS [-i KEY] [-p PORT] --lean-out /tmp/iyakki-test/lean-out2 --yes    # initrd-lean + lean menu entries, checksummed and read back
```
It needs only BusyBox on the remote side, merges `grub.cfg` on this computer, mounts the FAT volume read-write for the copy and puts it back. (Tried against the QEMU guest.)
After a failed lean attempt, read `boot/stage1.log` (FAT volume) and `iyakki/stage1-kmsg.log` (data volume) on any computer.

The kernel log of every boot (lean or normal) is also kept on the data volume across crashes: docs/TROUBLESHOOTING.md, "The computer crashed or froze: where is the log" (`iyakki log kernel --crash`).

### Open risks of the experiment
* The Mac firmware's USB driver and the first reads of the tree over USB 3 are unmeasured; a slow first sign-in is possible (the stick's random-read speed).
* The root now lives on the stick: if the drive drops off the bus (seen once with a T7 under UAS, docs/STORAGE-LOSS.md) the system loses its programs,
  not only its data. The drive-loss watcher still reports, but recovery cannot unmount a volume the root depends on. Use the "compatible USB mode (no UAS)"
  entry's quirk (`iyakki_usbq` is passed to the lean entries too) if it happens.
* Updates: the in-place update code does not know the data-volume roots (see "In-place update"); after an update to the normal boot files the lean tree no longer
  matches the kernel's modules. The lean entries are for this experiment only until that is built.
* Only `IYAKKI_AUTH_KEYS` or the keys baked into the image can sign in on the diagnostics entry.
* The failure text was seen in the serial console in QEMU; on the QEMU screen it drew as garbled pixels (the emulated display's mode differs from the firmware's frame buffer), so
  how it looks on the Mac's panel is **not verified**.

## Table for real hardware (to fill in)
| Machine | Layout | Menu entry to first picture (stopwatch) | Kernel clock at splash | Sign-in screen | Notes |
|---|---|---|---|---|---|
| MacBookPro14,1, T7 USB 3 | full initrd (today) | | 1.85 s | | |
| MacBookPro14,1, T7 USB 3 | lean | | | | |
| Mac mini 2018, T7 USB 3 | full / lean | | | | |

## The fast start is the default (2026-10-24)

* **Menu.** A stick that has `boot/initrd-lean` shows, in this order: `IYAKKI - fast start (lean boot)` (the default after 3 seconds), `IYAKKI - fast start
  + remote diagnostics (SSH, keys only)`, `IYAKKI` (the full boot, the same as before), `IYAKKI - safe graphics`, `IYAKKI - Install to a disk` and
  `More options...`. The submenu holds the rarely needed starts: full-boot remote diagnostics, compatible USB mode (full and fast), show boot messages,
  hardware check, the boot self-test, the NVIDIA start (nouveau off) and the "also in a browser" start. A stick without `initrd-lean` has the same menu minus
  the two fast starts (so the first entry is `IYAKKI`, as always).
* **No restart loop.** When the first stage cannot start the lean system (no drive in 30 s, no usable `current`/`previous` tree, root cannot be moved), it
  writes the file `boot/grub/iyakki-lean-failed` on the boot volume (with the reason), shows the message and restarts. GRUB sees the file and makes `IYAKKI`
  (the full boot) the default, so the next start works without anyone touching the menu. The first entry (the fast start) can still be chosen by hand;
  the first fast start that reaches "switching root" removes the file, so the default goes back to the fast start by itself.
  Code: `lean_marker` in `crates/iyakki-stage1/src/main.rs`; test: `boot_menu.rs` (`the_fast_start_entries_come_first...`). Not exercised on a real stick yet.
* **Fonts.** The menu rows use DejaVu Sans Bold at 30 pixels (made without hinting, so the letters are evenly spaced) and the text under the logo while the
  kernel loads uses DejaVu Sans Mono at 24 pixels (a terminal needs a monospaced font); `post-image.sh` builds `iyakki-menu.pf2` and `iyakki-term.pf2` with
  `grub-mkfont` (Latin only, about 40 KB and 8 KB) and only then does the theme name them and make the rows taller. The 2026-10-24 report of "gaps between the
  letters" was GRUB's font lookup: the theme asked for "Unifont Regular 16" but the file is named "GNU Unifont Regular 16", and GRUB then silently uses the font
  loaded last, so the loading text was drawn in the menu font's big letters on Unifont's narrow character grid (wide gaps, cut-off tails of g and y). The theme
  now names "GNU Unifont Regular 16" in full. Seen in QEMU at 2560 x 1600 (`dist/buildroot/qemu-grub-menu-test.py` style harness); not looked at on a real screen yet.
* **Putting it on a stick.** `lean-install.sh` (stick in a PC) or `lean-remote-refresh.sh --menu-from <images>/install-root/boot/grub` (stick in a running
  Mac): both put the lean block in front of the normal entry; the second also sends the whole new `grub.cfg`, the font and the theme.
