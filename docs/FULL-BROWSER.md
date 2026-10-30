# The optional full web engine

IYAKKI's own browser (`iyakki-web` + QuickJS) reads documents and runs small scripts. It cannot run YouTube or
other large web apps: that needs a fast JavaScript engine, a GPU-style compositor and video decoding. For those
sites a real web engine (WebKit) runs as a **separate helper program** and its picture is shown *inside the browser's
own page area*:

    iyakki shell (the BROWSER tile)                      helper program (one per tab, no compositor, no screen of its own)
      tabs, address bar, back/forward, stars  <-- messages (docs/WEBVIEW-PROTOCOL.md) -->  WebKit renders OFFSCREEN
      page area: draws the latest picture     <-- pixels in a shared file in /dev/shm -->  (GStreamer plays the video into it)
      keys, mouse, wheel, paste, zoom  -------------------------------------------------->

It works the same in the desktop window (`./run.sh window`), on the Linux framebuffer (QEMU, bare metal) and in the web
front end, because the shell only draws pixels. Nothing is linked into the `iyakki` binary: the default image has no helper,
and F says so.

| where | the helper program | engine |
|---|---|---|
| desktop window (`./run.sh window`, a development machine) | `dist/webview/host-helper.py` (found next to the checkout; `IYAKKI_WEBVIEW_HELPER` overrides) | WebKitGTK (`python3-gi gir1.2-webkit2-4.1 gir1.2-gtk-3.0`; for video also `xvfb`) |
| the `web` and `macweb` image variants | `/usr/libexec/iyakki/webview-helper` (a small C program, `dist/webview/wpe-helper/`, built by the Buildroot package `iyakki-webview-helper`) | WPE WebKit 2.50 with libwpe and WPEBackend-fdo (shared-memory buffers), GStreamer inside it |

The same helper plays the MEDIA app's videos on the `web` image: MEDIA starts a view of its own with `allow_hosts = [127.0.0.1]` and
points it at a loopback-only server that serves the one chosen file under a random token (`mediaserve.rs`; no `file:` addresses,
see `docs/MEDIA-SEARCH.md`, "Video on the web image").

When no helper is installed (the default, `hw` and `mac` images), the BROWSER tile says so plainly instead of showing a half-loaded page:
a banner above the page, "This site needs the full web engine.", appears for known web apps (YouTube, Netflix, Gmail, Outlook, Google
Docs/Sheets/Slides/Drive, Slack, Notion, Twitter/X, Facebook, Instagram, Maps, WhatsApp Web, Teams, Figma, Spotify; the list is
`HEAVY_SITES` in `webengine.rs`) and for any page the reader gave up on. Its "How to get it" button opens a short help page (no network).
With the helper present there is no banner: such pages open in the engine by themselves.

## Using it

In the BROWSER tile press **F** (or click **Full browser** in the toolbar). The current tab shows its page with the full
engine; the address bar, back, forward, reload, tabs, zoom, bookmarks (Ctrl+D), history and downloads stay the browser's own
and drive the engine. The button then reads **Reader**.

| Key | Does |
|---|---|
| typing, mouse, wheel, Ctrl+C, Ctrl+V | go to the page while it has the keyboard (the default after F or a click in the page) |
| Ctrl+L | the address bar (typing there never reaches the page); Enter sends the address to the engine; Esc gives the keyboard back to the page |
| Esc | the browser takes the keyboard back from the page (`b` back, `f` forward, `r` reload, `g` address, `+ - 0` zoom; Up, Down, PgUp, PgDn, Home, End, Space still scroll the page) |
| F or Esc again (browser has the keyboard), or the Reader button | back to the built-in reader on the same page |
| Ctrl+T, Ctrl+W, Ctrl+Shift+T, Ctrl+PgUp/PgDn, Ctrl+D, Ctrl+H, Ctrl+B, Ctrl+J | the browser's tab, bookmark, history and download shortcuts, as always |

**Clicking and typing (fixed 2026-10).** The helper used to divide mouse and wheel positions by the interface scale a second time
(WPE already divides device-pixel positions by the device scale factor), so at any scale above 100% (a 4K or large screen: the shell
picks 125% at 3840x2160) a click landed 20% or more away from where the pointer was: it missed text boxes, blurred Google's
autofocused search box and looked like "typing does not work". At 1280x800 (scale 100%) the bug was invisible, which is why QEMU
tests passed. Positions now go to WPE unchanged (the protocol always said device pixels). Checked in QEMU at 3840x2160 with a page
that logs its events (`dist/buildroot/qemu-web-input-test.py`, `dist/buildroot/qemu-web-test/input.html`): a click at screen
(378,282) reached the page as (302,99) = (378/1.25, (282-157)/1.25); letters, digits, shifted symbols, Backspace, Delete, Home,
Tab, Ctrl+A/C/V, the Google search box with suggestions and the Google sign-in email box all work.
The shell's keys go to the page while it has the keyboard; the way back is Esc (browser keys), Ctrl+L (address bar), Ctrl+T/W (tabs).
A touchpad (a device with BTN_TOOL_FINGER) is not used by the shell: use a mouse.

**Clipboard.** Ctrl+C or Ctrl+X in a page, a page's `copy`/`cut` event and `navigator.clipboard.writeText` (a "Copy" button such as a
sign-in code) all reach IYAKKI's clipboard (a "Copied" toast; the helper injects a small user script and sends the text as
`Clipboard`). Ctrl+V pastes the shell's clipboard into the focused field (`Text`). `navigator.clipboard.readText` returns an empty
string.

Each tab that uses the engine has its own helper (a tab in the background stops drawing). Ctrl+F (find in page), right-click
menus, pointer shapes and input methods are not available in the engine yet. A capital F typed in a page is a letter, not the
switch (press Esc first).

Rules: the account's host policy is applied first (a host the account may not open is refused), and again for every
address the engine reports (a redirect or a link to a refused host is undone with a notice and an audit entry). An account that
is limited to a list of sites cannot use the full engine at all, because pages pull in files from any site. Downloads the page
starts are cancelled in the engine and fetched by the browser's own download code (Downloads folder, never overwriting). The
profile (cookies, local storage, cache) is per person in `/var/lib/iyakki/web-profile/<user>` (mode 0700), so sign-ins
are kept like in a normal browser (on the data volume; in a live session without one they are gone at power-off). If the
helper is not installed, F says so; if it crashes or does not start, the tab goes back to the built-in reader with the reason in the status
line (and the audit log). The helper discovery and the profile folder are in `crates/iyakki-agent/src/webengine.rs`; the
client, the wire format and a fake helper for tests are the `iyakki-webview` crate; the browser side is
`crates/iyakki-shell/src/apps/browser_full.rs`.

Try it in window mode: `./run.sh window`, open the BROWSER tile, type `youtube.com`, press F. (For video, `sudo apt install xvfb` once:
the helper starts a private invisible X server for the engine. Without it pages work and video does not.) A page is drawn about 20 to 30
times a second while it changes.

### The old way: cage and cog (no longer in the image)

Before the helper existed, F handed the screen and keyboard to `cage` (a kiosk Wayland compositor) running `cog`. The `web` image
no longer contains cage, cog, wlroots, libinput, seatd or eudev (nothing else needed them; /dev is mdev like the default image), so
that path is dead there. The code stays for a custom image that has `/usr/bin/cage` and `/usr/bin/cog` and no helper
(`webengine::run`, `iyakki-shell`'s `external.rs`/`backend.rs`/`fb.rs`, `iyakki web-open URL`); it can be deleted later.

## Building

    ./run.sh build-web            # = dist/buildroot/build.sh web   -> target/buildroot-web/images/
    ./run.sh build-web --h264     # also ships H.264/AAC (see Codec policy)
    WEB_IRIS=1 ./run.sh build-web # also Mesa's iris driver for Intel gen8+ (adds clang: hours more)
    JOBS=12 ./run.sh build-web    # fewer parallel jobs when RAM is short
    ./run.sh build-macweb         # Intel Macs: the mac variant (drivers + firmware) and the web engine in one image -> target/buildroot-macweb/images/
    WEB_CROCUS=1 ./run.sh build-macweb   # also Mesa's crocus driver for Haswell (Intel gen4-7); adds the iris fragment (clang): hours more, unverified

The variant is the x86_64 defconfig plus `dist/buildroot/configs/iyakki_web.fragment` (Mesa with LLVM/llvmpipe, WPE WebKit,
GStreamer, glib-networking for HTTPS, fonts, the helper package) and the overlay `board/iyakki-web/overlay`; `web-post-build.sh`
adds the Noto fonts the shell ships, a marker `/usr/share/iyakki/web-gl-drivers` (`software` or `intel`: with an Intel GPU and `software` the shell runs the engine with `LIBGL_ALWAYS_SOFTWARE=1`), a machine id and removes gst-libav unless asked. aarch64 is not done (the same fragment would
need testing; llvmpipe and WebKit support aarch64 in Buildroot).
Host needs: ~40 GB disk, 16+ GB RAM (28 GB used here, `JOBS=5`), several hours the first time, minutes for an incremental build.

The helper is a Buildroot *external* package (`dist/buildroot/package/iyakki-webview-helper`, `_SITE_METHOD = local` on
`dist/webview/wpe-helper`; `external.mk` and `Config.in` list it). Only the web fragment selects it, so the default image is
unchanged. Rebuild just it: `make -C target/buildroot-src/buildroot-2026.02.3 O=$PWD/target/buildroot-web BR2_EXTERNAL=$PWD/dist/buildroot iyakki-webview-helper-dirclean`
then `./run.sh build-web`. Cross-compile by hand: `make -C dist/webview/wpe-helper CC=... PKG_CONFIG=...` (needs
wpe-webkit-2.0, wpebackend-fdo-1.0, wayland-server, glib-2.0; `make clean` afterwards: Buildroot copies the folder and would
reuse a stale binary).

Build notes (things that went wrong, so the next person does not lose time):

* **`gpgrt fatal: sizeof lock obj`**: libgpg-error cross-builds its lock object with an awk script; with `mawk` (Debian/Ubuntu hosts)
  it writes an empty one and WebKit's web process dies at start. `external.mk` replaces it by the package's ready-made header.
* **No HTTPS** ("TLS support is not available"): WebKit's network process needs the GIO TLS module, so `BR2_PACKAGE_GLIB_NETWORKING`
  (OpenSSL flavour; the CA bundle is the image's `/etc/ssl/certs`).
* **YouTube "Your browser can't play this video"**: MediaSource refused `audio/webm; codecs=opus` until gst-plugins-bad's
  `opusparse` (`BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_OPUS`) was built. Changing a package's options needs `<pkg>-dirclean` first.
* **A plain `<video src=...>` fails with error code 4** while MediaSource video (YouTube) plays: WebKit buffers a progressive video
  through GStreamer's `downloadbuffer` into `/var/tmp/WebKit-Media-XXXXXX`, and the image had no `/var/tmp` ("Could not create temp
  file"; seen with `IYAKKI_WEB_ENV_EXTRA="GST_DEBUG=3,webkit*:5 GST_DEBUG_FILE=/tmp/gst.log"` on the kernel command line, which the
  shell passes to the helper). `web-post-build.sh` now creates it (mode 1777). It is RAM on the live image, so a plain video larger than
  the free memory cannot be played that way. The MEDIA app's video player depends on this.
* `WPE_BACKEND_LIBRARY=libWPEBackend-fdo-1.0.so.1` is set by the helper (the web process loads the same backend); rendering
  uses WPEBackend-fdo with only `wpe_fdo_initialize_shm()`, so Mesa's Wayland EGL falls back to wl_shm buffers (llvmpipe), which
  is what `export_shm_buffer` hands over. WPEPlatform (the newer API) is not built in WPE 2.50.5 here.
* initrd tricks for testing: a second cpio appended to `rootfs.cpio.zst` works only when the first file's size is a multiple of 4.

Test it in QEMU: `dist/buildroot/qemu-web-test.sh` (a wrapper of `qemu-web-test.py`: boots, creates the account, opens BROWSER,
loads the local test page, google.com and youtube.com in the full engine and saves PNGs; `SSH_KEY=` adds a key so you can
`ssh -p 2222 root@127.0.0.1`; set `IYAKKI_WEBVIEW_DEBUG=1` in the helper's environment for a frame statistics line every 5 s).

### The `macweb` variant (Intel Macs)

`dist/buildroot/build.sh macweb` = the x86_64 defconfig + `iyakki_firmware.fragment` + `iyakki_mac.fragment` (kernel drivers: applesmc, hid_apple,
Cirrus audio, i915, Broadcom Wi-Fi/Bluetooth bits) + `iyakki_web.fragment` (+ `iyakki_web_h264.fragment` with `WEB_H264=1`, + iris/crocus with
`WEB_IRIS=1` / `WEB_CROCUS=1`). The fragments do not set the same option twice (the Mac one sets the kernel fragment list, the web one the rootfs
overlay and post-build hooks), so the order does not matter. Release files are named `iyakki-macweb-*` (`./run.sh release --with-macweb`).
The reason it exists: the `mac` and `web` images could not be combined, and the Mac mini 2014 booted from `mac` has no full engine. Why the
web engine renders in software on a Haswell Mac mini and what to expect: `docs/YOUTUBE-ON-IYAKKI.md`. Not built or booted yet.

## Troubleshooting

* A site "does not load" although the engine is installed: check the audit log for `browser.full`. Without it the built-in reader showed
  the page. YouTube, Google and the other known heavy sites open in the engine by themselves; other pages move to it when the reader judges
  it cannot show them, or press F. After the engine stops, or after F leaves it, the shell stops sending sites to it until the BROWSER is reopened.
* To test the engine alone on a device, use the `probe` example of `iyakki-webview` (see `docs/YOUTUBE-ON-IYAKKI.md`, Google or YouTube
  does not load). On a Mac mini 2018 it loaded Google and YouTube at 1920x1080 with software GL and the real LAN.
* **No sound while the radio, TALES or another program is playing.** On a card that one program holds at a time (the T2 Mac's) the
  engine's sound cannot open it. When a page plays and a named player or an outside program holds the card, the browser shows a dialog
  ("Radio is playing. Stop it so the browser can play?") with **Stop it**, **Keep it** and **Cancel** (`y`/Enter, `n`, Esc). Stop it ends
  IYAKKI's own player cleanly, or an outside program through the same guarded stop the power-off uses (only the owner of an open playback
  stream, never init or the shell; SIGTERM, then SIGKILL), and then the page's media reconnects its sound. Keep it leaves things as they are.
  Every answer is in the audit log (`browser.sound.*`). The other way round, a radio station that starts while the page plays asks the
  same question. See `docs/development/AUDIO-MIXER.md`.
* **Browser sound through the mixer (experimental).** Settings -> Sound has a switch (`W`, off by default, needs the mixer, applies the
  next time the engine starts) that sends the engine's sound through a named pipe into IYAKKI's mixer so it can play with the radio.
  It is not proven on real hardware; if the browser goes silent with it on, turn it off. Its status row says what it is doing.
* Video that needs H.264 (TikTok): without the audio and video add-on the engine has no decoder. The helper notices the failed player and the
  shell shows one bar ("This video needs the audio and video add-on. Install it ..."); see *Audio and video add-on* below.

## MEDIA says "Error relocating ... symbol not found" (or "undefined symbol")

That is the dynamic linker, from a program whose libraries are not all there. Seen on a Mac mini 2018 `macweb` image: the web post-build had
removed FFmpeg's libraries (codec policy) but left `/usr/bin/ffmpeg` and `libavdevice`, and MEDIA ran that ffmpeg. Fixed twice: the hook
(`dist/buildroot/board/iyakki-web/web-post-build.sh`) now removes the whole FFmpeg set unless `WITH_FFMPEG=1` or `WEB_H264=1`, and the player only
uses an ffmpeg that starts. All GStreamer plugins of the image load (`GST_REGISTRY_1_0=/tmp/r gst-inspect-1.0` on the device lists 37 and no error).

## Video does not start

The helper allows autoplay (without it `play()` fails with NotAllowedError and a page waits for a click). VP8/VP9 and Opus/Vorbis play;
H.264, AAC and AV1 do not; an H.264-only page now gets the add-on bar instead of a silent black player (below). Measurements: `docs/YOUTUBE-ON-IYAKKI.md`.

## Camera and microphone (getUserMedia)

State on 2026-10-25: **a page cannot use the camera or microphone in the web image**, because the engine is built without media-stream
(`BR2_PACKAGE_WPEWEBKIT_MEDIA_STREAM` is not set in `iyakki_web.fragment`, so `-DENABLE_MEDIA_STREAM=OFF`; `-DENABLE_WEB_RTC=OFF` is fixed
by Buildroot's package) and the image has no `v4l2` GStreamer plugin. The shell side is in place and tested with the fake helper: the helper
passes the page's request to the shell (`PermissionRequest` / `PermissionAnswer`, [`WEBVIEW-PROTOCOL.md`](WEBVIEW-PROTOCOL.md)), the browser
asks the person (Allow once / while this site is open / always / block), remembers the choice per person and shows a red **Camera on** chip
while a page captures. The helper turns the setting on and denies on silence. Causes, the Buildroot change, cost and decisions:
[`development/BROWSER-MEDIA-DEVICES.md`](development/BROWSER-MEDIA-DEVICES.md).

## Codec policy

Shipped by default: VP9 (libvpx), Opus (with opusparse), Vorbis; AV1 is NOT playable (libdav1d is built but GStreamer 1.24 in Buildroot has no dav1d plugin: `MediaSource.isTypeSupported('av01...')` is false); MP4 and WebM demuxing; audio through GStreamer's ALSA sink
(no PulseAudio/PipeWire). H.264, AAC and MP3 come from FFmpeg through gst-libav; WPE's "multimedia" option selects
gst-libav, so it is *built*. What the image carries is set by `web-post-build.sh`: by default (`WEB_AV_ADDON=1`) only the inactive, LGPL
bridge plugin (next section) and no FFmpeg; `WEB_AV_ADDON=0` neither; `WEB_H264=1` (`configs/iyakki_web_h264.fragment`) the whole, active codec. Without it, YouTube still plays because it offers VP9 to browsers that
announce it (measured); other sites that only have H.264 will not play video. These codecs are patent-encumbered in some
countries (a separate matter from the copyright licences below); whoever builds and distributes the H.264 variant is
responsible for that.

**Decision for the alpha (owner, 2026-10-18): leave H.264 and AAC out.** Questions looked at, not legal advice (a lawyer must
confirm before any image ships these codecs):

* **The Apache-2.0 licence does not help.** Its patent grant covers only patents of the people who contributed code. The H.264
  and AAC patent pools are third parties, and the licence disclaims warranties ("AS IS").
* **"Link to the platform's decoder" has no platform here.** On macOS, Windows or Android the system vendor has paid for the codec
  and an application only calls its API. IYAKKI is the operating system: the image would be the thing that carries the decoder.
  A design where IYAKKI ships only the glue and the person installs the decoder (as with ffmpeg in Quick install, downloaded
  from its vendor at the person's request, never in an image) is consistent with how IYAKKI already treats third-party tools.
* **Options for later, none done:**
  1. *Hardware decoding through VA-API* for H.264 video (GStreamer's `va` plugin, libva and Intel's media driver are MIT; the
     Intel GPU in the Macs has a fixed-function H.264 decoder, and AMD GPUs have one too). The decoder is the chip's, not ours.
     Video only: AAC audio has no hardware route. Needs a build option, GPU testing on a Mac and a legal opinion about whether
     driving the chip's decoder is covered by the chip vendor's licence.
  2. *Cisco's OpenH264 binary* (GStreamer's `openh264` plugin loads it at run time). Cisco pays the royalties only for the
     binary downloaded from Cisco's own servers, never for a self-built copy, and it does not cover AAC. Its Linux builds are
     believed to be glibc builds (to verify), which would not load on IYAKKI's musl images.
  3. *A person-installed libavcodec*: this is the route taken, see *Audio and video add-on* below (decision 2026-10-24, after the owner's
     "go for ffmpeg support"). It needs libraries built for musl and for the FFmpeg version of the image.
* **What works without any add-on:** YouTube (VP9 and Opus), most video platforms that offer WebM, and any H.264 file through
  the installed ffmpeg in IYAKKI's own player. Sites that only have H.264/AAC do not play video in the browser.
* **MP3 in the browser (2026-10-28):** the image carries its own MP3 decoder, the GStreamer element `iyakkimp3dec` (`dist/webview/wpe-helper/mp3dec.c`, built around minimp3, CC0 / public domain, the C original of the `nanomp3` crate the radio uses; `crates/iyakki-agent/tests/mp3dec_match.rs` checks both give the same samples). MP3 patents expired in 2017, so it ships in the image, with no download. Found on the Mac: a recorder site whose only format is MP3 stalled on "Processing..." because the engine had no MP3 decoder. The boot self-test decodes a sample through it (`path mp3-decoder`). Only the Mac can say that real pages play.
* **AAC and H.264 still need the add-on, and the add-on has no download yet (2026-10-28):** `avaddon.rs` expects a Store item `ffmpeg-libs` (FFmpeg 6.1 shared libraries built for musl); `legal/downloads-manifest.json` has none (only `cli:ffmpeg`, a static program the radio uses). Until the owner picks a build, the add-on cannot be installed.

## Audio and video add-on (H.264, AAC, MP3 in the browser)

**What is shipped: nothing of FFmpeg.** The `web` and `macweb` images carry GStreamer's `gst-libav` plugin, which is LGPL-2.1+ glue code
(it is the part that speaks GStreamer's API; the codecs are in FFmpeg's libraries). `web-post-build.sh` parks it in
`/usr/lib/iyakki-av/gstreamer-1.0/`, a folder GStreamer does not scan, so it is inactive, and removes every FFmpeg library and program.
`/usr/share/iyakki/av-addon-abi` lists the sonames the plugin was linked against (FFmpeg 6.1: `libavcodec.so.60`, `libavformat.so.60`,
`libavutil.so.58`, `libavfilter.so.9`, `libswresample.so.4`, `libswscale.so.7`, `libpostproc.so.57`).

**Why this design (investigated, not built yet).** `gst-libav` hard-links the libraries (DT_NEEDED), it does not `dlopen()` them, so
there is no switch that makes it load libav* "if present": without the libraries the plugin simply fails to load and GStreamer
blacklists it. That is acceptable here: the plugin sits outside the scanned path, and `crates/iyakki-agent/src/avaddon.rs` puts three
things into the engine's environment only when every soname of the ABI file exists in the add-on's library folder
(`<data>/addons/ffmpeg/lib`, or `IYAKKI_AV_ADDON_DIR`): `GST_PLUGIN_PATH_1_0` (the parked plugin folder), `LD_LIBRARY_PATH` (the library
folder) and a separate `GST_REGISTRY_1_0` (`cache/gst-registry-av-<hash>.bin`, hashed from the host name and the folder's file names and
sizes, so the list of plugins with the add-on never mixes with the one without it), plus `GST_PLUGIN_FEATURE_RANK` for `avdec_h264`,
`avdec_aac`, `avdec_mp3`. Without the add-on the environment is exactly the old one. The decoders run in the web engine's own
sandboxed WebProcess, as before. The price is an ABI pin: the downloaded libraries must be **FFmpeg 6.1 sonames, built for musl** (the
images are musl) and with the H.264, AAC and MP3 decoders; a vendor that only offers glibc builds or another major version does not
work, and the installer's manifest must say which download provides them. The fallback if that pin proves too brittle is a small
in-house GStreamer element that `dlopen()`s whatever libavcodec the person has through its public C API (more code, no ABI pin of
the plugin); it is not built.

**The person's side.** When a page's player fails for want of a decoder, `media-probe.js` (injected by the helper into every frame, see
`docs/WEBVIEW-PROTOCOL.md`, MediaUnsupported) tells the shell, and the BROWSER tile shows a bar over the bottom of the page: "This video
needs the audio and video add-on. Install it (your click, downloaded from the vendor, LGPL, licence shown)." with **Install**, **Not now**
and **Don't ask for this site**. It appears once per site and session; the last choice is kept per person (`browser-av-<user>.json`).
Install does not download anything by itself: it opens Store -> Setup on the add-on's entry (the radio's audio add-on uses the same one),
where the vendor, the licence and the size are shown and the person presses the button. When the libraries are in place the browser
starts its engine again on the same page, because the engine reads its environment at start. The probe only reports when the engine's
own `canPlayType` says the codec is missing, so a broken file never produces the message.

**Developer tools for the chat agent.** A second injected script, `dev-probe.js`, reports failed requests, slow resources, console errors
and warnings and page load timings (protocol message 0x90 `DevEvent`, `docs/WEBVIEW-PROTOCOL.md`). The shell stores them, scrubbed and in
bounded ring buffers, only while the person has turned on *Developer tools for the chat agent* in Settings -> Browser (default off, audited),
and the chat tools `browser_failed_requests`, `browser_timings` and `browser_console` read them (`docs/BROWSER-AGENT.md`). The helper source
(`helper.c`, `dev-probe.js`, `Makefile`) builds only in the full web image: **not compiled or run** in the development checks.

**Developer tools panel (for the person).** F12, Ctrl+Shift+I or the small `</>` chip in the BROWSER tile's tool bar (its click label is still "Dev tools F12"; grey, amber with failed requests, red with errors) opens a panel
with Console, Network and Timings (Esc, the Close button or the same keys close it). It docks at the bottom (about 35 % of the page area)
or, on very wide windows or with `d`, at the right; the page and the engine are resized to what is left, so nothing is drawn over the page.
The divider can be dragged and `+` / `-` resize it. It reads the same buffers as the chat agent's tools (`iyakki_agent::devtools`, through
`DevTools::seq` and `snapshot`, so a heartbeat costs one comparison): **Console** (errors and warnings with level colour, time, source and
line; filter by text and level, Clear), **Network** (failed and slow requests with address, status or error and time; Failed only, sort by
newest or slowest, filter, Clear) and **Timings** (first response, DOMContentLoaded and load as a bar of the load phases, and the slowest
resources). **Copy as text** puts the rows the filters leave (scrubbed, no query strings) on the clipboard. Opening the panel starts
collecting for this person for as long as it is open, **without** changing the saved *Developer tools for the chat agent* switch (the chat
tools still follow that switch only); closing it stops collecting and forgets what was collected, unless the switch is on. Nothing is
written to disk. The toolbar button shows the counts of failed requests and console errors while something is collecting. Lines from before
the panel was opened are not kept: reload the page to see its start. There is **no JavaScript console input and no Elements tab**: the
web view protocol has no safe way to run a script and return a result, so none was added. The panel needs the full web build for the
`dev-probe.js` probe and says so when no probe line has arrived. In the built-in reader the panel's Network tab lists the reader's own
requests (addresses scrubbed). F12 is also the shell's screenshot key: the tile answers `App::claims_f12`, but the shell must consult it before starting a
screenshot (not wired yet, so for now Ctrl+Shift+I and the button are the working ways in; Print Screen still takes screenshots). Not run against real pages.

**Patents and licences.** The LGPL libraries are downloaded by the person from their vendor; IYAKKI ships none of them and does not
redistribute them. H.264 and AAC are patent-licensed formats in some countries: decoding them is the person's matter after their own
download, as it is with the ffmpeg of Quick install. IYAKKI's own code only calls the public GStreamer API. The legal opinion
(RELEASE-READINESS item 23, BETA-PLAN decision 15) should cover this route too: shipped inactive LGPL glue plus a user-initiated download.

**Remaining image-build steps** (nothing here needs the Mac): (1) the existing `target/buildroot-web` has no `libgstlibav.so` any more (the
old hook deleted it from `target/`): run `make -C target/buildroot-web gst1-libav-reinstall ffmpeg-reinstall` so the plugin and the
libraries are in `target/` again, then `./build.sh web` (or `smart-build.sh`); `repack.sh` cannot add the plugin to a finished rootfs
and says so (a warning). (2) Check `cpio -t` of `rootfs.cpio.zst`: `usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so` present, no `libavcodec`,
`usr/share/iyakki/av-addon-abi` with seven lines. (3) On a device with the add-on folder filled, run `GST_PLUGIN_PATH_1_0=/usr/lib/iyakki-av/gstreamer-1.0
LD_LIBRARY_PATH=<folder> GST_REGISTRY_1_0=/tmp/r gst-inspect-1.0 avdec_h264` and then open a TikTok video. (4) Make the add-on's installer
entry (radio side) provide the libraries listed in the ABI file; its id must match `avaddon::INSTALL_ITEM` (`ffmpeg-libs`).
`dist/buildroot/test-av-addon.sh` checks the hook on a fake rootfs.

## Licences

WPE WebKit LGPL-2.1+/BSD-2-Clause, libwpe/WPEBackend-fdo BSD-2-Clause, Mesa MIT, LLVM
Apache-2.0 with exceptions, GStreamer LGPL-2.1+, glib-networking LGPL-2.1+, fonts OFL/Vera; the helper itself is IYAKKI's Apache-2.0. They are
separate programs and dynamically linked libraries; IYAKKI only starts them, so its Apache-2.0 licence is unaffected.
Source and licence texts: `target/buildroot-web/legal-info/` (made by `build.sh`). See `LICENSING.md` item 9 and `NOTICE`.

## Results (QEMU, KVM, 4 CPUs, 4 GB RAM, llvmpipe, 1280x800, 2026-10-12)

Image (web variant, `./run.sh build-web`): `rootfs.cpio.zst` 149 MB (531 MB unpacked, kept in RAM), ISO 164 MB (the default image is far smaller).
The helper program is 47 KB. Boot to the sign-in screen needs about 94 MB of process memory plus the 520 MB the unpacked image
occupies; with a page open: helper 91 MB RSS, network process 86 MB, web process 320-430 MB (Google, Wikipedia), about 1.1 GB while
YouTube plays 4K VP9 (about 1.9 GB used in the machine in total, so 4 GB is comfortable and 2 GB is not tested).

Seen with the screen read through the QEMU monitor (the shell on the Linux framebuffer, keys sent as real key presses):

| page | result |
|---|---|
| the local test page | full page: CSS grid, canvas animation (the page measured 47-60 fps itself), fonts, JavaScript, WebGL context available |
| https://www.google.com/ | the home page, typing "hello world" gave live suggestions, Enter opened the results page (AI overview, links, images), PgDn scrolled |
| https://en.wikipedia.org/wiki/WebKit | full article; the wheel scrolled it (3 notches about 150 px), a click on the "Large" text radio changed the page (the shell's pointer is drawn over the page) |
| https://www.youtube.com/ and a watch page | the page, search box, recommendations; Big Buck Bunny (VP9 + Opus through MediaSource) played with moving pictures after the key `k`; the virtual sound card received audio samples (the mixer starts muted: `amixer sset Master 80% unmute` or Settings -> Sound) |

Frames: about 26-28 pictures a second reach the shell while the canvas page or the video plays (the helper's cap is 30; the engine
delivers 30 a second), 0 while nothing changes. Not seen: AV1 (not built), H.264 (by policy), a real GPU, bare metal, a USB stick boot.
Once, after navigating a view from Google to a YouTube watch page and pressing `k` twice, the player area stayed white
while the engine kept delivering identical pictures; a fresh load of the same page played, so it is probably a YouTube/engine state
and was not reproduced.

Known limits: AV1 and H.264/AAC do not play; the user-agent shows `Version/60.5` (WPE's own); WebKit's sandbox (bubblewrap) is off;
find in page and pointer shapes are not wired; right-click is the shell's.
