# Browser: camera, microphone, screen share and WebRTC (getUserMedia)

Status 2026-10-25. Owner report on a real Mac, full web engine: "the browser does not support features for accessing media devices".
This is item 1 of the v1.0.0 list in [`../project/BACKLOG.md`](../project/BACKLOG.md). Owner decisions of the same day: **WebRTC calls and screen
sharing are in scope for v1.0.0**; permissions are per website and per kind (microphone, camera, screen), each asked on its own; an indicator
that cannot be missed; mock devices so that tests and a QEMU guest run without hardware.

This page has the verified causes, what exists in code (shell side, tested with the fake helper), what the engine rebuild must change (prepared as
a switch, **not run**), the screen-share design, the test pages, the steps and the open decisions.

## 1. Root causes (verified in code and in the build tree)

Four independent layers each stop it. Fixing one alone changes nothing the person sees.

| # | Layer | Evidence | Effect |
|---|---|---|---|
| 1 | **Engine build flag** | `dist/buildroot/configs/iyakki_web.fragment` sets `BR2_PACKAGE_WPEWEBKIT_MULTIMEDIA=y` (line 30) but never `BR2_PACKAGE_WPEWEBKIT_MEDIA_STREAM`. Buildroot 2026.02.3 `package/wpewebkit/wpewebkit.mk:70-75` then passes `-DENABLE_MEDIA_STREAM=OFF`. The finished tree agrees: `target/buildroot-web/build/wpewebkit-2.50.5/CMakeCache.txt` has `ENABLE_MEDIA_STREAM:BOOL=OFF` (line 526). | The engine does not contain `MediaDevices`: `navigator.mediaDevices` is `undefined`, so a page prints exactly "browser does not support media devices". This alone explains the report. |
| 2 | **WebRTC flag** | `wpewebkit.mk:34` hard-codes `-DENABLE_WEB_RTC=OFF` (no Config.in option exists); the cache has `ENABLE_WEB_RTC:BOOL=OFF` (line 704) although `USE_GSTREAMER_WEBRTC=ON` (line 1200). | No `RTCPeerConnection`: Meet, Zoom, Jitsi calls cannot work even after layer 1 is fixed. |
| 3 | **Helper setting** | `dist/webview/wpe-helper/helper.c` (before this change) line 1018: `webkit_settings_set_enable_media_stream(st, FALSE);`, and lines 495-498: `on_permission` called `webkit_permission_request_deny(req)` for every request. | Even with an engine that has media-stream, the setting was off and every permission request was denied; there was no way for the person to say yes. |
| 4 | **Image packages** | `target/buildroot-web/.config` line 820: `# BR2_PACKAGE_GST1_PLUGINS_GOOD_PLUGIN_V4L2 is not set`; `target/buildroot-web/target/usr/lib/gstreamer-1.0/` has `libgstalsa.so` (so `alsasrc` and its device provider exist) but no `libgstv4l2.so`, no `jpeg`, `videorate`, `videotestsrc`, `audiotestsrc`, `rtp`, `rtpmanager`, `udp`, and no `webrtc`, `dtls`, `srtp`, `sctp`, `nice` plugin. | The engine (once built with layer 1) finds no camera; MJPEG webcams cannot be decoded; the mock devices have no sources; no WebRTC plugins for layer 2. |

Not a cause (checked):

* **Secure context.** `getUserMedia` needs https or `http://localhost`; real sites are https, and the helper loads pages by `webkit_web_view_load_uri`
  (`helper.c:423`, `:802`, `:856`) with no special scheme. The policy below refuses non-secure origins itself.
* **Kernel.** `dist/buildroot/board/iyakki/linux.fragment:307-312` has `SND_USB_AUDIO=m`, `MEDIA_SUPPORT=y`, `MEDIA_CAMERA_SUPPORT=y`, `MEDIA_USB_SUPPORT=y`,
  `USB_VIDEO_CLASS=m` (same in `board/iyakki-aarch64/linux.fragment:281-285`): USB webcams and USB microphones get `/dev/video*` and ALSA capture cards.
* **Settings that exist in this WPE (2.50.5 headers in `target/buildroot-web/staging`):** `enable-media-stream`, `enable-mock-capture-devices`,
  `enable-webrtc`, `webrtc-udp-ports-range`; permission classes `WebKitUserMediaPermissionRequest` (audio, video, display flags) and
  `WebKitDeviceInfoPermissionRequest`; `camera-`, `microphone-` and `display-capture-state` with setters on the web view (setting NONE ends a capture).
* GStreamer is 1.24.13 (WebKit needs 1.20 for its WebRTC path); OpenSSL 3 is in the image (`libssl.so.3`), as WebKit's WebRTC check requires.
* `media-probe.js` and `dev-probe.js` do not touch media devices.

### Per device kind: what is missing and in which layer

| Feature | Engine build | Helper | Permission UI | Image packages | Kernel / device nodes |
|---|---|---|---|---|---|
| `enumerateDevices` (names) | `ENABLE_MEDIA_STREAM` (missing) | done (answered yes only for a site that already has a device) | done | `v4l2` plugin for cameras; `alsa` present | none extra |
| **Camera** (USB webcam) | `ENABLE_MEDIA_STREAM` (missing) | done (not run) | done | **`v4l2`, `jpeg` (MJPEG), `videorate`** (missing) | `USB_VIDEO_CLASS=m` present; device node permissions under mdev not checked |
| Camera (T2 built-in FaceTime) | same | same | same | same | needs the `facetimehd` driver and firmware the person extracts (`docs/RESTRICTED-FIRMWARE.md`); **unverified**; not a v1.0.0 promise |
| **Microphone** (USB) | `ENABLE_MEDIA_STREAM` (missing) | done | done | `alsa` plugin present (`alsasrc`); which ALSA device it opens is not checked | `SND_USB_AUDIO=m` present |
| Microphone (T2 built-in) | same | same | same | same + `libgstiyakkimic.so` (section 5, Microphones) | Capture streams are refused by the driver unless `apple_bce` `capture=1` (off by default: opening one while sound plays has hung the T2). Listed by the plugin only when the switch is on; unproven on the Mac. A USB microphone or headset works. |
| **Screen share** (`getDisplayMedia`) | `ENABLE_MEDIA_STREAM` (missing) | request, question, stop and the file path are done; the source is **not** (section 5) | done (this time only / block) | PipeWire + our portal service + a frame feeder (none exist) | none |
| **WebRTC calls** | `ENABLE_WEB_RTC=ON` (Buildroot hard-codes OFF; override prepared) | `enable-webrtc` default | n/a | `webrtc` + `dtls` + `srtp` + `sctp` plugins, `nice` (libnice), `rtp`, `rtpmanager`, `udp` (missing) | UDP to the internet |

## 2. Permission model and what the person sees

Principle: the page never decides. The helper only reports a request and obeys an answer; silence means no.

### Per kind, per site, per person

Microphone, camera and screen share are three separate questions, each remembered separately. A request for several kinds is asked **one kind at a time**
(microphone, then camera, then screen); the page hears nothing until every kind is answered, and one no ends the request.

| Kind | Buttons (keys) | What is remembered (per person, per site, in `browser-media-<person>.json`) |
|---|---|---|
| Microphone | **1** Allow once, **2** Allow while this site is open, **3** Always allow this site, **4** Block this site, Esc Not now | Always and Block. "While open" lives in memory and ends when the person goes to another site or clicks Stop. |
| Camera | same | same |
| **Screen share** | **1** Share this time only, **4** Block this site, Esc Not now (no "while open", no "always") | **Only Block.** An allow is never remembered, not for the session either. |

**Rule for the screen (decided as the owner's default, safest sensible):** the screen shows everything open on this computer, including other tiles, the
vault and chat, so it is asked every single time and a remembered allow does not exist, not even for a site the person trusts for the camera. A camera allowed
for ever does not allow the screen. Block is still remembered (and listed for removal), because blocking is the safe direction. If you want a trusted-site
"always" for the screen later it is one line in `MediaPerms::apply` and one button; I recommend against it.

Other rules, first match wins: only secure origins (https, or localhost); a tab in the background never gets a device (answered no at once, audit line);
a second question while one is open is answered no; a blocked kind is refused without asking; an allowed kind is not asked again; the device-name request
(`enumerateDevices` labels) is answered yes only for a site that already has a device. The question is answered no when the person opens another tab or
the page moves. The helper adds: 60 s without an answer is no, at most 4 requests wait, a navigation or a crash is no, location and notifications are
always no, a screen request is refused by the helper (no question) unless the image has a screen source (`IYAKKI_WEBVIEW_SCREEN=1`) or mock devices are on.

### Indicator, unmissable

* A red chip **Camera on / Microphone on / Screen on / Camera + microphone on ...** in the browser tool bar (from the engine's capture state, so a page
  cannot hide it).
* A red sign **"A page is using your camera and microphone. Click to stop."** at the bottom left of **every screen of the shell** (every tile, the launcher,
  dialogs), above the assistant's camera sign when both show. Clicking it stops what every page captures (the engine is told to set the capture states to
  NONE), ends "while open" allowances and writes an audit line. It disappears when the capture ends or the browser is closed.

### Protocol (`docs/WEBVIEW-PROTOCOL.md`)

* helper to shell **PermissionRequest 0x91** (`u32 id`, `u8 kinds`: 1 microphone, 2 camera, 4 screen, 8 device list; `str origin`).
* shell to helper **PermissionAnswer 0x13** (`u32 id`, `bool allow`); **StopCapture 0x14** (`u8 kinds`); **ScreenSource 0x15** (`str path`, empty ends).
* helper to shell **CaptureState 0x92** (`u8 kinds`, 1, 2, 4).
* All optional. Audit lines `browser.media.ask|allow|deny|stopped`: host, kind words, outcome. Never a path, picture, sound or device name.

### Interaction with sound and the Camera tile

* **Capture vs playback.** The in-house mixer ([`AUDIO-MIXER.md`](AUDIO-MIXER.md)) shares playback; capture is another ALSA direction, so a call can play and
  record on a USB headset together. The T2 speaker path uses `plughw` and cannot be shared; recording on the T2 built-in card is the case that has hung
  the machine, so it stays off. Which device the engine's `alsasrc` picks is not verified.
* **Camera tile and the browser.** A UVC camera can be opened twice but only one opener can stream. The CAMERA tile and the assistant's camera tools already say
  "close the app first"; the browser should answer a page with a calm "camera in use" in the same case (not done, step 7).
* Opening capture must not trigger the "stop the radio so the browser can play" question; capture states are separate from `AudioPlaying`.

## 3. What exists now, and how it is tested

Compiled and tested here (Rust, fake helper `fake://media/KINDS/HOST`, `fake://capture/KINDS`; an allow in the fake starts the capture the page asked for):

* `crates/iyakki-webview/src/proto.rs`, `client.rs`, `bin/fake_helper.rs`, `tests/helper.rs`, `examples/probe.rs`: the messages; round-trip, exact bytes, truncation.
* `crates/iyakki-shell/src/apps/browser_media.rs`: policy (`MediaPerms::decide/apply/list/revoke`), store, question, indicator text, the sign's shared state,
  the screen feed (`ScreenFeed`, header and seqlock format) with unit tests; wiring in `browser.rs`, `browser_full.rs`, `backend.rs` (the feed call after each
  picture), `shell.rs` (`draw_capture_sign`, click id `Tab(913)`); integration tests in `browser_media_tests.rs`.
* `FEATURES.md` rows F-BROW-69 to F-BROW-73.

Written, **not run** (needs the full web build):

* `helper.c` and `proto.h`: media-stream setting on (`IYAKKI_WEBVIEW_MEDIA=0` turns it off), mock devices with `IYAKKI_WEBVIEW_MOCK_MEDIA=1`, the permission queue,
  capture-state signals including the display, `StopCapture`, the screen-path message. It passes `gcc -fsyntax-only -Wall -Wextra` with the build tree's cross compiler
  against the WPE 2.50.5 headers (no warnings); a source-reading test pins the wiring. Harmless on today's engine, so the helper can ship before the rebuild.
* The test pages (section 6) have never run in an engine.

## 4. The engine rebuild (prepared, NOT started)

A new switch, off by default, like `WEB_AV_ADDON`: **`WEB_MEDIA=1`**.

* `dist/buildroot/configs/iyakki_web_media.fragment` (every symbol checked to exist in Buildroot 2026.02.3):

  | Option | Why | Licence (Buildroot metadata) |
  |---|---|---|
  | `BR2_PACKAGE_WPEWEBKIT_MEDIA_STREAM` | getUserMedia, getDisplayMedia, enumerateDevices (selects `videorate`) | WPE WebKit LGPL-2.1+, BSD-2-Clause (already shipped) |
  | `BR2_PACKAGE_GST1_PLUGINS_GOOD_PLUGIN_V4L2`, `BR2_PACKAGE_GST1_PLUGINS_GOOD_JPEG` | USB webcams, MJPEG | gst-plugins-good LGPL-2.1+ |
  | `BR2_PACKAGE_GST1_PLUGINS_BASE_PLUGIN_VIDEORATE`, `_AUDIORATE`, `_VIDEOTESTSRC`, `_AUDIOTESTSRC` | rate conversion; mock devices | gst-plugins-base LGPL-2.0+ |
  | `BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_WEBRTC` (selects `dtls` with OpenSSL, `sctp` with the bundled usrsctp, `srtp` with libsrtp) | `webrtcbin` | gst-plugins-bad LGPL-2.0+; **libsrtp BSD-3-Clause**; usrsctp (bundled, BSD-3-Clause); OpenSSL 3 (Apache-2.0, already in the image) |
  | `BR2_PACKAGE_LIBNICE` | ICE, and the `nice` GStreamer plugin | **MPL-1.1 or LGPL-2.1** |
  | `BR2_PACKAGE_GST1_PLUGINS_GOOD_PLUGIN_RTP`, `_RTPMANAGER`, `_UDP` | RTP for WebRTC | gst-plugins-good LGPL-2.1+ |

  All dynamically linked; no GPL part, no new codec (Opus, VP8 and MJPEG only; no H.264 encoder, so the patent decision is untouched). The gst-plugins-bad
  `legal-info/` of the build must still pass the licence gate: the package flags some of its other sub-plugins as GPL or unknown, none of the ones above.
  `BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_WPE` (`libgstwpe.so`, 84 KB, unused by IYAKKI) `depends on !MEDIA_STREAM` and drops out by itself.
  PipeWire (`BR2_PACKAGE_PIPEWIRE`, MIT; `_GSTREAMER`) is in the fragment as comments, for step 9.
* `dist/buildroot/package/iyakki-web-media/iyakki-web-media.mk`: appends `-DENABLE_WEB_RTC=ON` to `WPEWEBKIT_CONF_OPTS` when `IYAKKI_WEB_MEDIA=1` (a later `-D`
  wins in CMake; WPE's GStreamer WebRTC path needs `gstreamer-webrtc` and OpenSSL 3, both present). It is its own file so that `external.mk` stays unchanged: editing
  `external.mk` makes `changed.sh` demand a **full** rebuild of everything.
* `build.sh` (`WEB_MEDIA=1` appends the fragment and exports `IYAKKI_WEB_MEDIA`), `changed.sh` (composes it, `media=` in the environment record),
  `smart-build.sh`, `test-changed.sh` (the decision table has the case; 37 pass). `changed.sh` now reports `PACKAGES` for the switch.
* Sizes, estimates: v4l2 + jpeg + rate + testsrc plugins well under 1 MB; MEDIA_STREAM and WebRTC code in `libWPEWebKit` (116.7 MB today) a few MB; webrtc, dtls, srtp,
  sctp, nice, rtp plugins plus libnice and libsrtp a few MB. Within the owner's rule that the desktop image may grow a few MB for hardware.

**Exact commands** (from the repository root, on the build machine; `macweb` the same with `macweb`):

```
dist/buildroot/changed.sh web --adopt          # once, only if target/buildroot-web images are current
WEB_MEDIA=1 dist/buildroot/smart-build.sh web --dry-run   # prints the decision and the commands
WEB_MEDIA=1 dist/buildroot/smart-build.sh web
```

**Cost:** the decision is `PACKAGES` (no toolchain change): WPE WebKit is dirclean-ed and rebuilt, **about 9 h (541 min) with this tree, which has no ccache**
(`BR2_CCACHE` is not set; `docs/BUILD-INCREMENTAL.md` lines 19, 49, 139); the GStreamer plugin packages, libnice, libsrtp and the helper add minutes (the
bad-plugins rebuild a few minutes). With a warm ccache of the same sources 30-60 min. Then the image is repacked. Do capture, WebRTC and the mock-device
sources in this one rebuild; do not split them.

## 5. Screen share: how the screen reaches the engine

Verified in the WebKit source in the build tree (`Source/WebCore/platform/mediastream/gstreamer/GStreamerDisplayCaptureDeviceManager.cpp`, `DesktopPortal.cpp`):
for a real `getDisplayMedia` the engine does **not** use a GStreamer device provider. It lists one fixed device "Capture Screen", then calls the D-Bus
**`org.freedesktop.portal.Desktop` ScreenCast portal** (CreateSession, SelectSources, Start, OpenPipeWireRemote), gets a **PipeWire** node id and file descriptor, and
reads the node with `pipewiresrc`. With mock capture devices on it uses `MockDisplayCaptureSourceGStreamer` instead ("Mock screen device 1/2", test pattern, no portal).
Upstream sources are never patched in this project (`upstream_sources_are_never_patched`, boundary test), so the engine's side cannot change.

| Option | Verdict |
|---|---|
| Custom GstDeviceProvider / `appsrc` | Impossible: the display path never asks the device monitor (see above). |
| **v4l2loopback** (shell writes frames to a virtual camera) | The page would see a *camera* "IYAKKI screen", not `getDisplayMedia`, and it pollutes `enumerateDevices`. v4l2loopback is **GPL-2.0+** (Buildroot metadata), an out-of-tree kernel module: allowed in the same way as `apple-bce` (GPL-2.0, module only, never linked into the program), but it gives the wrong API. Not chosen. |
| Shell framebuffer grab straight to the helper over a pipe | The right *source* (the shell already holds the picture), but the engine cannot read a pipe. It needs a PipeWire node in front of it. |
| **Chosen: in-house portal + PipeWire node fed from the shell's own picture** | The only route the unpatched engine accepts. Pieces: (1) the shell writes its screen into a shared file (done: `ScreenFeed`, about 4 pictures a second, RGBA, odd/even sequence number so a reader never copies a half-written picture; the file lives in /dev/shm, mode 0600, removed when the share ends) and tells the helper its path (`ScreenSource`, done); (2) in the helper, a feeder: GStreamer `appsrc ! videoconvert ! pipewiresink` reading that file (to write); (3) a minimal in-house implementation of the ScreenCast portal on a private session bus started by the helper (about 300 lines of GDBus: the four methods and the `Request::Response` signals; it returns the feeder's node id and the PipeWire socket fd) (to write); (4) `BR2_PACKAGE_PIPEWIRE` (MIT, about 2-3 MB) with its GStreamer plugin and a `pipewire` daemon started by the helper (fragment lines are commented out). |

Least risky, in order: ship everything else first (capture, WebRTC) and keep the screen **gated** (the helper refuses a screen request unless `IYAKKI_WEBVIEW_SCREEN=1`
or mock devices are on, so nobody is asked a question that cannot be fulfilled); the mock display exercises question, indicator, stop and the shell side end to end; then
build (2) to (4) behind `IYAKKI_WEBVIEW_SCREEN`. Risks: PipeWire needs a session manager or explicit linking to connect a `pipewiresrc` to a `pipewiresink` node (not verified
without a session manager; test in the QEMU guest first); the portal needs a D-Bus session bus (dbus is in the image); sharing "the screen" means the shell's own picture,
i.e. every tile, which the question says in plain words; a window picker is not offered (the whole shell picture only).

### One open per camera (shared camera plugin)

WebKit's GStreamer capture builds one `v4l2src` per `getUserMedia` call, and a USB (UVC) camera node can be opened and format-set by
one streamer only. A page that asks twice (webcamtests.com does, and so does any page that probes presets) got "Device is busy",
`S_FMT failed` and `not-negotiated` on the second pipeline and waited for ever on "Starting your webcam".

The web process is a separate process, so the fix is a GStreamer plugin it loads, not code in the helper:
`dist/webview/wpe-helper/camshare.c` (installed as `/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so`). It registers the device provider
`iyakkicamshare`, which wraps the v4l2 provider: each camera is listed once with the same name and ONE fixed mode (the smallest mode
of at least 1920x1080 at 24 frames/s or more, MJPEG or raw as the camera offers it; else the biggest). `create_element()` gives each
pipeline a small leaky `appsrc`; a per-camera hub in the web process owns the only `v4l2src ! capsfilter ! appsink` and copies every
frame to every live consumer. It opens when the first consumer starts, and closes two seconds after the last one stopped. The
engine's own `decodebin3`, `videoscale` and `videorate` then give each page the size and rate it asked for. The helper puts the raw
`v4l2deviceprovider` on rank 0 (like the ALSA provider) so the device monitor lists the wrappers only, and only when the plugin file
exists; `IYAKKI_WEBVIEW_CAMSHARE=0` switches it off. Consent, the indicator and the audit are unchanged (they sit above the engine).
`make -C dist/webview/wpe-helper check` runs the self check with a test camera that refuses a second open.

### Microphones (the `iyakkimic` plugin, 2026-10-28)

Owner's T2 MacBook, full engine: `getUserMedia({audio})` made an `alsasrc` and failed with `gstalsasrc.c:527:set_hwparams: Rate doesn't match
(requested 44100Hz, get 0Hz)`, then `not-negotiated`; the page recorded a flat line. WebKit also logged `Could not open device hw:0,N for
inspection!` for the stock ALSA device provider's `hw:` devices (that provider opens every stream to ask for its caps). Replacing `pcm.hw` by a
`plug` layer in `/etc/asound.conf` made it worse (`no device was found amongst 0 devices`: the stock provider lists nothing then).

What was proven and what is inferred:

* **Proven on a PC (real HD-Audio capture card):** `gst-launch alsasrc device=hw:1,0 ! audio/x-raw,rate=16000,channels=1` fails
  `not-negotiated` (the card does 44100 to 192000 Hz only); the same with `device=plughw:1,0` works, as do `arecord -D plughw:1,0` at
  16000 Hz mono, 44100 Hz stereo and 48000 Hz stereo. alsa-lib's `plug` layer converts rate, format and channels; a raw `hw:` source cannot.
* **Proven in code:** the T2 driver patch (`apple-bce/0003`) refuses every capture open with `-ENODEV` unless `capture=1`; IYAKKI's dictation
  never records through `default` (a playback-only softvol device) but through `-D plughw:C,D` of the microphone chosen in the Voice tile.
* **Inferred, not proven:** that `get 0Hz` comes from the T2 `hw:` capture parameters (the Mac's hardware parameters of `Digital Mic` were
  never read; the helper's own probe could not open them). The plugin logs them next time (below).

The fix is a device provider plugin like the camera one: `dist/webview/wpe-helper/micshare.c` with the scan in `micscan.h`
(`/usr/lib/gstreamer-1.0/libgstiyakkimic.so`, device provider `iyakkimic`, rank primary+1; the helper puts the stock `alsadeviceprovider` on rank 0
whenever the plugin file exists, so nothing probes `hw:` devices or logs that noise). It
* reads `/proc/asound/cards` and `card<N>/pcm<D>c/info`; it never opens a playback stream; it skips HDMI, DisplayPort, IEC958/S-PDIF and
  loopbacks (the T2 `Bridge Loopback`); order: a stream called *mic* (T2 `Digital Mic`), then a USB microphone, then analog inputs
  (`Codec Input` on the T2), then the rest; the person's choice goes first;
* asks alsa-lib what each device offers (rate and channel ranges; `hw:` first, then `plughw:`, the exact error kept) with a non-blocking open
  that is closed at once; a device that cannot be opened for any reason but "busy" is not listed (and the log line says why);
  **a T2 sound card is listed only when `/sys/module/apple_bce/parameters/capture` is on and is never opened to be inspected**;
* names devices from the card, e.g. `Built-in microphone (Apple T2 Audio)`, `EMEET SmartCam Nova 4K (USB microphone)`;
* gives each page request a small `appsrc`; one hub per device owns the only `alsasrc device=plughw:C,D ! audioconvert ! audioresample !
  S16LE 48000 Hz (1 channel unless the hardware has two) ! appsink`, opens it when the first page starts and closes it 2 s after the last
  one stopped, so the card is never held while idle; nothing here touches IYAKKI's mixer or softvol output (capture only);
* logs to stderr (the system log) `iyakki-mic: microphone: requested by a page; capture devices (n): [plughw:0,1 'Built-in microphone (...)' score
  100, <probe result>] ...; skipped: [...]; the person's choice ...; chosen plughw:0,1 via alsasrc ! audioconvert ! audioresample as S16LE 48000
  Hz 1 ch` once per request (from the helper, whole) and at provider start, plus `opening ...`, `stopped: <GStreamer error and debug line>`
  and `closed ...` from the hub. `IYAKKI_WEBVIEW_MICPROVIDER=0` switches the plugin off (the stock provider stays on rank 0 only when
  `IYAKKI_WEBVIEW_ALSA_DEVICES` is unset, as before).

**One choice for Voice and Browser (decision).** The Voice tile's Microphone setting used to live only in the tile's memory, so the Browser could
not know it. It is now `VoiceSettings.mic` (an ALSA id such as `plughw:1,0`, empty is *Automatic* = the first microphone found, never the loopback and
never `default`). The Voice tile reads it when it opens and writes it when the person picks; the Browser's microphone question has a *Microphone*
select box (click, or key M; type to filter; Enter chooses) that reads and writes the same setting, and the shell hands it to the plugin through
`iyakki-browser-mic` in the shared memory folder (`/dev/shm`; `IYAKKI_MIC_DIR` moves it for tests). A page that passes its own `deviceId` still wins. One shared setting was
chosen over separate ones because the person has one microphone in mind and the two places would otherwise disagree (dictation working, the browser
silent). A choice for a device that is gone falls back to Automatic. Audit line `browser.media.microphone`.

**When a microphone cannot be opened.** The hub writes `<device name><TAB><reason>` to `iyakki-browser-mic-status` (and removes it when sound
arrives); the Browser reads it every second and puts a plain sentence in its status line, e.g. *The microphone Built-in microphone (Apple T2 Audio)
could not be used: another program is using it. Choose another microphone in the question or in the Voice tile.* (reasons: busy, not offered or
switched off, not allowed, format refused, else the sound system's own words). The page itself still gets the GStreamer error.

Tests: `make -C dist/webview/wpe-helper check` runs `mic-check` (fake `/proc/asound` trees: a T2-like layout with playback-only devices, an HDMI card,
a webcam's USB microphone and the capture switch on and off; a PC with an IEC958 capture and a loopback card; nothing at all; the person's choice;
three pages at 44100 Hz stereo, 16000 Hz mono and 48000 Hz stereo on one open of a test microphone that refuses a second open; the close after the
last page; a failing microphone writes its status). `crates/iyakki-webview/tests/micshare.rs` runs it when the GStreamer and alsa development files exist.
Shell tests: the chooser, the shared setting, four screen sizes without overlap, the failure sentence. A QEMU check (`-device intel-hda -device
hda-duplex` gives a capture card to the selftest "audio-capture card") can list the device with `gst-device-monitor-1.0 Audio/Source` in a booted
image: **not done**, it needs a repacked image.

**Follow-up (not built): the camera.** The same idea for the camera: the Camera tile already has a camera picker; the Browser's camera question
should get a *Camera* select box with the same shared setting, and the camera hub (`camshare.c`) should list the chosen camera first.

## 6. Mock devices and test pages

* **Mock devices:** `IYAKKI_WEBVIEW_MOCK_MEDIA=1` makes the helper set `enable-mock-capture-devices`. The engine then offers (verified in `MockRealtimeMediaSourceCenter.cpp`):
  "Mock audio device 1/2/3", "Mock video device 1/2" (1280x720 and 640x480 test pictures), "Mock screen device 1/2" (1920x1080 and 3840x2160), with synthetic sound and picture
  from `audiotestsrc` / `videotestsrc` (in the fragment). No hardware is needed. The same switch lets the helper accept screen requests.
* **Fake helper (no engine at all):** `fake://media/KINDS/HOST` asks, `fake://capture/N` sets the capture state; used by the Rust tests of section 3.
* **Pages** in `dist/buildroot/qemu-web-test/media/` (served by the existing `server.py` at `/media/`; each page reports `MEDIA-TEST <name> PASS|FAIL <detail>` in its title,
  in `#result`, and to `/report`), expected results in `expected.json`:

  | Page | Checks | Expected with mock devices |
  |---|---|---|
  | `enumerate.html` | getUserMedia then `enumerateDevices` | PASS, 3 audio and 2 video devices with "Mock ..." labels |
  | `gum-audio.html` | microphone, AudioContext analyser | PASS, peak above 0.001 |
  | `gum-video.html` | camera into a `<video>` | PASS, size 640x480 or 1280x720, at least 10 frames in 5 s |
  | `rtc-loopback.html` | two `RTCPeerConnection`s in one page, camera + microphone + data channel | PASS, ICE connected/connected, decoded frames > 0, audio packets > 0, echo "ping" (FAIL "RTCPeerConnection is undefined" means no WebRTC in the engine) |
  | `display.html` | `getDisplayMedia` after a click on the button | PASS, "Mock screen device 1 1920x1080" |
  | any page with `?expect=denied` | the refusal | PASS means NotAllowedError (Block, Not now) |

  Secure context: the existing QEMU server is reached as `http://10.0.2.2:PORT`, which is not a secure context, so `navigator.mediaDevices` is undefined there (that is itself a correct
  FAIL to assert for the insecure case). For the real flow forward the server into the guest's loopback and open `http://localhost:PORT/media/`:
  `ssh -p 2222 -N -R PORT:127.0.0.1:PORT root@127.0.0.1` (the QEMU test already has the key and the 2222 forward), then answer the questions with `sendkey 1` on the monitor.
  The QEMU smoke test that does this is the next step; it needs a `WEB_MEDIA=1` image with `IYAKKI_WEBVIEW_MOCK_MEDIA=1` in the helper's environment.

## 7. Steps, sizes, risks

| # | Step | Size | Done |
|---|---|---|---|
| 1 | Protocol, fake helper, per-kind questions, store, indicator chip, shell sign with stop, screen feed, tests | M | **yes** |
| 2 | Helper: setting, permission queue, capture state, stop, screen path, mock switch | S | written, syntax-checked; **not run** |
| 3 | Fragment, switch, package file, build script and `changed.sh` support | S | **yes, off by default** |
| 4 | Test pages and expected results | S | **yes**, never run |
| 5 | The engine rebuild (owner runs the commands in section 4) | XL machine time | no, waits for the owner |
| 6 | QEMU smoke test with mock devices (guest loopback forward, the five pages, the denied variants) | M | no |
| 7 | Settings -> Browser -> Camera, microphone and screen list (Remove) from `MediaPerms::list/revoke`; user-guide screenshots; "camera in use" answer when the Camera tile or the assistant holds it | S | no |
| 8 | Real devices: USB webcam + USB headset on the T2 Mac (device node permissions, which ALSA device `alsasrc` opens, `v4l2` device provider without udev, MJPEG at 720p, CPU of llvmpipe), then an AMD PC; echo and latency of a call on the T2 speaker | M | no |
| 9 | Real screen share: feeder, in-house portal, PipeWire (section 5) | L | no |
| 10 | Real calls: Jitsi, then Meet and Zoom web; echo cancellation (`webrtcdsp`, check its licence first) | L | no |

Risks: (a) the 9 h rebuild fails at configure for WebRTC (first configure of `ENABLE_WEB_RTC=ON` is untried: `GSTREAMER_WEBRTC_LIBRARIES` was NOTFOUND in today's cache);
(b) the engine's `alsasrc` picks the T2 card and hangs the Mac: never offer the built-in microphone and make the default capture device explicit;
(c) llvmpipe cost of a 720p camera and WebRTC encode/decode on a MacBook; (d) an old shell with a new helper waits 60 s then denies (harmless);
(e) a page can ask again and again: one question at a time, Not now remembers nothing; add a cool-down if abused; (f) the whole-screen share shows secrets: asked every time, red sign, one click stops it.

## 8. Decisions for the owner

1. **Screen "always allow" is not offered** (only "this time only" and Block). Tell me if you want a trusted-site always for the screen; I recommend against.
2. **Whole-screen only** for the first screen share (no window picker). OK?
3. **Devices first:** USB webcam and USB headset or microphone; built-in T2 microphone stays off (BACKLOG options A and E); built-in FaceTime camera not promised.
4. **Camera and microphone "Always allow this site":** built; keep, or remove the button so these never persist?
5. **Approve the rebuild:** `WEB_MEDIA=1` (about 9 h without ccache; do you want ccache switched on for this tree first, which makes every later WebKit change 30-60 min?).
6. May the new helper ship in the next image before the engine rebuild (harmless; nothing visible changes)?

## Choosing and deselecting devices (2026-10-28, F-BROW-78)

The owner's rule: the question lets the person pick the default camera and microphone and approve; a defective device can be deselected, and a
deselected device is never given to a page (`enumerateDevices` does not list it); "Always allow this site" remembers the device for that site.

* **In the question.** The microphone question has had its Microphone chooser (key M) since 1028g; the camera question now has the same
  Camera chooser (key C). The list has Automatic and each device once. **Delete** on a highlighted device marks it "don't use": it leaves
  the list at once and the list stays open. Delete on Automatic does nothing.
* **Always allow this site** also stores the device shown at that moment, per site (`MediaPerms::set_site_device`, in
  `browser-media-<user>.json` next to the allow). It is kept only for a site that is allowed always, is dropped when the site is blocked or
  removed, and is ignored when that device is unplugged or hidden (the default is used instead, nothing is asked again).
* **Hidden devices** are per person (`browser-devices-<user>.json`: the default camera and the hidden cameras and microphones;
  `iyakki_agent::browserdev`). Settings -> Browser shows them under "Cameras and microphones you marked don't use" with **Use again (1..9)**;
  the section is drawn near the top, only when something is hidden, because the tab does not scroll.
* **Naming.** A microphone is its ALSA id (`plughw:1,0`). A camera is its Video4Linux card name with the ends trimmed (the engine's display
  name, for example `FaceTime HD Camera (Built-in):`); two identical cameras share a name and are listed once.
* **The plugins.** Four files in the shared folder (`devlist.h`): `iyakki-browser-mic` and `iyakki-browser-cam` (use first), and
  `iyakki-browser-mic-hidden` and `iyakki-browser-cam-hidden` (one per line). The shell writes them when the engine starts and before it
  answers every allow. `micscan.h` skips a hidden microphone (reported as "hidden by the person"); `camshare.c` does not wrap a hidden
  camera and, every 2 s, rebuilds its device list (chosen camera first, hidden ones gone) once no camera is open.
* **The shared folder.** `/dev/shm` was never mounted on the images, so the shell used the temp folder while the plugin read `/dev/shm`: a
  chosen microphone could not reach the plugin on the Mac. `/dev/shm` is now mounted at boot (`iyakki-core::boot`, tmpfs, nosuid, nodev,
  1777), and the plugin uses the shell's rule (`IYAKKI_MIC_DIR`, else `/dev/shm`, else the temp folder).
* **Not proven.** That the engine takes the first camera of the list as the default one (the order is set; only a computer with two cameras
  shows it), and `make check` of the plugins on a machine with GStreamer (the code compiles with the image's toolchain).
