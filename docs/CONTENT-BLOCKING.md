# Blocking ads and trackers

Settings -> Browser -> **Block ads and trackers**. In plain words: your browser can refuse to send requests that a filter
list names (advertising servers, trackers, analytics), so those pages carry less and load less. It is **on by default**, but it does
nothing until you have downloaded a list; with no list, a broken list, or the switch off, every page loads as if there were no
blocker (it fails open).

## What it blocks, and what it does not

* **Requests, not pages.** A request to an address a rule names is not sent at all. The counter in the browser's bottom bar says
  "N blocked" for the page you are on; **B** opens the list of what was blocked and which rule did it (for debugging).
* **A page you open is never blocked silently.** If a list blocks the page itself (rules marked `$document`), you get a page
  titled "Blocked by your filter list" that names the rule, and **A** (or "Allow on this site") opens the site anyway.
* **Per site.** **A** on any page switches blocking off for that site (and its subdomains) and again on. The choice is yours alone
  and is listed in your own settings file; Settings -> Browser -> Block ads and trackers switches it off everywhere.
* **Hiding rules** (the `##.ad-banner` lines of a list) are used by the full web engine only, and only simple selectors: tags,
  `#id`, `.class`, `[attribute=value]` with space and `>`/`+`/`~` combinators. The reader browser does not hide elements.
* **YouTube.** Blocking video advertisements on YouTube is a moving target: those ads come from the same servers as the video,
  and the lists chase YouTube's changes. This feature removes the requests the lists name and **does not promise ad-free video**.
  What it measurably did to pages is in docs/BROWSER-SPEED.md.

## Where the lists come from

The lists are **third-party data with their own licences. They are not part of IYAKKI and are not in the image.** Nothing is
downloaded until you press a button in Settings -> Browser -> Ads and trackers:

| button | what it does |
|---|---|
| Download lists (l) | fetches EasyList and EasyPrivacy from easylist.to over HTTPS |
| Update (n) | fetches again every list that has an address |
| Add address (u) | a list at an address you type (any `https://` address) |
| Add file (o) | a list file on this computer (the file picker) |
| Remove lists (r) | deletes the lists |

Known lists, with the licence as their authors publish it (check their sites; the program only displays this text):

| list | licence | what it is for |
|---|---|---|
| EasyList | GPL-3.0 or CC BY-SA 3.0 (dual licence) | advertisements on English-language sites |
| EasyPrivacy | GPL-3.0 or CC BY-SA 3.0 (dual licence) | trackers and analytics |
| uBlock filters | GPL-3.0 | ads, trackers and annoyances (only the rules this blocker understands are used; add it by address) |

For every list the settings page shows its name, how many rules were usable, when it was fetched, the first characters of the
SHA-256 of what was downloaded, the licence and the site it came from. Lists with an address are refreshed by the same code once
a week, at the first page you open after start, but only lists you added yourself. A failed or empty download keeps the old
text. A downloaded file that does not look like a filter list (no readable rule) is refused. Lists are stored per person, under
`browser-lists/<user>` in the data folder, at most 24 MB each and 12 lists.

## What the rules can say

Supported: `||domain^`, `|https://`, trailing `|`, `*`, `^`, exceptions `@@`, options `script`, `image`, `stylesheet`, `font`,
`media`, `xmlhttprequest`, `subdocument`, `ping`, `websocket`, `other`, `document` (and `~` for none of these),
`third-party`/`first-party` (`~third-party`), `domain=a.com|~b.com`, `important`, `match-case` (ignored: matching is not
case-sensitive), and `@@||site^$document` (the site is never filtered). Skipped and counted: regular-expression rules,
`redirect`, `csp`, `removeparam`, `badfilter`, `popup`, scriptlets and procedural cosmetic rules, and any option not listed
here. "Third party" compares the registrable domain of the request and the page using a short built-in list of two-part suffixes
(`co.uk`, `com.au`, `github.io` ...), so a site on an unusual suffix may be misjudged.

With the real EasyList and EasyPrivacy (October 2026) the blocker read 141,735 rules: 112,393 blocking, 1,388 exceptions and
22,783 hiding rules; 4,843 lines were skipped (regular expressions, options it does not know). Compiling took about 110 ms in a
release build, memory about 26 MB, and a request is checked in about 2.5 microseconds.

## How it works in the two engines

* **The reader browser (iyakki-web).** Every request goes through one wrapper (`FilteredFetch`) that sits outside the cache and
  the network code: a blocked request is never sent and never cached. The page that asked is the referrer, or the page now shown.
  Matching is by index: a rule starting `||host.tld^` is found through the request's host and its parent domains, every other
  rule through the longest whole word it contains (the URL is cut into words and each is looked up); the few rules with no
  usable word are tried on every request, at most 4,000 of them. There is no regular-expression engine, so no address and no rule
  can make matching slow. At most 300,000 network rules and 40,000 hiding rules are kept; the rest is counted as dropped.
* **The full web engine (WPE WebKit; YouTube, Google and other heavy sites).** The same rules are written as WebKit's
  content-blocker JSON into the person's engine profile (`content-filter.json`, with an id that is a hash of its text) and the
  helper loads them through WebKit's own `WebKitUserContentFilterStore` before the first page, so WebKit blocks inside its network
  stack. WPE 2.50 (the version in the images) has this API; it needs no change to WebKit. Limits that come from WebKit: it refuses
  a list with more than 150,000 rules, so at most 100,000 are written (whole-domain rules first, then path and word rules, the
  rest dropped and counted; the real lists have 112,000 blocking rules, so about a fifth of the path rules are left out of the
  full engine); a rule that names both included and excluded domains cannot be written; frame-only rules need `$subdocument`;
  WebKit **does not tell the helper which requests it blocked**, so the "N blocked" counter and the list count only what the
  reader blocks. WebKit compiles the rules once after a change (kept by its store under the profile; unchanged rules are loaded,
  not compiled again). Compiling 100,000 rules was **not measured on a Mac**: it runs off the main thread, the first page waits at
  most 20 seconds for it, and if it fails or takes longer the engine runs without a filter. "Allow on this site" reaches the full
  engine the next time it is opened (WebKit cannot be told about a change while it runs).
* The weekly refresh and the downloads use the browser's own network, as the person, on a thread of their own.

## Tests

`cargo test -q -j 3 -p iyakki-web --lib blocker` (parser, matcher, anchors, wildcards, separators, options, exceptions, domain
restrictions, malformed lines never panic, limits, a 100,000-rule performance test, the WebKit JSON, list store with a fake
server, the fetch wrapper with a fake network); `cargo test -q -j 3 -p iyakki-shell --lib -- browser_block browserblock
blocked_requests_are_counted a_blocked_page_shows with_blocking_off` (settings page at five sizes and in panes, the keys, the
counter, A, the explanation page); `cargo test -p iyakki-webview --test helper` (the helper's source loads the filter and fails
open); `cargo run --release -p iyakki-web --example blocker_stats -- LIST...` prints the figures above for any list files;
`cargo run --release -p iyakki-js --example browser_timing -- --js --preload --block LIST,LIST URL` loads a page with and without
the lists. Not run: the helper itself against a real WPE WebKit (the engine cannot run on the development PC and the Mac was
offline).
