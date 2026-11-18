# Every failure an app author can hit, and the fix

| Where it appears | Message starts with | Fix |
|---|---|---|
| `app-check`, the app's tests | `WHAT: a control has no name` | give the widget a label or `Hit.name` |
| same | `WHAT: ... leaves the pane` | lay out from `c.size()`, `m.gutter()`; `fit()` text |
| same | `WHAT: ... under 44 units high on touch` | widgets or `m.min_target()` |
| same | `no_styling_numbers_in_the_drawing_code`, `styling-number` | `m.size/gap/radius/line` |
| same | `WHAT: ... contains rgb(` / `std::fs` ... | colours from `cx.colors()`; use a port |
| same | `WHAT: Cargo.toml [dependencies] has ...` | delete the IYAKKI dependency |
| same | `the phone frame changed` | intended: delete `tests/golden`, rerun, read the diff, commit; else undo |
| `ui_agent_uses` | `the UI ... now names iyakki_agent::` | ask the core (`cx.core.<facet>()`) or take a port; if no facet exists, stop and ask |
| `dependency_ratchet` | `a tile crate depends on an IYAKKI crate it may not` | remove the dependency |
| `boundary` | row missing for a file that spawns a process or touches the base OS | the app must not spawn processes; if a core file does, add a BOUNDARY.md row (owner decision) |
| `feature_matrix` | `expected 9 columns`, `bad ID`, `duplicate ID`, `Test N must be one of 0, 30, 50, 70, 100`, ceiling problems | see `.claude/skills/iyakki-feature-rows`; run `python3 dist/features/migrate-impl-test.py` |
| `events-missed`, `events-late` (app-check) | the app registers for events but never handles `missed` / never looks at `late` | `.on_missed(...)` on the EventRouter; check `e.late` before ringing or starting something (docs/sdk/events.md) |
| docs | `the SDK docs are stale` | `python3 dist/sdk/gen-widget-docs.py`, commit `docs/sdk` |
| `chat_reachability_gate` | a Settings tab has no row or tools | add the tools and the row in CHAT-REACHABILITY.md (OS Settings pages only; ask) |
| `compat-check.sh` | surface/core API fixture differs | a public item changed: stop and ask (additive changes need a fixture) |
| compile | `error[E...]` | fix the compiler error first; `docs/sdk/reference/*.md` has the real signatures |
