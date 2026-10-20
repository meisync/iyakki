# QA with FLOWS: browser nodes how-to

FLOWS can drive a browser for you: open a page, take pictures, read the page, fill and submit forms, check what appears, compare a picture with
a saved one, and keep a report. This is the how-to; the reference (every node, the safety rules, the limits) is the "Browser nodes (QA)" part of
[`FLOWS.md`](FLOWS.md).

## Try it in one minute

1. FLOWS, **Add step**... or DESIGN's **Templates**: choose **QA: login form smoke test**, **QA: responsive screenshots at 3 sizes** or **QA: console errors and broken links**.
2. Your account's browser setting must be `interactive` for steps that click and type (Settings, Users); reading steps (open, picture, read, check) work in `read_only`.
3. Put the test account's password in your vault under the name the template says (`qa-password`) and run it: **Run**. The first time, the run asks once which sites it will open;
   mark the flow **trusted** in the flow settings (the panel DESIGN shows when no step is selected) if it should run from a schedule without asking.
4. Open the RUN tab. While the browser works, the header shows **Browser session** (click it for the live picture). Afterwards the **Pictures** tab is the gallery
   and **Report** lists every step, every check, console errors and DOM snippets, with **Export HTML** (one file, pictures inside) and **Export JSON**.

## The shape of a QA flow

`Open URL` -> some of `Click`, `Type text`, `Fill form`, `Select option`, `Press key`, `Wait for` -> `Check` -> `Screenshot`. Every node's result is data for the next:
`{{ nodes.shot.output.path }}` is the picture's file, `{{ nodes.check.output.passed }}` and `{{ nodes.check.output.message }}` the check, `{{ nodes.rows.output.rows }}` a table read
from the page.

* **A check fails the node** with a plain message (`expected "#result" to equal "Welcome" but it is "Invalid login"`), and the node's error port runs if you drew one. Turn on
  **Soft** to record the failure and go on (the report lists it). A check keeps looking for its **Keep trying for** time, so a result that appears a moment later still passes.
* **Retry until something appears**: give a Check or Wait node a retry policy (the node's Retry field, for example `{"attempts":8,"backoff":{"kind":"fixed","delay_ms":500}}`).
* **Loops**: a `foreach` over `{{ nodes.rows.output.rows }}` runs a sub-flow per row.
* **Vision**: an Agent step can look at a picture: set **Pictures** to `{{ nodes.shot.output.path }}` and choose a model that can look at pictures in Settings; a model that
  cannot is refused with a message instead of being sent a picture. "Summarise this DOM" is an Agent step whose prompt contains `{{ nodes.dom.output.text }}`.

## Forms and secrets

`Fill form` takes a map of CSS selector to value: `{"#email": "qa@example.com", "#password": "vault:qa-password", "#remember": true, "#country": "India"}`. Write a secret only as
`vault:NAME`; it is read when the node runs and appears nowhere afterwards (run file, history, logs, report, DOM dumps of the session). A file input is refused. The page's own
password fields and token-like fields are removed from the DOM dumps. **Screenshots can show secrets that were on the screen**; do not export reports with pictures of sign-in
pages that show a password in clear.

## Visual regression

`Compare to baseline` takes a picture (of the viewport, or of an element) and compares it with `<Flows>/baselines/<flow>/<name>.png`. The first run saves the baseline and passes.
Later runs fail when more than **Allowed change** percent of the pixels differ by more than the **Colour tolerance** in some colour channel, and make a diff picture (changed pixels in red).
Set the same **Set viewport** size every time (a different size is reported as such); turn on **Replace baseline** for one run to accept a change.

## Responsive checks

`Set viewport` with the presets (phone 375x667, tablet 768x1024, laptop 1280x800, desktop 1920x1080) before a `Screenshot` (with **Whole page** on for long pages: the screens are joined, up to 12).

## Safety in one paragraph

Private browser per run, thrown away at the end; asked once per run (or the flow is marked trusted by you, never by the assistant or an import); hosts must pass your account's host list
(localhost is fine); an address with a user name or password is refused; downloads go to the flow's work folder or a folder you may write to; `Run JavaScript` is for administrators; budgets
of actions and minutes per run; the session is closed when the run ends or is stopped.

## From the command line and the assistant

The assistant (`flow_create`, `flow_edit`, `flow_run`, `flow_run_status`) can make and edit flows with these nodes and read a run's report; it can set the budgets but cannot mark a flow trusted or
choose your own profile. `flow_run_status` of a run with browser steps includes the QA report lines (checks, console errors, picture files).

## Tests

* Engine, kinds and report: `crates/iyakki-flow/tests/browser_kinds_tests.rs`. Host with a local fixture web server and the in-house JavaScript engine as the page: `crates/iyakki-agent/src/flows/browser_tests.rs`.
* With a real engine: `cargo test -p iyakki-agent --lib browser_tests::live -- --ignored --nocapture --test-threads=1` (needs python3-gi, gir1.2-webkit2-4.1 and Xvfb). It drives the same fixture with WebKit:
  real screenshots at the viewport size, an element picture, a phone viewport, console messages from clicks and timers, real mouse and key input, an iframe, a download and a whole-page picture.
* The designer and run view: `crates/iyakki-shell/src/apps/flows_tests.rs` (palette at the four reference sizes, the check step's form, the trusted setting, the run view with its session badge, live
  picture, gallery and report export).
