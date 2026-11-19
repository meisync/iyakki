---
name: iyakki-feature-rows
description: Write and update FEATURES.md rows for IYAKKI work: the nine columns, honest Impl and Test numbers, status words, the partial ceiling and its raise lines, KNOWN-ISSUES entries, and the checks that validate them. Use whenever a change adds, splits, finishes or re-rates a feature row.
---

# Feature-matrix rows

`FEATURES.md` is the one place that says what is built and how well it is proven. A test (`feature_matrix`) validates it.

## The row (exactly nine columns, one line)

`| ID | Feature | Phase | Status | Pri | Impl/Test | Manual | Tests | Notes |`

- **ID** `F-AREA-nn` (area letters, then a number; never reused). For this SDK work the area is `SDK` and the next free number after the highest. Find it: `grep -o '^| F-SDK-[0-9]*' FEATURES.md | sort -t- -k3 -n | tail -1`.
- **Feature**: one sentence of what exists, in plain words.
- **Phase** `P0`..`P7`; **Pri** `P0`..`P3`.
- **Status**: `planned` (0/0), `partial` (anything between), `done` (Impl 100, Test 100, Manual none or passed), `blocked`, `deprecated`.
- **Impl/Test** `a/b`: Impl = how much of the row's behaviour is written (code written). Test = what was RUN: 0 nothing, 30 parts or fakes, 50 the ceiling for fakes-only, 70 against the real thing in part, 100 a real run. Allowed Test values: 0, 30, 50, 70, 100. Never raise Test for something you did not run.
- **Manual**: `none`, `needed`, `passed <date> <machine>` or `failed KI-nnn`.
- **Tests**: backticked test names joined with `<br>`, each of the form `crate::module::test_name` (integration tests: `crate::file::test_name`); each must exist in the source.
- **Notes**: what was run and where, what was not, the chat tool (or why none), what the row split off.

No newlines inside a cell; no `|` inside a cell.

## After every edit

1. `python3 dist/features/migrate-impl-test.py` (rewrites the totals line).
2. `docs/development/partial-ceiling.txt` must equal the real partial count (read it from the totals line). It may fall; it may rise only for new rows, each with a line in `docs/development/partial-ceiling-raises.txt`: `F-SDK-nnn YYYY-MM-DD new partial row: what, Impl a, Test b`.
3. `cargo test -p iyakki-core --test feature_matrix` (through the remote wrapper if that is how this machine builds).
4. A bug found while working gets a `KNOWN-ISSUES.md` entry `KI-nnn`; a new feature gap gets a row, not a note. Never hide unwritten code by relabelling a row; to split a row, the built part stays and the unbuilt part becomes a new planned row.

## Common failures

`expected 9 columns` (a `|` or a line break in a cell), `bad ID`, `duplicate ID`, `Test N must be one of 0, 30, 50, 70, 100`, a Tests entry that names a test that does not exist, status `done` with Impl or Test under 100, ceiling mismatch (run step 1 and 2). In a merge, the totals line conflicts: take either side, run step 1, and set the ceiling to the new count.

## Example (SDK component row)

`| F-SDK-150 | UI SDK widget `CalendarView` over the `CalendarPort`, in-memory port `MemCalendar` | P5 | partial | P2 | 100/50 | none | `iyakki-ui-sdk::calendarview_tests::draws_at_three_sizes` | Ran 2026-11-16: 9 tests on the in-memory port; not run against the core. Chat tool: `calendar_status` (free), `calendar_set` (asks). |`
