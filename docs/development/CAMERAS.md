# CAMERAS: live view and recording of USB and network cameras

Feature rows: F-CAM-02 (the tile), F-CAM-15 (the RTSP and HTTP Motion-JPEG client), F-CAM-16 (cameras, vault and access), F-CAM-17
(the assistant's tools), F-CAM-03 (recording, see "Recording" below), F-CAM-04 and F-CAM-05 (local motion watching, zones, privacy masks,
events and inbox alerts, see "Motion watching" below). Nothing looks at what is in a picture unless the owner switched motion watching on
for that camera.

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

## Motion watching (F-CAM-04) and alerts (F-CAM-05)

Code: `crates/iyakki-agent/src/camdetect/` (`motion.rs` detector, `geom.rs` polygons and masks, `events.rs` event store, `alerts.rs`
rate limits, `person.rs` the person hook, `settings.rs` settings on disk, `watch.rs` the service), the picture tap in `cameras.rs`
(`FrameTap`, `Hub::keep_alive`), the mask hooks in `camrec.rs` and `Hub::snapshot`, the tools in `camerastools.rs`, the page in
`crates/iyakki-tile-cameras/src/motion.rs`, the shell wiring in `crates/iyakki-shell/src/apps/cameras.rs`. Everything runs on this
computer; nothing is sent anywhere, no model is used, no new dependency was added.

### The rules

* **Off by default, per camera, owner only.** The owner turns it on from the page (key `m` on a camera, then `w`) or by chat
  (`camera_motion_set`). Turning it on always shows what is analysed, what is stored and which sign shows; in chat the same text is the
  approval card and it is a hard ask (Auto mode does not skip it). An administrator may turn it OFF. A person the camera is shared with
  sees THAT it is watched, nothing else. Network cameras only: this computer's own USB camera is never watched in the background.
* **Modes.** `watched`: motion is looked for only while the camera's picture is on a screen or the camera is being recorded (both
  already show a sign: the picture itself, the red REC sign). `always`: the hub's stream is kept running by the watcher and a red sign
  "REC: Front door (motion watch). Click to stop." shows on every screen while a person is signed in; one click switches it off. In
  every mode a blue MOTION mark sits on the camera's picture in the grid while its pictures are being analysed. Nothing analyses a
  camera without one of these.
* **Stops at sign-out.** The events are encrypted with the camera's key in the owner's vault. When the vault closes the watcher finds
  the key gone within a second or two, closes the detector and drops pictures; nothing is analysed or stored until the vault opens
  again, and then the background starts fresh (a scene that changed meanwhile is not an event).
* **Rights.** Turning on, zones and masks, alert settings, reading events and thumbnails: the camera's owner. A refusal is written to
  the camera's log (`motion.setting`, `motion.events` with "refused: not the owner"). A stranger does not learn the camera exists.

### The detector (`motion.rs`)

A pure library: it is given the picture, its size and its time, and returns events; no clock, no files. Steps: shrink to a 64-cell
wide grey grid; a running-average background model per cell whose learning rate adapts (fast for still cells, slow at first for
changed cells and faster the longer they stay changed, so a parked car is absorbed after about a minute and a half); the median
brightness ratio and shift of all watched cells are taken out first (clouds, lamps, flicker and exposure changes move every cell the
same way and are not motion); a per-cell threshold of at least 18 grey levels (scaled by sensitivity 1 to 10) and three times the
cell's own noise; lone cells dropped, the rest joined into blobs of at least 0.4 % of the watched area; a change touching more than
60 % of the watched cells (lights on or off, night mode) replaces the background and reports nothing; motion in 2 pictures in a row
starts an event, 4 seconds without motion ends it, 2 seconds of cool-down, at most 10 minutes per event. The detector gets the hub's
small decoded picture (up to 480 px wide, at most 5 a second), so it adds no decoding of its own.

Honest limits: it finds change, not people. Wind in a tree, rain, insects at night, reflections and flashing screens can alarm (an
exclude zone over the tree helps); a slow walker in a dark corner can be missed; a person filling more than 60 % of the watched area
looks like a global change. False alarm rates on real scenes are unknown.

### Zones and privacy masks (`geom.rs`)

Polygons in picture-relative coordinates (0 to 1). A zone is `include` (with at least one, motion counts only inside the include zones,
each with its own sensitivity) or `exclude` (never watched). A privacy mask is blacked out in a copy of every picture BEFORE the
detector, the thumbnail and the person hook see it. A polygon is checked before it is kept: 3 to 32 corners, every corner inside the
picture, no repeated corner, an area of at least 0.05 % of the picture, no crossing or folded edges; refused in words otherwise, and a
refusal changes nothing. At most 12 zones and 12 masks per camera; a shape with the same name (any case) replaces. Drawing in the tile:
`z` zone, `x` exclude, `m` mask; click the corners on the picture; Enter or a click on the first corner closes it and asks for a name
(and sensitivity); Backspace takes the last corner back, Esc cancels. By chat: `camera_zone_set` with `points` or `rect`.

**"Also in recordings and snapshots"** (`s` on the page, `mask_storage`, off by default): the masks are then black before the recorder
writes a frame and before a snapshot is saved. Recorded Motion-JPEG frames are decoded, blackened and encoded again (so a masked
recording is no longer "as received": more CPU, and a little quality loss; not measured), a picture that cannot be decoded or masked is
not stored and the recorder stops with a visible message, and an H.264 camera with masks set for recordings stops the same way (no
decoder, so the masks cannot be applied). If the settings file exists but cannot be read, stored pictures are covered completely
(fail closed). With the box not ticked the masks apply to detection only and recordings are stored exactly as before.

### Events (`events.rs`)

Motion start and end with time, zone, score (1 to 100, the changed share of the watched area) and one small thumbnail (160 px wide,
from the masked picture) are stored per camera in `<data>/cameras/events/<camera id>/YYYY/MM/DD/HHMMSS.iev`, in the same encrypted file
format as recordings (`camrec_format`, kind `Events`: AES-256-XTS with encrypt-then-MAC per record, a power cut loses only the last
record, a changed byte is found and the record is never shown) under the same key (`camrec.<id>` in the owner's vault, made when
watching is first turned on). No zone name, score or picture is readable on disk; times and sizes of records are, like recordings.
Retention is the camera's recording `max_days` (default 14, 0 = none) plus a 256 MB cap per camera, checked every 5 minutes, oldest
first; the file being written is never removed. They show in the Motion page's events list (`e`, with the thumbnail of the selected
one) and as amber ticks above the day timeline of the recordings page.

### Alerts (`alerts.rs`)

Per camera, off by default, to the owner's inbox (source `cameras`, linking back to the camera). At most `alerts_max_per_hour` notices
per camera in any rolling hour (default 4, never more than 30); the first event of a burst sends one notice and opens a burst of 10
minutes; the burst's other events send nothing and ONE grouped notice ("7 more motion events at Front door", with the times) follows
when it ends; events past the hourly limit are stored but make no notice, and the next notice says how many were held back; quiet hours
(`22:00-06:30`, may wrap midnight) send nothing, and ONE summary follows when they end. The inbox cannot carry a picture, so the
thumbnail passes only to a sink that can (none ships yet), and only when the person allowed it (`t` on the page); the notice text says
the picture is in CAMERAS.

### Person detection (the hook, `person.rs`)

There is no person detector. A heuristic on shapes or motion would only guess, so the default `PersonDetector` ("not available") says
so in every status and no event is ever labelled person. The route for the real thing is the data-science toolkit
(`DATA-SCIENCE-ADDON-PLAN.md`): an isolated job on the person's own k3s cluster receives ONE picture and returns boxes. The agent asks
only for an event that already passed the motion detector, only with the masked picture, and only when the camera's `person` switch is
on and a detector exists. Request and answer (versioned JSON, checked on the way back, `person::request_json` / `parse_answer`):

```text
request  {"v":1,"task":"person","min_confidence":0.5,
          "image":{"format":"jpeg","width":480,"height":270,"data_b64":"..."}}
answer   {"v":1,"boxes":[{"x":0.41,"y":0.20,"w":0.12,"h":0.55,"confidence":0.91,"label":"person"}]}
error    {"v":1,"error":"words"}
```

Boxes are fractions of the picture; at most 50; a box outside the picture, a confidence outside 0 to 1 or a missing number makes the
whole answer invalid; boxes under `min_confidence` or with another label are ignored. `FakePersonDetector` stands in for tests. Not
built and not run: the job, the transport to it, the cluster (the cluster was not touched).

**Follow-up, not built (rest of F-CAM-05):** "person at the front door, 02:14". When a detector exists and the person opts in (a
separate switch, off by default, with a card that says the thumbnail goes to the chat model they chose), the existing chat model gets
the thumbnail plus the zone and time and writes one line for the notice; it uses the agent's budgets and approvals, and any action
beyond a notice (switching something on, calling someone) is an ordinary approval. Nothing of this sends a picture anywhere today.

### Chat tools

| Tool | Risk | What |
|---|---|---|
| `camera_motion_status` | safe | mode, state, zones, masks, alerts, whether a person detector exists (details for the owner) |
| `camera_motion_set` | asks every time, no standing grant; **turning ON is a hard ask** (also in Auto) | mode off / watched / always, sensitivity, alerts (on, per hour, quiet hours, thumbnail), `mask_storage`, `person` |
| `camera_events_list` | safe, owner only | events of the last `hours` (default 24) or of a `day`: time, zone, strength, duration |
| `camera_zone_set` | asks every time, no standing grant | add or remove a zone, exclude zone or privacy mask from `points` or `rect` |

The tools are offered when the shell gave the tools a motion service (`CameraViewTools::with_motion`).

### Cost

Measured with a standalone build of `motion.rs` and `geom.rs` (rustc, optimisation level 3, no other code) on synthetic 640x480
pictures at 5 a second: about 0.34 ms per picture for mask plus detection, that is about 0.2 % of one core; at optimisation level 0
(what the test profile builds) about 6 ms per picture, 3 % of one core. The real pipeline adds the hub's decoding (already done for
the live view; with nobody watching and "always" mode the watcher is the only reason the stream is decoded). Not measured on the
target machines, not measured with a real camera, not measured for masked recordings (decode and encode of every frame).

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

* Motion watching: `iyakki-agent::camdetect::motion::tests` (a moving rectangle, slow lighting drift, mains flicker, camera noise and hot pixels,
  a sudden global change and recovery, a speck, a blink, a parked object, quiet time and cool-down, zones, per-zone sensitivity),
  `camdetect::geom::tests` (point in polygon, bad polygons, spans, blackening), `camdetect::alerts::tests` (switch, grouping, hourly limit,
  quiet hours, thumbnail), `camdetect::person::tests` (not available by default, request, answer checks, fake), `camdetect::events::tests`
  (round trip, no readable text, wrong key, tampering and power cut, retention), `camdetect::watch::tests` (rights, detection with masks,
  encryption and retention, vault closing, watched mode, alerts, quiet hours, person hook, shapes, masked recordings and snapshots, a live
  fake camera feeding the detector, thumbnails, settings), `camerastools::tests::motion_tests` (tools, refusals, cards, end to end),
  `iyakki-tile-cameras::motion_tests` (the page, drawing, naming, events, marks), `iyakki-shell::apps::cameras::tests` (the shell wiring).

## Not verified

Recording: **no real camera was recorded** (only the in-house fake camera and scripted feeds), **no long run** (the longest test
is a few seconds; behaviour over days, memory growth and file counts of weeks are untested), and **disk speed is unmeasured**
(a 1080p Motion-JPEG stream is roughly 1 to 4 MB a second; XTS encryption speed on the target machines was not measured). Playing
a recording decodes with the in-house JPEG decoder on the tile's own thread, so a large picture may make playback and the screen
stutter (not measured). H.264 cannot be played until the video add-on exists. The red sign and the REC pill were drawn in tests
and not looked at on a real screen. The vault key route is tested with a fake vault and with the real vault only through the existing
camera password route.

Motion watching: **no real camera was used** (synthetic pictures and the in-house fake camera), **false alarm and miss rates on real
scenes are unknown** (rain, trees, insects, headlights, auto-exposure and night-mode switching are the likely trouble), the CPU figure is
from a standalone synthetic run, the red sign, the MOTION mark and the drawing page were drawn in tests and not looked at on a real
screen, masked recordings were tested with the fake JPEG source and the fake camera only, alerts reach the inbox but no other channel,
and there is no person detector (the hook is tested with a fake).

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
12. Motion watching, on a network camera you own: open **CAMERAS**, select it, press `m`. Press `w` and read the question (it must say
    nothing leaves the computer, what is stored, and the sign); press `y` for "while watched". Walk in front of the camera while its picture
    is on a screen: a blue MOTION mark on the picture and, after a few seconds, an event under `e` with a thumbnail. Press `w` again: the
    next mode (all the time) asks again; then a red sign "REC: ... (motion watch)" must appear on every screen and a click on it must stop it.
13. Press `z`, click four corners round the driveway, Enter, name it: only motion inside counts. Press `x` over a tree in the wind. Press
    `m`, draw a mask over a neighbour's window, name it: the picture in the page still shows it, but the thumbnail of an event must show
    it black. Press `s` and record a minute: the recording page must show the area black; with `s` off it must not.
14. Press `a` for alerts and walk around five times: the inbox gets one notice, then one grouped notice ("N more motion events"), and no
    more than the per-hour limit. Set quiet hours with `c` that include now and walk: nothing arrives until they end, then one summary.
15. Sign out and in: while signed out no MOTION mark, no event, no sign; after sign-in a camera switched on resumes (the first pictures
    only settle the background).
16. Say what false alarms you get and from what (the log line is `motion.start` with the zone and score in the camera's log, `l` on the
    recordings page).
