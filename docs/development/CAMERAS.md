# CAMERAS: live view and recording of USB and network cameras

Feature rows: F-CAM-02 (the tile), F-CAM-15 (the RTSP and HTTP Motion-JPEG client), F-CAM-16 (cameras, vault and access), F-CAM-17
(the assistant's tools), F-CAM-03 (recording, see "Recording" below). Detection is F-CAM-04 and is not built; nothing here looks at
what is in a picture.

## What a person gets

* The **CAMERAS** tile (Web & Media group, everybody but guests): a grid of 1, 2, 4 or 6 views (keys `1 2 4 6`), each with the
  camera's name and state, **click or Enter to enlarge**, **s** or the Snapshot button for one picture saved in the person's own
  Pictures (`camera-<name>-<date>-<time>.jpg`, never overwriting), `a` to add a network camera, `e` to edit, `d` to remove (asks).
* States, in words on the view: Live, Connecting, Lost (with the reason), In use (a USB camera another app holds), Needs password
  (the owner's vault is locked), **Needs video add-on** (an H.264 camera).
* This computer's own USB cameras appear by themselves (found through the shared camera module, `crates/iyakki-shell/src/camera`).
  While a picture of one is on the screen a red band says so, always: "This computer's camera is on and shown in this window.
  Nothing is recorded." The camera is opened only while it is shown and released a few seconds after the person leaves. This
  computer's own camera is never recorded (recording is for network cameras).
* The CAMERA tile (photos, video, passport photos) is unchanged. If it holds the camera, the CAMERAS view says "In use".

## Cameras, secrets and who may watch

A saved network camera is one record in `<data>/cameras/cameras.json` (mode 0600): name, address **without** a user name or
password, user name, owner, the people allowed, and the *name* of a vault entry. The password is in the **owner's vault**
(`cam.<id>`). An address typed or pasted with `user:password@` is split on save: the user goes to the record, the password to the
vault, and the address kept and shown is clean. Audit lines, errors and displays pass through `iyakki_rtsp::url::scrub`.

Access (`iyakki_agent::cameras`, checked on every call, in the tile and in the tools alike):

| | watch | change, share, remove | password |
|---|---|---|---|
| owner | yes | yes | yes |
| person on the list | yes | no | no |
| administrator (not on the list) | **no** | yes | no (only the owner) |
| anybody else | does not see it | no | no |

USB cameras of this computer follow the account's CAMERA app right. The stream signs in with the owner's password, so while the
owner's vault is locked a camera shared with others shows "Needs password" (the stream does not start). Viewing is audited
(`camera.view`, at most once a minute per person and camera), as are `camera.add/edit/remove/snapshot`.

## The streams

`Hub::view` is what the tile calls for each camera on the screen; it starts the stream when none runs and keeps it for 8 s after
the last call. So only what is on screen is streamed, and a stream is dropped (connection closed, USB device released) when nobody
looks. Pictures are decoded at most 5 times a second for a grid cell (480 px wide) and 10 for an enlarged view (1280 px wide),
whatever the camera sends. A lost stream is retried after 1 s, doubling to 30 s; a good picture resets the pause.

## The RTSP client (`crates/iyakki-rtsp`, no dependencies)

* `url`: `rtsp://` and `http://` addresses; `https://` and `rtsps://` are refused in words (no TLS client for cameras yet).
* `msg`: RTSP responses, server requests and interleaved `$` frames from a byte stream, with hard limits (header 16 KiB, 100 headers,
  body 64 KiB); a read timeout in the middle loses nothing.
* `auth`: Basic and Digest (MD5, `qop=auth`), checked against the RFC 2617 example; SHA-256 digest is refused in words.
* `sdp`: media sections, `rtpmap`, `fmtp` (H.264 parameter sets), `control`.
* `client`: `DESCRIBE`, `SETUP` over TCP (interleaved), `PLAY`, keep-alives (`OPTIONS` at half the session timeout), the media loop.
  UDP transport and redirects are not supported.
* `rtp`: RTP header parsing; **Motion-JPEG (RFC 2435)** complete for types 0 and 1 (4:2:2 and 4:2:0), restart markers, Q 1-99 and Q
  128-255 with tables in the packet or cached from an earlier frame; frames come out as complete JPEG files and decode with the
  repo's own decoder (`iyakki_web::jpeg`). **H.264 (RFC 6184)**: single NAL units, STAP-A, FU-A into access units and Annex-B bytes.
  Gaps drop the damaged picture, never the stream; frames over 8 MiB are dropped.
* `httpmjpeg`: `multipart/x-mixed-replace` with or without part lengths and with chunked transfer encoding (found by pointing it at a real ffmpeg server), and single-picture URLs (fetched again for each picture,
  at most twice a second), Basic and Digest.
* `fakeserver`, `packetize`: a scripted RTSP / HTTP camera on the loopback address for tests and demos.

### H.264 and the video add-on

H.264 needs a codec and this system ships none. The client reads the packets, the hub notices the codec when the stream opens and
the camera shows **Needs video add-on**; the text says to use the camera's Motion-JPEG stream or snapshot address meanwhile.
The route for the real thing, not built yet: the person downloads the audio and video add-on themselves (Store, Setup; the
vendor's build, never shipped, see `docs/FULL-BROWSER.md` and `docs/development/AV-ADDON-K3S-BUILD.md`), and an
`H264Support` implementation hands `AccessUnit::annex_b()` to the add-on's decoder in its own isolated process and reads RGBA
pictures back. `Hub` already takes any `H264Support`; the unit tests prove the route with a fake decoder.

## The assistant (`camerastools.rs`)

`cameras_list` (free; names and errors are marked untrusted), `camera_snapshot` (asks every time, no standing grant; a USB camera
shows the existing red "in use" sign while it works), `cameras_add` (asks; an address with a user name or password and a
`password` that is not `true` are refused; `password: true` opens the same masked form as `vault_request`, the person types it,
it goes to their vault and the assistant learns only "stored"), `cameras_remove` (asks every time), `cameras_access` (asks).

## Recording (F-CAM-03)

Code: `crates/iyakki-agent/src/camrec.rs` (the recorder), `camrec_format.rs` (the file format), `camerastools.rs` (the chat tools),
`crates/iyakki-tile-cameras/src/rec.rs` (the recordings page), `crates/iyakki-shell/src/apps/cameras.rs` (wiring, the red sign).

### The rules

* **Off by default, per camera, opt-in.** Nothing records until the camera's **owner** turns it on, from the recordings page (key
  `p` on a camera, then `r`), from its settings (`c`) or by chat (`camera_recording_set`). Turning it on always shows a
  confirmation that says what is stored, where, for how long, that a red REC mark and a red sign will show, that one click stops
  it, and what losing the vault means; in chat the same text is the approval card and it is a hard ask (Auto mode does not skip it).
  No key is made and the camera is not contacted before that.
* **Visible while it runs.** A red **REC** pill sits on the camera's picture in the grid, the enlarged view and the recordings page
  for as long as the camera's recorder runs (also while it waits for the camera or the vault). The shell draws a red sign "REC:
  Front door. Click to stop." bottom left of every screen while a person is signed in and a recorder runs; the click switches off
  every recording of that person's cameras (`camrec::stop_all_for`). Turning off is one key (`r`) or one click and asks nothing.
* **Who.** Only the owner turns recording on, plays it back, exports it, lists it or reads the log. An administrator may turn it
  OFF and change limits, but is never given a picture. A person the camera is shared with sees *that* it records (the REC mark,
  `camera_recording_status`), nothing more. A refusal of any of these is written to the camera's log; a stranger does not learn the
  camera exists. This computer's own USB camera is never recorded.
* **Modes.** `continuous`, or `schedule` with weekly windows written `mon-fri 08:00-18:00`, `daily 22:00-06:00` (past midnight is
  allowed: the window belongs to the day it starts), `sat,sun 00:00-24:00`, `weekdays`, `weekends`. The flow scheduler's cron lines are
  single instants, not windows, so recording has this small model (`camrec::Window`). Local time is the computer's zone.
* **Resumes by itself.** Recording that is on starts again when the shell starts and the owner signs in (the key is in the owner's
  vault), and windows open and close without anyone looking. When the vault closes (sign-out) the recorder closes its file and
  waits ("the recording key cannot be read from the owner's vault"), within about ten seconds; it never keeps recording with a key the
  vault no longer gives.

### What is stored, and where

`<data>/cameras/recordings/<camera id>/YYYY/MM/DD/HHMMSS.irec`, one file per 60 s (or 64 MiB), local-time folders, mode 0600, never
overwriting. Next to them: `index.txt` (a cache), `audit.log`, and `recordings/settings.json` (modes, windows, limits; no secrets).

* **No re-encoding.** The recorder opens its **own** connection (the live view is not disturbed; a camera that only accepts one
  connection will show "lost" in the recorder while somebody watches, and the page says so). Motion-JPEG frames are stored as the
  complete JPEG files the client assembled; HTTP Motion-JPEG and snapshot addresses (at most 2 pictures a second) the same way.
  H.264 access units are stored in Annex-B form, a new file starting at a key frame, with the parameter sets (from the SDP)
  as the first record of every file. **Playing H.264 back needs the video add-on**; exporting it does not (one `.h264` file in
  Videos that a player of the person's own opens). Sound is not recorded.
* **Segment format** (`camrec_format.rs`, documented at the top of the file): a header (magic, kind, start time, 16-byte nonce, MAC)
  and records `cipher_len, time stamp, flags, cipher, MAC`. Time stamps and sizes are readable on purpose (the timeline and the
  index need no key, and an index can be rebuilt from the files alone); the pictures are not.

### Encryption (decision)

Segments are encrypted with a **per-camera 256-bit key held in the owner's vault** (`camrec.<camera id>`, stored as hex through the
same vault route as the camera's password), using the **primitives of `iyakki-volcrypt`**: AES-256-XTS (`xts::Xts`) and
HMAC-SHA256 with a constant-time compare (`mac`). The volume *container* of that crate was **not** used: it is a whole-disk sector
store with key slots, while recordings are many small append-only files that must survive a power cut and be deleted one by one.
So: a fresh random nonce per file; the XTS key pair and the MAC key come from HMAC-SHA256 of the camera key and the nonce (a file
never shares a key stream with another); each record is XTS-encrypted with its record number as the tweak and then MACed
(encrypt-then-MAC, the MAC covers the record number, the header bytes and the cipher text). A wrong key is told from damage by the
header MAC; a flipped byte, a swapped or moved record is found when that picture is read and is never shown.
No new dependency (`iyakki-volcrypt` is a workspace crate; its `aes`/`sha2` were already in the tree).

**Not detected:** pictures removed from the *end* of a file, or whole files deleted (a power cut looks the same). Someone with the
disk also sees when pictures arrived and how large they are (that hints at motion), the camera's id and the folder dates.
**Losing the vault key makes the recordings unreadable, for everybody, and nothing can bring them back**; the page, the
confirmation and the assistant's text all say so (`camrec::KEY_NOTE`). Keep the vault's recovery key (F-STORE-05 when it
is built). Playback reads the key from the vault on every picture, so a locked vault stops playback at once.

### Retention and the disk

Checked at start and every 5 minutes (`camrec::Recorder::enforce`), oldest first, each pass written to the camera's log and the
general audit log:

1. by age: older than `max_days` (default 14, 0 = no limit);
2. by size per camera: more than `max_mb` (default 20 GB, 0 = none);
3. by size in total: more than `total_max_mb` (default 100 GB; administrators set it).

**Free-space floor** (default 5000 MB free, never below 256 MB, administrators set it): below it, a recorder stops with a visible
message ("the disk has only 100 MB free, below the floor of 5000 MB; recording stopped so the disk does not fill up") instead of
writing, tries a retention pass first, is written to the log, and starts again by itself a minute later once there is 10 % more
room than the floor. The floor never deletes recordings that are within their limits; only the three limits above delete.
A removed camera stops recording; its files stay and are cleaned by the same limits (and cannot be opened from the tile).

### Playback and export (the recordings page)

`p` opens it for a network camera. It shows the state in words, the limits, the vault-key warning, the picture, a **timeline of the
day** (recorded stretches in colour, click anywhere to jump; hour marks), **day picker** (`[` older, `]` newer), **play/pause**
(Space), **step** (`,` / `.` or arrows; stepping crosses files and days), **export** (`e`), **log** (`l`), **settings** (`c`).
Playback shows the pictures at the speed they were stored; a file is only listed after it is closed, so the newest minute appears a
minute later. **Export** copies the pictures of a clip (1, 5, 15 or 60 minutes from the picture on the screen; `m` changes it)
decrypted into the person's `Pictures/camera-<name>-<date>-<time>/frame-000001.jpg ...` (folder 0700, files 0600) after a confirmation
that says the copies are not encrypted; at most 20 000 pictures or 2 GB per export (it says when it stops), refused when the
destination has less than 512 MB free or the account's file rules forbid the folder. The API (`Recorder::export`) refuses unless the
caller passes `confirmed = true`, which the tile and the chat approval card do only after the person said yes.

### The log of who looked at what

`<camera>/audit.log` (JSON lines, 0600, rolls over at 1 MiB, one old file kept), shown to the owner (`l`; `Recorder::audit`):
`live.view` (a person's live view of the camera, at most one line per person a minute), `live.snapshot` (including refusals),
`playback.start`, `playback.error` (a picture that failed its check, a missing key), `export` (range, count, folder, or why not),
`recording.start/stop/setting` (including refusals and the administrator's switch-off), `retention.delete` (counts and sizes, no
file names), `recording.limits`. Each line has the time, who, the event, a short detail and whether it worked. No pictures, passwords,
keys or addresses with credentials (everything passes `iyakki_rtsp::url::scrub`; control characters cannot forge a line).
Live views of a USB camera are only in the general audit log (they have no folder here).

### Chat tools

| Tool | Risk | What |
|---|---|---|
| `camera_recording_status` | safe | state, mode, windows, limits, what is stored (owner only), the disk floor, the key warning |
| `camera_recording_set` | asks every time, no standing grant; **turning ON is a hard ask** (also in Auto) | mode off / continuous / schedule, `windows`, `max_days`, `max_gb` |
| `camera_recordings_list` | safe, owner only | days and recorded times (no pictures) |
| `camera_export` | asks every time, hard ask | `from` / `to` as local times like `2026-11-08 14:30`; decrypted copies in the person's folder |

Everything the tile does is reachable by chat. The tools are offered when the shell gave the tools a recorder
(`CameraViewTools::with_recorder`).

## Tests

* `iyakki-rtsp`: unit tests for every parser with hostile input, and `tests/loopback.rs` against the fake camera (Digest and Basic,
  wrong and missing password, H.264, silent and closing cameras, hostile DESCRIBE answers, HTTP streams and snapshots).
* `iyakki-agent::cameras::tests`: records, access rules, streams on loopback, idle stop, retry, USB through a fake provider.
* `iyakki-agent::camerastools::tests`: risks, no password through a tool, the masked form, cancelling, sharing.
* `iyakki-tile-cameras::tests`: the tile against a fake source (layouts, privacy band, snapshot, remove asks, access).
* `iyakki-shell::apps::cameras::tests`: the shell side with a fake `/sys` and the fake V4L2 opener.
* Recording: `iyakki-agent::camrec_format::tests` (round trip, no plain bytes on disk, wrong key, every changed byte found, swapped
  records, a power cut at every byte of the last record, H.264 and key text); `iyakki-agent::camrec::tests` (off by default and the
  sign, owner-only rights, locked vault, frames stored byte for byte, the real RTSP client against the fake camera, H.264 Annex-B,
  lost key, retention by age / size / total, the free-space floor stopping and resuming, crash recovery and index rebuild, a damaged
  picture, the timeline and stepping, access refusals logged, export confirmation and permissions, the log without secrets,
  windows including past midnight, schedule, restart, lost and removed cameras, the one-click stop);
  `iyakki-agent::camerastools::tests` (the four tools, cards, hard asks, bad input, sharing rules, end to end);
  `iyakki-tile-cameras::rec_tests` (the REC mark, playback with a fake source, day picker, confirmations, settings form,
  non-owner, H.264 message, log page).

## Not verified

Recording: **no real camera was recorded** (only the in-house fake camera and scripted feeds), **no long run** (the longest test
is a few seconds; behaviour over days, memory growth and file counts of weeks are untested), and **disk speed is unmeasured**
(a 1080p Motion-JPEG stream is roughly 1 to 4 MB a second; XTS encryption speed on the target machines was not measured). Playing
a recording decodes with the in-house JPEG decoder on the tile's own thread, so a large picture may make playback and the screen
stutter (not measured). H.264 cannot be played until the video add-on exists. The red sign and the REC pill were drawn in tests
and not looked at on a real screen. The vault key route is tested with a fake vault and with the real vault only through the existing
camera password route.

Live view:

No real camera was used. The one program that is not ours which the client was run against is ffmpeg serving Motion-JPEG over HTTP (`cargo test -p iyakki-rtsp --test ffmpeg_manual -- --ignored`, ffmpeg is the developer machine's own copy and is never shipped); there was no RTSP server to try on this machine. Unverified: streaming from a real USB camera through the hub, real RTSP cameras of any make (Hikvision,
Dahua, Reolink, Tapo ...: their SDP, `Transport` replies and keep-alive habits differ, and a camera that only offers UDP, an
https/rtsps stream or SHA-256 digest is refused), the speed of the in-house JPEG decoder on 1080p/4K frames, and the red band on a
real screen. The assistant tools are tested with scripted calls, not a real model.

## Steps for the owner

1. USB camera: plug it in, open **CAMERAS**; it should appear within about two seconds with the red band. Press `s`: a JPEG
   appears in Pictures. Open the CAMERA tile at the same time: one of the two shows "In use".
2. RTSP camera: in the camera's own settings pick **Motion-JPEG** (or note its H.264 address for later), create a view-only
   account, then in **CAMERAS** press `a`: name, `rtsp://<ip>:554/<path>`, user, password (masked), people. The view should go from
   Connecting to Live. Press `Enter` to enlarge, `s` to save a picture.
3. H.264 camera: the view should say "Needs video add-on" and not crash; nothing else changes.
4. Second account: add its name in People; sign in as it and check the camera is listed and watchable but not editable; remove the
   name and check it disappears.
5. Say which camera makes and models behave differently: the log line is `camera.view` / the state text on the view.
6. Recording, on a network camera you own: open **CAMERAS**, select it, press `p`, then `r`. Read the confirmation (it must say what is
   stored, the REC mark and sign, the vault warning), press `y`. Check: a red REC pill on the camera, a red sign bottom left on every
   screen, and after about 70 seconds a picture on the recordings page. Press Space (play), `,` and `.` (step), click the timeline.
   Click the red sign: the recording must stop and the sign go away.
7. Look in `<data>/cameras/recordings/<camera>/`: the `.irec` files must not contain readable pictures (`strings` or a hex viewer);
   `settings.json` and `audit.log` must contain no password.
8. Log in as a second account the camera is shared with: it must show the REC mark, open the page and say only the owner can open the
   recordings, and `l` must be refused. As the owner, `l` must list that person's views.
9. Fill the disk to below the floor (or set the floor high under Settings, as an administrator, with
   `camera_recording_set`/the limits) and check the recorder stops with the message, then resumes when you free space.
10. Sign out and in: recording that was on must start again after sign-in; while signed out nothing is written.
11. Export a minute: confirm, check `Pictures/camera-<name>-.../frame-000001.jpg` opens, and that the log has an `export` line.
