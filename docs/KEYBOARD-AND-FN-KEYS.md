# Keyboard and function keys

Volume, brightness, keyboard light, media, WiFi/Bluetooth and power keys work on the IYAKKI screen itself. The shell reads the
keys straight from the input devices (evdev), acts on them, and shows a small bar at the bottom centre of the screen for about
a second and a half. The bar never takes focus: whatever app is open (the web browser included) keeps the keyboard, and the key
is **not** passed to it. A page cannot see these keys.

Status: **built and tested against made-up machines; not yet tried on a real Mac or laptop.** The checklists at the end are for
that. Rows `F-KBD-01` to `F-KBD-09` in `FEATURES.md` say what is partial and what is planned.

## What the keys do

| Key (evdev code) | What happens |
|---|---|
| Volume up / down (115 / 114) | IYAKKI's volume +/- 5 %; hold to repeat (about 10 steps a second after a short delay). A step also unmutes. |
| Mute (113) | Mutes or unmutes IYAKKI's volume. The level is remembered. |
| Microphone mute (248) | A notice only: "Microphone mute is not available yet". |
| Brightness up / down (225 / 224) | Screen backlight, 16 even steps, never fully dark. |
| Auto brightness (244) | A notice: not supported. |
| Keyboard light up / down / toggle (230 / 229 / 228) | Keyboard backlight; steps include off; toggle returns to the previous level. |
| Play/pause (164, 200, 201, 207) | In the Radio app or the media player: play or pause. Elsewhere: the radio, if it plays in the background. |
| Next / previous (163 / 165) | Radio: next or previous station. Media player: 30 seconds on or back. |
| Stop (166) | Stops the radio, or closes the media player. |
| Power, Sleep (116, 142, 205) | Opens a card with **Power off / Restart / Cancel**. Nothing happens until you choose. The key again, Esc or any other key closes it. Sleep is not built yet (see known gaps). |
| WiFi, Bluetooth, Airplane (238, 237, 247) | Switches those radios and says what it did ("WiFi off", "Airplane mode on"). If the kernel switches by itself first, IYAKKI only reports it; it never switches twice. A hardware switch is reported, not overridden. |
| Switch video mode (227) | A notice. |
| Print (210) | Starts the screenshot (the same as F12). |
| Screen lock (152), Super+L on a PC keyboard, and Control+Command+Q or Control+Command+L in the Mac profile | Locks the screen: a password prompt covers the desktop, the apps keep running ([the lock screen](SCREEN-SAVER.md#the-lock-screen)). |
| Calculator (140) | A notice: there is no calculator app. |
| Home page (172) | From the launcher: opens the browser. |

The speaker icon in the home screen's top bar moves the same volume with the mouse: a click mutes or unmutes, the wheel over it changes it
5 % a notch, pressing and holding (or dragging) shows a slider, and `v` on the home screen opens the slider for the keyboard (Up/Down, `m` mutes,
Esc closes). Each change shows the same volume bar as the keys.

The same table is in Settings, Keyboard (scroll with Up/Down). Without a sound output the volume keys say "No sound output".
Without a backlight they say "No brightness control on this display". IYAKKI has no software dimming.

Volume goes through the same setting as Settings, Sound and the Devices popover in the top bar, so all three stay in step. With a
software volume (HDMI, the T2 Mac speaker) it is the ALSA `IYAKKI` control; every program that plays through IYAKKI follows it.

## Ctrl+Alt chords (layouts)

Ctrl+Alt with an arrow, a digit, Tab or M is the shell's, not an app's: Ctrl+Alt+1 to 9 apply a pane layout, Ctrl+Alt+0 opens the
layout picker, Ctrl+Alt+arrows snap the open app to a side, Ctrl+Alt+Tab changes pane, Ctrl+Alt+Shift+arrows move a divider and
Ctrl+Alt+M moves the open app to the next display (with several displays). Ctrl alone and Alt alone work as before. See
`docs/MULTI-DISPLAY.md`.

## Settings, Keyboard (administrators)

* **Function keys**: *media keys first* (Fn + F1..F12 gives F1..F12), *F-keys first* (Fn + F-key gives the media key),
  *automatic* (the keyboard decides) or *not changed*. For Apple keyboards this writes `hid_apple` `fnmode`
  (`/sys/module/hid_apple/parameters/fnmode`: 1 media first, 2 F-keys first, 3 automatic) now, and again at every start. On a
  MacBook Pro 2019 it also sets the Touch Bar's own mode (`hid-appletb-kbd` `mode`: 2 media/brightness keys, 1 function keys).
  If the driver loads later the choice is applied then (checked every ten seconds). Kept in the settings file (`keyboard`).
* **ISO layout** and **Option/Command swap** (`iso_layout`, `swap_opt_cmd`): off / on / not changed. A keyboard that is already
  plugged in may need to be plugged in again to pick up a change.
* **Key repeat**: delay (200 to 1000 ms) and rate (10 to 40 per second), written to every keyboard with the kernel's `EVIOCSREP`
  and to keyboards plugged in later. Also the delay and rate of held volume and brightness keys. "Kernel's own" is 250 ms and 30 per second.
* **Scrolling** (wheel and trackpad, every tile and the browser; `crates/iyakki-shell/src/scrolleng.rs`). One engine in the input
  thread turns raw wheel, hi-res wheel and two-finger movement into pixel steps. **Scroll speed** `s` (50 to 300 %, default 100),
  **Wheel lines** `y` (lines per click, 1 to 10, default 3), **Wheel direction** `w` (traditional by default), **Trackpad direction**
  `n` (natural by default), **Smooth scrolling** `g` (on: a wheel click eases to its target in about 90 to 140 ms and never passes it),
  **Momentum** `m` (on: a trackpad scroll keeps going after the fingers lift, slowing exponentially; a new touch or the end of the
  list stops it). Saved under `keyboard` (`scroll_speed_pct`, `wheel_lines`, `wheel_natural`, `trackpad_natural`, `scroll_smooth`,
  `scroll_momentum`; also `iyakki dev config`). The **Scroll test** list on the page is a tall list to feel the choices on.
  While a scroll goes on the shell draws up to 60 pictures a second; it returns to its low idle rate when the scroll settles.
* **Long lists of rows** (the Settings pages Network, Bluetooth, Display, Sound, System sounds and Screen saver; `crates/iyakki-shell/src/listwindow.rs`).
  A click chooses the row where it is and the list does not move, so the next click lands on the row it shows. A key that moves the choice
  (Up, Down, Home, End) scrolls only as far as needed to show it. The wheel scrolls the rows (3 a click) and leaves the choice where it is;
  the next key brings the list back to the choice.
  Tiles that scroll by pixels (Settings lists, the chat conversation, DISKS, the page list, the plain browser, the Scroll test) move
  smoothly; other tiles receive the same movement as whole wheel clicks. The browser's full engine receives each step once,
  as a fraction of a click (the protocol counts 1/120), un-eased, so its own smooth scrolling is not doubled.
* **Layout**: there is one layout, US. Other layouts are planned.
* **Detected controls**: the screen backlight that the keys use (and other backlights), the keyboard light, the Apple driver and
  Touch Bar state, the sound state. Test buttons: **Test screen** (dims for a moment and restores), **Test keyboard light**
  (off and on), **Test sound** (the chime).
* **Last special keys**: press a media or Fn key and its code appears here, with what IYAKKI does with it. A key IYAKKI does not
  use shows as "a key IYAKKI does not use": that is the code to report.
* **Table of recognised keys.**

## Input profile: Mac or PC (F-KBD-19)

Settings, Keyboard, **Input profile** (button `v`) chooses how the shortcut keys behave. **Automatic** (the default) uses Mac when the
kernel lists an Apple keyboard or trackpad (USB, Bluetooth, the T2 and SPI MacBooks; the Touch Bar and a Magic Mouse do not count),
and PC otherwise. **Mac** and **PC** force it. The choice is saved as `keyboard.profile` (`""`, `mac`, `pc`; a file from before the
profile loads as Automatic) and works at once. `iyakki dev config keyboard.profile mac` sets it from a paired computer.

| Mac profile | What happens |
|---|---|
| Command+C / X / V / A / Z / F / S / W / T / N / L / R | The shortcut the PC's Ctrl+letter is. Command+Shift+Z is redo. |
| Command+Left / Right | Start / end of the line (Home / End; with Shift it selects). |
| Command+Up / Down | Top / bottom of the document (Ctrl+Home / End). |
| Option+Left / Right | Word jump. Option+Backspace deletes the word before; Command+Backspace deletes to the line start. |
| Command+Tab | The next pane (the same as Ctrl+Alt+Tab; Shift goes back). |
| Command+Q, Control+Command+Q | Nothing. IYAKKI never quits on a key and has no lock shortcut. |
| Control+C | Stays Control+C. |
| Control+click | A right click (a two-finger tap on a trackpad already is one). |

The PC profile is exactly what it was before. **Option/Command swap** (`hid_apple` `swap_opt_cmd`) is the kernel's swap on an Apple keyboard.
In the Mac profile the keys are named by what is printed on them, so the swap changes nothing there and the two never swap twice; in the
PC profile the swap is the kernel's alone (the key under your thumb sends Alt). The swap counts only while an Apple keyboard is present.

**Reset to profile defaults** (`x`) puts the pointer choices back: trackpad tap on, trackpad natural, mouse wheel classic, speed 100 %,
smooth scrolling and momentum on, 3 lines a click. Both profiles start from the same values today. Key repeat, Touch Bar and the Fn mode are left alone.

Under every control the page shows a line of what it does with an example, and a **What you will see** pair of rows: the last key
(for example `Command+C -> Copy`, `Control+C -> Copy`, `Command+Q -> Quit (ignored ...)`) and the last scroll
(`Scrolled content up (natural, trackpad)`), so a confused person can see the mapping. Three-finger drag is not built.
Known gap: Command+C and Control+C both arrive in an app as Ctrl+C; a terminal program inside a Linux window therefore sees an
interrupt for Command+C (no per-window rule yet).

## Which backlight

`/sys/class/backlight/*`, best first: `intel_backlight`, `amdgpu_bl*`, `radeon_bl*`, `nvidia*`/`nv_backlight`, `apple-panel-bl`,
`gmux_backlight`, then `acpi_video*`. One is used. The Touch Bar's own backlight (`appletb_backlight`) is never treated as the
screen. Keyboard lights: `/sys/class/leds/*kbd_backlight*` (`smc::kbd_backlight` on Macs, `tpacpi::`, `asus::`, ...).
The shell runs as root on the IYAKKI image and writes the files directly, as Settings, Display already does. No setuid program is
added and no program is started for any of these keys. If a write is refused the bar says "Not allowed to change the brightness (needs root)".

## Per-machine notes

* **Mac mini 2018 + Apple or USB keyboard**: volume and mute keys work if the keyboard sends them (Apple Magic/Wired keyboards do).
  Sound goes to the HDMI monitor or the T2 speaker. **No brightness keys do anything**: the HDMI monitor has no backlight file
  (the bar says so). DDC/CI is planned. An Apple keyboard on `hid-apple` gets the Function keys choice.
* **MacBook Pro 2019**: built-in keyboard (T2) with `hid-apple`, panel backlight (`intel_backlight` or `amdgpu_bl*` through the
  display driver), keyboard light (`smc::kbd_backlight`, needs `applesmc`; the image has it). Touch Bar: see known gaps.
* **Ordinary PC or laptop**: volume keys work where there is a sound output; brightness through `intel_backlight`, `amdgpu_bl*`
  or `acpi_video*`; the WiFi key may be handled by the kernel (`rfkill-input`) and is then only reported.
* Volume and media keys often sit on a device node of their own ("Consumer Control", "Video Bus", "Power Button"). IYAKKI reads
  every input device that has any of the keys above, not only typing keyboards. If two devices report one press, it counts once.

## How to find your key codes

Open Settings, Keyboard and press the key: its code and what IYAKKI does with it appear under "Last special keys". On the
hardware check screen the live tester shows the keys too. `iyakki hwreport` lists the input devices. If a key does nothing and
the page does not list it, the kernel is not sending a code for it (some laptops need a driver, e.g. `thinkpad_acpi`, `asus_wmi`).

## Known gaps

* **Touch Bar (MacBook Pro 2019)**: the image kernel has the Touch Bar keyboard-mode and backlight drivers (`hid-appletb-kbd`,
  `hid-appletb-bl`, built as modules, backported to 6.12) and the Apple USB bridge, so the Touch Bar can show the function row
  or the media/brightness row, and its keys arrive as the key codes above. It has **no display driver** (`appletbdrm`), so
  there are no custom buttons and no icons beyond what the firmware draws. Whether the Touch Bar comes up on a real MacBook is not
  tried. No kernel option was added for this work.
* **External monitors by DDC/CI** (`/dev/i2c-*`, VCP 0x10): **planned, not built.** It needs `i2c-dev` in the kernel (a
  kernel rebuild you schedule), read-only probing first, and writes only to a monitor the person selected. Many monitors do not
  support it. The Mac mini's HDMI monitor is the likely first test.
* **Sleep**: the Sleep and Power keys open the power card, which has Power off and Restart only. Suspend to RAM is not built
  (it is unreliable on T2 Macs).
* **Microphone mute**, **automatic brightness**: notices only.
* **Layouts** other than US.
* A web page cannot ask for these keys (the "unless the page asks" case): the shell always takes them.
* The kernel's own key repeat is ignored for these keys; the shell repeats volume and brightness itself, so a device that does
  not repeat still works. If a release event is lost the repeat stops by itself after ten seconds.

## Real-device checklist

Mac mini 2018 (needs the rebuilt image; the keyboard tests below use the USB or Apple keyboard):

1. Boot, sign in as an administrator, open Settings, Keyboard. The rows say "Apple keyboard driver: loaded, fnmode N" (Apple
   keyboard) or "not loaded" (other keyboards), and "Screen brightness: none".
2. Press Volume up. The bar appears bottom centre for about 1.5 s with the level; Settings, Sound shows the same level.
3. Hold Volume down: it falls in steps of 5 % about ten times a second and stops when you let go. Open the browser and repeat:
   the page must not react.
4. Mute: the bar shows "Muted" with the level kept; the Devices popover (F8) also says muted. Press again: back at the same level.
5. Play the radio (Radio app, then leave it). Press Play/pause: the radio pauses, again: it resumes. Stop: it stops.
6. With the monitor unplugged (no speaker), press Volume up: "No sound output".
7. Press Brightness up with the HDMI monitor: "No brightness control on this display". Check `ls /sys/class/backlight` (hwreport): empty.
8. Settings, Keyboard: press `f` until "F-keys first". Press F1 on an Apple keyboard: it must act as F1 (the F1 key opens help),
   not as brightness. Press `f` to "media keys first": F1 acts as a media key and Fn+F1 as F1. Restart: the choice holds.
9. Press `d` and `r` and type with a key held: the repeat follows. Back to "kernel's own" afterwards.
10. Press the Power key if the keyboard has one: the card opens and nothing shuts down; Esc closes it.

Scrolling (any machine with a wheel or a trackpad): in Settings, Keyboard, put the pointer on the **Scroll test** list.
(a) Roll the wheel one click: about three rows, eased, no jump. (b) `s` to 300 %, again: farther; to 50 %: shorter. (c) `w`: the
direction flips. (d) `g` off: it jumps again. (e) Two-finger scroll on a trackpad: the list follows the fingers; lift while moving:
it coasts and stops; touch to stop it; `m` off: no coasting. (f) Open YouTube or a long page in the browser: the wheel and two
fingers move it smoothly, once, with no second easing. (g) Scroll a list to its end with momentum: it stops at the end.

MacBook Pro 2019:

1. Settings, Keyboard: "Screen brightness: intel_backlight (or amdgpu_bl0)", "Keyboard light: smc::kbd_backlight", Touch Bar
   "1 device(s)" or "not found". Write down which.
2. Brightness keys (Touch Bar or F1/F2): the bar shows, the panel changes in about 16 steps; the lowest step is dim but not black.
3. Press Test screen: the panel dims for a moment and returns. Press Test keyboard light: the light goes off and on.
4. Keyboard light keys: steps, and the toggle returns to the same level.
5. Function keys choice (item 8 above) with the built-in keyboard; note whether the Touch Bar shows the function row in "F-keys first".
6. Volume keys: the T2 speaker follows (Settings, Sound shows "Apple T2 Audio" in use).
7. Press the WiFi key if there is one (Mac keyboards have none): or use Airplane via Settings, Keyboard's last-key list to see the code.
8. Close the lid and open it, press a volume key: still works (sleep is not built; nothing should change).
9. Press every unusual key and send the codes shown under "Last special keys" for any that are not listed.

## For developers

`crates/iyakki-shell/src/fnkeys.rs` (key map, curves, sysfs writers, OSD drawing), `shell_fnkeys.rs` (what the shell does),
`input.rs` (`Event::Fn`, device selection, `EVIOCSREP`), `apps/settings_keyboard.rs` (the page),
`crates/iyakki-agent/src/keyboard.rs` (settings and the `hid_apple` writer). Tests: `fnkeys_tests.rs`, `shell_fnkeys_tests.rs`,
`apps/settings_keyboard_tests.rs`, `keyboard::tests`. Every path hangs off a `sys` root so tests use a temporary directory.
