# Speed of the page-script engine

The browser runs page scripts in QuickJS-ng compiled to WebAssembly (`dist/js/build.sh`) inside wasmtime
(`crates/iyakki-js/src/engine.rs`). This note records where the time went, what was changed, and what is still slow.
Numbers are from one machine (16 threads, a long Buildroot build and other work running beside the tests, so single runs
vary by 10 to 20 %; the "after" column is the best of three, the "before" column is the first measurement with the
original code, one run each unless marked). Seconds, release build.

## How to measure

`crates/iyakki-js/tests/bench.rs` (all `#[ignore]`d):

    cargo test -q --release -p iyakki-js --test bench -- --ignored --nocapture --test-threads=1 bench_loops
    # also: bench_strings_arrays_json  bench_dom  bench_startup  bench_hostcall  bench_page

* `bench_page` replays a real page from a disk cache (`target/bench-cache`, filled with `curl` on the first run, so later
  runs measure the engine and not the network). `IYAKKI_BENCH_URL` picks the page, `IYAKKI_BENCH_UA` and
  `IYAKKI_BENCH_CACHE` pick another cache and user agent (YouTube sends a different page to a Chrome 120 agent: more
  scripts, more work), `IYAKKI_BENCH_PRE=file.js` runs a script before the page's own.
* `IYAKKI_JS_STATS=1` prints, for a page run: the phases (parse, engine start, scripts, load events and timers), every
  script that took over 30 ms, and the host calls by `module.op` with their counts and times.
* `IYAKKI_JS_PROFILE=1` samples the scripts: every engine interrupt tick (about every 10 000 loop turns or calls) the
  script stack is counted, and a self/inclusive table is printed at the end. (`perf` could not be used here: the kernel's
  `perf_event_paranoid` is 4, and the wasm code has no symbols anyway.)
* The `probe_scripts` test in `iyakki-shell` (the original benchmark) could not be built during this work: the shell crate
  did not compile because of other people's unfinished changes. It uses the same `run_page`, so the numbers carry over,
  except that it fetches over the network.

## What the numbers say about the starting point

The task said YouTube took about 5.5 minutes with every limit lifted. That was **not reproducible here**: with the original
code the page's scripts took 2.4 to 5.3 s (varying with the user agent served), once the 11 MB bundle and the pages' other
scripts were on disk. Over the network the first run took 10 to 12 s, of which 4.4 s were 207 downloads made by the scripts
themselves (host time, which the engine does not count against the script). Where did 5.5 minutes go, then? The original
host calls tell: 1 393 `dom.query` calls cost 2.0 s (1.5 ms each) and `dom.clone` copied the *whole node table* on every
call, so the cost grows with the page. A page with more nodes than the one served here (a signed-in or consent-flow page, a
longer run after the `__shady` error was fixed) multiplies that. The quadratic parts are fixed (below); the 5x and
"under 60 s" targets are met by a wide margin on the pages I could get, but I could not run the one that took minutes.

## Where the time went (original code)

| Part | Cost |
|---|---|
| Compiling the engine (wasmtime, Cranelift) | 0.7 to 1.1 s on every process start |
| Parsing the 11 MB bundle (QuickJS compiles all of it, nothing is lazy) | 1.1 to 1.3 s on every page load |
| Host calls into the DOM | `dom.query` 1.5 ms per call (parse the selector, build a list of every element, test them all, then cut to one for `querySelector`), `dom.by_id` 67 us per call (a full scan), `dom.clone` copies all nodes, `dom.info`/`parent` many thousands of calls per page |
| Guest side of the DOM bridge | 2 to 3 us per call in `JSON.stringify` / `JSON.parse` alone, more than the host call itself (0.6 us); `isConnected` walked up the tree with one call per level on every insert; `tagName` and `getAttribute` were host calls |
| Fuel metering | every back-edge and call paid for fuel accounting; replaced by epoch checks, see below |
| The interpreter | what is left: QuickJS running in WebAssembly |

## What changed

| Change | Effect |
|---|---|
| Compiled engine cached on disk (`Module::serialize`, key = SHA-256 of the wasm and wasmtime's compatibility hash, per-user private directory, checksummed, owner and mode checked) | engine start 1.12 s -> 0.007 s (cold: 0.7 s) |
| Fuel replaced by epoch interruption (a watchdog thread ticks every 50 ms while an engine is in use and sleeps otherwise) plus a tick budget counted in the engine's interrupt handler (`Limits::fuel` now means "about this many wasm instructions", counted as 150 000 per tick). The epoch trap is the hard backstop for code that never reaches the engine's own check, and reports `Sandbox` (the engine is not reused after it) | loops about 20 % faster; runaway loops, the time limit and the instruction budget are all still enforced (tests) |
| Engine built with `-O3 -DNDEBUG` (assertions off) | another 5 to 12 % |
| Compiled form of big scripts cached on disk (`JsEngine::eval_cached`, scripts of 128 KB or more; key = SHA-256 of engine build, script name and text; 512 MB cap with oldest-first removal; a bad file is a miss) | the 11 MB bundle 1.3 s -> 0.4 s the second time |
| `domtree`: `clone_node` no longer copies the table; `getElementById` and `#id` queries use an id index kept by the host; selectors are parsed once and kept; `querySelector` stops at the first match; `closest`, `contains`, `isConnected`, custom-element candidates (`dashed`), script discovery on insert are single host calls with no selector matching | `dom.query` 2.0 s -> 0.016 s on the YouTube page, `dom.by_id` 0.96 s -> 0 |
| Script side of the bridge: nodes arrive with their tag name (no `info` call), parent / children / document parts are remembered until the tree changes shape, attributes are read once per element and kept in step with writes, listener tables are created on first use, hot calls build their JSON by hand and skip parsing answers that are `null` or one number | `getAttribute`/`tagName` need no call; host calls per YouTube page 126 000 -> 38 700 |
| `__eval(source, name)` for scripts added by scripts | errors name the file instead of `<input>` |
| Not kept | `-flto`, bulk memory, sign-extension, `DIRECT_DISPATCH=0`, memory reservation settings, a size-class allocator in the engine: each measured within noise (0 to 4 %) and was dropped |

## Results

| Benchmark | Before | After | |
|---|---|---|---|
| engine start, cold cache | 1.12 | 0.70 | |
| engine start, warm cache | 1.12 | 0.007 | 160x |
| pure loop, 30 M turns | 2.14 | 1.55 | 1.4x |
| `fib(30)` | 0.127 | 0.115 | |
| strings, arrays, objects | 0.421 | 0.374 | |
| JSON, 20 round trips of 20 000 objects | 2.56 | 1.90 | 1.35x |
| regex and `replace` | 0.765 | 0.678 | |
| DOM: 10 000 elements created, attributes set, appended | 0.61 (1.73 with engine compile) | 0.25 | 2.4x |
| DOM: 400 `querySelectorAll` over 10 000 elements | 1.14 | 0.69 | 1.7x |
| DOM: 10 000 `getAttribute` + `tagName`, 10 x, and `getElementById` | 2.97 (about 1.9 without compile) | 0.40 | 4.7x to 7x |
| YouTube page scripts, Chrome 120 agent (more work), second load | about 5.3 (4.17 + compile 1.1) | 1.6 | 3.3x, and the page now runs further |
| YouTube page scripts, Iyakki agent, second load | 2.4 to 3.5 (stopped early by the `__shady` error) | 1.6 to 1.9 (runs to the end) | |
| YouTube, first load, cold caches | 12.2 incl. 4.4 s of downloads | 4.0 (pages read from disk, so no network time) | |

The DOM-heavy cases (what real pages do) improve 2x to 7x, loading the same big bundle again 3x, process start 160x. The
pure interpreter improved 10 to 40 %. Whole-page speed-up relative to the original on the pages available: 3x to 3.5x on
the heavier variant, which is **below the 5x target**; most of what is left is the page's own code running in QuickJS.

Where the YouTube page spends its 1.6 s now (second load): 0.02 s engine start, 0.5 s running the 42 scripts (the bundle:
0.4 s to load its bytecode and run its start-up), 1.1 s in load events and timers (Polymer element start-up), host calls
0.1 s. In the profile, 63 % is inside the app bundle, 11 % in the regular-expression engine (`String.prototype.match`), and
dom.js's `dispatchEvent`/`_invoke` is the only bridge code with a visible share (13 % inclusive, almost all of it the
handlers it calls).

## What is still slow

* The interpreter. QuickJS runs in WebAssembly, without a JIT; build flags, link-time optimisation, bulk memory, wasmtime
  memory settings and a custom allocator each changed it by 0 to 4 %. Only a different engine, or QuickJS outside the
  sandbox, would be a large lever, and neither fits the sandbox rules. (I did not measure a native build for comparison.)
* The first visit: parsing 11 MB of script (1.3 s) happens once per machine per script version.
* Every `querySelectorAll` answer is JSON text with a tag name per node; very large result lists cost about 3 us per node
  on the script side.
* `nextSibling` loops over thousands of siblings scan the sibling list on the host each call.
* The `perf` tool and wasmtime's guest profiler were not usable here, so the time inside the interpreter is attributed by
  script function (the sampler), not by wasm function.

## Platform gaps fixed on the way (YouTube)

* `TypeError: cannot read property '__shady' of undefined` (webcomponents-sd.js): the link accessors `host`, `hostname`,
  `pathname`... were defined for every element and ignored writes on anything that is not `<a>`/`<area>`; the Shady DOM
  polyfill assigns `this.host = element` on its shadow root and read back `undefined`. Writes on other objects now create
  an ordinary property.
* `attributes.getNamedItem` (and `setNamedItem`, `removeNamedItem`, `getNamedItemNS`) were missing.
* `new Audio()` threw "Illegal constructor"; setters for `style` (forwards to `cssText`), `classList` (to `className`),
  `text`, `selectedIndex` and `nodeValue` were missing, which Polymer's property effects assign.
After those the page's start-up runs without a script error. What it still needs to be drawn is the full engine.

## Safety notes

* The compiled-code cache is read only from a directory we created with mode 0700 (or found owned by us and not writable by
  others), from files we own that others cannot write, whose checksum matches. Bytecode is run only inside the sandbox, so a
  damaged file can at worst fail the script, not the host. Script cache names use SHA-256 so that another site cannot make a
  script of its own collide with a script of yours. `IYAKKI_JS_CACHE` overrides the directory.
* The limits are unchanged in kind: memory (StoreLimits), the stack, the wall clock (checked every tick and by the epoch
  watchdog), and an instruction budget. `Limits::default()` (budget 12e9, 20 s per call) and `TOTAL_WALL` (40 s per page)
  are unchanged: a 20 s call is already about 800 M loop turns, far more than any page here used.
