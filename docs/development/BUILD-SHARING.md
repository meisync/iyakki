# Sharing builds with the team (shared build store)

Status: built 2026-11-15; the objects exist on this PC's k3s but no pod could stay running there (see "What was proven"). Owner decision of the same day: "push layers/builds to a giveaway by machine type so team mates can share builds; host it on this PC's k3s for now"; later the same day: keep ONLY THE LATEST build per machine type and channel). Related: [`COMPONENT-UPDATES-PLAN.md`](COMPONENT-UPDATES-PLAN.md) (the store server and its blobs), [`LAYERED-ROOT-DESIGN.md`](LAYERED-ROOT-DESIGN.md) (format 2 packs), `docs/DEVELOPER-MODE.md` section 7 (installing a pack on a device).

## What it is

A teammate who does not build IYAKKI can fetch the newest tested build for the kind of machine they have and install it on their own device. The publisher (the person whose PC built and boot-tested the pack) uploads the update pack folder (`update.json` plus boot files, what `nightly-push.sh` / `quick-push.sh` produce) to a store; the store keeps **only the newest build per machine type and channel**.

* **Machine type** (the image `variant`): the kind of hardware a build boots on.
* **Channel**: `dev` (every build that passed the boot test) today; `beta` later.
* **Build id**: a name the publisher gives (for example `alpha4b`); the commit and version are recorded with it.

### Machine types

| Machine type | Hardware that runs it | Status |
|---|---|---|
| `pc-x86_64` | Lenovo, HP, both Mac minis (Intel; Secure Boot signed image), the MacBook Pro 14,1 | in use (today's one image) |
| `pc-aarch64` | ARM PCs and servers | later, no image yet |
| `mac-arm64` | Apple Silicon (VM) | later, no image yet |

The machine type is a label the publisher chooses; the pack's own `update.json` says `variant: "pc"`, which is what `dev update` checks on the device (the list records both). A new machine type needs no server change: publish with a new `--variant`.

## How it is built (and why it is not the component list)

* The store is the existing `iyakki-store-server` (static musl) in the namespace `iyakki-store` of this PC's k3s: [`dist/k8s/iyakki-store/`](../../dist/k8s/iyakki-store/). A pack's files are plain blobs addressed by their SHA-256 (`PUT/GET /v1/blobs/<sha256>`, streamed, `Range`, resumable upload), as for components. A file shared by two builds (the kernel when only the root image changed) is stored once.
* The list of builds is **`/v1/builds.json`** (`store/buildshare.rs`): builds with machine type, channel, build id, version, commit, publish time, sizes, and the file list (path, size, SHA-256), plus a `latest` pointer per machine type and channel. It is **signed offline by the update key** with the same document signing as the component list (`trust::sign_doc`) but its own purpose string `builds`, so a component list can never pass for a build list.
* Why not `components.json`: that list is one blob per component and is signed by the key held in the cluster (the catalog key). A pack is a folder of 35 files, and the owner's rule is that the signing key for shared builds is the update key, which never leaves the publisher's disk. The store only checks the list (signature, the pinned public key `--builds-key`, every blob present with the right size, a serial that only goes up); it cannot make one.
* **Only the newest build is kept** (`--keep N`, default 1, per machine type and channel). When a new build replaces an old one the blobs only the old build used are removed, in this order, so a fetch during a publish keeps working: (1) the new build's blobs are uploaded (the old build is untouched); (2) the new signed list is sent (the store refuses it unless every blob is there); (3) the publisher reads the list and every blob back; (4) only then `POST /v1/builds/purge` removes blobs that only earlier lists named. Blobs the new build reuses stay. A device that started downloading the old build before step 2 can finish until step 4; after that it gets a 404 and the fetch script re-reads the list and starts the new build (it says so).
* Transport is plain HTTP on the LAN (the server has no TLS; the dev-mode pairing certificate belongs to a different server). Everything that matters is protected without it: the list by the update key's signature, every file by its SHA-256 in that signed list. What plain HTTP does not protect: the read token travels in clear on the LAN (a stranger on the LAN could read builds; they could not alter them), and the publish token too (do publishing from the PC itself or a trusted wire; the script's default store address is the PC).
* Tokens: reads need the **read token** (one string for all teammates; the publish token also reads); writes need the **publish token**. Both are random 64 hex characters in files outside the repository, held in the cluster as the Secret `store-secrets`. The **update signing key** is never in the cluster, the repository, a log or a command line.

## For a teammate: fetch and install

You need four things, none of them in a chat window in clear if you can avoid it:

1. **The store address**, for example `http://STORE-HOST:30580` (this PC's address is DHCP: it can change; ask the publisher or use the PC's host name where your LAN resolves it).
2. **The read token**: a file with one line. Ask the publisher; save it as `~/.config/iyakki/build-store-read.token` with mode 600 (`chmod 600`).
3. **The update public key**: it is in the repository, `dist/release/update-signing.pub` (64 hex characters). The fetch script uses it by default; compare it with what the publisher says if you do not trust the copy of the repository you have.
4. **The tool `iyakki-buildshare`** (verifies signatures and checks the folder): `cargo build --release -p iyakki-store-server --bin iyakki-buildshare` (set `IYAKKI_BUILDSHARE` to the binary or put it on `PATH`; the scripts also look in `target/`).

Then:

```sh
export IYAKKI_BUILD_STORE=http://STORE-HOST:30580
dist/release/fetch-build.sh --list                      # what the store holds
dist/release/fetch-build.sh --variant pc-x86_64 --channel dev --out ~/iyakki-builds/dev     # latest
dist/release/fetch-build.sh --variant pc-x86_64 --channel dev --build alpha4b --out DIR     # a named build, if still there
```

The script reads the signed list (refused unless the update key signed it), downloads each file (resumable: run the same command again after a cut; a finished folder is not fetched twice), checks size and SHA-256 of each file against the signed list, then runs the device's own pack check on the folder. It never touches a device. It ends by printing the exact commands:

```sh
iyakki dev update ~/iyakki-builds/dev --device NAME     # stages it on the paired device (docs/DEVELOPER-MODE.md section 7)
iyakki dev reboot --device NAME
```

Use the machine type that matches your device (table above). Installing a `pc-x86_64` build on anything else is refused by the device's checks, but do not try.

## For the publisher: publish

One-time setup (this PC): the update key `~/.config/iyakki/update-signing.key` (mode 600; made with `iyakki-buildshare keygen --out FILE`, which prints the public key to commit as `dist/release/update-signing.pub`), the publish token `~/.config/iyakki/build-store-publish.token` (mode 600), and the cluster (next section).

```sh
dist/release/publish-build.sh ~/iyakki-work/packs/alpha4b --variant pc-x86_64 --channel dev --build-id alpha4b --store http://STORE-HOST:30580
# options: --keep N (default 1)  --commit SHA (default: HEAD)  --key FILE  --token-file FILE  --dry-run  --no-purge
```

It verifies the pack (`update.json` and every listed file by size and SHA-256), reads the store's list, adds the build and signs the list here, uploads missing blobs in 32 MiB pieces (an interrupted upload continues from where the store's part file ends), sends the list, reads everything back, then purges. `--dry-run` stops after signing and uploads nothing. `--no-purge` leaves the old build's blobs until you call the purge yourself (the script prints the command). `nightly-push.sh --publish` prints this command with the right pack folder after the boot test passed; it never runs it (nightly-push keeps its rule of no network step).

Publish a build that has passed the boot test. A teammate will install it on real hardware.

## Quick how-to for a second person (publish and fetch with the signed list)

Nothing secret is in the repository. Tokens and the update key are files you keep outside it (mode 600, folder mode 700) and the scripts take their paths, never their contents, on the command line.

**Fetch (anyone on the LAN):**

1. Build the checker once: `cargo build --release -p iyakki-store-server --bin iyakki-buildshare`.
2. Save the read token you were given (a file with one line) as `~/.config/iyakki/build-store-read.token`, then `chmod 600` it. Or pass `--token-file PATH`.
3. `export IYAKKI_BUILD_STORE=http://STORE-HOST:30580`, then `dist/release/fetch-build.sh --list`, then `dist/release/fetch-build.sh --variant pc-x86_64 --channel dev --out DIR`.
4. The script refuses a list the update key did not sign (the public key is `dist/release/update-signing.pub`; compare it with the publisher's) and any file whose size or SHA-256 differs. Install with the two `iyakki dev` lines it prints.

**Publish (the person who built and boot-tested the pack):**

1. Hold the update signing key (`iyakki-buildshare keygen --out FILE`, mode 600) and the publish token file. Two publishers must share the SAME update key, or teammates see "not signed by the update key"; hand the key over by a trusted path, never through chat or the repository.
2. `dist/release/publish-build.sh PACKDIR --variant pc-x86_64 --channel dev --build-id ID --store http://STORE-HOST:30580 --key FILE --token-file FILE`. Try `--dry-run` first (signs, uploads nothing).
3. Only the newest build per machine type and channel stays. Two people publishing the same machine type and channel replace each other's build; use different `--channel` values for parallel work.
4. Do not publish a pack that has not passed the QEMU boot test.

## Scaling the store up once the disk has room (k3s)

The manifests under `dist/k8s/iyakki-store/` are ready. The kubelet refuses to run pods while this PC's disk is above its eviction threshold (the node then carries the `disk-pressure` taint and evicts pods within seconds). Do not delete other people's files to get under it. When there is room (check with `df -h /`, and `kubectl describe node | grep -i -A3 taints` shows no `disk-pressure`):

```sh
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
kubectl -n iyakki-store get pvc,pods                         # claim Bound? if Pending see the static volume line in "Deploying"
kubectl -n iyakki-store scale deploy/iyakki-store --replicas=1
kubectl -n iyakki-store rollout status deploy/iyakki-store --timeout=120s
curl -s http://STORE-HOST:30580/healthz
dist/release/publish-build.sh PACKDIR --variant pc-x86_64 --channel dev --build-id test1 --store http://STORE-HOST:30580   # then fetch it from a second machine
```

To stop it again: `kubectl -n iyakki-store scale deploy/iyakki-store --replicas=0` (the claim and its files stay). F-BSHARE-04 stays partial until a pod has run, answered `healthz` and served a publish and a fetch from a second machine.

## Deploying (once, on this PC)

All objects are under `dist/k8s/iyakki-store/` and live only in the namespace `iyakki-store`; nothing in the other namespaces of the cluster is read or changed. The PVC is 10 GiB (`local-path`, which does not enforce the size, so the server has its own cap, `--store-cap-mib 8192`); the quota allows 10 GiB, 1 GiB memory in limits, 2 CPUs. One 0.42 GB build per machine type and channel is kept, so 10 GiB holds many machine types.

```sh
cd <repository>
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml; kubectl get nodes            # the one node must be listed and Ready
# 1. the static binary and an image FROM scratch
cargo build --release --target x86_64-unknown-linux-musl -p iyakki-store-server
B=$HOME/iyakki-work/build-store/image; mkdir -p $B/data; chmod 777 $B/data
cp target/x86_64-unknown-linux-musl/release/iyakki-store-server $B/iyakki-store-server
cat > $B/Dockerfile <<'EOF'
FROM scratch
COPY iyakki-store-server /iyakki-store-server
COPY --chown=10001:10001 data /data
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/iyakki-store-server"]
EOF
(cd $B && DOCKER_BUILDKIT=0 docker build -t localhost:5000/iyakki-store-server:bs-2 .)     # FROM scratch pulls nothing
# 2. into k3s's containerd, no registry (needs root: sudo k3s ctr)
docker save localhost:5000/iyakki-store-server:bs-2 | sudo k3s ctr -n k8s.io images import -
# 3. secrets: files outside the repository (mode 700 folder, 600 files)
S=$HOME/iyakki-work/build-store/secrets; mkdir -p $S; chmod 700 $S; umask 077
for f in publish.token read.token catalog-seed; do [ -f $S/$f ] || openssl rand -hex 32 > $S/$f; done
kubectl apply -f dist/k8s/iyakki-store/00-namespace.yaml
kubectl -n iyakki-store create secret generic store-secrets --from-file=publish-token=$S/publish.token --from-file=read-token=$S/read.token --from-file=catalog-seed=$S/catalog-seed
# 4. the rest
# if the claim stays Pending (the local-path helper pod cannot start, for example under disk-pressure): sudo mkdir -p /var/lib/iyakki-store && sudo chown 10001:10001 /var/lib/iyakki-store && kubectl apply -f dist/k8s/iyakki-store/12-pv-static.yaml
kubectl apply -f dist/k8s/iyakki-store/10-pvc.yaml -f dist/k8s/iyakki-store/30-deployment.yaml -f dist/k8s/iyakki-store/40-service.yaml
kubectl -n iyakki-store rollout status deploy/iyakki-store
curl -s http://<this PC>:30580/healthz
```

The Service is a NodePort, fixed at **30580**, reachable on every address of the PC. The address is DHCP and can change. Scripts take the URL as a parameter or `IYAKKI_BUILD_STORE`. To update the server: rebuild, `docker build` with a new tag, import, change the tag in `30-deployment.yaml`, apply.

Look at what the store holds: `curl -s -X POST -H "Authorization: Bearer $(cat $S/publish.token)" 'http://HOST:30580/v1/builds/gc'` is a dry run that reports blobs, bytes, unreferenced blobs and the unfinished uploads; add `?apply=1&grace=SECONDS` to remove unreferenced blobs older than `grace` (default a day, which protects an upload that is waiting for its list).

## Retention and disk

* Default **keep 1** per machine type and channel (`--keep N` when publishing, 1 to 50). Dropped builds' own blobs go at the purge; blobs a kept build uses stay.
* The server refuses an upload that would take the blobs plus unfinished uploads over `--store-cap-mib` (507, the message says so). Unfinished uploads older than 7 days and unreferenced blobs are removed by `POST /v1/components/retention` (the component cleanup knows the build blobs and never removes them) or the build gc above.
* A layered pack (format 2) carries only the changed layers; it is stored like any other, so it is smaller. A teammate whose device holds an older base needs the matching base: publish full packs for teammates until a base-aware flow exists (see "Not done").

## Troubleshooting

| You see | Cause and fix |
|---|---|
| `the store refused the read token` | The token file is wrong or has been rotated. Ask the publisher. |
| `the build list is not signed by the update key ... refused` | The list was changed, or was signed by another key, or your `update-signing.pub` is not the publisher's. Do not install anything. Compare the key with the publisher. |
| `a downloaded file does not match the signed list: refused` | A stored or transferred file is damaged or was changed. Nothing was kept. Tell the publisher (the store's copy may be damaged: re-publish). |
| `a file is gone: the build was replaced while fetching` | A newer build was published while you downloaded. The script re-reads the list and continues with the new latest. |
| `no such build ... (try --list)` | With keep 1 an older build id is gone. Use `latest`. |
| `the store answered 000`/connection refused | The PC's address changed (DHCP) or the pod is down. `kubectl -n iyakki-store get pods` on the PC; use the new address. |
| `the store is full` (507) | The cap was reached: lower `--keep`, or the administrator runs the gc above. |
| The pod stays Pending / pods cannot reach the API | The node address changed since k3s started (seen 2026-11-07: `dial tcp 10.43.0.1:443: no route to host`). That is the cluster's own problem, not the store's. |

## Not done

* HTTPS: the store serves it itself (`--tls-cert FILE --tls-key FILE`, or `--tls-self-signed-dir DIR` which keeps a self-signed certificate and prints its SHA-256 fingerprint at start; `--tls-name a,b` adds names). Without a flag it is plain HTTP as before. Per-teammate READ tokens exist (2026-11-16): `--read-tokens-file FILE` with one `name token` per line (tokens at least 24 characters, names unique); each read of the build list is audited under the person's name and a person is revoked by removing their line and restarting the store. They work beside the shared read token. Tested in process (`store::builds_tests`). Not done: an HTTPS front, and per-person tokens for blobs in the audit log (only the list read is named).
* The device reads this store itself: the chat tools `builds_fetch` (download only) and `builds_install` (download, check, install through the device's own update path, no restart; asks every time). A layered build is chosen only when the device holds its base layers (the `needs` list in the signed build list); otherwise the newest full build is used, so publish with `--keep 2` including a full pack. `iyakki-buildshare select --held FILE` does the same choice on a PC.
* Channel promotion: `dist/release/promote-build.sh --variant V --from dev --to beta [--build ID|latest]` makes a build of one channel the newest of another with the same files (nothing is uploaded; the list is signed here with the update key). Tested by `store::buildshare::tests` and `dist/release/test-build-share.sh`.
* No aarch64 or Apple Silicon image exists yet.
* Nothing was installed on a device from the store.

## Chat tools

`builds_status` (read-only: the newest build per machine type, from the signed list), `builds_store_set` (asks: store address, update public key, optional read-token FILE path; a token is never typed into chat) and `builds_fetch` (asks every time, never a session grant: the card names the build, size and folder; downloads into `<data>/local/builds/<machine type>-<channel>`, every file checked against the signed list; installs nothing and prints the `iyakki dev update` line). Administrators only. Publishing is deliberately NOT a chat tool: it needs the update signing key, which the assistant never touches; `nightly-push.sh --publish` prints the command.

## What was proven (2026-11-15)

* Real k3s (this PC, `kubectl` against the real API server): the namespace, quota, limit range, claim, deployment and service are accepted (`--dry-run=server`); the image was imported into k3s's containerd; the 10 GiB claim bound to the static volume; the Service holds NodePort 30580. NOT proven: a pod running on k3s. The node had the `disk-pressure` taint (this PC's disk was 97% full, the kubelet limit is 10% free) and the kubelet evicted every store pod within seconds; the local-path provisioner's helper pod (kube-system, not ours) could not be scheduled either, hence `12-pv-static.yaml`. The deployment is left at 0 replicas until the disk has room (then `kubectl -n iyakki-store scale deploy/iyakki-store --replicas=1`).
* Real container, real image, outside k3s: the same `FROM scratch` image run by Docker with the deployment's arguments, user 10001, read-only root, no capabilities, the real tokens and the real update key: publish of a real 0.42 GB pack, a second publish that dropped the first build's six unique blobs (store 423 MB afterwards: keep only the latest), list, fetch of the latest into a new folder with signature, size, SHA-256 and `update.json` checks, a read without token refused (401).
* Local store on 127.0.0.1, 32 checks (`dist/release/test-build-share.sh`): resume, tamper, wrong key or token, keep 1 and keep N per machine type and channel.
* In-process unit tests: `store::builds_tests` (7), `store::buildshare::tests` (5), `buildtools_tests` (4, fake backend and fake streamer).
* Not done: a teammate on another machine, a real install on a device, TLS.
