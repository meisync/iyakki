# IYAKKI pipeline: boot test (working) and build stage (design)

Automates "does this image boot?" so nobody types QEMU commands. Everything runs in the namespace `iyakki-build` of **this machine's**
k3s (or, without k3s, in plain Docker with the same image and checks). Design and honest limits of the whole pipeline:
[`docs/development/BUILD-PIPELINE-K3S.md`](../../docs/development/BUILD-PIPELINE-K3S.md).

## Use

    dist/pipeline/run-boot-test.sh  IMAGE [SECONDS]     # k3s Job; waits, prints result.json, exits 0 pass / 1 check failed / 2 harness failed
    dist/pipeline/run-boot-test-docker.sh IMAGE [SECONDS]   # same container under Docker (no cluster needed)

`IMAGE` is a raw disk image such as `~/iyakki-work/images/<name>/iyakki-lean-x86_64.img`; `SECONDS` is how long the guest runs
(default 90). Results go to `${PIPELINE_RESULTS_DIR:-$HOME/iyakki-work/pipeline-results}/<run-id>/` (world-readable):
`result.json` (pass/fail per check), `serial.log`, `shot-003s.png` ... `shot-NNNs.png` (screendumps from the QEMU monitor at 3, 8, 15, 30, 60 s and
SECONDS-3), `qemu.err`, `job.yaml`, `pod.log`, and a `DONE` marker. The image file is mounted read-only; QEMU writes to a throw-away overlay.

### One-time setup

1. The container image (about 316 MB, Debian 12 slim + QEMU + OVMF + Python): `docker build -t iyakki-build/boot-test:1 dist/pipeline/boot-test-image`,
   then for k3s `docker tag iyakki-build/boot-test:1 localhost:5000/iyakki-build/boot-test:1 && docker push localhost:5000/iyakki-build/boot-test:1`
   (k3s on this machine mirrors `localhost:5000` as http; override with `BOOT_TEST_IMAGE`). Rebuild only when `boot-test-image/` changes.
2. A kubeconfig for **this machine's** k3s: `export KUBECONFIG=/etc/rancher/k3s/k3s.yaml` (readable on this PC). The default `~/.kube/config` may point to another
   cluster; `run-boot-test.sh` refuses to run unless the node it targets has this machine's `/etc/machine-id` (the Job mounts host paths and `/dev/kvm`,
   which only exist here). `PIPELINE_NODE` overrides the node name (default: `hostname`). Check `kubectl get nodes` shows this PC before applying anything.
3. `kubectl apply -f dist/pipeline/namespace.yaml` (the wrapper does it when the namespace is missing).

## What the boot test checks

1. `no_panic`: no line of `serial.log` matches `panic|failed printing|Kernel panic` (lines containing `panic=` are the kernel command line and are skipped).
2. `serial: stage1: switching root` and `serial: IYAKKI-BOOT supervisor up` both appear.
3. `qemu_ran_to_the_end`: QEMU was still running when the time was up (did not exit or reboot early; `-no-reboot` is set).
4. `screendumps_taken`, `final_screen_not_blank` (more than 0.5 % lit pixels in the last screendump).
5. `no_tofu`: a pixel heuristic on every screendump. Foreground = pixels far from the most common brightness; connected components that are glyph sized
   (5-64 x 7-96 px), have four almost full edges and an empty inside are "hollow boxes"; six or more with the identical size = tofu (the rows of equal rectangles
   of 2026-11-07). Tested on a synthetic row of boxes (detected), on rendered text and on real 1107f screendumps (not flagged). **Limits**: it only knows
   hollow boxes (a font whose missing-glyph is a solid block or a "?" is not caught), and no real tofu screenshot was available to test against. The PNGs are
   always kept: the screendump at 15 s and later show the sign-in screen and are meant for a human look when in doubt.

Not checked (no OCR): that the login/first-run text says the right words. At 3 s the screen is still black (firmware); the GRUB menu did not show in the
1107f run because the lean image boots its default entry at once, so the "boot menu" screendump of the original request does not exist for this image.

## Why `privileged: true`

QEMU needs `/dev/kvm`. Kubernetes mounts it as a `hostPath` CharDevice, but the container runtime's device allow-list does not cover a device that is only
mounted, so an unprivileged container gets `EPERM` on open (Docker's `--device` adds the allow rule, a hostPath does not). The alternative is a device plugin
(a DaemonSet from the cluster's own set of tools), which is more to maintain than one privileged Job in a namespace that holds nothing else.
**Verified on k3s (2026-11-07):** with `PRIVILEGED=false` the pod reported `/dev/kvm is not usable in this pod` (result `error`, exit 2); with privileged KVM works. `PRIVILEGED=false ./run-boot-test.sh ...` tries without. The namespace carries the label
`pod-security.kubernetes.io/enforce: privileged` so the Job is admitted. The pod mounts only the image directory (read-only), the results directory,
`/dev/kvm` and an emptyDir. It does not mount any secret and never `~/.config/iyakki/secureboot`.

## Resource limits

`ResourceQuota`: requests 8 CPU / 10 GiB, limits 12 GiB memory, 6 pods, **no PVCs**. `LimitRange`: defaults 0.5 CPU / 512 MiB requested, container max 6 CPU / 9 GiB.
A boot test requests 2 CPU / 2 GiB, limit 3 CPU / 3 GiB (guest 2 GiB). The build skeleton requests 4 CPU / 6 GiB, limit 9 GiB: a build and a boot test fit together
(8 CPU, 8 GiB requested, 12 GiB limits) and nothing else does.

## What it does not do

* It does not build anything and does not sign anything (see the build design; the Secure Boot key stays on the host).
* It does not replace `dist/buildroot/qemu-test.sh` (the self-test initrd run) or `qemu-splash-test.py`; it boots the finished **disk image** as a stick would.
* It does not test hardware (touch, Wi-Fi, sound), Secure Boot, or an image that needs a data disk.
* The screen is not read, only measured (see the limits above).

## Removing it

    kubectl delete ns iyakki-build          # removes the quota, the Jobs, pods and everything labelled app.kubernetes.io/part-of=iyakki-build

Only hostPaths are used (no PVC), so no PV exists; had local-path PVs been used they would go with the namespace (reclaim policy Delete). Outside the namespace
remain, by design: results under `~/iyakki-work/pipeline-results/`, and the container image in the registry (`docker rmi`, and delete the repository
`iyakki-build/boot-test` in the registry if wanted).

## Files

`namespace.yaml` (namespace, quota, limit range), `boot-test-job.yaml.tmpl` and `run-boot-test.sh` (k3s), `run-boot-test-docker.sh` (Docker),
`boot-test-image/` (Dockerfile and `boot-test.py`, standard-library Python), `build-job.yaml.tmpl` (build stage design, never launched).
Third-party software pulled (Debian 12 archive, into the test container only; nothing is shipped in an IYAKKI image): `debian:bookworm-slim`, `qemu-system-x86`
and `qemu-utils` (GPL-2.0, used as separate programs, not linked), `ovmf` (BSD-2-Clause-Patent), `python3` (PSF). No vendor firmware.
