# YouTube on IYAKKI

YouTube is a large web app: fast JavaScript, Media Source Extensions, VP9 or AV1 video, Opus audio, WebGL-free but heavy
CSS. The built-in browser engine (`iyakki-web` + QuickJS) cannot run it. A full engine (WPE WebKit, a separate helper
program) can. Which image has which:

| Where | Engine | YouTube |
|---|---|---|
| `./run.sh window` on a PC | the host's WebKitGTK (helper `dist/webview/host-helper.py`) | works (video needs `xvfb` once) |
| `libre`, `hw`, **`mac`** image | built-in reader only, no helper | **does not load**; the BROWSER tile shows a banner "This site needs the full web engine" with a button that explains the options |
| **`web`** image | WPE WebKit 2.50 + GStreamer + Mesa (llvmpipe) | works in QEMU (Big Buck Bunny, VP9 + Opus, about 27 frames a second); not tried on real hardware |
| **`macweb`** image (`./run.sh build-macweb`) | the same, plus the Mac drivers (keyboard, trackpad, sensors, Wi-Fi firmware, audio) | the one to try on the Mac mini 2014; **not built or booted yet** |

The owner's observation "works in window mode, not on the Mac mini" is exactly the first two rows: the Mac mini was booted from
the `mac` image, which has no full engine.

## What the built-in engine does with YouTube and Google (checked live, 2026-10-18)

Loaded over the same stack as the BROWSER tile (`cargo run --release -p iyakki-shell --example browse_shot -- outdir "cats" "https://www.youtube.com/"`):

* `https://www.youtube.com/` and a `watch?v=` page answer 200 with about 1 MB of HTML that only a script can turn into a page; the tile
  shows the "This site needs the full web engine" banner (it names the `web` and `macweb` images, "macweb is the Mac image with the web
  engine") and an empty page. Nothing is faked.
* `https://www.google.com/search?q=cats` (with or without `gbv=1`) answers 200 with a page whose only content is "enable JavaScript" (the old
  basic-HTML version is gone), so the tile sends the same words to the chosen search engine, with a note. Typing words in the address bar
  never goes to Google. Google's home page loads; its Sign in button leads to `accounts.google.com`, which is a script-driven sign-in and
  is listed with the sites that need the full engine.
* A mistyped name such as `youtub.com` does not resolve; the tab says "Couldn't find youtub.com" and offers a search and "Did you mean
  youtube.com?" instead of the resolver's message.

## What the web engine needs, and where it comes from

Checked against `dist/buildroot/configs/iyakki_web.fragment`, Buildroot 2026.02.3 and the finished `target/buildroot-web` tree:

| Need | In the image |
|---|---|
| Media Source Extensions (YouTube's DASH) | WPE 2.50.5 built with `ENABLE_MEDIA_SOURCE=ON`, `ENABLE_VIDEO=ON`, `ENABLE_WEB_CODECS=ON`; encrypted media is OFF (not needed for ordinary videos; Netflix and paid content will not play) |
| Video decoding without H.264 | `libgstvpx` (VP9 through libvpx), `libgstmatroska` (WebM), `libgstisomp4` (MP4 container), `libgstvideoparsersbad`, `libgstvideoconvertscale`. YouTube offers VP9 to browsers that announce it |
| Audio | `libgstopus` + `libgstopusparse` (needed before MediaSource accepts `audio/webm; codecs=opus`), `libgstaudioconvert`, `libgstaudioresample`, `libgstvolume`, `libgstalsa` (ALSA output, no PulseAudio) |
| Playback pipeline | `libgstplayback` (playbin3), `libgstapp` (appsink/appsrc), `libgsttypefindfunctions`, `libgstopengl` + `libgstwaylandsink` (GL path), software paths otherwise |
| H.264 / AAC / MP3 | gst-libav, **inactive in the image** (parked in `/usr/lib/iyakki-av`, no FFmpeg libraries) and switched on by the person's own download of the audio and video add-on (docs/FULL-BROWSER.md); `WEB_H264=1` / `./run.sh build-web --h264` ships the whole, active codec; `WEB_AV_ADDON=0` leaves the bridge out |
| AV1 | **not playable**: Buildroot 2026.02 has no GStreamer dav1d plugin option (`MediaSource.isTypeSupported('av01...')` is false). YouTube falls back to VP9 |
| JavaScript speed | JavaScriptCore with JIT on x86_64 (`ENABLE_JIT`, `ENABLE_DFG_JIT`, `ENABLE_FTL_JIT` all ON in the build); `ENABLE_C_LOOP` OFF |
| HTTPS | `glib-networking` (OpenSSL GIO module) and `ca-certificates` (`/etc/ssl/certs/ca-certificates.crt`); both in the image |
| Fonts | Liberation (Arial/Times/Courier metrics), DejaVu, the Noto scripts the shell ships (Arabic, Bengali, Devanagari, Gujarati, Gurmukhi, Hebrew, Kannada, Malayalam, Tamil, Telugu). **No CJK**: Chinese, Japanese and Korean titles show as boxes. YouTube's own UI is Latin and renders |
| Clock | **A wrong date breaks HTTPS** (certificates "not yet valid" or "expired"). The Mac mini has a battery clock that may be flat: the image starts SNTP once the network is up (`crates/iyakki-agent/src/ntp.rs`; a clock before 2024 is set by itself). Check `date` before blaming YouTube |
| DNS | from DHCP, or `dns = ` in `/etc/iyakki/net.conf` (for example `dns = 1.1.1.1`) |
| Sound | the mixer starts **muted**: Settings -> Sound, or `amixer sset Master 80% unmute` |

Nothing was missing in the fragment for YouTube; no option was changed for the codec set. The one thing added for the Mac
is the explicit GL fallback (next section).

## Graphics on a Haswell Mac mini (HD 5000)

* The Macmini7,1 GPU is Intel Haswell, generation 7.5. Mesa's **iris** driver starts at generation 8 and does not support it.
  **crocus** (generation 4 to 7) does. The old i965 classic driver was removed in Mesa 22, so in Mesa 26.0.1 (what Buildroot
  2026.02.3 builds) crocus is the only hardware driver for Haswell.
* The default `web` fragment builds only `llvmpipe` (software), `virgl` (VMs) and `radeonsi`. So **on the Mac mini the web
  engine renders in software by default**, and this is stated explicitly:
  * `web-post-build.sh` writes `/usr/share/iyakki/web-gl-drivers` (`software` or `intel`);
  * when an Intel GPU is present and the file says `software`, the shell starts the engine with `LIBGL_ALWAYS_SOFTWARE=1`
    (`webengine.rs`, `force_software_gl`), so EGL uses llvmpipe instead of failing on a render node (`/dev/dri/renderD128`,
    made by the `i915` module) that has no Mesa driver behind it.
* Pages are not drawn by the GPU anyway: the helper renders into shared memory (`wl_shm` buffers through WPEBackend-fdo) and the shell
  copies the picture. The GPU driver only matters for WebGL and GStreamer's GL elements. Haswell has **no VP9 hardware decoding**
  in any case (VP9 decode arrived with Skylake/Kaby Lake), and there is no VA-API in this image, so video is decoded by libvpx on the CPU.
* Optional: `WEB_CROCUS=1 ./run.sh build-macweb` (or `./run.sh build-macweb --crocus`) adds crocus. Mesa 26 builds it
  with its precompiled OpenCL compiler (clang + libclc for the build machine), which Buildroot only enables through iris, so
  `build.sh` adds `iyakki_web_iris.fragment` as well. That costs hours and about 1 GB of image. **Unverified: never built.**
  If it fails to configure, drop it; software rendering is complete.
* The kernel side is in the Mac fragments: `CONFIG_DRM_I915=m` (plus DRM, KMS, `DRM_FBDEV_EMULATION`, simpledrm and EFI framebuffer
  in `linux.fragment`). Haswell needs no firmware blob. If the screen goes black when `i915` loads, boot the GRUB entry
  "safe graphics (simple framebuffer, no GPU drivers)".

## Expected speed on the Mac mini 2014

The Macmini7,1 entry model has an i5-4260U (2 cores, 4 threads, 1.4 GHz, turbo 2.7 GHz); other options are i5-4278U/4308U and
i7-4578U. Measured numbers from QEMU on a PC (4 CPUs, llvmpipe, 1280x800): about 27 frames a second to the shell, YouTube 4K VP9 played
with 1.1 GB for the web process. Those are not Mac mini numbers. Expectation, not measurement:

* page load and scrolling: slower than a PC, usable; the first YouTube page takes tens of seconds (JIT warm-up, software raster);
* video: **480p and 720p VP9 should play**; 1080p at 30 fps is at the limit of two cores; 60 fps and 4K will stutter or drop frames. Choose
  720p in YouTube's gear menu if it stutters. Software VP9 decoding is multi-threaded across the 4 hardware threads;
* the fan will run.

## RAM and size

* Mac mini 2014 came with 4 GB, 8 GB or 16 GB (soldered, not upgradeable). The image is unpacked into RAM (the live root file system):
  **web: 150 MB compressed, 531 MB unpacked** (measured, `target/buildroot-web/images`; ISO 164 MB). The `macweb` image adds the Wi-Fi
  and other firmware of the `hw` variant (hundreds of MB of linux-firmware are not all included, only the named ones), so expect
  roughly 200 MB compressed and 600 to 700 MB unpacked (estimate).
* A page with YouTube open uses about 1.1 GB (web process) plus 0.2 GB (helper and network process), so **4 GB works, 8 GB is comfortable,
  2 GB is untested**. Do not run a plain `<video>` larger than the free RAM: it is buffered into `/var/tmp`, which is RAM on the live image.
* Install to the stick's data volume (not the live-only boot) to keep sign-ins: the profile lives in `/var/lib/iyakki/web-profile/<user>`.

## Test steps for the Mac mini

1. Build (on the build machine, not the Mac): `./run.sh build-macweb` (add `--h264` for H.264/AAC; add `--crocus` to try the Haswell GL driver).
   Hours the first time. Check the kernel options first: `VARIANT=macweb dist/buildroot/verify-kernel-config.sh`.
   Output: `target/buildroot-macweb/images/iyakki-install-x86_64.img`. Write the stick with the IYAKKI USB writer, as for the other images.
2. Boot the Mac holding Option, pick the "EFI Boot" stick, pick "IYAKKI" in GRUB.
3. Sign in. Check `Settings -> Time`: the date must be right. Connect the network (Ethernet is the sure way; the BCM4360 Wi-Fi of the 2014 mini needs a driver pack).
4. Settings -> Sound: unmute.
5. Open the BROWSER tile. Type `example.org`: the reader shows it. Type `www.youtube.com`: with the full engine installed the tab opens it
   in the engine by itself (or press F). Without it (wrong image) you get the banner: "This site needs the full web engine."
6. Open a video, pick 480p, then 720p. Note: does it play, with sound, how many dropped frames (right-click is the shell's; use the YouTube
   "Stats for nerds" through the page's own settings if available).
7. Collect diagnostics (below) and report with the Mac model, RAM and whether the clock was right.

## Diagnostics

* Kernel and graphics: `dmesg | grep -iE 'i915|drm|wpe|gst'` (on the live image: the "remote diagnostics (SSH, keys only)" GRUB entry, then
  `ssh -i ~/.ssh/iyakki_diag root@<ip>`, as in `docs/MAC-HARDWARE.md`).
* Render node present: `ls -l /dev/dri/` should list `card0` and `renderD128`; `cat /usr/share/iyakki/web-gl-drivers` should say `software`
  (or `intel` for a `--crocus` build); `cat /sys/class/drm/card0/device/vendor` prints `0x8086`.
* Helper messages: the helper's standard error reaches the shell line by line; press `c` in the BROWSER tile for the Notes list of the
  tab, and the audit log records the engine starting and stopping. `IYAKKI_WEBVIEW_DEBUG=1` adds a frame statistics line every 5 seconds.
* WebKit and GStreamer logging: add `IYAKKI_WEB_ENV_EXTRA="GST_DEBUG=3,webkit*:5 GST_DEBUG_FILE=/tmp/gst.log"` to the kernel command line
  (GRUB, `e`), then read `/tmp/gst.log`.
* Codec check: open a plain `.webm` (VP9 + Opus) link; if it plays but YouTube does not, the problem is the site or the clock, not the codecs.
* Memory: `free -m` and `ps` for the web process while a video plays.

### Google or YouTube "does not load" on a real Mac

Found on a Mac mini 2018 (`macweb`, software GL): google.com and youtube.com looked empty while tiktok.com worked. The engine was fine: the
audit log (`/var/lib/iyakki/audit.log`) showed `browser.visit` for them and `browser.full` only for tiktok.com, so the built-in reader had
been showing Google and YouTube (an empty or stripped page), and only tiktok.com had been handed on to the engine. Since this version YouTube,
Google and the other heavy sites go straight to the engine. To check an engine by hand, on the device:
`probe /usr/libexec/iyakki/webview-helper https://www.youtube.com/ --size 1920x1080 --allow '*' --profile /tmp/p --env HOME=/tmp/p
--env XDG_RUNTIME_DIR=/tmp/p/run --env LIBGL_ALWAYS_SOFTWARE=1 wait:15 shot:/tmp/y.png` (the `probe` example of `iyakki-webview`, built with
`cargo build --release -p iyakki-webview --example probe --target x86_64-unknown-linux-musl`; it prints every load event and the helper's
messages). A page that loads there but not in the tile points at the shell, not at the network or the engine.

### Video on the Mac mini 2018 (measured 2026-10-19, macweb, software GL, 1920x1080, HDMI monitor)

* Capabilities in the engine: VP8, VP9, Opus and Vorbis `probably` (MSE and `canPlayType`); AV1, H.264 and AAC absent (by policy).
* Before the fix video did not start: `play()` was rejected with NotAllowedError (autoplay of a video with sound), so YouTube's watch page
  showed its first frame and sat paused at 0:00; one click started it. The helper now allows autoplay (`website-policies`, `helper.c`).
* After: a local VP9+Opus WebM played (currentTime advanced in real time, 129 decoded, 0 dropped), the ALSA PCM `card1 pcm3p` (HDMI)
  went to RUNNING (S16, 48 kHz, 2 channels, through `iyakkivol`, volume control at maximum); YouTube's Big Buck Bunny 60fps played by itself at
  1280x720 (VP9): 628 frames in 11 s, 57 dropped, WPEWebProcess about 4.4 of 6 cores. 1080p60 will drop more frames; pick 720p or lower.
* A local H.264 MP4 fails with MEDIA_ERR_SRC_NOT_SUPPORTED (code 4): expected, see the codec policy.
* Only one engine can have the HDMI PCM at a time (the engine opens `default` = `iyakkivol` = `plughw:1,3`, no mixing).
* Option, not enabled: hardware H.264 through VA-API needs `BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_VA` plus `BR2_PACKAGE_INTEL_MEDIADRIVER`
  (MIT and BSD-3, pulls in intel-gmmlib, libva and Mesa); not tried on the Mac.
* How to measure: the `probe` example (steps `eval:JS`, `click:X,Y`, `shot:FILE`), `/proc/asound/card1/pcm3p/sub0/status`, and
  `/proc/<WPEWebProcess>/stat`.

## Window mode (what works today)

`./run.sh window`, BROWSER tile, type `youtube.com`; the host helper uses WebKitGTK. Video needs `sudo apt install xvfb` once.

## Input notes

Clicks and typing in the page: see "Clicking and typing" in `docs/FULL-BROWSER.md`. Pointer positions are device pixels and must not be
divided by the interface scale again (that made clicks miss at 125% and above). Click the search box (or let YouTube focus it), type, Enter.

## Unverified

No `macweb` image has been built or booted. Everything about Haswell performance, crocus, the final image size, the black-screen risk
on `i915` load and the 2014 Wi-Fi is expectation. The YouTube result on the `web` image comes from QEMU with a PC (llvmpipe, no real GPU).
