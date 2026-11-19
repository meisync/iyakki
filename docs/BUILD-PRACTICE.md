# Build practice: where to build, how to keep the build machines clean

**For:** anyone building and testing IYAKKI, people and code assistants.
**You will get:** which machine does what, the helper scripts, the habits that keep build hosts healthy, the compressed build disk, and the gate that decides a release.
See also [QUICKSTART-BUILD.md](QUICKSTART-BUILD.md) (images), [BUILD-INCREMENTAL.md](BUILD-INCREMENTAL.md) (build only what changed) and `docs/development/LINUX-ONLY-TESTS.md`.

## Who does what
| Machine | Use it for | Not for |
|---|---|---|
| Your own computer | editing, `git`, reading results | long compiles when a build host exists |
| Linux build host | the release gate, image builds, the `iyakki-core` integration tests and every test that needs Linux | day-to-day compile checks (they crowd the gate) |
| macOS build hosts | compile checks and unit tests of the crates that build on macOS (feature work) | `iyakki-core` and the Linux-only tests |

Rules: one build at a time per machine; build only what changed (incremental); never leave background cargo or test processes running; test scratch files live under the build folder, never in a RAM-backed `/tmp`.

## The helper scripts (`dist/buildhost/`)
* `rcargo DIR CARGO_ARGS...` runs cargo for the worktree DIR on the Linux build host (the source is synced; its own source and target folders on the host).
* `mcargo DIR CARGO_ARGS...` the same on the least loaded macOS build host.
* `rcargo --retire DIR` / `mcargo --retire DIR`: the worktree is finished: delete its copies and stop its leftover processes. Call it when a branch or lane ends.
* `setup-compressed-disk.sh`: one-time setup of the compressed build disk (below), run on the Linux host.
* Settings live in one file per person, never in the repository: `~/.config/iyakki/buildhosts.env` with `LINUX_BUILD_HOST=user@host`, `LINUX_BUILD_DISK=/mnt/iyakki-build` and `MAC_BUILD_HOSTS="user@host user@host"`. Use key-based ssh.

What they do by themselves: after every `rcargo` run (also a failed one) they stop the test processes still running from that worktree's scratch folder and delete the scratch (`tmp`, `tmp-priv`); before a run they refuse to build when a disk is above 85% (exit 96) or the compressed disk is not mounted (exit 95). They never delete anything else on their own: a person or an agent calls `--retire`.

## The compressed build disk
A sparse image file formatted as btrfs and mounted with `compress=zstd:1,noatime` (a real 25.8 GB target folder took 5.6 GB, ratio 4.6). It is a hard ceiling (builds cannot fill the root disk) and costs almost no build speed. Create it with `dist/buildhost/setup-compressed-disk.sh [IMAGE] [SIZE] [MOUNT]`; it needs `sudo` once, adds a `nofail` line to `/etc/fstab` (after saving a copy) and installs `btrfs-progs` if missing. If it is not mounted, `rcargo` refuses to run, so it can never fall back to the root disk.

## Why this exists
Every worktree used to keep its own target folder (about 10 GB each). About a hundred forgotten ones filled a build host to 95%; the cluster on that host put a disk-pressure taint on the node, pods were evicted and the git server stayed down, while a stray test process pinned 13 cores for 100 minutes. The fixes are the habits above: scratch deleted after each run, finished worktrees retired, a disk guard, a compressed disk with a ceiling.

## The gate
`./run.sh release-green [--commit REV]`: both test layers on one commit (the failing tests are rerun alone to tell load flakes from real failures), one `RESULT.txt`, one lock so only one run goes at a time, about an hour. Launch it once and wait for it; read only `RESULT.txt`. A release also needs `./run.sh alpha-check --release` on the same commit (licences, links, secrets, versions); the green stamps are tied to the exact commit that is tagged.
