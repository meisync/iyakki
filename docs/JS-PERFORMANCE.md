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

## Faster script start: measured plan (F-BROW-99, alpha.7, 2026-11-19)

What a page pays before its own scripts run, and what it pays for them, measured on the AMD build host (release build, a shared machine
with a load of 10 to 18, so read the figures as "about"; best of several runs). Tools: `IYAKKI_JS_STATS=1` makes `JsEngine::new` print a
`JS-START` line (the wasm instance, QuickJS's own start, the platform preludes and the six slowest preludes), and
`cargo test --release -p iyakki-js --test bench -- --ignored --nocapture --test-threads=1 bench_start_phases` runs six blank pages with it.

| Part of a new engine (blank page, compiled forms cached) | Time |
|---|---|
| wasm instance (`Linker::instantiate`, `_initialize`) | 0.03 to 0.11 ms (0.11 only the first time) |
| QuickJS's own start (`jb_init`) | 0.22 ms |
| the platform preludes, evaluated from their compiled form | 3.9 to 4.3 ms |
| of which the `dom` prelude (136 KB of source) | 2.7 ms |
| of which `indexedDB` (26 KB) / `window`, `localStorage`, `fetch`, `url`, `console` | 0.29 ms / 0.12 to 0.14 ms each |
| a blank page in all | 4.7 to 5.0 ms (12 ms the first time: the compiled engine is read from disk) |

Parsing the preludes is already gone (they come from the compiled form); what is left is running them: the definitions of the page object,
its classes and its listeners. A page script is the other half: from the earlier lane measurements, a function-heavy script costs about 0.09 ms
per KB to parse and about 0.02 ms per KB to load from the compiled form (32 KB: 2.9 ms against 0.8 ms; 100 KB: 8.5 ms against 1.7 ms), and
the scripts' own run time (BBC's bootstrap 0.2 to 0.4 s, YouTube about 1.5 s cached) is the interpreter running page code: 30M loop turns in
3.6 s, `fib(30)` 0.23 s, 200k host calls 0.2 s (table in the row's notes).

**What this says.** For a page that loads a few hundred KB of script, the start (5 ms) and the parse of what the compiled cache already holds
(tens of ms) are NOT where a second or more goes: it is the page's scripts running in QuickJS inside wasmtime, and the host calls they make
(38 700 per YouTube page after the bridge work). A new snapshot would save about 3 ms a page; only the interpreter, or less script work, moves
the big numbers.

Plan, in the order it should be done, each step measured with the tools above before and after, kept only if it is clearly faster and every
test of the crate passes:

1. **Lazy page object (saves up to about 2.5 ms a page, low risk).** Run only the part of the `dom` prelude every page needs (`document`,
   `window`, the element base class) at start; define the rest (the `HTMLxxxElement` classes, `Range`, `Selection`, `MutationObserver`, the form
   and media classes) as accessors on `globalThis` that run their part of the prelude on first use and replace themselves. Risk: a page
   that enumerates `window` sees the accessors; fix by making them enumerable data-like getters. Test: the existing `dom_*` tests plus a new
   one that every global name exists before and after the first use.
2. **Heap snapshot of the platform (about 3 ms a page, medium risk, only if step 1 is not enough).** Run the preludes once per process in a
   scratch engine, copy its linear memory and the mutable wasm globals, and start each new engine from that copy instead of running the
   preludes (the Wizer approach, done by hand because the engine has host imports). Needs: the host-call tables of every module to be
   re-bound by name (they are closures over per-page state, not part of the snapshot), the preludes' own state (ids, caches) to be position
   independent, and the copy to be done with wasmtime's copy-on-write memory image so it costs microseconds. Not done: a stale snapshot would
   be a silent wrong page, so it needs a build-time check that the snapshot's key is the hash of every prelude text and of the engine.
3. **Parse cost of small scripts (about 0.07 ms per KB saved on the second visit).** The compiled cache starts at 16 KiB per script
   (`SCRIPT_CACHE_MIN`, alpha.7 lane d1). Pages with hundreds of small inline scripts keep paying; keeping the compiled form of inline scripts
   by a content hash in a per-user pack file (one read per page instead of one per script) would remove it. Measure first on a real page with
   `bench_page`: the saving shows only above about 200 KB of small scripts.
4. **The interpreter itself (the only step that moves YouTube's 1.5 s).** In order of effort: build QuickJS with `-O3 -flto` and wasm
   `-msimd128 -mbulk-memory` (the build script `dist/js/build.sh` has neither flag yet; try each alone and keep a flag only if the bench table
   improves and the node comparison test still agrees), then wasmtime settings (`Config::cranelift_opt_level(Speed)` is on; try
   `memory_reservation` and guard pages so bounds checks go away, `parallel_compilation` off for a one-time engine compile), then host-call
   batching (the guest asks for several DOM answers in one call; 38 700 calls at 0.5 to 1 us each are 20 to 40 ms, small against the
   interpreter), and last the choice of another interpreter, which is a different project.
5. **Skip scripts that are not the page (largest practical gain on real sites).** Analytics, ad and tracking scripts are the biggest share of a
   heavy page's script time; the blocker (`iyakki-web::blocker`) already drops listed addresses before they are fetched, and a page's run
   report names every script over 30 ms (`IYAKKI_JS_STATS=1`), so the report decides which list entries to add. This needs a real-page run with
   network, which this host cannot do from a test.

Done in this round: the `JS-START` timing and `bench_start_phases` (so steps 1 and 2 can be measured the same way on any machine, a Mac
included); nothing in the interpreter changed. Not done: steps 1 to 5 (each is measured against the table above), and a measurement of a real
site (needs network).

### Step 3 done (alpha.7, lane W3): pack file of small compiled scripts

Scripts from 512 bytes to 16 KiB keep their compiled form in one per-user file (`smallpack-<engine digest>.pack`, `iyakki-js/src/pack.rs`),
read once per process, merged with what another process stored when an engine that added entries is dropped, capped at 24 MB (oldest entries
first), trusted under the same owner, mode and checksum rules as the single files. Measured with `bench_small_scripts_pack` (80 inline scripts of
about 3 KB each, release build, AMD build host, shared machine, fresh cache folder): the first page 40.5 ms (parses, stores), the pages after 10.1 to
11.9 ms, so about 30 ms a page for 240 KB of small scripts. The blank-page start is unchanged (4.5 to 5.7 ms, `bench_start_phases`). `page_scripts_chrome`
(KI-996/997) passes. Steps 1, 2, 4 and 5 are not done: step 1 changes what a page sees when it enumerates `window` and wants the node comparison tests
widened first; the interpreter and wasmtime memory settings were each measured within noise in the earlier round.

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
