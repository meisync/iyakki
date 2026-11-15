# Real-browser tests (dist/browsertest)

These tests drive a real Chrome or Chromium (headless) against the real web service, the real client page and the tile renderer. They are
in-house, Python standard library only (a WebSocket client, a DevTools client, a PNG reader), and **not part of the default gate**: nothing in
`cargo test` or `alpha-check` starts them. Run them by hand before a release and after a change to `ui/*.js`, `tilerender.js`, the client
page (`crates/iyakki-remote/src/clientpage`) or the client engine.

## What you need

| Needed | Why |
|---|---|
| Python 3.9 or newer | the runner and the unit tests |
| Chrome or Chromium 112 or newer | found in the usual places (macOS `/Applications/Google Chrome.app`, Linux `/usr/bin/google-chrome*`, `/usr/bin/chromium*`, `/snap/bin/chromium`, `/opt/google/chrome`) or on `PATH`; or set `IYAKKI_CHROME=/path/to/chrome` |
| `cargo` | builds two test binaries of `iyakki-agent` (the web service fixture and the tile goldens); or pass `--agent-bin` and `--tiles-bin` |
| `git` | the Git page test makes a small repository (the test skips that part when git is missing) |

Nothing else: no node, no pip or npm packages, no network. The service listens on 127.0.0.1 only, Chrome's debugging port is bound to 127.0.0.1,
and every page checks that nothing is requested from another address.

## Run it

```
python3 dist/browsertest/run_pages.py                       # everything (about 5 minutes)
python3 dist/browsertest/run_pages.py --only pages,more     # some parts: pages, functions, more, jsdrivers, tiles, client
python3 -m unittest discover -s dist/browsertest -p 'test_*.py'    # the unit tests of the harness itself: no Chrome, a few seconds
```

Flags and environment:

| | |
|---|---|
| `--only a,b` | parts to run: `pages` (every page, accessibility, widths, themes), `functions` (what a person does), `more` (Runs states, Remote access, Git, tenant switch, keyboard, zoom, RTL, long text, load numbers), `jsdrivers` (the node drivers of `crates/iyakki-agent/tests/webui_*_js.rs` run in Chrome), `tiles`, `client` |
| `--out DIR` | where screenshots and logs go. Default `$IYAKKI_WORK/browsertest/<date>` (`IYAKKI_WORK` defaults to `~/iyakki-work`); also `IYAKKI_BROWSERTEST_OUT` |
| `--work DIR` | throw-away Chrome profiles, fixture files, goldens. Default `$IYAKKI_WORK/browsertest/work` |
| `--target-dir DIR` | cargo target folder for the two test binaries (default `$CARGO_TARGET_DIR`, else `$IYAKKI_WORK/browsertest-target`) |
| `--agent-bin`, `--tiles-bin` | already built binaries (`cargo test -p iyakki-agent --lib --no-run`, `cargo test -p iyakki-agent --test webui_tiles_js --no-run`): skips cargo |
| `IYAKKI_CHROME` | the Chrome or Chromium to use |
| `IYAKKI_CHROME_FLAGS` | extra Chrome flags, for example `--disable-gpu-sandbox` (a Chrome run as root already gets `--no-sandbox`) |
| `--hard-limit SECONDS` | Chrome is killed after this long whatever the tests do (default 1500) |

Chrome is always killed at the end, also after a failure, a time-out or Ctrl+C; its profile folder is deleted. The service fixture stops itself
after 20 minutes at the latest.

## Exit codes

* `0` every check passed. Known issues are printed (`KNOWN KI-nnn`), they do not fail the run.
* `1` at least one check failed. The list is printed at the end and is in `results.json`.
* `2` **nothing was tested**: no Chrome or Chromium (or too old), no `cargo` and no binaries given, or the build failed. The run says
  `NOTHING WAS TESTED` and why. Treat 2 as "not run", never as a pass.

## Evidence

Everything goes to `--out` (outside the repository): `results.json` (every check with its result and detail, and the measurements: load sizes,
draw times, requests per minute of each page), `build.log`, screenshots of every page at phone, tablet and desktop width in light and dark,
the tile pictures next to their Rust reference pictures, the client page states, and the logs of the JavaScript drivers. Nobody has to trust
the summary: open the pictures.

## What it does not do

Safari and Firefox are not driven. No real IYAKKI machine, microphone, camera, speaker or paste is involved: the client page talks to a
scripted fake machine (`fakehost.py`). A native `<select>` cannot be operated by arrow keys in headless Chrome on macOS (it opens a popup that
is not part of the page), so the tenant switch sets the value and sends the change event.
