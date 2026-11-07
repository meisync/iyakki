# Apple Silicon Mac: VM and native window

Plan file: `apple-silicon-vm.md` | Checks: 3 | Estimated time: about 45 minutes (0.8 hours)

Use `RESULTS-TEMPLATE.md` to report. Lines marked with (A), (code) or 'add ...' in the row text are developer tasks: skip that part and test the rest.

## What you need

- An M-series Mac (macOS), UTM installed.
- The aarch64 image.
- Ollama installed for the Metal row.

## Safe start

Everything runs in a VM or a normal window; nothing outside the VM is changed.

## How to collect logs

Use developer mode on the test machine (Settings -> Developer mode; approve the pairing on the machine). From the paired computer:

- `iyakki dev diag` : one bundle with state and recent logs. Attach it to every failed row. It holds no secrets (a row checks this; if you see a key in it, tell the owner and do not share it).
- `iyakki dev logs --system` : the system log. Attach the lines around the failure.
- `iyakki dev dmesg` : the kernel log, for boot, Wi-Fi, sound and USB problems.
- The data-folder logs: files under `/var/lib/iyakki/log` (for example `engine.log` for the browser and `kmsg-<boot-id>.log` after a crash). On a machine with no network, copy them to a USB stick from the Files tile.
- A photo of the screen helps for any display, freeze or layout problem. Say how long after the action it happened.

## Order of checks

1. VM boot first, then the native window, then the Metal model.

The checks below are already in this order. Do not skip a failed early row: write it down and go on.

## Checks

Fill the Result line: pass, fail or note. For a fail, attach the files named in the row.

- [ ] F-BOOT-08 aarch64 image for VMs (Apple Silicon Macs via UTM/QEMU, ARM64 boards): UEFI disk image with GRUB, kernel and initramfs: do this: Run the aarch64 image in UTM on an Apple Silicon Mac -> expect this: it works as the row says; the open point is: Notes: boots in QEMU; never run on a Mac; if it fails, copy: `iyakki dev diag` output, `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log.
  Result: ______ (pass / fail / note)

- [ ] F-APPLE-01 Apple Silicon models through the macOS bridge (route 2 of docs/APP-MODE-AND-APPLE-SILICON.md): any M-series Mac runs the model with Metal, IYAKKI and k3s call it over the network; M3 and later work this way because Asahi does not support them: do this: On an M-series Mac run the model with Metal and call it from IYAKKI over the network; VM route is a later build -> expect this: it works as the row says; the open point is: 'Same limits as F-MODELHOST-02'; route 3 (Linux VM on macOS) documented, not built; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-SHELL-20 `./run.sh window` on a Mac with Apple silicon (M-series): the desktop window runs natively on macOS, finds and uses an Ollama running on the same Mac (Metal, unified memory) or on the network, and a guide (`docs/MACOS.md`) for the window, for Ollama and for running IYAKKI in a VM that talks to the host's Ollama: do this: Build and run ./run.sh window on an Apple silicon Mac with Ollama -> expect this: it works as the row says; the open point is: Cross-checked only with cargo check for macOS; no Apple C compiler or Apple silicon run; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

