# FLOWS autopilot and copilot: a configured agent that watches a flow, fixes it, upgrades it and (if allowed) publishes

Owner request (2026-10-27): a flow can run on autopilot or with a copilot; the configured agent (with its model or models) can fix,
upgrade and publish, and asks the person for permission when it needs it. This plan says what exists, what is missing, the model,
the permissions, the guards, the screens, the open questions and the decisions the owner has to make. Section 12 lists what the
first slice built (and what it did not), section 13 what to do next. Read [`../FLOWS.md`](../FLOWS.md) first (the engine, healing,
versions); this builds on it and changes none of its rules.

## 1. What exists today (read in the code, 2026-10-27)

| Piece | Where | What it does today |
|---|---|---|
| Healing inside a run | `iyakki-flow/src/heal.rs`, `driver.rs` (`begin_heal`, `proposal`), agent `flows/host.rs` (`propose_patch`) | After a node failed for good, the built-in chat model gets a redacted failure context and answers one JSON patch. The harness classifies the patch (`heal::classify`: Simple, NeedsApproval, Refused) from the patch, never from the model's claim. Policy per flow and node: `HealMode` off, suggest, auto_fix_simple, always_ask; per-run, per-day and cost caps; `auto_publish_simple` (default false, stripped on import). A card, an inbox item and a chat message ask the person. The fix goes to a draft; a simple fix is tried with the recorded input. |
| "Ask the agent to fix" | agent `flows/failfix.rs`, shell `flows_fail.rs` | The failure card (redacted run summary, input names only) offers the chat assistant; at most 3 attempts per failing streak; everything the assistant then changes goes through its ordinary Ask tools. |
| Assistant tools | agent `flows/tools.rs` | `flow_create`, `flow_edit` (drafts only), `flow_run`, `flow_publish`, `flow_rollback`, `flow_apply_to_runs`, `flow_delete`, `flow_drop_version` are Ask; `crate::agent::hard_ask` makes publish, rollback, apply-to-runs, delete, drop-version and a widening `flow_set_heal_policy` ask in every mode, including Auto and Autopilot. `apply_defaults` refuses camera `allowed`/`unattended` and browser `trusted`/`profile` from the assistant. |
| Versions | `iyakki-flow/src/store.rs` | Draft plus immutable published versions with hashes, `publish` (refuses an agent's changes without an approver unless `auto_publish_simple` and all changes simple), `rollback` (publishes an older content as a new version), diff, tags, library nodes with versions. A run pins its version; node `track` pinned, latest-published, compatible; `node_updates`/`apply_node_update` hand running and scheduled runs a newer node. |
| Trust flags | `model.rs`, `templates::sanitize_import` | Camera `allowed`/`unattended`, browser `trusted`/`profile`, `production`, heal mode above suggest: all stripped from imported flows and Store templates. |
| General autopilot | agent `autopilot.rs` | The chat's goal-driven loop for the AI IDE (board rows, step caps, token caps, hard-ask still asks). Not related to flows; its harness ideas (caps, pause, resume, never raises its own limits) are the model for this plan. |
| Cost and tokens | `usage.rs`, `agent::cost_cents`, `HealRequest.max_cents`, `docs/development/TOKEN-BUDGET.md` | Usage ledger per user, provider, model; a price per million tokens in settings; the heal loop has a per-run cents cap. No per-day cents cap per flow, no per-model choice for flows. |
| Inbox and bell | agent `inbox.rs`, `Host::notify`, `Host::notify_channels` | Notices with a link; Matrix channel through `notify` (`flows/notify_matrix.rs`). |
| Audit | `Host::audit`, `flow.heal.*`, `flow.publish` | Names and places, never values. |
| Tainted runs | `docs/development/MESSAGING-CONNECTORS-PLAN.md` section 5.2 | A designed hard rule (a run that read an external message gets a reduced tool set); the code is not built. |

Missing, verified by reading: a per-flow setting for how far an agent may go; any way to publish a fix by itself other than the
narrow `auto_publish_simple` (which applies only to heal fixes made inside a run, has no tests of the version after, no rollback, no cap and
no notice); a stored, reviewable suggestion that outlives the run; a named agent profile with model choice and fallback for flows
(`HealRequest.agent` exists and is always empty; the model is `settings.chat`); a per-day budget per flow; a watcher of the first runs
of a published fix; a cool-down; a dry-run of a whole draft (the engine has `test_node` and the heal loop's try with recorded input, not
a no-side-effects run of a flow); upgrade detection; the screens for all of it.

## 2. The model

Per flow, in the flow's own settings (`Flow.defaults.assist`, written only when it differs from the default, so a flow written before
this existed loads as **off** and every old content hash still verifies):

| Field | Default | Meaning |
|---|---|---|
| `mode` | `off` | `off`, `copilot`, `autopilot` |
| `auto_publish` | false | autopilot may publish a tested FIX by itself |
| `allow_fix`, `allow_upgrade` | true, false | classes of work the agent may draft for this flow |
| `max_publish_per_day`, `max_drafts_per_day` | 1, 4 | per flow per 24 hours |
| `max_cents_per_day`, `max_cents_per_fix` | 50, 10 | spend on the model for this flow |
| `max_diff_nodes`, `max_diff_fields` | 2, 10 | the biggest change that may be published without asking |
| `watch_runs` | 3 | a published fix is rolled back if one of its first N runs fails |
| `cooldown_hours` | 24 | after a rollback autopilot behaves as copilot for this long |

The setting lives in the **published** version (the one that runs), so switching Assist on is itself a published change with a diff and
the person's confirmation. Runtime state is not part of the flow: the account's kill switch and caps (`_assist.json`), and per flow the pause,
the cool-down, the watch, the counters and the suggestions (`assist.json`, `suggestions/<id>.json`, all with `iyakki-schema` versions).

Account-wide (later in Settings, Flows; today in the file and the API): `enabled` (the kill switch, default on, every flow is still off),
`max_publish_per_day` across flows (3).

**Which agent and models.** Today the work is done by the built-in chat model (`Host::propose_patch`, which records usage). The target
is a named *agent profile* for flows: a list of models in preference order with a fallback, a local-model preference (so no data leaves
the machine unless the person chose a cloud model), a timeout per request, the per-day and per-fix cents above, and optionally an
external agent through the executor seam. The setting would hold the profile name (`assist.agent`, empty = default); the profile is
edited once in Settings. The fields `agent` and `models` are NOT in the first slice because nothing would honour them yet (decision D3).

## 3. Action classes and the permission matrix

| Class | What | Off | Copilot | Autopilot |
|---|---|---|---|---|
| FIX | Patch the failing step's settings into a proposed version, run QA and a trial | deny | allow (a suggestion is the ask); class switched off: ask | allow; class switched off: ask |
| UPGRADE | Newer compatible library step, changed names, dependency bumps, switch track | deny | allow if switched on, else ask | allow if switched on, else ask |
| PUBLISH | Promote a tested draft to the published version | deny | **ask** | allow only for a FIX, with `auto_publish` on and every guard passing; otherwise ask with the reasons |
| ROLLBACK | Publish an older version again | deny | ask | allow only for a version the agent published whose first runs failed; otherwise ask |
| APPLY-TO-RUNS | Hand running and scheduled tasks a newer step | deny | ask | ask (never automatic) |
| DELETE | Delete a flow or a version | deny | deny | deny (never) |

The effective mode is `off` when the account's kill switch is off; a pause or a cool-down turns autopilot into copilot. The matrix is
pure code (`iyakki_flow::assist::decide`) and is tested for every mode, class, origin, setting and guard (a table of about 12,000
combinations plus the invariants stated separately: nothing is allowed in `off`; delete and apply-to-runs are never allowed; a publish is
allowed only for autopilot, a fix, `auto_publish` and all eleven guards).

## 4. Guards for an automatic publish (all required, all in code, all tested)

A guard that was not evaluated counts as failed. Failure turns the publish into an approval request that carries the reasons.

1. **QA**: the proposed flow validates (as the engine sees it, library steps expanded) and the patch is a *simple* fix by `heal::classify`;
   a production flow never qualifies.
2. **Trial without side effects**: every changed step is run once with the failed run's recorded input, only if it is read-only
   (`NodeKind::idempotent`) and not a browser, camera, message, model or remote step (`assist::never_in_trial`, whatever the kind says).
   Anything else is "not run", which fails the guard.
3. **Diff stored**: the diff text, reason, QA and trial result and every guard line are stored with the suggestion.
4. **Size**: at most `max_diff_nodes` steps and `max_diff_fields` settings.
5. **No new external effect**: no added step that reaches outside, no kind change, no HTTP step going from reading to writing.
6. **Nothing protected touched**: no change to credentials (`{{secret`, `vault:`, credential-named headers), trust flags, camera or channel
   permission, schedule or trigger, runner or sandbox, allow-lists, the first word of a command, the heal policy, the `production` flag, or the
   `assist` setting itself; no removed step, no changed connection. The person's own settings (assist, camera, browser trust, heal policy,
   production) are refused outright when a suggestion is made, in every mode.
7. **No new host**: every address in the changed flow's steps and variables was already in the published version.
8. **Based on the published version** and the person's draft holds nothing else (their work is never overwritten).
9. **Caps**: publishes per flow per day, per account per day; drafts per flow per day; spend per day and per fix.
10. **Cool-down and pause**: after a rollback, or when the person pauses, autopilot is copilot.
11. **Everything audited** (`flow.assist.suggested`, `.loaded`, `.approved`, `.rejected`, `.published`, `.rollback`, `.watch.ok`, `.refused`, `.failed`, `.nofix`,
    `.pause`, `.resume`), names and places only; an input value is removed before the model sees it and before anything is stored.
12. **Notification after every automatic publish** (the bell with a link; the Matrix channel through the notify design, later) and a notice that needs the person
    after every rollback and every question.
13. **One-click rollback and Pause autopilot** on the flow's strip.

The version history records `author: agent:assist` and `approved_by: policy:autopilot` for a version nobody approved by hand: the
person's own setting allowed it, and that is visible.

**Rollback watcher.** After an automatic publish the version is watched: if any of its first `watch_runs` top-level runs fails, the
previous content is published again (as a new version), the cool-down starts, the suggestion is marked rolled back, and a warning that needs
the person is sent. `watch_runs` good runs end the watch. A rolled-back change is not tried again by itself while cooling.

**Loop prevention.** At most one open suggestion per flow; each failed run is answered once; the model is asked at most `max_drafts_per_day` times;
the agent that published `MAX_SELF_FIX` (2) versions in a row stops and waits for the person (a person's publish ends the chain); the fixer
never edits its own settings (the `assist` path is a hard-forbidden change) and never fixes a production flow.

## 5. Permission asks

When an action is outside the allowed classes or fails a guard, the agent does not act: it stores the suggestion with the reason (`asks`)
and raises a notice that needs the person. The person's answer is the **existing path**: Approve puts the change in the draft and opens the
ordinary Publish dialog (diff, changelog, blockers); the store still refuses to publish an agent's change without an approver. Reject restores
the draft. The assistant API (the chat's tools) keeps `flow_publish`, `flow_rollback`, `flow_apply_to_runs`, `flow_delete` as hard-ask.

* **Imported flows, Store templates, samples and assistant-made flows start at `assist: off`.** `sanitize_import` resets it; the assistant's `flow_create`
  and `flow_edit set_default` refuse any `assist` key ("only the person can switch Assist on", in the Flows settings). The person can set autopilot
  on any flow in the UI, including an imported one, after reading it (decision D6 asks whether an imported flow should first need a successful
  manual run).
* **Tainted input.** A run whose evidence came from external data (messages, web pages) is a prompt-injection risk. The fixer today gets only
  the redacted failure context, no tools, and answers one JSON patch that the harness classifies; it cannot call anything. The MESSAGING plan's
  reduced tool set (section 5.2) applies when the fixer later becomes a tool-using agent; then an autopilot publish is refused when the evidence
  was tainted (decision D9).
* **Kill switch.** `AssistStore::set_enabled(false)` turns every flow's effective mode to off at once (no UI yet).

## 6. Screens (Flows tile)

Built in the first slice (shell `apps/flows_assist.rs`, `flows_props.rs`):

* **Settings, Assist** (the flow's settings form, shown when no step is selected): mode, autopilot may publish, fixes, upgrades, publishes per day,
  spend per day, most steps per automatic publish. Each field says what it does; "takes effect when you publish".
* **Strip on the open flow**: "The agent suggests: ... (and N more)" with **Review (S)**; while a watch runs, "Autopilot published version N ..." with
  **Roll back to vN (B)** and **Pause autopilot (U)**; paused or cooling: a line with Resume.
* **Suggestions list** (a dialog): rows (id, status, summary), the detail (why, the stored diff, QA, trial, failed guards, what it waits for), **Approve and publish (a)**
  (goes to the Publish dialog), **Reject (x)**, Close. The buttons wrap and stay inside the panel at 480 x 600, 800 x 480, 1024 x 600, 1280 x 720, 1920 x 1080.
* **Failure card**: unchanged ("Ask the agent to fix"); with copilot or autopilot on, the suggestion strip appears under it when the agent has answered.

Not built yet: global defaults page (Settings, Flows), agent and model pickers, allowed-class checkboxes beyond fix and upgrade, the **Activity log** (the
book already keeps the events; the audit has the same lines), the Flows list marker ("autopilot", "3 suggestions"), and the CLI / developer-mode readout
(`iyakki dev flows assist <flow>`: mode, waiting suggestions, watch, counters, last decisions, as JSON for scripts; read-only, administrator scope).

## 7. Open questions

1. Should the per-flow setting live in the flow (versioned, diffed, exported) or beside it? The owner asked for the flow model; the cost is that a rollback restores the older
   setting. Mitigation chosen: policy is read from the published version and a rollback is the agent's last act before it pauses.
2. Whole-flow QA: the first slice's QA is validation plus the classifier plus a read-only trial of the changed steps. A real "flow's own QA checks" run
   (br_assert and friends) needs a no-side-effect mode in the engine that does not exist.
3. Dry-run: nodes with external effects cannot be tried at all today; a `dry_run` flag in each effectful kind (the notify kind has one) would let more fixes qualify.
4. Cost of a model call is estimated (`chars / 4`); a local model costs nothing, so the cents caps only matter for cloud models.
5. Does an unattended pass need the model loaded at night on a small machine (Mac mini 16 GB)? The pass asks only after a failure and at most `max_drafts_per_day` times.
6. What counts as one failure when a flow fails every minute: today the newest failed run, once, and none while a suggestion waits.

## 8. Owner decisions (numbered, with a recommendation)

| # | Decision | Recommendation |
|---|---|---|
| D1 | Is `mode` per flow in the flow model, as built? | Yes. Switching it on is a published, diffed change. |
| D2 | Autopilot publishes only a FIX in this release; UPGRADE always asks. | Yes; revisit UPGRADE (track-compatible library bumps) after a month of fixes. |
| D3 | Named agent profile (models, fallback, local first, timeout, budget) before beta? | Yes, as the next slice; until then the chat model is used and the setting says so. |
| D4 | Defaults: 1 automatic publish per flow per day, 3 per account, watch 3 runs, cool-down 24 h, 50 cents a day. | Yes. |
| D5 | Notify after every automatic publish through the bell always, and Matrix when the person set a channel. | Yes; no extra switch to turn the bell off. |
| D6 | Imported flows: can the person switch autopilot on at once? | Recommended: allow copilot at once, and autopilot only after one successful run by the person. |
| D7 | Kill switch place: Settings, Flows, one button plus `iyakki dev`. | Yes. |
| D8 | An automatic rollback also pauses autopilot until the person resumes (not only for 24 h)? | Recommended: yes for the first release (24 h then asks, resume is one click). |
| D9 | Tainted-evidence rule when the fixer gets tools. | Yes, as a hard rule: no automatic publish after tainted evidence. |
| D10 | Should the agent edit flows it did not make or the person did not open? | Yes in copilot (it only suggests); autopilot only where the person switched it on. |

## 9. Sizes (small, medium, large; one person-day is the unit)

| Work | Size |
|---|---|
| Setting, matrix, guards, store, controller, tests (built) | 4 d |
| Strip, list, settings fields, five-size tests (built) | 2 d |
| Named agent profile, model fallback, timeouts, local preference | 3 d |
| Activity log screen, flows list marker, global defaults page, kill switch UI | 2 d |
| CLI / developer-mode readout | 1 d |
| Matrix channel notice for publishes and rollbacks | 1 d |
| Upgrade detection beyond library nodes (names, deprecated settings, CLI minimum), auto-publish of upgrades | 4 d |
| A no-side-effect whole-flow trial and the flow's own QA checks | 5 d |
| Tainted-evidence rule when the fixer is a tool-using agent | 2 d |
| Real model and real run on the Mac mini, five scenarios | 1 d |

## 10. Files

`crates/iyakki-flow/src/assist.rs` (matrix, guards, diff reading, watch verdict), `assist_store.rs` (suggestions, book, account switches),
`assist_run.rs` (the work: suggest, publish by the agent, adopt, reject, watch, tick, upgrades, the trial), `model.rs` (`AssistMode`, `AssistPolicy`),
`templates.rs` (import), agent `flows/assist.rs` (the account's store, the unattended pass every 60 s), `flows/mod.rs` (the assistant cannot set `assist`),
`flows/tools.rs` (after-publish hook), shell `apps/flows_assist.rs`, `flows_props.rs`, `flows_view.rs`, `flows_keys.rs`, `flows.rs`.
Tests: `crates/iyakki-flow/tests/assist_tests.rs`, `crates/iyakki-agent/src/flows/assist_tests.rs`, `crates/iyakki-shell/src/apps/flows_tests.rs`.

## 11. What was checked and what was not

Checked with fakes: the whole path with the fake host, a scripted model, the real engine and the real flow store (a failed run, a suggestion, approve, reject,
autopilot publish, guard failures, rollback, cool-down, caps, loop bound, upgrades); the same through the agent's real host with a scripted model and a dead
network; the settings form and the strips at the usual five sizes. Not checked: a real model's answers (how often a real patch is a simple fix), a real unattended
night, real cost numbers, the screens on a real display.

## 12. What the first slice built

The `assist` setting (old flows load as off), the matrix and guards as pure code, copilot suggestions on a failed run, autopilot for FIX with `auto_publish`,
the rollback watcher, pause, cool-down, caps, the loop bound, upgrade detection and suggestion (never published by itself), the unattended pass, the settings fields,
the strip and the list, the audit lines, the notices, the docs and the matrix rows.

## 13. Recommended next

1. Run the five scenarios on the Mac mini with a real local model (a moved path, a timeout, a renamed field, a refused change, a rollback).
2. The named agent profile (D3), then the Activity log and the flows-list marker.
3. The no-side-effect trial of a whole flow (so more fixes qualify and the flow's own QA checks count).
