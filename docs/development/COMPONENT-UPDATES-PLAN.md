# Component updates and nightly builds: separate pieces, version diffs, our own store (plan)

Status: plan written 2026-11-07 from the owner's direction ("nightly build kernel, core, the shared layer that talks to the kernel, and apps separately, so a running OS gets the version diffs and updates; put them in our store on this machine's k3s with a PVC"). Part of it is built (the private store server, signed packages, system update packs); the rest is design. Related: [`COMPILE-SPEED-PLAN.md`](COMPILE-SPEED-PLAN.md) (the build loop), [`REMOTE-PATCH-DELTA.md`](REMOTE-PATCH-DELTA.md) (binary delta, being built), [`BUILD-PIPELINE-K3S.md`](BUILD-PIPELINE-K3S.md) (the pipeline namespace), [`../STORE.md`](../STORE.md) section 13 (private store), [`../UPDATES.md`](../UPDATES.md) (system update centre).

## Built so far (first slice, 2026-11-07; tested with fakes and loopback servers, no real store, no device)

* **Server** (`store/server_components.rs`, `store/compmanifest.rs`): streamed blobs `PUT`/`GET /v1/blobs/<sha256>` (Range, resumable upload, size cap per component, disk-space check, hash verified while streaming), `POST /v1/components`, signed `GET /v1/components.json`, withdrawal, and the retention cleanup (`POST /v1/components/retention`, report first, `apply=1` to remove). Routes and rules: `../STORE.md` section 13.
* **Publisher CLI** (`store/compcli.rs`): `iyakki store component publish|list|plan|yank|retention`.
* **Device** (`store/components.rs`): verify the signed list against the pinned key, plan (delta from the installed file when the store has one for exactly that file, else the whole file; `requires` checked against what the device will have after the update, the refusal names the part that must go first), resumable hash-checked download into a staging folder, delta applied with `bindelta::apply_delta` (a delta that does not fit falls back to the whole file), and hand-over as the inputs the existing machinery reads: an update payload folder (`update.json` + `boot/...`, what `dev update` and the Update centre read) for kernel/boot/base, and a patch pack (`IYPATCH1`) for core and the kernel layer. It stops at "verified and staged, here is the plan". Assistant tool `components_status` (read-only).
* **Not built:** applying a component to a running system; writing `components-installed.json` after an update (today nothing writes it, so a device reports "not on record" for every part); the Update centre page; the build job that produces artifacts, deltas and the boot-tested publish; channels' promotion command; the nightly CronJob; the kernel-layer split; HTTPS front; scheduled retention. The deployment in `dist/store-k3s/` was not changed (see COMPONENT-STORE-FINDINGS.md for the flags and the memory limit).

Order of work above: step 4 (manifest and kind) is done in this slice, as a separate streamed blob kind and a signed `components.json` rather than a `system-component` package kind (see the findings); step 5 (device side) is done up to the plan and the staged hand-over.

## What is separate, and why

Today a system update replaces the boot files as one unit (a 420 MB pack: kernel, small start RAM disk, the root image holding the OS and the 80 MB `iyakki` binary). Most changes touch one part. Parts, from slowest-changing to fastest:

| Component | What it is | Changes | Update cost today | Target |
|---|---|---|---|---|
| **kernel** | `bzImage` + modules + firmware set | weeks | in the 420 MB pack | its own file (about 15 MB) plus a version-matched module set; needs a restart |
| **boot** | stage1 / the lean RAM disk, GRUB config, loader | rare | in the pack | tiny, replaced with the strongest safeguards (previous version kept) |
| **base OS** | the root image except IYAKKI's binary: Busybox, libraries, the web engine, k3s tooling | weeks | in the pack | its own signed image; delta against the installed one; needs a restart |
| **core** | the `iyakki` binary: supervisor, services, shell | daily | 80 MB inside the root image | binary delta (a few MB); a service restart, not a machine restart |
| **kernel layer** | the code that talks to `/proc`, `/sys`, `/dev` (`iyakki-core-local`, devctl, hwinfo, storage, netscan) | with core, but must match the kernel | inside core | after the split it is its own unit with a declared kernel range (`requires_kernel: 6.12..6.13`); an update that would break the pairing is refused |
| **apps** | tile apps, skills, tools, extensions, WASM tiles | hourly | packages through the store (already) | unchanged mechanism; the nightly build publishes them as store packages |

The rule that keeps a running OS safe: **every component has a version and a compatibility range for the others** (`requires: {kernel: ">=6.12", core: ">=1107", abi: "tile-abi 1"}`). The device refuses an update whose range it cannot meet and says which part must go first.

## Nightly build (on this machine's k3s, with the build loop's caches)

1. A scheduled job (`CronJob` in `iyakki-build`, or started by hand) checks out the repository state, builds **only the components whose inputs changed** (hash of inputs per component: kernel config + patches; base overlay + package list; Cargo sources of the crates in core; app sources), and skips the rest. This is what makes it fast: nothing is rebuilt that has the same input hash as last night.
2. Each built component is published as an **artifact** with a manifest entry: `{component, version, sha256, size, requires, built_from, channel}` plus, for every previous version still supported (default: the last 3), a **delta artifact** (`from -> to`, sha256 of both ends) made with `bindelta` (the in-house delta; for the root image a block-level delta of the squashfs).
3. The manifest is **signed on the host** with the update signing key (an ed25519 key that is NOT the Secure Boot key and never enters the cluster); the signing step is the only manual or host-side step, and it is a small program (`iyakki dev update-pack --sign` exists for packs).
4. Boot test before publish: the QEMU boot-test job (pipeline) boots the assembled image from the new components; a failure stops the publish. A published nightly is therefore at least "boots in QEMU".
5. Channels: `nightly` (every build that passed), `beta` (promoted by hand), `stable` (promoted by hand after a device has run it). A promotion copies a manifest entry; it never rebuilds.

## Our store on this machine's k3s

* **What it is:** the existing `iyakki-store-server` (private store: signed catalog, immutable `/packages/<sha256>`, publishing with a token, yanking). It already serves apps. It is extended with **system component packages**: a new package kind `system-component` (kinds are an allow-list in `store/vet.rs`) whose payload is the artifact and whose store information carries the component, version, requires and the delta references. The trust model is unchanged (devices work out trust from the package and the pinned catalog key).
* **Where:** a new namespace `iyakki-store`, one Deployment (the static server, `FROM scratch`), a `PersistentVolumeClaim` (local-path, sized from the retention policy below) mounted at the data folder, a Service (NodePort for the LAN, ClusterIP inside), and a Secret holding **only** the catalog key (which signs the catalog; it can be rotated) and the publish token. The update-signing key and the Secure Boot key stay on the host.
* **Retention** (bounded by the PVC, not by hope): keep, per component, the newest N full artifacts (default 3) and the deltas between them; older ones are yanked and removed by a scheduled cleanup; a size report shows what would be removed first. Withdrawn versions are never served.
* **Access:** devices on the LAN reach the NodePort; an HTTPS front (the cluster ingress or the existing portal) is a later step, but **integrity does not depend on transport**: every manifest and package is signed and the device verifies it.

## What a running OS does (client side)

1. Check: the Update centre asks the store (the pinned one first) for the manifest of its channel (hourly, or on demand; no data leaves the device but the component versions it has).
2. Plan: for each component, if a delta from the installed version exists, take it; else the full artifact. Show the person what changes, sizes, whether a restart is needed (apps: no; core: a service restart; kernel, base, boot: a restart), and the previous version kept.
3. Apply in the order boot safeguards require (existing machinery of `dev update` and the Update centre: verify signatures and hashes, previous version kept in the boot menu, atomic swap). A core-only update uses the binary delta path (`dev patch` semantics), swaps the binary and restarts the supervisor's services; the machine does not reboot.
4. Verify after: the new version reports itself; if the core fails to come up (the existing unclean-start detection), the previous binary is restored automatically.
5. Tell the person in plain words what changed (one line per component) and let them go back.

## Order of work

1. **Binary delta** (in progress by an agent): the base for the core component.
2. **Pipeline** (in progress): the boot-test job and the build job skeleton.
3. **Deploy our store on k3s** (next, with an agent): namespace, PVC, Service, the existing server; publish a test app package and read it back from another machine on the LAN; add it to this PC's and the Lenovo's STORE as a store with its key pinned.
4. **Component manifest and the `system-component` kind** (format, vet rules, tests); the build job produces artifacts + deltas + manifest for one component first (core).
5. **Device side:** the Update centre reads component manifests; core-only update without reboot; the refusal rules (requires ranges).
6. **Nightly CronJob** with input hashing and the boot-test gate; channels and promotion.
7. **Kernel layer split** with a declared kernel range (needs Phase 2/3 of the compile plan).
8. HTTPS front and an automatic retention cleanup.

## What this does not solve (said plainly)

* A kernel or base-OS change still needs a restart and a bigger download; deltas help but the kernel image compresses poorly.
* A nightly that passes the QEMU boot test can still fail on real hardware (Wi-Fi, sound, touch); the channels exist for that reason: nightly to the test machines first, beta by promotion.
* The store is on one PC. If it is down, devices keep running and keep their versions; there is no automatic failover.
