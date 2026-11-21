# Device UI tests

**For:** people who test the shell on a real device or in QEMU, and the pipeline. **Status:** F-DEV-17, partial (see the row).

Owner request 2026-11-15: whenever something is tested by hand on the Lenovo (or any machine), the same steps are captured as screenshots
and scripted so the pipeline can run them. `dist/devtest/` does that through developer mode (`docs/DEVELOPER-MODE.md`): `dev state`
(app, page, focused field, text and buttons with places), `dev input key|click|tap|text`, `dev screenshot`, `dev config get`.

## Run it

```
./run.sh device-ui --device lenovo-t7 --scenarios all --out ~/iyakki-work/devtest-runs/RUN
./run.sh device-ui --qemu target/buildroot-pc-push --scenarios all --out DIR
```

`--device` takes a name from `iyakki dev devices` (the client is `$IYAKKI_CLIENT` or `target/release/iyakki`; the runner never builds it).
The device must be signed in and showing Home or Settings; anything else in front means every scenario is SKIPPED with that reason
(the person is using it, or it is at the sign-in screen). The runner uses only paired devices you name.

## Reading the report

`DIR/<device>/report.html` is one file: a table of scenarios (pass, fail, error, skipped, with the reason), then for each scenario its
checks (including the skipped ones, marked) and its screenshots as thumbnails with the text read from them (OCR when tesseract is
installed). The full PNGs sit in `DIR/<device>/<scenario>/NN-label.png`; `results.json` has the same data. Exit status 0 means no scenario
failed or errored (skips are fine) and at least one ran; 1 a failure; 2 nothing could run (device unreachable, all skipped).
A FAIL is a finding, not noise: for example the contrast checks measure the real pixels, so a Light theme page whose subtitle is grey on
a dark wallpaper fails with the measured ratio.

## Safety rules (the guard in `devui.py` enforces them; tests in `dist/devtest/tests`)

* `dev input` cannot answer approval cards or the power menu; scenarios never depend on one, never use `dev shell` or `dev reboot`.
* Never type a password, never sign in. A screen that is not signed in refuses all input.
* Every key, text and tap is checked against a **fresh** `dev state` first. On Home only navigation keys, `s` and `w` go through (Esc, `q`,
  `o`, `p` and every other letter are shortcuts there: sign out, power menu, apps). `text` goes only into a focused, non-secret field and never
  on Home. Power, sleep, lock and the sign-out chord are refused everywhere; taps on labels that look like power, sign out, delete, reset are refused.
* Scenarios that change a setting read the original first and register `t.on_restore(fn)` BEFORE changing it; the runner runs the restore in a
  `finally`, whatever the checks said, and a failed restore fails the scenario. Then it returns to Home with the top bar's home button.
* QEMU only: the throw-away guest gets a scratch account made by typing, the way `dist/qemu/dev-pair-test.py` does it. That never happens on a real device.

## Writing a scenario

Python, in `dist/devtest/builtin_scenarios.py`:

```python
@scenario("my-page", devices=("real", "qemu"), restore="none (read-only)")
def my_page(t):                       # t is a devui.Test
    st = open_page(t, "Network")       # Home -> `s` -> sidebar button; raises Skip when the page is not in this build
    s = t.shot("network", st)          # NN-network.png
    t.assert_not_blank(s)
    t.assert_line_contrast(s, r"^Network$", 4.5)
    t.assert_no_overflow(st)
```

Helpers: `t.dev.state() key() click() click_sidebar() tap() tap_text() wait_for(app=, page=, text=) config_get()`, `t.shot`, `t.check`,
`t.assert_text` (OCR, else the state's text), `assert_not_blank`, `assert_contrast(region)`, `assert_text_contrast`, `assert_line_contrast`,
`assert_no_overflow` (sideways only: a page taller than the screen scrolls), `t.differs(a, b)`, `t.on_restore`. A feature that is not in the
build: `raise Skip("not in this build")`, never a failure.

Text files, `dist/devtest/scenarios/*.scn`, one step per line (`#` comments; header lines `name:`, `devices:`, `restore:`, `doc:` first):
`key NAME`, `text ...`, `click LABEL`, `click-sidebar LABEL`, `wait app=X page="Y Z" text=REGEX`, `shot LABEL`, `expect-text REGEX`,
`expect-not-blank`, `expect-page NAME`, `skip-unless-button LABEL`, `restore-click LABEL`. See `scenarios/firewall-page.scn`.

## First scenarios

`home-launcher`, `settings-pages` (8 sidebar pages plus the search box), `display-theme` (restores the theme), `wallpapers` (`w` on the Display page,
restores by picture), `appearance` (skipped when the build has no such page), `model-picker` (chat, Ctrl+P, Down keys; never Enter), `text-size`
(restores `display.size`), `firewall-page-text` (a text scenario).

## QEMU mode

`--qemu BUILD_DIR` boots `images/` of that folder the way `dist/qemu/dev-pair-test.py` does (the `Vm` class of that script is the boot recipe and the pairing steps, and
`qemu_guest.py` calls it: nothing is mirrored), makes a scratch account, switches developer mode on, reads the one-time code off the
screen with tesseract, pairs this computer and grants the diagnostic scopes. **Without tesseract the pairing code cannot be read**: the guest is
booted, two monitor screenshots (first screen, Home) are checked (not blank, they differ) and every other scenario is SKIPPED with that reason.
Set `IYAKKI_DEVTEST_KEEP=1` to keep the scratch folder.

## Limits (honest)

No mouse wheel in `dev input` (lists are scrolled with keys); no scrollbar detector yet; OCR checks need tesseract (without it the state's text
is used and labelled); contrast is measured on a box estimated from the line's place, so it is a screening check, not a WCAG audit; the text
shown on screen in chat, inbox, terminal and editors is not in `dev state` (only button names).

## In the pipeline

`./run.sh device-ui ...` is a thin wrapper of `dist/devtest/run.py`. `dist/buildroot/nightly-push.sh --device-ui NAME` prints (never runs) the
command to use after the person has installed the pack; the script still has no network step. Unit tests: `python3 -m unittest discover dist/devtest/tests`.
