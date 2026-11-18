# BIOS PC and QEMU first: broken update fall-back, release file, self test

Plan file: `pc-bios-and-qemu.md` | Checks: 7 | Estimated time: about 78 minutes (1.3 hours)

Use `RESULTS-TEMPLATE.md` to report. Lines marked with (A), (code) or 'add ...' in the row text are developer tasks: skip that part and test the rest.

## What you need

- A build PC with QEMU and the built image (the developer gives you the commands).
- An older PC that boots in BIOS (legacy) mode.
- A second stick.

## Safe start

QEMU runs never touch real disks. On the BIOS PC boot from the stick and do not install.

## How to collect logs

Use developer mode on the test machine (Settings -> Developer mode; approve the pairing on the machine). From the paired computer:

- `iyakki dev diag` : one bundle with state and recent logs. Attach it to every failed row. It holds no secrets (a row checks this; if you see a key in it, tell the owner and do not share it).
- `iyakki dev logs --system` : the system log. Attach the lines around the failure.
- `iyakki dev dmesg` : the kernel log, for boot, Wi-Fi, sound and USB problems.
- The data-folder logs: files under `/var/lib/iyakki/log` (for example `engine.log` for the browser and `kmsg-<boot-id>.log` after a crash). On a machine with no network, copy them to a USB stick from the Files tile.
- A photo of the screen helps for any display, freeze or layout problem. Say how long after the action it happened.

## Order of checks

1. QEMU rows first (cheap), then the BIOS PC, then the UEFI PC.

The checks below are already in this order. Do not skip a failed early row: write it down and go on.

## Checks

Fill the Result line: pass, fail or note. For a fail, attach the files named in the row.

- [ ] F-BOOT-01 Boot on real PCs: test matrix (UEFI/BIOS laptops, desktops, mini PCs) and a hardware compatibility list: do this: Run docs/BOOT-TEST.md on more PCs and fill HARDWARE-MATRIX; a living row, never fully closes -> expect this: it works as the row says; the open point is: Progress 20%: only Mac mini, T2 MacBook and one AMD PC tested; Intel and NVIDIA PCs pending; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-BOOT-33 Every boot entry restarts the machine 20 s after a kernel panic (`panic=20`); no `oops=panic`, no soft-lockup or hung-task panics (reasons in docs/TROUBLESHOOTING.md): do this: Build an image and provoke a panic in QEMU to see the 20 s restart **(can destroy test data or stop the machine: read the step twice)** -> expect this: it works as the row says; the open point is: Notes: menu text only until an image carries it; a real panic not provoked; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-BOOT-35 System update of a lean install (`dev update`, `devmode/update.rs`): a payload made from a lean image (`dev update-pack`; the small `boot/initrd-lean` counts as the RAM disk, the previous-version entry keeps the one that was running) carries `boot/root.sfs`; it is staged on the data partition, moved to `iyakki/root/<version>-<sha8>/root.sfs` (size and SHA-256 checked), the old `current` pointer becomes `previous` and `current` is switched through a temporary file and a rename; the seed `boot/root.sfs` on the boot partition is never replaced; the kernel and the 1 MB initramfs are replaced after it by the usual steps; the boot menu's previous-version entry starts the old kernel with `iyakki.root=previous` (first stage option: the root the update replaced, or the seed when there was none, never the new one); roots no pointer names are removed; an image already in place or equal to the installed seed is not sent again: do this: Run dev update on a lean install in QEMU then the Mac -> expect this: it works as the row says; the open point is: Notes: no lean install updated this way on a real machine; kernel/module match not checked; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-UPD-02 Boot health check and automatic rollback with a boot counter: an install or rollback adds a guard to `grub.cfg` (only together with the previous-version entry) and sets `iyakki_trial=1` in `boot/grub/grubenv` (a 1 KB environment block written here); the guard moves it to 2 and tries the new set once; the shell sets 0 after its first picture with the data volume mounted, otherwise the menu boots "IYAKKI (previous version)" by itself; nothing is armed without a guard and a fall-back entry; the previous set is kept until the next successful update: do this: Boot a QEMU image with a broken update (BIOS and UEFI) and then the Mac mini; confirm the counter falls back to the previous entry. Check loadenv module in the image GRUB -> expect this: it works as the row says; the open point is: 'NOT tried on the Mac or in QEMU with a broken bundle'; unproven that GRUB has loadenv and grubenv works on real BIOS/UEFI/Secure Boot; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-UPG-06 Upgrade run in QEMU: a data disk made from the alpha.1 fixtures is booted by the current image; the boot changes nothing on the volume, then the stores are opened by the new code and the settings, sources and inbox come through, the backup is the old file, a second run changes nothing: do this: Build image, run qemu-upgrade-test.sh, then run checklist on Mac mini and MacBook -> expect this: it works as the row says; the open point is: Stage needs a newly built image; not run on Mac mini or T2 MacBook; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-REL-01 Version everywhere: one workspace version (`0.1.0-alpha.1`) inherited by every crate, followed by the separate GUI workspace and both lock files; `iyakki --version`, the `IYAKKI <version> starting` boot line, the window title, the SYSTEM overview "About" row, and `/etc/iyakki-release` (version, build date, git describe, variant, kernel) written into every image by `post-build.sh` from values `build.sh` exports: do this: Build an image, boot in QEMU and look at About and the release file -> expect this: it works as the row says; the open point is: Notes: /etc/iyakki-release step has not run on a built image; About not looked at on a screen; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-SELFTEST-01 Boot self-test "smallest call per path" (`iyakki-agent::pathcheck`, NEXT-PLAN N1): after the existing lines the self-test starts a loopback HTTP listener (never the internet) and runs, through production code with no fake, the CA bundle load, the image programs on PATH (`sh ip git ssh aplay arecord amixer`), platform connect (`NetHttp`), registry probe (`netprobe`), registry setup (`ocipush::TcpHttp`, any-certificate reachability), cluster tools (`wsexec::System`, a missing kubectl answered in words), the Git client (`NetGit` and `git --version`) and DOCTOR's probes; each prints `IYAKKI-SELFTEST path <name> ok` or `FAILED <why>`; `dist/buildroot/qemu-test.sh` requires every name and fails on any FAILED: do this: Repack image, run qemu-test.sh and check the pathcheck lines; then Mac mini boot -> expect this: it works as the row says; the open point is: No QEMU run yet; images predate the step and need repacking; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

