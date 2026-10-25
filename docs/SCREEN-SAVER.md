# Screen saver and screen off

What the screen does when nobody has touched the machine for a while: show a calm screen saver (the IYAKKI logo drifting on
black), dim, and then turn the screen off. Everything is a choice; nothing here stops sound, downloads, installs or the assistant's
work. Status: built and tested on fake clocks, fake folders and a fake framebuffer call; **not yet seen on a real panel** (see
[the device test steps](#device-test-steps)).

Rows: F-SCRN-01 to F-SCRN-06 in [FEATURES.md](../FEATURES.md). Where the code is: `crates/iyakki-shell/src/saver.rs` (the idle clock and the
animation), `shell_saver.rs` (the shell's side), `panel.rs` (backlight and the framebuffer's blank call), `apps/settings_screensaver.rs`
(the page), `crates/iyakki-agent/src/screensaver.rs` (the choices).

## The choices

Settings -> Display & sound -> **Screen saver**. Everyone changes their own; they are kept in your own file and follow you to any
screen you sign in on. A guest uses the standard choices (and can still preview). The page has one card per choice, with the choices
as buttons (click one, or Up/Down to pick a card and Enter for the next choice, `N` for the previous, `p` to preview).

| Choice | Offered | Standard |
|---|---|---|
| When the screen is idle | Do nothing, Screen saver, Turn the screen off | Screen saver |
| Idle for | Never, 1, 2, 5, 10, 15, 30 minutes | 10 minutes |
| Then turn the screen off after | Not at all, 1, 5, 10, 30 minutes (more minutes after the saver started; only with the saver) | 10 minutes |
| Screen saver style | IYAKKI logo, Logo and clock, Small logo (OLED friendly), Breathing logo | IYAKKI logo |
| Dim the screen first | Do not dim, 50 %, 25 %, 10 % of the brightness you had | 50 % |
| Shorter on battery | On (every time above is halved while on the battery, never below 15 seconds) or Off | On |
| Try it now | Preview: the saver for 5 seconds, or until a key, a click or a movement | |

**The sign-in screen** has no person yet, so an administrator sets it (the last three cards of an administrator's page, "Sign-in
screen"): it starts as the saver after 5 minutes and the screen off 10 minutes after that. It is kept in the machine's settings file.

Why these standards: a saver with the logo is safe (black background, the picture always moves) and cheap; turning the screen off
after 10 more minutes saves power and the panel. Laptops dim to half first. Both are one click to change, and "Never" turns the
whole thing off.

## What keeps the screen on

Any input restarts the idle time: a key, the pointer moving, a click, the wheel, a touch, the Touch Bar, input a paired computer sends
in Developer mode, input from a remote (browser) screen. And the screen is held on, as if you were using it, while:

- the radio or the media player is playing, or a tale is being read aloud (the app says so);
- an approval is waiting, an alarm is ringing, or a screenshot, the sign-in dialog of an AI tool or the power card is open;
- the low-battery card or its countdown is up, or the drive-loss banner is showing;
- the hardware check or the first-run setup is on screen;
- something called `Shell::inhibit_screen_off(reason, seconds)` (it returns a token; `release_screen_off(token)` ends it, and it expires by
  itself). Nothing in the shell uses this for installs and downloads: only what a person is watching or must answer holds the screen.

If one of these appears while the saver or the black screen is up (an approval card, the drive banner), the screen wakes by itself.
Sound is never paused by the saver or by the screen going off.

Not detected yet: video playing inside the web browser (the web engine does not tell the shell whether a page plays media), so
watching a video in the browser without touching anything lets the saver start. Move the pointer now and then, or choose Never.

## Waking

- Any key, click, wheel turn, touch or pointer movement wakes the screen.
- **The first key or click only wakes**: it is not delivered to the app, so a stray click does not press a button. Pointer movement
  just wakes. The release of a click that woke the screen is dropped too. (A key that is held down still repeats into the app.)
- The Touch Bar, Developer-mode input and the power button wake it too; Developer-mode input also reaches the app.
- Waking restores the backlight first, then unblanks the screen, then draws the whole desktop once.
- While the screen was only dimmed (before it goes off with nothing shown first) the first input is delivered normally: you could see the
  desktop.

## The screen saver

The real IYAKKI logo (the one of the boot screen), on pure black:

- it drifts slowly across the whole screen like a bouncing logo, but with easing: at a turn it still moves, so the picture never rests
  (this is also what protects against burn-in); it crosses the screen in about 38 seconds at most and never faster;
- it breathes (5 %), and its glow pulses slowly through the theme's accent colours (blue, teal, purple, cyan);
- "Logo and clock" adds the time under it, "Small logo" is a small dim logo without glow, "Breathing logo" hardly drifts.

Cost: the saver draws 15 frames a second on mains, 8 on battery and 4 on a remote (browser) screen, and each frame repaints only the
rectangle the logo left and the one it moved to (a few percent of a 2880x1800 screen), never the whole picture. The backend does
not redraw the saver by itself. The log line `shell: N frames in 10s, draw X ms` shows the real figures.

## Screen off, machine by machine

When the screen goes off the shell paints black once, stops drawing, and runs only the heartbeats its watchers need (the battery, the
drive banner, alarms, the paired computer's input). The battery, clock and drive watchers are separate threads and keep running.

| Machine | What happens |
|---|---|
| Laptop with a backlight (`/sys/class/backlight`) | the backlight goes to 0 (after dimming), then `FBIOBLANK` powers the framebuffer down |
| Desktop monitor, Mac mini, no backlight | black picture, then `FBIOBLANK` powers the framebuffer down (the monitor sees no picture, or the GPU driver turns it off, whatever that driver does) |
| MacBook with a Touch Bar | the same, and the strip goes dark with the screen |
| The desktop window (`iyakki-gui`) | the saver draws in the window; screen off is a black window. The host's display is never touched |
| A remote (browser) screen | the same, at 4 frames a second |
| Several displays (the KMS path) | every display shows the saver or black; the backlight and the blank call are only for the single framebuffer path (follow-up) |

**A black screen is never a trap.** The old brightness is written to `/run/iyakki/backlight-restore` (a RAM file, nothing is written
to a drive) before the backlight is touched; a shell that crashed and was restarted by the keepalive puts it back at start, and a
shell that starts on a panel already at 0 raises it to about 60 %. The shell also unblanks the framebuffer when it starts and when
it stops or hands the screen to another program. Any input wakes it (the unblank is made within one heartbeat, about 33 ms).

## Not built (follow-ups)

- **Locking.** IYAKKI has no lock screen (signing out is the only way to protect a session), so there is no "ask for my password when
  waking" and no "lock the screen with the saver". A lock needs a session lock state in the shell; it is a separate piece of work.
- Photo slideshow from Pictures, a starfield behind the logo, and a key chord to start the saver now (Preview is on the Settings page).
- Video playing inside the web browser as a reason to keep the screen on.
- Backlight and blank calls on the several-displays path.

## Troubleshooting: the screen stays black

1. Press a key or move the pointer, and wait a second. The first one only wakes the screen.
2. Press the power button briefly (it wakes the screen; a second press opens the power card).
3. If nothing: the screen may be off at the hardware (the backlight at 0 after a crash of the machine, not of the shell). Restart the shell
   (it raises a dark panel); with a keyboard on a laptop, the brightness keys do not go below the lowest step but start from it.
4. Remote login (`iyakki.ssh=1`): `cat /sys/class/backlight/*/brightness` should not be 0; `echo 600 > /sys/class/backlight/<name>/brightness`
   brings it back. The shell's log lines starting `screen:` say what it did ("Awake -> Saver", "framebuffer powered down").
5. Choose Never in Settings -> Screen saver to rule the feature out; please tell us the machine and what the `screen:` lines said.

## Device test steps

On a laptop and on a desktop machine, from a stick that has this build:

1. Settings -> Screen saver -> Preview: the logo drifts for 5 seconds on black, then the desktop returns. Press a key during it: it stops.
2. Set "Idle for" 1 minute, saver then off after 1 minute. Wait: saver (logo moves, no flicker, the panel dims on a laptop), then the screen goes dark. Move the pointer: the picture is back within a moment, brightness exactly as before.
3. Play the radio, wait 2 minutes: nothing happens. Stop it: the saver starts after the idle time.
4. Unplug the charger on a laptop: the times halve. With "Shorter on battery" off they do not.
5. Click a button while the saver runs: the click only wakes (nothing is pressed).
6. On a MacBook with a Touch Bar: the strip goes dark with the screen and comes back.
7. Watch `top` (or the log line `shell: N frames in 10s`) during the saver: low CPU, 15 frames a second on mains.
8. Kill the shell while the screen is off (`kill -SEGV`): after the restart the screen is lit and the brightness is back.
