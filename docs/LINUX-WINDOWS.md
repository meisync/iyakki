# Linux windows: a graphical Linux program in an IYAKKI window

Status: **built and tested with fakes** (a fake display server that speaks RFB, a fake sandbox, a fake `/proc`) and **run once for real on a
development PC** (section 8): the real Debian packages, the real TigerVNC `Xvnc` and xterm, and this client. Not yet tried on an IYAKKI device,
and the tile itself was only driven against fakes. This is the minimal "phase 2" of [`LINUX-APPS.md`](LINUX-APPS.md) (section 11).

## 1. What it is, in plain words

IYAKKI itself is small and has no Linux desktop programs. The Linux apps sandbox (a Debian inside a folder) can run them, but a
graphical program needs a screen. **Linux windows** gives it one: inside the sandbox a display server draws into memory instead
of onto a monitor, and the **LINUX WINDOW** tile (System section of the home screen) shows that picture, sends your keys and
mouse to the program, and shows how much processor and memory the sandbox uses.

Nothing of Debian, the display server or the programs is shipped in an IYAKKI image: `apt` inside the sandbox downloads them at
your click, and each package keeps its own licence.

## 2. Install (one click)

Store, **Setup**, group **Linux apps**, card **Linux windows (graphical programs)**, or the **Install Linux windows** button
inside the tile. It makes a Debian 12 sandbox called `gui` (if there is none) and installs with `apt`: TigerVNC's X server
(`tigervnc-standalone-server`, it draws into memory and speaks RFB), `matchbox-window-manager` (one maximised window, no
decoration), `fonts-dejavu-core`, `libgl1-mesa-dri` (software OpenGL) and `xterm` as a first test program. About 250 MB, needs the
network. When it is done the sandbox's network is switched **off** again (a sandbox made by this job); a program that needs the
internet gets it on the Linux apps page (the tile then says "network on" in its strip). Only an administrator can install.

The tile needs the **full command-line permission** (administrators and operators have it), like the Linux apps page, because
running a program of your choosing is what a shell is. A sandbox with its network on can reach any server, so only an account
allowed every host may start a window in it.

## 3. Use

1. Open **LINUX WINDOW**. The list shows the sandbox's graphical programs (read from the sandbox's `.desktop` files, see section 6).
2. Choose one (click, or Up/Down) and press **Start** (or Enter). It takes a few seconds.
3. The window fills the tile. Click into it to use it; **keys, mouse buttons, the wheel and drags** go to the program. Typing with
   Shift, Ctrl, the arrow keys, F-keys and held keys (repeat) work. There is no Alt key yet (the shell does not pass it on).
4. The strip above the window shows the program, the size, **CPU %**, **memory** (and its peak) of the sandbox's processes and
   whether the network is on. **Stop** (or closing the tile) ends the program and every process of the sandbox.
5. **Install xterm / x11-apps / mousepad** (buttons under the list) add a small test program with `apt` (an administrator's click).
   Other packages: the Linux apps page, Install.

It works full-screen and in split panes. When you change the size of the tile or pane, the screen of the program follows after a
moment (the display server supports RFB's *Extended Desktop Size*).

## 4. The clipboard

Nothing is shared between IYAKKI and the program by itself. Two buttons in the strip, and an explicit paste, do it, **as text only**:

* **Paste into window**: your latest IYAKKI clipboard text is put on the program's clipboard (control characters other than
  newline and tab are removed, at most 64 KB, ISO 8859-1: other characters become `?`) and Ctrl+V is pressed in the program.
* **Copy from window**: the text the program last put on its clipboard becomes your IYAKKI clipboard.

Ctrl+C and Ctrl+V typed in the window are ordinary keys for the program (its own clipboard). Pictures and files are not bridged.

## 5. How it works (for developers)

```
 tile (iyakki-shell apps/linuxwin.rs)  --RFB over a Unix socket-->  Xvnc  <--  matchbox  <--  the program
        iyakki-rfb (own client)             <data>/linuxapps/<user>/gui/run/display.sock  (inside: /mnt/iyakki-display/display.sock)
```

* `iyakki-rfb` (no dependencies): the protocol as a state machine without I/O (`Session`), a threaded `Link` for a Unix socket, an
  own DEFLATE decoder for ZRLE (`inflate.rs`). Encodings: Raw, CopyRect, Hextile, ZRLE; pseudo-encodings DesktopSize and
  Extended Desktop Size. Security type None only (the socket is private). Pixel format is fixed: 32 bits, depth 24, little endian.
* Bounds: screen at most 8192 per side and 4096 x 4096 pixels, clipboard 1 MiB, one ZRLE payload 64 MiB (and the decompressed size
  of a rectangle at most 4 bytes a pixel plus tile overhead), at most 1024 queued events (updates are merged). A rectangle outside
  the screen, an unknown encoding or a bad stream closes the session with an error; nothing panics (fuzz-style tests with random
  and corrupted streams, and byte-by-byte feeding).
* `linuxwin.rs` (agent): installs, builds the argument array, reads the launcher, starts and stops the window, reads cpu and memory.
  The sandbox command is `iyakki linuxapp-init ... --bind-rw <run folder> /mnt/iyakki-display -- /usr/local/bin/iyakki-window-session
  <width> <height> <program words...>` (an argument array, no shell on the host). The script is fixed text written by IYAKKI into the
  sandbox; sizes must be digits, the program words are passed as separate arguments (`"$@"`).
* The display runs as `Xvnc :73 -geometry WxH -depth 24 -rfbunixpath <socket> -rfbport -1 -SecurityTypes None -nolisten tcp -nolisten local -auth <file>`:
  no TCP listener, no abstract X socket and only clients that know the secret key (`xauth`, made fresh by the script in the sandbox's
  private `/tmp`). The sandbox with its network on shares the host's network namespace and so its abstract sockets, where a desktop's X
  server may already be `:0` or `:1`; hence the unusual display number. Only the RFB socket in the runtime folder, which is mode 0700 and
  owned by the sandbox user, is a way in from outside.
* Closing: `Window::stop` kills the process IYAKKI started; the sandbox's parent-death signal and its pid namespace end every
  process inside. Dropping the tile does the same.
* CPU and memory: the processes below the started one are found in `/proc` (parent links), resident memory from `VmRSS`, cpu from
  `utime + stime` ticks; the strip shows the change per second and the peak. There are no cgroup figures (non-root runs have no limits).
* Audit: `window.install.start`, `window.install`, `window.install.failed`, `window.start` (program name and argument count, size),
  `window.stop` in `<data>/linuxapps/audit.log`; the install job and button also write `quickinstall.start`.
* Why no GPU, no audio, no Chromium sandbox changes: see section 7.

## 6. The launcher

`linuxwin::programs` reads `usr/share/applications/*.desktop` of the sandbox's root with the strict reader rules: no link on the
way (a link to a host file is ignored, a linked `applications` folder is ignored), at most 400 files, 64 KiB each. Only the
`[Desktop Entry]` group is read: `Type=Application`, not `NoDisplay`/`Hidden`, not `Terminal=true`, with a `Name` and an `Exec`.
`Exec` is split into words by the Desktop Entry quoting rules and the field codes (`%f`, `%U`, ...) are dropped; nothing is
expanded and no shell runs it. The program is started with `DISPLAY=:1` (set by the session script).

## 7. Limits (honest)

* **No graphics card.** Drawing is by the processor (Mesa llvmpipe for OpenGL, `LIBGL_ALWAYS_SOFTWARE=1`). Video, WebGL and games are slow.
* **No sound** in the window yet (no PulseAudio or PipeWire socket).
* The picture travels over RFB with ZRLE or Hextile: fine for tools, editors and terminals; smooth video is not a goal.
* One window (one program) at a time per tile; the program's own pop-up windows are placed by matchbox as maximised windows.
* No Alt key, no key release timing (a key is pressed and released at once), no right-to-left or compose input; the keymap is the
  display server's.
* The clipboard is text only and only by the buttons (section 4).
* A browser such as Brave is glibc and Chromium based and cannot run natively on IYAKKI (musl); it could run in this window later.
  It is **not** part of this work, and Chromium's own sandbox would need `--no-sandbox` because the sandbox's system-call filter
  refuses the namespaces it wants. Not decided, not built.
* Wine (Windows programs) would come on top of this and is a separate task.
* Everything of the sandbox's security model (`LINUX-APPS.md` section 4) applies; the program inside can use the sandbox's network if
  you switched it on, and can reach services on the machine's loopback then.

## 8. What was tested, and what was not

Tested with fakes: see `crates/iyakki-rfb/tests/session.rs` (handshake versions, raw, CopyRect, Hextile, ZRLE of every tile kind over
one zlib stream, byte-by-byte feeding, out-of-bounds rectangles, bad ZRLE, a decompression bomb, resize through both
pseudo-encodings and the bounds of a resize request, input messages, 4000 corrupted streams and 2000 random starts, the decoder
against a real compressor, the threaded link over a socket), `crates/iyakki-agent/src/linuxwin_tests.rs` (script, command, `.desktop`
reading with links and size limits, install with a fake runner, the golden argument array with only the sandbox folder and the
runtime folder as host paths, start and stop with a fake spawner, audit, cpu and memory from a fake `/proc`) and
`crates/iyakki-shell/src/apps/linuxwin_tests.rs` (key mapping, geometry at 360x740, 800x480, 1280x720, 1920x1080 and 2560x1600 and in
panes, the whole path against a fake sandbox that serves RFB on the real socket path, resize, the explicit clipboard, role gating,
the memory strip, closing the tile).

**Real run (development PC, normal user, network on; the ignored test `crates/iyakki-rfb/tests/real_window.rs`):** a real Debian 12 sandbox
was downloaded (26 MB, checked) and the display packages installed with `apt` (`tigervnc-standalone-server` 1.12.0, matchbox, xterm,
fonts, Mesa, xauth). Then `Window::start` ran the session script in the sandbox, `Xvnc` listened on the runtime folder's socket, IYAKKI's
RFB client connected (RFB 3.8, security None, ZRLE), got the 800x500 screen, saw non-black pixels once xterm was up, asked for 640x400
through Extended Desktop Size and got it, typed into xterm and saw the screen change, read 7 processes and 94 MB below the started process,
and after `stop` no process of the sandbox was left. Every sandbox it made was deleted.

What the real run found and fixed (none of it showed in the fake tests):

* **`/usr/bin/Xvnc` is a link to `/etc/alternatives/Xvnc`**, an absolute link that dangles when seen from the host: the "installed" check followed it and said "not installed". It now counts a link without following it.
* **A sandbox with its network on shares the host's abstract X sockets.** `DISPLAY=:1` made the sandbox's X clients try the host desktop's `:1` first and hang. The display is now `:73` (not one a desktop uses), the server does not listen on the abstract socket (`-nolisten local`), and it only takes clients that know a secret key (`xauth`, a random 16-byte cookie made by the script in the sandbox's private `/tmp`).
* **xterm needs a pseudo-terminal** and the sandbox had no `/dev/pts`. The sandbox now mounts its own `devpts` instance (best effort; `docs/LINUX-APPS.md`).
* Debian's launcher names are `debian-xterm.desktop` (a menu entry), not `xterm.desktop`; the list shows them by `Name`.
* xterm wanted the core font set: `xfonts-base` was added to the install list.

**Not tried for real:** the tile itself (drawing, keys, pointer, the status strip) against a real display; an IYAKKI device or image; the Setup
job and the tile's install buttons (they use the same `install` function, which the run exercised, but not through the job engine);
root with the wide id range (this PC runs the sandbox as a normal user, so the sandbox has **one id**, and packages whose scripts change
owners fail to install there: `libutempter0` and `fontconfig-config` did, until a **test-only** shim made `chown` a no-op inside the
throw-away sandbox; on a device, which runs as root, this is not expected); `mousepad` and `x11-apps`; a second window; a big
screen (2560x1600) against a real server; sound and GPU (not built).
