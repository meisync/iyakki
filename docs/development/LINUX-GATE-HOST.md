# A Linux computer as a gate runner

Any Ubuntu or Debian computer with KVM can run the workspace tests and the QEMU boot tests, so a build does not have to wait for the development PC.
`dist/release/setup-linux-host.sh` makes a computer ready (apt packages, rustup, the Rust targets) and checks it; `--check` only checks.

```
dist/release/setup-linux-host.sh
. dist/release/env.sh
setsid nohup dist/release/test-with-rerun.sh target/alpha-check > target/alpha-check/summary.log 2>&1 < /dev/null &
```

`IYAKKI_RUST=1.99.0` pins the Rust toolchain (the default is `stable`). Never build on a person's own computer when a runner exists.

## What differed on a fresh Ubuntu 26.04 host (2026-11-13) and what fixed it

| Difference | Effect | Fix |
|---|---|---|
| Cargo 1.100 puts a test program in `target/debug/build/PKG/HASH/out/`, older cargo in `target/debug/deps/`. Four test helpers looked for the fake web engine two folders above the test program | about 60 browser, media, web agent and IDE tests: "could not build the fake helper" | the helpers find the profile folder (`debug` or `release`) among the program's parents; `iyakki-fw`'s kernel test uses the same rule |
| A program started in the background by a non-interactive shell (`nohup ... &`, a CI runner) has SIGINT and SIGQUIT ignored, and ignored signals survive `exec` | Ctrl+C never stopped `sleep` in the terminal tests (`iyakki-vt` and the dev terminal) | the pty children reset both signals to their default before `exec` (a real defect: a person's terminal command must get normal signals) |
| Cargo 1.100 warns about an unused workspace dependency (`iyakki-hw`) | one warning per build | the unused entry is removed from the root `Cargo.toml` |
| Ubuntu 26.04 uses uutils coreutils (`sleep`, `install`, ...) | none of the tests needed GNU; `install` is why the release scripts put their GNU shim first | none |
| Fixture files ignored by `*.log` in `.gitignore` are not in a clone | two `upgrade_tests` fail with NotFound | see below |

Result on the host after these fixes (head 3bf1951 plus this branch, `dist/release/test-with-rerun.sh`): 6 tests failed in the parallel run, 2 of them passed alone
(`jobs::tests::a_real_child_is_seen_alive_...` reads `/proc/PID/comm` between fork and exec, a race under 16 busy threads; `apps::settings::tests::the_voice_test_never_opens_the_real_sound_device`),
and 4 failed alone. None of the 4 is a host difference:

* `apps::settings::modelopts_tests::the_page_has_the_option_controls_inside_the_screen_at_four_sizes_and_they_work_by_click`: at 800 x 480 the Models page has no list button
  (the Gemini block of `settings.rs` pushed it off the screen).
* `monkey_tests::overflow_every_tile_at_the_largest_text_size`: the cameras tile overflows at 360 x 740 and 800 x 480.
* `upgrade_tests::the_audit_log_of_alpha1_verifies_...` and `::a_line_cut_short_by_a_power_loss_...`: they read `tests/fixtures/upgrade/alpha1/audit.log` and `current/audit.log`, which
  `.gitignore` (`*.log`) kept out of git, so a clone has none. `.gitignore` now has `!tests/fixtures/**/*.log`; the two files must be added once from the computer that has them
  (`git add -f tests/fixtures/upgrade/*/audit.log`); they cannot be rebuilt here because the alpha.1 file is a recorded hash chain.

## The boot loader (iyakki-boot) on a Linux host

The loader is a separate repository (`iyakki-boot`, a submodule). Needs the Rust target `x86_64-unknown-uefi` (the setup script adds it; stable is enough).

```
cd iyakki-boot
cargo test --locked                                                     # host tests of boot-core and the rest
cargo build --release --locked --target x86_64-unknown-uefi -p boot-uefi -p test-efi
tools/qemu-test.sh                                                      # QEMU + OVMF tests a to m (a2 needs --lean-image IMAGE.img)
BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg cargo build --release --locked --target x86_64-unknown-uefi -p boot-uefi --target-dir target-cfg
```

The last build is the one an image carries (`dist/buildroot/loader-build.sh`): the config file name is compiled in. `qemu-test.sh` needs the one WITHOUT it (it reads `/boot.cfg`).
Test `m` (new 2026-11-13): a `boot.cfg` that names `/boot/initrd` on a volume that has only `/boot/initrd-lean` (and the reverse) must start with the sibling and log
`initrd: NAME cannot be read; SIBLING is used instead`; with neither file the start is refused as before.

## Boot an image through the loader, not GRUB

`dist/buildroot/qemu-loader-boot-test.py IMAGE.img [--layout lean|sfs|sibling|all] [--loader boot-uefi.efi] [--wait N] [--keep]`

`dist/buildroot/qemu-patch-layer-test.py LEAN-IMAGE.img [--loader boot-uefi.efi] [--wait N]` boots the lean image twice per case on a private copy: once to make the data volume, then with a patch layer planted by debugfs (A a whole layer is laid over the root, B a damaged layer.txt is skipped, C boot-count 3 switches patches off). The system must come up in every case (F-DEVMODE-09).

The existing `qemu-lean-test.py` and the other QEMU scripts boot through GRUB. This one makes a private copy of an image (never the image), puts the loader at
`EFI/BOOT/BOOTX64.EFI` (and the shim's second-stage name when the volume has one), sets the layout, and boots it with OVMF (KVM when present):

* `lean`: `/boot/initrd-lean` present, no `/boot/initrd`, `boot.cfg` names `/boot/initrd-lean` (seed and stick images)
* `sfs`: `/boot/initrd` present, no `/boot/initrd-lean`, `boot.cfg` names `/boot/initrd` (loader-default sfs images)
* `sibling`: only `/boot/initrd-lean`, `boot.cfg` names `/boot/initrd`: the loader must log the sibling line (needs `--loader` with a build that has the rule)

A layout passes when the serial console shows no GRUB, no unreadable file, and the first stage's `switching root`. Exit 0 all passed, 1 a failure, 77 a tool is missing.

IMAGE is an `iyakki-lean-x86_64.img` of `dist/buildroot/lean-image.sh` (needs a finished Buildroot build: `dist/buildroot/nightly-push.sh`, or `lean-image.sh IMAGES_DIR OUT --root seed`
on an `images/` folder) made with the loader staged (`boot-loader-stage.sh`; without `iyakki-boot` built there is no `EFI/iyakki-boot/loader.efi` and the script says so; `--loader` supplies one).
Without an image, `--synthetic [--kernel BZIMAGE]` builds a small fixture from a kernel with the EFI stub (for example a copy of the computer's `/boot/vmlinuz-*`, which is
readable only by root) and a static first stage that prints the same two lines; it proves the loader and the script, not the IYAKKI system.

## Kernel tests and QEMU proofs (M80, 2026-11-14)

The Linux-only pieces that unit tests on a Mac cannot run are checked on a Linux host in two ways, both opt-in (nothing in `alpha-check` calls them):

* `dist/linux-host/kernel-tests.sh` runs the ignored real-kernel tests as root inside throw-away network and mount namespaces (`sudo unshare -nm`; the host's network, routes,
  firewall and mounts are never changed, nothing is installed): `iyakki-fw` (`tests/kernel.rs`, `counters_sets_kernel.rs`, `tablegen_kernel.rs`: our nf_tables encoder against the
  real kernel, `nft list` decodes it, a second namespace is let in or dropped, blocked attempts counted and logged, the VPN guard and DNS-redirect tables) and `iyakki-agent`
  `tests/vpn_kernel.rs` (two real WireGuard devices configured through IYAKKI's own netlink messages, handshake, traffic, teardown). Ubuntu 26.04 restricts unprivileged user
  namespaces (`apparmor_restrict_unprivileged_userns=1`), so the older `unshare -rn` as a normal user skips itself there ("passes with a note"); this script runs the same
  tests as root, from a world-readable scratch copy (a root process inside a user namespace cannot read a private home folder). A test that skips itself is reported as NOT RUN.
  On 2026-11-14 (kernel 7.0, nft 1.1.6): all programs ran and passed; one test needed a fix because nft 1.1 prints `counter packets 0 bytes 0` inside rules.
* `dist/qemu/*.sh` boot a built image in QEMU/KVM (see `dist/qemu/README.md`): sandbox network, boot-order recording and prefetch, compressed memory, BIOS boot of a shared disk. All four
  PASS on 2026-11-14 on a libre image of head dc9748435 (with `CONFIG_FANOTIFY=y` added to the kernel for the prefetch test).

Use `systemd-run --user -p ManagedOOMPreference=omit` for the long ones.

Also (M80, same day): `dist/linux-host/kernel-tests.sh` now runs `tests/vpn_kernel.rs` with two tests (WireGuard; the VPN kill switch, fail-closed guard and DNS-redirect table on real
traffic between two namespaces), the netscan lib test `real_avahi_tests` (our mDNS announcer and resolver beside a real `avahi-daemon` without D-Bus on its own tmpfs `/run`, over a veth cable, a real `dig` reading our records, a name conflict in both directions; `ONLY=avahi dist/linux-host/kernel-tests.sh` runs just it) and `tests/disks_crypt_kernel.rs` (the DISKS helper runner against real `cryptsetup` and dm-crypt, LUKS1 and LUKS2, on a loop device in a scratch folder; the
mapping and the loop device are removed at the end). `dist/linux-host/real-tools-tests.sh` (no root) runs our terminal session against the real ssh client and sshd (F-CLI-08) and, with `google-chrome` and node 22, the browser voice helper page and the client and window pages in a real headless Chrome (F-VOICE-06, F-CLIENT-01 to 03), checks the partition tables and FAT32 volumes our partition manager writes with `sfdisk`,
`fsck.fat`, `blkid` and mtools. `dist/qemu/install-entry-test.sh` reads the install stick's GRUB entry and the installer's first page on the text console with `tesseract` (installed by
`setup-linux-host.sh`, like `cryptsetup`).

Also (M80): `dist/linux-host/hwsim-hotspot-test.sh` builds a small kernel with `mac80211_hwsim` (about 3 minutes, once) and runs the Wi-Fi hotspot test: our access point against the host's real
`wpa_supplicant` on virtual radios inside QEMU, 15 checks, 42 s (F-HOTSPOT-03, 04, 08, 09, 16, 17). `dist/qemu/drive-loss-test.sh` pulls and re-plugs a virtual USB data disk (banner on the screen,
reattach with real mount calls). `dist/qemu/homebind-test.sh` shows /home on the data disk surviving a power-off. `dist/qemu/startup-disk-test.sh` (OVMF real UEFI variables) `dist/qemu/sound-output-test.sh` (emulated HDA and USB audio cards) and `dist/qemu/computer-profile-test.sh` (real DMI strings from QEMU -smbios), `dist/qemu/usb-awake-test.sh` `dist/qemu/fnkeys-test.sh` (media keys through the kernel's evdev) and `dist/qemu/pointer-speed-test.sh` (a mouse through the same) are in the same family. The boot stages `homebind`, `driveloss`, `startupdisk`, `sound`, `computer`, `usbawake`, `fnkeys` and `pointer` are new: an image built earlier needs
the new `iyakki` program laid over its RAM disk (append a second zstd cpio with `usr/bin/iyakki` to `rootfs.cpio.zst`; the kernel unpacks the later archive over the earlier one).

Also (M80, later): `dist/linux-host/webservice-test.sh` starts the built `iyakki serve --profile cloud` as the normal user in a private mount namespace (its own /var/lib, an empty /dev/snd, shims for the
sound programs, `IYAKKI_IN_SANDBOX=1`) and drives it with the real `curl`, `openssl` and `git`: health routes, HTTPS only, sign-in and token, 401 and 403 rules, `/metrics`, the absent hardware routes, the file
routes (mkdir, move) and the git routes on a real clone; it also checks that the cloud profile starts no sound program and opens no sound device. `dist/qemu/shell-pages-test.py` boots the real image in
QEMU on a virtual screen, creates the first account by typing, drives Settings pages, the Devices & System tiles, the volume keys and the lock screen with the keyboard and reads the screenshots with tesseract
(F-DISP-06, F-SND-04, F-KBD-02, F-CLOUD-01, F-DISKS-01, F-DISKS-06, F-DOCTOR-02, F-LOCK-01 and more); it keeps the pictures with `--out`. Every root test and script here follows the shared-machine rules: it runs only
inside `unshare -nm`, with a private tmpfs where it must write, `IYAKKI_IN_SANDBOX=1`, scratch paths for every daemon, and the `/dev/null`, `/dev/zero` and `/dev/urandom` check before and after.

Also (M80, later): `dist/linux-host/cloud-listen-test.sh` runs the built `iyakki cloud listen` as root in `unshare -nm` (IYAKKI_IN_SANDBOX=1, tmpfs on /run and /var/lib) on the real ports 80 and 443 with the real nft gate, and checks the table after SIGTERM, after SIGKILL plus a restart, and after SIGINT.
