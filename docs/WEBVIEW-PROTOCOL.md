# The web view protocol (shell <-> engine helper)

IYAKKI's browser can show a page with a full web engine (WebKit) *inside its own page area*, in the desktop window, on a Linux
framebuffer (QEMU, bare metal) and in the web front end alike. The engine runs as a separate **helper program** that renders
**offscreen**; the shell draws the helper's pictures in the page area and forwards keys, mouse and wheel to it. There is no
compositor and no hand-over of the screen. This document is the contract between the two programs. The shell side is the crate
`crates/iyakki-webview` (client, wire format, a pure-Rust fake helper); the helper on the image is a C program written against
this document (WPE WebKit: libwpe + WPEBackend-fdo); the helper for a development desktop is `dist/webview/host-helper.py`
(WebKitGTK).

Everything here is normative unless marked *Notes for implementers*.

## 1. Processes and files

```
iyakki shell (client)                                    helper (WPE WebKit / WebKitGTK)
  |  socketpair(AF_UNIX, SOCK_STREAM)  -- messages -->     fd 0 (stdin)
  |  <-- messages --
  |  /dev/shm/iyakki-webview-<pid>-<n>.shm   (pixels, written by the helper, read by the shell with pread)
```

The shell starts the helper once per browser tab that uses it, without a shell, with this command line:

```
HELPER [fixed arguments of the build...] --socket-fd 0 --shm /dev/shm/iyakki-webview-1234-0.shm
```

* **fd 0 is the socket** (a connected `SOCK_STREAM` Unix socket; the shell's end is the other half). It is bidirectional and is
  the only channel for messages. `--socket-fd N` always says which descriptor; today N is 0 because the shell hands the
  socket over as the helper's standard input. A helper must not read or write fd 0 in any other way.
* **stdout is closed** (/dev/null). **stderr is a pipe**: every line is shown to the shell as a log line (also used for the reason
  of a crash). Keep it to diagnostics; a few lines per second at most.
* `--shm PATH` is a file made by the shell (mode 0600, sparse, owned by the user the helper runs as). The helper opens it
  read/write (`open(O_RDWR)`, then `pwrite` or `mmap(MAP_SHARED)`) and never resizes, truncates or deletes it. Its length is
  `2 * max_w * max_h * 4` (see Hello). The shell removes the file when the view ends.
* The current directory is the person's profile folder; the environment is the one the shell passes (on the image: `HOME`, `XDG_*`
  inside the profile, `LANG`; on a desktop: the desktop's own). The helper must keep every file it writes inside the profile
  folder named in `Hello.profile`.
* The helper **must not save files by itself** (downloads are reported, section 6) and must not open windows or other
  connections to the display. It runs as an unprivileged user.

When the socket is closed by the shell (EOF on a read), the helper must exit promptly. When the helper closes the socket or
exits, the shell treats the view as crashed (section 9).

## 2. Framing and types

Every message in both directions is

```
u32  length        bytes that follow (type + payload), little-endian, 1 <= length <= 4194304 (4 MiB)
u8   type
...  payload       length - 1 bytes
```

All integers are little-endian. `i32` is two's complement. `bool` is one byte, 0 or 1 (any non-zero is true when reading; write 1).
`str` is `u32 byte-length` followed by that many bytes of UTF-8 (no terminating NUL), at most 1 MiB; invalid UTF-8 is replaced when
read. A receiver ignores bytes after the last field it knows of in a known message (room to add fields at the end later) and
**skips messages of unknown type** (reads and discards them). A length of 0 or above 4 MiB is a protocol error: close the socket.
Messages are written whole (one `write` of the framed buffer, or loop until written); the receiver reassembles from the stream.

Message types: shell to helper `0x01..0x7f`, helper to shell `0x81..0xff`.

## 3. Conversation

1. The shell starts the helper and sends **Hello** immediately. It is always the first message.
2. The helper creates its engine and view of the size in Hello (device pixels), opens the shared file, and sends **Ready**. Until
   Ready arrives the shell shows "Starting the web engine..." (and gives up after 25 s: the helper is then killed and the browser
   falls back to its own engine).
3. The helper sends a first **Frame** covering the whole picture as soon as the view has painted (or immediately, white), then
   Frames whenever the picture changes. It also reports **Title**, **Url**, **Loading**, **History**, **Cursor** as they change.
4. The shell sends **Navigate** (the first one right after Hello) and input and control messages whenever they happen.
5. The shell sends **Quit** to end the view (or just closes the socket). The helper exits within 2 s; otherwise it is killed.

The helper may receive any shell message at any time after Hello, including before Ready has been sent (queue them).

## 4. Shell to helper

| type | name | payload |
|---|---|---|
| 0x01 | Hello | `u32 version` (= 1), `u32 w`, `u32 h`, `u32 scale_pct`, `u32 max_w`, `u32 max_h`, `str user_agent`, `str profile`, `str locale`, `u32 n`, `n x str allow_hosts` |
| 0x02 | Navigate | `str url` |
| 0x03 | Reload | `bool hard` (true: bypass the cache) |
| 0x04 | Stop | (empty) |
| 0x05 | Back | (empty) |
| 0x06 | Forward | (empty) |
| 0x07 | Resize | `u32 w`, `u32 h` |
| 0x08 | Mouse | `u8 kind` (0 move, 1 down, 2 up), `i32 x`, `i32 y`, `u8 button`, `u8 buttons`, `u32 mods`, `u8 clicks` |
| 0x09 | Wheel | `i32 x`, `i32 y`, `i32 dx`, `i32 dy`, `u32 mods` |
| 0x0A | Key | `bool down`, `u32 keysym`, `u32 mods`, `str text` |
| 0x0B | Text | `str text` |
| 0x0C | Zoom | `u32 percent` |
| 0x0D | Focus | `bool focused` |
| 0x0E | SetVisible | `bool visible` |
| 0x0F | Ack | `u8 slot` |
| 0x10 | Quit | (empty) |
| 0x11 | Script | `u32 id`, `u32 timeout_ms`, `str js` (protocol 1.1) |

**Hello.** `version` is the protocol version (1; a helper that does not know it exits with status 2 after writing a line to
stderr). `w`,`h`: the size of the picture in *device pixels* (the size of the page area on the shell's screen). `scale_pct`: the
device scale factor in percent (100 = one device pixel per CSS pixel, 200 = HiDPI); the page's CSS viewport is `w*100/scale_pct`
by `h*100/scale_pct` and `window.devicePixelRatio` is `scale_pct/100`. `max_w`,`max_h`: the largest picture the shared file has
room for; the shell never sends a Resize beyond them. `user_agent`: text to add to the engine's own user agent after the
engine's browser identification (for example `IYAKKI/0.1.0`; empty = leave alone) - never replace the engine's user agent with
something that makes sites refuse it. `profile`: absolute path of the folder for cookies, local storage, caches and anything
else persistent (create the sub-folders needed; empty = keep nothing, private mode). `locale`: BCP 47 tag for
`Accept-Language`/`navigator.language` (empty = engine default). `allow_hosts`: host names the account may open, `*` = any host,
otherwise a host matches when equal to an entry or a sub-domain of one. The helper should refuse **top-level navigations** to other hosts (and
report them as a log line); the shell checks every address again (section 7), so this is defence in depth and need not
cover sub-resources.

**Navigate.** Load this `http`/`https` address in the top-level frame. (The shell has already vetted it.) Navigating while loading
replaces the load. Other schemes are not sent.

**Reload / Stop / Back / Forward.** As in any browser. Back and Forward do nothing when the history cannot go that way.

**Resize.** Make the picture `w` x `h` device pixels (same scale factor). The helper then draws frames of the new size (the next
Frame carries the new `w`,`h` and a rectangle covering the whole picture). Frames of the old size may still arrive for a short time;
the shell copes. `w`,`h` >= 1 and <= max_w,max_h.

**Mouse.** Position `x`,`y` in *device pixels relative to the top-left of the picture* (may be negative or beyond the size during a
drag that left the area; the helper clamps only if its engine requires it). `kind` 0 = pointer moved (also sent while a button is held:
a drag), 1 = button pressed, 2 = released. `button` is the button that changed (0 none, 1 left, 2 middle, 3 right) for kinds 1 and 2,
0 for a move. `buttons` is the bit mask of buttons held *after* this event (bit 0 left, bit 1 middle, bit 2 right). `mods`: modifier
bits (section 8). `clicks`: the click count the shell counted for a press (1, 2 or 3; 0 for moves and releases); the helper may use it
for double and triple click (or count itself). The shell sends a move to the press position just before every press.

**Wheel.** Pointer position as in Mouse. `dx`,`dy` are in 1/120 of a wheel notch: 120 is one notch. `dy > 0` scrolls the page down
(content moves up, like `deltaY > 0` in DOM `wheel` events); `dx > 0` scrolls to the right. The helper turns one notch into what a
normal wheel does in its engine (3 lines, about 53 CSS px in WPE).

**Key.** One key press (`down` true) or release (false). `keysym` is an X11 keysym (section 8). `mods` modifier bits. `text`: the text the key
types, UTF-8 (empty for keys that type nothing, such as arrows, function keys, and every key with Ctrl or Alt held; "\r" for Return, "\t"
for Tab). The shell sends a press followed directly by a release (no auto-repeat) for each key. The helper must give the engine a
key event with the keysym (so scripts see `keydown`/`keyup` with the right `key` and `code`) and make the key type `text` into the focused field when `text` is not empty.

**Text.** Insert this text into the focused editable element as one input (paste, an input method's result). It is *not* a sequence
of key events.

**Zoom.** The page zoom in percent relative to the device scale (100 = normal, 125, 150, ... the user's per-site zoom). The effective
magnification of the page is `scale_pct/100 * percent/100`. Sent when the user changes the zoom, when the shell's own interface
scale changes while a view is open (the shell then folds the change into this number), and once after Hello when the site's zoom is not 100.

**Focus.** The keyboard focus of the page area: true when keys go to the page (show the caret, deliver key events), false when
the browser's own address bar or shortcuts have it (stop blinking carets; do not deliver keys). Sent once after Hello with true.

**SetVisible.** The tab is shown (true) or hidden behind another tab (false). While hidden the helper should not produce Frames
(it may throttle timers and pause rendering). When shown again it sends a Frame covering the whole picture.

**Ack.** See section 5: the shell has finished reading this slot.

**Quit.** End. Exit with status 0 within 2 s.

**Script** (1.1, optional, used by the assistant's browser tools and the test runner, see `docs/BROWSER-AGENT.md`). Evaluate `js` as a
script in the top-level page's main world and answer with **ScriptResult** carrying the same `id`. `timeout_ms` (clamped to 0.1-120
s, 0 = 10 s) is how long the helper waits for the page; after it the helper answers `ok` false with `json` = `script timed out`. The
script runs synchronously (a returned Promise is not awaited; the caller polls). On success `ok` is true and `json` is the result
converted to JSON text (`undefined` and `null` become `null`; a string becomes a JSON string). On an exception `ok` is false and
`json` holds the error text. Scripts that never finish must not block later messages. A helper that does not know type 0x11
skips it (section 2) and the shell's own timeout ends the wait, so the protocol version stays 1: the shell only needs Script
for the assistant's tab. The shell never logs script text (it may carry a typed secret); a helper must not either. Screenshots need
no message: the shell keeps the latest Frame.

## 5. Pixels: the shared file and the frame handshake

The file holds **two slots** (0 and 1) of `max_w * max_h * 4` bytes each: slot `k` starts at byte offset `k * max_w * max_h * 4`.
A picture of `w` x `h` is stored in the first `w * h * 4` bytes of a slot, **tightly packed**: row `r` starts at `r * w * 4`
(the stride is the width of *that frame*, not `max_w`). Each pixel is 4 bytes **B, G, R, A** in memory (little-endian
ARGB32, Cairo `ARGB32`, Skia `BGRA_8888`), **premultiplied alpha**. A normal page is opaque (A = 255); where a page leaves
pixels transparent the shell composites over white.

**Frame** (helper to shell, 0x82): `u8 slot`, `u32 w`, `u32 h`, `u32 n`, `n x (u32 x, u32 y, u32 w, u32 h)` (n <= 64).

* The helper writes pixels into a slot **it owns** and then sends Frame for it. Only the pixels *inside the rectangles* of the Frame are
  guaranteed valid in that slot; the rest of the slot may be stale. The rectangles cover everything that changed since the previous
  Frame (whatever slot it used). The shell keeps its own copy of the whole picture and updates only those rectangles, so a small
  change costs a small copy. `n = 0` means "the whole picture". The first Frame, the first after a Resize or after SetVisible(true), and
  any Frame whose `w`,`h` differ from the previous one **must cover the whole picture** (one rectangle `0,0,w,h`, or n = 0).
* A slot is owned by the helper until the helper sends Frame for it; it is then owned by the shell until the shell sends **Ack(slot)**.
  The helper must **not write to a slot between its Frame and the matching Ack**. With both slots out, it keeps drawing into its own
  (private) buffer and sends the next Frame when an Ack arrives (merging the changes: union the rectangles). The shell acks every Frame
  it has copied, in order, usually within one display refresh (~16-33 ms).
* The helper sends at most about 30 frames per second (more is wasted: the shell shows at most 30). When nothing changes it sends nothing.
* Frames of an unknown slot (> 1), `w` or `h` of 0 or above `max_w`/`max_h`, are ignored by the shell.

The shell reads a slot with plain `pread` (no `mmap`). A helper may `pwrite` or `mmap` as it likes, but must make its writes visible before sending
the Frame (a `msync` is not needed on the same machine, a normal write is visible to `pread` at once).

*Notes for implementers (WPE).* With WPEBackend-fdo the exportable's `export_shm_buffer` callback gives a `wl_shm_buffer`: `wl_shm_buffer_begin_access()`, copy the
changed rows into the free slot (`wl_shm_buffer_get_data`/`get_stride`; formats ARGB8888/XRGB8888; XRGB has no alpha: set A = 255), `wl_shm_buffer_end_access()`, send
Frame, and call `wpe_view_backend_exportable_fdo_dispatch_frame_complete()` and `wpe_view_backend_exportable_fdo_dispatch_release_shm_exported_buffer()` as the
fdo API requires. Damage rectangles are not given by the exported buffer; use the whole picture or compare with the last one. Video: WPE
composites it into the same buffers (GStreamer), so nothing special is needed. Use `WPE_BACKEND`/`libWPEBackend-fdo` with EGL on llvmpipe or `WEBKIT_DISABLE_COMPOSITING_MODE=1`.

## 6. Helper to shell

| type | name | payload |
|---|---|---|
| 0x81 | Ready | `u32 version` (= 1), `str engine`, `str engine_version` |
| 0x82 | Frame | see section 5 |
| 0x83 | Title | `str title` |
| 0x84 | Url | `str url` |
| 0x85 | Loading | `bool loading`, `u8 progress` (0..100) |
| 0x86 | History | `bool can_back`, `bool can_forward` |
| 0x87 | Cursor | `u8 kind` |
| 0x88 | Download | `str url`, `str name` |
| 0x89 | Crashed | `str reason` |
| 0x8A | Log | `str line` |
| 0x8B | Clipboard | `str text` |
| 0x8C | ScriptResult | `u32 id`, `bool ok`, `str json` (protocol 1.1) |

* **Ready**: engine name and version for "about" displays (`WPE WebKit` `2.46.3`). Sent once, after Hello.
* **Title**: the document title; sent when it changes (an empty title is sent as empty).
* **Url**: the *committed* top-level address (what the address bar should show), sent when it changes: after a navigation commits,
  on a redirect, on `history.pushState`. This is also what the shell vets (section 7).
* **Loading**: `loading` true from the start of a navigation until it finishes or fails; `progress` the estimated progress (100 when done).
* **History**: whether Back / Forward can do something. Sent when either changes (after every navigation).
* **Cursor**: the pointer shape the page wants over the point last given in a Mouse message: 0 default, 1 pointer (link), 2 text, 3 wait/progress,
  4 crosshair, 5 move, 6 not-allowed, 7 resize east-west, 8 resize north-south, 9 grab, 10 help, 11 none. Informational: the shell may use it.
* **Download**: the page started a download (`Content-Disposition: attachment`, an unsupported type, a user click on a download link). The helper
  **cancels** the download and reports `url` (the address to fetch) and `name` (the suggested file name; empty if unknown). The shell's own
  download code decides whether to fetch and where to save (account policy, folder, never overwrite). The helper never writes the file.
* **Crashed**: the *engine* inside the helper failed (the web process crashed) but the helper is alive and has started recovering (for
  example reloading). The shell shows `reason` and goes on. If the helper cannot recover it should exit instead.
* **Log**: a diagnostic line (kept by the shell for diagnosis; not shown to the person).
* **Clipboard**: the page copied text (Ctrl+C / Ctrl+X in the page, `navigator.clipboard.writeText`); the shell puts it on its own clipboard.
  A helper that cannot observe copies may leave it out; the shell's Ctrl+V sends **Text** into the page, so pasting works either way.

## 7. Navigation policy

Every navigation the helper reports with **Url** is checked by the shell against the signed-in account's host list. If the host is
refused (or the address has no valid host and is not `about:`, `data:` or `blob:`), the shell sends **Back** (or **Navigate** to the last
good address, or `about:blank` if there is none), shows a notice and writes an audit entry. The helper therefore reports every top-level change, including redirects. A
well-behaved helper also blocks top-level navigations to hosts outside `Hello.allow_hosts` itself and writes a Log line. Pop-up
windows and `target=_blank` links are loaded in the same view (the shell has one page area per tab). Certificate errors are
**not** bypassable: the load fails (show the engine's error page). Geolocation, camera, microphone, notifications and similar permission requests are
**denied**. Cookies and storage live only in `Hello.profile`.

## 8. Keys, modifiers and the shell's key types

`keysym` values are **X11 keysyms** (`X11/keysymdef.h`), the numbering WPE (`wpe_input_keyboard_event.key_code`) and GTK (`GDK_KEY_*`) use:

* printable characters: the Unicode code point when it is in `0x20..0x7e` or `0xa0..0xff`; otherwise `0x01000000 + code point`
  (`€` U+20AC is `0x010020ac`);
* editing and navigation (`0xff..` page): BackSpace `0xff08`, Tab `0xff09`, ISO_Left_Tab (Shift+Tab) `0xfe20`, Return `0xff0d`, Escape `0xff1b`,
  Delete `0xffff`, Home `0xff50`, Left `0xff51`, Up `0xff52`, Right `0xff53`, Down `0xff54`, Page_Up `0xff55`, Page_Down `0xff56`, End `0xff57`, Insert `0xff63`;
* function keys F1..F12: `0xffbe..0xffc9`.

`mods` bits: **1 Shift, 2 Ctrl, 4 Alt, 8 Super**. (Lock keys are not sent.) The shell keeps no modifier state beyond what its key type
carries (below), so Alt and Super are never set today and Mouse/Wheel `mods` are 0; a helper must still honour them.

Mapping from the shell's `ui::Key` (`crates/iyakki-shell/src/ui.rs`) to `Key` messages (a press with `down = true`, then a release with `down = false` and empty `text`):

| shell key | keysym | mods | text |
|---|---|---|---|
| `Char(c)` | as above for `c` | Shift if `c` is an upper-case letter | `c` |
| `Ctrl(c)` | as above for `c` (`'c'` = `0x63`, `'C'` = `0x43`) | Ctrl (+ Shift when `c` is upper case) | empty |
| `Enter` / `Tab` | `0xff0d` / `0xff09` | 0 | `"\r"` / `"\t"` |
| `BackTab` | `0xfe20` | Shift | empty |
| `Backspace`, `Delete`, `Esc` | `0xff08`, `0xffff`, `0xff1b` | 0 | empty |
| `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown` | `0xff52`, `0xff54`, `0xff51`, `0xff53`, `0xff50`, `0xff57`, `0xff55`, `0xff56` | 0 | empty |
| `Nav{nav, shift, ctrl}` | the key above | Shift if `shift`, Ctrl if `ctrl` | empty |
| `F(n)` | `0xffbe + n - 1` | 0 | empty |
| `Event::Paste(t)` | (not a key) sent as **Text** `t` | | |

Keys the shell uses for itself never reach the helper (F1 help, F9 inbox, F12 screenshot, Ctrl+Shift+V clipboard history, Ctrl+T/W/L/D/H/B/J browser shortcuts, and
Esc, which first hands the keyboard back to the browser). Right-click is the shell's context menu (Back, Forward, Reload and Clipboard history…) today and is not forwarded to the page.

## 9. Failure, restart, shutdown

* **Helper exits or closes the socket** (crash, kill, `exit`): the shell sees EOF (and/or the exit status) once, reports "the web engine stopped" with the
  last stderr lines, and switches the tab back to its own engine on the last good address. The person can press F again for a fresh helper; the shell's
  client (`WebView::restart`) also supports restarting by itself and replaying size, zoom and the last good address (not an address that crashed it).
* **No Ready within 25 s**: the shell kills the helper and falls back the same way.
* **Shell exits or closes the socket**: the helper sees EOF and must exit (and its engine sub-processes must follow).
* **Quit** (or closing the socket): clean exit within 2 s. After 2 s the shell sends SIGKILL to the helper process (not to its descendants: sub-processes must notice that
  their parent is gone, e.g. `PR_SET_PDEATHSIG`).
* A message that cannot be decoded is skipped by the shell (and logged); the helper should do the same.

## 10. Checking a helper

* `cargo run -p iyakki-webview --example probe -- HELPER https://example.org/ wait:5 fps:3 shot:/tmp/page.png` starts any helper,
  navigates, prints what it reports, measures frames per second and saves what it drew as a PNG. Steps for `move:X,Y click:X,Y type:TEXT key:ret wheel:0,240 nav:URL resize:W,H`.
* `crates/iyakki-webview/src/bin/fake_helper.rs` is a reference implementation of the helper side without an engine (test pattern, moving bar, echoes the mouse).
* `crates/iyakki-webview/tests/helper.rs` and the browser tests (`cargo test -p iyakki-shell --lib -- browser`) exercise the protocol against it.
* `dist/webview/host-helper.py` is the WebKitGTK helper used in window mode on a development machine (virtual X server `Xvfb` for video, or `GtkOffscreenWindow`).

A conforming helper: answers Script with ScriptResult (when it supports 1.1); answers Hello with Ready; produces a whole-picture Frame first; never writes a slot it has sent and not had acked;
reports Url for every committed top-level address; never writes files outside the profile folder; cancels downloads and reports them;
denies permission prompts; exits on Quit or EOF; skips unknown messages.
