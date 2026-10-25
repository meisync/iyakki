# The IYAKKI installer: partitions, plans and dual boot

**For:** anyone installing IYAKKI on a disk that may also hold another system, and anyone reviewing how the installer keeps
that other system safe.
**You will get:** the four ways to install, what each does to the disk, the rules that are enforced in code, the plan file,
what happens if the power fails, and what is not built yet.

> **Alpha.** Everything below is tested on disk images (plain files standing in for disks, never a real device).
> Nothing has been run against real hardware yet; the numbered checklist at the end is for that first run. Start with a spare
> USB stick as the target, not an internal disk.

Contents: [Ways in](#ways-in) · [Modes](#modes) · [Rules](#the-rules-that-are-code) · [The plan](#the-plan) ·
[Plan file](#the-plan-file) · [Boot-time install mode](#boot-time-install-mode) · [Window installer](#window-installer) ·
[Starting the new system](#starting-the-new-system) · [If it is interrupted](#if-it-is-interrupted) ·
[Other computers](#windows-and-macos-hosts) · [Hardware checklist](#first-hardware-test)

> **Experimental, separate from the installer:** a lean boot (a 1 MB first stage instead of the 173 MB initrd, the system read from the data volume) can be added to a stick
> that is already installed, as extra boot entries next to the normal one, with `dist/buildroot/lean-install.sh`. The installer never copies it: see "Fast start is not copied" below.
> See [BOOT-SPEED.md](BOOT-SPEED.md), "The experiment on a real stick", for the commands, the checks the script makes, and how to go back.

## Ways in

| Way | Command / entry | Where it runs |
|---|---|---|
| Terminal pages | `sudo iyakki-usb --plan [IMAGE]` | any Linux computer, with the install image (`iyakki-install-x86_64.img`) |
| Boot-time install mode | GRUB entry **IYAKKI - Install to a disk** | the computer being installed, started from an IYAKKI USB stick |
| Window installer | `iyakki-installer` (crates/iyakki-gui) | a Linux desktop with polkit |
| Scripts / CI | `iyakki-usb --plan --disk sdb --mode free-space --dry-run --plan-file plan.json` | anywhere; `--dry-run` needs no administrator rights for a plan on a readable disk |

All of them use one library: [`planner.rs`](../crates/iyakki-usb/src/planner.rs) reads the disk and builds the plan,
[`apply.rs`](../crates/iyakki-usb/src/apply.rs) writes it, [`wizard.rs`](../crates/iyakki-usb/src/wizard.rs) is the page
state machine the terminal ([`tui.rs`](../crates/iyakki-usb/src/tui.rs)) and the window both draw. The older commands keep
working unchanged: `iyakki-usb --install --table auto --erase DRIVE IMAGE` writes a stick; `iyakki install` copies the booted
stick onto a whole disk.

## Modes

| Mode | What it does | What stays |
|---|---|---|
| `whole-disk` | Erases the disk; makes the IYAKKI layout (GPT: BIOS boot + `IYAKKI-BOOT` + `IYAKKI-DATA`, or MBR). The long-standing installer. | nothing |
| `free-space` | Creates `IYAKKI-BOOT` (FAT32, EFI System type, size taken from the image) and `IYAKKI-DATA` in the largest (or a chosen) block of unallocated space. | every existing partition, byte for byte; the disk's table, GUID and boot code |
| `custom` | The same with sizes you type (MiB). | as above |
| `reuse` | You choose two existing partitions: one becomes BOOT (formatted FAT32), one DATA (marked, formatted by the first boot). | everything else |
| `reuse` / `free-space` with `--keep-esp` | **Dual boot.** The disk's existing EFI System Partition is kept; only the folder `EFI/IYAKKI` (kernel `KERNEL.EFI`, RAM disk `INITRD`) and a UEFI boot entry are added. The data partition is a new one (free space) or a chosen one (reuse). | the other system's loaders and files, untouched |

No mode shrinks or resizes an existing filesystem. If there is no free space, the installer says so and points you to the other
system's own disk tool (Windows Disk Management, GParted, Disk Utility) to shrink a partition first.

A blank disk (no table) in `free-space` mode gets a fresh table with just the two partitions (UEFI only; use `whole-disk` for
legacy-BIOS boot). A disk with a filesystem but no partition table is refused in the partial modes.

## The rules that are code

Each rule is in `planner.rs` / `apply.rs` and pinned by a test in `planner_tests.rs`.

1. **Nothing runs until the plan is confirmed.** `build_plan` is pure. The writer rebuilds the plan from a fresh read of the
   disk and refuses if its digest differs from the one confirmed (the disk changed in between).
2. **Confirmation** (changed 2026-10-24, owner's decision: **this relaxes the earlier rule**, which was to type the drive name and
   then its size on every install). Why: typing on a laptop's boot console was slow and error-prone, and a plain summary with a
   clear button protects as well when it is built the right way. The pages (boot menu entry, terminal pages, window) now show a big
   summary (drive model, size and name, what will be **created**, **kept** and **ERASED**) and one large **Install** button, with
   **Back** and **Cancel** (keyboard: Tab or the arrows move, Enter presses, Esc is Back; the window also takes the mouse and touch).
   * A plan that **erases nothing** (free space, custom sizes into free space, `--keep-esp`, reuse of partitions that hold no
     filesystem) needs that one press.
   * A plan that **deletes a partition or formats one that holds a filesystem** (whole disk; reuse of a partition with data) needs a
     second, deliberate step that is not typing: the button says `Erase sda (500.1 GB) and install` in red, a box
     "I understand that everything on this drive will be erased" must be ticked first (Space or a click), and the focus starts on
     **Cancel**, so Enter alone never erases.
   * The warnings stay as visible lines, never as typing: the disk the running system uses, an internal disk that holds another
     system (Windows NTFS or macOS APFS signatures), partitions that are unmounted first.
   * **The plan file still carries proof**: `"confirmed": {"method":"button","drive":"sda","size":500107862016,"erase_ack":true}`.
     The writer (`--apply-plan`) refuses a file without it, a button proof for another drive or size, and a plan that erases data
     whose proof lacks `erase_ack` (`planner::check_confirmation`). The older typed form
     `{"name":"sda","size":"500.1 GB"}` is still accepted. The command line (`--yes --confirm NAME`, the typed prompts of
     `iyakki-usb --plan --disk ...` and `iyakki install`) is non-interactive or typed by design and is **not** changed.
3. **Never touch a mounted partition.** A mounted partition that the plan would format or add files to is unmounted first (the
   plan lists it under "Unmounted first") unless it is part of the running system (`/`, `/boot`, `/usr`, the IYAKKI data folder, ...),
   in which case the plan is refused. The writer re-reads `/proc/mounts` right before writing.
4. **The disk the running system uses** is changed only in the partial modes, only with the extra confirmation
   (`--confirm-running-disk` on the command line, the warning line and the Install button on the Confirm page), and never in `whole-disk` mode.
5. **The stick IYAKKI started from** is the source of an install and is never a target (boot-time mode removes it from the list).
6. **Never delete or write a partition that is not in the plan.** Only `whole-disk` deletes (every partition, all named). Kept
   partitions are fingerprinted before (first and last MiB) and compared after; sector 0 of a GPT disk (the protective MBR with
   the other system's boot code) must be byte-identical afterwards, and every old GPT entry (GUID, range, attributes, name) too.
7. **No shrinking.** A custom size larger than the free space is an error that says so; there is no resize code.
8. **1 MiB alignment** for every new partition; the aligned free space is computed inwards around odd foreign partitions (an old
   Windows XP partition at sector 63 is respected).
9. **GPT**: the backup header is rewritten at the true end of the disk (also for a table made for a smaller disk), 128 entries of
   128 bytes (a GPT with another geometry is refused, not rewritten), the table is `check()`ed (inside the usable area, no overlaps)
   and read back from both copies. **4096-byte sectors**: supported (boot volume rebuilt at 320 MiB). **MBR**: 4 primary slots; a
   disk with no free slot is refused with advice; sector counts must fit 32 bits; boot code and the other entries stay.
10. **BOOT size comes from the image** (`SourceSpec::boot_required`): the PC image's boot volume is 512 MB; 320 MiB on 4096-byte
    sectors. A chosen partition that is smaller is refused.
11. **A fresh GUID** for a reformatted partition (another system may remember the old one).
12. **Foreign ESP**: only an unmounted FAT32 ESP with enough free space is used; the writer adds only `EFI/IYAKKI`, never edits an
    existing directory entry (it fills free slots, extends a full directory by one cluster), writes data first, the FAT next, the
    directory entry last, and refuses if `EFI/IYAKKI` exists already. After writing, every file that was on the volume is compared.
13. **UEFI boot entry**: only our entry ("IYAKKI") is ever created or replaced; it is appended to `BootOrder` so the other system
    stays the default. No `efibootmgr`: [`efi.rs`](../crates/iyakki-usb/src/efi.rs) writes the variables through `efivarfs`.

## The plan

```
Disk /dev/sda (Samsung SSD, 500.1 GB, 512-byte sectors), INTERNAL
Mode: Free space only: add IYAKKI in unallocated space, touch nothing else
Partition table: GPT -> GPT (existing entries kept as they are)

  #  action      partition      start        size        what
  1  keep        sda1               1.0 MB    104.9 MB  fat32 “EFI system partition”
  2  keep        sda2             105.1 MB     16.8 MB  Microsoft reserved
  3  keep        sda3             121.6 MB    262.1 GB  ntfs “Basic data partition”
  4  keep        sda4         262.2 GB      1.1 GB  ntfs “Windows RE”
  5  create      new 5        263.3 GB    536.9 MB  IYAKKI-BOOT FAT32, EFI System (...)
  6  create      new 6        263.8 GB    236.2 GB  IYAKKI-DATA (ext4 on first start)

Nothing that exists now is erased or changed.
```

Destructive rows say what is lost (`[loses: its ntfs filesystem “...”]`). `Erased or reformatted:` lists every destroyed
partition by name.

## The plan file

`--plan-file FILE` writes the plan as JSON (mode `0600`). `--dry-run` prints the plan and exits. Fields (format
`iyakki-install-plan-1`): `disk` (name, path, model, size, sector size, `fingerprint` of the table sectors, `internal`,
`running_system`), `request` (everything the person chose: mode, free space start sector, partition numbers, keep_esp, sizes,
table, first-boot settings), `table_before`/`table_after`, `steps[]` (`action` create|format|delete|keep|add files, `partition`,
`number`, `first_sector`, `sectors`, `bytes`, `role`, `what`, `loses`, `destructive`), `unmount[]`, `destroys[]`, `warnings[]`, `digest`.

`iyakki-usb --apply-plan FILE` is the privileged writer. It accepts a file only if it is owned by the caller (or the
`pkexec`/`sudo` user) and not group/world-writable, carries a `confirmed` object (the Install button's `{method, drive, size, erase_ack}` or the older typed name and size), matches a fresh
plan (digest) and the confirmation matches the drive (and, for a plan that erases data, carries `erase_ack`). The window installer writes that object when you press Install (rule 2).

## Boot-time install mode

GRUB entry **IYAKKI - Install to a disk** adds `iyakki.install=1`. PID 1 (`iyakki-core`) then, after the usual early boot (drivers
for storage, input and display; the boot splash is skipped so the text is visible), does *not* start the desktop, web server or
supervisor. It opens `/dev/tty0` (the screen's text console and the keyboard), runs the same pages
(`iyakki_usb::plan_cli::run_boot`), shows "Remove the installation USB stick, then press Enter to restart", and restarts.

**What is copied** (the payload is the install image that is booted, read from the boot stick's own boot partition, read-only):

| Target | Content |
|---|---|
| own BOOT partition (`free-space`, `custom`, `reuse`, `whole-disk`) | the whole FAT32 boot volume of the stick rebuilt into the new partition: `EFI/BOOT/BOOTX64.EFI` (GRUB), `boot/grub/grub.cfg` + font + theme, `boot/bzImage` (kernel), `boot/initrd` (the root filesystem as RAM disk). The new system's menu has every entry the stick has. |
| the other system's ESP (`--keep-esp`) | only `EFI/IYAKKI/KERNEL.EFI` (the kernel; it is an EFI program) and `EFI/IYAKKI/INITRD`, started by the firmware's own "IYAKKI" entry with `initrd=\EFI\IYAKKI\INITRD` and the command line of the stick's first menu entry. No GRUB, no menu there. Secure Boot must be off for this route. |
| DATA | one marker block (first MiB): "format me" plus your first-boot settings; the first start of the new system makes it ext4 `IYAKKI-DATA`. It is never formatted by the installer. |

**Fast start is not copied.** If the stick's `boot/grub/grub.cfg` carries the fast-start block (`# BEGIN lean-boot` ...
`# END lean-boot`, managed by `dist/buildroot/lean-sync.py`), the copy on the new disk is made without it, without
`boot/initrd-lean`, `boot/stage1.log`, `boot/grub/iyakki-lean-failed` and `*.before-lean` (`crates/iyakki-usb/src/leanstrip.rs`,
applied while the boot volume is rebuilt: the same marker text and algorithm as `lean-sync.py grubstrip`; the pages report it).
The new disk has no lean tree and no data volume yet, so a copied fast-start entry would be the default and the first start would
wait for a data volume that does not exist. The default entry of the copy is the normal "IYAKKI" one. (The older `iyakki install`
command and the stick writer copy the volume as it is.)

**Text size.** The boot-time pages are drawn on the kernel's text console, whose own font is 8x16 pixels: tiny on a 2560x1600
screen. At start the installer reads the screen size (`/sys/class/graphics/fb0/virtual_size`) and doubles the console font
(`scale_for_screen`: 1 below 1900x1150, 2 from there; each font pixel becomes a 2x2 block, loaded with the `KDFONTOP` ioctl and put
back when the installer ends). **`+` and `-`** change it while it runs; `iyakki.ui_scale=N` (1 to 4) on the kernel command line
fixes it. The kernel's console fonts are at most 32x32 pixels, so the 8x16 font can be doubled and no more; a bigger text would
need the pages drawn on the framebuffer. A terminal window has its own zoom. The console takes no mouse, so on the boot menu entry
the buttons are keys.

The stick must be an IYAKKI install stick (made by `iyakki-usb --install`, MBR or GPT). A stick written from the live ISO has no
boot volume to copy: the installer says so. The list of targets excludes the stick, shows every other disk (internal and USB),
and offers the same four modes. The same plan and rules apply; the confirmation is rule 2.

BIOS (legacy) boot: `whole-disk` installs GRUB for BIOS exactly as before. On a *shared* disk (free space, reuse, keep-ESP)
legacy BIOS boot is **not** set up: rewriting the first sector of a disk that belongs to another system is not done silently.
Planned: an explicit `--bios` option for MBR disks with an empty gap (the old boot code is saved first).

## Window installer

`iyakki-installer` (Linux; `cd crates/iyakki-gui && cargo run --release --bin iyakki-installer`) has the pages
**Image, Drive, Mode, Plan, Confirm, Progress, Done**.

* **Image**: a file, or an `https://` link downloaded with a progress bar into `~/.cache/iyakki/`; when `<link>.sha256` exists the
  download must match it (otherwise it is deleted); a local file's `.sha256` can be checked with a button. Plain `http://` is refused
  (except localhost). The download uses the workspace's HTTPS client; the logic is `iyakki_usb::fetch`.
* **Elevation**: the window itself never runs as root and never sees a password. It starts `iyakki-usb` through **`pkexec`**
  (polkit shows the desktop's own prompt): once to list the drives as JSON (`--plan --list --json`), once to write
  (`--apply-plan FILE --machine`, progress lines come back on stdout). With no polkit agent it shows the `sudo` command to run in a
  terminal instead. The plan file lives in a `0700` folder under `$XDG_RUNTIME_DIR` and is deleted afterwards. A process running as
  the same user could write a different plan file; the polkit prompt (your password) is the barrier, and the writer still applies
  every rule above to whatever plan it is given.
* Windows and macOS: see below.

## Starting the new system

Restart and open the firmware's boot menu (F12, F10, Esc or Option at power-on, depending on the make) and choose **IYAKKI**.
The installer appended its entry at the *end* of the boot order (internal disks on a UEFI start only; a USB stick target is simply picked in the boot menu), so the other system is still the default; make IYAKKI the default
in the firmware settings if you want. If the firmware does not show an entry, the disk itself is listed (an EFI System partition
with `EFI/BOOT/BOOTX64.EFI`); on a Mac hold Option. The first start formats the data partition (a few seconds) and asks for the first
account.

## If it is interrupted

The order of writes is the recovery story ([`apply.rs`](../crates/iyakki-usb/src/apply.rs)):

1. new data goes into free space (or the partitions you gave up);
2. the partition table is changed last: backup GPT, primary entries, primary header (MBR: sector 0 with only the new entries);
3. read-back, then the UEFI entry.

* **Power fails before step 2** (tested by `an_interrupted_install_leaves_the_old_table_and_the_old_partitions_intact`): the table
  is the old one, the other system boots as before; the free space just holds unused bytes. Run the installer again.
* **Power fails inside step 2**: GPT readers take whichever copy is valid; both describe either the old or the new table. A
  half-written pair is detected by the CRC and repaired from the good copy by the next tool you run (`sgdisk -e`, GParted); the
  installer's next run reports "the primary GPT is damaged; the backup copy was read".
* **`reuse` mode** has already formatted the partitions you gave up; their old contents are gone (that was confirmed).
* **keep-ESP**: data clusters first, FAT second, directory entry last: at worst unreferenced clusters remain; `fsck.vfat` (or
  Windows' check) reclaims them. The other system's files are never rewritten.
* **Verification fails**: the installer says "Do not boot" for the new partition and which check failed; kept partitions are
  compared too, so it also says if one changed.
* **To undo a free-space install**: delete the two partitions `IYAKKI-BOOT` and `IYAKKI-DATA` with any disk tool, and (keep-ESP)
  delete the folder `EFI/IYAKKI` and the firmware entry "IYAKKI". Nothing else was changed.

## Windows and macOS hosts

* **Linux**: everything above.
* **macOS**: `crates/iyakki-usb/src/macos.rs` parses `diskutil` and unmounts for the *whole-drive stick writer*
  (`iyakki-usb --install`/`--iso`). The planner's partial modes need a macOS disk reader (APFS containers, `diskutil` geometry) and
  are **not** available there yet; the pages say so and `--plan` stops with that message. Planned.
* **Windows**: not built. Planned work, in order: enumerate disks with `DeviceIoControl` (`IOCTL_DISK_GET_DRIVE_LAYOUT_EX`,
  `IOCTL_STORAGE_QUERY_PROPERTY`), lock/dismount volumes (`FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`), raw `\\.\PhysicalDriveN`
  writes with sector-aligned buffers, UAC elevation by re-launching the writer with `runas`, an egui window (the planner is
  already OS-independent), and the BCD-free UEFI entry through `SetFirmwareEnvironmentVariable`. Use Rufus or balenaEtcher in DD mode meanwhile.

## First hardware test

See the numbered list in the release notes of the change that added this file; in short: (1) dry run on the Mac mini with
`--list` only; (2) a spare USB stick as the target from the booted stick; (3) free-space on an *empty* spare stick; (4) reuse; (5)
free-space on a stick holding a Windows-like partition; (6) only then an internal disk's free space.

A drive that disconnects while IYAKKI runs from it (loose USB-C cable, bus glitch): [STORAGE-LOSS.md](STORAGE-LOSS.md).

## Per-install identity

**Status: design only, nothing of this is implemented.** Owner decision needed before it is.

**Problem.** The stick and a new internal install both carry the labels `IYAKKI-BOOT` and `IYAKKI-DATA`, and the system finds its
data volume by label and content (`iyakki_storage::find_data`, `find_pending`, the lean first stage's `DATA_LABEL`). With both
disks attached it may mount the other install's data (accounts, vaults, documents): a wrong-volume mount, and with
`find_pending` a wrong-volume *format* of an unformatted marked partition.

**Design.**

1. The installer makes an install id (16 random bytes, 32 hex characters; short form `i-` plus the first 8 for display) when it
   builds the plan. It is part of the plan file and of the digest.
2. It is written to the target's BOOT partition as a file `boot/install-id` (and `EFI/IYAKKI/install-id` for keep-ESP), and into the
   command line of the copied menu: every `linux` line of the copied `grub.cfg` gets `iyakki.install=<id>` (the copy is
   already edited while it is cloned: `leanstrip.rs`'s editor is the place); keep-ESP puts it into the firmware entry's arguments.
   The stick itself has no id (it is a different thing from an install).
3. The DATA partition records the id: the pending marker block (first MiB) carries it, and the first start formats ext4 with the
   filesystem label `IYAKKI-DATA` unchanged but the **filesystem UUID derived from the id** (`mke2fs -U`), plus a file
   `iyakki/install-id` inside. (Changing the label itself would break every older finder; the UUID is invisible to them.)
4. The finder prefers the volume whose id matches `iyakki.install=` of the running kernel command line. It falls back to the label
   alone **only when exactly one candidate exists**; with two or more candidates and no match, boot does not guess: it runs from
   RAM and the boot screen says "two IYAKKI data volumes found; none belongs to this system".
5. A system without `iyakki.install=` (the stick, an old install) behaves as today, except that it refuses a volume that carries
   another install's id when a second candidate exists.

**Code that would change.**

* `crates/iyakki-usb`: `planner.rs` (the id in `Plan`, `to_json`, `digest`), `apply.rs` (write `boot/install-id`, the marker, the
  menu edit through the `fatclone::Editor` hook, `kernel_args` for keep-ESP), `gpt_install.rs`/`install.rs` for the older paths.
* `crates/iyakki-storage`: `find_data` and `find_pending` take the wanted id and rank candidates (match, then unique label);
  `read_pending_conf` returns it; the `Content::Iyakki` check reads the UUID or `iyakki/install-id`; `mount_data`/format use the
  UUID. `gpt_hints` is unchanged (it only waits).
* `crates/iyakki-core/src/boot.rs`: `mount_data_volume` reads `iyakki.install=` through `cmdline_param` and passes it down; the
  "storage lost" and first-boot messages name the id.
* `crates/iyakki-stage1`: the first stage chooses the data volume (`DATA_LABEL`) before the real system runs, so it must apply the
  same rule from the same command line; owned by another change.
* `dist/buildroot/lean-*.sh`, `lean-sync.py`: a lean tree is already keyed by a root id; it should be written under the install id.

**Risks.**

* An install made before this change has no id: it must keep working (label fallback with a single candidate), and a stick that
  is copied to a second computer is the same install on two machines (an id belongs to one disk set, not to a computer).
* Editing the copied menu is one more place where the menu can be wrong; the existing menu tests must also cover the id.
* Re-running the installer on a disk (free-space mode, reuse) creates a new id: the old DATA partition would still hold the old id
  and is no longer found. Reuse must either keep the old id (when the DATA partition is kept) or say that the data is orphaned.
* The first stage and PID 1 must agree, or the lean boot mounts one volume and the system looks for another.
* A volume copied block for block (cloning a disk) carries the same id twice: the "no match among several" refusal covers it, but
  it needs a message the owner can act on.
