# Waiting that actually waits: the agent loop's pending-work ledger (plan, not built)

Status: plan written 2026-11-07 after the owner saw an assistant say "I'll wait and report back" and then not report. First slice built 2026-11-08 (see "Built so far" at the end); the rest is still plan. Related: [`../JOBS.md`](../JOBS.md), [`../PARALLEL-TASKS.md`](../PARALLEL-TASKS.md), [`FLOW-AUTOPILOT-PLAN.md`](FLOW-AUTOPILOT-PLAN.md), `crates/iyakki-agent/src/{agent,assistrun,autopilot,jobtools,jobs,tasks}.rs`.

## The problem, stated exactly

A language model cannot wait. It produces one answer per turn; after it answers, nothing runs until something sends it another turn. "I'll wait for the build and tell you" is a promise the model has no way to keep unless the program around it keeps it. Today the program keeps it only in one case.

What exists (verified in the code and `docs/JOBS.md`, 2026-11-07):

| Piece | What it does | Where it stops |
|---|---|---|
| `job_start`, `job_wait` (up to 60 s), `job_status`, `job_tail` | A detached job outlives the chat; `job_wait` returns early when it ends | One 60 s wait per call; the model has to call it again and again |
| `Tools::take_notes` | A finished job is added to the conversation as a note at the start of the **next model turn** | There is no next turn unless the person types one |
| `Tools::jobs_running` + `autopilot::wait_for_jobs` | An **Autopilot** step that ends with a job running waits (not stalled, time not charged, six-hour cap) | Only Autopilot, and only for `job_start` jobs |
| `task_spawn` / `task_status` (parallel tasks) | Tasks run in isolated trees | The model must poll `task_status`; Autopilot does not wait for them |
| Installs, model pulls, flows, rollouts, git and browser jobs | Each has its own progress state | Not visible to the loop at all |

So in a plain chat or in the AI IDE's chat pane, a run that starts a job and ends its turn with "I'll report when it finishes" is **stranded**: the job finishes, the note is queued, and nothing wakes the model. The person sees a promise that was never kept. That is the failure to remove.

Two more holes sit behind it:

1. **"Finished" is not "done".** A job exiting 0 does not mean the thing the model was asked to achieve happened (the service answers, the test passes, the file exists). Today the model decides from the last lines of output.
2. **Nothing checks the promise itself.** A final answer that says "I'll wait / I'll let you know / I'll check back" with nothing pending is simply accepted.

## Design: one ledger, one waker, one gate

### 1. The pending-work ledger (`Pending`)

One small shared store per chat or workspace session (persisted under the data volume, `pending.json`, so it survives a restart). Anything asynchronous registers a **ticket**:

```
Ticket { id, source, label, started, deadline, state, check, on_done }
source  = job | task | install | pull | flow | rollout | browser | git | external
state   = running | finished_ok | finished_failed | stopped | lost | timed_out
check   = optional acceptance probe (see 3)
on_done = what the model should do when it lands (one line the model itself wrote)
```

Every existing async producer gets a thin adapter that registers and updates tickets (the producers keep their own state; the ledger only mirrors it): jobs, parallel tasks, Quick install jobs, model pulls, flows, k8s rollouts, git jobs, browser jobs. A new tool, `wait_for`, lets the model register a ticket on something that has no producer of its own (a URL that must answer, a file that must appear, a port that must open) with a deadline: that covers "wait until the server is up" without a shell loop.

`Tools::jobs_running` and `take_notes` become views of the ledger (all sources, not only jobs); the old methods stay as thin wrappers so the wrapper tools keep compiling.

### 2. The waker (who speaks next)

When a ticket leaves `running`, something must start a model turn. The waker is a small thread owned by the chat/IDE session, not by the model:

* **Session open, run idle:** start a **synthetic turn** at once: `[IYAKKI] <label> finished: <state> after <time>. <last output or check result>. You said you would: <on_done>`. The turn is visible in the chat as a step ("woken by: build finished"), uses the normal approvals and budgets, and is rate-limited (at most one wake per ticket, batched when several land together).
* **Run in progress:** the note is added at the next step as today (no change).
* **Session closed or restarted:** the ledger is reloaded at start; a ticket that finished meanwhile produces one wake on open with "while you were away"; one that was running when the machine stopped becomes `lost` and says so (never silently dropped).
* **Autopilot:** `wait_for_jobs` becomes `wait_for_pending`: the same rules (not a stall, time not charged, Stop and Pause immediate, six-hour cap) now cover every source.
* **External agents (Claude Code and others through the policy bridge):** they cannot be woken, so the bridge offers a **long-poll** `mcp__iyakki__wait_for` (blocks up to a few minutes, returns the first ticket that lands) and the generated instructions say so: never end a turn with "I'll wait"; call `wait_for` or finish.

### 3. The gate (what the loop will not accept)

At the end of every run, before the final answer is accepted:

* **Pending tickets exist and the answer claims waiting** → the run is not ended. Two legal outcomes: (a) the answer is kept and the session shows **Waiting for: …** (the waker will continue it), or (b) in Autopilot, the run waits as it does today. Either way the person sees an explicit waiting state, never a silent end.
* **The answer promises to wait or report back and nothing is pending** (a small phrase test with a model-free fallback, like the existing "settle the board rows" nudge in `unresolved_work`) → one retry with: "You said you would check back but nothing is pending. Register what you are waiting for with `wait_for`/`job_start`, or finish now and say what you did not verify."
* **A ticket has a `check`** → "done" is the check passing, not the exit code: the ledger runs it (a command in the workspace under the same approval rules, an HTTP probe through the policy gateway, a file test) when the producer finishes, and reports `finished but check failed: <why>` rather than success. No check set → the report says "exited 0, not verified".

### 4. What the person sees

The existing Jobs strip becomes the **Waiting strip**: one row per ticket (spinner, label, source chip, elapsed, deadline, Stop), plus the line "I will continue when this ends" when a wake is armed. A woken turn is labelled. Settings → Assistant: auto-continue on or off (off: the note waits for the person's next message, the old behaviour), maximum automatic wakes per session (default 20), maximum total wait (default 6 h).

## Reachable by chat (owner rule)

`wait_for` (register, Safe), `pending_list` (Safe), `pending_cancel` (asks), `pending_settings` read/set (asks for the change). Approval risks live in `runs.rs`, groups in `toolblock.rs`, audit lines `pending.register`, `pending.wake`, `pending.cancel`, `pending.timeout`.

## Safety limits (a loop that wakes itself must not run away)

* A wake is one turn with the normal step and token budgets; a woken turn that registers new tickets counts against a **chain limit** (default 20 per session per day) and then stops with a plain message to the person.
* Wakes never approve anything: a woken turn that needs an approval waits for the person like any other.
* Tickets expire at their deadline (`timed_out`, reported once). A `check` command is a normal tool call: same policy, same approval, same redaction of its output.
* The ledger stores labels, states and short result lines only, never command output or secrets beyond what the jobs log already keeps (cleaned and redacted).

## Build order

1. `Pending` ledger + persistence + adapters for jobs and tasks; `jobs_running`/`take_notes` read it. Tests with scripted models and fake producers.
2. The waker (synthetic turn on an idle session, batching, chain limit, restart recovery) and the end-of-run gate (both branches and the promise nudge).
3. `wait_for` tool, `check` probes, the Waiting strip, the Settings rows, the chat tools above.
4. Adapters for installs, pulls, flows, rollouts, git and browser jobs; the external-agent long-poll and instructions.
5. A real run on a device: start a build from chat, close the chat, reboot, reopen: one "while you were away" wake.

## Tests that must exist before any row says done

* A scripted model that answers "I'll wait and report" after `job_start`: the run does **not** end silently; the session shows Waiting; when the fake job ends, a synthetic turn runs and the model's follow-up reaches the chat (the exact failure reported).
* The same with the session closed and reopened (ledger reload), and with the machine restarted (`lost` is reported, not dropped).
* A promise with nothing pending triggers exactly one nudge, then ends.
* A job that exits 0 but whose `check` fails is reported as failed-check.
* Chain limit, per-ticket single wake, batching of two tickets landing together, Stop and Pause during a wait, auto-continue off.
* A parallel task and a Quick install job each wake the session (not only `job_start`).
* Nothing a woken turn does bypasses an approval.

## Decisions for the owner

1. **Auto-continue default:** on (recommended: it is the whole point) or ask first the first time a session would wake itself.
2. **Check probes:** command and HTTP only at first (recommended), or also a model-judged check ("does this log say the server is ready").
3. **Window of a wake:** continue in the same chat (recommended) or open a separate "follow-up" thread so a long chat is not interrupted mid-conversation.

## Built so far (first slice, 2026-11-08)

Code: `crates/iyakki-agent/src/pending.rs` (ledger, waker, phrase test), `pendingtools.rs` (the chat tools), the gate in `assistrun.rs`, `Jobs::agent_told` in `jobs.rs`, the chat wiring in `crates/iyakki-shell/src/apps/chat.rs`. Tests: `pending_tests.rs`.

* **Ledger.** `<data>/pending/<person>.json`, written atomically, per person; tickets for jobs and parallel tasks are opened by mirroring (no producer needed), so Quick install and every other `Jobs` user is covered. `Ledger::report` is the entry for producers without a store (install, pull, flow, rollout): the first report opens the ticket. After a restart a running ticket nobody can vouch for becomes `lost` and is told once; jobs (their own store), tasks and probes carry on.
* **Tools.** `wait_for` (a job, a file in the read scope, a port on this machine; Safe; `block_seconds` is the long-poll for agents that cannot be woken), `pending_list` (Safe), `pending_cancel`, `pending_check` (an acceptance command, only reads, runs in the job's folder when the job exits 0), `pending_settings` (all three ask). Through the bridge for external agents; their instructions say never to end a turn with "I'll wait".
* **Waker.** A thread per chat session syncs the ledger every 2 s; when something ended that the assistant was not told about and no run is going, the chat starts a synthetic turn (`[IYAKKI] ...`) with the batched results, once per ticket, within a daily chain limit (default 20), auto-continue switchable. A job the run already reported is not woken for.
* **Gate.** An answer that promises to wait or report back: with something pending it stands and the chat shows "Waiting for: ..." (and the stall nudge does not push the model to act); with nothing pending one nudge asks the model to register or finish; an answer that still promises gets a plain notice appended. `jobs_running` now reads every source, so Autopilot waits for tasks and waits too.
* **Not built yet.** The Waiting strip and Settings rows (the Jobs strip still shows jobs only), HTTP probes and model-judged checks, adapters that call `Ledger::report` for installs outside the job engine, pulls, flows, rollouts, git and browser jobs, the per-session "follow-up thread" option, and the real device run (step 5).
