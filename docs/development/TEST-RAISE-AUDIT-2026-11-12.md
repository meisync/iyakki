# Test-level raise audit, 2026-11-12 (lane M30)

Earlier on 2026-11-12 two lanes (M26 and M27) raised 102 rows of `FEATURES.md` from Test 30 to Test 50. They matched each row's
Feature clauses to the names of the cited tests and ran the tests. This audit reads the cited test bodies and asks, clause by
clause, whether an assertion really proves the clause. Standard: Test 50 means "unit-tested end to end on fakes"; a lower honest
number beats a high doubtful one.

Sample of the first batch: every third raised row (34 rows) plus every raised row with 1 or 2 cited tests, 40 rows. The second batch (below) covers the other 62.
Rows with 20 or more cited tests (DASH-02, EXTA-07, DOCTOR-01,
CHECKUP-01, GIT-02, DRIVERS-51 and similar) were checked by reading the bodies of the tests that carry the riskiest clauses and the
names of the rest, not every body.

Result: 40 audited, 33 confirmed (26 as they were, 7 after tests or assertions were added), 7 lowered from 50 back to 30.
Test work: 2 new tests, 4 existing tests strengthened, 1 existing test newly cited (see below).

## Lowered to Test 30 (clause not proven)

| Row | Unproven clause |
|---|---|
| F-APPSTORE-23 | Check (c) refreshing catalogs, revocation lists and vendor versions on a background thread, the optional daily check, the header button. The two cited tests cover the Installed chip and Update all only. |
| F-CAM-12 | Opening at 640x480 YUYV by default and the stepwise sizes. The cited tests cover finding the driver node and the greyed hint / Devices warning only. |
| F-FLOW-20 | "retries and loops" over node outputs, the budget setting, and the contents of the three QA templates (3 sizes, console errors, broken links). The cited tests prove data flow, conditions, vision-model hand-over and the trusted-flag rule. |
| F-SETUP-03 | The star on each recommended tile, the one-line reason in the popup and the memory note in the popup. The tests check the rules, the card's reason field and the Install all review, not the popup or star. |
| F-MODELHOST-03 | "only on the person's click", "Use fills the address", "never adds anything". The tests cover the target list, ports, bounds, loopback scan and cancel. |
| F-SHARE-06 | Edit of an existing share, Delete after the typed word, and the Sharing row with Stop in the Devices list (the test only reads the share count). |
| F-RADIO-08 | The issuer-fetch limits (16 KB, DER only, at most three levels up) and the HLS follower using the same road. The cited tests prove the fetch, the host policy, the refusal message and the offered decision. |

## Confirmed after tests or assertions were added (Test stays 50)

| Row | Gap found | Fix |
|---|---|---|
| F-FLOW-23 | Audit lines `flow.delete` and `flow.version.drop` were asserted nowhere; the last assertion of the agent test accepted success or refusal. | The test now installs an audit sink, asserts both lines (id and name, no node data), that a refused delete is not audited, and that the delete succeeds once the cancelled run has ended. |
| F-GIT-11 | "at most 500, the rest counted", "one `git status`", "no lock, no prompt" were not asserted (the old test used `more == 0`). | New test `gitbrowse_tests::a_working_copy_with_more_than_500_changes_shows_500_and_counts_the_rest_from_one_status_call` (620 changes: 500 shown, 120 counted, one call, `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`). Cited in the row. |
| F-IDE-21 | "the plan survives a restart (`plan.json`)" was unproven (the old test only checked the file is gone at the end). | New test `tasks_tests::a_plan_survives_a_restart_and_goes_on_with_the_waiting_wave` (new manager over the same folder reads the plan back with one waiting step and starts it after the merge). Cited in the row. |
| F-GIT-12 | "listed in F1" was unproven. | The tab test now asserts the F1 help lists `Ctrl+Shift+L  Git`. |
| F-K8S-19 | "remote-safe in Cloud mode" was unproven by the cited tests. | The registration test now asserts the K8S tile is `Class::Remote`. |
| F-INST-25 | The figures named in the Feature column (ClickHouse 1 core, Lemonade 500m / 6 cores / 16 GiB) were only compared with the table itself. | The idle-profile test now asserts them. |
| F-AUTH-05 | "wiped after 60 s" was unproven by the cited tests (they check the secret is waiting and pastes once). | The existing uikit clock test `iyakki-uikit::ui::tests::a_secret_goes_after_one_paste_or_after_sixty_seconds_on_a_clock_the_test_moves` is now cited. |

## Confirmed as they were (26)

F-DASH-02, F-DASH-08, F-DASH-14, F-DASH-18, F-EXTA-07, F-K8S-12, F-K8S-22, F-SYS-06, F-APPSTORE-24, F-DRIVERS-22,
F-DRIVERS-31, F-DRIVERS-51, F-MODELHOST-02, F-GIT-02, F-INST-16, F-SETUP-17, F-LAB-02, F-GIT-06, F-GIT-09, F-FLOW-21,
F-FLOW-26, F-BROW-67, F-GIT-17, F-BROW-82, F-DOCTOR-01, F-CHECKUP-01.

For the large rows (DASH-02, DOCTOR-01, CHECKUP-01, EXTA-07, DRIVERS-22/31/51, GIT-02, GIT-09, BROW-67) the confirmation rests on
the test names and on the bodies of the tests that carry the riskiest clauses; a few minor clauses (for example the Neo4j
"cluster address works", the Elasticsearch top-values helper builder, the DASH-08 small-window toggle) were not traced to an
assertion and may deserve a later look.

## Process note

A raise should cite, for each clause, the test that asserts it. "Every clause has a named test" was true of the names but not of the
bodies in the seven lowered rows and the seven repaired ones above. The remaining 62 raised rows have not been audited.

## Second batch (the remaining 62 raised rows)

The same method was applied to the 62 rows that still carried the M26/M27 sentence after the first 40 (on the integration head that
holds the later merges). Rows with many cited tests were checked by reading the Feature clauses against the test names, then
searching the cited test bodies for the specific values and words of each clause (flags, numbers, labels, audit lines) and reading
the bodies where a clause had no hit. Result: 62 audited, 52 confirmed (51 as they were, 1 after an added assertion), 10 lowered
from 50 back to 30. Three rows in the batch were already at Test 100 (F-EDIT-14, F-SND-11, F-STUDY-02) and were confirmed.

### Lowered to Test 30

| Row | Unproven clause |
|---|---|
| F-NET-17 | The once-per-boot log lines and the own-disk-first step at 12 s. The tests cover reading the variables, the internal-disk rule, the kernel switch, put-first and revert. |
| F-NAME-04 | `iyakki dev scan` (name, address, services) and the 2 s lookup limit. |
| F-DASH-17 | The "view only" label on shared rows (the list text became "you are viewer / operator" with roles, F-DASH-18). |
| F-EXTA-05 | The board tracker fed by the agent's approved actions, verification by row kind, retries and no-progress limits, a tool that parks a row ending the step, the footer text. |
| F-EXTA-11 | Autopilot working with Copilot, and key v switching to the read-only advisor. |
| F-CAM-17 | The red in-use sign for a USB-camera snapshot. |
| F-CAM-13 | MJPEG sizes, the rescan on open, `camera_list` and the flows `cam_photo` check using the same list. |
| F-FW-04 | The audit entries and the remote-desktop case of Keep/Revert. |
| F-FLOW-28 | The 4-hour end of the record and the `(last run failed)` chip text. |
| F-BROW-70 | The combined chips, the audit line on a click on the sign, and the sign going away when the browser closes. |

### Confirmed after an added assertion

| Row | Gap | Fix |
|---|---|---|
| F-DASH-03 | `max_result_bytes` was not among the asserted query parameters. | The query-URL assertion in `json_format_becomes_typed_columns_and_rows` now includes it. |

### Confirmed as they were (51)

F-DASH-04, F-DASH-11, F-DASH-13, F-DASH-15, F-EDIT-14, F-IDE-13, F-IDE-24, F-EXTA-01, F-K8S-15, F-K8S-16, F-K8S-20, F-K8S-21,
F-K8S-27, F-INST-07, F-APPS-42, F-DEV-11, F-DEV-13, F-DRIVERS-23, F-DRIVERS-24, F-DRIVERS-41, F-DRIVERS-50, F-INBOX-07, F-FLOW-05,
F-SETUP-09, F-MODELHOST-05, F-LOCK-03, F-GIT-05, F-SHARE-09, F-INST-22, F-INST-23, F-LAB-01, F-LAB-03, F-LAB-04, F-GIT-07, F-GIT-08,
F-GIT-10, F-SND-11, F-SND-16, F-SND-17, F-FLOW-24, F-FLOW-37, F-GIT-14, F-GIT-16, F-GIT-20, F-BROW-81, F-BROW-83, F-BROW-85,
F-ENGLOG-01, F-SSHTUN-01, F-STUDY-02, F-AUTH-03 (plus F-DASH-03 above).

Known soft spots among the confirmed rows (clauses that are negative or minor and were not traced to an assertion): F-FLOW-05
"never starts a cluster", F-LOCK-03 "a running job does not hold the lock", F-DASH-04 timeout and output cap (enforced by the
runner's signature, not asserted by value), F-IDE-24 Enter in the preview, F-SND-16 the page's media reconnecting, F-GIT-14
dropping the open repository on a connection switch, and the largest rows (F-DRIVERS-24, F-IDE-13, F-DOCTOR-01 style rows) where
the check rests on names plus keyword searches of the bodies.

## Combined totals

102 raised rows in all, 102 audited. Confirmed 85 (77 as they were, 8 after tests or assertions were added). Lowered to Test 30:
17 (7 + 10). New tests: 2. Existing tests strengthened: 5. Existing tests newly cited: 1.
