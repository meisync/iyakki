# iyakki-boot

A small boot loader in Rust for UEFI (and later legacy BIOS) PCs: it starts a Linux kernel and initrd, rolls back after failed starts,
shows a keyboard and touch menu, and finds and chain-loads other operating systems on the machine.

Status: steps 1 to 3 of the plan are done. `crates/boot-core` (no_std + alloc): config parser, entry choice, boot assessment, command
line rules, discovery model and menu model, unit-tested on the host. `crates/uefi-sys`: in-house UEFI type definitions (types only).
`crates/boot-uefi`: the UEFI program (about 95 KB): reads the config from its own boot volume, keeps the boot-tries counter and the
one-shot choice in UEFI variables, reads held keys, draws the menu (text mode, or the graphics output with big rows and a built-in
font; mouse through the simple pointer), starts a Linux kernel through the EFI stub with the initrd offered through the initrd
protocol, chain-loads other `.efi` loaders, and scans all FAT volumes for other systems (read-only). `crates/test-efi` is a tiny
stand-in kernel / loader for the tests. `tools/` has the QEMU tests and the signing scripts. See [docs/PLAN.md](docs/PLAN.md) and
[docs/SIGNING.md](docs/SIGNING.md). Also done 2026-11-04: a GRUB-like menu under the logo with a second level ("More options"), `more` / `requires` / `kind firmware` in the config, an SBAT section tool, the shim chain with MOK (the loader asks shim to verify the kernel it starts), and a confirm variable the running system writes. Not done: the BIOS front end, TPM, real-machine tests.

## Menu under the logo, second level

With a splash configured, a normal start shows only the logo. On the first key (or touch) the logo moves up and the menu is drawn below it on the
same background: the entries without `more` plus a `More options` row; Enter on that row opens the entries marked `more` (with a `Back` row first;
Esc also goes back). Found systems (`scan yes`) are always on the second level. Placement is `boot_core::layout::under_logo` (text and rows grow with
the screen height, rows are at least 44 px at 1080p, at least three rows stay visible from 640 x 480 to 4K; the picture is cut to its content and
only shrinks). Without graphics output or without a splash the menu is the flat list as before (text or big rows), now also with the second level.

New entry keys (strict, same quoting rules: no quotes, no spaces in paths):

```
more                         # flag: list this entry on the second level
requires /EFI/x/y.efi        # leave the entry out unless this file exists on the boot volume
kind firmware                # restart into the firmware's setup screen (no kernel line; left out when the firmware cannot do it)
```

A running system confirms a good start by writing the variable `BootConfirm` (any content) under the loader's vendor GUID; the loader reads it
at the next start, refills the tries and deletes it (see `docs/development/GRUB-REPLACEMENT-PLAN.md` in the IYAKKI repository for the design).

## Secure Boot with shim and MOK

`tools/add-sbat.sh IN OUT SBAT.csv` adds the `.sbat` section shim 15.3+ requires (before signing), `tools/sign.sh` signs. Started as shim's second stage the
loader wraps the firmware's `Security2Arch.FileAuthentication` while it starts something, so a kernel signed with a MOK key loads (shim's lock
protocol verifies it; the firmware's own "yes" always stands, nothing else is allowed). `tools/qemu-secureboot-test.sh --kernel BZIMAGE` runs the
chain in QEMU with the Secure Boot firmware and throwaway keys. See [docs/SIGNING.md](docs/SIGNING.md).

## Splash screen

The loader ships no logo; the distribution supplies one. Three optional keys at the top of `boot.cfg`:

```
splash /boot/splash.bmp     # path on the boot volume
splash-bg #101820           # background colour (default #000000); the menu uses it too while a splash is configured
splash-ms 1500              # least time the splash stays up before a normal start, 0 to 10000 ms (default 0)
```

File format: an uncompressed BMP, 24 or 32 bits per pixel (`BI_RGB`, or 32-bit `BI_BITFIELDS` with the plain B8G8R8 masks), bottom-up or
top-down, header of 40, 52, 56, 64, 108 or 124 bytes; alpha is ignored. No palettes, RLE or OS/2 headers. Limits: file 16 MiB, each side
4096 pixels, 2048 x 2048 pixels in all; anything else is refused and the loader starts as if no splash was configured (a one-line reason
goes to the serial log). Decoder: `crates/boot-core/src/splash.rs` (bounds-checked, tested against truncated, mutated and random input).

Behaviour (UEFI, needs the graphics output; no mode change): right after the config is read the background colour is filled and the
picture drawn centred in one off-screen copy, scaled down by nearest neighbour if it is bigger than the screen (never enlarged). A normal start
(timeout 0, or the countdown with nobody pressing a key) shows only the splash and then starts the kernel; the screen is not cleared
before the kernel starts. The menu appears, over the same background, when it is requested (Esc or Space at start), for a one-shot or
hold key (no menu at all), for `StopAndWait` (shown at once), or when a key or touch arrives during a countdown (that first key only
shows the menu). While the splash is up, log lines go to the serial port only, because the firmware text console would print over the
picture. No graphics output, or `BootText` set: no splash, exactly as before.

Hand-over to Linux: the EFI stub leaves the firmware framebuffer (GOP) in place and describes it in `screen_info`; with `efifb`
or `simpledrm` the kernel keeps writing to that same memory, so the picture stays until the kernel console or a display server
draws over it (`quiet`, `vt.global_cursor_default=0` and no early text help). Not guaranteed: a driver that does its own mode set
(i915, amdgpu, nouveau) can blank or flash the screen at that point, and a firmware that resets the display on `ExitBootServices` loses
the picture; `splash-ms` is a minimum counted from the stalls the loader knows about, not a precise timer. Tested in QEMU/OVMF only.

Build the UEFI programs and run the tests:

```
cargo build --release --target x86_64-unknown-uefi -p boot-uefi -p test-efi   # needs the x86_64-unknown-uefi target
tools/qemu-test.sh [--lean-image DISK.img]      # QEMU + OVMF, throwaway disks; see the header of the script for settings
tools/qemu-secureboot-test.sh --kernel BZIMAGE  # shim + MOK + signed loader and kernel under the Secure Boot firmware
tools/sign-test.sh target/x86_64-unknown-uefi/release/boot-uefi.efi           # signing with a throwaway key
```

The config file name is a constant at the top of `crates/boot-uefi/src/main.rs` (`/boot.cfg`); set the build variable
`BOOT_CONFIG_PATH` to use another name. Variables use the project's own vendor GUID (`crates/boot-uefi/src/vars.rs`).

Run the host tests (own workspace, no dependencies):

```
cargo test --manifest-path iyakki-boot/Cargo.toml
```

This folder is a standalone project inside the IYAKKI repository so it can be split out and published on its own. 
Licence: `MIT OR Apache-2.0`, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
