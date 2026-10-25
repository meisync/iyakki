# `iyakki sdk emulator`: target spec (NOT BUILT)

**Status: NOT BUILT.** No QEMU code exists in the `iyakki` program. This file says what `iyakki sdk emulator start|stop|status` would do, so it can be built in one pass and reviewed first.
It builds on `docs/LOCAL-TEST.md`, `docs/development/SDK-API.md` section 7.6 and the existing scripts in `dist/buildroot/` (`qemu-test.sh`, `qemu-persist-test.sh`, `qemu-web-test.sh`, `qemu-test-aarch64.sh`).

**Why.** A coding agent (or a person without a spare computer) needs a device to push to: `iyakki dev push`, `logs`, `screenshot` and `input` against a throwaway IYAKKI, with no hardware.

## Commands

| Command | What it does |
|---|---|
| `iyakki sdk emulator start [--image FILE] [--name qemu] [--memory 2048] [--no-display]` | boots the image, waits until the device answers on the forwarded port, pairs it (see below), prints the next step |
| `iyakki sdk emulator stop` | ends the QEMU process it started (by the pid file), leaves the disk file |
| `iyakki sdk emulator status` | running or not, the image, the ports, whether `iyakki dev whoami --device qemu` answers |

## What it would run

The same shape as `qemu-test.sh`, minus the self-test flag, plus a disk:

* `qemu-system-x86_64 -accel kvm -accel tcg -m 2048 -smp 2` (aarch64 hosts: `qemu-test-aarch64.sh` flags with `-M virt`; Apple silicon: `-accel hvf`, as in LOCAL-TEST.md).
* Boot from the install image or `bzImage` + `rootfs.cpio.zst` of a build (`--image`; no default download: the person names the file). A disk file `~/.local/share/iyakki/emulator/data.img` (qcow2, 4 GiB, created on first start) holds the data volume. The image file itself is opened read-only (`snapshot=on`) and never written.
* User-mode networking with forwards to the host loopback only (`127.0.0.1`): host 7979 -> guest 7979 (developer mode), host 8443 -> guest 8443 (the browser front end, optional), 2222 -> 22 only if SSH is switched on in the image.
* `virtio-gpu` + `virtio-keyboard/tablet` so the shell draws and `screenshot` and `input` work; `-display none` with `--no-display`. No audio device (or `-audiodev none`), no camera, no microphone.
* State in `~/.local/share/iyakki/emulator/` (pid file, log of the console, the disk). Nothing is written to any physical drive.

## Pairing: OPEN (owner decision 11 of SDK-API.md is still open)

Pairing normally needs a person to press Pair on the device and type a code. For an emulator started by a script that is not wanted, so one of:

1. **Kernel-line switch** `iyakki.devmode_autopair=1` on the production code path, plus a first-boot file `dev-autopair.json` on a virtual disk (PC public key, scopes `install,debug,input,config`, expiry 1 hour). Smallest change; the production image contains the code but no image sets the switch, and a test must prove the switch is ignored without it.
2. **A separate dev-only image variant** that contains the auto-pair code; production images never do. Safest for review, costs an extra image to build and ship.
3. **No auto-pair**: `start` prints the pairing code read from the guest console log (the shell prints it when Pair a computer is chosen over a serial/console channel) and runs `iyakki dev pair 127.0.0.1:7979 --code ...` itself. Needs a person (or a script) to switch developer mode on inside the guest, so it is not hands-free.
4. **Console-driven pairing**: `start` injects keystrokes through the QEMU monitor to open Settings -> Developer mode and read the code from a screenshot. Fragile; not recommended.

Recommended by the design note: 1 (matches SDK-API.md section 7.6). Until decided, nothing here is implemented.

## What the image needs

* Developer mode switched on at boot in the emulator only (via the same kernel-line switch family, or a setting on the data disk).
* The auto-pair read at first boot (small, in the shell/devmode start, if option 1 or 2).
* `dev whoami` reporting `hardware: "qemu"` so agents skip speech, audio and T2 hardware assertions.
* The existing virtio drivers (net, gpu, input, block) in the kernel config: `qemu-test.sh` already boots with virtio-net and others; gpu and input devices must be confirmed in `dist/buildroot/` kernel fragments.
* A way to run unattended after sign-in: either a pre-created account (`iyakki user add ... --if-missing` exists) with auto sign-in for the emulator, or the agent stops at the sign-in screen (the dev API answers `Unavailable: no one is signed in`).

## Tests it would need

A unit test of the QEMU command line builder (pure function, no QEMU), of the pid/status file handling, and of the refusal to write to anything outside the state folder; the ignored end-to-end test that boots an image and runs `dev whoami` and `dev screenshot` (same style as the `qemu-*.sh` scripts). None of these exist.
