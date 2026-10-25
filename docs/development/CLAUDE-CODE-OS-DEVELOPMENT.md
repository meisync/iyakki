# Working on the IYAKKI OS with Claude Code

**For:** Claude Code (or any coding agent) that builds, installs, debugs and verifies IYAKKI itself on real hardware.
**You will get:** the rules the owner set, the build and image loop with the traps we hit, how to update a drive without wiping it,
how to debug a Mac over SSH, the install checklist, developer mode (`iyakki dev`), and how to report honestly.
Read `FEATURES.md`, `KNOWN-ISSUES.md` and the memory notes of the project first; this page is the working method.

## 1. Rules of the owner (they override your defaults)

* **Build in-house.** Prefer own code to a dependency; permissive licences only (MIT, Apache-2.0, BSD, ISC, Zlib, CC0, 0BSD, Unlicense, BSL-1.0).
  Third-party tools (ffmpeg, Claude Code, ...) are downloaded from the vendor at the user's request and never shipped in the image.
* **Incremental builds only.** Build what changed, reuse existing images and outputs, share caches (`docs/BUILD-INCREMENTAL.md`).
* **Commit only when told.** Nothing is committed unless the owner says so. The tree is dirty on purpose; do not "clean up".
* **Ask before every write to a USB drive or disk.** Never write an NVMe system disk of the development PC. A whole-image write wipes the
  data partition; an in-place update (section 4) does not. Say which one you mean.
* **No personal names or home paths in the repository.** Use `admin`/`iyakki`, placeholders, `$HOME` or environment settings.
* **Docs live in `docs/<topic>/`.** The root keeps README, SECURITY, CONTRIBUTING and CODE_OF_CONDUCT (the reorganisation is one pass, later, with `git mv`).
* **FEATURES.md is validated by a test.** A new feature needs a row with real test names (`cargo test -p iyakki-core --test feature_matrix`).
* **Report honestly.** Separate what a test or measurement showed from what you believe. "Built" is not "works on hardware".
  If a step was skipped or a test failed, say so with the output.

## 2. Repository map

| Path | What |
|---|---|
| `crates/iyakki-agent` | services: sound (`softvol.rs`, `audioout.rs`), keyboard and Touch Bar settings (`keyboard.rs`), web engine settings (`webengine.rs`), k3s dev cluster (`k3sdev.rs`), store, platform services, dev mode (`devmode/`) |
| `crates/iyakki-shell` | the screen: apps, Touch Bar (`touchbar*.rs`), browser tile (`apps/browser.rs`) |
| `crates/iyakki-core` | boot, supervisor, installer logic, tests such as `boot_menu`, `feature_matrix` |
| `crates/iyakki-stage1` | static first stage of the lean boot (mounts the data volume, builds the overlay, switches root) |
| `crates/iyakki-webview`, `dist/webview/wpe-helper/helper.c` | the web engine protocol and the WPE WebKit helper |
| `dist/buildroot/` | Buildroot external tree, board files, `repack.sh`, `lean-*.sh`, kernel fragments, `package/apple-bce` (T2 driver and our patches) |
| `dist/skills/` | skills shipped to the assistant (text is compiled into the agent) |
| `docs/` | guides; `docs/INSTALLER.md`, `docs/DEVELOPER-MODE.md`, `docs/BOOT-SPEED.md`, `docs/MAC-HARDWARE.md`, `docs/TOUCH-BAR.md` are the ones you need |

## 3. Build and test loop

* Unit tests: `cargo test -p <crate> --lib <filter>`. A full `cargo test -p iyakki-agent --lib` takes about 70 s. Some tests touch `/tmp` and
  fail when `/tmp` is full or has leftovers (`k3sdev`, `sharedfolder` flakes); rerun a failing test alone before blaming your change.
* Images are made by **`dist/buildroot/repack.sh`**, not by a full Buildroot make. The variant the owner tests on T2 Macs is `macweb`:

  ```sh
  WEB_IRIS=1 TMPDIR=$HOME/.cache/iyakki-tmp IYAKKI_SSH_AUTHORIZED_KEYS=$HOME/.ssh/<diag key>.pub \
    dist/buildroot/repack.sh macweb --base target/buildroot-web --kernel-from target/buildroot-mac [--cargo]
  ```

  `--cargo` runs the release build of the agent first (several minutes: run it **in the background** and wait for the `EXIT` line; a
  foreground call hits the tool time limit). `WEB_IRIS=1` must match how `target/buildroot-web` was built or repack refuses. Without
  `--cargo` the binary from the last cargo build is packed, so check its timestamp is newer than your source edits.
* Then the lean boot export: `cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1` (the first stage is **not** rebuilt by
  repack), then `dist/buildroot/lean-image.sh target/buildroot-macweb/images OUT --root export` (writes `initrd-lean`, `tree/`, `tree.manifest`,
  `root-id`, `grub-lean-block.cfg`; it only reads the images).
* **Traps we hit**
  * Buildroot refuses this PC's Rust `install` (uutils). Put GNU install first for that command only:
    `mkdir -p ~/.cache/gnubin && ln -sf /usr/bin/gnuinstall ~/.cache/gnubin/install` and run `PATH=$HOME/.cache/gnubin:$PATH make ...`.
  * A single package: `cd target/buildroot-web && make iyakki-webview-helper-rebuild` (also `apple-bce` in `target/buildroot-mac`). **`-rebuild` does not
    re-apply patches**: after adding or changing a kernel-module patch run `make apple-bce-dirclean` and then `make apple-bce`; check the result
    (`strings apple-bce.ko | grep one_frame_period`).
  * Repack takes out-of-tree modules from the donor's packed rootfs unless they are listed in `OOT_MODULES` in `repack.sh` (now includes `apple-bce`).
    A module rebuilt alone is silently ignored otherwise. Likewise the web helper in the image comes from `target/buildroot-web/target/usr/libexec/iyakki/`.
  * Do not `pkill -f` a pattern that appears in your own command line (it kills your shell).
  * A test copy of a component (for instance a helper with a test default) must never be left as the source default; revert it before the image build.

## 4. Updating a drive without wiping it (install)

The owner's test drive is a USB SSD with `IYAKKI-BOOT` (FAT32, about 256 MB, **almost full**: about 19 MB free) and `IYAKKI-DATA` (ext4, user data, k3s data, browser profile).
Update it **in place** with the PC (drive plugged in, Mac off):

```sh
udisksctl mount -b /dev/sdX2 ; udisksctl mount -b /dev/sdX3          # check label and size with lsblk first
IMG=target/buildroot-macweb/images/install-root
cmp $IMG/boot/bzImage $B/boot/bzImage                                 # kernel normally identical
cp $IMG/boot/initrd $B/boot/initrd && sync && cmp $IMG/boot/initrd $B/boot/initrd   # in place: no room for a second copy
cp $IMG/boot/grub/grub.cfg $B/boot/grub/grub.cfg                      # plus fonts/iyakki-menu.pf2 and themes/iyakki/theme.txt when the menu changed
sudo dist/buildroot/lean-install.sh --boot $B --data $D --lean-out OUT --yes   # initrd-lean, lean menu entries, new tree, pointers, read-back
udisksctl unmount -b /dev/sdX2 ; udisksctl unmount -b /dev/sdX3
```

* `lean-install.sh` without `--yes` prints the drive (serial, size) and writes nothing. It keeps the previous tree (`previous`) as the way back.
* If the drive is in a running Mac, `dist/buildroot/lean-remote-refresh.sh --ssh root@HOST -i KEY --lean-out OUT [--menu-from <image>/install-root/boot/grub] --yes`
  refreshes only the **boot** part. The tree on the data volume needs the drive in the PC.
* Install to an internal disk: the installer has a **Free space only** mode that adds `IYAKKI-BOOT` and `IYAKKI-DATA` in unallocated space and touches
  nothing else (`docs/INSTALLER.md`). It has never run on a T2 Mac; ask first, advise a backup.
* The GRUB menu: fast start (lean boot) is first and the default; `boot/grub/iyakki-lean-failed` (written by the first stage when it fails)
  makes GRUB default to the full boot. Check any `grub.cfg` change with `grub-script-check`.

## 5. Install verification checklist (do all, report each)

1. Before: `lsblk` shows the expected drive label, size and model; nothing else is touched.
2. After the copy: `cmp` of every file you replaced; `lean-install` prints "fresh read of N entries matches" and the new `current`.
3. The tree really contains the change (`strings` for a new string, size of a binary, module parameter present).
4. Unmounted cleanly (`udisksctl unmount`), then boot on the Mac and check on the device (section 6): uptime, the boot entry used
   (`/proc/cmdline`; a lean boot has an overlay root: `mount | grep ' / '`), versions (`/etc/iyakki-release`), the feature you changed.
5. State what was not seen (screen, sound, hardware) and ask the owner to look at exactly that.

## 6. Remote debugging on the Mac

* Boot the entry **"fast start + remote diagnostics (SSH, keys only)"** (kernel argument `iyakki.ssh=1`). Dropbear listens on 22 with the key baked in at
  repack (`IYAKKI_SSH_AUTHORIZED_KEYS`). The host key is stable (`/var/lib/iyakki/ssh/`). Use
  `ssh -i KEY -o ConnectTimeout=8 -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@HOST`.
  **The address changes**: when it times out, ask the owner for it (the screen shows it); do not scan the network.
* The userland is BusyBox: no `pgrep`, `timeout`, `curl`, `ldd`, `top -p`; `date` has no milliseconds; wget has no HTTPS. Use `ps | grep '[x]'`,
  `/proc`, shell loops. Dropbear's small window caps a file copy at about 23 Mbit/s (not the network). Pipe files with `cat file | ssh ... 'cat > /tmp/x'`.
* **Where to look.** `dmesg`; `/var/lib/iyakki/boot-lean.log` (boot log of the lean boot, includes `iyakki: [iyakki INFO ]` lines and a shell frame line every 10 s);
  `/var/lib/iyakki/log/kmsg-previous-*.log` (earlier boots); `/var/lib/iyakki/settings.json`; `/etc/asound.conf`; `/proc/asound/card0/pcm0p/sub0/{status,hw_params,sw_params}`;
  `/sys/class/backlight/*`; `/sys/module/<driver>/parameters/*`; `/proc/interrupts`; `/proc/<pid>/task/*/{stat,comm,wchan}`.
* **Measure, do not guess.** Per-thread CPU: read `/proc/PID/task/*/stat` fields 14+15 twice, 10 s apart, subtract (ticks are 1/100 s; 1000 ticks in 10 s is one full core).
  GPU use of a process: `drm-engine-render` in `/proc/PID/fdinfo/*`. Overall load: two reads of the first line of `/proc/stat`. Audio: the PCM `status` file
  (`RUNNING` with `hw_ptr` rising is playing; `PREPARED` with `appl_ptr 0` is open but silent). `aplay` prints `underrun!!!` on stderr without `-q`.
* **Live changes without a reboot** (memory only, gone after reboot): bind-mount a file over the real one
  (`mount --bind /tmp/new /etc/asound.conf`, `umount` to undo); you can bind a helper binary over `/usr/libexec/iyakki/webview-helper` (the owner reopens the browser).
  The agent rewrites `/etc/asound.conf` when the output changes; a bind mount stops that. Write sysfs parameters directly. **Never `rmmod apple_bce`** on a T2 Mac: the
  internal keyboard, trackpad and Touch Bar hang off it, and capture streams have hung the machine.
* Always put a temporary change back (`umount`, restore the copy) and say so.

### Hardware facts learned on a MacBookPro16,2 (T2)

* **Sound** (`apple-bce` `aaudio`, card `AppleT2x4`): speaker is device 0, 4 channels S24_LE 48 kHz, a single subdevice, so **one program at a time**. The stock driver had a
  one-frame period; our patch `0003` gives real periods (256 to 4096 frames; module parameter `one_frame_period=1` restores the old behaviour). **dmix spins about 90% of a core**
  on this card whatever its settings; the speaker therefore uses `plug > softvol > plughw:0,0` (about 2% CPU). A shared mixer of our own is a later idea (memory note
  `project-own-mixer-t2-audio`). Consequence: pages that open several media elements hold the device in the first one, so the web helper gets
  `IYAKKI_WEBVIEW_AUTOPLAY=deny` from the agent when `/etc/asound.conf` has no dmix.
* **Touch Bar**: the IYAKKI-drawn strip never opens on this Mac (the USB configuration switch falls back every boot). What you see is the T2's key row. The kernel driver dims
  the backlight after 60 s and turns it off 15 s later (`hid_appletb_kbd` parameters `autodim`, `dim_timeout`, `idle_timeout`); the agent sets `autodim` off and relights a dark bar.
* **Browser rendering**: the helper must use the GPU route (EGL, `drm-engine-render` growing); in shared-memory mode all eight llvmpipe threads burn CPU. There is no hardware video decoder
  in the image, so video decode is CPU (a few `multiqueue*` threads).
* **k3s dev cluster** runs in the background (Prometheus, Elasticsearch ... in test namespaces) and takes a real share of CPU; stop it before judging browser or audio performance.

## 7. Developer mode (`iyakki dev`): adb for IYAKKI

Full reference: `docs/DEVELOPER-MODE.md`. Summary:

* On the device (administrators): Settings -> Developer mode -> `e` switch on (off by default, auto-off after 4 h), `p` shows a one-time pairing code and the certificate fingerprint. TCP 7979 over TLS.
* On the PC: `iyakki dev pair ADDRESS` (compare the fingerprint with the screen), then `whoami`, `info` (model, kernel, modules, sound, GPU, firmware), `status`, `ps`, `dmesg [--follow]`,
  `logs --system [--source shell|boot|audit]`, `files push|pull|ls`, `screenshot [-o FILE.png]`, `input tap|key|text`, `config get|set|export`, `forward LOCAL:REMOTE`,
  `shell [-- CMD]` (asks on the device's screen), `reboot` / `poweroff` (ask every time).
* Scopes decide what a paired computer may do (`debug`, `input`, `forward`, `power`, `update`, shell). The owner grants them per computer on the device. Do not ask for more than you need.
* **System update**: `iyakki dev update-pack IMAGE.img -o payload` then `iyakki dev update payload [--reboot]`: checks first, staging on the data volume, the way back
  (`IYAKKI (previous version)` boot entry) before any replacement, read-back after. It replaces boot files only; it is not a substitute for the lean tree update in section 4.
* SSH diagnostics and developer mode overlap; use SSH for bring-up on new hardware (works before the shell is up, needs the diagnostics entry), developer mode for day-to-day work
  that must be audited.

## 8. Working agreements for a session

1. Say what you will change, change it with tests, run the narrowest tests that cover it, then the crate's lib tests.
2. Long builds in the background with a log file and a `EXIT n` line; wait for the line, not for the launcher's "completed" notice.
3. One image per round: collect fixes, then repack once. Tell the owner what is in the image and what to look at.
4. When the owner reports a symptom ("sound breaks", "icon not visible"), get a measurement from the device at the moment it happens before you design a fix; revert a fix the measurement does not support.
5. Update the memory notes and the docs you touched; keep `FEATURES.md` and `KNOWN-ISSUES.md` truthful.

## 9. Open items (2026-10-24)

* Fullscreen in the browser tile (YouTube) is being implemented; not yet in an image.
* Own mixer for the T2 speaker (several programs at once without dmix's cost).
* Install to the internal NVMe free space (about 490 GB on the owner's MacBook) is planned after the drive update.
* Docs reorganisation into `docs/<topic>/` (one pass).
* Not verified on hardware in this round: the new GRUB menu look, the failed-start fallback, the metrics-server in the dev cluster, the Touch Bar fix after a reboot.
