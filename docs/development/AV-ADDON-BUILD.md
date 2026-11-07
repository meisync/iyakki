# The audio and video add-on: how it is built, packaged, installed and checked

Written 2026-11-04 (owner decision the same day: "build the add-on path"). Design and policy: [`../FULL-BROWSER.md`](../FULL-BROWSER.md)
sections *Codec policy* and *Audio and video add-on*. This page is the how, with what was measured.

**What it is.** The web image carries only GStreamer's LGPL bridge plugin `libgstlibav.so`, parked inactive in `/usr/lib/iyakki-av/gstreamer-1.0`.
The H.264, AAC and MP3 decoders are FFmpeg's shared libraries, which are **never in any image**: the person presses *Install* in the browser's bar,
Store -> Setup shows the item "Browser audio and video (H.264, AAC)" (id `ffmpeg-libs`), and the libraries go to `<data>/addons/ffmpeg/lib`.
`crates/iyakki-agent/src/avaddon.rs` then gives the engine the plugin folder, the library folder and a registry of its own at its next start.
The decision "alpha images leave H.264 out" is unchanged.

## 1. Feasibility (answered by doing it)

| Question | Answer (measured 2026-11-04 on this machine, 16 cores, 28 GB, `-j4`) |
|---|---|
| Steps | (1) FFmpeg 6.1.5 from Buildroot's own package, configured LGPL-only with `configs/iyakki_web_av_addon_libs.fragment` (build.sh adds it when `WEB_AV_ADDON=1` and not `WITH_FFMPEG=1`); (2) `make ffmpeg-dirclean ffmpeg gst1-libav-reinstall` in the **existing** `target/buildroot-web` (nothing else is rebuilt); (3) `dist/buildroot/package-av-addon.sh` strips the six libraries and makes the archive. |
| Time | **About 30 s of compiling** (configure 7 s, build 20 s with the restricted decoder list, install 1 s); the whole three-command run took 90 s including Buildroot's start-up. Not hours: WPE WebKit (9 h) is not touched. From an empty tree it would also build the musl toolchain and GStreamer first: about 25 to 30 minutes, never WebKit (`make ffmpeg gst1-libav` does not depend on it). |
| Disk | FFmpeg's build folder 113 MB, gst-libav 12 MB; the six libraries 4.8 MB; the archive **1.8 MB**; installed on the person's data folder about 5 MB. |
| RAM | Well under 1 GB (the restricted FFmpeg compiles one small file at a time). The 12 GB scope cap was never near. |
| Licence rules | `--disable-gpl --disable-nonfree --disable-version3` (Buildroot passes them because `BR2_PACKAGE_FFMPEG_GPL` and `_NONFREE` are unset); no libx264, libx265, libfdk-aac; extra libraries (libvpx, opus, openssl, libdrm, vorbis, dav1d, bzip2, iconv, xml2, harfbuzz, fontconfig) are switched off in `BR2_PACKAGE_FFMPEG_EXTRACONF`, so the libraries need only musl and zlib. Decoders compiled: `h264 aac aac_latm mp3 mp3float` (HEVC off); no encoders, muxers or network protocols but `file`. Shared libraries, so a person may replace them. `package-av-addon.sh` **refuses** to package when `config.h` says GPL, nonfree or version3, when the configure line names a GPL/nonfree option, or when a library needs anything but musl, zlib and the add-on's own libraries. |
| Licence texts and source offer | Inside the archive: `COPYING.LGPLv2.1`, `LICENSE.md` (from FFmpeg), `NOTICE` (version, licence, decoders, exact configure line with paths shortened, source address and SHA-256, patent note), `SOURCE-OFFER.txt` (the written offer: tarball address, the Buildroot patches shipped in `source/`, three years on request), `av-addon-abi`. The installer refuses an archive without the three legal files (`avaddon::check_unpacked`). |
| Patents | H.264 and AAC are patent-licensed formats in some countries. The add-on is installed on request and never put into an image; the item's texts say so (licence line, `NOTICE`). Same position as the CLI `ffmpeg`. Not legal advice: RELEASE-READINESS item 23 should cover this route. |
| **Finding: libpostproc** | The design listed **seven** sonames including `libpostproc.so.57`. That library only exists with `--enable-gpl` and `gst-libav` does not link it (checked with `readelf -d`: it needs `libavfilter.so.9 libavformat.so.60 libavcodec.so.60 libavutil.so.58`). An LGPL-only build has **six** libraries, so `DEFAULT_SONAMES` in `avaddon.rs` and the ABI file written by `web-post-build.sh` list six; the docs that said seven are corrected. Requiring a GPL-only library would have made the add-on impossible. |

## 2. The build, as run

```
# one-time: the option set goes into the existing tree's .config (build.sh does the same by itself on the next web build)
#   BR2_PACKAGE_FFMPEG_{ENCODERS,DECODERS,MUXERS,DEMUXERS,PARSERS,PROTOCOLS,FILTERS,EXTRACONF} from configs/iyakki_web_av_addon_libs.fragment
# (a diff of .config before and after shows only those lines)
flock ~/.cache/iyakki-br.lock systemd-run --user --scope -p MemoryMax=12G -p MemorySwapMax=8G nice -n 10 ionice -c3 \
  make -C target/buildroot-src/buildroot-2026.02.3 O=$PWD/target/buildroot-web BR2_EXTERNAL=$PWD/dist/buildroot BR2_JLEVEL=4 -j4 \
  ffmpeg-dirclean ffmpeg gst1-libav-reinstall           # PATH needs ~/.cache/br-path first (GNU install; uutils install is refused)
dist/buildroot/package-av-addon.sh                      # -> ~/iyakki-work/addons/ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz (+ .sha256), staging/ beside it
```

`build.sh` now also records the FFmpeg options (`rebuild_if_changed ffmpeg`), so a later change of the decoder list cleans FFmpeg by itself, and
`changed.sh` knows the new fragment. Result (the build is reproducible: two runs gave the same bytes, `gzip -n`, fixed order and times):

| | |
|---|---|
| Archive | `ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz`, 1 846 186 bytes, SHA-256 `fa2a07baed5353aece7962e17db033bcbf8fc9ab9c6702ecc69e59fef290dc47` |
| Libraries (stripped) | `libavcodec.so.60` 2.5 MB, `libavformat.so.60` 420 KB, `libavutil.so.58` 899 KB, `libavfilter.so.9` 215 KB, `libswscale.so.7` 563 KB, `libswresample.so.4` 121 KB |
| Needs | `libc.so`, `libz.so.1` (both in the image) and each other; nothing else |
| `iyakki-av-check` | `bin/iyakki-av-check` inside the archive: the same test as `avaddon::detect_at` (every soname of the image's `/usr/share/iyakki/av-addon-abi`, else the archive's own copy, is a plain file, plus the three legal files); `--version` prints `ffmpeg-libs 6.1.5` |

`target/buildroot-web/target/usr/lib/gstreamer-1.0/libgstlibav.so` is back in `target/` (the old hook had deleted it) and `web-post-build.sh`, run by hand on
a copy of the libraries, parks it and writes the ABI file with the six sonames (`dist/buildroot/test-av-addon.sh`: 9 of 9 pass). **No image was built.**
The next `./build.sh web` (or `smart-build.sh web`) now yields an image **with** the parked bridge: `changed.sh` will say what is needed.

## 3. Who supplies the libraries (owner decision 2026-11-04): the administrator, never IYAKKI

**IYAKKI ships no FFmpeg binaries, not even the archive, in any image or on any project server, and hosts nothing.** The earlier idea (a CLI-catalogue entry with a
project download address) was dropped; no `addons.iyakki.invalid` placeholder remains. What the **image** carries (written by `web-post-build.sh`, also on a repack):

* the parked inactive bridge plugin `/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so` (LGPL glue, no codec) and `/usr/share/iyakki/av-addon-abi` (six sonames);
* `/usr/share/iyakki/av-addon/`: the **recipe** (`package-av-addon.sh`, `iyakki_web_av_addon_libs.fragment`, `iyakki-av-check`, `README` with the exact commands, `LICENSES`) and
  `/usr/bin/iyakki-av-check`; the glue plugin's licence is listed in `LICENSES` and in `legal/image-rules.txt` (the one reviewed `except` row; the `local` link is a `generated` row);
* `/local` -> `/var/lib/iyakki/local`, so **`/local/packages/`** is `<data>/local/packages/` (created at first use, mode 0750, administrators only).

**Where the libraries come from** (`crates/iyakki-agent/src/avaddon_install.rs`, Setup item `ffmpeg-libs` "Browser audio and video (H.264, AAC)", the bar's Install button opens it).
Tried in this order; whichever is chosen, an administrator first reads a **review** (what was found, FFmpeg version, the NOTICE with the configure line and source address, the checks) and confirms
"I built or obtained these libraries and accept their licences and any patent obligations in my country."; nothing is installed before that, and non-administrators are refused.

1. **A local package** `ffmpeg-libs-*-linux-musl-<arch>.tar.gz` (the layout `package-av-addon.sh` makes) in `/local/packages/`, or read-only on a stick named `LOCAL-PACKAGES` or in `iyakki-packages/` on any removable medium.
   Scanning takes plain files only: a link is skipped, so nothing outside the folder is ever read. No network is used. Checks (`inspect_archive`): one folder inside; every soname of the image's ABI file present as a plain file of a sensible size
   (so a build for another FFmpeg version is refused, naming what is missing); `COPYING.LGPLv2.1`, `NOTICE`, `SOURCE-OFFER.txt` present; the NOTICE names the version and a configure line with `--disable-gpl --disable-nonfree` and none of
   `--enable-gpl`, `--enable-nonfree`, libx264, libx265, libfdk-aac.
2. **The libraries of an installed ffmpeg** (owner addition: "Quick install can install it if ffmpeg is already installed"): in the CLI ffmpeg's own folder, the six sonames must exist as shared libraries (a link must stay inside the folder), be ELF
   for this processor (x86_64 or aarch64), need no glibc (`libc.so.6`, `ld-linux`, libm.so.6 ...) and no missing library, libavcodec must contain the h264, aac and mp3 decoders and a configure line that has no `--enable-gpl` or `--enable-nonfree`. They are **copied**
   (never linked) into `<data>/addons/ffmpeg/lib` with any `COPYING.LGPL*` or `LICENSE*` found beside them, plus a generated NOTICE and SOURCE-OFFER. A static single-file ffmpeg (most vendor builds) gets the honest text: "Your ffmpeg is one file without shared libraries, so the browser
   cannot use it. Put the add-on libraries in /local/packages (see the recipe ...) or set a mirror." A glibc build, a GPL build, another processor or other sonames are refused with the reason. Nothing promises a vendor build works; the checks are the feature.
3. **A mirror** an administrator typed (`save_mirror`: https only, no user info, no spaces; an optional SHA-256; **no default address exists**). The download goes through the installer's network stream; with a hash set a mismatch installs nothing, without one the review says the file is not pinned.
   The same full archive checks as for a local package apply either way. (The agent side is done; the Settings -> Browser add-on page that calls `save_mirror` is **not built yet**.)

With nothing supplied the card says exactly where to put the file (`/local/packages/`) and shows the one-line recipe command. Installation puts the files in `<data>/addons/ffmpeg` as a whole (a copy beside it, two renames); the engine picks them up at its next start.
`legal/downloads.txt` has one row (`ffmpeg-libs`, delivery `nothing-installed`): IYAKKI downloads nothing for it.

**Policy hook (documented, not built):** a tenant administrator forbids the add-on in a tenant with a tenant-config item kind `media.codecs` (`allow: false`), checked by the installer next to the role check; `tenantconfig` has `media.policy` for browser media today and `media.codecs` would join it. Until it exists, a tenant that must not have H.264 simply
does not put a package in `/local/packages` and sets no mirror (the card then installs nothing).

## 4. Verified without the Mac (QEMU, no stick)

`dist/buildroot/qemu-av-addon-test.py` boots the existing web image `target/buildroot-web/images` (the Oct 28 initrd, which has **no** bridge plugin: that is exactly
what the owner's Mac has) with a second cpio appended: the plugin parked by the real `web-post-build.sh` and its ABI file, plus an SSH key. It signs in, opens BROWSER,
loads `qemu-web-test/media/av-test.html` (an H.264 baseline + AAC-LC mp4 in a `<video>`; the page reports to the test server), starts the full engine (Shift+F) and presses
Enter on the page's button (QEMU's engine runs with autoplay `deny`: no shared sound device, the real policy of the helper). Results of 2026-11-04:

| | Add-on absent (bridge plugin present, libraries not) | Add-on installed (archive unpacked to `/var/lib/iyakki/addons/ffmpeg`, as the installer does) |
|---|---|---|
| Engine environment | `GST_REGISTRY_1_0=.../gst-registry.bin` only | `GST_PLUGIN_PATH_1_0=/usr/lib/iyakki-av/gstreamer-1.0`, `LD_LIBRARY_PATH=/var/lib/iyakki/addons/ffmpeg/lib`, `GST_REGISTRY_1_0=.../gst-registry-av-d69c1dc6.bin`, `GST_PLUGIN_FEATURE_RANK=avdec_h264:300,...` |
| `canPlayType` | `video/mp4; codecs="avc1.42E01E"` -> `""`, `mp4a.40.2` -> `""` | both `probably` |
| The page's `<video>` | `error` event, code 4, 0 frames; engine log "Your GStreamer installation is missing a plug-in" | `loadedmetadata, canplay, playing`; after 45 s `t=4`, **99 decoded frames**, `result: PASS playing`; screenshot shows the test pattern |
| The bar | "This video needs the audio and video add-on. Install it (your click, downloaded from the vendor, LGPL, licence shown)." with Install, Not now, Don't ask | none |
| In the guest | `gst-inspect-1.0 avdec_h264` -> No such element | `avdec_h264`, `avdec_aac` found; `gst-launch-1.0 filesrc ! qtdemux ! h264parse ! avdec_h264`: **100 frames**; `... aacparse ! avdec_aac`: **174 buffers** (F32LE, 44.1 kHz) |

Also run: the first plain base image (no overlay) shows the bar of an image without the bridge, "This video needs H.264 or AAC, which this image does not include", which is what the owner sees today.
Found on the way, not caused by this work: with QEMU's `virtio-vga` the helper picks the llvmpipe render node and **segfaults** (`webview-helper ... segfault at 0 in libc.so`, engine log
"graphics: hardware llvmpipe ... the helper closed the connection") before any page loads; the test uses `-device VGA`, which has no render node, so the helper takes its shared-memory path.
The test also needed byte-range support in `qemu-web-test/server.py` (the engine's HTTP source treats a plain 200 for a range request as an error).

**Not proven here (only the Mac can):** the real image build with the parked plugin (a repack cannot add the plugin: it needs `./build.sh web`), the download itself (no host), TikTok's own pages
(MSE, DRM-free H.264 profiles above baseline, sound out of the Mac's speakers, performance of software decoding at 720p/1080p on that CPU), and the installer's job ring on the real shell.

## 5. What the owner does on the Mac

1. Run a real web build: `WEB_MEDIA=1 dist/buildroot/smart-build.sh web` (or `./build.sh web`). **A repack of the existing web base cannot make the image complete**: `repack.sh` re-runs `web-post-build.sh`, so the recipe folder, `/usr/bin/iyakki-av-check` and the `/local` link do come with a repack, but the Oct 28 base has no `libgstlibav.so` and a repack cannot add it
   (the ABI file is not written either). The parked plugin needs the build (link-only here: the plugin and libraries are already built in `target/buildroot-web`). Check `zstd -dc rootfs.cpio.zst | cpio -t | grep -E 'iyakki-av|av-addon|^local|libav'`: plugin, ABI file with six lines, recipe, no `libav*`.
2. Boot-test in QEMU (`dist/buildroot/qemu-av-addon-test.py`), then deliver as usual.
3. Put `~/iyakki-work/addons/ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz` (you built it; the project hosts nothing) into the Mac's `<data>/local/packages/` (over `iyakki dev files push` or a stick named LOCAL-PACKAGES), open a TikTok video, press the bar's Install, read the review, confirm.
4. Afterwards `iyakki-av-check` should say "add-on complete: 6 libraries".
