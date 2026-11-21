# The per-release site check (F-BROW-33)

Mail, documents and office web apps are the sites people judge a browser by. This is the list that is looked at before every release,
in the **full web engine** (the one Google and YouTube use), and the script that does the part a machine can do.

## What is checked by the script

`dist/webview/release-sites.sh` opens each site of `dist/webview/release-sites.tsv` as a signed-out visitor with `iyakki browser-test`
(`docs/BROWSER-AGENT.md`), waits for the sign-in screen's text, checks that the page is not Google's "this browser may not be secure"
wall, takes a screenshot and says `pass` or `FAIL`. A pass means the page loaded, drew its sign-in screen in time and the engine stayed
up. It does **not** mean the app is usable: that needs an account and a person (the Manual column of F-BROW-33).

The list today: Gmail, Google Docs, Google Sheets, Outlook, Notion, Slack (the owner's six) and Proton Mail (a mail app that is neither
Google's nor Microsoft's). The texts the script waits for are first guesses for a signed-out visit; the first run on a device settles
them (change the list, with the date, in the commit).

## How to run it on a Linux host (the QEMU web image) or a device

1. Boot an image that has the full engine (`web` or `macweb`) with network, sign in, and pair developer mode if the run is from another
   computer (`docs/development` dev-mode notes).
2. Copy `dist/webview/release-sites.sh` and `release-sites.tsv` to the machine and run `sh release-sites.sh --out /tmp/sites`. One site:
   `--only Gmail`. Nothing is installed and no account is used; each run starts a private browser profile.
3. Read the table. For every `FAIL` the folder `/tmp/sites` has the spec, the result JSON (`--json`), the error text and the screenshot
   the runner took at the failing step. Put the table in the release's results file (`docs/project/test-plans/RESULTS-TEMPLATE.md`).
4. A site that fails because it blocks automated browsers is a **finding**, not a pass: write down what it showed.

`sh release-sites.sh --dry-run` writes the specs and checks that they are valid JSON; it needs only `sh` and `python3`. The test
`iyakki-agent/tests/release_sites.rs` runs it and reads every spec with the real browser-test parser.

## The signed-in pass (written 2026-11-19, never run)

`sh release-sites.sh --signed-in DIR` is the "usable" half of the row. `DIR/<slug>.json` (for example `gmail.json`) holds the steps after the
goto, in the spec format of `docs/BROWSER-AGENT.md`, and a text that only a signed-in page shows:

```json
{"steps": [{"fill": {"label": "Email", "value_env": "IYAKKI_TEST_GMAIL_USER"}},
           {"click": {"role": "button", "name": "Next"}},
           {"fill": {"label": "Password", "value_env": "IYAKKI_TEST_GMAIL_PASSWORD"}},
           {"click": {"role": "button", "name": "Next"}}],
 "ok_text": "Inbox"}
```

A secret is never in a file: a `fill` step names an environment variable (`value_env`, only names that start with `IYAKKI_TEST_`, so a spec cannot
read any other variable) and the runner reads it when the step runs; a report and the script's table name the variable and never the value. A site
with no steps file, or whose variables are not set, is `skip` with the reason. The test accounts and the steps files (the selectors differ per site
and change) are the owner's: nothing in this repository creates or holds an account. Test: `iyakki-agent/tests/release_sites.rs` builds the specs
from a fixture, reads them with the real parser, and checks that a run with secrets leaves them in no file and in no printed line.

## What is not done (KI-791)

* The script has never been run against the real sites in the full engine (no machine with the engine was reachable from the build
  host); the texts to wait for are unverified.
* No sign-in has been run: the signed-in pass above is written, but the accounts and the per-site steps files are the owner's, so whether Gmail, Docs, Sheets, Outlook, Notion and Slack are *usable* is the manual part. A person with test accounts steps
  through `docs/project/test-plans/browser-real-sites.md` for each.
* Sites change; the list is reviewed every release.
