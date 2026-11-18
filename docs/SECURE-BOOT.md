# Secure Boot (shim + Machine Owner Key)

Feature F-BOOT-02. **Alpha.** Built and tested in QEMU with Secure Boot firmware; **not yet tried on a real PC or Mac** (see the end).
Images built without a key are unsigned, exactly as before, and need Secure Boot **off**.

## The design in one picture

```
firmware (Microsoft keys in db)
  -> EFI/BOOT/BOOTX64.EFI   shim 16.1, signed by Microsoft, taken from Debian (not built by IYAKKI)
       checks the next step against the firmware db, then the MOK list (the key YOU enrolled)
  -> EFI/BOOT/grubx64.efi   GRUB 2.12, one file with every module built in, signed with YOUR key
       (loads only what shim_lock approves: the kernel)
  -> boot/bzImage           the Linux kernel's EFI stub, signed with YOUR key
       with Secure Boot on the kernel locks itself down (integrity) and takes modules signed by a key in the MOK list
  -> modules (*.ko)         every module in the initial RAM disk, apple-bce included, signed with YOUR key
```

Also on the boot partition: `EFI/BOOT/mmx64.efi` (MokManager, the blue key screen, signed by Debian, carried inside shim's trust)
and `EFI/IYAKKI/iyakki-mok.cer` (your certificate, DER) with a README and the shim provenance. No `fbx64.efi` and no
`BOOTX64.CSV`: those make shim write boot entries into the firmware's NVRAM, which IYAKKI never does.

One key does everything (GRUB, kernel, modules). IYAKKI's update/package signing key is a different thing and stays separate.

## Why these choices (sources)

* **shim from a distribution, not our own.** Firmware trusts Microsoft's "UEFI CA", so only a Microsoft-signed shim starts on
  ordinary PCs. Getting our own shim signed is the shim-review process (F-BOOT-11, later):
  <https://github.com/rhboot/shim-review>. Upstream shim is BSD-2-Clause (<https://github.com/rhboot/shim>); the signed binary
  carries the distribution's build and vendor certificate. We download Debian 13's `shim-signed` 1.51 (shim 16.1) and
  `shim-helpers-amd64-signed` (MokManager) at build time and pin the SHA-256 of both packages (they match Debian's `Packages`
  index) and of the unpacked files; nothing is stored in the repository or redistributed by us outside the image we build, and
  the copyright files come along. The check refuses a shim without a Microsoft signature.
* **MOK, not firmware db.** The standard route for owner-signed Linux: <https://wiki.debian.org/SecureBoot>,
  <https://github.com/rhboot/shim/blob/main/README.md>. The key is enrolled once with MokManager "Enroll key from disk".
* **SBAT.** shim refuses components whose SBAT generation is revoked (<https://github.com/rhboot/shim/blob/main/SBAT.md>). Debian's
  shim 16.1 revokes `grub` below generation 5. Buildroot's GRUB 2.12 does not have the fixes of that generation, so our GRUB
  declares only its own component `grub.iyakki,1` and never claims to be `grub,5`. What protects the machine is that only your
  key signs GRUB. **Before a beta: move GRUB to a release with the 2024/2025 security fixes.**
* **Monolithic GRUB.** With Secure Boot on, GRUB refuses to load modules from disk and disables some commands, so
  `grub-mkimage` builds every module the menu needs into the file (the same list as the unsigned build, `BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI`).
  The theme, `gfxterm`, `loadfont`, `configfile`, `search`, `linux` and `initrd` all work (menu tested with the real `grub.cfg`).
  In GRUB 2.12 the `shim_lock` verifier is part of GRUB's core, no module is needed.
* **Kernel.** Config in `board/iyakki/linux.fragment` (all x86-64 images; about 100 KB): `MODULE_SIG`, lockdown LSM, platform and
  machine keyrings, `LOAD_UEFI_KEYS`. `MODULE_SIG_FORCE` stays **off** so driver packs still load with Secure Boot off. The stock
  6.12 kernel does not lock itself down when Secure Boot is on (distributions carry a patch), so one small GPL-2.0 patch does it
  (`board/iyakki/patches-sb/`, BOUNDARY.md rule 3). With Secure Boot off nothing changes. Kernel lockdown: <https://man7.org/linux/man-pages/man7/kernel_lockdown.7.html>.
* **Certificate.** Code-signing EKU only. The "module signing" OID (1.3.6.1.4.1.2312.16.1.2) makes shim refuse the key for GRUB and
  kernel (found by testing).
* **sbsigntool** (GPL-3.0, build host only, not shipped) signs the PE files; the kernel's `scripts/sign-file` signs modules.
  An in-house Rust PE signer was considered and left for later: the tool is on every distribution and the format (PKCS#7 inside
  the PE certificate table) is not small to get right.
* **Apple T2 Macs.** Startup Security Utility: Full Security checks the OS against Apple; Medium Security accepts an OS "properly
  signed by Apple or Microsoft"; No Security enforces nothing; external boot is off by default
  (<https://support.apple.com/en-us/102522>). A shim signed by Microsoft is therefore expected to start with Medium Security and
  "Allow booting from external media"; with Full Security it may not. Intel Macs without a T2 chip have no Secure Boot. t2linux
  advises No Security. **Untested; see below.**

## Owner steps

1. **Make the key once** (on a machine you trust; keep the files out of git and off shared disks):
   `dist/buildroot/secureboot.sh genkey ~/iyakki-sb "My IYAKKI key"` (RSA 2048, ten years). Back it up: a lost key means a new
   enrolment on every computer.
2. **Build**: `IYAKKI_SB_KEY=~/iyakki-sb/iyakki-sb.key IYAKKI_SB_CERT=~/iyakki-sb/iyakki-sb.crt dist/buildroot/build.sh mac`.
   Needs on the build host: `sbsign`/`sbverify` (package sbsigntool), `openssl`, `curl`, `dpkg-deb`, `mkfs.vfat` and mtools
   (Buildroot supplies the last two). `secureboot.sh check` lists what is missing. Without the two variables the build says
   "building the UNSIGNED image" and continues. The key reaches: the modules (`post-build.sh`), the kernel, GRUB and the boot
   partitions of the ISO and the install template (`post-image.sh`).
3. **Write the stick/disk** with `iyakki-usb` as usual. The installer tells you the image is signed and prints the steps; the
   partition holds shim, GRUB, MokManager and the certificate.
4. **Enrol once per computer** (Secure Boot on):
   1. Start from the stick. A grey "Verification failed: (0x1A) Security Violation" box appears. Press Enter.
   2. When "Press any key to perform MOK management" shows, press a key within 10 seconds.
   3. "Enroll key from disk" -> the disk -> `EFI` -> `IYAKKI` -> `iyakki-mok.cer`. "View key 0" shows the fingerprint; compare it
      with the one in `EFI/IYAKKI/README-SECURE-BOOT.txt` (and `SECURE_BOOT_CERT_SHA1` in `/etc/iyakki-release`).
   4. "Continue" -> "Yes" -> "Reboot". **No password is asked** for a key chosen from disk (a password belongs to `mokutil --import`,
      which IYAKKI does not use). IYAKKI starts.
5. **Verify**: SYSTEM -> About shows "Secure Boot: ON, kernel lockdown: integrity"; `iyakki hwreport` prints the same
   ("Secure Boot: ON", "Kernel lockdown: integrity"); the hardware check at boot shows it too.
   On the build host: `secureboot.sh verify <esp dir> <kernel>`.

The enrolment lives in the firmware's variable store; it survives rewriting the stick. A different key (a rebuilt image from another
owner) shows the violation screen again until that key is enrolled. To remove a key: MokManager "Delete key"
(Perform MOK management) or the firmware's reset to defaults.

## What Secure Boot ON takes away (kernel lockdown, integrity mode)

Unsigned or foreign-signed kernel modules, `/dev/mem` and `/dev/port` writes, MSR writes, raw PCI access, `kexec` of unsigned
kernels, hibernation, ACPI table overrides, `iopl`. A search of IYAKKI's code found none of these in use (it has no `/dev/mem`,
MSR, `kexec`, debugfs, `ioperm` or hibernation code; the swap file is "no hibernation" already). Impact:

* **Driver packs** (docs/MAC-HARDWARE.md section 9, `iyakki-drivers`): a prebuilt module (the Broadcom `wl.ko`) is refused by the
  kernel unless it is signed with an enrolled key. Sign it with your key (`secureboot.sh` uses the kernel's `sign-file`;
  `dist/drivers/build-wl-pack.sh` should call `sign-file` when the key variables are set - not done yet) or run with Secure
  Boot off. The pack installer should read `hwinfo::secure_boot_summary` and refuse early with that sentence - not done yet.
* **apple-bce** (Mac image): built by Buildroot, signed by `post-build.sh` with the rest.
* Third-party tools the person downloads (ffmpeg, Claude Code) are user-space programs: unaffected.

## The initial RAM disk is not signed (known limit)

The initial RAM disk (`boot/initrd`, the whole IYAKKI system) is read by GRUB and not verified. Someone who can write the boot
partition can replace it with their own userland. Lockdown still stops that userland from loading unsigned kernel code, but this
is weaker than a signed unified kernel image; GRUB's `grub.cfg` is not verified either. Planned: a signed unified image
(kernel + initrd + command line in one signed file) for beta, which also removes the need for the monolithic GRUB. Protect the
boot partition physically on machines that matter.

## Tests

`dist/buildroot/qemu-secureboot-test.py` (docs/BOOT-TEST.md), kernel kit by `secureboot-testkit.sh`, plus the repo test
`crates/iyakki-core/tests/secure_boot.rs` (script syntax, pins, throwaway key generation, kernel options, no private key in the
tree, docs).

### Verified in QEMU (OVMF Secure Boot firmware, Microsoft keys enrolled, 2026-10-18)

| Scenario | Result |
|---|---|
| Secure Boot off, plain files | boots, lockdown `none`, unsigned module loads |
| Secure Boot on, key in MokList | Debian shim 16.1 accepted by the Microsoft db; our GRUB 2.12 with the real `grub.cfg` menu; signed kernel starts: "Secure boot enabled", "Kernel is locked down from UEFI Secure Boot mode", `.machine` keyring loads MokListRT; module signed with the key loads; unsigned module and module signed by another key refused (EPERM); `/dev/mem` refused |
| Secure Boot on, other key enrolled / nothing enrolled | shim stops with "Verification failed: (0x1A) Security Violation", then offers MokManager |
| Secure Boot on, old unsigned files | firmware: "Access Denied -- rejected probably by Secure Boot" |
| Enrolment done by key presses in the real MokManager (Enroll key from disk -> EFI -> IYAKKI -> iyakki-mok.cer -> Continue -> Yes -> Reboot), then boot | works, no password asked; chain boots locked down |

The QEMU kernel is a small 6.12.112 with the same Secure Boot options and the same patch, not the full image kernel (a full
image build was not run). The packaging in `post-image.sh` was exercised through `secureboot.sh image` on copies (signed kernel, ISO
EFI partition rebuilt as a 6 MB FAT with shim, MokManager, GRUB and the certificate).

### Only on real hardware

* A PC with Secure Boot on (firmware with the 2011 and 2023 Microsoft CAs: shim 16.1 carries both signatures; very old firmware
  without the 2011 CA is not an issue, very new without it is expected to accept the 2023 signature).
* A T2 Mac with each Startup Security Utility setting, external boot allowed.
* The full image kernel under lockdown with real drivers (i915, amdgpu, Wi-Fi modules, apple-bce) loading signed.
* Firmwares whose MokManager browses FAT differently; Windows dual boot is not touched.
* The installed-on-disk layout under Secure Boot (the same files, copied by the installer).
