# Data volume filesystem: ext4 now, F2FS and Btrfs evaluated (F-STORE-07)

**For:** anyone deciding what `IYAKKI-DATA` is formatted with, and anyone asked "why not Btrfs for snapshots?".
**You will get:** the decision, the reasons, what each alternative would cost in this code base, and the facts that would change the decision.

## Decision

The data volume stays **ext4**. F2FS and Btrfs are not added to the kernel, the installer or the first-boot format step. This is a desk
evaluation against what the code relies on today; it is not a benchmark. The measurements that could reopen the question are listed at the end.
The kernel fragment is pinned by a test (`the_data_volume_is_ext4_and_the_evaluation_note_says_why`), so adding a second data filesystem
is a visible, deliberate change.

## What the code already assumes about the data volume

| Piece | Assumption | Where |
|---|---|---|
| First boot format | `mke2fs`, label `IYAKKI-DATA`, UUID derived from the install id (`mke2fs -U`) | `docs/INSTALLER.md` |
| Lost-drive handling | ext4's journal replay on re-mount, `errors_count` as a health signal | `docs/STORAGE-LOSS.md` |
| Disks tile | resize with `e2fsck` and `resize2fs`, shrink included | `docs/DISKS.md` |
| Encrypted home | a container file on the volume, attached to a loop device; works on any filesystem that supports large files | F-STORE-05 |
| Update layout | whole-system trees as generation folders (`iyakki/root/<id>/tree`, `current`, `previous`) | `docs/BOOT-SPEED.md` |
| Kernel | `CONFIG_EXT4_FS=y` built in; Btrfs, XFS and F2FS are not built (`docs/DISKS.md` lists them as foreign filesystems without a driver) | `dist/buildroot/board/iyakki/linux.fragment` |

## The candidates

| | ext4 (today) | F2FS | Btrfs |
|---|---|---|---|
| Fit to flash sticks and SSDs | Good enough; no flash-specific layout | Log-structured and designed for flash; the best case is a cheap stick with a poor controller | Copy-on-write, so more small writes; compression can reduce them |
| Data checksums | No (metadata checksums only) | Metadata checksums; data checksums not offered | Yes, data and metadata; detects silent corruption, repairs only with a second copy |
| Snapshots and rollback | None | None | Cheap subvolume snapshots |
| Transparent compression | None | Optional, per file | Optional (zstd), per mount or file |
| Sudden power loss on a pulled stick | Journal, mature repair tool | Roll-forward recovery; less field history on removable media | Copy-on-write keeps a consistent tree; repair tooling is more delicate when it does not |
| Shrink | Offline, works (`resize2fs`) | Grow only in practice | Online shrink works |
| Kernel and tools to add | Nothing | Driver and `mkfs.f2fs` / `fsck.f2fs` (GPL-2.0, separate programs, as `e2fsprogs` is) | Driver and `btrfs-progs` (GPL-2.0, larger) |
| Code to change here | None | Format step, installer, Disks resize and health, storage-loss re-mount, recovery | The same, plus space accounting (`df` is not truthful on Btrfs) and quota |

## Reasons, in order

1. **Every recovery path is written for ext4.** Re-mount after a vanished drive, the health signal, the resize in the Disks tile and the
   installer's UUID rule would each need a second implementation and a second set of hardware tests. The hardware loop (Mac mini, MacBook Pro, HP, Lenovo
   on one T7 SSD) has proven ext4 end to end; a second filesystem restarts that.
2. **Rollback does not need a snapshot filesystem.** The system already updates as whole generation folders with `current` and `previous`; an
   agent rollback of the person's files is a file-level feature (F-BACKUP rows), which works the same on any filesystem. Reflink copies would make
   agent workspaces cheaper, but ext4 users lose only disk space, not correctness.
3. **Compression is already done where it matters.** The root is a zstd squashfs; memory is compressed by zram or zswap (F-BOOT-31). Compressing the
   data volume would mainly save space for text and logs.
4. **The one case where F2FS is attractive is a cheap USB stick**, and the owner's stick testing is on an SSD. Nothing measured says ext4 is the problem
   on flash here.
5. **Btrfs' main promise, detecting silent corruption, is real** but needs a second copy to repair; on one stick it only reports. A cheaper step in the
   same direction is an end-to-end check above the filesystem (the update trees and backups are already hash-checked).

## What would reopen it

Any one of these, measured on the real hardware loop, with the numbers written here:

* write amplification or free-space collapse of the data volume on a consumer stick after weeks of use (SMART or controller counters, or `iostat` over a fixed workload) that F2FS would plausibly fix;
* a corrupted file found on a healthy-looking volume that a data checksum would have caught;
* the agent-rollback feature needing snapshots of a whole workspace in under a second;
* a power-pull test (fifty cuts during writes) where ext4 loses more than the journal promises.

How to measure when it comes to it: format a spare partition of the same stick with each candidate, run the same fixed workload (small-file create and
delete, 1 GiB sequential, `fsync` loop, the encrypted-home container) and compare time, bytes written to the device and recovery after a cut. Nothing here
was run: this document records reasoning, not results.

## What was not decided

XFS and bcachefs were not considered: XFS cannot shrink; bcachefs is not in a kernel the image carries. A choice offered to the person at install
time is not planned: the format step hides the choice on purpose.
