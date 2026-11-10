# Layouts, panes and several displays

Status: **phases 0, 1 and 2 are built and tested on made-up screens and a made-up graphics card (a fake DRM backend). The real
DRM code has never been run: not on the development computer and not on a Mac.** The owner checklist at the end says what to
try on a MacBookPro14,1 with an external monitor and on a Mac mini with HDMI, and what to send back. Rows `F-SHELL-36` onward and `F-DISP-*` in
`FEATURES.md` say what is done and what is not.

## 1. What exists today (measured in the code)

* The shell (`crates/iyakki-shell/src/shell.rs`) owns **one** `Screen` at a time: `Screen::App(Box<dyn App>, PApp)` or
  `Screen::Launcher`. One header strip (`HEADER_H` = 46 units) is drawn by the shell above the app's area; the app draws into
  `area` with `App::render(g, area, s, cx, tick)` and handles `Event`s through `App::handle`.
* Apps are ordinary objects. They can be kept alive side by side: the shell already does it for the inbox (`stash`) and the
  Browser/Radio keep running in the background. They get their size only through `area` (and `g.width()/g.height()` in a few
  places: `browser_full`, `mediaplay`, `story`, `ide_browser`). They do not read the screen size from a global.
* Global state that matters: `ui.rs` thread-locals (UI size, drag flag, click count, shift, caret/focus-ring counters, window
  focus), the clipboard (one history per UI thread, shared by design), `ctxmenu` field notes (per frame), the single-entry
  background cache (`fx::background`, one size), `web_agent::TABS` (per user, not per app instance). None of them is per
  screen, so two apps in two panes share the clipboard and the pointer state, which is what a person expects.
* Input: evdev keys become `Event::Key` (`input.rs`; the remote desktop maps browser keys in `iyakki-remote/src/input.rs`).
  Clicks are resolved against the **hit regions of the last frame** (`Gfx::hit`), which carry an `Event` (a tab number, a row,
  a text field); the shell dispatches that event to the screen. There was no Alt modifier anywhere.
* Display: `/dev/fb0` (fbdev emulation of DRM). The kernel scans out **one buffer on every connected connector as a clone**;
  the shell reads `/sys/class/drm/card*-*/{status,modes,enabled}` (`display.rs`, `visible_size`) and lays itself out for the
  preferred mode of the primary connector (the built-in panel, else the smallest). A second monitor shows the top-left part of
  the same picture. fbdev cannot show two different pictures.
* `scale_for(w, h)` scales fonts and controls from the size (0.9 at 1280x720, about 1.0 at 1080p, 1.25 at 4K, clamped 0.6..3.0).
  `launcher::page_rects(w, h, ...)` already computes the tile grid and page count from any size.
* The framebuffer loop (`backend.rs::run_fb_once`) renders the whole shell into one `Gfx` when `Shell::take_stale()` says so and
  copies it to the buffer. The remote desktop (`iyakki-remote`) owns a `Shell` per browser session at the browser's size.

## 2. The model

```
Display (output)  = one connected DRM connector with its mode, position in the arrangement, scale.
Workspace         = what one display shows: a Template (a tree of splits) and its Panes.
Pane              = a rectangle that hosts one app instance, or the launcher when it has none; has a title strip and a focus.
```

* A **Template** is a tree: a leaf is a pane, a split has a direction (side by side or stacked), a ratio (the first part's
  share) and an optional *link* id (splits with the same link move together: the 2x2 grid keeps both columns aligned).
* The **layout engine** (`crates/iyakki-shell/src/panes.rs`) is pure: rectangles in, rectangles out. It does splitting with a
  gap, minimum sizes, divider rectangles, divider drag with snapping, hit testing, focus order and neighbour search.
  Everything that draws or touches an app is in `shell_panes.rs`.
* The signed-in person's workspace lives in the `Shell` (`ws: Option<Workspace>`). `None` is today's behaviour and is the state
  of every machine until the person picks a layout. **The focused pane's content is the shell's own `screen`**, so every
  existing path (F-keys, help, context menu, inbox stash, Back, jump-to-app) keeps working on the focused pane unchanged. The
  other panes keep their content (app or launcher) inside the workspace and swap with the focused one on a focus change.

### Templates (ratios; the first number is the left or top part)

| # | Key | Name | Split | Panes |
|---|-----|------|-------|-------|
| 1 | Ctrl+Alt+1 | Single | one pane | 1 |
| 2 | Ctrl+Alt+2 | 50:50 side by side | 0.50 | 2 |
| 3 | Ctrl+Alt+3 | 50:50 stacked | 0.50 | 2 |
| 4 | Ctrl+Alt+4 | 30:70 | 0.30 | 2 |
| 5 | Ctrl+Alt+5 | 70:30 | 0.70 | 2 |
| 6 | Ctrl+Alt+6 | 33:33:33 | 1/3, then 1/2 | 3 |
| 7 | Ctrl+Alt+7 | 2x2 grid | 0.5 and 0.5 (linked) | 4 |
| 8 | Ctrl+Alt+8 | 70:30, right side stacked | 0.70, right 0.5 | 3 |
| 9 | Ctrl+Alt+9 | 60:40 | 0.60 | 2 |
| - | picker only | 25:75, 75:25, 40:60, 30:70 stacked, 70:30 stacked, 33:33:33 stacked | | 2-3 |

**Dividers.** Drag with the pointer, or Ctrl+Alt+Shift+arrows (5 % per press) for the divider next to the focused pane. Both
snap to the table ratios (25, 30, 33, 40, 50, 60, 67, 70, 75 %) when within 2 % of the display, and stop at the neighbours'
minimum sizes.

**Minimum sizes.** Each app has a minimum pane size from a table in `memgate.rs` (light apps 360x240, medium 480x300, heavy
640x400, in units before scaling; the table is data, not a trait method, so no app changes). A pane smaller than its app's
minimum does not draw the app: it shows a message ("CHAT needs at least 480x300; this pane is 320x540. Pick a bigger pane
or Layout") and the app keeps running. The divider drag stops at the minimum, so this only happens when a template is applied
to a small display. The picker greys templates whose smallest pane would be under 300x200 at that display size.

### Keys (no Super key; they exist on every keyboard; the remote desktop sends them too)

| Keys | Action |
|------|--------|
| Ctrl+Alt+Left / Right / Up / Down | Snap the focused app to the pane on that side (swap with that pane's content). In a single pane it first makes 50:50 (left/right side by side, up/down stacked) and puts the app on that side. With no pane on that side nothing happens (phase 2: the app goes to the display on that side). |
| Ctrl+Alt+1 ... 9 | Apply template N (the table above). Apps keep their order; see "Fewer panes". |
| Ctrl+Alt+Tab | Focus the next pane (Ctrl+Alt+Shift+Tab: previous). Plain Alt+Tab is left alone (browsers and desktops use it). |
| Ctrl+Alt+M | Move the focused app to the next display (phase 2; on one display a notice says so). |
| Ctrl+Alt+Shift+arrows | Move the divider next to the focused pane by 5 %. |

Conflicts checked: no code handled Alt before; no existing `Ctrl+letter` shortcut changes (Alt must be held too); F-keys are
untouched; Linux VT switching (Ctrl+Alt+F1..F12) is not used by IYAKKI. A browser running the remote desktop may keep
Ctrl+Alt+arrows for its own window manager: the LAYOUT button works everywhere.

**Fewer panes.** Applying a template with fewer panes keeps the apps of the first panes in order; the apps of the panes that
disappear are closed and a notice names them ("Closed for the smaller layout: CHAT, EDIT"). Apps keep their own saved state
(chats resume), nothing else is lost. Going to a layout with more panes opens launchers in the new panes.

### Mouse and touch

* Top bar: a **LAYOUT** button (grid icon) next to the clipboard button opens the template picker: a card with a preview of each
  template (small rectangles), the memory line, and (administrators) the switch.
* Each pane has a **title strip** with the app's icon and name and buttons: `Move`, `Layout`, `Back` (leave the app, show this
  pane's launcher), `Close` (remove the pane: the layout goes to a template with one pane fewer; the last pane cannot close).
* **No drag and drop of windows**: a click opens things on mouse-down in this shell. The owner accepted *pick up, then click the
  target* as the stand-in: `Move` on a strip picks the pane up (outlined; the others show "Put here"); a click on another
  pane swaps the two; Esc cancels. Phase 2 adds targets for the other displays.
* A click in a pane focuses it and acts on the control it hit; the focused pane has a coloured frame and a lit strip.
  This holds on every display: the first click on a display that was not the working one makes it the working display *and* focuses the pane under the pointer, then the control gets the click (F-SHELL-46; before, it reached the pane that had the focus there last time). Tests: `panes_tabs_tests` in `shell_panes_tabs_tests.rs` (tabs, keys, press, release, drag and wheel at four sizes, one and two displays).

### Focus, input and the number keys

Keys go to the focused pane. The launcher's `1`-`9` tile keys, F-keys that open apps (F7 Settings, F9 inbox) and Esc/Back act on
the focused pane. The wheel and pointer moves go to the pane under the pointer. `Event::Tick` goes to **every** pane's app so
background work (chat streaming, downloads) continues.

### Popups, cards and modals

Apps draw their own cards inside their `area`, and each pane has its **own `Gfx` surface**, so a card cannot leave its pane.
The shell-level things (context menu, F1 help, clipboard picker, the Devices popover, the picker) are drawn by the shell over
the whole working display (with several displays, only that one); the power card, sign-in dialog, alarm ring and screenshot are meant to cover the whole display. This is a
decision: it keeps the shell code that opens them unchanged. Pane-local versions of the shell-level popups are a later polish.

### Cost and drawing

Each pane renders into its own `Gfx` (a private pixmap of the pane's size plus that surface's glyph cache) **only when it is
dirty**: an event for it, a pointer crossing one of its hover regions, `App::wants_redraw`, or the same per-app conditions the
shell checks today (radio playing, calendar clock). Otherwise the cached surface is copied into the frame (a row copy). A test
counts redraws: typing in one pane renders one pane. The launcher in a pane is drawn without the big top bar (a title strip
instead) and its tile grid adapts through `launcher::page_rects` at the pane's size.

### Apps that want the whole screen

The web engine tile, the camera preview and full-screen video get the pane's rectangle like any app. The WPE helper supports
`Resize` (`docs/WEBVIEW-PROTOCOL.md`, message 0x07) and the browser already resizes to its `area`. Small panes are protected by
the minimum size. A "full display" toggle for video is not in phase 1.

## 3. Memory gate (owner requirement: "only if there is enough memory")

`crates/iyakki-shell/src/memgate.rs`: pure functions plus a reader that reuses `iyakki_agent::sysinfo::memory` (`MemTotal` and
`MemAvailable` of `/proc/meminfo`, as Settings -> Memory does). Swap and zram swap are **not** counted: zram-backed pages are
already inside the `MemTotal`/`MemAvailable` accounting (`docs/ZRAM-AND-INSTALL.md`). Missing or garbled numbers mean *unknown*
and give the conservative answer (single pane, single display).

`screen_features(mem, displays, wanted) -> Allowed { max_panes, max_displays, multi_display, reason, cost, budget }`

* **Tier by MemTotal** (a "4 GB" machine reports about 3.6-3.9 GiB, a "2 GB" one about 1.8-1.9 GiB, a "1 GB" one about 0.9 GiB):

  | MemTotal | panes | displays |
  |---|---|---|
  | unknown or under 1.5 GiB | 1 | 1 |
  | 1.5 to 2.5 GiB | 2 | 1 |
  | 2.5 to 3.5 GiB | 3 | 2 |
  | 3.5 GiB and more | 4 | 4 |

* **Budget by MemAvailable**: the screen features use at most 25 % of `MemAvailable`, and what is left must stay above 512 MiB
  (`MIN_FREE`): `budget = min(25 % of available, available - 512 MiB)`.
* **Cost** of a layout: pane surfaces = display area x 4 bytes (all panes together cover the display) plus 3 MiB per pane for
  the glyph cache and scratch; each extra display = width x height x 4 x 2 (double buffer) plus 3 MiB; plus one **app
  instance** per pane beyond the first by class: light 8 MiB, medium 40 MiB, heavy 200 MiB. Heavy: Browser (full engine), IDE,
  Camera, Data, Dash, Flows, Media, Voice. Medium: Chat, Rag, Edit, Mcp, Cli, Store, Skills, Tales, Scan, Mesh, Runs, System,
  Usage, Calendar. Light: the launcher, Settings, Users, Inbox, Radio and the rest. These are estimates from the code (frame
  buffers dominate light apps; engines and the IDE hold pages, indexes and surfaces); they are constants with tests, to be
  adjusted after measuring on hardware.
* If the cost of the wanted layout exceeds the budget, `max_panes` is lowered until it fits.
* Switch (per machine, administrators): **Automatic** (default), **Always on** (advanced; may be slow or run out of memory:
  tiers and budget are ignored) and **Off** (the shell behaves exactly as today). Stored in `screen-features` in the services
  directory.
* Re-evaluated every 10 seconds and when an app opens. A layout that no longer fits is **not** closed silently: a card says
  "Not enough free memory for this layout: switching to <smaller template>", the person's wish is remembered and the layout
  comes back when memory is free again. Opening an app into a pane while memory is low says "Low memory: close an app or use
  a smaller layout" and does not open it.
* The LAYOUT picker and Settings -> Displays show: `Layouts: available (4 GB, 2.9 GB free)` / `limited to 2 panes (1.8 GB)` /
  `single screen only (1 GB)` with the reason.
* Remote desktop sessions run the same gate per session (each has its own surfaces).

## 4. Several displays (phase 2)

fbdev cannot drive two pictures, so several displays need **KMS dumb buffers per output**. The code is in `display/kms.rs` (the
ioctls and their structs written from the kernel's `include/uapi/drm/drm_mode.h`, Linux 6.12, with size and offset assertions),
`display/outputs.rs` (the output manager, pure), `backend_kms.rs` (the display loop) and `shell_displays.rs` (one workspace per
display). No crate was added.

### When the shell drives the displays itself

Only when **all** of these hold, otherwise the framebuffer loop goes on exactly as before and **one log line** says why
(`shell: several displays are not driven by the shell: <why>; using the framebuffer`, also shown in Settings -> Displays):

1. two or more connectors are connected on one graphics card (read from `/sys/class/drm`, like the visible-size logic);
2. the memory gate allows several displays (section 3; Screen features is not Off; Always on forces it);
3. the shell does not run in a window (`DISPLAY` / `WAYLAND_DISPLAY` set means a desktop owns the screen);
4. `/dev/dri/cardN` opens, has dumb buffers, and **`SET_MASTER` succeeds** (it fails when another program is the DRM master, and
   then the shell does not take the screen);
5. a first plan can be shown (buffers and a mode on every CRTC).

A single display, a windowed shell, every test and every machine without a second monitor never touch `/dev/dri`. The
framebuffer path, the splash and `visible_size` are unchanged. `simpledrm` has one output, so such a machine stays on fbdev.

### The model

* **Plan** (`outputs::plan`): the connected connectors, the primary first (the person's choice, else the built-in panel `eDP`/
  `LVDS`/`DSI`, else the first by name), at the chosen resolution (highest refresh rate of that size), else the preferred mode.
  Each output gets its own CRTC (a matching, so an output keeps the CRTC it is on when it can); an output that cannot get one is
  left off with a note. Outputs beyond what the memory gate allows are left off with a note.
* **Extend**: the primary at (0, 0); others stack outward on the chosen side (right of, left of, above, below); the whole
  arrangement starts at (0, 0). **Mirror**: every output shows the primary's picture at the biggest resolution they share; if they
  share none, the displays are extended and a note says so.
* **Buffers**: two XRGB8888 dumb buffers per output (the pitch comes from `CREATE_DUMB`, never width x 4). The first frame sets the
  mode with `SETCRTC`; later frames are drawn into the back buffer and `PAGE_FLIP`ped; when a flip is refused (busy or unsupported)
  that frame uses `SETCRTC`, and after a failure the loop stops flipping. Tearing is possible then.
* **Pointer**: one pointer on the arrangement. Moves are clamped to the nearest point on an output, the display it is on is told
  to the shell, and the arrow is drawn into the buffer of that display only (software cursor, no cursor plane).
* **Scale**: each display lays itself out with `scale_for(w, h)` of its own size (the 2560x1600 panel about 1.3).
* **Hot-plug**: the connectors are read every second; a change plans again (a new output joins on the right, an unplugged one
  gives its apps to launcher panes on the displays that are left, and a notice names them). If the new plan cannot be shown, the
  old one is shown again.
* **Settings -> Displays** (administrators): the list of displays with resolution, main display, side, a small picture of the
  arrangement, Mirror/Extend, **Identify** (a big number on each display for 3 seconds), **Apply** and the **Screen features** switch
  with the memory line. Apply starts a trial: the old arrangement comes back after **15 seconds** unless **Keep** is pressed
  (**Go back** at once); an arrangement the card refuses is undone immediately. The kept arrangement is stored per person in
  `displays-<user>.json` with each display's layout template (which app is in which pane is not saved).
* **Shell-level pop-ups** (context menu, F1 help, clipboard picker, Devices, the layout picker, cards) are drawn on the working
  display only; the other displays keep showing their panes untouched.
* **Move an app to a display**: Ctrl+Alt+M (next display); Ctrl+Alt+arrow when the working display has no pane that way and another
  display lies that way (or the layout is a single pane); or **Move** on a pane strip, then click any pane on any display (the two
  swap). The keyboard follows the app.
* **The working display** is the one with the keyboard focus; a click on another display makes it the working one. The wheel goes
  to the pane under the pointer on any display. Heartbeats reach every app on every display.
* **Screenshots and the remote desktop** show the working display (the shell renders each display into its own surface; the
  remote desktop has one shell per browser session and one display). A display selector for `iyakki dev screenshot` is not built.

### Risks that remain

DRM master needs root (the shell runs as root on the image). A driver may refuse two active CRTCs together (the plan then leaves
one off with a note, or the trial reverts). Page flip timing and tearing are untested on hardware. The pitch and XRGB8888 are
handled but only the fake card proves it. Hot-plug is polled every second, not event driven. Taking the screen from the console
switches it to graphics mode for good (like the splash); if the shell stops, the console does not repaint by itself. The real code
path has never run.

## 5. Test plan

* Pure layout engine: all templates at several sizes: no overlap, full coverage, minimum sizes, dividers, snapping, focus order,
  neighbour search.
* Shell: headless rendering at 1280x720 with 50:50 and 30:70 showing both apps, no overlap and the strips; key routing; focus;
  Move; a card inside a pane stays inside; Back in a pane; Close; dirty-pane redraw count; remote sessions.
* Memory gate: totals x free x displays x resolutions; the reserve rule; app classes; garbled `/proc/meminfo` in a temp dir;
  the downgrade card; the Off and Always-on switches; the default at low memory is unchanged.
* Phase 2: struct size assertions for the ioctl structs, the fake backend (mode choice, extend, mirror, hot-plug), the output
  manager, the Displays page, apply with revert.

## 6. Owner checklist for a machine with an external monitor

Needed: the stick with this build, a MacBookPro14,1 (eDP-1 2560x1600) with an external monitor on HDMI or USB-C, then a Mac mini
with HDMI. Nothing here can damage the machine; the worst case is a black screen on the external monitor, and the 15-second
trial brings the old arrangement back.

1. Boot with the monitor **unplugged**. Sign in. Nothing looks different from before: no panes, no LAYOUT button unless the
   memory allows two panes (a Mac has plenty). Press **Ctrl+Alt+0**: the layout picker opens and shows the Memory line. Send the
   line.
2. Plug the monitor in. Within a few seconds the screen restarts and the monitor shows its **own** picture (not a copy). Each
   display has a thin bar and a pane with a title strip. If the monitor shows a copy, or nothing: press nothing, wait 10 s, then
   open Settings -> Displays (or send the log, step 9).
3. Move the pointer off the right edge of the panel: it appears on the monitor. Move it back. Click in a pane on the monitor and
   type: the keyboard goes there.
4. Open an app on the panel, press **Ctrl+Alt+M**: it moves to the monitor and the keyboard follows. Press **Ctrl+Alt+Left**:
   it comes back (the monitor is on the right of the panel). Try **Move** on a pane strip, then click a pane on the other display.
5. Settings -> Displays: press **i** (Identify): a big 1 and 2 appear for 3 seconds. Press **Down**, then **r** to choose another
   resolution for the monitor, **y** to apply: the picture changes and a yellow box counts down 15 seconds. Do **not** press
   anything the first time: the old picture must come back by itself. Repeat and press **k**: it stays.
6. Press **a** (side) to put the monitor below or left of the panel, **y**, **k**; check that the pointer follows the new
   arrangement. Press **m** then **y**: mirror (or a note that the two share no resolution).
7. Unplug the monitor while an app runs on it: the app should appear in a pane of the panel (a notice names it). Plug it in again:
   its layout is back.
8. Choose a layout on the monitor (Ctrl+Alt+7, 2x2), sign out and in: the layout is remembered. The apps are not.
9. **Send back**: what you saw at each step (especially 2, 5 and 7); the Memory line from step 1; a photo of Settings ->
   Displays; and the log lines that start with `shell:` (`journalctl` or the console log: the line `shell: several displays driven
   by the shell through /dev/dri/card0: ...` or `shell: several displays are not driven by the shell: <why>`); `ls /sys/class/drm`
   and `cat /sys/class/drm/card*-*/status` and `modes`.
10. If something is wrong: Settings -> Displays, **s** until *Off* (the shell then uses the framebuffer and one picture), or boot
    with the monitor unplugged.

Known limits for this test: tearing is possible; the pointer is a software arrow; a USB-C monitor behind a dock may appear as a
different connector name; the Mac mini 2018 has not been tried with two monitors.
