# Browser: what is still needed for day-to-day use

The detailed rows, with status and tests, are in `FEATURES.md` (F-BROW-*). This page is the reading order.

## What works today (October 2026)
Pages with text, links, forms, tables, pictures (PNG, JPEG, GIF), style sheets (colours, boxes, flex, grid), cookies,
JavaScript (QuickJS in a sandbox: ES modules, Shadow DOM, custom elements, streams) that the user switches on per site,
search from the address bar (Brave or DuckDuckGo), video and audio files described and handed to an installed player,
screenshots, a clipboard queue; tabs, find in page, text selection and copy, zoom (per site), bookmarks, history and address-bar
suggestions, downloads, saved sign-ins in the vault with a password generator, a Secure / Not secure mark, and Hindi, Tamil,
Bengali and the other Indic scripts, Arabic and Hebrew. About 17 of 20 popular sites run their scripts without an error.

## Done since the first cut (F-BROW-19 to 26, 34 and 37, partly)
Tabs, find, selection and copy, downloads (no progress bar, files that fit in memory), bookmarks and history, passwords from the
vault, zoom, complex scripts (not CJK or emoji), a first cut of safe browsing, address-bar suggestions and short search words.
What each still lacks is written on its row in `FEATURES.md`.

## Needed next (P0: without these the browser is not a daily tool)
1. **CJK and emoji fonts** (the rest of F-BROW-26), autofill of names and addresses.
2. **Video inside pages** with permissive decoders (now served by the full web engine, F-BROW-50 and F-BROW-43); this is also what YouTube and TikTok need.
3. **Layout sizes and responsive pages** for scripts (F-BROW-28).
4. **Safe browsing**, the rest: certificate details, permissions. Ad and tracker blocking: first cut done (own filter-list engine, reader and full engine, F-BROW-52 to F-BROW-55, [`CONTENT-BLOCKING.md`](CONTENT-BLOCKING.md)); not done: a counter for the full engine, regular-expression rules, scriptlets, YouTube-specific rules (F-BROW-34).
5. **Speed** (the full engine's measured state and what needs a rebuild: Mesa without iris, no Brotli, no WOFF2, F-BROW-56, [`BROWSER-SPEED.md`](BROWSER-SPEED.md) "Full engine (WPE)"): preload scanner, memory and disk cache with revalidation, per-person cache and Settings page done (F-BROW-51, measurements and what is not done in [`BROWSER-SPEED.md`](BROWSER-SPEED.md)); engine snapshot, HTTP/2, style and layout CPU (F-BROW-36) and the full engine's own speed on Macs still open.

## Then (P1)
Canvas/SVG/WebP (29), real form controls (30), persistent storage and cookies in the vault (31), real-time web (32), a checked
list of mail/office web apps (33), printing and PDF (35), address-bar suggestions (37, started), accessibility (38), assistant tie-ins
(39), more CSS and JavaScript standards (42).

## Later (P2)
Optional package (40), developer tools (41).

## How progress is measured
`probe_many` (an ignored test) loads a list of real sites and reports script errors; the commands are in the project memory
note "Browser compat harness". Each release should rerun it and note the result here.
