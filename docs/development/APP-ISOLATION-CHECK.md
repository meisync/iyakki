# App isolation check

Part of [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) (section 6). It shows, against a running service, that an app stays inside its walls, and that the web UI and the app fit a phone-sized window.
First run: 2026-11-14, on the web service built without the shell (`--no-default-features --features headless`), with the five web tiles (Counter, Settings, Edit, Chat, Runs).

## Run it

```
cargo build -p iyakki-core --no-default-features --features headless --bin iyakki
BIN=target/debug/iyakki bash dist/scripts/app-isolation-check.sh
```

It starts the service itself over a temporary folder (never the real data folder) with two test accounts, runs the checks and prints `PASS` or `FAIL` per line. Screenshots of the home page and of every tile at 390x844 go to a
`shots` folder (the last line says where; set `ISOLATION_OUT` to choose it). It needs `google-chrome` and node 22 for the phone-sized part and says `SKIP` when they are missing. Exit code 0 means every check passed.

## What it checks

| Group | Check |
|---|---|
| Walls (API) | no token and a bad token are refused (401); an unknown tile is 404 |
| Other people | a person reads their own run; another person gets 404 on it and does not see it in their list |
| Each tile only calls its own tools | after the browser drives each tile, the audit log (`tile.call` lines) shows only that tile's tools: Settings `settings_get`, Runs `runs_list`, Chat `chat_models`; Edit calls nothing. A call outside the tile's list fails the run |
| Phone window | the header takes at most 40% of the screen; no page name is broken over several lines; every tile fits inside the 390 px window and adds no horizontal scroll |

When you add a tile or change what one calls, update the `allowed` table in the script together with the tile's manifest; the script and the manifest must say the same.

## By hand, in any browser (mobile view)

1. Open the web UI (`/ui/`), open the developer tools and switch to the device toolbar (mobile view), 390x844 and again 360x740.
2. Sign in as a test person, open **Tiles**, pick your app.
3. It draws, scrolls and fits; nothing is cut at the right edge; the buttons are large enough to touch.
4. Make a refused call on purpose (something you did not declare). Read the refusal in the app's Info panel (web-bundle apps) or the audit log (tiles), and check the thing did not happen.
5. Sign in as a second test person: they see none of the first person's data.

## What the first run found (2026-11-14)

| Finding | Status |
|---|---|
| On a phone the header stacked the page buttons in a column and broke "Notifications" in the middle ("Notificatio / ns"); the header took two thirds of the first screen. | Fixed: at 520 px and under the pages get a row of their own (`ui/app.css`); the header is 157 px and the check keeps it that way. |
| Edit tile: the toolbar button "CLEAR" is cut at the right edge of the 332 px tile (the tile draws its toolbar for a wider area). | Open, KI-990. |
| Edit tile: typing from a phone's soft keyboard into the tile was not checked; the tile draws on a canvas and takes keys from the page. | Open, KI-991. A person must try it on a real phone. |
| The walls held: no unauthenticated access, no cross-person access, and no tile called a tool outside its list. | Checked by the script. |
