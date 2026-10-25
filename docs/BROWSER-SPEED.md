# Browser speed and memory

Owner request: "YouTube and Google sites load slowly. Increase memory (caches), parallel requests to a domain might help, threads
also." This page says what was measured, what was changed, what did not help and what is not done. Numbers are from one
development PC (16 cores, 29 GB, a fast wired link, round trip about 10 ms) that was **busy with other builds the whole time**
(load average 10 to 50 on 16 cores), so absolute times are noisy; only runs made back to back are compared.

## The most important finding

On the `web` and `macweb` images **YouTube and Google do not load through IYAKKI's own browser engine**. They open in the full web
engine (WPE WebKit in a helper program, `docs/FULL-BROWSER.md`, F-BROW-49), which has its own network stack (libsoup), its own cache
folder and its own threads. Nothing in `iyakki-web`, `iyakki-js` or the HTTP client in `iyakki-proxy` touches those loads. The
changes below speed up **pages of IYAKKI's own reader** (every page on the default `hw` and `mac` images, and any site the engine
does not take over: news, wikis, documentation, search results, forms). For the full engine this change set adds only a
measurement script and a list of candidates (end of this page); **no claim is made that YouTube or Google got faster.**

## How to measure

Own reader, on a development PC (network needed, public sites only):

    cargo run --release -p iyakki-js --example browser_timing -- [--js] [--runs 5] [--preload] [--cache DIR] [--delay-ms 60] URL

It prints the request timeline (start, end, bytes, requests in flight to that host, type), subresources by type, the most requests in
flight per host, and the phases: system DNS lookup, TCP connect, document cold against warm (the difference is connect plus TLS),
HTML parse, script phase (with the time spent waiting for script files), style phase (with the time waiting for sheets), pictures
(fetch and decode), cache hits and early fetches used, and a one-line `TIMING ...` summary with the median of the runs.
`--delay-ms N` adds N ms to every request to imitate a slower link. Not measured by this example: layout and painting (they are in
the shell), the split of connect and TLS inside the HTTP client (it does not expose them: estimated as cold minus warm), and
connection reuse counts (not exposed; the pool is checked through request times).

Full engine, on a device (or a desktop with the helper): `sh dist/webview/page-timing.sh 3 https://www.youtube.com/ ...` runs
`iyakki browser-test` with a one-step spec per address and prints the wall time of each run and the median (run 1 cold, 2 and 3
with the engine's own caches). This is the number to collect on the Macs.

## Baseline (before): what the reader does with a heavy page

`js=false` is the reader without scripts (the default for a site the user has not allowed), `js=true` runs the page's scripts.
Median of 3, same PC, load average about 30 to 50 at the time.

| page | scripts | usable (ms) | requests | most in flight (all hosts / one host) | what the time is |
|---|---|---|---|---|---|
| google.com search | off | 85 | 3 | 1 / 1 | a stub that needs scripts: 0 blocks shown |
| google.com search | on | 1120 | 3 | 1 / 1 | script run only |
| youtube.com | off | 452 | 8 | 5 / 4 | 5 style sheets, 3.2 MB |
| youtube.com | on | 10233 | 62 | 6 / 3 | script run and 40+ files fetched one at a time |
| wikipedia article | off | 817 | 34 | 4 / 4 | 745 ms style and layout CPU, 57 ms waiting for sheets |
| wikipedia article | on | 2086 | 43 | 4 / 4 | same, plus scripts |
| bbc.com/news | off | 91 | 24 | 4 / 4 | small |
| bbc.com/news | on | 3883 | 86 | 4 / 4 | **59 scripts fetched strictly one after another (concurrency 1), 3.9 s of summed request time** |

Findings from the timelines:

* Scripts were fetched **one at a time, in page order, each only when the script loop reached it**, and each new host paid its own
  connection and TLS set-up (150 to 700 ms for the first file from a CDN). Style sheets were only started after all scripts had run.
  This is the dominant network cost of script pages.
* Pages without scripts are CPU bound in the reader's style and layout code (Wikipedia: about 530 ms of cascade for a 1 MB page and
  a 220 KB sheet), not network bound, on this fast link.
* Pictures used 4 workers and at most 4 connections to a host; the HTTP client kept only 3 idle connections per host, so every burst
  of more than 3 requests reconnected (TLS again).
* The disk cache did not store anything requested with a cookie, which is nearly every static file of a site that sets one; it had
  no memory tier, no revalidation (`ETag`, `Last-Modified`), no `Vary` rule, a one-day limit, 2 MB per file, 64 MB in all, one
  folder shared by everybody, and it rescanned the whole folder on every write.

## What changed

| change | where | measured effect |
|---|---|---|
| **Preload scanner**: the page's HTML is scanned as soon as it arrives and its style sheets, scripts (not `nomodule`, not data) and up to 16 pictures are fetched at once (4 to 12 workers from the processor count, 3 when memory is short), most urgent first; the page's own loading code does not change, a request for something in flight joins it, something still queued is fetched at once by the page, answers nobody asked for are dropped when the page finishes or is left; hosts the account may not open are never asked; cookies must match or the page asks again | `iyakki-web/src/preload.rs`, `browser.rs` | **bbc.com/news with scripts: 2667 to 1756 ms (-34 %), 6523 to 2682 ms (-59 %) with 60 ms added per request.** wikipedia with scripts 1047 to 993 ms (-5 %, within noise). youtube with scripts about 6300 to 4970 ms (-21 %, very noisy: scripts take 4 to 9 s to run whatever the network does). Pages without scripts: no change (wikipedia 384 to 401 ms, bbc 45 to 48 ms). |
| Connection limits: 6 per host, 32 overall, document, then sheets and scripts, then fonts, then others, pictures last | `iyakki-web/src/net.rs` | Prevents flooding; no measurable gain by itself on this link (the examples never exceeded 6 per host before either). |
| HTTP client pool: 8 idle connections per host (was 3), 64 overall (was 10), kept for 60 s | `iyakki-proxy` | Not separable in the numbers above (small effect on a 10 ms link; it matters with TLS cost on slow links). Kept because it removes reconnects after bursts of more than 3 requests. |
| DNS answers kept for 60 s (a lookup took 5 to 400 ms; a page opens several connections per host) | `iyakki-proxy` | Not separable; kept because it is free. |
| **HTTP cache rewrite**: memory tier in front of the disk tier; entries with `ETag` or `Last-Modified` are revalidated with `If-None-Match` or `If-Modified-Since` and a `304` renews them; a stale copy is served when the network is down; static files requested with cookies are now cached; a week at most (was a day), 8 MB per file in memory 1/16 of the tier; sizes from the machine (below); index kept in memory (no folder scan per write); least recently used out first | `iyakki-web/src/cache.rs` | Warm-cache runs are bounded by the files still needing a round trip; not yet measured against the old cache on the same sites in this session. Unit tests cover the semantics. |
| Cache is **per person** (`cache/web/<user>`); requests that do not say who they are (the assistant's web tools) are not cached; files an older version left in the folder are removed | `iyakki-web/src/cache.rs`, `shell.rs` | Privacy fix, not speed. |
| Pictures: fetched after the scripts and sheets (lowest priority), workers from the processor count (4 to 8; 2 when memory is short) | `browser_images.rs`, `browser_speed.rs` | Not separable. |
| Settings -> Browser -> **Speed and memory** | `settings_browserspeed.rs` | Visibility only (below). |

### Did not help, or not shown to help

* Nothing helped pages **without** scripts on this link: they are CPU bound in style and layout (Wikipedia 350 to 400 ms of CPU).
  Speeding that up needs work in `style.rs` and `browser_layout.rs`, not in the network layer. Not done.
* Larger HTTP client pools, the DNS cache and the connection limiter show no difference at 10 ms round trip. They were kept for
  slower links but **have no measurement behind them**.
* The cache rewrite has no before/after on live sites yet (warm and cold runs were taken, but with the old cache's different rules the
  comparison is not clean). Treat its effect as expected, not proven.

### After table (same PC, runs made back to back, load about 10 to 17)

| page | scripts | before preload (ms) | with preload (ms) | change | with +60 ms per request: before / with |
|---|---|---|---|---|---|
| google.com search | on | 165 | 194 | noise | 252 / 234 |
| youtube.com | off | 502 | 561 | noise | 551 / 520 |
| youtube.com | on | 6311 (1321 to 8544) | 4971 | -21 % | 8150 / 8253 (no change: script run bound) |
| wikipedia | off | 384 | 401 | noise | 498 / 535 |
| wikipedia | on | 1047 | 993 | -5 % | 1655 / 1648 |
| bbc.com/news | off | 45 | 48 | none | 112 / 106 |
| bbc.com/news | on | 2667 | 1756 | **-34 %** | 6523 / 2682 (**-59 %**) |

Early fetches: bbc 55 used of 87 requests, youtube 14, wikipedia 3. Maximum concurrency rose from 4 to 9 overall and 6 to a host.
Targets were not given as promises; what was reached: script-heavy reader pages load a third to a half faster; pages without scripts
and pages whose scripts take seconds to run are not faster; **YouTube and Google in the full engine are unchanged**.

## Memory and cache sizes

Computed by `iyakki_web::net::budget` from `MemAvailable` and the free space of the data volume, re-read at most every 5 seconds while
the browser loads pages (`apps/browser_speed.rs`), so the memory gate can take cache back:

| Setting | memory tier | disk tier | each file in memory |
|---|---|---|---|
| Auto (default) | 6 % of available memory, 8 to 256 MB | a tenth of free space, 64 MB to 1 GB | a sixteenth of the tier, 256 KB to 8 MB |
| Large | 12 %, 16 to 512 MB | a fifth of free space, 128 MB to 2 GB | same rule |
| Small | 2 %, 4 to 32 MB | 32 to 64 MB | same rule |

Under 256 MB available the memory tier is off; under 96 MB nothing is cached; the memory tier never takes the last 96 MB, the disk tier
never the last 256 MB of free space (at least 32 MB are always allowed when there is room). Shrinking is immediate: the least recently
used entries go first. Workers: early fetches 4 to 12 (twice the processors), pictures 4 to 8 (one per processor), both reduced when
memory is short.

Settings -> Browser -> Speed and memory: **Change (k)** cycles Auto, Large, Small (saved per person, audited); the rows show memory
and disk in use against what is allowed, the share of requests answered without a full download, files fetched early and used or not
needed, and bytes downloaded since start; **Clear cache (x)** empties this person's cache only. Everyone may change their own choice
(it is their browser); the machine's free memory always limits it.

## What the cache stores, and what it never does

Kept: plain GETs with status 200 whose `Cache-Control` gives `max-age` (reduced by `Age`, at most a week) or that have a validator;
`no-cache` is kept only to be asked about. Never kept: `no-store`, `private`, answers that set cookies, `Vary` naming anything
but Accept, Accept-Encoding, Accept-Language, User-Agent or Origin (so never `Cookie` or `*`), partial bodies of media files, other statuses, and
pages (HTML, JSON, anything not a style sheet, script, picture, font or wasm) requested with cookies unless the server said `public`.
Heuristic freshness from `Last-Modified` alone and `s-maxage`, `stale-while-revalidate`, `immutable` are not implemented. `private`
answers are not kept although a single-person browser cache could (conservative on purpose).

## Limits and not done

* **Full engine (YouTube, Google, everything on the `web` images): not changed.** To collect on a Mac: `page-timing.sh`. Candidates,
  each to be measured there before it is kept: Brotli in libsoup (`BR2_PACKAGE_BROTLI` in the web fragment; Google and YouTube send
  gzip instead, about 15 to 20 % more bytes of script), the Skia CPU raster path under software GL (`WEBKIT_SKIA_ENABLE_CPU_RENDERING=1`
  in the helper's environment; the Mac mini images use llvmpipe) and the number of painting threads (`WEBKIT_SKIA_CPU_PAINTING_THREADS`; the name first written here did not exist in this WPE version, see "Full engine (WPE)" below),
  a shared network process or cache directory between tabs (each tab runs its own helper with its own network session), and whether the
  profile cache survives on a live USB session without a data volume (it does not: the profile is in RAM there). None of these was
  changed or measured here because the engine cannot run on the development PC.
* HTTP/2: the reader's HTTP client (ureq) speaks HTTP/1.1 only; every site above speaks HTTP/2. An in-house HTTP/2 client (HPACK, flow
  control, stream scheduling) is a large piece of security-sensitive code; with 6 connections per host and the preload scanner the
  network wait on script pages is already close to the slowest single file, so it was judged not the dominant cost and not started.
  Revisit if timelines on a slow link show requests queued behind the 6 connection limit.
* Happy Eyeballs (parallel IPv4 and IPv6 connects) and TLS session resumption are not implemented (the HTTP client connects to
  addresses one after the other).
* Resource scheduling not done: lazy loading of pictures below the fold (the reader fetches up to 60 pictures in document order),
  `async` and `defer` scripts still run in order at the point the script loop reaches them, `preconnect` and `dns-prefetch` hints,
  fonts named inside style sheets, scripts that scripts insert later (these are fetched when asked, as before), and cancelling a request
  already on the wire (queued early fetches are cancelled on navigation; running ones finish and are dropped).
* Parse and style off the main path, decoding in the browser thread pool and compile on a worker: pictures were already decoded in
  worker threads and compiled scripts are already cached on disk (`iyakki-js`, F-BROW-36); style and layout still run on the page
  thread. The decoded-picture cache, parsed-style-sheet cache and compiled-script cache by URL are not added (the script bytecode
  cache keyed by content already exists).
* The preload store belongs to one tab slot of the browser (a second tab's load cancels the first's queued early fetches).
* Machine load during measurement was high; repeat on a quiet machine before quoting percentages.

## Tuning knobs

Settings -> Browser -> Speed and memory (Auto, Large, Small); `iyakki_web::net::{PER_HOST, GLOBAL}` (6 and 32);
`iyakki_web::preload::{MAX_HINTS, MAX_IMAGE_HINTS, MAX_HELD}`; `iyakki_proxy::https::{POOL_PER_HOST, POOL_TOTAL}`; the cache location is
`<data>/cache/web/<user>`. Tests: `cargo test -q -j 3 -p iyakki-web --lib cache:: net:: preload:: netstats::`, and in `iyakki-shell`
`browser_speed`, `browserspeed`, `apps::browser`.

## Full engine (WPE)

Owner request: "YouTube and Google sites load slowly", then "start with browser WPE and ad blockers". YouTube and Google open in
the full engine (WPE WebKit 2.50.5 in `webview-helper`), so this is the part that matters for them. **What was and was not
measured, said first:**

* **No timing of the full engine was taken in that first pass.** The MacBookPro16,2 was gone then (see "Interactive performance"
  below for the pass that measured it: page load is not the problem, scrolling and video are), and WPE cannot run on the
  development PC (the image's engine is built against musl for the image). Everything below the line "Found in the build" is read
  from the engine's own sources and build outputs (`target/buildroot-web`, WPE 2.50.5, Mesa 26.0.1, libsoup 3.6.6), not guessed.
* Measuring on a Mac needs one command, below; the same command also times a change made at run time.

### Found in the build (images `web` and `macweb`)

| question | answer | how it was checked |
|---|---|---|
| Does WPE draw on the Intel GPU? | **No, not on these images.** Mesa is built with llvmpipe, virgl and radeonsi only; **iris (Intel gen8 and newer: the 2017 and 2020 Macs) and crocus (Haswell) are not built** (they pull in clang and a precompiled OpenCL compiler: hours). The shell therefore sets `LIBGL_ALWAYS_SOFTWARE=1` for the engine on any Intel GPU (`webengine::force_software_gl`), so the web process uses **llvmpipe**, Mesa's CPU renderer, even though the kernel has the i915 driver and a render node. | `libgallium-26.0.1.so` has no `iris_` symbols; `.config` has `GALLIUM_DRIVER_IRIS` unset; `dist/buildroot/configs/iyakki_web.fragment` |
| Skia backend | Skia is on. With nothing set, WPE paints with Skia's **GPU path (Ganesh over GL, here llvmpipe) on 1 painting thread**; `WEBKIT_SKIA_ENABLE_CPU_RENDERING=1` switches to Skia's own CPU raster on a pool of half the cores. Which of the two is faster on llvmpipe is the first thing to measure; no environment variable is set today. | `SkiaPaintingEngine.cpp`, `WebProcessGLib.cpp` in the 2.50.5 sources |
| Variables that exist in this version | `WEBKIT_SKIA_ENABLE_CPU_RENDERING`, `WEBKIT_SKIA_CPU_PAINTING_THREADS`, `WEBKIT_SKIA_GPU_PAINTING_THREADS`, `WEBKIT_SKIA_GPU_PAINTING_MIN_AREA`, `WEBKIT_SKIA_HYBRID_PAINTING_MODE_STRATEGY`, `WEBKIT_SKIA_MSAA_SAMPLE_COUNT`, `WEBKIT_SKIA_USE_LINEAR_TILE_TEXTURES`, `WEBKIT_DMABUF_RENDERER_DISABLE_GBM` (GTK only), `WEBKIT_FORCE_VBLANK_TIMER`, `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS`. **`WEBKIT_DISABLE_COMPOSITING_MODE` and `WEBKIT_SKIA_PAINTING_THREADS` (named in the earlier report) do not exist in 2.50.5.** | grep of the sources |
| JavaScript engine | **JIT is on**: `ENABLE_JIT`, `ENABLE_DFG_JIT`, `ENABLE_FTL_JIT`, WebAssembly BBQ and OMG JITs all ON, `ENABLE_C_LOOP` OFF. Not an interpreter. | `CMakeCache.txt` |
| Processes | WebProcess, NetworkProcess **and a GPU process** (`ENABLE_GPU_PROCESS=ON`). Bubblewrap sandbox is off. The helper runs one engine per tab. | `CMakeCache.txt`, `helper.c` |
| Network | libsoup 3.6.6 **with HTTP/2** (it links nghttp2 1.64). **No Brotli**: `BR2_PACKAGE_BROTLI` is not set, so libsoup is built `-Dbrotli=disabled` (Buildroot's `libsoup3.mk`); servers send gzip instead, 15 to 20 % more script bytes (the earlier report's estimate; not measured here). | `readelf -d libsoup-3.0.so`, `libsoup3.mk` |
| Fonts | **`USE_WOFF2` is OFF**: WOFF2 fonts (what Google and YouTube serve) are not decoded; text falls back to other fonts. Needs the same brotli library. | `CMakeCache.txt`, `wpewebkit.mk` |
| Frames to the shell | WPE draws into a `wl_shm` buffer; the helper copies it (1), compares it with the last frame row by row and writes only the changed rows into the shared file (2), and keeps a copy for the next comparison (3); the shell draws from the shared file. A full frame is copied three times, and an `XRGB` buffer is also touched pixel by pixel to set alpha; 30 frames a second at most (`IYAKKI_WEBVIEW_FPS`). At 1280x720 that is about 11 MB of memory traffic a frame, which is small next to software GL painting. The shell's own per-frame cost is in its log line `frames in 10s, draw ... screen write ...` (read it with `IYAKKI_WEBVIEW_DEBUG=1` on the device). | `helper.c` `flush`, `export_shm_buffer` |
| Disk cache | The helper gives WebKit a cache folder under the person's profile (`webkit_network_session_new(data, cache)`) with WebKit's default policy. The speed settings of this page (Auto, Large, Small) only size the reader's cache; **they do not reach WebKit's cache**. | `helper.c` |

So on the Macs the likely biggest cost is software painting and compositing on the CPU through llvmpipe with a single GPU-path
painting thread. This was a **hypothesis from the build**; "Interactive performance" below measured it (it is right: llvmpipe threads are the biggest cost).

### To measure and to try, on a Mac (no rebuild)

    sh dist/webview/page-timing.sh 3 https://www.youtube.com/ "https://www.google.com/search?q=rust"

(run 1 is cold, runs 2 and 3 use WebKit's cache and warm connections; it prints each run and the median). Then try the
rendering experiment, which needs only a restart of the browser's engine:

    echo cpu    > /var/lib/iyakki/web-profile/web-tuning     # Skia CPU raster, WebKit's default thread count (half the cores)
    echo cpu:4  > /var/lib/iyakki/web-profile/web-tuning     # the same with 4 painting threads
    echo gpu:0  > /var/lib/iyakki/web-profile/web-tuning     # GL painting on the main thread
    : > /var/lib/iyakki/web-profile/web-tuning               # back to WebKit's default

The file takes several words separated by spaces (later words for the same setting win): `cpu`, `cpu:N` and `gpu:N` as above, `lp:N`
(llvmpipe threads; the default is half the logical CPUs, `LP_NUM_THREADS`), `gstgl:off` (GStreamer's own video sink instead of the GL
one), `nice:N` (1 to 19: the engine N steps below the shell) and `fps:N` (5 to 60: the most pictures a second the helper sends,
default 30). `webengine::helper_env` reads the file (an empty file or any other word changes nothing; thread counts are clamped to the
number of cores; the code and tests are in `crates/iyakki-agent/src/webengine.rs`). The file is not a setting in Settings: it is there so
experiments can be run over SSH. **What the experiments showed: see "Interactive performance" below** (the words other than `cpu`,
`cpu:N` and `gpu:N` need an `iyakki` built after this change).

### Needs a rebuild (not built; exact options)

| change | options | why, and the expected gain (not measured) |
|---|---|---|
| Intel GPU in Mesa | `WEB_IRIS=1 ./run.sh build-macweb` (the `iyakki_web_iris.fragment`: `BR2_PACKAGE_MESA3D_GALLIUM_DRIVER_IRIS=y`; Haswell needs `WEB_CROCUS=1`) | The web process could paint and compose on the Intel GPU through iris instead of llvmpipe; `force_software_gl` turns itself off when the image's marker says an Intel driver is present. The helper still receives frames through `wl_shm`, so the finished picture is read back to memory once per frame; the gain is in painting and WebGL, not in the transfer. Hours of build and about 1 GB more image. |
| Brotli | `BR2_PACKAGE_BROTLI=y` | libsoup then builds with Brotli (`-Dbrotli=enabled`), WebKit sends `br`, and Google and YouTube answers shrink. |
| WOFF2 | `BR2_PACKAGE_WOFF2=y` (needs the brotli package) | WebKit decodes WOFF2 fonts, so pages show the fonts they ask for. |

A WPE rebuild is hours (it was not started); Brotli and WOFF2 together would be one rebuild of WPE and libsoup.

### What the WebKit disk cache and the memory gate can do

Nothing was changed. WebKit has its own memory-pressure handling inside each web process; the shell's memory gate (`memgate.rs`)
can already stop a tab's engine when memory is short (the helper reports "the web process used too much memory"). Giving WebKit's
cache a size from the same Auto, Large and Small switch needs the cache size policy of `WebKitNetworkSession`, which only
exposes clearing; a larger cache would be a patch to the helper's profile handling and was not attempted without a measurement
that shows the cache is the cost.

### Ad and tracker blocking, measured on the reader (the only engine that could be run)

Same PC, release build, 3 runs each, `--js --preload`, real EasyList and EasyPrivacy (downloaded for this measurement into a
scratch folder, not into the repository), **machine load average 13 to 20 the whole time** (other builds), so time differences are
mostly noise:

| page | requests blocked of requests | usable ms before (min to max) | usable ms with the lists (min to max) |
|---|---|---|---|
| bbc.com/news | 8 of 86 | 3163 (2580 to 3248) | 1255 (1072 to 1377) |
| theguardian.com/international | 1 of 108 | 6807 (5899 to 9402) | 8624 (8439 to 10211) |
| cnn.com | 3 of 121 | 6868 (5508 to 8603) | 7457 (6878 to 7613) |
| youtube.com | 1 of 61 | 7140 (6395 to 8861) | 5913 (940 to 6391) |

What this says honestly: the reader loads far fewer ad requests than a full browser would, because it does not run most of a
page's scripts (it blocked 1 to 8 requests a page); the ranges overlap, so **no speed-up can be claimed from these numbers**
(bbc looks faster, the Guardian and CNN look slower, all within the load noise). The request count in the `TIMING` line still
includes blocked requests (its timing list sits outside the blocker); the `BLOCKER` line is the true count. For the full engine,
where YouTube's and Google's own scripts run and ask for ads, analytics and tracking, the lists name many more requests; the
effect there could not be measured and **YouTube video ads are not promised to go** (docs/CONTENT-BLOCKING.md).

Matching cost with the real lists: 141,735 rules read, compiled in about 110 ms, about 26 MB, a request checked in about 2.5
microseconds (`cargo run --release -p iyakki-web --example blocker_stats -- LIST...`).

### Not done in this pass

Measured since: see "Interactive performance" (Skia mode and threads, the frame path, the shell's drawing). Still not done: WebKit
cache sizes, memory budgets, any change that needs a rebuild.

## Interactive performance (measured on a 2020 MacBook Pro)

Owner request: "make the browser faster"; YouTube and Google sites felt slow in the browser tile. **Loading is not the problem**:
`page-timing.sh 2` gave Google search 5.3 s, YouTube 2.6 to 4.2 s, BBC 2.7 s, and the web-tuning words `cpu:8` and `gpu:4` did not move
them (3.7 s and 3.6 s: noise). What is slow is **what happens after the load**: scrolling, video, animation in the streamed tile.
This section is that measurement, the cause, what was changed and what was not.

### The kit

`dist/webview/interactive-timing.sh` runs on the device (`sh interactive-timing.sh [-s secs] [-z WxH@PCT] [-e "K=V ..."] [-q quality]
[-v video] [page...]`; pages `google youtube watch wikipedia bbc` or an address). It starts the **same client the shell uses**
(`iyakki-webview`, through the `probe` example, which gained the steps `measure:SECS[,WHEEL,STEP_MS]`, `bench:W,H` and
`--bench W,H`) with the **same environment the shell gives the engine** (`webengine::helper_env`), in a scratch profile under `/tmp`,
and while it scrolls (a wheel notch every 25 ms, turning round every 2 s) or plays a video it records: pictures a second that the
helper's stream delivered, the longest gap between pictures, how much of the picture each one changed, the CPU of every process from
`/proc/<pid>/stat` deltas (WPEWebProcess, WPENetworkProcess, the helper, the shell, anything else above 1 %), the busiest web
process **thread by thread**, memory, and for a YouTube page the player's own counters (`getVideoPlaybackQuality`: size, frames
shown, frames dropped). Build the probe with `cargo build --release -p iyakki-webview --example probe --target
x86_64-unknown-linux-musl` and copy it to `/tmp/probe` on the device (there is no `scp`: `ssh root@DEVICE 'cat > /tmp/probe && chmod
+x /tmp/probe' < probe`). What it cannot see is the shell's own drawing: that is in the shell's log line `shell: N frames in 10s,
draw X ms, screen write Y ms, slowest Z ms, M tile-only` (`dmesg | grep frames`) while the real tile is used.

### Findings (BEFORE: the image as it is, nothing set)

Machine: MacBookPro16,2, Core i5-1038NG7 (4 cores, 8 threads), 16 GB, 2880x1800 panel shown at 2560x1600, kernel 6.12, image `macweb`
(no iris), nothing else running (the k3s dev cluster idles at 3 %). Page area 2560x1400 at scale 100 (what the shell asks for), the
engine drawing with llvmpipe. Each line is one 10 s run unless several are listed.

| what | pictures a second the tile gets | longest gap | tile changed per picture | web process CPU | notes |
|---|---|---|---|---|---|
| Wikipedia, long page, fast scroll | 20.0, 24.9 | 0.43 s, 0.08 s | 69 % | 309 %, 380 % | 8 llvmpipe threads at about 45 % each |
| BBC News, fast scroll | 19.0 | 0.28 s | 91 % | 370 % | |
| YouTube search results, scroll | 21.2 | 0.8 s | 68 % | 387 % | |
| Google results, scroll | 11.3, 15.7, 16.6 | 0.7 to 1.1 s | 85 to 89 % | 190 to 288 % | a short page: the scroll runs out, noisy |
| YouTube watch page, 24 frames a second clip, the player chooses | 6.0 | 0.56 s | 57 % | 648 % | the player picks 1080p; 5 to 7 frames dropped |
| the same at 720p | 9.7 | 0.37 s | 52 % | 720 % | |
| YouTube watch page, 60 frames a second clip, the player chooses | 1.8 (and 1.7, 1.9 with the other settings below) | 1.2 to 2.0 s | 45 to 55 % | 390 to 600 % | 1080p; more frames dropped than shown |
| the same at 720p | 6.2 to 8.1 (4 runs) | 0.2 to 0.8 s | 52 % | 566 to 670 % | **428 to 564 of 650 to 800 frames dropped** in 4 runs |
| the same at 480p | 10.9 | 0.11 s | 51 % | 698 % | nothing dropped |
| the shell, login screen (nothing to draw but its own scene) | 7.5 | | | init 20 to 29 % | `draw 23.4 ms` a frame at 2560x1600: the whole scene |

What these say, in order of importance:

1. **The engine's rasteriser threads are the cost.** Scrolling a page spends nearly all of its CPU in `llvmpipe-0..7` (Mesa's software
   GL: the compositor draws every tile of the page into the output buffer on the CPU, 8 threads, about 45 % each); the web process's
   own main thread is at 7 %, the compositor thread at 15 %. The helper (copies, row compare, `pwrite`) costs 15 to 20 % of one core,
   the shell's pread and copy a few percent: **the frame stream is not the bottleneck**, the engine is.
2. **Video pays for pixels twice**: decode and composite. A 60 frames a second VP9 clip at 720p spent about 340 % of CPU in llvmpipe
   and about 200 % in the four decoder threads (`multiqueue*:src`): all 8 threads busy, 4 of them hyper-threads. The decoder loses:
   the player's counters show **61 to 80 % of the frames dropped**, and the tile only gets 6 to 8 pictures a second. At 1080p (what the
   player chooses on this page size) the four decoder threads are at 60 to 74 % each and the clip is a slideshow (1.7 fps).
3. **The tile's frame rate follows the pixel count**: the same 720p clip in a quarter of the pixels gave 27 pictures a second (below).
4. **A thread of the web process, `audiosink-ringbuffer`, uses 83 to 99 % of a core for as long as any video plays** (also muted, and
   also with the web PCM of `softvol.rs`). `/proc/asound/card0/pcm0p/sub0/*` shows why: the T2 audio device runs with `period_size: 1`
   (one frame) and `avail_min: 1`, so the sink is woken for every one of the 48,000 frames a second. That is the audio driver
   (MAC-HARDWARE.md, "period of one frame"), not the browser; it costs one of the 8 threads in every video measurement above and is
   **not fixed here**. A larger period needs a layer between the sink and the device that the driver accepts (a dmix with a sized
   period was not tried on this device) or a driver change.
5. **Not the cause**: Skia's CPU raster (`WEBKIT_SKIA_ENABLE_CPU_RENDERING=1`: 24.8 and 24.1 pictures a second on Wikipedia and BBC,
   but 378 % and 505 % CPU, about 25 to 40 % more than the default for the same rate), GL painting on the main thread (`gpu:0`: 11.9
   fps on Google against 15.7, worse), the page-load speed, and the frame stream (see 1).

### What changed, with numbers

Measured with the same kit, nothing else changed between a pair of runs; **run to run noise is large** (the same configuration gave
6.4 and 8.1, 11.3 and 16.6 pictures a second), so only differences much larger than that are claimed.

| change | where | result (before -> after) |
|---|---|---|
| llvmpipe threads: half the logical CPUs instead of all (`LP_NUM_THREADS`, `webengine::llvmpipe_threads`; the web-tuning word `lp:N` overrides) | `crates/iyakki-agent/src/webengine.rs` (needs a new `iyakki` in the image) | **Video: frames dropped 428 to 564 of about 700 (4 runs) -> 0 (4 of 4 runs, 60 fps at 720p)**: the decoder gets cores of its own. Wikipedia scroll: 380 % -> 271 and 274 % of CPU for 24.9 -> 25.5 and 25.5 fps (-29 %). BBC: 370 % -> 356 %, 19.0 -> 23.0 fps. YouTube results: 387 % -> 279 %, 21.2 -> 15.3 fps (one run each: noise). The visible video frame rate did not change (5 to 8 pictures a second at 2560x1400): the pixels are the limit. With 2 threads the scroll CPU fell further (192 %) but one page got slower, so half was kept |
| **Smooth pages** (Settings -> Browser, key `m`; default **off**): the engine draws at half the width and height (a quarter of the pixels, the zoom halved so the layout is the same), the shell stretches the picture 2x with a 3:1 weighted average | `browser_full.rs`, `iyakki-webview/src/blit.rs`, `browser_config.rs`, `settings.rs` | **60 fps clip at 720p: 6.2 to 8.1 -> 27.4 pictures a second (6 runs: 26.9 to 27.5), longest gap 0.2 to 0.8 s -> 43 to 50 ms, 0 gaps over 100 ms, dropped frames 0 (with the thread change; 251 in one run without it)**. 24 fps clip at 1080p (the player's own choice): 6.0 -> 24.0 pictures a second, nothing dropped, longest gap 62 ms (the clip's own rate). Scrolling text pages: Wikipedia 20.0/24.9 -> 26.8 fps at 131 % CPU (was 309 %), BBC 19.0 -> 15.6 and YouTube results 21.2 -> 15.6 (both at 35 % less CPU) - **no consistent scroll gain, a CPU saving**. The price is softer text and pictures (half the resolution, stretched) |
| the shell paints **only the page rectangle** when only the engine's picture changed (`App::redraw_kind` = `Live`, `Shell::live_update`, `Framebuffer::present_banded_rows`; a whole scene is still drawn at least every 100 ms and on any other change) | `shell_live.rs`, `backend.rs`, `fb.rs`, `browser_full.rs` | **Not measured on the Mac** (the shell needs a signed-in session and nobody was there; the shell cannot be driven from outside). Known costs: a whole scene is `draw 23.4 ms` even on the login screen at 2560x1600 (12 to 95 ms with the tile, from the owner's log lines), against a tile copy of 1 to 3 ms plus the screen write of the changed rows (2 to 5 ms). Tests prove the in-place paint equals a whole redraw pixel for pixel, sharp and smooth. Expect up to the engine's rate (the shell could not draw faster than about 1 / (25 + 10) s before), but this is **an expectation, not a result** |
| the white fill of the whole page area every frame is gone (only the strips the picture does not cover are filled) | `browser_full.rs` | 14 MB of writes less per whole redraw (0.4 ms on the Mac, `probe --bench`) |
| `LP_NUM_THREADS`, `nice:N` and `fps:N` words and a multi-word `web-tuning` file (`cpu:4 lp:2 gstgl:off`) | `webengine.rs`, `helper.c` | an A/B of `nice` (the engine 5 below the shell) while a video played gave no consistent change in the shell's `draw` time (37 ms then 23 ms for the same setting in two runs), so it stays **off** by default |

`probe --bench 2560,1400` on the Mac (per call, milliseconds): channel swap of a whole picture 4.5 (the loop as it was) and 6.2 (the
function that is now shared; no gain, the same work), white fill 0.42, 1:1 copy of a whole picture 1.0, a 1 % rectangle 0.007, the 2x
stretch of a whole 1280x700 picture to 2560x1400 12.8 (the first version took 27 on the development PC, 16 after a second pass; a
640x350 picture to 1280x700 takes 3.0).

### What did not help, or was not done

* **Skia CPU raster** and **GL painting on the main thread**: worse (see 5 above). `WEBKIT_GST_DISABLE_GL_SINK=1`: 8.8 pictures a
  second against 6.2 for a 720p clip in one run each, inside the noise; not adopted (the web-tuning word `gstgl:off` stays for trying it).
* **Fewer llvmpipe threads than half**: `lp:2` saved more CPU but one page lost frame rate.
* **Video quality as a setting (Automatic, 720p, 480p)**: not built. YouTube has no stable switch; what works is YouTube's own
  preference record: writing `localStorage["yt-player-quality"]` (what the player writes when someone picks a quality in its menu) and
  reloading made the player start at 480p (854x480, nothing dropped) where it had chosen 1080p, tried by hand on the Mac. Choosing the
  quality **once in the player's own menu (the gear)** does the same and YouTube remembers it in the browser profile, with no code. A
  setting would have to run a script on `youtube.com` after its first load and reload once: too site-specific to ship blind.
* **Codecs, from the image**: only `vp9dec` (libvpx), `vp8dec`, `opusdec` and `vorbisdec` exist as decoders (no `dav1d`, no H.264 or
  AAC decoder: the parsers are there, the decoders are not). `MediaSource.isTypeSupported` answers VP9, VP8 and Opus true and `avc1`,
  `av01` and `mp4a` false, so **YouTube always serves VP9 and Opus, decoded in software**; there is no cheaper codec to ask for.
* **WebKit knobs**: `WEBKIT_SKIA_*` and the thread counts were tried (above); `WEBKIT_DISABLE_COMPOSITING_MODE` and the old
  `WEBKIT_SKIA_PAINTING_THREADS` do not exist in 2.50.5. Names confirmed in the shipped binary with `strings`: `WEBKIT_FORCE_VBLANK_TIMER`,
  `WEBKIT_GST_DISABLE_GL_SINK`, `WEBKIT_GST_USE_VIDEOCONVERT_SCALE`, `WEBKIT_GST_DMABUF_SINK_DISABLED`, `WEBKIT_GST_ALLOW_PLAYBACK_OF_INVISIBLE_VIDEOS`,
  `WEBKIT_GST_MAX_AVC1_RESOLUTION` (H.264 only), `WEBKIT_SHOW_FPS`, `WEBKIT_DISABLE_MEMORY_PRESSURE_MONITOR`.
* **The frame stream** (three copies in the helper): not the cost (15 to 20 % of one core), so the helper's `flush` was not rewritten;
  WPE gives no damage rectangle for `wl_shm` buffers, the row compare stays. Dropping the alpha loop would save about 2 ms a frame.
* **Scroll compositing offsets, adaptive pacing**: WPE reports no scroll-only frames; the helper already waits for the shell's `Ack`
  before it sends the next picture (a slow shell slows the stream by itself) and the shell draws at most every 33 ms.
* **The real fix is the Intel GPU driver** (`WEB_IRIS=1`, building in the background; not in any image): llvmpipe's CPU compositing
  is 60 % of every number above. Smooth pages and the thread change are what helps until then.
* **Not measured**: the shell's drawing with a live tile (above), the effect of the in-place paint on the screen, and any change on a
  Mac mini or a machine with a working GPU driver (where `LP_NUM_THREADS` is not set at all).

### Try it and measure it

    sh /tmp/it.sh -s 10 wikipedia bbc                  # scrolling, the image as it is
    sh /tmp/it.sh -s 10 -e LP_NUM_THREADS=4 wikipedia  # the same with 4 llvmpipe threads
    sh /tmp/it.sh -s 10 -z 1280x700@50 -q hd720 watch  # Smooth pages: a quarter of the pixels, 720p video
    sh /tmp/it.sh -s 10 -e "WEBKIT_SKIA_ENABLE_CPU_RENDERING=1" bbc

Smooth pages in Settings -> Browser (key `m`) takes effect at once on open pages (the engine is resized and its zoom halved). The
default `LP_NUM_THREADS` and the tile-only drawing are in the `iyakki` program, so they need a new image or `iyakki dev update`;
the helper's `IYAKKI_WEBVIEW_NICE` needs a rebuild of the web image.

## Hardware pictures in the helper (GPU path)

Cause found on a 2020 MacBook Pro (Ice Lake, Mesa iris): the helper called `wpe_fdo_initialize_shm()`, so the compositor inside it offered
only `wl_shm`; Mesa's Wayland EGL platform then fell back to software (`kms_swrast`), and the web process ran 8 llvmpipe threads with
0 ns of GPU render time even though iris works (a standalone EGL test on the render node reports Iris Plus, ES 3.2).

Change (`dist/webview/wpe-helper/helper.c`): at start the helper opens `/dev/dri/renderD128` (to 131, or `IYAKKI_WEBVIEW_RENDER_NODE`),
makes a GBM EGL display and a GLES2 context, calls `wpe_fdo_initialize_for_egl_display` and creates the view with
`wpe_view_backend_exportable_fdo_egl_create`. Each exported EGL image (a dma-buf) is bound to a texture and read back with
`glReadPixels` (BGRA when `GL_EXT_read_format_bgra`, else RGBA swapped on the CPU) into the same private picture the shm path fills,
so the row compare, the shared file and the protocol are unchanged. **Damage:** the GPU path gives no damage rectangle either (the
`wl_shm` path did not), so tile-only damage is still the row/column compare in `flush`; the read-back itself is always a full frame.
Fallback to `wl_shm` happens automatically when: `LIBGL_ALWAYS_SOFTWARE` is set, `IYAKKI_WEBVIEW_GPU=0`, no render node opens, EGL lacks
`EGL_WL_bind_wayland_display`/image support, no ES2 context, or fdo refuses the display. The helper logs one line (`graphics: ...`) and
its 5 s stats say `stats(gpu)` or `stats(shm)` with `IYAKKI_WEBVIEW_DEBUG=1`. Builds only against EGL, GLESv2 and gbm that the image already has.

Status: compiles with the web toolchain. Real-hardware result: see the table below (filled in only from measurements).

Baseline on the Mac before the change (same video playing, load average about 10 because k3s and Elasticsearch were also running):
web process 4094 ticks in 10 s (4.1 cores), of which 8 llvmpipe threads 2667 (2.7 cores), GStreamer software decode queue 860,
`drm-engine-render` 0 ns.
