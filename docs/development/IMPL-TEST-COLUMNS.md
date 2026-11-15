# Impl/Test and Manual columns in FEATURES.md

FEATURES.md shows implementation and proof separately: `Impl/Test` (`100/60`) and `Manual`. The full definition is in the "How to use it" section of FEATURES.md; the checks are in `crates/iyakki-core/tests/feature_matrix.rs`.

* Impl: how much of the row is written (0..100). Rule of 2026-11-12 (owner): "fully written" means the code is implemented, "fully proven" means fully tested; so Impl 100 is about code only and unverified work belongs to Test and Manual (docs/development/IMPL-RULE-2026-11-12.md). Test: 0 nothing, 30 fakes or parts, 50 end to end on fakes, 70 integration/QEMU/harness/script, 100 also seen on the real hardware or screen (or a manual test passed).
* Manual: `none`, `needed`, `passed YYYY-MM-DD <where>`, `failed KI-nnn`.
* `done` needs 100/100 and Manual none or passed; everything else is `partial`. The Totals line splits partial into impl open and unproven.
* Gate: the count of `partial` rows may not exceed `docs/development/partial-ceiling.txt`; lower the file when rows get done, never raise it.
* The public progress page (`site/progress.html`, built by `dist/site/build-site.py`, checked by `dist/site/check-site.py`) shows overall implementation and tested bars and a per-area table from these columns.
* When you prove a row on hardware: set Manual `passed <date> <where>`, raise Test, and lower the ceiling file if the row becomes `done`.
* `dist/features/migrate-impl-test.py` made the first migration (idempotent). It used only each row's own Status, Progress, Tests and Notes and `docs/development/partials-triage.json`; a pass was written only where a Notes sentence reported a dated real run. Done rows whose notes said something was not tried became `partial` (Impl 100, unproven).

## Second pass: Impl for class B rows

Triage class B rows (docs/development/partials-triage.json: built and unit-tested, never seen on hardware) had Impl = the old Progress, which already counted the unverified part. The second pass of `dist/features/migrate-impl-test.py` (runs on every call, idempotent) sets Impl to 100 for a class B `partial` row only when its own Notes name no missing implementation and the triage row has no `also_code_gap` flag. Test and Manual stay as assigned (the verification gap shows in Test and Manual, so these rows count as unproven, not impl open). Rows that name a gap, and classes A, C, D and E, keep their Impl.

Gap words (case-insensitive) that keep Impl below 100: not built / implemented / written / wired / added / supported / done / connected / finished (with optional "yet"), isn't built / wired / written, missing, no code, later, planned, TODO, stub, only the first slice, unfinished, to be built / written / added, will be added / built / written, next slice, follow-up, cannot yet, does not (yet) exist / work / handle / support, nothing (is) stopped / enforced, left to do, remaining, not (yet) available, no way to.

The word list is a rough reading of sentences, not understanding: raising Impl is a claim a reviewer should sample. Lower an Impl by hand if a row turns out to have a gap.
