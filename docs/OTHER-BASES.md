# IYAKKI on another base operating system

The Buildroot image (Linux 6.12 + musl + BusyBox) is the *tested default*, not a requirement. IYAKKI is one static program
(`iyakki`) that touches the base system in a short, listed way (`BOUNDARY.md`): a Linux kernel interface, a few ordinary
programs, a few files. So it runs on anything that has that.

## As a program or a service on a Linux distribution  (tested)

Build the static binary once (`cargo build --release --target x86_64-unknown-linux-musl -p iyakki-core`, or take it from a
release), copy it anywhere, and run it. It needs no libraries, so the distribution does not matter.

```
./iyakki detect                                   # where it thinks it is
./iyakki user add admin --role admin --password-stdin < password-file
./iyakki serve --web                              # the supervisor and the desktop in a browser (https://this-machine:8443)
./iyakki web http:8080                            # only the browser front end, plain HTTP behind your own TLS proxy
./iyakki run guest.wasm                           # a WebAssembly guest in the sandbox
```

`dist/test-bases.sh` runs exactly this on **Alpine, Debian, Ubuntu, Fedora and Arch Linux** (in containers) and checks that the
program starts, runs a guest, serves the page and signs an account in: all five pass.

As a service: `dist/systemd/iyakki.service` (copy it to `/etc/systemd/system/`, make the `iyakki` user, `systemctl enable --now
iyakki`). It was run on a real systemd with the unit's hardening (a dynamic user, the file system read-only apart from
`/var/lib/iyakki`, no capabilities, no new privileges, only internet and local sockets): it started, detected `baremetal`,
and answered. The window (`./run.sh window`) needs a graphical desktop and is built from source (`crates/iyakki-gui`).

What a distribution does **not** give you without more work: the framebuffer desktop (IYAKKI as PID 1 drawing on the screen
itself), driver autoload and the container cluster's cgroup setup, which the Buildroot image arranges. Use the browser front
end or the window instead.

## Swapping the Buildroot base for another (checklist)

The image is made by `dist/buildroot` (a Buildroot external tree). To use another base (Yocto, Alpine's `mkimage`, a Debian
`debootstrap`, a hand-built initramfs) the new base must provide:

1. **The kernel** with the options in `dist/buildroot/board/iyakki/linux.fragment` (`verify-kernel-config.sh` checks that a
   kernel source tree honours every one; the container and nftables options are what k3s needs).
2. **`/init`** that starts `/usr/bin/iyakki serve` as PID 1, or any init that does so and keeps it running.
3. **The programs in `BOUNDARY.md` table B** that you use: `modprobe` and `mdev` (drivers), `ip` / `wpa_supplicant` / a DHCP client
   (network), `aplay` / `arecord` (sound), `mke2fs` (data volume), `dropbear` (remote diagnostics). Each is optional: the feature that
   needs a missing one says so.
4. **The files in table D**: `/etc/iyakki/` (read-only configuration) and a writable `/var/lib/iyakki/` (preferably a volume).
5. **The command line** options in table C if you want the boot entries (`iyakki.web=1`, `iyakki.ssh=1`, `iyakki.data=ram`, …).

`dist/buildroot/qemu-test.sh` and `qemu-persist-test.sh` boot an image in QEMU and check all of this; run them on the new image.

## A port to a different kernel

Table A of `BOUNDARY.md` lists every system call and ioctl IYAKKI uses and the one file that uses it. A port replaces those few
call sites (mount, reboot, input and framebuffer devices, random bytes, signals) and the ordinary programs in table B. The
desktop window code already builds on macOS-friendly crates (`iyakki-sec`, `iyakki-storage` and others check for
`aarch64-apple-darwin`; see `docs/MACOS.md` for what is still Linux-only).
