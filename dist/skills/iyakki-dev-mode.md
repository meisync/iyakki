---
name: iyakki-dev-mode
description: Work on a real IYAKKI device from a paired computer with developer mode: pair, install and live-reload apps, read logs, take screenshots, drive the screen, push files, hot-patch and update, with the approval and safety rules
triggers: dev mode, developer mode, iyakki dev, pair device, dev shell, hot patch, dev logs, device screenshot, push to device, update the device
---
Use when the person wants to build, test or fix something on a real IYAKKI device (a Mac, a PC) from another computer. ACT yourself: run the commands and read the output, never hand the person commands to paste.

1. Setup: on the DEVICE Settings -> Developer mode -> On (administrators; off by default; switches off by itself after 4 h, Extend 4 h). Pair once: `iyakki dev pair ADDRESS`, the person types the code shown on the device. `iyakki dev whoami` shows the device, account and what is on. Rights are scopes (install, debug, files, config, input, shell, update, forward, kube, power): a refusal names the missing one, tell the person to grant it on the device.
2. Look first: `iyakki dev info|status|ps`, `logs --system [--source shell|boot|audit|engine]`, `dmesg --grep X`, `state` (what the screen shows and its buttons), `screenshot -o /tmp/x.png`. ALWAYS give -o: a PNG written in the repo fails the licence gate.
3. Apps: `iyakki dev push FOLDER --watch --open` (pack, install, live reload), `install`, `uninstall ID`, `list`, `logs ID --follow` (the tile's console), `app data|config|clear-data|reset APP` to seed test data.
4. The screen: `input click "LABEL"` (by button text; a common label can hit the wrong widget, check `state`), `input tap X Y` (ORIGINAL pixels, not the screenshot's size), `input text 'abc'`, `input key Enter|Esc|Tab|Right`. A private page hides its text: use a screenshot. Never answer a camera, microphone or consent prompt for the person.
5. Files: `files ls|push LOCAL REMOTE [--force]|pull`, only the home, workspaces and /tmp/iyakki-dev. The vault, passwords and keys are never readable: never try.
6. Shell: `iyakki dev shell --minutes 30 -- CMD ARGS` runs one command as `nobody` with BusyBox (no root, no sound cards, no kill of root processes). A new grant needs the person's approval on the device screen: batch the commands into ONE call, say so BEFORE you ask, and wait. The approval card offers `y` (approve and stop asking for 4 h, plain shell only) and `w` (stop).
7. Fix without an image: `patch build DIR -o X.ipatch` (DIR/patch.json and DIR/files/<absolute path>), `patch apply X.ipatch --live`, `patch list`, `patch remove ID`. The PID 1 binary and kernel modules wait for a restart. After a restart `patch list` may show "next start": remove a patch once an image carries it.
8. Images: `update-pack IMAGE.img -o DIR`, then `update DIR --force` replaces the boot files and keeps "previous version". Boot the image in QEMU FIRST. NEVER reboot, poweroff or `--reboot` without the person's word.
9. Rules: no real tokens, keys or personal paths in files or logs; never write a disk; one change, then check the screen or the log and report what was verified and what was not.

Board: track steps with board_add (done only after a passed check). Bugs and failures: own board row with a B- id (never F-); bug_report captures logs; fix, verify, attach the passing run.

Example: "the recorder page hangs" -> `state` and `screenshot -o /tmp/a.png`, `logs --system --source engine`, one `shell` call that reads the engine log, a patch for the fix, `patch apply --live`, ask the person to reopen the page, screenshot to confirm.
