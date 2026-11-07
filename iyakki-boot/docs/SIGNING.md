# Signing the loader for Secure Boot

Status: the signing and verification scripts work and are tested with a throwaway key (`tools/sign-test.sh`). Since 2026-11-04 the signed loader is also
started in QEMU by shim under the Secure Boot firmware with the test key enrolled as a MOK, and starts a MOK-signed kernel (`tools/qemu-secureboot-test.sh`,
sections 4 to 6). Not tried on a real machine.

## 1. Sign and verify

```
tools/sign.sh boot-uefi.efi boot-uefi.signed.efi --key KEY.pem --cert CERT.pem
```

* The key and the certificate are always given as paths. The script never searches for a key, never prints or copies one, and
  refuses to overwrite its input. It needs `sbsign` and `sbverify` (package `sbsigntool`).
* It writes the output next to its destination, verifies it with `sbverify --cert CERT.pem`, and moves it into place only if the
  check passes. `--no-verify` skips the check.
* Test: `tools/sign-test.sh boot-uefi.efi` makes two throwaway RSA keys in a temp directory, signs, verifies, and checks that a
  different certificate, the unsigned file and a file with one changed byte are all refused. Nothing real is read.

## 2. Shim and MOK (the usual path on PCs and Macs)

Most machines trust only the Microsoft keys. A distribution loader therefore starts through `shim` (signed by Microsoft), which in
turn trusts the keys in the Machine Owner Key list (MOK). The loader is signed with your own key and that key is enrolled in MOK.

1. Make the certificate in DER form for enrolment: `openssl x509 -in CERT.pem -outform DER -out CERT.der`.
2. Sign the loader as above. The same key must be used for later releases, or MOK must be enrolled again.
3. Enrol the certificate: `mokutil --import CERT.der` on the running system (it asks for a one-time password), reboot, and in the blue
   MokManager screen choose "Enroll MOK", then enter the password. (On a machine without a running system, put `CERT.der` on the
   boot volume and use MokManager's "Enroll key from disk".)
4. Put the files on the EFI system partition (a copy step, nothing is built there):
   * `\EFI\BOOT\BOOTX64.EFI` = `shimx64.efi` (the signed shim of your distribution), or a firmware boot entry pointing at it;
   * the signed loader next to it under the name shim loads as its second stage, normally `grubx64.efi` (the name is only a file
     name; no GRUB is involved), or the name given to shim in its load options;
   * `MokManager` (`mmx64.efi`) and `fbx64.efi` beside shim, as the shim package ships them;
   * the loader's config file at the root of the same volume (see `CONFIG_PATH` in `crates/boot-uefi/src/main.rs`).
5. Shim verifies the loader through the firmware's `LoadImage` hook (it checks the Microsoft db, the vendor certificate built into
   shim, and MOK). The loader itself starts everything else with `LoadImage` / `StartImage`, so the same check runs for the kernel.

## 3. The kernel EFI stub must be signed too

The loader never reads a kernel into memory by itself and never jumps to it. It calls `LoadImage`, and the firmware (with shim's
hook) refuses an image that is not signed by a trusted key. So:

* the kernel image (`bzImage` with the EFI stub, or a unified kernel image) must be signed with a key that is in MOK, the
  firmware `db`, or shim's built-in vendor certificate, the same way as the loader (`sbsign` on the kernel file, then
  `sbverify`). An unsigned or re-signed-with-another-key kernel gives "Access Denied / Security Violation" and the loader shows the
  error and waits in the menu instead of booting;
* chain-loaded systems (Windows' `bootmgfw.efi` and others) are checked the same way, and the scan greys out loaders that carry no
  signature at all when Secure Boot is on;
* with Secure Boot on, the loader does not allow editing the kernel command line (`boot-core`'s `cmdline::Rules`);
* the initrd is not signed or checked by the firmware: it is read from the boot volume and handed to the kernel stub through the
  `LINUX_EFI_INITRD_MEDIA_GUID` protocol. Anyone who can write the boot volume can change the initrd and the command line in the
  config file. A strict setup should use a signed unified kernel image (kernel, initrd and command line in one signed file) as a
  chain entry; the loader can start it as any other `.efi` file.

## 4. SBAT

shim 15.3 and newer refuse a second stage without an `.sbat` section (the QEMU test shows it with the Ubuntu shim). `tools/add-sbat.sh IN OUT SBAT.csv`
adds one (GNU objcopy; the section is placed after the last one). The CSV is the distribution's: the `sbat,1,SBAT Version,...` line plus one line with
the distribution's OWN component name (`iyakki.loader,1,...` for IYAKKI, `dist/buildroot/board/iyakki/loader-sbat.csv`). Never claim `grub`, `shim` or
`systemd`: the revocation list names those. Add the section BEFORE signing.

## 5. The shim chain, tested

`tools/qemu-secureboot-test.sh --kernel BZIMAGE` (QEMU, OVMF Secure Boot firmware `OVMF_CODE_4M.secboot.fd` with `OVMF_VARS_4M.ms.fd`, a
Microsoft-signed shim from `/usr/lib/shim`, `virt-fw-vars` to put a THROWAWAY certificate in the MOK list as MokManager would; no real key is read).
Verified: shim -> loader as `grubx64.efi` (signed, with SBAT) -> MOK-signed kernel boots and the kernel reports "Secure boot enabled"; an unsigned
loader, a loader signed by a key that is not enrolled, a kernel signed by another key and an unsigned kernel are all refused; with nothing enrolled shim
stops; a signed loader without `.sbat` is refused by that shim.

One finding: shim's security override is not in place for an image the SECOND STAGE loads, so a plain `LoadImage` of a MOK-signed kernel fails with
"access denied" (seen first in this test). The loader therefore wraps the firmware's `EFI_SECURITY2_ARCH_PROTOCOL.FileAuthentication` while it starts an
image (`crates/boot-uefi/src/shim.rs`): the original check runs first and stands when it says yes; when it says no and `SHIM_LOCK_PROTOCOL` exists,
`Verify(buffer, size)` decides. The path-based `EFI_SECURITY_ARCH_PROTOCOL` is not touched (it has no bytes to show shim). Without shim, or with Secure
Boot off, nothing is installed. The hook is removed when the started image returns.

## 6. What is not tested

* Real firmware (the Mac mini's T2 startup security levels, PCs), the shim of those machines, MokManager's key presses with the loader as second
  stage (the loader changes nothing about MokManager; the repository's GRUB test drives it).
* The loader signed with the owner's real key (the build step `secureboot.sh loader-image` is tested with a throwaway key only).
* A shim older than 15.3 (it would accept the loader without `.sbat`; not needed).
