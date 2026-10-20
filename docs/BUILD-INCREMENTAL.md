# Building images without rebuilding everything

Rule (owner, 2026-10-18): build only what changed; otherwise use the images that exist. This page says what triggers what, how to
reuse images, and how to share caches between variants. The scripts are in `dist/buildroot/`:

| Script | Job |
|---|---|
| `changed.sh <variant>` | Decide: `NOTHING`, `IYAKKI-ONLY`, `PACKAGES a b`, `KERNEL` or `FULL: reason`. Never builds. `--record` stores the state after a good build, `--adopt` vouches that existing images are current, `--explain` prints every input hash. |
| `repack.sh <variant>` | The fast path for `IYAKKI-ONLY`: new program and overlay into the existing rootfs, new ISO and install image, no Buildroot `make` (about 30 s with `--fast-zstd`, a few minutes with the release `zstd -19`). Also builds `macweb` from the `web` and `mac` outputs (below). |
| `smart-build.sh <variant>` | The wrapper: runs `changed.sh`, then takes the cheapest action. `--force-full`, `--clean`, `--dry-run`. |
| `test-changed.sh` | Decision-table test of `changed.sh` on fake output directories (33 checks). |

## Where the time goes (measured from the stamps of the existing builds, 16 cores, `JOBS=5`)

| Build | Sum of package build times | Of which |
|---|---|---|
| `libre` | about 20 min (the whole toolchain included) | kernel about 14 min |
| `mac` | about 40 min | kernel + apple-bce + firmware |
| `web` | about 10 h | **WPE WebKit 9 h (541 min)**, host-llvm 12 min, llvm 13 min, kernel 14 min, gcc 7 min, mesa 4 min, ffmpeg 1 min, everything else minutes |

So one package is 90% of a web or macweb build, and the toolchain (gcc/binutils/musl, about 7 min) is not worth sharing. A `macweb`
build compiles the same WebKit again: the userland of `macweb` is the `web` userland (the two defconfigs differ only by the kernel patch
and fragment list, `apple-bce` and the firmware packages). Nothing else in a rebuild matters next to that.

## What shares and what does not today

* **Downloads: already shared.** `BR2_DL_DIR` is `$(TOPDIR)/dl`, and every variant uses the same Buildroot tree
  (`target/buildroot-src/`), so `target/buildroot-src/buildroot-*/dl` (1.4 GB) serves all of them. Deleting that tree loses the cache;
  to keep it elsewhere export `BR2_DL_DIR=$HOME/.cache/iyakki-dl` before building (Buildroot honours the variable).
* **Compiler output: not shared** (no ccache yet). Each `target/buildroot-<variant>/` has its own toolchain, host tools (host-llvm,
  host-python ...) and per-package objects, 13 to 26 GB each.
* **Toolchain: not worth sharing.** libre/mac and web/macweb already have different toolchains (the web fragment adds
  `BR2_TOOLCHAIN_BUILDROOT_CXX` and `WCHAR`). An external toolchain (`BR2_TOOLCHAIN_EXTERNAL_CUSTOM` pointing at another output's
  `host/`) would save about 7 minutes per variant, but loses Buildroot's `BR_CCACHE_HASH` (it hashes the gcc sources and options so
  that ccache hits survive across output directories), needs C++ and wchar to match exactly, copies libstdc++ into images that did not
  have it, and puts the second tree at the mercy of the first one being kept. Not recommended.
* **`rebuild_if_changed` in `build.sh`** already cleans `grub2`, `linux` and `apple-bce` when GRUB options, kernel patches or the Mac
  kernel fragment change (Buildroot would not notice). `changed.sh` watches the same inputs and more.

## What triggers what

| You changed | Decision | Action | Cost |
|---|---|---|---|
| nothing (docs, README, tests, `*.md`, the Rust code of a crate that is not in the binary) | `NOTHING` | reuse `target/buildroot-<v>/images/` | 0 |
| the `iyakki` program or the store server (the cargo output is hashed, not the sources) | `IYAKKI-ONLY` | `repack.sh` | minutes |
| `board/iyakki/overlay`, `board/iyakki-web/overlay`, `post-build.sh`, `web-post-build.sh`, `prune-firmware.sh`, `mac-firmware.keep`, `grub.cfg`, `grub/` theme, `genimage-install.cfg`, `post-image.sh`, `secureboot.sh`, LICENSE/NOTICE/LICENSING/THIRD_PARTY files, `dist/samples`, the bundled Noto fonts | `IYAKKI-ONLY` | `repack.sh` re-runs the real hooks and post-image | minutes |
| a package directory of this tree (`package/apple-bce` is part of the kernel group, `package/iyakki-webview-helper`) | `PACKAGES name` | `make <name>-dirclean`, then `make` | minutes |
| a package switched on in a fragment | `PACKAGES name` | `make` | that package and its new dependencies |
| a sub-option of a package (`BR2_PACKAGE_MESA3D_...`) | `PACKAGES mesa3d` | dirclean that package, `make` | that package (WebKit options: 9 h without ccache) |
| GRUB modules or built-in config | `PACKAGES grub2` | dirclean, `make` | a few minutes |
| kernel version, `linux.fragment`, `linux-mac.fragment`, kernel patches (`patches`, `patches-sb`) | `KERNEL` | `linux-dirclean` (build.sh does it), `make` | 15 min plus modules |
| a package switched off, the architecture, libc, compiler, kernel headers, C++ flags, init system, Secure Boot on/off, `external.mk`, `Config.in` | `FULL: ...` | `smart-build.sh <v> --clean` | whole build |
| no record, no images, images deleted | `FULL: ...` | `build.sh` (Buildroot is incremental itself) or `changed.sh <v> --adopt` if the images are current | varies |

Comments, blank lines and anything outside `BR2_*` lines in fragments are ignored. The record is `target/buildroot-<v>/.iyakki-changed/`
(one hash per input group plus a copy of the composed settings). It is written only by `--record` / `--adopt`.

First use on a tree that already has images: `dist/buildroot/changed.sh web --adopt` (only if you know the images were built from
the current tree; otherwise the first decision is `FULL`).

## repack.sh in detail

1. unpacks `images/rootfs.cpio.zst` under `fakeroot` (owners and `/dev/console` survive; the listing was compared entry by entry);
2. copies the overlays named in `.config`, runs the post-build hooks named in `.config` (the same scripts Buildroot runs, with the same
   environment `build.sh` exports; module signing is skipped because the modules already carry the signature, the certificate line of
   `/etc/iyakki-release` is kept);
3. `find | sort | cpio -H newc`, `zstd -19` (as Buildroot does);
4. `post-image.sh` and `genimage.sh -c genimage-install.cfg` on a staging copy in `OUT/.repack/` (so the ISO, the install template and the
   1 MiB data marker `IYAKKI-FORMAT-ME-v1` come out with exactly the partition layout of the board files);
5. only then moves the new files into `images/`. A failure leaves the old images untouched.

It does not refresh `rootfs.iso9660` and `rootfs.tar` (Buildroot's own intermediates; a full `make` does). It never writes to a drive.
For Secure Boot run it with the same `IYAKKI_SB_KEY`/`IYAKKI_SB_CERT` as the build. `aarch64` is not supported (use the full build).
Tested on a copy of `target/buildroot-web` (2689 entries, the metadata listing identical except the one overlay file added since the build;
ISO system area and install-image partition table identical; size of the data marker identical).

## macweb from web + mac, no recompile (layered rootfs)

```
dist/buildroot/changed.sh web        # must say NOTHING or IYAKKI-ONLY (the web build is current, same WEB_* switches)
dist/buildroot/changed.sh mac        # must say NOTHING (current kernel, patches, apple-bce)
dist/buildroot/repack.sh macweb --base target/buildroot-web --kernel-from target/buildroot-mac
```

Takes `rootfs`, host tools, GRUB tree and ISO tree from the web build, then replaces `/lib/modules` and `/lib/firmware` with the mac
build's (kernel modules, `apple-bce.ko`, pruned firmware) and uses the mac `bzImage`; the hooks run for `macweb-x86_64`, so the release
file says so and the Mac firmware pruning is applied again. Output: `target/buildroot-macweb/images/` (a copy in a scratch dir took 27 s
with `--fast-zstd`, and booted to the supervisor and DHCP in QEMU). It checks that the web base carries the same Mesa Intel and H.264
choice as this run (`WEB_IRIS`, `WEB_CROCUS`, `WEB_H264`) and refuses otherwise.

Why this is correct: `macweb` = `web` + `iyakki_firmware.fragment` + `iyakki_mac.fragment`, and `mac` = `libre` + the same two. The
fragments add a kernel patch directory, a kernel fragment, `apple-bce` and linux-firmware packages. Nothing in the web userland depends
on them, and the kernel/modules/firmware of `mac` are the ones `macweb` would build (same version, same fragments). Risks:

* the kernel of the `mac` build must be current (`changed.sh mac` = `NOTHING`). The existing `mac` and `web` trees were configured before
  `patches-sb` (the Secure Boot lock-down patch) was added to the base defconfig, so rebuild the kernel of `mac` once
  (`changed.sh mac` says `KERNEL`) before using it as a donor for a Secure Boot image;
* `/lib/modules/<ver>/modules.dep` comes from the donor and the web userland must not add kernel modules (it does not);
* a Mac-only userland package added later (not a kernel or firmware one) would not be in the layered image: then `changed.sh` on the
  `macweb` tree is the authority, and a real `build-macweb` is needed;
* it is an assembled image, not a Buildroot tree: `legal-info/` of the layered image is the union of the two builds' (copy both);
* boot-test it in QEMU (`qemu-test.sh`-style) and on the Mac before a release, like any image.

## Shared compiler cache (ccache) for the next full builds

A web-class build repeats WebKit (9 h) whenever the output directory is new. With ccache the second compile of the same source with the
same flags is a cache hit, across output directories: Buildroot's compiler wrapper tells ccache a hash of the gcc sources and options
(`CCACHE_COMPILERCHECK=string:<hash>`) and sets `CCACHE_BASEDIR` to the output directory, so `web` and `macweb` produce the same keys.
Expected: the second WebKit build in a new tree drops from about 9 h to roughly 30-60 min (link, CMake configure and unity files that
miss); host-llvm and llvm (25 min together) are cached the same way. The first build that fills the cache costs about 5% more.
The cache needs an estimated 20-30 GB for web-class builds (check `target/.../host/bin/ccache -s`; Buildroot's ccache is
`host/bin/ccache`, `BR_CACHE_DIR` is the cache path).

Proposed settings, added by the `build.sh` patch below only for a NEW output directory (turning ccache on in a finished tree
would make Buildroot rebuild the compiler wrapper; `changed.sh` ignores the ccache options on purpose):

```
BR2_CCACHE=y
BR2_CCACHE_DIR="$HOME/.cache/iyakki-ccache"
BR2_CCACHE_INITIAL_SETUP="--max-size=30G"
BR2_CCACHE_USE_BASEDIR=y
```

Limits: the cache only helps compiles of identical sources/flags/toolchain config; a WebKit version bump misses everything. Per-package
directories (`BR2_PER_PACKAGE_DIRECTORIES`) do not help across output directories and are not proposed.

## Patch to apply to build.sh after the running build ends

Do not edit `build.sh` while a build runs (the shell reads it as it goes). Afterwards:

```diff
--- a/dist/buildroot/build.sh
+++ b/dist/buildroot/build.sh
@@
 if [ "${WITH_FFMPEG:-0}" = 1 ]; then cat "$HERE/configs/iyakki_media.fragment" >> "$OUT/defconfig"; fi
+# Shared compiler cache (docs/BUILD-INCREMENTAL.md): only for an output directory that has no build yet, or when asked for
+# (IYAKKI_CCACHE=1); IYAKKI_CCACHE=0 switches it off. A finished tree is not changed behind its back.
+if [ "${IYAKKI_CCACHE:-auto}" = 1 ] || { [ "${IYAKKI_CCACHE:-auto}" = auto ] && [ ! -d "$OUT/build" ]; }; then
+  cat >> "$OUT/defconfig" <<EOF_CC
+BR2_CCACHE=y
+BR2_CCACHE_DIR="${IYAKKI_CCACHE_DIR:-$HOME/.cache/iyakki-ccache}"
+BR2_CCACHE_INITIAL_SETUP="--max-size=${IYAKKI_CCACHE_SIZE:-30G}"
+BR2_CCACHE_USE_BASEDIR=y
+EOF_CC
+elif [ -f "$OUT/.config" ] && grep -q '^BR2_CCACHE=y' "$OUT/.config"; then
+  # keep it on for a tree that already has it (the defconfig is composed again every run)
+  grep '^BR2_CCACHE' "$OUT/.config" | grep -v '^# ' >> "$OUT/defconfig"
+fi
```

`BR2_DL_DIR` needs no patch (export it if you want it outside the Buildroot tree). To also make `build.sh` itself skip work when
asked: call it through `smart-build.sh`, or add at its very top
`[ "${IYAKKI_SKIP_UNCHANGED:-0}" = 1 ] && [ "$("$(dirname "$0")/changed.sh" "$VARIANT" | head -n1)" = NOTHING ] && exit 0`
(after `VARIANT=` is set). Do not call `changed.sh --record` from inside `build.sh`; `smart-build.sh` records after a good build.

## Patch to run.sh

```diff
-  build) exec dist/buildroot/build.sh libre ;;
-  build-mac) exec dist/buildroot/build.sh mac ;;
+  build) shift; exec dist/buildroot/smart-build.sh libre "$@" ;;
+  build-mac) shift; exec dist/buildroot/smart-build.sh mac "$@" ;;
   build-macweb)
     shift
-    for a in "$@"; do case "$a" in --h264) export WEB_H264=1 ;; --iris) export WEB_IRIS=1 ;; --crocus) export WEB_CROCUS=1 ;; *) echo "build-macweb: unknown option $a" >&2; exit 2 ;; esac; done
-    exec dist/buildroot/build.sh macweb ;;
+    SB=()
+    for a in "$@"; do case "$a" in --h264) export WEB_H264=1 ;; --iris) export WEB_IRIS=1 ;; --crocus) export WEB_CROCUS=1 ;; --force-full|--clean|--dry-run) SB+=("$a") ;; *) echo "build-macweb: unknown option $a" >&2; exit 2 ;; esac; done
+    exec dist/buildroot/smart-build.sh macweb "${SB[@]}" ;;
   build-web)
     shift
-    [ "${1:-}" = "--h264" ] && export WEB_H264=1
-    exec dist/buildroot/build.sh web ;;
+    SB=(); for a in "$@"; do case "$a" in --h264) export WEB_H264=1 ;; *) SB+=("$a") ;; esac; done
+    exec dist/buildroot/smart-build.sh web "${SB[@]}" ;;
+  build-macweb-layered) shift; exec dist/buildroot/repack.sh macweb --base target/buildroot-web --kernel-from target/buildroot-mac "$@" ;;
+  build-changed) shift; exec dist/buildroot/changed.sh "$@" ;;
```

(`${SB[@]}` on an empty array needs bash 4.4+ with `set -u`; run.sh's shebang and options decide, adjust with `${SB[@]+"${SB[@]}"}` if needed.)
`build-arm` stays on `build.sh` (the ARM image is not repackable).

## Rules of thumb

* Before any build: `dist/buildroot/changed.sh <variant>`. If it says `NOTHING`, use the images. If it says `IYAKKI-ONLY`, repack.
* Never `rm -rf target/buildroot-*` or `make clean` to "make sure". Use `<pkg>-dirclean` for one package.
* Do not run two builds of the same variant, and do not edit files under `dist/buildroot/` while a build runs (build.sh and the hooks
  are read as they execute).
* Disk: each output directory is 13 to 26 GB (`web` 26 GB, `macweb` 14 GB and growing, `mac` 17 GB, `libre` 13 GB, ARM 14 GB), the ccache
  about 20-30 GB, the downloads 1.4 GB. A layered `macweb` needs only the images (about 1.5 GB) once `web` and `mac` exist. After a release,
  the `build/` folders of variants you will not touch for a while can be dropped, keeping `images/` and `legal-info/`; a later change then
  makes the next build `FULL`, which is why ccache is worth its 30 GB.
* `legal-info/` is made by `make legal-info`; an `IYAKKI-ONLY` repack does not change the package set, so the old one stays valid.
