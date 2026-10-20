# The media player (video and sound without a window system)

The Mac mini image and the other framebuffer images have no window system, no GPU userspace and no mpv, VLC or ffplay
(ffplay needs SDL2, and SDL2 has no plain framebuffer backend: only KMSDRM, Wayland and X11). So IYAKKI plays video and
sound itself and uses **`ffmpeg` only as a decoder**.

## Using it

1. **Install ffmpeg once.** IYAKKI menu (F10) -> Quick install -> *Media* -> **Media player (ffmpeg)**, or IYAKKI menu ->
   Media player -> **Install the media player**, or the CLI tile (`ffmpeg`). It downloads a static build from
   johnvansickle.com, checks it against the MD5 file the builder publishes, unpacks it into the data folder and links `ffmpeg`
   and `ffprobe` (no sudo, nothing written outside the data folder). About 42 MB to download on x86-64 (19 MB on ARM64) and
   about 150 MB on disk. Only an administrator can install; it is a background job you can follow beside the clock. Installing
   again updates it ("Update Media player").
2. **Play.** MEDIA -> Videos (or any file in the library, the camera's `video-*.avi` recordings included), or IYAKKI menu ->
   Media player -> type a path -> Play. MP3 still plays inside IYAKKI without ffmpeg; every other video and sound format
   (MP4, MKV, WebM, AVI, FLAC, OGG, Opus, WAV, M4A) goes to the player.

| Key / mouse | Does |
|---|---|
| Space, `p`, Enter, click the picture | pause and play (plays again after the end) |
| Left, Right | back and forward 5 seconds |
| Down, Up | back and forward 30 seconds |
| Home, `0` | from the start |
| `f` or the Fullscreen button | the page area without the bars; the controls come back when the mouse moves or a key is pressed |
| click the bar | seek to that place |
| Esc (in fullscreen), then Esc or `s` | leave fullscreen, then stop |

The top bar shows the title and an info line (`h264 1280x720 25 fps . aac 48000 Hz stereo . 1:23:45`); the bottom bar the
position and length. Subtitles are not drawn.

## How it works

`crates/iyakki-agent/src/player.rs` (the logic, tested with a fake ffmpeg) and `crates/iyakki-shell/src/apps/mediaplay.rs`
(the view). Two `ffmpeg` processes per playback, started with fixed arguments and never through a shell (`BOUNDARY.md`): one
writes raw RGBA frames scaled to the player area, the other 48 kHz stereo PCM, which goes to `aplay -D iyakki`, the softvol
device, so the volume and mute of Settings -> Sound apply. **The audio is the clock**: the position is the PCM written to the
speaker less a fixed lag (0.45 s) for what the pipe and the card still hold, and each frame waits for its time; frames far
behind are dropped. Without sound (or without `aplay`) the wall clock is used. Pause holds the clock and stops feeding both;
seeking, and a new size (fullscreen, a resized window), start the decoders again at `-ss`.

## Other ways to get ffmpeg

* **In the image (optional, off by default).** `WITH_FFMPEG=1 ./build.sh mac` (any variant) adds
  `dist/buildroot/configs/iyakki_media.fragment`: Buildroot's ffmpeg and ffprobe as an **LGPL** build (no GPL, no nonfree, no
  ffplay, no SDL) with only the decoders, demuxers and filters the player uses (H.264, HEVC, VP8, VP9, AV1 via dav1d, MPEG-4,
  MJPEG, AAC, MP3, Opus, Vorbis, FLAC). Estimated cost: about 5 to 8 MB in the root filesystem (the download in Quick install
  is a 150 MB static build with every codec); not measured, no image was built for this. H.264/HEVC/AAC/MP3 follow the
  `WEB_H264` policy: the default images do not carry them, so this is a choice by whoever builds the image
  (`docs/FULL-BROWSER.md`, "Codec policy").
* **A copy on the machine.** An `ffmpeg` on the `PATH` (or in `/usr/bin`) is found and used.

## ffplay on the web images

Not added. The `web` and `macweb` images run WPE WebKit with a private offscreen Wayland display and **no compositor** (no
cage), so SDL2's Wayland backend would have nothing to connect to; SDL2's KMSDRM backend would need the display for itself
while the shell draws on it. The web engine plays video there already (MEDIA's embedded engine), and the in-house player works
there too once ffmpeg is installed or built in. Nothing in the web fragment was changed.

## Limits

No subtitles; audio-only files show moving level bars (not a real analysis); the A/V lag is a constant (tune `audio_lag` in
`player.rs` if lips and voice drift on a particular sound card); software decoding only, so 1080p H.264 on a slow machine may
drop frames. Seeking restarts the decoders, so it takes a moment on long files.
