# When the boot drive disappears

**For:** anyone who runs IYAKKI from a USB SSD (or any external drive) and saw "I/O error" everywhere, and for people who work on the storage code.
**You will get:** what happened, what IYAKKI now does about it, what can and cannot be saved, and how to test it on a real machine.

## What was seen

On a 2020 MacBookPro16,2 running from a Samsung PSSD T7 over USB-C (kernel 6.12, `uas` driver) the kernel logged, at uptime 3088 s:

```
usb 2-2: USB disconnect, device number 2
usb 2-2: cmd cmplt err -108
uas_zap_pending ...
I/O error, dev sda, sector ... WRITE
EXT4-fs (sda3): failed to convert unwritten extents ... potential data loss!
```

Within 0.6 s the same drive came back as a NEW device `sdb` (`sdb1 sdb2 sdb3`, same serial). Nothing mounted it again, so `/var/lib/iyakki`
and `/home` stayed mounted on the dead `sda3`: every read and write answered EIO while the system, which lives in RAM, kept running.
The camera could not save, the chat said "work folder missing", Claude "failed to load", the AI IDE said "io error".
Whether it was a loose cable, a power glitch on the bus or the T7 and UAS misbehaving is **not known**; this has been seen once, in the log above,
and has not been reproduced.

## The states

The watcher (`iyakki-core/src/storageloss.rs`, decisions in `iyakki-storage/src/loss.rs`) looks once a second, on its own thread, without starting a
process. It reads `/proc/self/mountinfo` and `/sys/block/<disk>/device/state`, and writes one byte to the hidden file
`/var/lib/iyakki/.iyakki-probe` and fsyncs it (once a second; one 4 KiB block a second is negligible wear). The write runs on a helper thread and is waited for 1.5 s; a probe still stuck from before is not started
twice, so a hung device never hangs the watcher.

| State | Meaning | How it is noticed |
|---|---|---|
| Healthy | all is well | |
| Lost | the disk behind the mount is gone or offline | `/sys/block/sda` is gone, or its `device/state` is not `running`, or nothing is mounted at the data folder |
| Errors | the disk is there but fails | two probes in a row failed (EIO) or timed out; or ext4's `errors_count` grew since the start |
| ReadOnly | the filesystem was remounted read-only | `ro` in the mount options |
| Reattachable | the same filesystem is on a new node | a partition with the **same UUID and the label IYAKKI-DATA**, mounted nowhere, on another device number |
| Reconnecting | a reattach is running | |
| Failed | reconnecting did not work (the reason is shown) | |

One failed probe is a hiccup; the state changes at the second. When the old mount works again by itself the state goes back to Healthy.
Every change is a line in a RAM ring (200 events): "storage: drive lost at uptime 3088 s (the device is gone)", the USB port path, hub and speed of the drive
(read while it was still there), "drive back on a new device", "drive reconnected as sdb3 at uptime ...". The audit log lives on the dead drive, so
the ring is written to it (`storage.event`) by the shell after the drive is back, each event once.

## What the person sees

A card under the top bar on **every screen** (sign-in, launcher, apps, panes), drawn above everything: *"The IYAKKI drive was disconnected for a moment, so
saving is not possible right now. Check the USB-C cable. Anything that was being saved at that moment may be lost."* with **Reconnect now**, **Restart**
and **Minimise**. It cannot be dismissed while the drive is not healthy; minimised it is a slim strip that opens it again. A new loss opens it in full.
After a failed reconnect the buttons are **Restart**, **Check the drive** and Reconnect now, and the reason is in the text. When the drive is back a card says
"Drive reconnected, everything is saved again." (OK, or it goes by itself after about ten seconds).

Apps say "The IYAKKI drive is not available: it was disconnected. Check the USB-C cable ..." instead of a bare I/O error: `scope::explain_io`
(files the assistant and the pickers touch), every app's red status line (`fx::status_line`: camera save, chat work folder, AI IDE, ...), the chat's error line,
the TALES note, the toast, and the Claude Code / Copilot status and failure text. The wording changes only while the state is not Healthy.

## Reconnecting

Automatic, when the state is Reattachable and the new node has been there for **3 s without a flap**, and only for the exact UUID that was mounted
(a different disk, or one that merely carries the label IYAKKI-DATA, is refused and noted once in the ring). Manual: **Reconnect now** (same path, without the 3 s wait),
or `iyakki storage reattach` (asks the watcher through `/run/iyakki/storage-reattach.request`, waits for the answer in `/run/iyakki/storage-state`;
`iyakki storage status` prints the state).

1. stop the registered long-running parts that keep files open (the k3s dev cluster, through its own `stop`);
2. mount the new node on a staging folder `/run/iyakki-reattach` (`ext4`, `rw,nosuid,nodev,relatime`; **the mount replays the journal**). If this fails nothing else was touched: the stale mounts stay (no silent writes into RAM), state Failed;
3. lazily unmount (`umount2(MNT_DETACH)`) the stale mounts, deepest first: what is mounted under the data folder, the data folder, `/home`;
4. move the staged mount onto `/var/lib/iyakki` (a direct mount if the move is refused), bind `<data>/home` over `/home` again, make the runtime folders;
5. probe: it must write again, else Failed;
6. start the stopped parts again, remember the new device, state Healthy, write the ring to the audit log, show the card.

fsck is **never** run automatically. "Check the drive" runs `e2fsck -n` (read only) on the returned node when the tool is installed, otherwise shows the end of the kernel log.

**What re-opens by itself:** everything that opens files by path (the audit log, the settings and account stores, the apps). **What does not:** a program whose
working folder or open file was on the old mount (an open terminal in `/home/<name>`, a long-running tool started from there) keeps seeing the dead device until it is
restarted; Restart in the banner is always available. Other k3s clusters than the first are not stopped and started by the reattach (the first dev cluster is).

## What can and cannot be recovered

* What was in flight at the moment of the loss may be **lost**: the kernel logged "potential data loss" for writes it could not complete; ext4's journal keeps the filesystem
  consistent, not every file complete. A photo, a chat message or a file written in the last seconds before the loss can be missing or short.
* Nothing is repaired automatically. If the journal cannot be replayed the person sees the reason and chooses Restart or Check the drive.
* Data that only existed in RAM (the open apps' unsaved state) is still there, and can be saved after the reconnect.
* A drive that stays away (cable out) stays Lost; IYAKKI keeps running from RAM and keeps telling the person.

The kernel log copy that is kept for crash analysis (`/var/lib/iyakki/log`, docs/TROUBLESHOOTING.md) is written only while the state is Healthy; lines that arrive while the drive is lost or in error are dropped and counted.

## Prevention

* At start and after a reattach `power/control` is set to `on` (no runtime suspend) for the USB device and the hubs above it of every USB disk that holds IYAKKI-DATA or the FAT partition IYAKKI-BOOT.
  The kernel's `usbcore.autosuspend` default is not changed.
* ext4 is mounted with the defaults: journal on, `commit=` 5 s, `errors=` the filesystem's own setting. `errors=remount-ro` would not help when the device vanishes, so it is not forced.
* Settings -> Storage shows how the drive is connected: `SuperSpeed+ 10 Gbit/s` or `High-speed 480 Mbit/s`, the port (`2-2`, or `2-1.3` behind a hub), the driver (`uas` or `usb-storage`). It warns when the drive runs at USB 2 speed (usually a worn or USB 2 cable)
  and when it is behind a hub that also serves other devices. The same facts are one line in the log at start and at the loss.
* **Compatible USB mode (no UAS), optional, off by default.** A GRUB entry *"IYAKKI - compatible USB mode (no UAS)"* adds `usb-storage.quirks=04e8:4001:u` (the Samsung T7), and
  Settings -> Storage, key `u` (administrators) does the same for **the drive IYAKKI is running from**, for every later start: it writes `boot/grub/iyakki-usb-compat.cfg` on the boot partition
  (`set iyakki_usbq="usb-storage.quirks=<id>:u"`; the id is checked to be 4+4 hex digits), which every boot entry reads; the file is removed to go back. Trade-off, plainly: without UAS a drive is
  noticeably slower on random reads and writes (no command queueing; sequential speed is usually close), and in exchange it uses the older, simpler driver path that some drives and some USB 3.2 Gen 2 hosts handle more steadily.
  Whether it would have prevented the loss above is **unknown**; try it if the drive keeps disconnecting. The setting takes effect at the next start.

## Testing on a real machine (only after the fakes pass)

Everything above is tested with fakes (`cargo test -p iyakki-storage --lib loss`, `-p iyakki-shell --lib loss`). The real disconnect path has not been reproduced. On the MacBook:

1. Boot from the T7, sign in, open Settings -> Storage: check the line "runs at SuperSpeed+ 10 Gbit/s on USB port ...".
2. Over SSH: `iyakki storage status` (`drive: Healthy`).
3. Unplug the USB-C cable at the **Mac end**, count to 3, plug it back in (the drive comes back as a new `sdX`). Expect within about 3 s: the banner "disconnected for a moment"; `dmesg` shows the disconnect; `iyakki storage status` shows `Lost`, then `Reattachable`.
   About 3 s after the new node is steady the banner says "Reconnecting", then the "Drive reconnected" card; `ls /home/admin` works; `mount | grep iyakki` shows the new device; Settings -> Storage works.
4. Check the audit log (it is shown like every other event): `storage.event` lines for the loss and the reconnect.
5. Failure variant: unplug, wait 30 s with the cable out (stays Lost, banner stays), plug in, press **Reconnect now** before the 3 s are over.
6. If anything is off: `iyakki storage status`, `dmesg | tail -50`, and the log lines starting `storage:`.
