# DISKS: drives, mounting and partitions

The **DISKS** tile (System group) shows every drive of the machine, opens (mounts) and closes (unmounts) their partitions,
ejects USB drives and SD cards safely and, behind an **Advanced** button for administrators, changes partitions.

**Honest status.** Everything described here is built and tested against a made-up machine (fake `/sys` and `/proc`, a fake
mount runner, drives as byte buffers). Nothing has been run on a real drive yet. Try the partition manager on a spare USB stick
before you trust it. See `F-DISKS-01` to `F-DISKS-08` in [FEATURES.md](../FEATURES.md).

## What you can do

| You want to | Do this | Who |
|---|---|---|
| See the drives | Open DISKS. Left: every drive with an icon for how it is connected (USB stick, NVMe chip, SATA drive, SD card), its name, size, a bar for how full its open partitions are, a green dot when something on it is open. `/` filters by name, serial, size type or label. | everyone who has the tile |
| Open a USB stick, card or disk | Choose it, choose a partition, **Mount** (or `m`, or Enter). It appears in your own home, `/home/<account>/media/<label>` (made on mount, removed when empty after unmount, owned by your account). | administrators, operators: any drive; users: removable drives only |
| Close it | **Unmount** (`u`). IYAKKI flushes first. If something is using it, the message names the programs ("vlc (process 42)"). Nothing is ever forced. | same |
| Take it out | **Eject** (`e`, or the button on the drive's row): flushes, closes every partition, switches a USB drive off and says *You can remove it now.* | same |
| Look inside | **Open** copies the folder's path (there is no Files app): paste it into the file picker's path box, or press **Drives** in a file picker. | same |
| See details | **Properties** (`p`): device, file system, label, id, size, where it starts, where it would open, the owner of its files. | same |
| Change partitions | **Advanced: change partitions…** (`a`). See below. | administrators only |

Settings, Storage lists the open drives and `d` opens DISKS. A new USB drive or card that is plugged in while DISKS is open is
announced with a toast and chosen on the left (the tile looks at `/proc` every two seconds, two small reads, nothing is opened).

## Safety rules (they hold in the engine, not only on the screen)

1. **The system drive is never touched.** The drive that holds `/`, `/boot`, an `IYAKKI-BOOT` or `IYAKKI-DATA` volume, or swap in
   use is marked SYSTEM. Its partitions cannot be unmounted, it cannot be ejected, and the partition manager refuses it, whatever is
   typed. (That includes the stick IYAKKI booted from.)
2. **Nothing is forced.** A busy drive stays mounted; the message says what is using it.
3. **Programs on a drive do not run.** Every mount is `nosuid,nodev,noexec`. Administrators can switch on **Allow running
   programs** for one mount; `nosuid` and `nodev` stay. A **Read-only** switch opens a drive for reading only. A write-protected drive,
   disc images and Mac volumes are always opened read-only.
4. **Roles.** Administrators and operators mount, unmount and eject any drive that is not the system drive. Users can use removable
   drives only (USB, SD cards). Guests have no DISKS tile. Drives open in the signed-in person's own home, so no role needs more than its own home. Only administrators can allow running programs or change partitions.
5. **Every change to a drive asks for its name.** The partition manager shows the drive's name, size and serial number, shows the
   plan first (a dry run that writes nothing), asks you to type the drive's name or the last 4 characters of its serial number, and
   needs a second press on **Yes, change this drive**. Then the engine looks at the machine again before the first write.
6. **The drive must be quiet.** The partition manager refuses a drive with anything mounted ("unmount it first"), used as swap, held by
   RAID, LVM or an encrypted volume, or write-protected.
7. **Audit.** Mounts, unmounts, ejects, partition changes and every refusal are written to the audit log with the drive's name, size and
   serial number and what was done, never file contents.

## Where drives open, and sign-out

Every drive opens in the **signed-in person's own home**: `/home/<account>/media/<label>` (label made safe, never two the same, created on
mount, removed when empty after unmount). FAT, exFAT and NTFS files belong to that account (its ids from `/etc/passwd`). Mounting for
someone else is not offered. A User closes only what is open in their own `media` folder; administrators and operators can close any.
Signing out closes **nothing** by itself: it says a drive is still open in your home. Settings, Storage, key `o`, switches "close my drives
when I sign out" on (default off); a busy drive is then left open and said so.

## Partition manager

Behind **Advanced: change partitions…** (closed until you open it). Keys there: `n` new table, `a` add, `d` delete, `f` format, `r` rename, `z` resize, `l`
read the table again, Esc leaves.

| Operation | What it does | Notes |
|---|---|---|
| New table | An empty GPT (protective MBR, primary and backup tables) or MBR; erases the start and end of the drive | everything on the drive becomes unreachable |
| Add partition | Size (`8 GB`, `500 MB`, empty = the largest free space), optional file system, optional label | starts on a 1 MiB boundary; MBR: four primary partitions, up to 2 TB |
| Delete | Removes the entry and erases the start of the partition | |
| Format | ext4 (with `mke2fs`, already in the images), FAT32 (our own formatter), exFAT (`mkfs.exfat`) or NTFS (`mkntfs`, quick) | exFAT and NTFS need the person-installed programs (see below); without them the plan says which one is missing and nothing changes |
| Rename | Changes the label in place for ext2/3/4 and FAT | other file systems: *not supported yet* |
| Resize | Grows or shrinks one partition (its start stays) and its file system: ext2/3/4 (`e2fsck`, `resize2fs`) and NTFS (`ntfsresize`) | FAT32, exFAT, btrfs and anything not recognised are refused in words: no program IYAKKI can use, or a shrink could cut off files |
| Read the table again | Asks the kernel to re-read the table (needed after changes by other tools) | no confirmation needed; changes nothing on the drive |

### Format and resize: the programs, the plan and the confirmation (F-DISKS-08 part 2)

* **Programs.** `mkfs.exfat` (exfatprogs) and `mkntfs` / `ntfsresize` (ntfs-3g) are GPL-2.0 and never shipped: they are CLI tools entries
  (`exfatprogs`, `ntfs-3g`, source `Setting`, nothing fetched until the person gives an address and a SHA-256 and clicks), and a copy from the
  system's package manager is found too. `e2fsck` and `resize2fs` (e2fsprogs) come with the image. `disks_fmt.rs` runs them like the unlock helper: fixed
  argument lists built from checked parts (device paths must be plain `/dev/...`), no shell, cleared environment, nothing on standard input.
* **The plan** is the partition manager's own dry run, always shown first and changing nothing. A format names what is lost in plain words:
  "Everything on /dev/sdc1 (41.9 MB, label BACKUP) will be erased." A resize lists the order: grow = table first, then the file system; shrink =
  `e2fsck`, ask the smallest possible size (`resize2fs -P`, `ntfsresize --info`) and **stop unless the new size is enough**, shrink the file
  system, then the table. Sizes round up to whole MiB; the start must be on a 1 MiB boundary; a grow stops at the next partition.
* **Refused** (in the engine, not only on the screen): the system drive, any drive with a mounted partition, swap or a holder (an unlocked encrypted
  volume), a write-protected drive, non-administrators, a partition holding the running system, a resize of a file system IYAKKI cannot check
  (unknown content is never shrunk), a missing program.
* **Confirmation.** After the plan the person types the drive's name, the last four of its serial, or for one partition also its device name
  (`sdc1`) or its label, exactly (labels are case-sensitive); never a plain Yes. Then **Change this drive** starts a **5 second countdown**
  ("Starting in 4 s") with a Cancel button (Esc also cancels). Only then does the engine look at the machine again and write.
* **After** the steps the engine re-reads the table and checks the result (the partition shows the new file system or size); a mismatch is an
  error and an audit line (`disks.partition.verify_failed`). Audit lines carry the drive's name, size, serial, the operation and the program's name,
  never an argument list or contents.
* **Chat.** `disks_format_plan` (safe: the dry run for a format or a resize), `disks_format` and `disks_resize` (a hard ask every time, no standing
  allow; the card is the plan). Approving only leaves a request: the tile shows the plan, and the typed name and the countdown are on the
  screen only; the tools have no argument that could carry a confirmation.

MBR drives with an extended partition cannot be changed (make a new table, which erases the drive).

## File systems

| File system | Listed | Opened | Notes |
|---|---|---|---|
| ext2, ext3, ext4 | yes | read and write (`ext4` driver) | in the images |
| FAT12/16/32 | yes | read and write (`vfat`), owner = the IYAKKI session, `umask=0077` | in the images |
| exFAT | yes | `exfat` driver, read and write | **not in the images' kernel yet**: the tile says the driver is missing |
| NTFS (Windows) | yes | `ntfs3` driver, read and write | **not in the images' kernel yet.** A volume Windows did not shut down cleanly (fast start-up, hibernation) is opened read-only with a plain warning; shut Windows down completely or let Windows repair it ("repair needs Windows"), then plug it in again |
| Btrfs, XFS, F2FS | yes | `btrfs`, `xfs`, `f2fs` drivers | **not in the images' kernel yet** |
| ISO 9660 | yes | read-only | in the images |
| HFS+ (Mac) | yes | read-only `hfsplus` driver | **not in the images' kernel yet** |
| APFS (Mac) | yes | **no** | shown honestly: Linux cannot read it safely. Use a Mac, or format the drive as exFAT there |
| BitLocker (Windows) | yes | **read-only** | "Encrypted: BitLocker". Unlock (password or recovery key, masked box) needs the person-installed `dislocker`; never writable here |
| LUKS1/LUKS2 | yes | **read-only first** | "Encrypted: LUKS2". Unlock needs the person-installed `cryptsetup`; "Allow writing" is a second step; Lock closes it |
| swap | yes | no | nothing to open |

"Not in the images' kernel yet" means the kernel configuration fragments under `dist/buildroot` do not turn the driver on. The
tile reads `/proc/filesystems` and `modules.dep`, so a kernel that has the driver (a custom image, a desktop Linux) opens the drive.
Adding a driver is a kernel configuration change (`CONFIG_NTFS3_FS`, `CONFIG_EXFAT_FS`, `CONFIG_BTRFS_FS`, ...), made when an
image is built; this feature does not change any image.

## Where the code is

* `crates/iyakki-agent/src/disks.rs`: listing, superblock detection, mount options, mount, unmount, eject (the `DiskEnv` seam).
* `crates/iyakki-agent/src/disks_part.rs`: the partition manager (tables, FAT32 formatter, label changes, resize planner, plans, confirmations).
* `crates/iyakki-agent/src/disks_fmt.rs`: the person-installed programs (exFAT, NTFS, resize): runner seam, argument lists, answer parsers.
* `crates/iyakki-agent/src/disksformattools.rs`: the chat tools and the request file to the tile.
* `crates/iyakki-agent/src/disks_sys.rs`: the real machine (the only file that touches the base OS, see [BOUNDARY.md](../BOUNDARY.md)).
* `crates/iyakki-agent/src/disks_fake.rs`: the made-up machine the tests use.
* `crates/iyakki-shell/src/apps/disks_tile.rs` and `disks_tile_ui.rs`: the tile.

## Not built yet

extended MBR partitions, resizing FAT32/exFAT, our own exFAT/NTFS formatter,, a Files app to open a drive in,
and a drive-connected toast while DISKS is closed.

## Encrypted volumes

See `docs/development/DISKS-UNLOCK-HELPER.md`: the password is typed in a masked box in the tile (or the assistant asks you to), goes to the helper program through a pipe, is never stored or logged, and wrong tries are slowed down. Keys: `k` unlock, `w` allow writing, `l` lock.

### Owner test of format and resize (nothing here was run on a real drive)

Use a scratch USB stick with nothing on it. Install `exfatprogs` and `ntfs-3g` from CLI tools first.
1. DISKS, the stick, Advanced: new GPT table, add a 4 GB ext4 partition and a 4 GB FAT32 one. Check the plan text before each.
2. Format the FAT32 one as exFAT, typing its label. Check the countdown shows and Cancel stops it; then let it run; plug it into a Mac or Windows PC.
3. Format as NTFS; plug it into Windows (and `ntfsfix`-free: it should mount without a repair prompt).
4. Grow the ext4 partition into free space, then shrink it to half; fill it first with files and compare a checksum after.
5. Try to shrink below the data size: it must stop with "need at least" and change nothing.
6. Check that every unplug during a shrink is not tried (power loss can lose files, as the plan says).
Open questions: does `ntfsresize --force --size` skip its prompt and take bytes as given, does `resize2fs -P` print the line the parser expects with this e2fsprogs, does `mkntfs -Q -F -L` behave on a partition node.
