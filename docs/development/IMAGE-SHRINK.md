# Shrinking the images (F-PKG-11)

Status 2026-11-16: the logic that needs no image build is written and tested (`iyakki-drivers::modsplit`); no image was built, the
kernel configuration and the build scripts are not changed yet. About 98 MB of kernel modules sit uncompressed in the initramfs of the
x86 image; the ISO is 61 MB.

## What is built

* **The split** (`modsplit::plan`): reads the `modules.dep` of the kernel and decides which modules stay in the image and which become
  an optional pack. A module may only leave when it is on the rare list (`RARE`: staging, infiniband, DVB tuners, ISDN, MTD, CAN,
  ham radio, WAN, PCMCIA, one-wire, industrial I/O, ASoC codecs and similar) AND it is not on the essential list (`ESSENTIAL`: storage,
  file systems, USB, HID, input, graphics, Ethernet, Wi-Fi, Bluetooth, the network core, crypto, library, PCI/USB/HDA sound). A module a
  kept module needs (directly or through a chain) is pulled back, so the split is always closed: a test proves it even for a hostile
  list that marks everything rare.
* **The manifest** (`Plan::pack_manifest`): one `module <size> <sha256> <path>` line per optional module for the kernel release, so the
  optional pack can be fetched and checked file by file at the person's click, like the firmware pack (F-REL-08). Nothing is shipped
  in the image for it.
* **The compression** (`modsplit::gzip_module`): a valid gzip file of one module (CRC and length written, ELF check, a second run
  refuses an already compressed file). Checked against the real `gzip` (not shipped) on empty, tiny and 300 kB inputs: it accepts the
  files and gives back the same bytes.

## What is not done (the remainder of the row)

1. Kernel: `CONFIG_MODULE_COMPRESS_GZIP` or `ZSTD` with `CONFIG_MODULE_DECOMPRESS=y` in `board/iyakki/linux.fragment`, so the kernel
   can load `.ko.gz` itself (`finit_module` with `MODULE_INIT_COMPRESSED_FILE`). Needs a kernel rebuild and a boot test.
2. Order of build steps: sign first (`secureboot.sh` refuses compressed modules), then compress, then `depmod` (it must see the
   `.ko.gz` names). The post-build step that calls the two functions is not written.
3. The optional pack: building it from the manifest, listing it in STORE -> Setup, and the loader rule that a missing optional module
   is asked for when hardware that needs it appears (`modprobe` by alias finds nothing today and says nothing).
4. Real numbers: the saving in MB. A kernel tree of the real image is needed; none was measured here.
5. Zstd would compress better than gzip; an in-house encoder is not written (gzip uses the `miniz_oxide` deflate already in the tree).

## Update 2026-11-18 (alpha.7 lane impl-boot2)

Written, not built into an image: `modsplit::compress_tree` and the host tool `iyakki-modpack ROOT RELEASE [--pack-out DIR]` (moves the
optional set and its manifest out, gzips every other `.ko` in place, a rerun does nothing; tested on a made-up module tree), the
`IYAKKI_MODULE_COMPRESS=1` step in `board/iyakki/post-build.sh` (after signing, before `depmod`, off by default) and the kernel
options in `board/iyakki/linux-modcompress.fragment` (not part of the default kernel: it needs a kernel rebuild and a boot test).
Still open: the STORE listing of the optional pack, the loader asking for a missing optional module, the measured saving.
