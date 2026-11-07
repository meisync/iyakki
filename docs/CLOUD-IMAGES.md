# Cloud images: container, ISO, disk image, qcow2

One script makes them all: `dist/cloud/build-cloud.sh` (`docker`, `iso`, `disk`, `qcow2`, `all`, `test`, `clean`). Output goes to
`~/iyakki-work/cloud/<version>/`, logs to `~/iyakki-work/logs/cloud-*.log`. It never pushes or logs in anywhere, never writes a drive,
never uses sudo, and takes the shared build lock and memory caps like the other build scripts. Read with `CLOUD-MODE.md`,
`development/CLOUD-MULTITENANT-PLAN.md`, `development/TENANT-CONFIG-PLAN.md`, `LICENSING.md`.

## What each artifact is

| Artifact | File | For | Made by |
|---|---|---|---|
| Container image | `iyakki-cloud-<v>-amd64.tar` (`docker load`; also `-arm64.tar` with `--arm64`) | Docker, Podman, K3s, any Kubernetes | `build-cloud.sh docker` |
| ISO | `iyakki-cloud-<v>-x86_64.iso` | virtual machines, optical media (BIOS and UEFI) | `build-cloud.sh iso` |
| Raw disk image | `iyakki-cloud-<v>-x86_64.img` | a USB stick for a real machine (GPT, EFI boot partition, boot volume) | same repack as the ISO |
| qcow2 | `iyakki-cloud-<v>-x86_64.qcow2` | KVM, Proxmox, OpenStack, cloud import | `build-cloud.sh qcow2` |
| SBOM and licences | `iyakki-sbom.cdx.json`, `THIRD_PARTY_LICENSES.txt`, `LICENSE`, `NOTICE`, `buildroot-legal-manifest.csv` | compliance | copied from the repository's own generated files |
| Checksums and signature | `SHA256SUMS`, `SHA256SUMS.sig`, `release.json`, `release.json.sig` | verification | `iyakki release sign` |

The ISO and the disk image come from one **repack** of the finished `web` and `mac` Buildroot builds with `VARIANT=cloud-x86_64` in
`/etc/iyakki-release` (the release profile: no vendor firmware, no codec libraries). `variant_is_cloud` turns **Cloud mode on by itself**
(see `CLOUD-MODE.md`). No Buildroot `make` runs; the cost is one zstd pass (minutes) and a cargo build of the program. A distinct
Buildroot configuration for a cloud variant (for example without the desktop's GUI libraries) would be a full build and is not started by the
script; today the cloud image is the `web` image with Cloud mode on.

## The container image

`FROM scratch`: the static musl program `/iyakki`, the CA bundle, `/licenses`, two examples in `/etc/iyakki`. No shell, no package manager.
Non-root (65532), made for `--read-only` (the only writable places are the volume `/var/lib/iyakki` and `/tmp`), `HEALTHCHECK` is the
program itself (`iyakki healthcheck`, a `GET /healthz` on the local listener, no curl; `IYAKKI_HEALTH_URL` for another port). Labels follow
the OCI annotations (`version`, `revision` = `git describe`, `licenses=Apache-2.0`, `source`, `created`). No base image is pulled.

Run it:

```
docker load -i iyakki-cloud-<v>-amd64.tar
docker run -d --name iyakki --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 128 --memory 512m \
  --tmpfs /tmp:rw,size=64m,mode=1777 -v iyakki-data:/var/lib/iyakki -p 8443:8443 iyakki-cloud:<v>
docker logs iyakki          # with no account yet: the pairing code ("IYAKKI-ADOPT ... type the code K7QD-4MXA")
```

Open `https://HOST:8443/`, type the code, make the administrator. Or make the first account without the page:
`echo 'a long password' | docker run -i --rm -v iyakki-data:/var/lib/iyakki iyakki-cloud:<v> user add admin --role admin --password-stdin`
and start with `-e IYAKKI_ADOPT=0`.

| Command (after the image name) | What |
|---|---|
| `serve` (default) | the desktop in a browser on 8443 (`IYAKKI_WEB=1`); adopt mode while no account exists (`IYAKKI_ADOPT=1` in the image; `0` to switch off) |
| `serve --profile cloud --config /etc/iyakki/service.json` | the headless web service of the cloud profile (HTTPS, allow-listed routes); example `service.cloud.example.json` |
| `cloud listen --routes ROUTES.json ...` | the router for many tenants (`iyakki cloud listen --help`) |
| `cloud tenants|manifests|deploy|reconcile|snapshot|common|cert|roles ...` | the control plane; run where `kubectl` is |
| `user add|list ...`, `healthcheck`, `version` | accounts, probe, version |

Environment: `IYAKKI_WEB`, `IYAKKI_ADOPT`, `IYAKKI_LOG`, `IYAKKI_TENANT_DATA_DIR` (the tenant's data folder, where `cloud/config/tenant-config.json` is
read), `IYAKKI_BROWSER_MEDIA` (`allow` or `deny`: the browser's camera and microphone for the tenant), `IYAKKI_HEALTH_URL`, `IYAKKI_USERS`.
Certificates: put `cert.pem` and `key.pem` in `/var/lib/iyakki/tls` (a mounted Secret), else a self-signed pair is made there once.

Architectures: `linux/amd64` always. `--arm64` builds `aarch64-unknown-linux-musl` with the cross compiler of the aarch64 Buildroot
toolchain (no emulation, the same way `build.sh` does it) and builds the image with `--platform linux/arm64` (a `FROM scratch` COPY image needs no
emulation). The two images have separate tags (`-amd64`, `-arm64`); a combined manifest list needs a registry and is left to the owner.

## Kubernetes

`dist/cloud/k3s-cloud.yaml` is one tenant: namespace, **ConfigMap `iyakki-tenant-config`** (the operator's declarative `tenant-config.json`,
mounted read-only at `/var/lib/iyakki/cloud/config/tenant-config.json`; read on every use, so an edit takes effect without a restart), a
**Secret `iyakki-admin`** (first account), an optional **Secret `iyakki-tls`** (`cert.pem`, `key.pem`), an optional **Secret `iyakki-common`**
(the operator's common vault, `cvault:NAME`), a PVC, a hardened Deployment (non-root, read-only root, no capabilities, exec probes with
`iyakki healthcheck`) and a Service. Secrets are never in the file: the header lists the `kubectl create secret` commands. Config holds
only references (`vault:NS/NAME`); the vault keeps the values. For many tenants use the control plane: `iyakki cloud tenants add ...`,
`iyakki cloud manifests --config tenant.json`, `iyakki cloud reconcile --dir DIR --apply` (see `development/CLOUD-MULTITENANT-PLAN.md`).
`build-cloud.sh test docker` checks the manifests with `kubectl apply --dry-run=client` only; the local cluster is not touched.

## The ISO, the disk image and the config drive

The ISO is for VMs and optical media. **Real machines, and Apple Macs in particular, boot the raw disk image from a stick**, not the
ISO written to a stick: Apple's EFI does not take a hybrid ISO reliably. Write the disk image with the existing tool (you run it; the script never does):

```
iyakki-usb --list
iyakki-usb --install --erase /dev/sdX iyakki-cloud-<v>-x86_64.img      # replace sdX; naming the drive is the confirmation
```

On a Mac mini of 2014 (Macmini7,1) hold **Option** at the chime, pick the stick ("EFI Boot"), release.

**First boot, three ways (like cloud-init's NoCloud):**

1. **Nothing configured, no screen.** Cloud mode is on (the image is a cloud variant, or the machine has no display, keyboard or sound). With no
   account the node starts **adopt mode**: the console shows `IYAKKI-ADOPT node iyakki-<id>: open https://iyakki-<id>.local:8443/ and type
   the code K7QD-4MXA`, and the same code is in the mDNS TXT record (`adopt=`). Open the page, type the code, choose the administrator's
   name and password. Limits: the code lives 15 minutes (restart for a new one); 5 wrong codes lock the page for a minute, 20 burn the code;
   only local networks are answered (loopback, private, link-local, unique-local); it works only while no account exists. The page never
   shows the code. **The TXT record is readable by anyone on the same network segment**, so the code keeps out the wider internet and late
   visitors but not a hostile machine on the same segment during the 15 minutes; that is the design asked for.
2. **A config drive.** A volume labelled `IYAKKI-CONFIG` (ISO 9660 or ext4; for FAT use `IYAKKI-CFG`, since FAT labels hold 11 characters), or a folder
   `iyakki-config/` on the data partition. Files: `iyakki-cloud.cfg` (`hostname`, `admin.name`, `admin.hash` as an Argon2id string made by
   `iyakki user add` on any machine, `net.dns`, `net.static.eth0 = 192.168.1.50/24 gw 192.168.1.1`), `cert.pem`+`key.pem`, `tenants.json`, `tenant-config.json`.
   Read at boot on a cloud node only; bad input is refused with reasons on the console, never half-applied. An existing account file, certificate and
   `tenants.json` are kept; `tenant-config.json` is refreshed at each boot (it is the operator's file). DHCP is the default; no file, no change.
   Make one: `xorriso -as mkisofs -V IYAKKI-CONFIG -o config.iso DIR` and attach it as a second disk or USB stick.
3. **Self-install to the internal disk** (destructive, off by default): add **both** `iyakki.cloud.install=sda` and
   `iyakki.cloud.confirm=ERASE-sda` to the kernel line; the existing installer (`iyakki install --disk sda --confirm sda --yes`) does the work, then
   the machine powers off; **remove the stick** before starting it. A marker on the stick's data partition (`cloud-install-done`) stops a second run.
   One parameter alone does nothing; a mismatch is refused.

## Upgrade and rollback

Container: load the new tar, change the image tag; the volume keeps the data (the program migrates its stores and keeps `.pre-v*` copies). Roll
back by setting the old tag back; the old image stays loaded until you remove it. Kubernetes: `kubectl set image` and `kubectl rollout undo`
(the strategy is `Recreate`, one writer). Disk: write the new image to a new stick and boot it; the data partition is the stick's own, so for
an installed machine use the in-OS update. Always keep the previous `.tar` and `SHA256SUMS`; verify with `iyakki release verify DIR --pubkey HEX`.

## Sizes (build of 2026-11-04, version 0.1.0-alpha.1)

| Artifact | Size |
|---|---|
| Static program `iyakki` (musl, LTO, stripped) | 77.5 MB (74 MiB) |
| Container image (store size, compressed layers) / `docker save` tarball | 35.5 MB / 35.5 MB; about 77 MB of program plus 0.2 MB CA bundle and 0.8 MB licences unpacked |
| Build context sent to `docker build` | 75 MB, 11 files (a prepared folder, never the repository) |
| ISO | 242 MB |
| Raw disk image (install template, for `iyakki-usb --install`) | 270.5 MB (258 MiB); the stick's data partition grows to fill the stick at install |
| qcow2 of that image (compressed) | 240 MB, virtual size 258 MiB; a VM needs a second, larger disk for data or the installer |

## Licences and SBOM

The image holds one program plus the CA bundle (Mozilla's list, MPL-2.0 data). `/licenses` has `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`,
the notices index and `sbom.cdx.json` (CycloneDX 1.5, the repository's own generated file; the release gate fails when it is stale). That SBOM covers
the whole workspace's lock files, a superset of what one binary links; there is no per-binary SBOM yet. No third-party SBOM tool is used.
No GPL or LGPL program is in the container. The ISO and disk image carry Buildroot's GPL/LGPL parts: `buildroot-legal-manifest.csv` is
copied beside them; whoever redistributes an image must also provide the corresponding source from `legal-info/` (`LICENSING.md`).

## Reproducibility and signing

Cargo is built with the repository's release profile and `--locked`. The same sources at the same path give the same binary hash (kept
in `binary.sha256`; `dev.iyakki.binary.sha256` is a label of the image). The **image digest is not reproducible**: the classic builder stamps layer
times. `iyakki release sign` writes `SHA256SUMS` and Ed25519 signatures with a key kept outside the repository; the script makes a local development
key (`release-dev.key` in the output base) if none is named by `IYAKKI_RELEASE_KEY`; use the real release key for a release.

## Mac mini 2014 (Macmini7,1): the named acceptance target

Sure (from the kernel configuration of this repository): the kernel has `tg3` (module, Broadcom Gigabit), AHCI/PIIX SATA, NVMe, USB storage, XHCI/EHCI,
EFI stub and EFI framebuffer; the tg3 firmware names are in the restricted-firmware table (fetched by the person, never shipped); Wi-Fi is a
background item that never holds the boot. Not sure and to be confirmed on the machine: whether the BCM57766 NIC needs `tigon/tg357766.bin` for
a link on this kernel (without it the driver may run with reduced features); whether Apple's EFI shows the stick as "EFI Boot" for this image's
ESP; whether the Fusion drive or the PCIe SSD of this model is seen by the installer as one disk; the BCM4360 Wi-Fi needs a firmware we do not ship,
so the node is Ethernet-only; Thunderbolt 2 is not used. `build-cloud.sh test hw` checks the first group by configuration; nothing here was run on the machine.

## Known limits

* Adopt mode's timer is a monotonic clock started at listen time; the TXT record leaks the code to the local segment (above).
* A cloud node starts the browser front end by default (no `iyakki.web=1` needed); a desktop keeps its old rule.
* The unit tests of `cloudadopt.rs`, `cloudseed.rs` and `healthcheck.rs` were run from a scratch crate (`~/iyakki-work/cloud/scratch-tests`, which `#[path]`-includes the three files) because the shared `iyakki-agent` test build was broken by another change in progress at that time; run `cargo test -p iyakki-agent --lib -- cloudadopt cloudseed healthcheck` once the tree builds.
* mDNS announcing is **off by default in Cloud mode** (`devname::effective`); adopt mode announces regardless, until the first account exists. After that
  the node is found by its address unless an administrator turns the announcement on in Settings.
* The pairing page is its own small listener on 8443 until the account exists; then the normal front end starts on the same port.
* The firewall: a node's firewall may need TCP 8443 and UDP 5353 opened from the local network (not verified for every image).
* The config drive's FAT label limit (11) forced the second label `IYAKKI-CFG`.
* No multi-arch manifest list, no push, no signing of the container image itself (the tarball is signed).
* The ISO test boots a VM with a virtual display; the "no display" automatic decision is only exercised through the variant name.
