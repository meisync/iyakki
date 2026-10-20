# Memory (zram) and installing to disk

Technical details for two features of the alpha: compressed swap in RAM (zram) with an optional swap file, and the installer
that copies IYAKKI from the USB stick to a computer's disk. The short instructions are in `docs/INSTALL.md`; hardware limits are in
`docs/HARDWARE.md`.

## 1. Memory and zram

IYAKKI keeps the whole system in RAM, so memory matters more than on a normal Linux. At boot, **zram** is set up by IYAKKI
itself (no systemd, no zram-generator): a compressed block device in RAM used as swap with priority 100.

**What is built in.** The kernel has `CONFIG_ZRAM=y` with the zstd, lz4 and lzo-rle compressors built in (all variants, x86-64
and aarch64), `CONFIG_ZSWAP` (off unless asked for) and `CONFIG_PSI` for the pressure figures.

**When it happens.** In `crates/iyakki-core/src/boot.rs`, right after the data volume is mounted (so `system.conf` can be read)
and before the sign-in screen: `memory_setup()`. The optional swap file is made in the background in `late_boot`. Everything is
idempotent (a second run finds zram active and leaves it), logged to the boot log, and never fatal: a kernel without zram
prints one line and boot continues.

**What it does** (`crates/iyakki-zram`):

1. finds `/sys/block/zram0`, or asks `/sys/class/zram-control/hot_add` for a new device;
2. resets it when it was already set up, picks the compressor (zstd, else lz4, else lzo-rle, else lzo; your choice if the
   kernel offers it) and writes `comp_algorithm`, then `disksize`;
3. writes the swap header itself (`mkswap.rs`: version 1, `last_page`, a random UUID, label `IYAKKI-ZRAM`, the signature
   `SWAPSPACE2` at the end of the first page; it is byte-for-byte what util-linux `mkswap` writes, and a test compares them
   when `mkswap` exists on the machine);
4. calls `swapon(2)` with priority 100;
5. sets `vm.swappiness` (default 100), `vm.page-cluster=0`, `vm.watermark_boost_factor=0`, `vm.watermark_scale_factor=50`.

**Default size.** 50 % of RAM, at least 256 MiB and at most 8 GiB; machines with 2 GiB or less get 100 % of RAM. An explicit size
is not capped at 8 GiB but never exceeds 4 times RAM.

**Where the settings come from**, first match wins:

| Source | Examples |
|---|---|
| Kernel command line | `iyakki.zram=off`, `iyakki.zram=auto`, `iyakki.zram=4G,zstd`, `iyakki.zram=50%`, `iyakki.swappiness=60`, `iyakki.swapfile=2G`, `iyakki.zswap=on` |
| `/var/lib/iyakki/system.conf` (written by Settings and by the installer) | `zram=4G,zstd`, `swappiness=100`, `swapfile=off`, `zswap=off`, `hostname=name` (also set by Settings, System, Host name and `iyakki hostname NAME`; the boot option `iyakki.hostname=` overrides it) |
| Defaults | zram auto, swappiness 100, no swap file, zswap off |

A setting fixed by a boot option is shown in Settings as "locked by boot option": edit the boot entry to change it. Sizes take
`K`, `M`, `G`, `T` (binary, `GiB` also accepted) or `N%` of RAM.

**Swap file** (`iyakki.swapfile=2G` or the Settings page): `/var/lib/iyakki/swapfile` on the data volume, written out in full
(no holes and no `fallocate`, which some filesystems refuse for swap), mode 0600, priority 10 (below zram), only when the data
volume is a real disk and 1 GiB of space would remain. Off by default. IYAKKI does not hibernate; the file is only extra swap.

**zswap** (`iyakki.zswap=on`): a compressed cache in front of a swap file. It does little when the only swap is zram.

**Settings, Memory** (administrators): shows RAM, pressure, swap areas, zram size, algorithm, stored and compressed bytes and
the ratio; edit with the buttons; **Preview** lists what would change; **Apply now** reconfigures the running system (swap off,
reset, swap on) after a confirmation, and refuses when what zram holds would not fit back into free memory; **Save for next
boots** writes `system.conf` atomically. In a desktop window the page shows the host computer's figures read-only. The SYSTEM
tile's MEMORY card shows the zram ratio and memory pressure. From a terminal: `iyakki mem status|preview|apply`. The host name has its own page (Settings -> System -> Host name) and command (`iyakki hostname [NAME]`): applied at once and kept as `hostname=` in the same `system.conf`; the boot option `iyakki.hostname=NAME` overrides and locks it.

Check it on a running system:

    cat /proc/swaps                       # /dev/zram0 partition ... priority 100
    cat /sys/block/zram0/comp_algorithm   # lzo lzo-rle lz4 [zstd]
    cat /sys/block/zram0/mm_stat          # original size, compressed size, memory used, ...
    iyakki mem status

## 2. Installing to disk

The installed disk has one of two layouts. You choose with `--table auto|gpt|mbr` (default `auto`) or, in the shell, with the
**Partition table: Auto / GPT / MBR** button on the install page (key `t`, with a one-line explanation under it). The plan shows
the table the disk has now (read from the disk: GPT or MBR, each partition with its type and label) and the layout it gets.

**GPT** (new; any disk size, 512 and 4096-byte sectors):

| Part | Content |
|---|---|
| sector 0 | protective MBR (type EE) carrying GRUB's boot code, for legacy BIOS |
| sector 1 and on | primary GPT header and entry array (128 entries of 128 bytes) |
| partition 1, 1 MiB, `BIOS boot` (`21686148-6449-6E6F-744E-656564454649`), name `IYAKKI-BIOS` | GRUB's core image, embedded there for legacy BIOS |
| partition 2, 256 MB, `EFI System` (`C12A7328-F81F-11D2-BA4B-00A0C93EC93B`), name and FAT label `IYAKKI-BOOT` | FAT32: `EFI/BOOT/BOOTX64.EFI`, `boot/grub/grub.cfg`, the kernel and the initial RAM disk, as plain files |
| partition 3, the rest of the disk, `Linux filesystem`, name `IYAKKI-DATA` | marked "format me"; formatted as ext4 on the first boot; can pass 2 TiB |
| last sectors of the disk | backup entry array and backup header, at the true end of the disk |

Partitions start on 1 MiB marks, the disk and every partition get fresh random GUIDs, and the data partition ends on the last
whole MiB inside the usable area. Under UEFI the firmware reads the EFI System Partition; under legacy BIOS the protective MBR's
boot code loads GRUB from the BIOS boot partition, and GRUB (which has `part_gpt` built in) finds `/boot/grub/grub.cfg` on the
EFI System Partition by file, as it does on MBR.

**MBR** (what 0.1 had; 512-byte sectors only):

| Part | Content |
|---|---|
| sector 0 and the gap before 1 MiB | MBR with GRUB's boot code and GRUB's core image |
| partition 1, 256 MB, FAT32 `IYAKKI-BOOT`, type EF | GRUB for UEFI, the kernel and the initial RAM disk, as plain files |
| partition 2, the rest (up to 2 TiB), marked "format me" | formatted as ext4 `IYAKKI-DATA` on the first boot |

**Which one `auto` picks.** GPT when the target disk is larger than 2 TiB, has 4096-byte logical sectors, or the computer doing
the install was started through UEFI (`/sys/firmware/efi`); MBR otherwise, which keeps old BIOS-only machines working. The reason
is printed in the plan ("auto: the disk is larger than 2 TiB"). An explicit choice always wins, except that MBR on a 4096-byte
disk is refused. For a stick made with `iyakki-usb --install` on a UEFI computer and meant for an old BIOS-only PC, add
`--table mbr` if that PC's firmware does not take GPT (most do).

**Sector sizes.** All sector arithmetic uses the disk's logical sector size (`/sys/block/<disk>/queue/logical_block_size`).
512 and 4096 are supported; other sizes are refused. On a 4096-byte ("4Kn") disk the boot partition cannot be copied from the
install image, because the kernel and the firmware refuse a FAT filesystem whose sectors are smaller than the disk's. The
installer therefore reads the image's boot partition (`fatclone.rs`: its own FAT32 reader and writer, long names copied as they
are) and writes a new FAT32 volume with 4096-byte sectors and one sector per cluster; it is 320 MB instead of 256 MB (FAT32
needs 65525 clusters). **4Kn disks start through UEFI only**: GRUB's legacy BIOS boot sector reads 512-byte units, so on such a
disk the BIOS boot partition is left empty and the protective MBR has no boot code. (Real 4Kn disks sit in UEFI machines.)

**What `auto` and GPT mean for the other tools.** The data volume is found by its marker and label, not by the table, so MBR and
GPT sticks both work, whatever the disk's name (`sda3`, `vda3`, `nvme0n1p3`, `mmcblk0p3`). At boot, if a disk's GPT lists a
partition called `IYAKKI-DATA` whose device has not appeared yet, boot waits up to 10 s for it, and says in the log when the GPT
names a partition that is missing or holds no IYAKKI volume.

**Two ways to install**

1. **From another computer**: `sudo iyakki-usb --install --erase /dev/sdX [--table gpt] iyakki-install-x86_64.img` writes a USB
   stick (or any drive). With `--table gpt` it writes the GPT layout above (same code as the on-device installer) and always
   reads everything back; with MBR it writes the image as before.
2. **To this computer's disk, from the booted stick**: Settings, System, **Install to disk**, or in a terminal

        iyakki install --list                 # the disks, their tables and which may be used
        iyakki install --dry-run --disk sda   # print the plan, write nothing
        iyakki install                        # interactive: choose, answer, type the name and the size
        iyakki install --disk nvme0n1 --table gpt --zram 4G --swapfile 2G --hostname laptop --yes --confirm nvme0n1

   It copies the boot pieces from the stick it was started from (`--source sdb` or `--template FILE` name another source), makes
   the data partition fill the disk, and writes a "format me" marker there. **The marker also carries the choices** (zram, swap
   file, host name) in the same partition: the first boot reads them before formatting and stores them as `system.conf`. The
   first account is created at first start, as always. The stick itself stays an MBR install image (it boots anywhere); only the
   installed disk gets a GPT.

**What it will not do**

* Never writes to the disk it copies from, a disk with anything mounted ("unmount it first"), swap disks, write-protected
  disks, a disk holding the running system, or the disk the template file lives on.
* Refuses disks under 4 GB (the `iyakki-usb` stick writer has no minimum of its own beyond "the image fits"); warns under 16 GB.
* Refuses logical sector sizes other than 512 and 4096 bytes, and MBR on a 4096-byte disk.
* Whole-disk erase only. **Installing next to another system ("free space" mode) is planned**: `--free-space` says so and
  changes nothing. An existing GPT or MBR is read and shown in the plan, then replaced.
* Encrypting the data partition is **planned**; the data volume is not encrypted in this release (the vault encrypts secrets
  inside it).
* aarch64: the aarch64 image has no data partition and no install template, so the on-device installer is not available there
  yet. Use the aarch64 image directly in a virtual machine.
* Secure Boot: the loader and kernel are unsigned (F-BOOT-02), so Secure Boot must be off, as before.

**Order of writing and verification (GPT).** The old partition table and primary header are zeroed first (a half-finished install
must not look like the old system), then the data marker and settings, then the end of the disk is wiped, then the **backup**
table (entries, then the header in the very last sector), then GRUB's core image into the BIOS boot partition and the boot
volume, and last the **primary** table: entries, protective MBR with GRUB's boot code, and finally the primary header. Then every
piece that was written (both tables, the protective MBR, the BIOS boot partition, the whole boot volume, the data marker) is read
back with direct I/O and its SHA-256 compared, and both GPT copies are parsed from the disk again: signatures, header and
entry-array CRC32s, the headers' locations (the backup must be in the last sector), no overlaps, and the result must equal what
was planned. Any difference stops the install with "do not boot this disk".

**Order of writing and verification (MBR).** The data marker and the settings are written first, then the end of the disk is
wiped (a stale backup GPT would confuse the kernel), then the boot area without sector 0, and the partition table (sector 0)
last, so a half-finished install never looks bootable. The boot area is then read back (direct I/O) and its SHA-256 compared, and
the marker is read back.

**Boot assets, nothing new in the image.** The GPT install reuses the install image's pieces. GRUB's core image (already built
with `part_gpt` and `biosdisk`, see `BR2_TARGET_GRUB2_BUILTIN_MODULES_PC` in `iyakki_x86_64_defconfig`) is the gap between the
boot sector and partition 1; two numbers depend on where it sits and are patched when it is copied to the BIOS boot partition:
the "core image starts at sector N" field in the boot code (offset 0x5C) and the block list at the end of the core image's first
sector. The installer refuses a template whose GRUB does not look like that. The EFI loader is the image's
`EFI/BOOT/BOOTX64.EFI` with its built-in config, which finds the boot files by searching for `/boot/grub/grub.cfg`.

**Safety rules for the code and the tests.** Planning is pure (`crates/iyakki-usb/src/install.rs`): it works on values, so refusals
and layout maths are tested without a disk. `--dry-run` opens nothing for writing. Tests install onto plain files and QEMU
virtual disks only (never a block device). The shell page lists disks through the same code. The GPT reader/writer is
`crates/iyakki-storage/src/gpt.rs` (CRC32 included, no dependencies).

**Typed confirmation.** Both the terminal and the shell ask for the drive name and then its size exactly as shown ("238.5 GB").

## 3. Testing it

Unit tests: `cargo test -p iyakki-zram -p iyakki-usb -p iyakki-storage`, and the shell filters
`cargo test -p iyakki-shell --lib memory_tests install_tests`.

QEMU (needs sudo for `-accel kvm` here), after `./run.sh build`:

    sudo -E QEMU="qemu-system-x86_64" dist/buildroot/qemu-install-test.sh            # install template boots, data persists
    sudo -E dist/buildroot/qemu-memory-install-test.sh                              # on-device install, zram, first-boot settings

Both scripts take the partition table and the sector size of the installed disk from the environment (default: MBR, 512):

    sudo -E TABLE=gpt dist/buildroot/qemu-install-test.sh                    # GPT: BIOS (SeaBIOS) and UEFI (OVMF)
    sudo -E TABLE=gpt SECTOR=4096 dist/buildroot/qemu-install-test.sh        # 4096-byte logical sectors: UEFI only
    sudo -E TABLE=gpt dist/buildroot/qemu-memory-install-test.sh             # the on-device installer writes the GPT
    sudo -E TABLE=auto SECTOR=4096 dist/buildroot/qemu-memory-install-test.sh   # auto must choose GPT for 4096-byte sectors

`qemu-memory-install-test.sh` boots the install image as a stick with a second blank disk, runs the installer from inside,
boots the new disk (direct, BIOS and UEFI) and checks `/proc/swaps`, `comp_algorithm`, `system.conf`, the host name and
persistence. `qemu-install-test.sh` writes the template with `iyakki-usb --target-file` (`--table`, `--sector-size`), checks the
table with `sfdisk`, boots with the kernel directly twice (the data partition is formatted on the first boot and keeps its data on
the second) and then through GRUB under BIOS and UEFI.

The unit tests need no QEMU: `cargo test -p iyakki-usb -p iyakki-storage` builds GPTs on plain files (512 and 4096-byte sectors,
a 3 TiB sparse file, damaged tables, existing tables) and, where `sfdisk`, `sgdisk`, `gdisk`, `parted`, `fdisk` and `fsck.fat`
are installed, has them check the result; a missing tool is skipped.

## 4. On a real machine

1. Write the stick: `sudo iyakki-usb --install --erase /dev/sdX iyakki-install-x86_64.img` (check `--list` first).
2. Boot it (Secure Boot off, one-time boot menu).
3. `iyakki install --list`, then `iyakki install --dry-run --disk <name>` to read the plan.
4. `iyakki install` (or Settings, System, Install to disk).
5. Remove the stick, reboot, create the first account, then `cat /proc/swaps` and Settings, System, Memory.
