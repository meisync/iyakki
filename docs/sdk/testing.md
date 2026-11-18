# Testing

An SDK app is tested without a screen, a device or a network. The test loop for one app is seconds.

## The pieces

| Piece | What it is for | Where |
|---|---|---|
| `Host` | the same driver the shell uses; send events, record a frame | `iyakki_ui_sdk::host::Host` |
| `host.record()` | the frame as a `DisplayList`; read `texts()`, `a11y()`, `ops` | `list.rs` |
| `host.mouse(x, y, Phase::Down/Up, t_ms)` | a real tap through the gesture recogniser and the hit regions | `host.rs` |
| `host.touch = true`, `host.text_scale = 3.0` | the touch size rule and the largest text | `host.rs` |
| fake ports | `MemStorage`, `MemFiles`, `MemTerminal`, `MemMedia`, `MemBrowser`, `SharedAssistant` + `Turn::say/call` | the SDK modules |
| golden files | the first frame at three sizes, compared on every run | `tests/golden.rs` of a generated app |
| lints | `lint::style_literals`, the source scan, the manifest guard | in the generated tests |
| FakeCore | the core's own fakes for every facet (`FakeCore::file_tree_fake()`, `terminal_fake()`, `assistant_fake()`, `media_fake()`) for tests of the shell adapters | `crates/iyakki-core-fake` |

## The checks every app keeps

Copy them from the generated `tests/golden.rs`, they are the checklist in code: render at 360x740, 800x480 and 1280x720 and at text 1x and 3x; every control named, inside the pane, at least 44 high on touch; a tap, a key and a `Text` event do what they should; the storage port is used and its refusal is shown; the source has no styling numbers, no colour literals and no machine access; the manifest names no other IYAKKI crate.

## A test, step by step

```rust
let mut h = Host::new(MyApp::new(), Look::default(), Arrangement::single(360, 740),
                      Box::new(MemDock::knowing(&["my-app"])), Box::new(MemWallpaper::default()));
let list = h.record();                                   // draw one frame
let (x, y) = centre(&list, "More");                      // find a control by its name
h.mouse(x, y, Phase::Down, 0); h.mouse(x, y, Phase::Up, 60);   // tap it
assert!(h.record().texts().contains(&"Pressed 1 times"));
```

The helper `centre` is in the generated test and in `crates/iyakki-ui-sdk/tests/docs_recipes.rs`.

## Embedded components in a test

Give the app the fake port and drive it with ticks: files (`MemFiles::put`), terminal (`MemTerminal`), audio (`MemMedia::put_file`, `advance`), chat (`SharedAssistant::new(vec![Turn::call("text", "tool", "{json}"), Turn::say("Done.")])`). Assert the order the person would see: the tool waits, the card is on screen, Allow runs it. `docs_recipes.rs` has one of each.

## Real-adapter tests

The shell-side adapter that turns an SDK port into the core's call is tested against the core's fakes, not the SDK's memory ports: `FakeCore` in `iyakki-core-fake`. These tests live next to the adapter in `crates/iyakki-apps`. An app author writes one only when adding a component or an adapter.

## The contract tests (run before you hand work over)

`cargo test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses --test feature_matrix --test kernel_single_reader`. They guard the layer rules (no `iyakki_agent::` in the UI, ratchets only shrink), the feature matrix and the single kernel reader. A new app crate that depends on anything but the SDK trips them.

## app-check: the fast author check

`./run.sh app-check PATH` (or `python3 dist/sdk/app-check.py PATH`) runs only what an app author can hit and prints a short numbered list: `N. file:line [rule] what is wrong`, then `Fix:`, and `Before:` and `After:` where a snippet helps. Two parts: the static checks of the source and the manifests (other IYAKKI dependencies, machine access, colour literals, styling numbers, unnamed controls, a changing tool that does not ask, `app.toml`), which take milliseconds and need no build; then the app's own `cargo test`, whose failures are turned into the same list. `--no-tests` runs the static part only; `--contract` also runs `boundary`, `dependency_ratchet` and `ui_agent_uses` (inside the repository); `--cargo "CMD"` runs cargo through a wrapper (a remote build host). Exit code 0 means no problems. `python3 dist/sdk/app-check.py --selftest` checks the checker against samples and the recipes.

## The loop (build and test only what changed)

See `docs/development/FAST-LOOP.md`. For one app: `cargo test -p iyakki-app-NAME` is a few seconds when the cache is warm; `dist/scripts/test-affected.py --quick --run` runs only the crates you changed. The SDK's own tests: `cargo test -p iyakki-ui-sdk`.

## Docs are tests too

`cargo test -p iyakki-ui-sdk --test docs_recipes --test docs_fresh` compiles every recipe and fails when a reference page or a code block in these guides is stale.
