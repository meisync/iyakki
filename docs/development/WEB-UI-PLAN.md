# Web UI on one API: plan

Status: proposed 2026-11-03, owner agreed to go forward the same day. Started 2026-11-04: the shell at `/ui/` and the pages Cloud access and Remote access exist (`crates/iyakki-agent/src/webservice/ui/`, rows F-WEB-16 to F-WEB-19), served on `iyakki serve` and, since 2026-11-04, on the desktop browser server at port 8443 too (`/ui/` and `/api/v1/` on the same TLS port, the desktop sign-in carried over; row F-WEB-20; code `iyakki-agent::webservice::embed`, `iyakki-remote::api`), and never opened in a real browser yet; then (2026-11-04) the pages Files, Git, Flows and Runs (`files.js`, `git.js`, `flows.js`, `runs.js`, shared `common.js`; rows F-WEB-31 to F-WEB-34), not yet opened in a real browser; Git status/log/diff, flow start, file rename/move and a separate Chat page are 'planned' because the API has no routes for them (listed in section 6); the editor is not built; then (2026-11-04, owner: 'add tenant after login') the top bar shows the tenant and switches between the person's tenants (`ui/tenant.js`, `GET /api/v1/tenants`, `POST /api/v1/tenant/switch`; rows F-WEB-41, F-WEB-42), tested on fakes and under node only. Read with
`UI-OS-SPLIT-PLAN.md`, `CLOUD-MULTITENANT-PLAN.md` (section 3.4, the two ways in), `MINIMAL-WEBSERVICE.md` and `../FULL-BROWSER.md`.

## 1. Idea

The functionality of IYAKKI sits behind one API (HTTP plus a WebSocket for events). Two front ends use it:

* the **desktop** (today a native Rust renderer on the screen), and
* a **web UI** in plain HTML, CSS and JS, which a browser opens against a cloud pod, against a desktop that serves itself to the network
  ("Serve this desktop to browsers"), and which the desktop can also show in its own browser tile.

The desktop and a pod are the same service underneath, so one web UI works against both.

## 2. Where it stands

* The agent has a web service with an OpenAPI description (`webservice/`); files, flows, cloud roles and namespaces are reachable.
* Many shell pages still call agent code in process (Settings jobs, sound, Wi-Fi, the touch probe). A browser cannot reach those.
* The cloud connector streams pixels of the native shell (works, but is a picture, not a web UI).

## 3. Decisions

1. **Licence.** All UI code is Apache-2.0 like the rest; no outside JavaScript or CSS library is bundled (React and other frameworks are
   not used: build in-house). A check in the licence test requires the header on every `.html`, `.js` and `.css` file we add.
2. **One API.** Every action a Settings page or tile does becomes an API call with the same permission and audit line as today; events
   come over one WebSocket. The native shell is moved onto the same calls page by page (the UI/OS split).
3. **Order.** Settings and Cloud access first (forms, small), then Files and Git, then Flows and Runs, then Chat and the editor.
4. **A native core stays on the desktop.** Boot, first run, sign-in, Wi-Fi, keyboard and touch recovery, and a 'the web UI did not load'
   page are drawn natively, because they must work when the browser engine or the API is broken (the Mac touch problem showed why).
5. **Pixel stream stays** as the fallback for tiers with no browser and for the cloud connector until the web UI covers the page.
6. **Sign-in and roles.** The web UI uses the same sessions as the API: the machine's accounts on a desktop, the tenant's accounts and
   roles (`cloudroles`) on a pod. Every call is checked on the server; the UI hides what it may not do but never decides.

## 4. Risks

* The desktop would depend on our own browser engine drawing the UI well; `iyakki browser-test` and the compat harness measure that.
  A heavier page load on the Mac than the native shell is expected; measure before switching a page.
* Moving every page behind the API is the large part. Keep the native page until the web page passes the same task tests.

## 5. Open questions for the owner

* Should the web UI replace the native page on the desktop once equal, or stay optional next to it? (Assumed optional.)
* Which pages must always stay native beyond decision 4?

## 6. Status of the pages (2026-11-04)

| Page | State | What is missing in the API |
|---|---|---|
| Cloud access, Remote access | built | audit trail route |
| Files | built (cloud pods): upload, download, view, real new folder (`POST /api/v1/files/mkdir`), rename (`POST /api/v1/files/move`, same space, 409 before replacing) | a folder picker for moving into another folder (the route can already); quota view |
| Git | built: clones list and clone, then per clone status, branches, log (paged, 200 at most), diff (text, 256 KiB at most, binary refused), all read-only (`GET /api/v1/git/...`) | commit, checkout, pull and push (write routes, not planned yet); a diff between two refs |
| Flows | built: list, read-only definition, Run button, runs list (paged) and one run (`POST /api/v1/flows/{id}/run`, `GET /api/v1/flows/{id}/runs[/{run}]`) | a log text route; stopping a run; the form for required inputs; real engine check on a pod |
| Runs | built (start, follow by polling, stop, approve or deny by the person's own click) | events WebSocket (polling every 2 s today) |
| Chat | the Runs page is its web twin; no separate page | a session or history route |
| Editor | not built | none needed for read; write route exists (`files/write`, text up to 512 KiB) |

Shared rules for every page: loading, empty, error and not-allowed states; 16 px gutter; secrets redacted before drawing; the shell tries
`whoami` with the desktop cookie first so a browser already signed in on port 8443 sees no form.
