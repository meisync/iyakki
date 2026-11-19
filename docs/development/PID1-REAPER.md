# The PID 1 reaper and the children other code watches

Status: design note, 2026-10-27. Code: `iyakki_core::reap_children` (crates/iyakki-core/src/lib.rs), the reader side
`iyakki_core_api::child_status` (crates/iyakki-core-api/src/childstate.rs).

## What the reaper reaps

IYAKKI's agent runs inside PID 1. As PID 1 it inherits every orphan on the machine, and a zombie stays in the process table
until somebody waits for it. `reap_children()` therefore loops `waitpid(-1, WNOHANG)` and discards the statuses.

## Why there is a race

`-1` means any child of this process. The programs the agent starts itself (k3s, kubectl, ollama, aplay, cage, helper
programs) are children of PID 1 too, not only the orphans. Rust's `Child::try_wait()` and `wait()` call `waitpid(pid)` on
that one pid. A zombie can be collected only once, so when the reaper runs between the child's death and the watcher's next
look, the watcher's call fails with `ECHILD` instead of returning the status.

Code that read that error as "still running" never noticed a dead child (k3s was invisible for three outages). Code that
read it as a failure reported a successful command (a `kubectl` whose status was stolen) as failed.

## What was done (the readers)

Every reader now goes through one helper, `child_status(&mut Child) -> ChildState`:

* `Running`
* `Exited(Option<i32>)`: status read (`None` after a signal)
* `Gone`: `ECHILD`, or the pid no longer exists (`kill(pid, 0)` gives `ESRCH`): it ended, the code is unknown

For `wait()` callers `child_wait` turns `ECHILD` into `Gone`; the caller still reads the pipes it holds, so the output is
complete. Whether a `Gone` command succeeded is decided per caller: command runners that have output and nothing on stderr
treat it as done (`Out::status_lost`), the supervisors treat it as an exit and restart, audio players treat it as the end.
`iyakki-webview` has no dependencies and keeps its own tiny copy.

This makes the readers correct whoever wins the race. The loss is only the exit code (and the signal), which matters little
for the supervised daemons and a little for command runners.

## Options for the reaper itself

| Option | How | Pros | Cons |
|---|---|---|---|
| A. Leave it | readers handle ECHILD | nothing to break; no zombies ever | exit codes are lost at random for children of PID 1 |
| B. Registry of own pids, reaper skips them | every spawn goes through `spawn_managed`, which records the pid; the reaper skips recorded pids and `waitpid`s the rest one by one (needs a `/proc` or `waitid` peek to find the orphan zombies) | exact codes for everything started by us | about 90 spawn sites (`Command::spawn`, also in other crates); one forgotten site is a leaked zombie or a stolen status again; the registry needs cleanup when a child is dropped without a wait |
| C. `waitid(P_ALL, WNOWAIT)` peek | look at a zombie without collecting it, collect only if its pid is not ours | no extra spawn wrapper to call | still needs to know "ours": that is B again; a peek loop spins on a zombie that is ours and uncollected |
| D. Reap only orphans by ppid from `/proc/<pid>/stat` | orphans have ppid 1 | no registry | our own children also have ppid 1 (we are PID 1), so ppid cannot tell them apart; only works if the agent were not PID 1 |
| E. pidfd | `pidfd_open` for each child we start, `waitid(P_PIDFD)` | race free per child | Rust std has no pidfd wait on the stable path; still needs every spawn site; the reaper still has to be told which pids to skip |
| F. Split: a tiny init (PID 1) and the agent as its child | the init reaps; the agent is not PID 1, so its own children are its own and orphans go to the init | the cleanest answer: the race disappears at the source | a boot change (initramfs, supervision of the agent restart, shutdown order); needs hardware testing |

## Recommendation

Keep A now (done) and plan F: a minimal init that does the early boot, starts the agent, reaps orphans and restarts the
agent. Then `waitpid(-1)` only ever sees orphans and agent-started children keep their exact status. B is the fallback if F
is not wanted, but its cost is a spawn wrapper in every crate and the failure mode of one missed site is exactly the bug
being fixed.

## Not implemented, and why

No change was made to `reap_children()`. The only safe small improvement is a registry plus a skip list, and it is only
safe when all spawn sites use it; a partial registry would give a false sense of safety and no test could prove the
untouched sites. The readers are the fix; the reaper stays as is until F is decided.

## Tests

* `iyakki-core-api::childstate::tests`: a child whose status is taken (`waitpid` on its pid) gives `Err(ECHILD)` from std
  and `Gone` from the helper; output in its pipes is still read.
* `iyakki-agent::childgone_tests`, `ollama::gone_tests`, `cluster::gone_tests`: the monitors report the exit and `stop()`
  does not hang; a command keeps its output; a timeout is still a timeout.
* `iyakki-core::storageloss::tests`, `iyakki-webview::childstate::tests`.

For code that starts its own child, the test helper `steal_status_of_marked(marker)` finds a child by its command line and
takes its status from a thread, the way the reaper does.
