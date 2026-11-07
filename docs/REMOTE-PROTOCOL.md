# The browser front end's protocol (iyakki-remote)

`iyakki-remote` shows a person's own shell in a web browser. The page (`crates/iyakki-remote/src/page.rs`) is served at `/`; after
sign-in it opens a WebSocket at `/ws?csrf=TOKEN` (same origin, session cookie, one window per session). Everything below is JSON
in WebSocket **text** frames unless said otherwise. A page message the server does not understand is ignored, never trusted.

## Versions and negotiation

* **Version 1** (the original): no handshake. The server sends the changed rectangle as one PNG in a binary frame (see "Version 1
  pictures" below) whenever the screen changes, with a simple "newest wins" hand-over to the connection. It is still what a page
  gets that never says `hello`: an old cached page, a script, a test. It is the "Lossless / raw" level, not adaptive.
* **Version 2** (this document): the page sends `{"t":"hello","v":2,"caps":{"deflate":true}}` right after the socket opens
  (`deflate` says the browser can inflate raw deflate itself: `DecompressionStream("deflate-raw")`). The server answers
  `{"t":"hello_ok","v":2,"tile":64,"levels":[...],"max_message":524288}` and from then on sends **version 2 frames** (below)
  instead of PNG pieces. A `hello` with a higher `v` is answered with the server's own (2); a `hello` with `v` below 2 is
  answered `v:1` and the pictures stay version 1. A page that gets no `hello_ok` (an older server) keeps drawing version 1
  pictures: the shipped page does exactly that, so a new page works against an old server and the reverse.
* The WebSocket handshake never offers extensions: **`permessage-deflate` is not negotiated**, even if the browser asks, so tiles
  (already deflated) are never compressed a second time and no CPU is spent on it. A test checks the handshake.
* Small server messages produced in one pass of the server loop (cursor, speech, clipboard, statistics, pongs) are written in one
  `write`, so they travel in one packet.

## Page to server

| `t` | fields | meaning |
|---|---|---|
| `size` | `w`, `h` | window size (clamped to 640x400 .. 3840x2160) |
| `move` `down` `right` `up` | `x`, `y` | mouse, in canvas pixels |
| `wheel` | `d` | scroll steps (-20..20) |
| `key` | `key`, `ctrl`, `shift` | a key press |
| `shift` | `on` | shift held at the next click |
| `paste` | `s` | text from the clipboard (at most 200,000 characters) |
| `full` | | send the whole picture again |
| `media` | `kind` (`mic`, `camera`, `clipread`), `on` | the person switched a browser device on or off (only from a real click or key in the page; the server answers `media_denied` when the policy forbids it) |
| `audio` | `on` | the page started or stopped playing the session sound (WebAudio, binary `0x10` messages) |
| `sink` | `mode` (`browser`, `both`, `mac`) | "Play sound on": **browser** = this computer, and the machine's own speakers are kept silent for as long as this page's sound is on; **both**; **mac** = the Mac only, nothing is sent to the page. The page sends it just before `audio` on and whenever the person changes it. Kept for the session only. |
| `sp_voices` | `ok`, `voices: [{n, l, d}]` | the page's speech synthesis: `ok` false when it has none; each voice has name `n`, language `l`, `d` true when it is on the device. Sent when the socket opens, when asked (`speak_voices`) and on the browser's `voiceschanged`. At most 400 voices, names cleaned and cut. |
| `hello` | `v`, `caps: {deflate}` | version 2 handshake (see above); sent once, first |
| `ack` | `n` | the picture message with sequence `n` is drawn (sent after every message; the server never has more than 2 unacknowledged) |
| `quality` | `level`, `saver` | `level` is `auto`, `lossless`, `balanced`, `low` or `verylow`; `saver` is the data saver (caps the level at `low`) |
| `ping` | `ts`, `rtt` | round-trip probe every 2 s; `rtt` is the last round trip the page measured (ms) |
| `sp_start` | `id` | the browser started saying utterance `id` (`onstart`) |
| `sp_end` | `id` | it finished (`onend`) |
| `sp_error` | `id`, `e` | it failed (`onerror`, `e` is the error name such as `not-allowed`, `synthesis-failed`; `interrupted` and `canceled` are not reported because the server asked for them) |
| `sp_boundary` | `id`, `c` | a word was reached: character position (`onboundary`) |

## Server to page

Binary frames are pictures. **Version 1**: four little-endian u16 (x, y, width, height) then a PNG. **Version 2**: a frame message (below). Text frames:

| `t` | fields | meaning |
|---|---|---|
| `cursor` | `c` | CSS cursor name |
| `copy` | `s` | text for the clipboard |
| `readclip` | | read the clipboard and send it back as `paste` |
| `bye` | | signed out |
| `hello_ok` | `v`, `tile`, `levels`, `max_message` | answer to `hello` |
| `stats` | `level`, `auto`, `saver`, `bps`, `rtt`, `sent_bps`, `fps` | about once a second and on every level change: the level in force, the measured link speed (bytes/s), round trip (ms), what the server sent last second |
| `pong` | `ts` | answer to `ping` (at most 20 a second) |
| `speak` | `id`, `text`, `voice`, `lang`, `rate` | **browser voice**: say `text` as utterance `id`. `voice` is a voice name from `sp_voices` or null, `lang` a BCP 47 tag such as `ta-IN` (the browser picks a voice for it when `voice` is null), `rate` 0.5..2 (1 is normal). The page cancels whatever is being said first (a new piece replaces the old), waits 40 ms and calls `speechSynthesis.speak`. Text is at most 1,500 characters. |
| `speak_cancel` | `id` or null | cancel utterance `id` if it is the one being said, or whatever is being said when `id` is null. The id matters: a late cancel of an old piece never stops the next one. |
| `speak_pause`, `speak_resume` | | `speechSynthesis.pause()` and `resume()` (the reader does not use them: it cancels and says the piece again) |
| `speak_voices` | | send `sp_voices` again |

The same speech messages are used by the **voice helper** (`docs/VOICE.md`): a small page the desktop window opens in the person's browser, served on `127.0.0.1` by `iyakki-agent::voicehelper` with its own handshake (one-time link token, session secret, `Host` and `Origin` checks) and the same page code (`speech.js`); it adds one message, `{"t":"hb"}` every five seconds from the page, so the server can tell a silent tab from a closed one.

Connecting asks the page for its voices (`speak_voices`); until it answers, the browser voice is "checking". When the socket
closes, the server treats the browser voice as gone and fails what was being said. Speech messages travel on a channel of their
own on the server so a slow page never loses one (pictures may be skipped, speech may not).

Only the text of the story pieces being read is ever sent in `speak`.

Utterance timing: the server reports a piece as failed if `sp_start` has not arrived within 8 seconds (browsers that need a click
inside the page first), and with the page's error name when `sp_error` arrives.

## Where dictation would fit (not built)

The same page could use `SpeechRecognition` (`webkitSpeechRecognition` in Chrome) for dictation into the shell's text fields:
a server message `listen` (`lang`) starting recognition, page messages `sp_text` (`final`, `text`) with the transcript and `sp_listen_end`,
and a `sp_voices`-style capability flag (`recognition: true`). Chrome sends the audio to Google's servers for it, so it would need an
explicit, visible switch and a note in Settings → Voice. Nothing of this exists yet.

## Version 2 pictures

The screen is cut into 64x64 tiles (smaller at the right and bottom edges). The server keeps what the page has, per tile, and each
message holds only the tiles that differ from it, so an unchanged screen sends **nothing**. The picture is taken only when the
connection asks for one (pull, see "Flow control"): pictures made while the page is busy are never queued, the next message is
built from the newest picture. A big update is split by a byte budget (about a quarter second of the measured link, 16 KiB to
1 MiB) so a key press is never stuck behind a whole screen.

Frame message (binary WebSocket frame, little-endian):

| bytes | meaning |
|---|---|
| 0 | kind, 2 |
| 1 | flags: bit 0 = the body is raw deflate (`DecompressionStream("deflate-raw")`) |
| 2..6 | sequence number (acknowledge it with `ack`) |
| 6..10 | screen width, height (u16 each) |
| 10 | scale: 1, or 2 when the picture was halved (the page stretches it back) |
| 11 | level: 0 lossless, 1 balanced, 2 low, 3 very low |
| 12..14 | tile count |
| 14..18 | body length before deflate (at most 48 MiB; the decoder refuses more) |
| 18.. | body: the tiles, deflated together when that is smaller |

Tile in the body: `x y` (u16, in the scaled picture), `w h` (u8), type (u8), then:

| type | content |
|---|---|
| 0 solid | R G B |
| 1 raw | `h` rows of: filter byte (0 none, 1 left, 2 up, 3 Paeth), then `w*3` filtered bytes (PNG's filters) |
| 2 palette | colours-1 (u8), the colours (RGB), then rows of indexes at 1, 2, 4 or 8 bits each (rows start on a byte) |
| 3 PNG | u32 length and a PNG (only for pages that cannot inflate) |
| 4 YC | lossy: luma plane `w x h`, then two chroma planes at half size (green-minus-average, red-minus-blue, centred on 128), each as filtered rows; the luma and chroma are quantised, so they deflate well |

A tile is coded by what is in it, cheaply: one colour is solid; up to 64 colours is a palette (text, icons, flat user interface);
more is raw filtered, and only above 192 colours (a photograph, video, a smooth wallpaper) may it be coded lossily.
**Text and interface tiles are always exact at every level.**

## Levels

| level | when | lossy tiles | picture size | pictures per second (at most) | exact refresh of lossy tiles after |
|---|---|---|---|---|---|
| Lossless | fast link | never | full | 60 | n/a |
| Balanced | | photo tiles with 32 values per channel (error at most 4) | full | 30 | 250 ms still |
| Low bandwidth | | photo tiles as YC: luma step 4, chroma step 16 at half resolution | full | 10 | 400 ms still |
| Very low | | as Low with luma step 12, chroma step 32 | **halved** (the page stretches it) | 5 | 600 ms still (at the halved size) |

A key, click or scroll lets the next picture go after 8 ms whatever the level ("key echo matters more than frame rate").
The pointer shape is sent separately (`cursor`) and the browser draws its own pointer, so it moves without waiting for the server.

## Flow control and automatic quality (`crates/iyakki-remote/src/adapt.rs`)

* At most **2** picture messages are unacknowledged. While the window is full nothing is asked of the screen and nothing is
  queued; the server never holds more than the newest picture.
* The page sends `ack` after drawing each message. The time from sending to `ack` gives the round trip of the pictures; bytes
  over the time the link was busy give the throughput (smoothed). The page's own `ping`/`pong` gives the true round trip.
* Automatic level: speed at least 2 MB/s is Lossless, at least 375 kB/s Balanced, at least 100 kB/s Low, otherwise Very low; a
  round trip over 250 ms caps at Balanced, over 600 ms at Low. It goes **down at once** and **up one step after 4 s** of better
  readings. A message unacknowledged for 2 s steps one level down.
* The person can fix a level, and turn on the **data saver** (the level is never better than Low bandwidth) in the page's
  display settings (the small tab at the right edge, or Ctrl+Alt+Q). The choice is kept in the browser.
* Ctrl+Alt+S shows statistics: level, bytes per second received, pictures per second, round trip, decode time, link speed.

## The page

`/decoder.js` (served to everyone, it is code only) decodes frames. Where the browser has `Worker`, `OffscreenCanvas`,
`transferControlToOffscreen` and `DecompressionStream`, decoding and drawing run in a worker; otherwise on the main thread; a
browser without `DecompressionStream` says so in `hello` and gets PNG tiles. Touch: tap is a click, a long press (0.55 s) a right
click, one finger drags, two fingers scroll (pinch zooms the view up to 4x; while zoomed two fingers pan); a keyboard button opens the
phone's keyboard; a full screen button; on a window narrower than the 640 pixel minimum desktop width the picture is scaled to fit.
Clipboard: the page already bridges it (`copy`, `readclip`, `paste`).

## Browser sound, camera and microphone

Browser devices panel (gear, or Ctrl+Alt+Q): Start sound, Play sound on, Microphone, Camera, Clipboard reading. Everything is off until the
person clicks it (a real click or key; a script cannot), and each failure is shown in a red line at the top of the page in exact words
(blocked by you or the browser, blocked by the operating system, no camera found, camera in use by another program, page not in a
secure context, page policy).

* **Where the sound plays.** The mixer's output (radio, MEDIA, TALES, system sounds, speech) is copied to the page. "This computer (the
  browser)" is the default once sound is on: the machine's own output is fed silence while the controlling page plays. "Both" plays on
  both. "The Mac only" sends nothing to the page. The controlling session is the live page whose sound was switched on last; only its
  choice can silence the machine, other sessions are never touched. The silence ends at once when that page switches its sound off,
  disconnects, signs out, its shell ends, or just stops answering for 4 seconds (so the machine can never stay muted). A program that
  started on the play command (mixer off in Settings) before the page's sound began keeps playing on the machine until it ends.
* **Camera.** The page sends JPEG frames (`0x02`) after the person clicks Camera and allows it in the browser prompt. In a browser
  session the CAMERA tile is usable even on a machine without a camera (a Mac mini has none), offers "Browser camera", and asks the page
  to show its Allow button when it is off; the assistant's camera tools use the browser camera first when a page offers one (and still ask
  the person each time). Needs a secure context: the https:// address (accept the certificate warning once) or localhost.
* **Page headers.** The page is sent with `Permissions-Policy: camera=(self), microphone=(self), autoplay=(self), clipboard-read=(self), clipboard-write=(self), fullscreen=(self)`.
  (Without the header browsers allow these for the page too, so the header documents the intent rather than fixing a block.)
* **Policy.** `IYAKKI_BROWSER_MEDIA=allow|deny` (unset on a desktop = allow, in a pod always set) and the person's role (`iyakki-sec::media`).

## Security of what the page exposes

Unchanged by version 2, and documented here because the port is network facing: every route except `/`, `/decoder.js`, `/healthz`
and `/login/..` (the local-network sign-in relay) needs the session; sign-in checks the machine's accounts (which have their own
lockout) and is limited to 10 tries a minute per address; sessions are a random cookie (`HttpOnly`, `SameSite=Strict`, `Secure` over
TLS) plus a CSRF token that the WebSocket and sign-out must repeat; the origin must match; one window per session; at most 32
sessions and 128 connections. TLS is used when the server is given an identity (a made certificate or the machine's own); over plain
HTTP the password travels in the clear, so use it on a trusted network or with TLS. Messages from the page are at most 512 KiB, parsed
strictly, and unknown ones are ignored; the page cannot make the server allocate by sending sizes (frame sizes are produced by the
server). The pages carry a strict Content-Security-Policy. Memory per viewer at 4K is about 100 MB (newest picture, what the page has,
the halved copy).

## Measurements

Reproduce: `IYAKKI_MEASURE_OUT=/tmp/m.md cargo test --release -p iyakki-remote --test measure -- --ignored --nocapture`
(`crates/iyakki-remote/tests/measure.rs`). Frames are the real shell (home screen right after sign-in then idle; the CHAT app with 48
keys typed at 8 a second), a synthetic video region (640x360 moving picture with camera grain over the home screen, 60 frames at 30 a
second) and scrolling text drawn with the shell's own text engine (3 pixels a frame at 30 a second). *Before* is the version 1 way: one
PNG of the bounding rectangle of what changed. *After* is version 2 at each level with every frame sent (no rate limit, so these are
the bytes a link would need at the scenario's own frame rate; the real server also drops frames when the link is slow). *ms/frame* is
encoder CPU time on one core of the development machine (release build), per frame of the scenario. *Extra bytes once still* is the
exact refresh of lossy tiles after the picture stops. Home screen: 12 frames, the first is the whole screen and the rest are
unchanged (nothing is sent for them in either way), so *bytes/s* there is the one full frame spread over 6 s.

| size | scenario | way | msgs | bytes/frame | bytes/s | ms/frame | extra bytes once still |
|---|---|---|---|---|---|---|---|
| 1280x720 | home screen | **before**: PNG of changed rectangle | 1 | 24,228 | 48,456 | 0.4 | - |
| 1280x720 | home screen | after: Lossless | 1 | 10,532 | 21,063 | 1.4 | +0 |
| 1280x720 | home screen | after: Balanced | 2 | 10,883 | 21,767 | 1.5 | +0 |
| 1280x720 | home screen | after: Low bandwidth | 2 | 10,358 | 20,716 | 1.6 | +0 |
| 1280x720 | home screen | after: Very low | 2 | 5,169 | 10,338 | 2.4 | +0 |
| 1280x720 | chat typing | **before**: PNG of changed rectangle | 49 | 4,607 | 36,854 | 0.3 | - |
| 1280x720 | chat typing | after: Lossless | 49 | 2,895 | 23,156 | 1.1 | +0 |
| 1280x720 | chat typing | after: Balanced | 49 | 3,116 | 24,929 | 1.2 | +0 |
| 1280x720 | chat typing | after: Low bandwidth | 49 | 2,939 | 23,508 | 1.2 | +0 |
| 1280x720 | chat typing | after: Very low | 49 | 1,707 | 13,657 | 2.0 | +0 |
| 1280x720 | video-like | **before**: PNG of changed rectangle | 60 | 412,880 | 12,386,412 | 2.5 | - |
| 1280x720 | video-like | after: Lossless | 60 | 264,301 | 7,929,028 | 25.3 | +0 |
| 1280x720 | video-like | after: Balanced | 60 | 163,449 | 4,903,466 | 23.8 | +271448 |
| 1280x720 | video-like | after: Low bandwidth | 60 | 88,722 | 2,661,654 | 18.5 | +263582 |
| 1280x720 | video-like | after: Very low | 60 | 13,902 | 417,068 | 7.6 | +75124 |
| 1280x720 | scrolling text | **before**: PNG of changed rectangle | 60 | 912,218 | 27,366,542 | 4.0 | - |
| 1280x720 | scrolling text | after: Lossless | 60 | 123,976 | 3,719,294 | 8.8 | +0 |
| 1280x720 | scrolling text | after: Balanced | 60 | 121,973 | 3,659,196 | 9.0 | +0 |
| 1280x720 | scrolling text | after: Low bandwidth | 60 | 121,184 | 3,635,517 | 10.2 | +0 |
| 1280x720 | scrolling text | after: Very low | 60 | 32,557 | 976,696 | 4.9 | +2290 |
| 1920x1080 | home screen | **before**: PNG of changed rectangle | 1 | 34,600 | 69,199 | 1.2 | - |
| 1920x1080 | home screen | after: Lossless | 1 | 15,665 | 31,329 | 3.1 | +0 |
| 1920x1080 | home screen | after: Balanced | 2 | 15,515 | 31,030 | 3.0 | +0 |
| 1920x1080 | home screen | after: Low bandwidth | 2 | 14,751 | 29,501 | 3.4 | +0 |
| 1920x1080 | home screen | after: Very low | 2 | 7,841 | 15,682 | 5.2 | +0 |
| 1920x1080 | chat typing | **before**: PNG of changed rectangle | 49 | 6,715 | 53,722 | 0.7 | - |
| 1920x1080 | chat typing | after: Lossless | 49 | 4,595 | 36,758 | 3.0 | +0 |
| 1920x1080 | chat typing | after: Balanced | 49 | 4,661 | 37,284 | 3.1 | +0 |
| 1920x1080 | chat typing | after: Low bandwidth | 49 | 4,309 | 34,469 | 3.1 | +0 |
| 1920x1080 | chat typing | after: Very low | 49 | 2,482 | 19,854 | 4.3 | +0 |
| 1920x1080 | video-like | **before**: PNG of changed rectangle | 60 | 415,127 | 12,453,820 | 3.4 | - |
| 1920x1080 | video-like | after: Lossless | 60 | 256,311 | 7,689,335 | 26.4 | +0 |
| 1920x1080 | video-like | after: Balanced | 60 | 157,462 | 4,723,850 | 25.2 | +262233 |
| 1920x1080 | video-like | after: Low bandwidth | 60 | 84,894 | 2,546,812 | 19.8 | +254695 |
| 1920x1080 | video-like | after: Very low | 60 | 10,469 | 314,072 | 9.0 | +63278 |
| 1920x1080 | scrolling text | **before**: PNG of changed rectangle | 60 | 1,377,131 | 41,313,919 | 7.7 | - |
| 1920x1080 | scrolling text | after: Lossless | 60 | 181,282 | 5,438,449 | 14.3 | +0 |
| 1920x1080 | scrolling text | after: Balanced | 60 | 178,338 | 5,350,142 | 15.1 | +0 |
| 1920x1080 | scrolling text | after: Low bandwidth | 60 | 177,206 | 5,316,182 | 16.9 | +0 |
| 1920x1080 | scrolling text | after: Very low | 60 | 48,351 | 1,450,532 | 9.1 | +2290 |

What the numbers say, honestly:

* Typing and the home screen: 30 to 40 percent fewer bytes than the PNG rectangle at full quality (about 3.5 kB a keystroke at 720p),
  more at Very low (half size). Neither is a bandwidth problem on its own; the gain there is mostly that idle costs nothing.
* Scrolling text: About 7 times fewer bytes than before at the same exact quality (tiles that did not change are not resent, and
  palette tiles compress far better than a rectangle PNG), but still several MB/s because anti-aliased text moving 3 pixels changes
  almost every tile. A "copy the old pixels shifted" tile (scroll detection) is not built; it would cut this by an order of magnitude.
* Video-like motion: lossless is 35 to 40 percent smaller than the PNG rectangle (it only sends the video tiles), Balanced about 60
  percent smaller, Low bandwidth about 80 percent smaller and Very low about 97 percent smaller, with the video region blurred while
  it moves and made exact (at the level's size) about 0.25 to 0.6 s after it stops.
* CPU: the encoder costs more than the old PNG call (1 to 26 ms a frame here against 0.3 to 7.5), which is the price of the better
  coding. The video case at about 25 ms a frame means one core is about three quarters busy at 30 frames a second at Lossless;
  the lower levels cost less. It runs on the connection's own thread, not the shell's.
* Not measured: a real browser's decode time, a real slow link, the worker.

## Verified

`crates/iyakki-remote/tests/v2.rs` (version 2 negotiation, tiles, backpressure, levels, hostile input), `src/codec_tests.rs` and `src/adapt.rs` tests (codec and controller), `tests/decoder_js.rs` (the page's decoder under node) and `tests/server.rs` (speech messages reach a page as text frames; the page's events arrive as input; the page
has the speech code) and `tests/chrome_speech.rs` (ignored: the page's real speech code in a real headless Chrome through the
DevTools protocol).
