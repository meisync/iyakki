# Benchmark: three app tasks, graded automatically

Purpose: measure how well a model (or a new team member) builds an IYAKKI app with the SDK guide and the skill `iyakki-app-development`. Run the same three tasks with several models and compare the results.

| Task | What is built | What it exercises |
|---|---|---|
| T1 [tasks/T1.md](tasks/T1.md) | counter + notes pane with persistence + a switch with its chat tool | storage port, form, tabs, ChatView tools, approval (ask), switch and tool sharing one path |
| T2 [tasks/T2.md](tasks/T2.md) | FilesView + ChatView with an approval card | two embedded views, separate action ranges, context for the assistant, read tool and asking tool |
| T3 [tasks/T3.md](tasks/T3.md) | themed dashboard, list and detail | tokens only, size classes, accessible layout at 3 sizes and 3x text, status in words, skins |

## How to run a task (for the person who launches a model)

1. Make a **scratch checkout** of the repository, and delete `docs/sdk/benchmark/hidden`, `docs/sdk/benchmark/reference` and `docs/sdk/dry-run.md` and `docs/sdk/recipes/notes_app.rs` from it (the model must not see the hidden tests or a reference solution; `acceptance/` and `tasks/` stay). The model works in that checkout with the skills of `.claude/skills`.
2. Give the model the text of one task file (`tasks/T1.md`...) as its prompt, in a fresh session, with the folder where it may make its app. Note the start time.
3. When it says it is done (or after a time limit you choose, for example 45 minutes), note the end time and count the attempts (how many times it ran `app-check` or the tests: the session log shows it).
4. Grade, in the full checkout (where `hidden/` exists):
   `python3 docs/sdk/benchmark/grade.py T1 <app folder> --attempts N --seconds S --out result-T1-<model>.json`
   (add `--cargo "<wrapper> <app folder>"` if cargo runs through a wrapper).

## What the grade says

`grade.py` prints and writes JSON: `pass` (true only when `app-check` reports no problem, every visible acceptance test passes and every hidden test passes), `app_check_problems` and `rules` (how many problems per rule the finished app still has: styling-number, colour-literal, machine-access, unnamed-control, manifest-dependency, tool-should-ask, starter-test, bounds, target-size, ...), the visible and hidden test counts, and the `attempts` and `seconds` you measured. Score a model on: pass (1 or 0) per task; lint failures hit during the run (count the distinct rules in its `app-check` outputs from the session log; the final JSON shows only what remained); attempts; wall time.

## Validity of the tasks

`reference/T1.lib.rs`, `T2.lib.rs`, `T3.lib.rs` are solutions of the tasks. They were graded with `grade.py` to prove the tests can be passed (see the results at the end of this page). A solution is made by scaffolding the app with `iyakki new-app`, replacing `src/lib.rs` with the reference file and deleting `tests/starter.rs`.

## Results of the reference solutions

Graded 2026-11-16 on a Linux build host through the project's remote cargo wrapper, each reference in a fresh scaffold of `iyakki new-app`:

| Task | pass | visible tests | hidden tests | app-check problems |
|---|---|---|---|---|
| T1 | yes | 4 of 4 | 5 of 5 | 0 |
| T2 | yes | 2 of 2 | 4 of 4 | 0 |
| T3 | yes | 3 of 3 | 4 of 4 | 0 |

A copy of the T3 reference with one literal `14.0` added in drawing code was graded too: `pass` false, `rules` {"styling-number": 2}. The grade takes about 13 seconds per task on that host (app-check and both test binaries; the app is already built).

## Limits of this benchmark

Three tasks are a small sample; the hidden tests check behaviour the task text states, not taste; `attempts` and `seconds` come from the person running the model; lint failures hit during a run (not only at the end) must be counted from the session log. The hidden tests use the SDK's scripted assistant and in-memory ports, so a model that passes still has not run on a device.

## Results of the model runs (2026-11-16)

Same prompt for every run: the task file, the skill and the guide, in a scratch checkout of release/alpha.5 with `hidden/`, `reference/`, `dry-run.md` and `recipes/notes_app.rs` removed. Cargo ran through a remote build wrapper, so wall times include build-host load and are only roughly comparable (Haiku, Sonnet and Opus ran at the same time). Graded with `grade.py` (JSON in `results/`). Seconds are the agent's wall time from the task notification.

| Task | Model | Pass | app-check problems at the end | visible / hidden tests | Attempts | Seconds |
|---|---|---|---|---|---|---|
| T1 | Haiku | yes | 0 | 4 of 4 / 5 of 5 | 6 | 245 |
| T1 | Sonnet | yes | 0 | 4 of 4 / 5 of 5 | 5 | 175 |
| T1 | Opus | yes | 0 | 4 of 4 / 5 of 5 | 5 | 247 |
| T2 | Haiku | yes | 0 | 2 of 2 / 4 of 4 | 9 | 202 |
| T2 | Sonnet | yes | 0 | 2 of 2 / 4 of 4 | 6 | 120 |
| T2 | Opus | yes | 0 | 2 of 2 / 4 of 4 | 3 | 216 |
| T3 | Haiku | yes (agent reported "gave up": see below) | 0 | 3 of 3 / 4 of 4 | 9 | 246 |
| T3 | Sonnet | yes | 0 | 3 of 3 / 4 of 4 | 3 | 117 |
| T3 | Opus | yes | 0 | 3 of 3 / 4 of 4 | 2 | 215 |

All nine runs pass. The only static lint hit in the nine runs was `styling-number` (Haiku, T3: a literal `3.0` for gutters, fixed with a named constant). Every `compile` hit came from the setup, not from an app: app-check ran `cargo test` at the repository root and the SDK's own `docs_recipes` test could not find the deleted `notes_app.rs`. Haiku's T3 agent stopped there and reported "gave up" although its app was complete; the grade (run with the app's manifest) passes.

What the runs showed (friction, not model weakness; all three models hit the same things):
1. `iyakki new-app --sdk PATH` writes an absolute SDK path into `Cargo.toml`; it does not exist on a remote build host. Every run changed it to a relative path.
2. The scaffold's golden-file test needs `IYAKKI_GOLDEN_WRITE`, which a remote cargo wrapper does not forward, and written files do not come back. Runs replaced the test or transcribed the frames from the failure output.
3. `app-check --cargo CMD` runs `CMD test --quiet` in the app folder; with a wrapper that copies a whole checkout the app's manifest path must be passed. Every run wrote a small shim.
4. The approval card's Allow and Deny buttons appear only after the chat input is tapped (T2); the recipe should say so. `FilesView` has no close method (Esc closes the preview).

Limits of these results: one run per cell, three tasks, the same SDK state for all models; the hidden tests check stated behaviour, not taste; nothing was run on a device. A harder test of a smaller model would be a task that needs a new facet or an unfamiliar contract test.

## Friction from the runs: what was fixed (2026-11-16)

1. `iyakki new-app --sdk`: relative path inside or beside the SDK checkout, absolute otherwise (said in the output and in first-app.md).
2. The golden test: the first `cargo test` writes missing files and passes anywhere; `app-check --update-golden` brings them back from a build host through the test output.
3. `app-check --cargo CMD`: runs from the checkout root with `--manifest-path` of the app (only the app's tests); no shim. Documented in first-app.md and troubleshooting.md.
4. The approval card never needed focus; the field now gives up the focus when a card arrives so `y` and `n` answer at once. `FilesView::close()` added.
5. The `/ 3.0` lint stays as it is (const workaround documented).
