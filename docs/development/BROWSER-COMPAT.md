# Browser compatibility harness: method and 2026-11-12 findings (N2)

The reader browser runs a page's scripts in its own engine (`iyakki-js`). Two ignored tests in `iyakki-shell` load real sites and
say what the engine lacks. They need the network, so `cargo test` never runs them. The reader is a reader: a site that needs a real
engine stays with the full engine (F-BROW-04, F-BROW-43).

## Commands

```
# script errors per page (add IYAKKI_ALL=1 for one tab-separated NOTE line per message)
IYAKKI_URLS=sites.txt cargo test -p iyakki-shell --lib probe_many -- --ignored --nocapture --test-threads=1

# web APIs the page scripts name that the engine lacks, ranked by the number of sites
IYAKKI_URLS=sites.txt IYAKKI_APIS_SAVE=dir cargo test -p iyakki-shell --lib probe_apis -- --ignored --nocapture --test-threads=1
# the same ranking again, offline, from the saved scripts (after a change to the engine)
IYAKKI_URLS=sites.txt IYAKKI_APIS_FROM=dir cargo test -p iyakki-shell --lib probe_apis -- --ignored --nocapture
```

`probe_apis` asks the engine which names of `crates/iyakki-shell/assets/compat-apis.txt` are missing (385 names: globals, members of
prototypes, `navigator.*`, `Intl.*`, language built-ins) and counts in how many sites' scripts each missing name appears (a bare
name as a word, a member after a dot). A name that a script mentions is not proof that it was reached (feature tests sit in `if`s),
so the count is an upper bound. The catalog does not cover sizes: those were found from the errors and from the rows of F-BROW-28.
Build the tests in release mode to measure time; in a debug build the engine is about 10 times slower.

Rate and manners: one page at a time, 3 seconds between sites, no sign-in, no forms, `robots.txt` read first (`*` rules, the page
address itself): lobste.rs, imdb.com, w3.org, bing.com, brave.com, mojeek.com and a Wikipedia search address refused it and were
left out. apnews.com, nytimes.com and stackoverflow.com answered 403 to the reader and are in the list as they are.

## Sites (32)

en.wikipedia.org (an article), wiki.archlinux.org, developer.mozilla.org (a reference page), doc.rust-lang.org/book, docs.python.org,
kubernetes.io/docs, react.dev, nodejs.org, bbc.com/news, npr.org, apnews.com, theguardian.com, aljazeera.com, nytimes.com,
news.ycombinator.com, github.com (a public repository), stackoverflow.com (a tag page), html.duckduckgo.com (a results page),
duckduckgo.com (a results page), excalidraw.com, wikihow.com, medium.com, tailwindcss.com/docs, vuejs.org, angular.dev,
openstreetmap.org, archive.org, reddit.com/r/rust, cnn.com, espn.com, weather.com, twitch.tv.

## Script errors (scripts that threw), before and after

| Site | Before | After | What it was |
|---|---|---|---|
| medium.com | 2 | 0 | `getContext('2d')` returned null |
| duckduckgo.com (results) | 9 | 0 | `el.attributes[name]` was undefined; the rest followed (`$` not defined) |
| doc.rust-lang.org/book | 2 | 0 | its page went from 23 to 134 blocks (not reproduced in the bench copy: element limit, see below) |
| npr.org | 2 | 1 | `document.createAttribute` / `setAttributeNode`; one error in its bundle remains |
| openstreetmap.org | 3 | 2 | `URL.createObjectURL`; now an honest "Workers are not available here" |

The other 27 sites already ran with no script error. The ranking below was made on the saved scripts of the first run (the engine
before this change against the engine after it).

## Missing web APIs, ranked by sites (of 32): before and after

93 of the 385 catalog names were missing before; 17 after. The top of the "before" table:

| Sites | Name | After |
|---|---|---|
| 21 | HTMLMediaElement.prototype.duration (and currentTime, buffered, canPlayType) | added |
| 20 | HTMLImageElement.prototype.decode | added |
| 19 | HTMLDialogElement.prototype.show | added |
| 19 | navigator.userAgentData | added (brand "Iyakki") |
| 18 | Element.prototype.setAttributeNS | added |
| 18 | HTMLInputElement.prototype.selectionStart (setSelectionRange, files, labels, validity, valueAsNumber, requestSubmit) | added |
| 17 | navigator.connection | added |
| 16 | Element.prototype.clientLeft, clientTop | added (real borders) |
| 16 | navigator.storage | added (estimate) |
| 15 | Element.prototype.scrollBy; navigator.credentials; navigator.plugins | added |
| 14 | Element.prototype.assignedSlot; webkitMatchesSelector; HTMLElement.prototype.popover | added |
| 13 | Document.prototype.scrollingElement; navigator.serviceWorker | added |
| 12 | aria*/slot/contentEditable reflection; HTMLMediaElement.currentTime; URL.createObjectURL | added |
| 12 | navigator.share | left absent on purpose (feature test) |
| 11 | WebAssembly | left absent (no WebAssembly in the sandbox) |
| 5 | navigator.getBattery | left absent on purpose |
| 4 | navigator.mediaDevices | left absent on purpose |
| 2 | WebGL2RenderingContext; webkitAudioContext | absent (full engine) |
| 1 | Node.prototype.lookupNamespaceURI; Temporal | absent |

Also built because scripts of these sites needed them: a real `MutationObserver`, `IntersectionObserver` and `ResizeObserver`
that fire, `getComputedStyle` values, `matchMedia` from the real window, Cache storage inside a visit, `Intl.Locale.maximize`.

## Element sizes (F-BROW-28)

`iyakki-js::modules::boxes` estimates where boxes would be (see the module header for what it does and does not do) when the
embedder gives a window; the reader hands over the width its last page was laid out for. The numbers are not compared with a real
browser's.

## Speed (F-BROW-99), release build, Mac mini

| What | Before | After |
|---|---|---|
| bbc.com/news in a fresh process | 1.09 s | 0.40 s |
| blank page with one script | 569 ms first, 11 ms next | 15 ms first, 11 ms next |
| doc.rust-lang.org/book scripts | 2.0 s | 0.5 s |

The first row was the engine compiling again for every page on a host without `/proc` (the cache refused a directory whose owner it
could not read). The last row was `a.href` being parsed afresh on every read. Pages that run much code of their own (BBC's bootstrap
script 0.2 to 0.4 s, YouTube about 1.5 s with its compiled scripts cached) are not faster.

## Not reproduced, not done

- (fixed in round 2) the Rust book's "too many elements".
- The 27 sites that run clean were not compared with a real browser for what they should show.
- Percentage heights, `z-index`, real fonts in the size estimate (round 2 added position, floats, box-sizing and resize).

# Round 2 (2026-11-12, N2b)

## Rust book: "the page has too many elements" is fixed

The tree only grew. A script that made an element only to read a value from it (the book's searcher makes an `a` element for every
address it parses) used one of the 400 000 node numbers each time. Now: objects for nodes made by `createElement`, `createTextNode` and
`cloneNode` are held weakly until the node is put somewhere (inserted); every 1024 creations the dead ones are counted and the host
takes back the detached trees nobody holds; content replaced by `innerHTML` or `textContent` that no script holds is taken back at
once. The numbers are used again (`Tree::free`), and the id index learns the reused ones. Fixtures (no network): 410 000 create and drop
loops keep the tree under 5 000 nodes, 20 000 `innerHTML` replacements keep it under 100 and `getElementById` still finds the right
nodes, a node a script holds or has inserted keeps its own properties.

## Layout gaps (F-BROW-28)

`position` (relative offsets; absolute and fixed boxes against their containing block or the window, with left/right/top/bottom,
shrink-to-fit and stretching between two offsets; sticky counts as in flow), floats with `clear` (text beside a float gets narrower; a
float is kept in the height of a box that contains floats, such as `overflow: hidden`), `box-sizing: border-box` (a new field of the
computed style, which also makes the reader's own style code able to see it). `iyakki_js::resize(engine, viewport)` and
`LivePage::resize` tell a live page its window changed: `innerWidth` and friends, `change` events on media query lists whose answer
changed (not on the others), observers look again, the window gets `resize`; sizes and `@media` are worked out again for the new window.
Only hosts that give a window get any of this. The reader reads each page once and keeps no engine, so it does not call `resize`: the
next page it loads is told the new window.

## APIs from the first "before" table

All were added in round 1: media `duration`/`currentTime`/`buffered`/`canPlayType`, `HTMLImageElement.decode`, `HTMLDialogElement.show`,
`navigator.userAgentData/connection/storage/credentials/plugins/serviceWorker/geolocation/locks/mediaSession`, `setAttributeNS`,
`selectionStart` and friends, `clientLeft`/`clientTop`, `input.files`. Round 2 adds `Intl.DateTimeFormat()`, `NumberFormat()` and
`Collator()` without `new` and a `TreeWalker` that ends when it has no root.

## Second site round (32 listed, 30 loaded)

`robots.txt` was read first (all 36 candidates allowed it; the first 32 were used); 3 seconds between sites; no sign-in, no forms.
Not loaded: canada.ca and bestbuy.com (the connection failed). The pages and their scripts were saved once and loaded again offline
for the "before" engine (the engine without the platform layer of this work: no sizes, observers, canvas, attribute nodes and so on)
and the "after" engine, so both saw identical content.

Docs: docs.docker.com, postgresql.org/docs, git-scm.com/docs, learn.microsoft.com (.NET), docs.github.com, man7.org (a man page),
gnu.org (bash manual), go.dev/doc, typescriptlang.org/docs. Blogs: blog.rust-lang.org, martinfowler.com, smashingmagazine.com,
css-tricks.com, dev.to, hackaday.com, xkcd.com, joelonsoftware.com, daringfireball.net. Government and public bodies: usa.gov, gov.uk,
nasa.gov, nih.gov, europa.eu, who.int, census.gov. Shops (category pages): ebay.com (laptops), etsy.com (jewelry), walmart.com
(electronics), target.com (electronics), ikea.com (chairs), newegg.com (laptops).

Scripts that threw (sum over the 30 pages, 598 to 630 scripts ran):

| Site | Before | After |
|---|---|---|
| census.gov | 31 | 2 (a Google script address answered with a web page) |
| martinfowler.com | 2 | 0 |
| nasa.gov | 1 | 0 |
| learn.microsoft.com | 1 | 0 (`Intl.DateTimeFormat()` without `new`, then `TreeWalker`) |
| joelonsoftware.com | 1 | 1 (`WebAssembly` is not defined: left absent) |
| europa.eu | 1 | 1 (a script address answered with a web page) |
| total | 37 | 5 |

Missing web APIs (catalog of 385 names: 93 missing before, 17 after), by number of the 30 sites whose scripts name them:

| Before (top of the list) | Sites | After |
|---|---|---|
| HTMLMediaElement.duration | 20 | added |
| navigator.userAgentData | 18 | added |
| HTMLDialogElement.show | 17 | added |
| Element.webkitMatchesSelector, HTMLImageElement.decode | 14 | added |
| aria* reflection, input.labels | 13 | added |
| navigator.plugins, navigator.storage | 12 | added |
| getAttributeNode, input.files | 11 | added |
| navigator.connection, credentials, serviceWorker | 10 | added |
| URL.createObjectURL, draggable, popover | 9 | added |
| WebAssembly | 8 | absent on purpose |
| scrollingElement, clientTop, setAttributeNS, selectionStart, media currentTime | 8 | added |

Still missing after, with sites: `navigator.share` 5, `getBattery` 2, `mediaDevices` 2, `outerText` 1, `PerformanceMark` 1, `navigator.vibrate`
1, `webkitAudioContext` 1 (WebAssembly 8 is the one error left that is the engine's: the sandbox has none). Nothing new at the top of
the "after" list was a surprise: the second round found the same classes of gaps as the first, and most were already closed.
