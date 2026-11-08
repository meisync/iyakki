# Build and test pipeline on k3s (boot test working, build stage designed)

Written 2026-11-07. Usage and files: [`dist/pipeline/README.md`](../../dist/pipeline/README.md). This page says what the pipeline is for, what k3s adds,
what it does not speed up, and what stays manual. Companion of [`COMPILE-SPEED-PLAN.md`](COMPILE-SPEED-PLAN.md) (the table "End to end").

## The loop

    change -> [1 compile] -> [2 repack + sign] -> [3 lean image] -> [4 boot test] -> [5 pack] -> (push to a device: manual)

| Stage | Where | Status |
|---|---|---|
| 1 compile (cargo) | builder pod `dist/pipeline/build-job.yaml.tmpl` | **design only, never launched** |
| 2 repack (+ Secure Boot signing) | **the host**, by the owner's scripts | manual, on purpose (key never enters the cluster) |
| 3 lean image | the host (`lean-image.sh`) or the builder pod with the same script | design |
| 4 boot test | k3s Job `run-boot-test.sh IMAGE` | **built and validated** (see "What was run") |
| 5 pack | the host or the builder pod | design |

## The build stage design

* **A pod, not a rewrite.** The pod runs the same commands the owner runs (`BUILD_CMD`, for example `cargo build --release ...` or `dist/buildroot/repack.sh pc ... --cargo` without
  signing variables). The repository, `~/.cargo`, `~/.rustup`, `~/.cache/iyakki-tmp` and `~/iyakki-work` are `hostPath` mounts at their **host paths**, so the build is incremental
  (the same `target/` and registry as the dev PC) and scripts that `cd` into the checkout work unchanged. The builder image must have the host's libc (Ubuntu 26.04), because the
  mounted Rust toolchain and the Buildroot host tools were built against it.
* **Limits**: pod memory limit 9 GiB (request 6 GiB), 6 CPU limit, `CARGO_BUILD_JOBS=4`. The owner's `systemd-run -p MemoryMax=9G` cap becomes the pod limit; the kernel OOM-kills the pod, not the PC.
  Swap is not limited per pod (the host's zram/`MemorySwapMax=6G` policy does not carry over); with swap on the node a build can still page. Not tested.
* **One build at a time: a file lock on the shared hostPath.** The pod does `flock -w 7200` on `~/.cache/iyakki-build.lock`, **the file the owner's runner scripts already lock**
  (`flock ~/.cache/iyakki-build.lock ...`). So a pod build, a hand-started build and another coding agent's build queue behind one another with no new mechanism.
  A Kubernetes `Lease` would work only between pods: it cannot see builds started outside the cluster (the owner's, the agents'), and those are the common case. A file lock is also released by the
  kernel when the process dies, so a crashed pod cannot leave a stale lock; a Lease needs an expiry and renewal logic. The cost: the lock is per machine (fine: one dev PC) and an
  in-pod `flock` on a bind-mounted file is expected to share the host inode (standard, not yet tried here).
* **Secure Boot key stays on the host.** `~/.config/iyakki/secureboot` is never mounted into any pod; the template sets no `IYAKKI_SB_KEY` / `IYAKKI_SB_CERT`. `repack.sh` signs only when those are set,
  and `changed.sh` treats signed-to-unsigned as a FULL rebuild, so the pod must not repack into the signed output directory. Plan: the pod does the compile (stage 1, the slow, key-free part) and
  the host runs the signed repack (stage 2, about 2.5 min) from the shared `target/`. A pod-built unsigned image is good for QEMU boot tests (no Secure Boot there) but not for the Lenovo or Mac.
* **Not launched**: Buildroot (`FULL`) builds are hours and are left to the owner; the pod is meant for the cargo compile and repack.

## What k3s adds (honestly)

* **Isolation**: a hard memory ceiling and CPU limit per build and per test; a runaway build or QEMU cannot take the machine's 28 GB. The namespace quota caps the sum (8 CPU / 10 GiB requested, 12 GiB limits).
* **Queueing**: with the shared lock, Jobs wait their turn; a Job can be submitted and left.
* **A place for boot tests**: a fixed container (QEMU, OVMF, Python) that behaves the same for the owner and every agent, with results at a fixed path, instead of each person's `qemu` command line.
* **Unattended runs**: `run-boot-test.sh` returns an exit code, so a script, a Gitea Actions job or a loop can gate on it; Jobs clean up after themselves.
* **One removal**: `kubectl delete ns iyakki-build`.

## What it does NOT speed up

* **The compile itself.** It is limited by the crate size (`iyakki-agent` 350 000 lines, `iyakki-shell` 276 000) and by the release profile (fat LTO, one codegen unit: 8 m 50 s), plus the duplicate second
  cargo call (3 m 49 s). A pod runs the same rustc on the same cores; `CARGO_BUILD_JOBS=4` and a 9 GiB cap are slower than an idle PC, not faster. The wins are in `COMPILE-SPEED-PLAN.md`
  (the `device` profile, one cargo call, no re-pruning, the crate split, WASM apps, binary deltas). The pipeline only removes the human from the loop.
* **Pushing to a device** (420 MB upload) and the reboot.
* A boot test in QEMU takes 90 s of wall time by design (`SECONDS`).

## What remains manual

* Signing (host, key never in the cluster) and writing sticks.
* Pushing to the Mac or the Lenovo (`iyakki dev ...`), and everything that needs real hardware.
* Building the builder image, starting a build, and Buildroot `FULL` builds.
* Reading the screendumps when the tofu heuristic is in doubt; there is no OCR, so the words on the sign-in screen are not compared.
* Exporting `KUBECONFIG=/etc/rancher/k3s/k3s.yaml` (see the README).

## What was run on 2026-11-07, and what was not

* **Validated**: the boot-test container (image `iyakki-build/boot-test:1`, built from `boot-test-image/`) ran `~/iyakki-work/images/pc-lean-1107f/iyakki-lean-x86_64.img` with KVM under
  **plain Docker** (`--device /dev/kvm`, unprivileged): all 7 checks passed in 91 s, serial shows `stage1: switching root` and `IYAKKI-BOOT supervisor up`, the screendumps at 15, 30, 60 and 87 s show the
  sign-in/first-run screen ("Welcome - create the administrator account"), the 3 s screendump is black. A negative run (a blank 4 MB image) failed as it should (exit 1, both serial markers missing).
  Results: `~/iyakki-work/pipeline-results/{docker-validate-1107f,iyakki-lean-x86_64-1107-213638,neg-blank}/`.
* **Validated on this PC's k3s** (`KUBECONFIG=/etc/rancher/k3s/k3s.yaml`, one node, the same name as the hostname): namespace `iyakki-build` with quota and limit range created; the image was pulled by k3s from
  `localhost:5000/iyakki-build/boot-test:1` (the local Docker registry, mirrored as http; no `ctr import` and no sudo needed). `run-boot-test.sh` on the 1107f image, 90 s:
  **unprivileged**: refused, `/dev/kvm is not usable in this pod`, exit 2 (3 s); **privileged**: all 7 checks pass, accel kvm, the same sign-in screen. The wrapper deleted the Job after the run.
  Failing case: a blank image through the wrapper gave exit 1 with both serial markers false. Results under `~/iyakki-work/pipeline-results/` (`iyakki-lean-x86-64-1107-214123`, `k3s-neg-blank`).
* **Earlier mistake, cleaned up**: the default kubeconfig points to a different machine (192.168.50.68, node `node-main`). A first attempt created `iyakki-build` and one Job there; the pod never started
  (no such hostPaths), the namespace was deleted, and the pushed image's manifest was deleted through the registry API (HTTP 202; blobs stay until that registry runs garbage collection). The wrapper now refuses a node that is not this machine.
* **Still unverified**: the 9 GiB swap behaviour of the build pod, and the `flock` on the shared lock file from inside a pod.
* **Not run at all**: `build-job.yaml.tmpl` (design; the YAML parses and fits the quota: 4 + 2 CPU and 6 + 2 GiB requested, 9 + 3 GiB limits).
