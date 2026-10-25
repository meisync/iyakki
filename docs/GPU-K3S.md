# GPU for local models in k3s (alpha.2: F-GPU-01, F-GPU-03)

Local models (Lemonade, Ollama) run in IYAKKI's own k3s cluster. This page says how a pod gets the machine's GPU, what the **PC image**
is, what Quick install does for you, and, in section 8, the numbered **checklist for the RTX 4090 PC** with what to send back.

**Status (2026-10-20).** Stage 1 (Vulkan, any GPU) is built and was tried end to end on a real k3s v1.37.1 with an AMD Radeon 780M (not on
NVIDIA). Stage 3 (NVIDIA vendor driver) is built and tested with fakes; the open modules compile against the image kernel; **nothing of
stage 3 has run on a real NVIDIA card yet**: that is what section 8 is for. AMD ROCm (F-GPU-02) is stage 2 and not part of this change
except the kernel option (section 3).

Which machines are offered the GPU runtime at all (a GPU with at least 6 GB usable for models, or a remote cluster that has one) is decided by the AI hardware profile: [AI-HARDWARE.md](AI-HARDWARE.md).

## 1. The two stages

| | Stage 1: "GPU for local models (Vulkan)" | Stage 3: "NVIDIA GPU driver (CUDA)" |
|---|---|---|
| Works on | AMD (amdgpu, RADV), Intel (i915 or xe, ANV), NVIDIA Turing and newer through nouveau (NVK) | NVIDIA Turing and newer (GTX 16 series, RTX 20, 30, 40, 50) |
| Driver on the host | the kernel's own (in every image) | NVIDIA's **open** kernel modules (this PC image, built with `IYAKKI_NVIDIA_OPEN=1`) |
| User-space | Mesa **inside the pod image** (llama.cpp Vulkan image, Mesa 26) | NVIDIA's libraries and `nvidia-smi`, **downloaded for you** from NVIDIA, never in an image |
| How the pod asks | `resources.limits: iyakki.dev/gpu: 1` | `resources.limits: nvidia.com/gpu: 1` |
| What hands it out | a generic device plugin (squat/generic-device-plugin, Apache-2.0) that advertises `/dev/dri/renderD*` | NVIDIA's k8s device plugin (Apache-2.0) with CDI |
| Pod privileges | none (unprivileged pod; the device and its cgroup permission come from the plugin) | none (CDI adds the nodes, libraries and environment) |
| Restarts the cluster | no | once (containerd CDI drop-in) |
| Speed | NVK is slower than the vendor driver; RADV and ANV are good | the vendor's CUDA |

Both entries are in **Store → Setup** (the Hardware group; the IYAKKI menu's Quick install opens it), run as **detached jobs** (the installer indicator beside the clock
counts them, the log is in "Installer runs") and need k3s (installed first when missing, like Lemonade). The NVIDIA entry is **shown only
when an NVIDIA GPU is found**. A third button, **GPU details** (key `g`) on those rows, opens the **GPU page**.

### The GPU page

Store → Setup, a GPU card, **Open its page**: the graphics devices found (PCI vendor and device ids read from
`/sys/bus/pci/devices` and named from a small table in `crates/iyakki-agent/src/gpu.rs`; family, bound driver, render node, video memory),
whether Vulkan can work on each and why not, the driver state (nouveau/amdgpu/nvidia loaded, the open modules present in this image, firmware
the kernel could not load, the matching kernel log lines), the pods that use a GPU (`kubectl get pods -A` filtered for the resources above),
the install state of both stages, and the **Run test prompt** button (key `t`): a detached job that sends one chat request to the local model
server (Lemonade on 13305, or the GPU Ollama on 11435) and prints **tokens per second**; the last result stays on the page.

## 2. How it works

### Stage 1 (Vulkan)

1. Detection is read-only sysfs. A card is *usable* when a render node exists and the driver is amdgpu, i915/xe, or nouveau on Turing+.
2. The render nodes are made world-openable (mode 0666, as on most systems; devtmpfs makes them again at every start and IYAKKI repeats it).
3. A DaemonSet `iyakki-gpu-device-plugin` in `kube-system` (digest-pinned image `ghcr.io/squat/generic-device-plugin`, pulled by the cluster now)
   advertises each render node as `iyakki.dev/gpu` (8 pods may share one GPU). The node shows `iyakki.dev/gpu: 8`.
4. A smoke pod `gpu-smoke-vulkan` (llama.cpp `server-vulkan` image) asks for one and runs `llama-server --list-devices`; the step passes when the
   pod names a non-software Vulkan device (`Vulkan0: AMD Radeon 780M Graphics (RADV PHOENIX)`).
5. If Lemonade is deployed, it is patched: `iyakki.dev/gpu: 1`, `LEMONADE_LLAMACPP=vulkan`, and the render node's group as a supplemental group
   (the Lemonade image runs as user 10001).

Why not CDI here: a CDI annotation on the pod is **ignored** (tried on k3s v1.37.1: the container had no `/dev/dri`; kubelet does not hand pod
annotations to the runtime). CDI works when a device plugin names the device in its answer, which is what stage 3 uses.

### Stage 3 (NVIDIA)

1. **Refuses early**, before anything is downloaded or changed: no NVIDIA GPU, a card older than Turing, an image without the open modules, or
   **nouveau holding the card** (then: start the "IYAKKI - NVIDIA driver (nouveau off)" boot entry).
2. Shows the **licence notice** (first lines of the job log, and the review text before Install): what is NVIDIA's, what is open, the size.
3. **Downloads** `NVIDIA-Linux-x86_64-<version>.run` (423 MB) from `download.nvidia.com` only (redirects stay on NVIDIA's hosts), hashes it while
   it arrives and checks the SHA-256 against **the value pinned in IYAKKI and the checksum NVIDIA publishes next to the file**; a mismatch deletes it.
4. `sh <file> --extract-only` (no installer script runs; needs `sh`, `tail`, `xz`, all in the image). From the package's `.manifest` it keeps
   the 64-bit libraries (CUDA, NVML, the PTX compiler, video, OpenCL, the Vulkan ICD and its libraries), the vendor's link names, `nvidia-smi`,
   the CUDA MPS programs, the Vulkan ICD file and the two GSP firmware files, in `<data>/gpu/nvidia/<version>/stage/` (about 580 MB: the real package staged to 578 MB; OpenCL, ray tracing,
   video decode, the debugger and the windowing glue are left out); the download and the unpacked tree (1.6 GB) are deleted.
5. Loads the **open modules** (`modprobe nvidia`, `nvidia-uvm`) with the kernel's firmware path set to the stage, checks that the loaded
   version is the pinned one, makes `/dev/nvidiactl`, `/dev/nvidia<N>`, `/dev/nvidia-uvm*` (what `nvidia-modprobe` would make).
6. Writes the **CDI specs** `/var/run/cdi/nvidia.com-gpu.json` and `k8s.device-plugin.nvidia.com-gpu.json` (written in-house, `gpucdi.rs`):
   device nodes, each library as a read-only mount under `/usr/local/nvidia/lib64` (the real file at its own name and at every link name),
   `nvidia-smi`, the Vulkan ICD, and `LD_LIBRARY_PATH`. **No hook runs**: NVIDIA's container toolkit hook is a glibc program and the host is musl.
7. Makes sure k3s's containerd reads CDI: a drop-in `config-v3.toml.d/iyakki-cdi.toml` (and the containerd 1.7 style one) in the cluster's data
   folder; the cluster restarts once (the shell does it).
8. Deploys NVIDIA's **device plugin** (`nvcr.io/nvidia/k8s-device-plugin:v0.20.1`, `cdi-annotations`, hooks disabled, the stage mounted for NVML);
   waits for the node to report `nvidia.com/gpu`.
9. Smoke pod `gpu-smoke-nvidia` (CUDA base image) runs `nvidia-smi`; the step passes when it prints the pinned driver version.
10. If Lemonade is deployed it gets `nvidia.com/gpu: 1` and the CUDA backend.

After a restart of IYAKKI the RAM holds none of it: the shell puts the render-node modes, the NVIDIA modules, the device nodes and the CDI specs back
(`gpu_steps::ensure_runtime`, every few seconds, at most three tries per start).

### One version for the modules and the vendor files

`595.99.02` is NVIDIA's `latest` Linux release (on 2026-10-20). It is the release tag of `github.com/NVIDIA/open-gpu-kernel-modules` **and** the
`.run` file. One constant, `NVIDIA_VERSION` in `crates/iyakki-agent/src/gpu_nvidia.rs`, must equal `NVIDIA_OPEN_VERSION` in
`dist/buildroot/package/nvidia-open/nvidia-open.mk` (the test `nvidia_version_pins_agree` reads both, and the hash file). To move to another
release: change both, `nvidia-open.hash` (SHA-256 of the GitHub tarball of the tag) and `RUN_SHA256` / `RUN_BYTES` (from
`<run>.sha256sum` and `Content-Length`), then run the tests and rebuild the modules.

## 3. The PC image

`pc` is the macweb recipe for ordinary x86-64 PCs: **no compile**, a repack of the web image with the Mac build's kernel and modules and the
**whole** `linux-firmware` set pruned by `dist/buildroot/board/iyakki/pc-firmware.keep`:

* all GPU firmware: NVIDIA GSP 535.113.01 for Turing, Ampere and Ada (what kernel 6.12's nouveau loads) and the older nouveau files, all of
  `amdgpu` and `radeon`, all of `i915` and `xe` (DMC, GuC, HuC);
* Wi-Fi and Bluetooth of ordinary PCs: iwlwifi, rtw88, rtw89, rtlwifi, MediaTek MT76/MT792x, Atheros ath9k/10k/11k (laptop chips)/12k (WCN7850),
  Ralink, Realtek and Intel Bluetooth; Ethernet extras: Realtek, Broadcom tg3/bnx2, USB 3 host firmware;
* about 420 MB before compression (the Mac image keeps 46 MB): the Intel Wi-Fi files are chosen by what this kernel's `iwlwifi` declares (`board/iyakki/iwlwifi-keep.sh`: the newest usable file of each chip and the one below, 71 MB instead of 265 MB). The compressed initial RAM disk is about 260 MB, more than the 256 MB boot volume of the other images, so **the PC image's install template has a 512 MB boot volume** (`repack.sh` sets it for this variant; `iyakki-usb` takes the size from the image, and a drive with 4096-byte sectors gets 320 MiB). Broadcom Wi-Fi (`brcm/`, Macs and single-board computers) is not kept. The Apple-only drivers stay in the kernel as modules and bind to nothing on a PC.

```
./run.sh build-pc-layered        # minutes; builds the iyakki program, repacks; /etc/iyakki-release says VARIANT=pc-x86_64
```

Output: `target/buildroot-pc/images/iyakki-x86_64.iso` (live, hybrid ISO) and `iyakki-install-x86_64.img` (installs to a stick with a data
partition). **Nothing is written to any drive by the build.** Flash with the IYAKKI USB writer (docs/BOOT-TEST.md: `sudo target/release/iyakki-usb`,
choose the stick, type its name) or `dd`. Secure Boot off for the first tests (docs/SECURE-BOOT.md for a signed image).

### The kernel: what a 4090 PC needs

Checked in `target/buildroot-mac/build/linux-6.12.112/.config` (the kernel the PC image carries today):

| Need | State today | Note |
|---|---|---|
| nouveau (`DRM_NOUVEAU=m`) with GSP | yes, but GSP is on only for Ada (the 4090) by default; Turing and Ampere need `nouveau.config=NvGspRm=1` | **new option** `DRM_NOUVEAU_GSP_DEFAULT=y` makes it the default (needs the kernel rebuild) |
| NVMe, xHCI, USB storage and UAS, SATA AHCI, SD/MMC | yes (built in) | |
| EFI framebuffer, simpledrm, fbdev | yes | |
| Ethernet: r8169 (Realtek 2.5G too), igc, e1000e, igb, ixgbe, tg3 ... | yes (modules) | |
| Wi-Fi: iwlwifi/iwlmvm, rtw88, rtw89, mt7921e/mt7925e, ath10k/11k/12k | yes (modules); `iwlmld` (Wi-Fi 7 BE200 on newer firmware) does not exist in 6.12 | BE200 needs a newer kernel |
| Intel pin control (Alder/Raptor Lake, Tiger Lake, Cannon Lake) | yes | **new**: Ice Lake and Meteor Lake pinctrl, DesignWare I2C PCI, `intel_idle` |
| AMD pinctrl, amd-pstate, intel-pstate | yes | |
| AMD compute (`HSA_AMD`) | **no** | **new** `HSA_AMD=y`: resolves cleanly (`verify-kernel-config.sh` with `VARIANT=mac`); kernel side of ROCm, nothing user-visible yet |

The new options are in `dist/buildroot/board/iyakki/linux.fragment` (block "GPU compute and ordinary PCs"). `verify-kernel-config.sh` reports all
fragment options applied. **A kernel rebuild is needed** for them, and `changed.sh` will say KERNEL for every variant because the fragment changed
(on this repository: `./run.sh build-mac` rebuilds the Mac kernel; add `IYAKKI_NVIDIA_OPEN=1` to build the NVIDIA modules in the same run).
`HSA_AMD` is included now only to avoid a second rebuild for stage 2; leave it out if the extra kernel code is not wanted.

### The NVIDIA open modules (package `nvidia-open`)

`dist/buildroot/package/nvidia-open/{Config.in,nvidia-open.mk,nvidia-open.hash}`, off by default (`configs/iyakki_nvidia_open.fragment`, switched on by
`IYAKKI_NVIDIA_OPEN=1 ./run.sh build-mac`); images built without it are unchanged. Source: GitHub archive of the release tag, dual MIT / GPL-2.0.
Installed like apple-bce into `/lib/modules/<kernel>/updates/`; the post-build step signs them with the others when the image is Secure-Boot-signed;
`build.sh` rebuilds the package when the kernel changes.

Tried here without touching any build output: the sources compiled with **the Buildroot toolchain of `target/buildroot-mac`** against a private copy
of the 6.12.112 tree with the new fragment (`modules_prepare`): `nvidia.ko`, `nvidia-uvm.ko`, `nvidia-modeset.ko`, `nvidia-drm.ko`, `nvidia-peermem.ko`
built, vermagic `6.12.112 SMP preempt mod_unload`, all 595.99.02 "Dual MIT/GPL". The package itself (the `make` call through Buildroot) has not run.

## 4. Quick install licence wording (the review text)

The NVIDIA entry shows, before Install: *kernel modules MIT / GPL-2.0 (in the image); user-space libraries, nvidia-smi and GSP firmware are NVIDIA's
proprietary software under NVIDIA's driver licence, downloaded by you from NVIDIA, not shipped; device plugin Apache-2.0; pressing Install accepts
NVIDIA's licence for the downloaded files*, and the size (about 400 MB from download.nvidia.com). The same text starts the job log. The wording
needs the owner's legal review (docs/BETA-PLAN.md).

## 5. Third-party pieces and licences

| Piece | Licence | In the image? |
|---|---|---|
| NVIDIA open GPU kernel modules 595.99.02 | MIT and GPL-2.0 (dual) | yes, only in the PC image built with the switch; source in `legal-info/` with the kernel |
| NVIDIA user-space libraries, `nvidia-smi`, GSP firmware (`.run`) | NVIDIA driver licence (proprietary) | **never**; downloaded at the click into `<data>/gpu/nvidia/` |
| NVIDIA k8s device plugin v0.20.1 | Apache-2.0 | no; the cluster pulls `nvcr.io/nvidia/k8s-device-plugin` at the click |
| squat/generic-device-plugin | Apache-2.0 | no; pulled at the click (pinned by digest) |
| llama.cpp (Vulkan and CUDA server images), Ollama, Lemonade | MIT, MIT, Apache-2.0 | no; pulled at the click; the CUDA images carry NVIDIA's CUDA licence |
| Mesa (RADV, ANV, NVK) | MIT | inside the pod image |
| linux-firmware (GPU and PC subset) | each file its own redistributable licence | yes, in the PC image (the `hw` rules of LICENSING.md item 7) |
| CDI (Container Device Interface) | open specification | the writer is IYAKKI's own code |

## 6. Files

`crates/iyakki-agent/src/`: `gpu.rs` (detection, verdicts, report, pods, test prompt), `gpucdi.rs` (CDI writer, containerd drop-ins),
`gpu_nvidia.rs` (pins, notice, download, staging, device nodes, CDI spec, state machine, plugin manifest), `gpu_workloads.rs` (generic plugin,
smoke pods, Lemonade patch, Ollama), `gpu_steps.rs` (the two steps, restore after a start, the test job); `crates/iyakki-shell/src/brandmenu_gpu.rs`
(the page); `dist/buildroot/package/nvidia-open/`, `board/iyakki/pc-firmware.keep`, `repack.sh --firmware-from`, `./run.sh build-pc-layered`.

## 7. Verified here, and not

**Verified on this development PC (AMD Radeon 780M, a throw-away k3s v1.37.1 in a container that was removed afterwards):** the containerd
drop-in is imported and `crictl info` shows `enableCDI: true`; the generic device plugin registers and the node reports `iyakki.dev/gpu: 4`; an
**unprivileged** pod asking for `iyakki.dev/gpu: 1` runs `llama-server --list-devices` and prints `Vulkan0: AMD Radeon 780M Graphics (RADV PHOENIX)`
(without the device: `(none)`; privileged: works; hostPath without privileges: `(none)`); llama.cpp b11749 ran a 15M model on that GPU and its real
answer (with `timings`) is the fixture of the tokens-per-second parser; every manifest passed `kubectl apply --dry-run=server`; the Lemonade patch
and unpatch applied and reverted on a real Deployment; the vendor `.run` downloaded from NVIDIA (hash equals the pin), unpacked with the image's
BusyBox `sh`/`tail`/`xz` in 10 s, and staged by the real code.

**Not verified (needs the 4090 PC):** loading `nvidia.ko` on a real card; nouveau/NVK on the 4090; the GSP firmware path; the device nodes; that
containerd accepts the CDI specs and applies the mounts; that the NVIDIA device plugin starts without the toolkit, enumerates the GPU with the stage
mounted and answers with `k8s.device-plugin.nvidia.com/gpu=<index>` (the vendor name is from the plugin's source, not from a run); `nvidia-smi` and
a CUDA program in a pod; the vendor Vulkan ICD in a container; the CUDA backend setting of Lemonade (`LEMONADE_LLAMACPP=cuda` is from its documents);
the PC image booting on a PC at all; the Buildroot package build through `make`.

## 8. Real-hardware checklist: the RTX 4090 PC

Do the steps in order and write down what each one shows. Part A needs only the PC image as built today. Part B needs the image rebuilt with the
kernel options and the NVIDIA modules.

### Part A: boot, nouveau and Vulkan (stage 1)

1. **Flash** the PC image to a USB stick (`target/buildroot-pc/images/iyakki-install-x86_64.img` with `iyakki-usb`, or the ISO). Pick the stick by
   name; the tool refuses your system disk.
2. **Firmware settings:** UEFI, **Secure Boot off**, boot the stick from the one-time boot menu. Pick **"IYAKKI - hardware check"** first.
3. Run `iyakki hwreport` (the check screen shows it; `S` saves it). **Send back:** the whole report.
4. Reboot, pick **"IYAKKI"** (normal). **Expected:** the screen is up on the 4090 (nouveau, GSP firmware `nvidia/ad102/gsp/gsp-535.113.01.bin` from the
   image). If the screen stays black or garbled, pick **"IYAKKI - safe graphics"**, and send back what you saw.
5. Open a terminal (CLI tile) and run `dmesg | grep -i -E "nouveau|nvrm|firmware|gsp"`. **Expected:** `nouveau 0000:01:00.0: NVIDIA AD102`, no `Direct firmware load ... failed`.
   **Send back:** these lines.
6. Sign in as administrator. Store → **Setup**, group Hardware, **GPU for local models (Vulkan)**, **Open its page**.
   **Expected:** `GeForce RTX 4090`, driver `nouveau`, render node `renderD128`, "usable: ... NVIDIA Ada Lovelace through nouveau: Mesa NVK".
   **Send back:** a photo or the text of the page.
7. Press **Install** (k3s is installed first if missing; the first k3s start takes a few minutes). **Expected log (Installer runs):**
   `[0/6]` the 4090 usable, `[1/6]` cluster, `[2/6]` render node made openable, `[3/6]` the plugin, "the node reports iyakki.dev/gpu = 8", `[4/6]` the
   smoke pod pulls a 300 MB image, "the pod sees: Vulkan0: NVIDIA GeForce RTX 4090 (NVK AD102)", `[6/6] done`. **Send back:** the whole log.
   If the pod sees only `llvmpipe` or `(none)`: send `kubectl -n iyakki-ai describe pod gpu-smoke-vulkan` and `ls -l /dev/dri`.
8. Install **Lemonade** (Quick install), wait for it, pull a small chat model (Settings, Models, lemonade-local, e.g. a 1B model), then GPU page,
   **Run test prompt**. **Expected:** the log in Installer runs ends with `NN.N tokens/s (...)`. **Send back:** that line, and the same on an iGPU or CPU if you have
   one for comparison. (If Lemonade was installed before step 7, step 7 patched it already; if after, run step 7 again.)

### Part B: the NVIDIA driver (stage 3)

9. **Rebuild** on the development PC (hours for the kernel; tell the owner first): `IYAKKI_NVIDIA_OPEN=1 ./run.sh build-mac` (kernel with the new
   options and the four NVIDIA modules), then `./run.sh build-pc-layered`; flash the new image. In `iyakki-release` the kernel is the same version.
10. Boot the entry **"IYAKKI - NVIDIA driver (nouveau off)"** (it adds `modprobe.blacklist=nouveau`; the screen is the firmware's framebuffer).
    Check: `lsmod | grep nouveau` shows nothing; `ls /lib/modules/*/updates/` lists `nvidia.ko`.
11. Quick install: the row **NVIDIA GPU driver (CUDA)** is listed (not on a machine without an NVIDIA GPU). Select it: **read the review text**
    (licence, the 423 MB download from download.nvidia.com). **Expected on the GPU page:** the 4090 with no driver bound, "NVIDIA open modules: present".
12. Press **Install**. **Expected log:** the licence notice, `[0/9] where this stands: the vendor files are not downloaded yet`, `[2/9]` the download with
    percent lines, `[3/9]` unpacked, "staged N libraries ... firmware 2", `[4/9]` "loaded nvidia", "loaded nvidia-uvm", "the loaded driver is 595.99.02, as pinned",
    "GPU minor 0 · GPU-...", "made /dev/nvidiactl ...", `[5/9]` two CDI files, `[6/9]` "the cluster restarts once", `[7/9]` the plugin, "the node reports nvidia.com/gpu = 1",
    `[8/9]` the smoke pod, `[9/9] done`. **Send back:** the whole log, **even if it stops with an error**, plus:
    `dmesg | grep -i -E "nvrm|nvidia"`, `cat /proc/driver/nvidia/version`, `ls -l /dev/nvidia* /var/run/cdi`, `kubectl -n kube-system logs ds/nvidia-device-plugin`,
    `kubectl describe node | grep -i nvidia`.
13. In the CLI tile: `kubectl run -n iyakki-ai smi --rm -it --restart=Never --image=docker.io/nvidia/cuda:13.3.1-base-ubuntu24.04 --overrides='{"spec":{"containers":[{"name":"smi","image":"docker.io/nvidia/cuda:13.3.1-base-ubuntu24.04","command":["nvidia-smi"],"resources":{"limits":{"nvidia.com/gpu":1}}}]}}'`.
    **Expected:** the `nvidia-smi` table with the RTX 4090, 24 GB, driver 595.99.02. **Send back:** the output.
14. Check the CDI name the plugin really returns: `kubectl -n iyakki-ai get pod gpu-smoke-nvidia -o yaml` (while it runs, or run step 12 again and watch) and look
    for `cdi.k8s.io/` annotations: the value should be `k8s.device-plugin.nvidia.com/gpu=0`. If it is another vendor name, send it back: one constant
    (`PLUGIN_KIND`) changes.
15. Deploy a GPU model server: Lemonade (Quick install) is patched by step 12 (`nvidia.com/gpu: 1`, CUDA backend). Pull a chat model and press
    **Run test prompt** on the GPU page. **Expected:** a clearly higher number than step 8 (a 7B-8B 4-bit model should be well above 100 tokens/s on a 4090).
    **Send back:** model name and the `tokens/s` line. The Ollama variant: `kubectl apply -f` the manifest `ollama_gpu_manifest(Nvidia)` (dumped with
    `GPU_DUMP_DIR=/folder cargo test -p iyakki-agent --lib manifests_can_be_dumped`), then `kubectl exec -n iyakki-ai deploy/ollama-gpu -- ollama run llama3.2:1b hi`.
16. Reboot (same entry) and open the GPU page. **Expected:** within a minute the module is loaded again and the specs are back (the line "GPU: NVIDIA driver loaded
    again" appears beside the clock); `kubectl get nodes -o json | grep nvidia` shows the resource. **Send back** what you see if not.

### What to send back (summary)

The `hwreport`; the Quick install logs of both stages (Installer runs, "Copy" or photo); the `dmesg` lines above; `nvidia-smi` output; `ls -l /dev/dri /dev/nvidia* /var/run/cdi`;
`kubectl -n kube-system logs ds/nvidia-device-plugin`; the `tokens/s` lines (which model, which GPU path); anything that did not match "Expected".

## 9. Troubleshooting

| Symptom | Likely cause | What to do |
|---|---|---|
| Black screen on the normal entry | nouveau and the 4090 (GSP firmware or a driver bug) | "safe graphics" entry; send `dmesg` from "show boot messages" |
| GPU page: "no render node" | the DRM driver did not bind | read the kernel lines on the page; missing firmware is listed in red |
| Stage 1 log: "the pod sees only llvmpipe" or "(none)" | the pod did not get the device | `kubectl -n kube-system get ds iyakki-gpu-device-plugin`, its log; `kubectl describe node | grep iyakki.dev` |
| Stage 1: node never reports `iyakki.dev/gpu` | plugin pod pending (image pull) or the kubelet folder differs | `kubectl -n kube-system describe pod -l app.kubernetes.io/name=iyakki-gpu-device-plugin` |
| Lemonade does not use the GPU | its llama.cpp backend was not Vulkan/CUDA | check `LEMONADE_LLAMACPP` in the Deployment; pick the backend in Lemonade's own settings |
| Stage 3 refuses: "nouveau ... modprobe.blacklist=nouveau" | nouveau holds the card | boot "IYAKKI - NVIDIA driver (nouveau off)" |
| Stage 3 refuses: "no NVIDIA kernel modules" | the image was built without `IYAKKI_NVIDIA_OPEN=1` | rebuild (section 3) |
| Stage 3: "checksum ... not the one pinned" | a damaged or replaced download | the file was deleted; try again; if it repeats, NVIDIA changed the file: tell the owner |
| Stage 3: "unpacking failed" | `sh`, `tail` or `xz` missing, or no room in the data folder (needs about 2.5 GB free while unpacking) | free space on the data partition |
| `modprobe nvidia` fails | module built for another kernel, or Secure Boot refuses unsigned modules | Secure Boot off, or sign the image (docs/SECURE-BOOT.md); `dmesg | grep -i nvrm` |
| Driver loads, "found no GPU" | GSP firmware not found | `dmesg | grep -i firmware`: the stage must hold `firmware/nvidia/595.99.02/gsp_ga10x.bin`; the firmware path file is `/sys/module/firmware_class/parameters/path` |
| "the loaded driver is X, not the pinned" | the image's modules and the vendor files differ | rebuild the modules from the pinned tag (section 2) |
| Cluster did not come back with CDI on | k3s restarted before the drop-in was read | `ls <data>/k3s-dev/data/agent/etc/containerd/config-v3.toml.d/`; restart the cluster in Settings, Dev cluster |
| Plugin pod crash loops | NVML not found in the stage, or the toolkit expected | its log (`kubectl -n kube-system logs ds/nvidia-device-plugin`); `ls <data>/gpu/nvidia/595.99.02/stage/lib64/libnvidia-ml.so.1` |
| Pod: "NVML: Unknown error" | the CDI spec was lost (a restart) or the nodes are missing | the shell restores them within a minute; `ls /var/run/cdi /dev/nvidia*` |
| `nvidia-smi` in the pod: driver/library version mismatch | the stage and the loaded module are different versions | run the step again (it re-stages only when the stage is missing: delete `<data>/gpu/nvidia` first) |

## 10. Removing it

Delete the objects (`kubectl delete ds -n kube-system iyakki-gpu-device-plugin nvidia-device-plugin`, the smoke pods, `ollama-gpu`), the markers
`<data>/quick-install/gpu-vulkan.done` and `gpu-nvidia.done`, `<data>/gpu/`, and the drop-ins under `<data>/k3s-dev/data/agent/etc/containerd/config*.toml.d/iyakki-cdi.toml`
(then restart the cluster). `gpu_workloads::lemonade_unpatch` takes the GPU away from Lemonade. A one-click removal is not built yet.
