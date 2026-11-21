# UX notes from the test phase (2026-11-20)

Notes only: nothing here was changed. Screenshot names are in `~/iyakki-work/media/2026-11-20/web-ui/` (T3, real Chrome against the web service).

## Web UI (T3, real Chrome, loopback service)

| What | Where | Screenshot | Suggestion |
|---|---|---|---|
| Sign-in help text speaks of "the server" and of the page "hiding" what you may not do | Sign-in page | 01-sign-in | Say what the person needs: "Sign in with the name and password of your IYAKKI account." Keep the security promise in the docs. |
| First field is labelled "Name" | Sign-in page | 01-sign-in | Say "Account name" (or "User name"), the same word the Settings pages use. |
| A thick blue frame surrounds the whole content area on every page (it looks like a focus ring on the main region) | Tiles, Chat, Runs, Git, Settings | 02-tiles, 03-chat, 06-runs, 08-git | Show the ring only for keyboard focus on controls; a region that takes focus for the skip link should not draw a frame around empty space. |
| The Tiles page shows the tile demo with raw capitals ("TILE DEMO", "COUNT") and an empty box | Tiles | 02-tiles | Label the empty box (what it takes) and use the page's own type for tile text; or hide the demo tile outside dev mode. |
| The tile switcher mixes apps and names ("Counter (example tile)", "Settings (overview)", "Edit (scratch)") with plain "Chat" and "Runs", which are also top-level pages | Tiles | 02-tiles | Remove the duplicates of the top bar or mark them as tiles; use the same names as the top bar. |
| Top bar wraps to a second row on desktop in the tenant view (Tiles alone on row two) | Tenant pages | 04-settings, 06-runs | Move the less used pages (Cloud access, Notifications) into a "More" menu, or shrink gaps so ten items fit one row. |
| On a phone the bar takes three rows and most of the first screen before any content | All pages, phone width | 11-files-phone | Collapse into a menu button; keep Sign out inside it. |
| Files: the drive chooser appears twice, as a drop-down and as a one-button list ("My files") | Files | 11-files-phone | Show one of them; with a single drive show only its name. |
| Files: the upload row reads "No ...osen" (the browser's "No file chosen" is cut in the middle) | Files, phone | 11-files-phone | Hide the native text and show our own line ("Choose a file"), or let the row wrap. |
| Files: Rename and Delete sit on separate rows on a phone with the Delete button apart from its row | Files, phone | 11-files-phone | Put the file actions behind one "..." button per row. |
| Tenant settings: "Add or change an item" starts with a drop-down showing a technical key (`flows.quota`) and text boxes cut off ("Name (default for a single valu") | Tenant settings | 04-settings | Use a readable name next to the key; shorten placeholders or give the boxes their own line; say what "Team (empty: tenant level)" means in one sentence under the field. |
| "Vault: locked" is shown with no way to unlock it from the page and no hint what it blocks | Tenant settings | 04-settings | Add a button or a line "Unlock the vault to see secret values". |
| Chat: the placeholder "Ask the assistant ..." is the only prompt; Stop is shown greyed beside Send with no run | Chat | 03-chat | Disable Stop visibly (it is dim already) and hide it until a run is going; the Model drop-down's first entry is "The machine's default model": show which model that is. |
| Chat and Runs: "Tools this conversation may use (12 of 12)" is closed and says nothing about what is allowed | Chat | 03-chat | Name the count in words ("All 12 tools allowed, you are asked before each risky one"). |
| Git: four fields in a row with only placeholders (Folder, Branch, Depth); the placeholders vanish when typing | Git | 08-git | Give each field a visible label. |

## Test harness findings

* The client-page checks reported two false failures (404 for `uidisplay.js` and `files.js`) because the harness fake host did not serve the two newer client page files; fixed in `dist/browsertest/fakehost.py` with a test that every path of the real page is served.
* `ide_browser_tests::clicks_on_the_toolbar_reach_the_browser_and_clicks_elsewhere_do_not` failed whenever it ran in a group with the other browser tests (it expected "no engine", a process-wide fact other tests change); made independent of that fact.


Screenshots live in the media folder of the test run (media/2026-11-20/<app>/NN-step.png), not in the repository.

| What | Where | Screenshot | Suggestion |
|---|---|---|---|
| Version text reads "vv1.0.112" (double v) | Store, Installed, GitHub Copilot CLI row | store-setup/01-store | Strip a leading "v" from the reported version before the UI adds its own. |
| Disks shows the hardware serial number of every drive in the overview | Disks overview | disks/01-overview | Hide the serial behind a "Details" line so screenshots and screen shares do not leak it. |
| Partition letters in the PARTITIONS heading are drawn one per cell with gaps ("P A R T I T I O N S") | Disks overview | disks/01-overview | Draw the heading as one text run. |
| The Browser does not go back to Home with a visible button other than Back (top right); the Escape key and the Home icon behave differently | Browser, Radio | browser/01-open | Make "Back" a named button so `dev input click Back` and screen readers find it (Radio has no Back button in its list at all). |
| Typing a URL into the address field of a restored session showed the old page (YouTube) when the screenshot was taken 8 s later | Browser | browser/02-example-org | Check whether the first Enter after focusing the field is lost, or the load is slow; show a loading bar while a navigation is pending. |
| CJK language names in Wikipedia's portal draw as empty boxes (missing fonts, known English-first decision) | Browser | browser/03-wikipedia | Offer the optional CJK font pack from the Browser page when such a page is opened. |
| Settings Overview shows "The last session ended unexpectedly (2026-11-15)" five days later | Settings home | settings/01-settings-home | Clear the notice after it was shown once or the person pressed Fix it. |
| Radio: after Play the list header says "Radio Paradise" while the selected language is English with six entries and no row is marked as playing | Radio | radio/04-playing | Mark the playing station in the list. |


Format: what, where, screenshot, suggestion. Screenshots are under `~/iyakki-work/media/2026-11-20/`.

## T2 (QEMU boot of the lean image af2c1856, 2026-11-20)

* The Developer mode page is one very long wall of paragraphs and switches (Settings, Developer mode; screenshot of the page was not kept because it shows the paired computer's name). The Firewall, Core contract channel, Relay, UI socket and Resume lines each end with "tap here or press X" in running text. Suggestion: one row per switch with a visible on/off state and the key letter as a small label.
* On the Developer mode page the tooltip of a paired computer covers the first row of buttons (Settings, Developer mode). Suggestion: place the tooltip above the row or only on hover with a delay.
* The model router page shows "Router off" and "No models yet" but the on/off button is just "Router on/off" with no hint that it is off by default and experimental beyond the title (`settings/04-model-router.png`). Suggestion: make the first button read "Turn router on" while it is off.
* SAMPLE, Events while the link is down (`sample-app-events/02-link-down.png`): the Heard log shows "network offline" three times in a row (#2, #3, #4). Suggestion: fold repeats into one line with a count, or say why it was told again.
* The /connect page window form (`connect-page-app/03-window-form.png`): before the fix of this day the machine's own buttons (Full screen, Disconnect) stayed visible behind the app's title bar. Fixed in page.js; check the bar on a real phone as well.
* /connect page phone form (`connect-page-app/05-phone-form.png`): the app uses the whole width but only the top part of the height when the app's own list is shorter; the rest is black. Suggestion: fill the rest with the app's background colour.

## T5 (Mac mini 2018, Macmini8,1, signed-in Home, image 0.1.0-alpha.7-dev, 2026-11-20)

* Voice, Read aloud: pressing Play with the default voice (MOSS-TTS-Local) silently starts a download of about 4.4 GB from Hugging Face inside the Lemonade pod ("Auto-loading model ... Model not cached, downloading"). The tile only says "Getting the voice ready..." for minutes, no size, no progress bar, no question (`voice/mini-03-voice-playing.png`). Suggestion: ask first ("This voice is 4.4 GB, download now?") and show the percentage from the pod log; this also fits the rule that third-party downloads happen at the person's request.
* Settings, Network, Platform services (Clusters page, "Deploy Gitea or a private store"): Gitea read "not installed" although Store -> Setup runs it (iyakki-gitea 1/1), while the Registry row beside it said "no cluster chosen". Fixed on this branch (a row says "no cluster chosen" when the cluster was never asked).
* Settings, Sound: the Microphone section (and the "built-in microphone (experimental)" switch) sits below Mixer status; on a 1080p screen it is not visible and the page does not scroll with the mouse wheel or a swipe, only with the Down key after a click on the sidebar (`settings/mini-03-sound-scrolled.png`). Suggestion: a visible scroll bar that takes drags, or a "Microphone" jump link in the button row.
* Settings pages do not scroll with a swipe from `dev input swipe` (Sound, Browser: the Browser page's "Smooth pages" and "Protect browser data" cards are cut at the bottom edge, `settings/mini-04-browser-settings.png`). Suggestion: make the rows list a scroll area that reacts to a swipe and the wheel, show a "more below" hint.
* Settings, Bluetooth on a Mac mini 2018: the controller has no firmware patch file ("working; optional firmware file not found") yet scanning works and found four devices; the popup lists only low-energy devices as "Not supported" and hides three behind "Show 3 more devices" (screenshots of the scan were not kept: neighbours' device names). Suggestion: list the count by kind ("4 low-energy devices, none can be paired here") instead of one name and a button.
* Home, Devices & System group: the CHECK-UP tile now has 14 steps (Graphics card for models and USB devices were added) while the CHECK-UP row text, the hardware recipe and docs still said twelve (`home/mini-01-home.png`). Recipe and its text updated.
* Radio: a user-added station that answers 404 ends in "did not play: the station answered 404" with no way to remove it from that message (`radio/mini-02-radio-playing.png`). Suggestion: offer Remove (d) in the message.
* Disks, USB stick is an old Mac install (EFI + HFS+) listed with Mount/Eject/Advanced buttons and no hint that it is the stick the system was installed from earlier (`disks/mini-01-usb-drive.png`). Suggestion: a "Contains another operating system" chip.
* Browser reopens straight onto the last tab (a YouTube video that starts playing sound at once) when the tile is opened (`browser/mini-01-browser-open.png`). Suggestion: restore the tab but keep media paused until a click, so opening the Browser never starts sound by surprise.
