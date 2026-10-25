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

Behind **Advanced: change partitions…** (closed until you open it). Keys there: `n` new table, `a` add, `d` delete, `f` format, `r` rename, `l`
read the table again, Esc leaves.

| Operation | What it does | Notes |
|---|---|---|
| New table | An empty GPT (protective MBR, primary and backup tables) or MBR; erases the start and end of the drive | everything on the drive becomes unreachable |
| Add partition | Size (`8 GB`, `500 MB`, empty = the largest free space), optional file system, optional label | starts on a 1 MiB boundary; MBR: four primary partitions, up to 2 TB |
| Delete | Removes the entry and erases the start of the partition | |
| Format | ext4 (with `mke2fs`, already in the images) or FAT32 (our own formatter) | exFAT and NTFS: *not supported yet* |
| Rename | Changes the label in place for ext2/3/4 and FAT | other file systems: *not supported yet* |
| Resize | *Not supported yet*, and it says so | copy the files off, delete, add a new partition |
| Read the table again | Asks the kernel to re-read the table (needed after changes by other tools) | no confirmation needed; changes nothing on the drive |

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
| BitLocker (Windows) | yes | **no** | "Encrypted: unlock needs the key." Unlocking is not built; open the drive in Windows |
| LUKS | yes | **no** | "Encrypted: unlock needs the key." Unlocking is not built (no permissive in-house path) |
| swap | yes | no | nothing to open |

"Not in the images' kernel yet" means the kernel configuration fragments under `dist/buildroot` do not turn the driver on. The
tile reads `/proc/filesystems` and `modules.dep`, so a kernel that has the driver (a custom image, a desktop Linux) opens the drive.
Adding a driver is a kernel configuration change (`CONFIG_NTFS3_FS`, `CONFIG_EXFAT_FS`, `CONFIG_BTRFS_FS`, ...), made when an
image is built; this feature does not change any image.

## Where the code is

* `crates/iyakki-agent/src/disks.rs`: listing, superblock detection, mount options, mount, unmount, eject (the `DiskEnv` seam).
* `crates/iyakki-agent/src/disks_part.rs`: the partition manager (tables, FAT32 formatter, label changes, plans, confirmations).
* `crates/iyakki-agent/src/disks_sys.rs`: the real machine (the only file that touches the base OS, see [BOUNDARY.md](../BOUNDARY.md)).
* `crates/iyakki-agent/src/disks_fake.rs`: the made-up machine the tests use.
* `crates/iyakki-shell/src/apps/disks_tile.rs` and `disks_tile_ui.rs`: the tile.

## Not built yet

Unlocking LUKS and BitLocker volumes, exFAT and NTFS formatting, resizing, extended MBR partitions, a Files app to open a drive in,
and a drive-connected toast while DISKS is closed.
