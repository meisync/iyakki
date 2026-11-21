# dist/split: the scripts of the UI/agent split (F-SPLIT-03)

Every script runs from anywhere inside the repository, changes files in place and prints what it did. Commit before a run, build with
`~/iyakki-work/bin/rcargo . check ...` (or `mcargo` for a quick non-Linux compile), and put back what does not compile
(`git checkout -- <files>` or `revert_failed.py`). Regenerate the ratchet after a run: `IYAKKI_UI_USES_WRITE=1 cargo test -p iyakki-core --test ui_agent_uses`
and lower `docs/development/ui-agent-uses.allow`.

| Script | What it does | Example |
|---|---|---|
| `gen_facade.py` | pass 2: a generated facade for the free functions the UI calls (`iyakki-agent-api/src/generated.rs`) | `gen_facade.py --rewrite`, then `revert_failed.py` |
| `move_types.py MODULE...` | pass 1: pure types of `agent/src/MODULE.rs` that the UI names go to `iyakki-shared`, the agent re-exports them | `move_types.py voicehelper --dry` |
| `pass1.py` | `move_types.py` plus compile and exclusion of what does not build | `RCARGO="rcargo $PWD" pass1.py MODULE` |
| `move_submods.py` | whole pure submodules of directory modules (reference only) | |
| `merge_module.py M...` | pass 3: merge a whole agent module into the same-named module of `iyakki-shared` (`pub use` stays in the agent) | `merge_module.py boarddeps coverage` |
| `merge_store_sub.py X...` | the same for `agent/src/store/X.rs` -> `iyakki-shared/src/store_X.rs` | `merge_store_sub.py trust catalog` |
| `move_items.py FILE SHARED Names [--ui PATH]` | pass 5: move NAMED items (struct, enum, type, const, fn) and their `impl`s into a shared module | `move_items.py store/lifecycle.rs store_lifecycle Life,Report --ui iyakki_agent::store::lifecycle` |
| `rewrite_services.py MEMBER REPLACEMENT [--tests]` | pass 6: `cx.services.MEMBER` becomes `cx.REPLACEMENT` in the UI, one member at a time | `rewrite_services.py dir 'data_dir()' --tests` |
| `revert_failed.py` | puts back the lines a rewrite made that do not compile (reads the compiler output) | |

After a merge or move, errors of the kind "unresolved import", "cannot find function X in module Y" name what the moved code still needs from the agent:
move that too, copy a plain function, or put a trait in `iyakki-agent-api` and implement it in the agent (see `docs/development/SPLIT-GROUPS.md`).
Tests that need agent types stay in the agent (`#[cfg(test)] mod X_tests;` in `iyakki-agent/src/lib.rs`, `use iyakki_shared::X::*;`).
| `rewrite_services2.py RECV MEMBER REPLACEMENT` | the same for any receiver regex (`self\.0` in the ports, `self\.env` in the shell), non-test code only | `rewrite_services2.py 'self\.0' radio 'radio()'` |
| `gen_trait.py FILE STRUCT TRAIT m1,m2,...` | writes `/tmp/gen/TRAIT.api.rs` (a trait for iyakki-agent-api) and `TRAIT.impl.rs` (the delegating impl for the agent) for the named methods of a store struct | `gen_trait.py crates/iyakki-agent/src/jobs.rs Jobs JobApi get,list,start` |
| `gen_appcx.py m1,m2,...` | prints `AppCx` wrappers (`self.services.m(..)`) for `pub fn m` of `impl Services` (inherent only; trait methods are reported MISSING) | `gen_appcx.py quick_upstream,streamer` |
| `nontest_uses.py REGEX [sites]` | counts (and lists) the matches in the non-test part of the UI crates (before the first column-0 `#[cfg(test)]`) | `nontest_uses.py '\bservices\.[a-z_]+' sites` |
| `add_handle.py NAMES`, `gen_handle.py NAMES`, `h_iter.sh [cargo args]`, `handle_errs.sh`, `fix_handle_uses.py`, `fix_test_real.py ERRFILE` | alpha.6 split H: grow `ServicesHandle` (iyakki-agent-api `facets_h.rs` and its implementation) from compile errors of the UI crates, and repair the uses in UI code and in tests | `h_iter.sh -p iyakki-shell --lib` |
| `gen_services.py [--check]` | `RemoteServicesHandle` as a `ServicesHandle` (F-SPLIT-03 part 1, D-SPLIT-03-HANDLE): `iyakki-ui-rpc/src/gen_services.rs` (the client impl and the core's dispatcher `serve_services`, the `ServicesHandle` root object); `services-unserved.txt` lists the plain methods whose types cannot cross yet | `gen_services.py` |
| `gen_unserved.py [--check]` | `gen_unserved.rs`: an `Unserved` object of every facet trait the remote handle answers with when the core does not serve that trait yet (every method answers the quiet value, `Err("not served over the UI socket")`, `None`, empty, 0) | `gen_unserved.py` |
| `gen_rpc.py` supertraits and store kinds | `SUPERS` (`StoreApi: StorePublish` is one object), `STORE_TRAITS` (arguments of kind `fetch`, `audit`, `actor`, `policy` and `dir` are supplied or re-read by the core, D-SPLIT-03-STORE), `CACHED_OK` (a borrowed accessor with a default body read once) | |
| `gen_rpc.py --check` | writes nothing and fails when `gen_remote.rs` or `gen_serve.rs` differ from what the traits give now (the test `iyakki-ui-rpc::services_handle` runs the three `--check`s) | |
