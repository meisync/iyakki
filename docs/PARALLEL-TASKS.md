# Parallel tasks

The AI IDE can run several isolated pieces of work at the same time, the way the Claude Code and GitHub Copilot agent panels do:
each **task** has its own copy of the workspace, its own chat session, its own budgets and its own status. You watch them on
one screen (the **Tasks** tab next to Board), answer their questions in one queue, and **review and merge** what each one made.

Code: `crates/iyakki-agent/src/tasks.rs` (isolation, scheduler, review, merge; no UI, no model), `tasktools.rs` (the assistant's
`task_spawn`), `crates/iyakki-shell/src/apps/ide_tasks.rs` (the tab). Tests: `tasks_tests.rs`, `ide_tasks_tests.rs`.

## What a task is

| Field | Meaning |
|---|---|
| id, title, goal | `t1a2b3` (short, unique in the workspace), a one-line title, what to do |
| rows | board rows (`F-…`) the task works on, at most 12; a free-form goal has none |
| agent, model | the built-in assistant, Claude Code, the Copilot advisor or a custom agent (same ids as the chat's model chooser), and the model name |
| status | `queued`, `running`, `needs you` (an approval or a question waits), `paused`, `failed`, `done`, `merged`, `discarded`, plus `interrupted` (the window closed while it ran) |
| progress | rows done of total (or steps for a free-form goal), elapsed time, tokens and cost, the last event line |
| branch, tree | `iyakki/task-<id>-<slug>` and `<workspace>/.iyakki/tasks/<id>/tree` |

A task is made from a board row (context menu **Run as task**, or **Run selected rows in parallel**: one task per row), from the
**New task** button, or by the assistant: `task_spawn {title, goal, rows?, agent?, model?}` (or `tasks: [...]`; Ask level: you
approve each spawn in Ask mode, and in Auto mode it runs like any other Ask tool; at most 6 in one call and at most 12
unfinished tasks in a workspace; an "always allow" never covers it). `task_status` reads them. A workspace opened read-only (another
window holds it) and an account without the full command-line permission cannot start tasks.

## Isolation model

Each task works in its **own copy** of the workspace; nothing it does touches your files until you merge.

* **Git workspace** (the workspace folder is the root of a repository with at least one commit): `git worktree add -b
  iyakki/task-<id>-<slug> <workspace>/.iyakki/tasks/<id>/tree <HEAD>`. The base commit is recorded. IYAKKI never commits to
  your branches while the task runs; the task's own commits go to the task branch only. Uncommitted changes in the main
  workspace are **not** in the task (the task starts from HEAD); the card says so when the main workspace is dirty.
  `.iyakki/tasks/` is added to the repository's `info/exclude`, so your `git status` stays clean.
* **No git, or the folder is inside a larger repository**: a regular copy of the workspace files (skipping `.git`, `.iyakki`,
  plain names in the root `.gitignore` (not `.dockerignore`: that file describes an image, not the source), and heavy folders: `node_modules`, `target`, `dist`, `build`, `.venv`, `__pycache__`, `vendor`) with a
  throwaway private git repository inside the copy, used only to take a base snapshot and to make the diff. A size cap
  (200 MB, 20,000 files) refuses a bigger workspace with a clear message. Dependency folders are **not** symlinked (a link
  would let a task write into the real one); the task reinstalls them (`npm install`) inside its tree.
* **Git is required** to start a task (it is the diff and merge engine). Without it the New task form says so and nothing starts.

What is per task:

* **Chat session**: `task-<id>`, with its own history namespace tag (the workspace knowledge base records the steps under it)
  and, for Claude Code, its own `--resume` session (the session store is keyed by the task session, never shared).
* **Project state**: the PROJECT STATE block is built from the task's tree and the task's own board.
* **Board**: `.iyakki/tasks/<id>/FEATURES.md`, seeded with copies of the assigned rows only. The task's assistant plans and
  updates *that* board; Autopilot's verification (by row kind, run inside the tree), `B-` bug rows and their evidence
  (`.iyakki/bugs/` inside the task directory) all work there unchanged.
* **File jail**: the working folder is the tree; the IYAKKI file tools (`ProjectTools` rooted at the tree) and the
  external-agent bridge (workspace = the tree) refuse paths outside it. The task's policy gets read and write scope of the
  tree only, so the main workspace is **not readable** from a task either (a documented limit: read-only access to the main
  workspace for reference is not built).
* **Namespace**: tasks may build and test locally. **Deploys are refused** from a task (the `deploy_*` and `k8s_*` tools are
  not offered) unless you turn on *tasks may deploy*; that switch is a setting, off by default, and is **not wired to a
  per-task namespace in this version**: the namespace name `<namespace>-dev-<id>` is reserved and recorded on the task, the
  teardown hook runs when the task is merged or discarded, but no cluster call is made yet. Deploys go through the lead
  assistant after the merge.
* **Ports**: each task gets a port block (`20000 + 10 * n`) written into its prompt (told, not enforced); two servers that
  ignore it can collide.

### Shared state is written by one writer

`FEATURES.md` (the main board), `.iyakki/project.json` and `.iyakki/state.json` are never edited by a task. A task reports row
status through the manager, which is the only writer of the main board and does it under one lock (`with_board`), so two tasks
finishing together never lose an update. **Row ownership**: a row assigned to an unfinished task (not merged or discarded)
cannot be given to another task or run twice. The manager sets the row `partial` ("task t1a2b3 running") when it starts,
`review` when it finishes (work claimed, not merged), `done` after a merge of a task whose rows were all verified in the tree,
and puts the row back as it was when the task is discarded.

## Running

The **scheduler** keeps at most *N* tasks running (default 3, hard cap 6, setting `max_concurrency`); the rest wait as
`queued` in creation order. Each task runs the Autopilot machinery on its own worker thread with its own `Context`, `Executor`,
budgets (steps, minutes, tokens, cost; the Autopilot defaults, overridable per task and capped by the account's budget) and
honest summary. A task that goes over a budget is stopped and shown `failed` with the reason; **Retry** re-queues it, **Pause**
saves its place (Autopilot's pause) and **Resume** continues from it, **Stop** ends it.

**Approvals** from all tasks arrive in one queue: an inline card in the Tasks tab (and the task's detail) tagged
`[task title]`, an inbox item, and the card's status `needs you`. A task's approvals never apply to another (each run owns its
decision channel). Auto approves only what Auto approves anywhere (per-agent Auto setting), `hard_ask` calls still ask, and
**Merge is never auto-approved**: it always shows its own confirmation.

State is saved to `.iyakki/tasks/<id>/task.json` after every change. When the IDE closes (or a workspace is closed) while tasks run,
**running tasks are stopped** (their place is saved; this waits a few seconds) and a restart shows them as `interrupted` with
**Resume**; a window that died without closing is recovered the same way. The IDE does not ask first when it closes (not built).

## Review and merge

A finished task shows: the summary (what was executed, not what was said), the changed files, a **diff** (unified, against the
base, one block per file that collapses, selectable text, added and removed lines tinted), the test result run in the tree
(**Run tests** uses the workspace's test command inside the tree) and the evidence folder.

Actions:

* **Merge**: confirm, then (git workspace) `git merge --no-ff` of the task branch into the branch checked out in the main
  workspace, or (copy mode) `git apply` of the task's patch. Before anything is applied the **dry run** reports the result:
  `git merge-tree --write-tree` for a git workspace (conflicting files are listed), `git apply --check` for a patch. Files the
  task changed that you have uncommitted changes in are reported as blocking. On success the worktree and branch are removed.
* **Conflicts**: the card lists the conflicting files and offers **Rebase on latest** (git: `git rebase` of the task branch
  onto the main branch's HEAD in the tree; a rebase that conflicts is aborted and reported), **Open in editor** (opens the
  tree's file) or **Discard**. Merges happen **one at a time**, in the order you press Merge; each one is checked again
  against the main branch as it is then, so a task made stale by an earlier merge shows its new conflicts.
* **Discard**: asks first; removes the worktree, the branch and the task directory.
* **Keep**: leaves the branch (removes the worktree), so you can merge it by hand later.
* **Open in editor** (key `o` in the Files tab): opens that file of the task's tree in the editor (the tree is inside the
  workspace folder). Switching the whole file tree to the task is not built.
* **Ask follow-up**: sends a message to that task (Autopilot's interjection) and resumes it.

## The Tasks tab

Open it with the **Tasks** tab (right after Board), Ctrl+Shift+G, or the amber/cyan chip `tasks: 2 running · 1 need you` in the
top bar (it appears only while something is running, waiting or queued). Left: the counts, **+ New task**, the concurrency
control (`−` `max 3` `+`), the one list of waiting approvals (each card says `[task title] tool summary` with Allow and Deny
that answer only that task) and the task cards. Right: the selected task with the buttons that fit its status, and the tabs
**Summary**, **Files**, **Diff**, **Log**. In a narrow pane the list and the detail are pages of their own (Back or Esc returns).

Keys (the tab has no text field; dialogs hold the text): `↑ ↓` select · `Tab` next detail tab · `n` new · `s` stop · `p` pause ·
`r` resume or retry · `f` follow-up · `m` merge · `t` run the tests in the task's folder · `b` rebase on latest · `k` keep ·
`d` discard · `y`/`x` allow or deny the task's first waiting approval · `c`/`e` fold or unfold every file in the diff · `o` open
a file in the editor · `+`/`-` tasks at once · `Esc` back. The diff text can be selected and copied (right-click: Copy,
Select all, Copy all). On the Board, right-click: **Run selected row as a task** and **Run all planned rows in parallel**.

## Limits and what is not built

* No live model or vendor run has been exercised: tests use scripted models and a fake runner.
* The transcript in the detail pane is the task's event log (steps, tool results, approvals), not an embedded chat app; the
  follow-up box is the only input.
* The main workspace is not readable from a task; read-only reference access is not built.
* Deploys from tasks are refused; per-task namespaces are reserved, not created.
* Built-in tasks use the model chosen in the chat at the time they start; a per-task model for the built-in assistant is
  recorded for display only. External agents use their own configured model.
* Ports are advisory. Dependency folders are not shared between trees.
* One workspace session owns the tasks (the workspace lock): a second window shows them read-only.

## Security

* Writes are confined to the task tree twice (the IYAKKI tools and the external-agent bridge); symlinks are resolved first.
* Nothing is merged, deleted or deployed without a confirmation; the lead assistant has no merge or discard tool, only
  `task_spawn` (Ask) and the read-only `task_status`.
* Row and board writes go through one lock and one writer; task ids and branch names are generated by IYAKKI (a slug of
  letters, digits and dashes), never taken from model text, so a title cannot inject a git option or a path.
* Every git call is made with fixed arguments and `--` before paths; branch names are checked before use.
* Audit: `task.spawn`, `task.start`, `task.stop`, `task.merge`, `task.discard` with the task id.

## Rows that depend on rows, and agents

A task whose rows wait for rows that are not done (the board's optional `Depends` column) stays *queued* with the line "waiting for
F-A-1 to be done" and starts when they are done; the person can start it anyway (`override_deps`, only from the New task form of the
shell, never from the assistant's `task_spawn`). The scheduler never starts two tasks on one row (a row belongs to one unfinished task).
Every running task is also an **agent** with a stable name (`Agent 1`, ...), recorded with its state, timeline, files (with the size of
each change, from git) and notes in `.iyakki/board-assignments.json` (`assign.rs`); the Board's Agents strip and agent panel read it,
and **Message** (`Manager::guide`) pauses a running task after its step and resumes it with the text. When a task is merged and rows
become ready, the setting **auto-assign** (`off`, `ask`, `on`; `.iyakki/tasks/config.json`) offers or starts tasks for them
(`Manager::offer_ready`). Two tasks that change the same file are flagged and the person may queue the second. See `docs/AI-IDE.md`
(Dependencies between rows, Agents on the board).

Every task run is also recorded as a run log with `task_id`, row and agent (`RunLog`, `Hooks.record`; saved at start, every 40 events and at the
end; a new run id at every start), listed in RUNS as `Task <id> · Agent 2 · F-APP-01`; the Board's agent panel **Open run** hands that run to
RUNS (`("runs", "run:<id>")`), and RUNS goes back with `task:<id>` / `row:<id>`.
