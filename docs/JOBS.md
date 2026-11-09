# Jobs: long commands that outlive the chat

Some work takes minutes: an Android SDK and emulator install, a big build, a download, a long test run. A command run in a
foreground shell dies when the chat or the session ends, and the person sees nothing but text. A **job** is a command that runs
on its own, detached from the chat that started it, with its output in a log and its state on screen.

Code: `crates/iyakki-agent/src/jobs.rs` (the engine; no model, no screen), `jobtools.rs` (the assistant's six tools),
`crates/iyakki-shell/src/apps/jobs_ui.rs` (the strip, the log view and the list). Tests: `jobs.rs`, `jobtools_tests.rs`,
`jobs_ui.rs`, `apps/jobs_screens_tests.rs`, `shell_jobs_tests.rs`.

## What a job is

| Part | Meaning |
|---|---|
| id | `j1`, `j2`, … counting up per person; never reused while the job's folder exists |
| label | the name shown in the strip (the assistant gives one; otherwise the first 80 characters of the command) |
| command | a shell line (`sh -c`, so pipes and `&&` work) or a program with separate arguments (no shell) |
| folder | where it runs: inside the person's write scope and, in the AI IDE, inside the workspace |
| workspace | the IDE workspace it belongs to (empty outside the IDE); the strip of a workspace shows only its own jobs |
| state | `running`, `succeeded`, `failed (exit N)`, `stopped`, `lost` |
| progress | a percentage when the output has one (see below), otherwise unknown |

Everything about a job is on disk, in `<data>/jobs/<user>/<id>/`: `meta.json` (pid, start time, label, command, folder, owner,
workspace, what was announced), `log.txt` and, once the command ended, `exit` (the exit code, written by a small wrapper, so it
is known even when the shell was not there to see it end). A new shell finds running and finished jobs by listing the folder.

### How it is started, and what survives

The command starts in its **own session and process group** (`setsid`), with standard input closed and output and errors going to
the log. It does not belong to the chat, the run or the shell: ending the chat, signing out or restarting the shell does not end
it. A restart finds it again: the state is worked out from the files and from `/proc` (the process is alive, matched by its
process id **and** start time, so a reused process id is not taken for the job). A machine restart ends the job; it is then
`lost` (nothing says why it ended), unless the wrapper had already recorded an exit code.

The environment is cut down to program search paths, `HOME`, locale, proxy settings and the usual tool homes (`JAVA_HOME`,
`ANDROID_HOME`, `CARGO_HOME`, …). Nothing that looks like a secret is passed on; a job that needs one asks the person with
`vault_request`.

### States

| State | When |
|---|---|
| running | the process is alive |
| succeeded | exit code 0 |
| failed (exit N) | another exit code (128 + n when a signal ended it) |
| stopped | a stop was asked for (Stop button or `job_stop`) and the process is gone |
| lost | the process is gone and nothing explains it (killed from outside, the machine restarted) |

**Stop** sends TERM to the whole group, waits three seconds, then sends KILL to whatever is left. Children and grandchildren
go with it. In the screen, Stop runs on a thread, so the click returns at once and the row says `Stopping…`.

### Log

The log is capped at 5 MiB: when it grows past that, the first part is dropped and only the last 1 MiB is kept (the command
writes in append mode, so this is safe while it runs), with a note at the top. What the model or the screen is shown is
**cleaned** (colour codes and the redraws of progress bars are removed, only the last redraw of a line stays) and
**redacted** with the same credential redaction the bug reports use (`DB_PASSWORD=…`, tokens, private keys, `user:pw@host`).
A tail is at most 400 lines and 64 KiB.

### Progress

The engine looks at the last lines of the output for a percentage and shows a bar filled to it; with none, the bar slides
(indeterminate). Recognised: `42%`, `42.5 %` (the newest wins), `[ 42/100 ]`, `[12/340]`, `(3/4)`. Numbers over 100, a zero
total and dates such as `12/10/2026` are ignored. A succeeded job shows 100%.

### Limits

At most **8 running jobs** per person. The 30 newest finished jobs are kept; older ones are deleted when a new job starts.
A job asks for nothing it does not need: it has no network of its own, it uses the person's.

## The assistant's tools

| Tool | Risk | What it does |
|---|---|---|
| `job_start` | **asks** | start a job: `command` (shell line) **or** `argv`, optional `label` and `cwd`; returns at once with the id |
| `job_status` | safe | one job's state, time, progress, last line; without `id`, the list |
| `job_tail` | safe | the last lines of the output (`lines`, default 40, at most 400) |
| `job_list` | safe | the jobs of this workspace; look here before starting a job so it is not started twice |
| `job_wait` | safe | wait for a job up to `timeout_seconds` (never more than 60); returns **as soon as it ends**, with the last output. This is what replaces polling loops |
| `job_stop` | **asks** | TERM, then KILL |

The tools are offered to accounts that may run commands (`CliLevel::Full`, the same level as `run_command`).

**Approvals follow `run_command`.** Starting a job asks the person. In Auto it still asks for what `run_command` always asks for:
destructive programs and verbs (`rm`, `sudo`, `dd`, `chmod`, `delete`, `prune`, `--force`, …) and anything outside the
workspace (absolute or `..` paths), checked on **every command of the line** (split at `;`, `&`, `|`, newlines and `$(`). Policy
refusals apply in every mode: the folder must be inside the person's write scope and, in the IDE, inside the workspace; an account
that may not reach every host may not start a job (a shell line can reach any server); the cluster guard (no second cluster
while one is connected or in a desktop window); at most 8 running. "Always allow" never covers `job_start` or `job_stop`, and
"allow for this session" covers only the same command in the same folder. Status, tail, list and wait never ask; the output they
return is marked as untrusted data.

**The assistant is told when a job ends.** At the start of its next model turn the run adds one note (`[IYAKKI] JOB FINISHED: …
succeeded after 3m 12s. Last output: …`) and the chat shows it as a step. Each job is announced once to the assistant; one it
already saw through `job_wait` or `job_stop` is not announced again.

**A chat that is idle is woken.** A job that ends while no run is going no longer waits for the person's next message: the pending-work
ledger (`crates/iyakki-agent/src/pending.rs`, plan in `docs/development/AGENT-WAIT-PLAN.md`) mirrors every job, and the chat starts a
turn with the result (`[IYAKKI] Work you were waiting for has ended ...`), also for a job that ended while the chat was closed. An
answer that promises "I'll report back" with nothing pending is nudged once and then marked. `wait_for`, `pending_list`,
`pending_cancel`, `pending_check` and `pending_settings` are the chat tools (the last three ask).

**Autopilot waits.** An Autopilot step that ends while a job of the workspace is running does not count as a stall: the run
waits for the job (a note says what it waits for about once a minute), the time spent waiting is not charged to Autopilot's
time budget, and the step is counted neither as "no progress" nor as an attempt at the row. Stop and Pause still work at once; the
wait ends by itself after six hours (the job keeps running). When the job is over the normal rules apply again.

## External coding agents (Claude Code and others)

The six tools are offered through the policy bridge like `run_command`, as `mcp__iyakki__job_start` and the others, with the same
approval cards, the same policy refusals and the same audit lines (`extagent.decision`, `extagent.tool`). The generated agent
instructions tell the agent: for anything expected to take more than about 30 seconds, call `mcp__iyakki__job_start` instead
of running it in a foreground shell (which dies with the session, and the person cannot see it), do not poll with `sleep` or
until-loops, use `job_wait` / `job_status`, read output with `job_tail`, check `job_list` before starting the same job again.
The built-in assistant gets the same paragraph in its system prompt (`jobtools::PROMPT_RULE`).

IYAKKI cannot stop an agent from using its own foreground shell tool; it can only tell it not to and make the better route
available. Advisor agents (custom commands, Copilot set to advisor only) have no tools and are not told.

## What you see

**The Jobs strip** sits in the chat, and in the AI IDE's chat pane, just above the answer buttons and the composer. One row per
job of that workspace, running ones first, at most three rows (the rest are counted, all of them are in RUNS):

* a spinner while it runs, a check when it succeeded, a cross when it failed, was stopped or was lost;
* the label and how long it has run (or ran);
* a bar: a sliding segment while the output gives no percentage, a filled bar with `42%` when it does;
* the last line of output (cleaned and redacted);
* buttons **Log** and **Stop**; a finished row has **Log** and **Dismiss** and stays until dismissed, with how it ended.

**The log view** replaces the conversation area while it is open: a header with the job and its state, **Following** /
**Follow**, **Close**; the text scrolls with the wheel, `PgUp`/`PgDn`, follows the end until you scroll up (new output then does
not move what you are reading; **Follow** or `End` goes back to the end), and can be selected with the mouse and copied with
`Ctrl+C` or the right-click menu (`Ctrl+A` selects all while the composer is empty). `Esc` closes it (and only then leaves the
chat).

**RUNS → Jobs** (the second button at the top of RUNS, or `j`) lists every job of yours, in every workspace, with the same rows
and the log under the list.

**The inbox.** A job that ends while you are in another app leaves one note (finished, stopped or did not finish) that opens
the Jobs list. The chat that started it also shows the end as a step in the conversation.

### Animation and the idle shell

A chat or IDE with a running job asks for about ten frames a second for the spinner and the bar. With no running job it asks
for none: when the last job ends, one frame draws the result, then the screen is left alone. The tests count the frames
(`apps::jobs_screens_tests::the_ide_asks_for_frames_only_while_a_job_runs`,
`shell::jobs_tests::the_shell_draws_extra_frames_only_while_a_job_runs_in_the_open_chat`). The strip reads the jobs about once
a second (every 0.4 s while a run is going and none is known yet, every 3 s when nothing is running and nothing is shown).

Sizes: text uses the shell's scale (`14 px × scale`, never below 12 px) and the buttons the same size as the chat's; rows were
checked at 1280×720, 1920×1080, 1024×600 and 800×480 for fit, overlap and click-target size (at least 22 px tall).

## Verified and not verified

Verified by tests that really run `sh` in a temporary folder: detach and survive dropping every handle, a new manager adopting
running and finished jobs, own session and group, exit codes, Stop killing children and a process that ignores TERM, `lost`
detection including process-id reuse, percentages, the log cap, redaction, scope refusal, per-person isolation, the limit of 8,
announcements once per channel; the six tools with their approvals (including through the real policy bridge), the note a run
gets, Autopilot's waiting; the strip, log view, RUNS list, clicks and geometry at four sizes, the redraw rule, the inbox note.

**Not tried:** a real Android SDK or emulator install; a real Claude Code session using `mcp__iyakki__job_start` (the bridge is
tested with the gate and the fake agent, not with the vendor's program); the strip on the Mac mini or the USB image; a reboot
(`lost` is tested by killing the group, not by restarting a machine); a job that outlives a sign-out on a real multi-user image
(jobs are killed with the machine, not with the shell, but nobody signed out and back in on hardware). A job in a shell that was
**restarted** is found again in tests by a second manager over the same folder, not by restarting the shell program itself.
Other limits: the job tools are not offered to the Parallel-tasks workers; there is no "dismiss all"; a stop of 3 s grace is
fixed; log trimming can drop a few bytes written at the instant of the rewrite.

## Installer jobs (Quick install)

Store → Setup (formerly the IYAKKI menu's Quick install page) runs every step as a job of the workspace `iyakki-install`: the job is
`iyakki quick-install <k3s|duckdb|elasticsearch|claude-code|github-copilot|kokoro|voice-pack|lemonade|ollama> --data <folder>`
(`crates/iyakki-agent/src/quickinstall.rs`), which prints `[n/m]` steps the engine reads as progress. Those jobs are what the
installer indicator beside the clock counts and lists (with the same Log view as the chat's strip); they are not in a chat's
strip. Servers Quick install starts afterwards (Lemonade) run as jobs of `iyakki-services`; k3s and Ollama are started by their
own supervisors in the shell (a small request file in `<data>/quick-install/` hands over). Finished installer jobs leave the usual
inbox note.
