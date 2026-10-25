# Build your own image app

The owner's decision of 2026-10-21 replaced app stores with two things: ready-made **image apps** (the third tab of IYAKKI menu → Linux apps) and
**building your own**. This page is the second. Overview of the whole feature: [`LINUX-APPS.md`](LINUX-APPS.md). Nothing here was run on a real
device or cluster; what is tested is said in section 6.

## 1. What an image app is

A small JSON file. Put it in `<data>/image-apps/` (the data folder is `/var/lib/iyakki` on a device); the page lists it on its next refresh
(`r`). Files that do not pass are not listed and the page says which and why.

```json
{
  "id": "my-notes",
  "name": "My notes server",
  "description": "What it is, in a sentence or two.",
  "kind": "container",
  "image": "localhost:30500/my-notes:1@sha256:<64 lower-case hex digits>",
  "command": [],
  "args": [],
  "ports": [{ "name": "http", "port": 8080 }],
  "env": [{ "name": "MODE", "value": "demo" }],
  "volumes": [{ "name": "data", "path": "/data", "size_mb": 64 }],
  "licence": "MIT",
  "homepage": "https://example.org",
  "run_as_user": 10001,
  "memory_mb": 256,
  "cpu_milli": 500
}
```

Rules (checked when the file is read): the **image must end in `@sha256:<digest>`** (a tag alone is refused: a tag can be moved to different
contents); `licence` is required; at most 5 ports, 20 environment variables, 3 volumes (each an `emptyDir` with a size limit; the app's root
file system is read-only whenever it has a volume); `id` is lower-case letters, digits and `-`. **Run** applies a Deployment and a Service
(NodePort) to the namespace `iyakki-apps` of the dev cluster with the managed `kubectl`; the pod has no service-account token, no
privileges, all capabilities dropped and the default seccomp profile. Because of that, an image that wants to be root or bind a port below 1024
needs `run_as_user` and a high port (8080, not 80).

## 2. Write the image in the AI IDE

Create a workspace, add a `Containerfile` (or `Dockerfile`), the app's files, and the JSON above. The IDE's Deploy tab already knows
Dockerfiles ([`K3S-DEV.md`](K3S-DEV.md)). **IYAKKI's image ships no container builder** (no `buildah`, no `podman`, no `docker`, no BuildKit): the
Podman variant of the dev cluster is wired but not in the default images. So there are three routes on the image as it is:

### Route A: a program in one file (works today, no builder)

A single static binary (a Go or Rust program built with musl) is a complete image: `ocipush.rs` builds a one-layer image from one file in
memory and pushes it to the local registry (Store → Setup → Data services → *Local image registry*; `localhost:30500`), without any container tool.
The private store's deploy uses the same code. Copy the digest the push prints into `image` of your JSON.

### Route B: a Debian or Ubuntu root assembled in a sandbox, imported as a sandbox (works today for the sandbox runtime)

1. IYAKKI menu → Linux apps → New: make a sandbox (say `build`, Debian 12), network on.
2. In the CLI tile: `linux apt install build debootstrap` (the sandbox `build`, the package `debootstrap`). Then, as a root-inside job, assemble what you want, for example with `debootstrap` or simply the packages
   you need installed with `apt` in a *second* sandbox. (`debootstrap` needs `mknod` and mounts, which the sandbox refuses by design; its `--variant=minbase`
   with `--foreign` unpacking of the `.deb` files works as far as it needs no devices. If it fails, assemble the root by installing packages with `apt`
   in a sandbox instead: that root *is* the result.)
3. Archive the root: mount a folder read-write at `/mnt/out` and, inside, `tar -C / --exclude=/mnt --exclude=/proc --exclude=/dev --exclude=/tmp -czf /mnt/out/myroot.tar.gz .`
   (as a `bash -c`).
4. `linux import myroot /path/to/myroot.tar.gz` (CLI tile) unpacks it with the strict reader (links, devices and set-id bits handled as in
   [`LINUX-APPS.md`](LINUX-APPS.md) section 4) and gives you a new sandbox that runs your programs: *import tarball as image app*.

This route gives a sandbox on this machine, **not** a container image for the cluster.

### Route C: a real container image, built elsewhere (works with any registry)

Build with your own tools on another computer (`podman build`, `docker build`, GitHub Actions...), push to a registry, and look up the digest
(`podman inspect --format '{{index .RepoDigests 0}}' <image>`, or `skopeo inspect`). Put `registry/name:tag@sha256:...` in `image`. The cluster pulls it.
Public registries work as they are; a private one needs pull credentials in the cluster (not set up by IYAKKI).

## 3. Fill in a pin for the shipped examples

The three examples in `image-apps/` (`busybox-httpd`, `python-http-server`, `whoami`) carry an all-zero placeholder digest and cannot run until
pinned. `dist/release/pin-image-apps.sh` looks up the current digests at Docker Hub and rewrites the files (needs the network, `curl`, and
`python3`; it is a release step run by the owner, **not run when this was written**). You can also do it by hand: find the digest as in route C and
edit the `image` field.

## 4. Check it

Press `r` on the Image apps tab: the file appears with its licence and *image pinned by digest*, or under "not listed" with the reason. **Run**
applies it; the state line shows `starting (the image is being pulled)` and then `running, open http://<this machine>:<port>`. **Stop** removes it. Every
run and stop is in the audit log (`imageapp.run`, `imageapp.stop`).

## 5. Planned, not built

* A container **builder** in the image (a permissively licensed one) so a Containerfile builds on the device; for now routes A to C.
* Running a `kind: "rootfs"` catalog entry (a root tarball with a checksum) straight from the page; it is parsed and shown today.
* Turning a sandbox into a cluster image (a layer tar, config and manifest for `k3s ctr images import`): the fallback runtime of
  [`LINUX-APPS.md`](LINUX-APPS.md) section 9 needs the same piece.
* A network policy for image-app pods (today they are limited by the pod settings above, not by a policy), and pull credentials for private registries.
* Signature checks (cosign) on pinned images.

## 6. What is tested

Catalog parsing, the refusal of a tag without a digest and of malformed references, the placeholder rule, the generated Namespace/Deployment/Service
(no host network, no host path, no privilege, capabilities dropped, read-only root with volumes, the digest in the image), the exact `kubectl`
arguments for run and stop, and the import of a tarball (including a hostile one) are in `crates/iyakki-agent/src/linuxapps_tests.rs`. A real cluster was not used.
