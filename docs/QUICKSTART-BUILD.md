# Quick start: build, run, write to disk, deploy to the cloud

One page, the shortest path for each goal. Details live in the pages linked under each step. For a tour of what IYAKKI does once it runs, see [`QUICKSTART.md`](QUICKSTART.md).

## 1. Try it in a window (2 minutes after the first build)

```
./run.sh window          # the real shell in a desktop window; shows virtual disks only
./run.sh disk add 8G     # give the window a virtual disk to play with
```

First run: create the administrator account. Data lives in `~/.local/share/iyakki-dev`; reset with `rm -rf` on that folder. More: [`INSTALL.md`](INSTALL.md#1-window-mode-linux).

## 2. Build an image

| You want | Run | Notes |
|---|---|---|
| The standard ISO and install image | `./run.sh build` | first build 30-40 min and about 1 GB of downloads; later builds reuse the caches |
| An ordinary x86-64 PC (all GPU and common Wi-Fi firmware) | `./run.sh build-pc-layered` | vendor firmware that needs a licence is not in it; the person installs it from Store -> Setup |
| The big image with the full web engine | `./run.sh build-web` | hours, 30 GB disk, 16 GB RAM |
| An Intel Mac image | `./run.sh build-mac` or `build-macweb` | [`MAC-HARDWARE.md`](MAC-HARDWARE.md) |

Never delete `target/`: builds are incremental, and a change that touches only Rust code rebuilds only the changed crates. Faster loops for a one-app change (a device build profile, a cached repack, a delta push) are planned in [`development/COMPILE-SPEED-PLAN.md`](development/COMPILE-SPEED-PLAN.md).

## 3. Boot it in a virtual machine first (always, before a real device)

```
./run.sh vm-disk                       # boots a virtual disk like the stick (GRUB, data partition); --uefi for UEFI
dist/pipeline/run-boot-test.sh IMAGE.img 90     # the same unattended on this PC's k3s: pass or fail, screenshots, serial log
```

The unattended test needs `export KUBECONFIG=/etc/rancher/k3s/k3s.yaml` and passes only if the system reaches `supervisor up` with no panic; it keeps the screenshots for a human look ([`development/BUILD-PIPELINE-K3S.md`](development/BUILD-PIPELINE-K3S.md), [`BOOT-TEST.md`](BOOT-TEST.md)).

## 4. Write it to a disk

```
iyakki-usb --list                                                # find the stick; never the system disk
iyakki-usb --install --erase /dev/sdX iyakki-install-x86_64.img  # erases the stick; naming the drive is the confirmation
```

Boot the stick from the firmware boot menu (on a Mac hold **Option**). To install to the computer's own disk, choose **Install to a disk** in the boot menu ([`INSTALL.md`](INSTALL.md#3-install-to-a-computers-disk)). Secure Boot on: the PC image is signed; enrol the key (the boot screen explains) or turn Secure Boot off.

## 5. Update a machine that already runs IYAKKI (no stick)

On the machine: Settings -> Developer mode (a one-time pairing code, scopes, auto-off after 4 hours). On your PC:

```
iyakki dev pair <address-or-name.local> --code CODE --as NAME      # pair by the device's name (iyakki-XXXXXX.local) when the same install boots on several computers: it keeps working when the address changes
iyakki dev update-pack IMAGE.img -o DIR --version V --variant pc
iyakki dev --device NAME update DIR --force        # keeps "IYAKKI (previous version)" in the boot menu
iyakki dev --device NAME reboot                    # approve on the device's screen
```

Use an **optimised** `iyakki` for the `dev` commands (for example `target/x86_64-unknown-linux-musl/device/iyakki`), not `target/debug/iyakki`: the debug build hashes in unoptimised code and was measured CPU-bound at about 17 MB/s (100 MB in 5.8 s), against 100 MB in 1.7 s with the optimised build, on the same 2.5G link.

For a small change ship a **binary delta** instead of the whole pack: `iyakki dev patch build DIR -o PACK --delta-from OLD` then `patch apply` ([`development/REMOTE-PATCH-DELTA.md`](development/REMOTE-PATCH-DELTA.md)). Rule: a fix made live on a device must also land in the source and the next image.

## 6. Run it in the cloud

```
docker load -i iyakki-cloud-<v>-amd64.tar
docker run -d --name iyakki --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp:rw,size=64m,mode=1777 -v iyakki-data:/var/lib/iyakki -p 8443:8443 iyakki-cloud:<v>
docker logs iyakki        # the pairing code; open https://HOST:8443/ and type it
```

On Kubernetes apply `dist/cloud/k3s-cloud.yaml` (one tenant) or use `iyakki cloud tenants ...` for many. A cloud VM can also boot the cloud disk image or qcow2 with a config drive instead of the page ([`CLOUD-IMAGES.md`](CLOUD-IMAGES.md), [`CLOUD-MODE.md`](CLOUD-MODE.md)).

## 7. Apps and components from a store

* A private store runs on this PC's k3s (`iyakki-store` namespace, a persistent volume): add it in Store with its key pinned, publish apps with the store tools. Setup and commands: `dist/store-k3s/README.md` once that work lands ([`STORE.md`](STORE.md#13-private-store)).
* Planned: nightly builds of the kernel, core, the kernel layer and apps as separate signed components with deltas, so a running OS fetches only what changed ([`development/COMPONENT-UPDATES-PLAN.md`](development/COMPONENT-UPDATES-PLAN.md)).

## Rules that keep you out of trouble

1. Boot-test every new image in QEMU before it goes to a device. A PID 1 panic once reached a machine because this was skipped.
2. Never write the computer's system disk; `iyakki-usb` refuses it, and the window shows virtual disks only.
3. Keep the Secure Boot signing key out of the repository and out of containers.
4. Builds are memory-hungry: keep them under about 9 GB, and keep images and packs under your work folder, not `/tmp` (it is wiped on reboot).
5. Commit and push only when you mean to; the build never does.
6. One install can boot on several computers: sound, display, keyboard, microphone, GPU and power settings are kept per computer (Settings, System, This computer; [`HARDWARE.md`](HARDWARE.md#one-install-several-computers)).
