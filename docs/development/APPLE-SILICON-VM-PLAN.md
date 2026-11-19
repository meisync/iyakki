# IYAKKI in a light VM on Apple Silicon: design and plan

**For:** the owner and whoever builds it. **You will get:** the hypervisor choice with licences, the guest (arm64 image, k3s, memory by Mac size), the model bridge to the Mac's GPU, what devices work and what does not, licensing, install and update on macOS, testing, phases, risks, 10 owner decisions with recommendations, and proposed FEATURES rows. **Status:** plan only, nothing built, nothing run on a Mac (2026-10-27).

**Owner decision (2026-10-27):** for Apple Silicon, IYAKKI runs as a lightweight VM with k3s inside, plus a bridge to a model runner on the macOS host. Bare-metal Asahi Linux (F-BOOT-09, `docs/APPLE-SILICON.md` sections 1, 3, 4) is **out of scope for now**. Boot speed of the VM is accepted as it is. This plan replaces "route 3, not built" of `docs/APP-MODE-AND-APPLE-SILICON.md` and the "recipe C" stub of `docs/APPLE-SILICON.md` section 2.

Honesty tags: **verified** (read in this repository, or fetched from the upstream page on 2026-10-27, source named), **from memory** (I know it from Apple's documentation or earlier reading, but did not re-fetch it today; check before relying on it), **estimate** (my number, not measured), **not verified** (could not check).

## 0. Short version

1. **Hypervisor:** Apple's Virtualization.framework (it uses the Hypervisor framework underneath; nothing to install on the Mac). Driven by **our own small launcher, `iyakki-mac`, written in Rust** over the `objc2-virtualization` bindings (licence `Zlib OR Apache-2.0 OR MIT`, verified on crates.io). The launcher is Apache-2.0 and ours. First, a **phase 0 prototype with the existing `vfkit`** (Apache-2.0, `brew install vfkit`) so the guest is proven before any launcher code exists.
2. **Guest:** the existing aarch64 IYAKKI image, started by direct kernel boot (`Image` + `rootfs.cpio.gz`, no GRUB), RAM root as today, k3s downloaded inside at the person's click as on every other machine. Data lives on a **second raw sparse disk** (`IYAKKI-DATA`) that updates never touch.
3. **UI:** the VM is headless. The shell is shown by the existing browser front end (`iyakki web`, pixel stream) on a loopback port of the Mac, reached over vsock. A native window is later.
4. **Model bridge:** models run on the Mac (Ollama, LM Studio, llama.cpp, MLX; Metal). The launcher relays **vsock to the runner's loopback port** (an allow-list of ports, destination always `127.0.0.1`), the guest relay re-exposes it as `127.0.0.1:<same port>` inside the guest, so the existing provider config (`http://127.0.0.1:11434/v1`) works unchanged. The Mac runner never has to listen on the network. AI-HARDWARE gets a new profile: "no GPU here, models run on the host bridge".
5. **Ship:** the launcher (source, tap formula) and our Linux image. **Not shipped:** any Apple binary, any model runner, k3s. Install through a user-side Homebrew tap built from source and ad-hoc signed on the Mac; no Apple developer account is needed for the owner's own Mac.

## 1. What exists today (verified in the tree, 2026-10-27)

* **The image.** `dist/buildroot/configs/iyakki_aarch64_defconfig`: aarch64, musl, kernel 6.12.112 with `board/iyakki-aarch64/linux.fragment` (393 lines), GRUB arm64-efi, `rootfs.cpio.gz` initramfs, one 256 MB FAT ESP in `iyakki-aarch64.img` (`genimage.cfg`). Boot menu in `grub.cfg` has entries including `iyakki.web=1` (HTTPS on 8443) and `iyakki.ssh=1`. Console options are `console=tty0 console=ttyAMA0,115200`.
* **Tests.** `qemu-test-aarch64.sh` boots `Image` + initrd directly in QEMU `virt`, `-cpu cortex-a72 -accel tcg` (slow software emulation), and checks the self-test markers, an `eth0` address `10.0.2.x` and the arch line. `qemu-uefi-aarch64-test.sh` boots the disk image through AAVMF and GRUB with `virtio-gpu-pci`, `qemu-xhci`, USB keyboard and tablet, and checks "supervisor up" and "shell: WxH framebuffer". Both exist and pass on a Linux PC per F-BOOT-08 (`partial`, 90 %); **never run on a Mac**.
* **Guest kernel options present:** `VIRTIO`, `VIRTIO_PCI`, `VIRTIO_MMIO`, `VIRTIO_BLK`, `VIRTIO_NET`, `VIRTIO_CONSOLE`, `VIRTIO_INPUT`, `HW_RANDOM_VIRTIO`, `DRM_VIRTIO_GPU`, `DRM_SIMPLEDRM`, `USB_XHCI_*`, `USB_STORAGE`, `USB_UAS`, `VIRTIO_BALLOON=m`, `SND_VIRTIO=m`, `OVERLAY_FS=y`, `MEMCG`, `CGROUP_BPF`, `CFS_BANDWIDTH`, `BRIDGE=y`, `VETH=m`, `VXLAN=m`, `NF_TABLES=m`, `NF_NAT_MASQUERADE=y`, `EFI_STUB=y`.
* **Guest kernel options absent** (grep of the fragment, nothing in it): `VSOCKETS`, `VIRTIO_VSOCKETS`, `FUSE_FS`, `VIRTIO_FS`, `CPUSETS`, `MEMORY_HOTPLUG`, `IP_VS`. Whether the arm64 defconfig the build starts from (`BR2_LINUX_KERNEL_USE_ARCH_DEFAULT_CONFIG=y`) already turns some on (it may for `CPUSETS`) is **not verified**; no built `.config` was read today.
* **The model host page.** `docs/MAC-MODEL-HOST.md` and `crates/iyakki-agent/src/modelhost.rs` (1104 lines): `parse_target` accepts only private, link-local and loopback addresses (and `.local`/`.lan`/one-word names that resolve to such), `test` does `GET /v1/models` then a streamed chat and reports tokens per second, `add_to_models` registers the provider `mac-<nickname>` with a vault key reference `vault:mac-<nickname>-key`, and `service_manifest`/`publish` give pods a Service + Endpoints in `iyakki-ai`. Built and tested against fakes only. It speaks plain `http://`. **This is the existing "external provider" path; the bridge reuses it.**
* **Ollama on loopback.** `docs/MACOS.md`: at sign-in IYAKKI looks for an Ollama on `127.0.0.1:11434` and adds it as `ollama-local`; for a VM it today tells the person to use `10.0.2.2` (QEMU) or the Mac's own address with `OLLAMA_HOST=0.0.0.0` (UTM), which exposes the runner to the whole network. The bridge below removes that need.
* **Providers.** `providers.rs` (1786 lines): Ollama, OpenAI, OpenRouter, Anthropic, Gemini and "any OpenAI-compatible server"; keys are never stored in the provider, only a secret reference (`vault:`, `env:`, `file:`, `ext:`) resolved per request. `lemonade.rs` serves AMD's Lemonade; `hostengines.rs` is about speech engines on the machine, **not** model runners (I read its header only to make sure).
* **AI profile.** `docs/AI-HARDWARE.md`, `aiprofile.rs`: Apple Silicon memory rule (two thirds of unified memory up to 36 GB, three quarters above), tiers (None under 6 GB usable, Small, Medium, Large, XL), "Use a Mac's GPU" card recommended on machines with no GPU for models and **hidden on an Apple Silicon Mac**. A VM guest has no GPU and no Apple device tree, so today it would be read as "No GPU: CPU only" and hide Lemonade and Ollama.
* **Other facts used.** `docs/development/UI-OS-SPLIT-PLAN.md` (approved 2026-10-25): tiles as web bundles, `iyakki-core-api`, a future remote connector with the developer-mode pairing and scopes; steps 1-4 done, 5-9 planned. `docs/development/MINIMAL-WEBSERVICE.md`: `iyakki serve --web` / `IYAKKI_WEB` is the pixel-stream front end (`iyakki-remote`: rustls, machine-account sign-in, `HttpOnly` + `SameSite=Strict` + `Secure` cookie, CSRF token, `/healthz`), `dist/Dockerfile` and `dist/k3s-web.yaml` exist. `docs/K3S-DEV.md`: a local cluster costs "about 2 GB of memory, CPU cores and 3 GB of disk". `crates/iyakki-storage`: the data volume is an ext4 filesystem labelled `IYAKKI-DATA`; first-boot formatting exists for a marked GPT partition after an install. `dist/release/check-macos.sh` already runs `cargo check` for `aarch64-apple-darwin`.
* **The `deny.toml` licence allow-list** includes Apache-2.0, MIT, BSD-2/3, ISC, Zlib, Unicode-3.0, CC0, 0BSD, Unlicense, BSL-1.0 (so the objc2 family fits).

## 2. Question 1: which hypervisor on macOS

### 2.1 Options and licences

Licences below were read from the GitHub licence API and the repositories on 2026-10-27 unless marked.

| Option | Licence | What it is | For us |
|---|---|---|---|
| **Virtualization.framework** (Apple) | Apple system framework, part of macOS; not redistributable and not needed to redistribute (linked dynamically on the Mac) | High-level VM API over the hypervisor: Linux boot loader, EFI boot loader (macOS 13+), virtio block, net (NAT), console, entropy, balloon, vsock, virtio-fs, virtio-gpu (2D scanout), sound, USB keyboard/pointer, USB mass storage, Spice agent port. Device list cross-checked against the `vfkit` usage page fetched today | **Recommended base** |
| **Hypervisor.framework + QEMU** | QEMU is GPL-2.0 (from memory; not re-read) | Full device model, qcow2, many devices, `-accel hvf` | Possible as a **user-installed** tool (`brew install qemu`, which is what `docs/APPLE-SILICON.md` recipe B already says). We would not ship it. More devices and options than Virtualization.framework, but a large C code base, no suspend to file that is simple, and a Cocoa/SDL window we do not control. Keep as the **fallback and the CI stand-in**, not the product |
| **UTM** | Apache-2.0 for the app source (GitHub licence API). The app is also sold in the App Store; the free download exists (from memory) | GUI over QEMU or Virtualization.framework | Fine as a manual route for the owner today (recipe A of `docs/APPLE-SILICON.md`). Not a dependency: a GUI app we cannot script, and we would be building on its UX |
| **Lima** | Apache-2.0 | Linux VMs on macOS, default driver `vz` since Lima 1.0 (docs fetched today); signs its driver ad hoc with `com.apple.security.virtualization` plus network client/server entitlements (`Makefile` and `vz.entitlements`, fetched today) | Not our product (it manages Ubuntu/Fedora style cloud images, uses cloud-init, installs its own guest agent, wants `limactl` state in `~/.lima`). Useful as a proof that the ad-hoc signing path works and as a second prototype tool |
| **Colima** | MIT | Docker/containerd on top of Lima | Not relevant: it is a container runtime for developers |
| **vfkit** | Apache-2.0 | A small command line over Virtualization.framework (Go): `--bootloader linux` or `efi`, `virtio-blk`, `nvme`, `usb-mass-storage`, `virtio-net` (NAT, unix socket, fd), `virtio-serial`, `virtio-rng`, `virtio-balloon`, `virtio-vsock`, `virtio-fs`, `rosetta`, `virtio-gpu`, `virtio-input`, a REST API for state, `brew install vfkit` | **Phase 0 prototype**, downloaded by the person, never shipped (project rule for third-party tools). Also a precise reference for how the framework behaves |
| `krunkit` / `libkrun` | Apache-2.0 (licence files fetched today) | libkrun-based microVMs; the variant that can give containers a virtual GPU | Only for the later "GPU in the guest through Vulkan-to-Metal" experiment already mentioned in `APP-MODE-AND-APPLE-SILICON.md`. Not part of this plan |
| Tart (Cirrus Labs) | **Functional Source License 1.1, Apache-2.0 future licence** (licence file fetched today) | Virtualization.framework VM manager | **Do not use**: not an open-source licence while the source is under FSL |
| Our own launcher in **Swift** | ours (Apache-2.0); the Swift toolchain is Apache-2.0 (from memory) | Direct use of the native API, the API's own language | Strong second choice, see below |
| Our own launcher in **Rust** over `objc2` | ours; `objc2` 0.6.5 MIT, `objc2-virtualization` 0.3.2 `Zlib OR Apache-2.0 OR MIT`, `objc2-foundation` and `block2` MIT (crates.io, fetched today), all inside `deny.toml` | Same API through generated bindings | **Recommended** |

### 2.2 Recommendation: Rust launcher `iyakki-mac` over Virtualization.framework

Reasons (rough order of weight):

1. **Same language, repository, CI and licence gate as the rest.** A Rust crate (`crates/iyakki-mac`, `cfg(target_os = "macos")`, tiny non-macOS stub so the workspace still builds) is checked by the existing `check-macos.sh` (`cargo check --target aarch64-apple-darwin`) and by `cargo deny`. A Swift package needs Xcode or the Swift toolchain on a Mac to compile at all: nothing of it can be checked on the Linux build machine. Whether `cargo check` for the darwin target works for the objc2 framework crates without an Apple SDK present is **not verified**; I believe the generated bindings need no SDK to type-check, but test this first (phase 3 step 1).
2. **The relay and control logic are the real work, and they are plain Rust** (allow-lists, framing, port mapping, image manager, memory table, state machine). They are unit-testable on Linux behind a thin trait over the framework (`trait Machine { start, stop, request_stop, pause, resume, vsock_listen, vsock_connect, balloon_target }` with a fake). Only the trait's one implementation touches the framework.
3. **The in-house rule.** The owner prefers own code over dependencies, permissive licences only. The bindings are the one new dependency family, all permissive.
4. **Startup time and device set are those of the framework in every option** (Lima's `vz`, vfkit, UTM's Apple backend and ours all call the same API), so none of the options is faster by itself. What we control is what runs before the kernel: direct Linux boot (below) skips EFI and GRUB.
5. **Ad-hoc signing is enough for the owner's Mac** (section 8). Verified indirectly: Lima signs its driver with `codesign -f -v --entitlements vz.entitlements -s -` (ad hoc) and ships it, and the entitlement is `com.apple.security.virtualization`.

Costs and the reason Swift stays on the table: objc2 bindings are generated, `unsafe`-heavy and their documentation is thin; the framework wants its calls on the main dispatch queue (from memory) and uses completion-handler blocks and delegates, which `block2` supports but is fiddly. If phase 3 step 2 (boot a guest to a console log) takes more than about 5 working days, switch to Swift for the 400-line framework shim and keep everything else in Rust behind the same `Machine` trait (a small static library or a sub-process speaking the control protocol of section 4.5). That fallback does not change the plan.

**Not chosen:** our own full QEMU build (GPL, large, no gain), Lima/UTM as the product (their UX and state, not ours), anything with GPU passthrough.

### 2.3 What Virtualization.framework gives, against the list asked for

| Item | Result | Source |
|---|---|---|
| Startup time | Not measured. A kernel+initrd direct boot skips firmware and GRUB; the guest then needs the normal IYAKKI boot (the splash and the sign-in screen); the owner accepted boot speed. Record it in phase 0 | **not verified**; `docs/BOOT-SPEED.md` has the guest-side timeline tool (`dist/buildroot/boot-timeline.sh`) |
| Memory ballooning | `virtio-balloon` device, host can lower the guest's target (vfkit page: "reclaim memory from the guest when needed and return it when available"). Guest needs `CONFIG_VIRTIO_BALLOON` (it is `=m`: load at boot). Without a balloon the VM keeps what it touched | vfkit page (verified); behaviour of freed pages **from memory** |
| virtio-fs shared folders | Yes (`virtio-fs`, mount with `mount -t virtiofs <tag> /mnt`). Guest needs `FUSE_FS` and `VIRTIO_FS` (absent today). Ownership and permission mapping and missing inotify are the usual limits (from memory) | vfkit page (verified) |
| virtio-gpu, no 3D | Yes, a scanout with a configured resolution (vfkit: width and height); no 3D, no Metal in the guest. Guest already has `DRM_VIRTIO_GPU` | vfkit page (verified) |
| virtio-net NAT | Yes, `nat` is the default; the guest address comes from the host's DHCP and is found in `/var/db/dhcpd_leases` (vfkit page). The usual subnet is `192.168.64.0/24` with the host at `.1` (from memory, **not verified**) | vfkit page; subnet from memory |
| virtio-vsock | Yes. **macOS has no host `AF_VSOCK`**: vfkit exposes each port as a unix socket on the host; in the framework the host side is a `VZVirtioSocketDevice` with listeners and `connect(toPort:)` in the launcher's own process. Guest side is standard `AF_VSOCK`, host CID 2 (`nc --vsock 2 5` in the vfkit page). Guest needs `VSOCKETS` and `VIRTIO_VSOCKETS` (absent today) | vfkit page (verified) |
| Suspend / resume | Pause and resume in memory: yes (vfkit REST API `Pause`, `Resume`). Save the machine state to a file and restore it: the framework has it for Apple-silicon hosts from macOS 14; whether it works with Linux guests and with our device set (vsock, virtio-fs) is **not verified** | pause/resume verified; save to file **from memory** |
| Guest clock after host sleep | The guest clock stops while the Mac sleeps; vfkit solves it with a `qemu-guest-agent` over vsock. We do it ourselves: the launcher sends the host time on wake over the control channel | vfkit page (verified problem); our design |
| The IYAKKI image as a disk | **Raw or ISO only** (no qcow2). On APFS a raw file can be sparse (`truncate`) or a copy-on-write clone (`cp -c` / `clonefile`). ASIF (Apple's newer sparse image format, macOS 26) is **from memory**: not needed, raw sparse is enough | vfkit page (verified) |
| Boot | `VZLinuxBootLoader` (kernel must be the **uncompressed** arm64 `Image`, which `BR2_LINUX_KERNEL_IMAGE=y` makes) or `VZEFIBootLoader` (macOS 13+). Direct boot recommended: two files, no GRUB, update is a file swap | vfkit page (verified) |
| Rosetta | Not needed: the guest is arm64, and the project rule is nothing from Apple redistributed. (Rosetta would only matter for x86-64 containers in the cluster; out of scope) | vfkit page (verified that it is for x86-64 binaries) |
| Minimum macOS | EFI boot, vsock, balloon, virtio-fs need macOS 12 or 13; Lima's `vz` driver says macOS 13+ (docs fetched today). Recommended minimum: **macOS 14** (save/restore, fewer early bugs); decision 10 | partly verified |

## 3. Question 2: the guest and k3s

**Guest image.** A new variant `apple-vm` (name chosen so it is not confused with the bare-metal `apple` fragment, which stays parked), built from `iyakki_aarch64_defconfig` with:

* kernel fragment additions (all small): `CONFIG_VSOCKETS=y`, `CONFIG_VIRTIO_VSOCKETS=y`, `CONFIG_FUSE_FS=y`, `CONFIG_VIRTIO_FS=y`, `VIRTIO_BALLOON=y` (so the target works from the first second; a module would need the initrd to load it), `CONFIG_SND_VIRTIO` stays `=m`, the console on `hvc0` (virtio console; the grub entries use `ttyAMA0`, which is the QEMU PL011; which one Virtualization.framework offers for a Linux guest is **from memory** only: vfkit's example uses `console=hvc0`). Size: tens to a few hundred KB (**estimate**, to be measured by the incremental kernel build).
* two release assets instead of a disk image: `Image` and `rootfs.cpio.gz` (they are already produced; `genimage` just wraps them in an ESP). Kernel command line chosen by the launcher: `console=hvc0 iyakki.web=127.0.0.1:8443 iyakki.host=vz` (the last word is new: it tells the guest it is a VM with a host bridge; section 5).
* `iyakki.web` bound to the guest's loopback, **not** to the NAT address, so nothing on the Mac's virtual network reaches the UI except through the launcher.

**k3s.** IYAKKI downloads and checks the k3s program at the person's click (`docs/K3S-DEV.md`), as on x86-64 and on a normal aarch64 machine; nothing is shipped. What this plan needs checked:

* that the download picks the arm64 build (I found an `arm64` choice in `agent.rs:2506` and in `linuxapps_dist.rs:70`; I did **not** read the k3s download code path, so this is **not verified** for k3s);
* the k3s `check-config` list against the aarch64 kernel: present are `MEMCG`, `CGROUP_BPF`, `CFS_BANDWIDTH`, `OVERLAY_FS`, `BRIDGE`, `VETH`, `VXLAN`, `NF_TABLES`, masquerade; **not seen in the fragment:** `CPUSETS`, `CGROUP_PIDS`, `IP_VS`. A single run of `verify-kernel-config.sh` on the built tree answers it (phase 1);
* k3s runs as root in the guest: the guest has no GPU and **none is promised** (no `nvidia.com/gpu` or Vulkan in pods). Pods that need a model call the bridge (section 5). `libkrun` with a virtual GPU is a separate experiment and not part of 1.0.0.

**Memory and CPUs by Mac size (estimates, not measured; to be replaced by the phase 0 numbers).** The Mac's GPU models need the unified memory too (`MAC-MODEL-HOST.md`: model + context must fit in the Mac's memory minus about 4 GB for macOS), so the VM must stay small. Defaults chosen by the launcher from `hw.memsize`; the person can change them.

| Mac memory | VM memory | vCPUs | k3s | Notes |
|---|---|---|---|---|
| 8 GB | 3 GB | 2 | **off by default** (switch on at the person's risk; K3S-DEV needs about 2 GB) | Host keeps about 5 GB: 3B to 4B models only; the Settings page says so |
| 16 GB | 4 GB | 4 | on demand | 7B to 14B models on the host stay possible |
| 24 GB | 6 GB | 4 | on | |
| 32 GB | 8 GB | 6 | on | the AI profile counts about 21 GB usable for models on the host |
| 64 GB and up | 12 GB (cap) | 8 | on | a larger VM adds little; models are the memory user |

Rules: VM memory never above a quarter of the Mac's memory; vCPUs never above the performance-core count minus one (the launcher reads `hw.perflevel0.physicalcpu`, **from memory**, with `hw.physicalcpu` as fallback). With the balloon, the launcher may lower the target when the host is under memory pressure (`DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`, from memory) and raise it again; the first version only sets the size at start. The IYAKKI root is a RAM filesystem (aarch64 compressed initramfs about 65 MB per `APPLE-SILICON.md`; unpacked size not measured), so a share of the guest memory is the system itself.

## 4. Question 4: devices (what works, what does not)

| Need | Works? | How, or why not | Tag |
|---|---|---|---|
| Display | Headless by default; browser UI on the Mac | The pixel stream of `iyakki-remote` over the UI port. Retina is the browser's job; the loopback link has bandwidth to spare. Optional native window later: `VZVirtualMachineView` over virtio-gpu, resolution set by us; automatic resize and Retina scaling for Linux guests are **from memory** only | design |
| Keyboard, mouse | Through the browser UI. A native window would use the framework's USB keyboard and pointing devices (guest has `USB_XHCI_PCI`, `HID`) | | design / from memory |
| Audio out | **Probably yes**: `virtio-snd` with the host as sink. Guest has `SND_VIRTIO=m`. IYAKKI's own mixer opens an ALSA card as today (`AUDIO-MIXER.md`) | Not verified that the 6.12 `snd-virtio` driver talks to the framework's device; first test of phase 5 | from memory |
| Microphone | **Probably yes** through the same sound device with a host input stream. macOS shows its microphone prompt to the **launcher process**; a command-line tool needs the microphone usage text in an embedded `Info.plist` or it is refused (from memory) | Test on the Mac; if it fails, the browser path below | from memory |
| Camera | **No guest camera device exists** in the framework (none in the device list of vfkit's page, which mirrors it). **Minimal relay, recommended:** because the UI is a browser on the Mac, the **browser** can open the Mac's camera and microphone with `getUserMedia` (macOS asks the browser, not us) and send frames/PCM up the existing WebSocket to the guest. That is the same feature as the backlog item "phone microphone relay" and the "media devices" item; one relay serves a phone, a Mac browser and this VM. **Not built; `iyakki-remote` carries only display down and input up today** (checked `REMOTE-PROTOCOL.md` title and section list only). A host-side vsock camera daemon (AVFoundation, frames over vsock to a V4L2 loopback device) is possible but needs `v4l2loopback`, an out-of-tree GPL module, so it is **not recommended**. For 1.0.0: declare the guest camera **unsupported**, and make the browser relay a post-1.0.0 item unless the owner wants it earlier | design |
| USB passthrough | **No.** The framework has an emulated XHCI controller and lets the host attach **disk images** as USB mass storage (vfkit: `usb-mass-storage`, image file or ISO, read-only option); there is no API to hand a physical host USB device to the guest (from memory; the vfkit page lists nothing of the kind). Workarounds: a shared folder, a network service, or `iyakki dev usb` style links | verified list, no-passthrough **from memory** |
| Clipboard | Native: needs the Spice agent port (macOS 13) and `spice-vdagent` (GPL) in the guest: **not recommended**. In the browser UI the clipboard is the browser's, so paste works through the page | design |
| File sharing | `virtio-fs` of chosen host folders, mounted read-only by default; read-write per folder on the person's choice. The launcher only shares folders the person passed, never `$HOME`. Interaction with `docs/SHARED-FOLDERS.md` (shares over HTTP) and `docs/HOST-FILES.md`: not read in detail; the virtio-fs mount is a new "host folder" that the Files tile would show | design |
| Network | NAT (`virtio-net`). The guest is not reachable from the LAN. Outbound works; Wi-Fi/Ethernet settings pages are meaningless in a VM and should be hidden by the `iyakki.host=vz` flag (same pattern as cloud mode's hiding table) | design |
| Display scaling / HiDPI | the browser; no guest work | |
| Power button / shutdown | the launcher asks the guest over the control channel to power off cleanly (sync the data disk), then stops the VM; `requestStop` (a power-button event) is the fallback and relies on the guest handling it, which is **not verified** | design |

### 4.5 The one control channel

A single vsock port (say 1024) between launcher and guest carries line-delimited JSON, ours, unknown keys rejected, 64 KB limit: host to guest `shutdown`, `time {epoch}`, `balloon {mb}`, `host_info`; guest to host `ready {version}`, `status`, `bridge_ports`. Same style as the VPN helper protocol. It replaces `qemu-guest-agent`, `spice-vdagent` and any cloud-init (the IYAKKI image has its own first-boot logic).

## 5. Question 3: the model bridge

### 5.1 Shape

```
 pod / chat / flows (guest)  ->  127.0.0.1:11434 (guest relay, AF_VSOCK to CID 2 port 11434)
                                      |  virtio-vsock (private to this VM)
 iyakki-mac (macOS process)  ->  allow-list check  ->  TCP 127.0.0.1:11434 on the Mac
                                      |
                          Ollama / LM Studio / llama-server / MLX (Metal), listening on loopback only
```

* **Host side:** the launcher registers a `VZVirtioSocketListener` for each allow-listed port and, per accepted connection, opens a TCP connection to **`127.0.0.1:<port>` only** (the destination is never configurable to another address) and copies bytes both ways. Allow-list default: 11434 (Ollama), 1234 (LM Studio), 8080 (llama.cpp `llama-server`, MLX-LM). Others only by `iyakki-mac bridge allow <port>`. A connection to a port not on the list is closed. No host listener on any interface exists: the Mac runner may stay on its loopback default, which also closes the hole that `MAC-MODEL-HOST.md` section 2 warns about.
* **Guest side:** a small relay in the existing binary (`iyakki vsock-relay`, supervised by `iyakki-core`): for each port the host announced as open in the `host_info` message, listen on guest `127.0.0.1:<same port>` and connect each client to `vsock CID 2 : port`. The existing `ollama-local` detection (`127.0.0.1:11434`) then finds the Mac's Ollama by itself, and an added provider `http://127.0.0.1:1234/v1` works with the code in `modelhost.rs` unchanged (loopback is accepted by `parse_target`). The port number is the same on both sides so that written docs and presets stay true; a clash with a model server in the guest itself is possible: the guest relay refuses to bind a port that already listens and tells the Settings page.
* **Pods:** loopback is not visible from pods. When the person presses "Publish to cluster" (existing function), the relay also listens on the node's cluster-facing address (the `cni0` address, **not verified for this image**) and the Service + Endpoints point there instead of at a LAN address. Off by default; the Firewall already accepts `cni*`/`flannel*` input (VPN plan, verified there). Without it a pod cannot reach the model; that is deliberate: a pod can be any image.
* **Fallback and the other modes keep working:** a Mac (or PC) elsewhere on the network is still added through the existing Mac / GPU host page. A person who prefers a runner on the LAN can use the NAT gateway address of the guest (default route); the guest finds it from its routing table, no discovery needed. This path is the one the owner's note mentions as "VM gateway address" in `MAC-MODEL-HOST.md` section 5. Its cost is that the runner must listen on the virtual network interface, i.e. not on loopback only. **vsock is the recommendation for exactly that reason.**

### 5.2 Authentication

"Authenticated, loopback-only on the host" is met in layers; none of them is magic:

1. **Channel:** vsock between one VM and its launcher; no other VM and no network host can open it. Any process in **this** guest can, as with any local Ollama.
2. **Launcher:** fixed destination `127.0.0.1`, port allow-list, a cap on concurrent connections (default 32), idle timeout, no logging of payloads. Refusal tests in section 9.
3. **Engine key where the engine has one:** `llama-server --api-key`, LM Studio's token. The key is typed into IYAKKI's existing provider form and kept as the vault reference `vault:...`; Ollama and MLX-LM have no key, so layer 1 and 2 are all there is.
4. **Optional hardening, not in v1:** the launcher parses the first HTTP request and injects a per-install bearer token that the runner has no idea about (only works with a proxy that checks it). Skipped because it needs a real HTTP parser in the launcher and does not help engines without a key. Decision 5 keeps it as a later option.

### 5.3 What the launcher tells the guest, and what changes in AI-HARDWARE

The control channel's `host_info` carries what only the host knows: chip name (`machdep.cpu.brand_string`, **from memory**), `hw.memsize`, the allow-listed ports and which of them currently accept a connection on the host (the launcher probes loopback with a 200 ms connect; it never sends data). The guest stores it in `/run/iyakki/host-bridge.json` (0644, no secrets).

Changes to `docs/AI-HARDWARE.md` and `aiprofile.rs` (to write, in phase 2):

* New source "host bridge": when `iyakki.host=vz` is on the command line and the file exists, the profile line reads **"No GPU in this virtual machine. Models run on the host: Apple M2 Pro, 32 GB unified memory, about 21 GB usable for models (the Mac's engines on the bridge: Ollama)"**. The usable memory uses the existing Apple rule applied to the **host's** memory, minus the VM's own memory because both live in the same pool (a 32 GB Mac with an 8 GB VM leaves less: show the rule's result minus the VM size, floor at 0).
* Tier and "fits: 7B Q4, ..." come from that number, so the model picker hint and the Models page are right. The existing "memory of a remote Mac is not read" limitation does not apply to the bridge host.
* The table row for tools: **Lemonade, Ollama (guest installs):** hidden, reason "Models run on the Mac through the host bridge; a runner inside this virtual machine would use the CPU and the VM's small memory"; **GPU for local models (Vulkan):** hidden; **NVIDIA driver:** hidden; **Use a Mac's GPU:** replaced by a **Host bridge** card (status of each port, test button, tokens per second through the existing `modelhost::test`); Kokoro and the rest unchanged (CPU voices). "Show all AI tools" still reveals everything.
* The ambiguity to keep honest: the guest cannot know the **Mac's GPU state**. The profile says what the host reports, nothing about Metal availability. The Test button measures it.
* Tests: fake `host-bridge.json` files in `aiprofile_tests.rs` in the same style as the fake `/proc` trees (8, 16, 32, 64 GB hosts, bridge down, no file, `iyakki.host` absent).

### 5.4 Model runners on the Mac (what the person installs)

Ollama (MIT), llama.cpp (MIT), MLX-LM (MIT) and LM Studio (free, proprietary) are the person's own downloads, as `MAC-MODEL-HOST.md` section 1 says. `iyakki-mac doctor` only **checks** which of the ports answer and prints the install hint; it installs nothing. One thing changes in that document: its "bind to your network address" advice applies to a Mac host used from other machines, **not** to the VM bridge, where the default loopback binding is the right one.

## 6. Question 5: licensing

| Thing | Who provides it | Licence | Shipped by us? |
|---|---|---|---|
| `iyakki-mac` launcher (source, Homebrew formula) | us | Apache-2.0 | yes (source and tap) |
| Rust crates `objc2*`, `block2` linked into it | crates.io | MIT / Zlib / Apache-2.0 (verified today) | compiled in; notices go into the NOTICE/SBOM like other crates |
| Linux arm64 kernel, initramfs, userland | us (Buildroot) | GPL parts with source offer through `legal-info/`, as for every image | yes (release assets `Image`, `rootfs.cpio.gz`) |
| Virtualization.framework and macOS | Apple | Apple's licence | **no**, system frameworks on the person's Mac, dynamically linked |
| Rosetta | Apple | Apple's | not used, not shipped |
| k3s | k3s project (Apache-2.0, from memory) | Apache-2.0 | no: downloaded inside the guest at the person's click, checked |
| Ollama, llama.cpp, MLX-LM, LM Studio | their projects | MIT / MIT / MIT / proprietary free | no: the person installs; we only talk HTTP |
| vfkit, Lima, UTM, QEMU | their projects | Apache-2.0 / Apache-2.0 / Apache-2.0 / GPL-2.0 | no: optional tools the person downloads for the prototype or fallback |

Rules kept: nothing from Apple is redistributed; no firmware (a VM needs none: there is no Wi-Fi or Bluetooth chip); "Apple", "macOS", "Apple Silicon", "M1" are used nominally with the usual disclaimer. Running a Linux VM on Apple hardware with Apple's own framework is the framework's intended use (Apple's documentation, from memory); this is not legal advice and **no legal text was read today**. A line for `LICENSING.md` will be proposed when the launcher exists, not before.

## 7. Question 6: install and update on macOS

### 7.1 Signing, notarisation, the Apple account

* **Own Mac (the owner's):** `codesign -s - --entitlements iyakki-mac.entitlements iyakki-mac` (ad hoc, no account, no cost) is enough to use Virtualization.framework. Entitlements needed: `com.apple.security.virtualization`; if the process is sandboxed or hardened, also network client and server (Lima lists both). Verified by Lima's Makefile and `vz.entitlements` fetched today. A `Info.plist` section embedded in the binary with `NSMicrophoneUsageDescription` is needed for the microphone prompt (from memory).
* **Other people's Macs:** a binary downloaded through a browser carries the quarantine attribute, and Gatekeeper refuses an unsigned or ad-hoc-signed program until the person overrides it in System Settings -> Privacy & Security (from memory; the exact steps changed in recent macOS releases). Files fetched with `curl` do not get the attribute (from memory), and **Homebrew** installs without it. Notarisation requires the paid Apple Developer Program (from memory: 99 US dollars a year; price not re-checked) and a Developer ID certificate. The App Store is not required and not wanted (the sandbox would block what the launcher does).
* **Result:** the Homebrew tap path needs **no Apple account**. A `.dmg`/`.pkg` for people who do not use Homebrew needs the account and is a later decision (decision 7).

### 7.2 Install

`brew tap <owner>/iyakki` (a user-side tap in the owner's GitHub or Gitea account; no cost), `brew install iyakki-mac`. The formula builds from source (needs Rust, which Homebrew pulls as a build dependency) or installs a bottle built on a Mac we control; either way it ad-hoc signs with the entitlement in the formula (the pattern vfkit and Lima formulas use, from memory). No notarisation. Home of the tap's formula source: a `Formula/iyakki-mac.rb` in the repository, kept in sync by the release script (**not built**).

First run (all in `iyakki-mac`, each step prints what it does):

1. `iyakki-mac doctor`: Apple Silicon yes, macOS version, free disk, RAM, entitlement present, which model runners answer on loopback.
2. `iyakki-mac init`: downloads the release pair `Image` + `rootfs.cpio.gz` from our release URL into `~/Library/Application Support/IYAKKI/images/<version>/`, checks SHA-256 and our release signature (the same ed25519 release key and `iyakki-relcheck` rules as the images; the format of the signed manifest is whatever the update path uses, **not read today**), creates the data disk as a sparse raw file (`truncate`, default 32 GB; APFS keeps it small) and writes `config.json` (memory and CPUs from section 3).
3. `iyakki-mac run`: starts the VM in the foreground; `iyakki-mac install-agent` writes a LaunchAgent (`~/Library/LaunchAgents/...plist`) to start it at login (opt-in).
4. `iyakki-mac open`: opens the default browser at the loopback UI port. First sign-in creates the first account as today.

### 7.3 Updates and the data disk

* The system is **two files per version**; an update is: download, verify, put into a new folder, switch a `current` symlink at the next start, keep the previous version for rollback. Never an in-place edit of a disk image.
* The data is the **separate disk** `data.img`, attached as the second virtio block device; the guest finds it by label `IYAKKI-DATA` as everywhere (`crates/iyakki-storage`). Updating the system never writes to it. This is the macOS counterpart of the upgrade-safety release gate (BACKLOG gate 5: "install and update tools never touch what they were not asked to touch").
* **Before a version change** the launcher makes an APFS clone of `data.img` (`clonefile`, instant, no extra space until the files differ; verified as a property of APFS in the vfkit page) as `data.img.pre-<version>`, so the person can roll back the data too, and removes it after N successful starts (default 2) or on request.
* **First boot of a blank data disk.** The guest today formats a *marked partition* after an install. A blank whole disk needs a new, narrow rule: with `iyakki.data=blank-ok` on the command line (set only by the launcher on a disk it just created) the guest formats `/dev/vdb` as ext4 labelled `IYAKKI-DATA` **only if its first 1 MiB is all zero and it has no partition table**, and never otherwise. Refusal test: a disk with any non-zero byte in the first MiB is left alone and the Settings page says so. (Code to write, in `iyakki-storage`; reviewed like the other disk-writing paths of `DISKS.md`.)
* Guest-side data migration across versions is the OS's own versioned-settings work (BACKLOG gate 5), unchanged.
* The window for in-guest updates (developer-mode `update-pack`) still works over the forwarded developer port; the launcher's file swap is the supported route.

## 8. Question 7: testing

**On a Linux PC / in CI (no Mac):**

* Pure logic of `iyakki-mac` behind the `Machine` trait with a fake: configuration and memory table (8/16/24/32/64 GB), the allow-list and the loopback-only destination, byte-copy relay under cut connections, idle timeout, connection cap, control protocol (oversize, unknown keys, wrong order), image manager (verify, atomic switch, rollback, pre-update clone through an injected clone function), LaunchAgent plist text. `cargo check --target aarch64-apple-darwin` through the existing `check-macos.sh` for the macOS-only module.
* Guest side: the relay with `AF_VSOCK` replaced by TCP behind a trait; `host-bridge.json` profile tests; the blank-data-disk rule on a loop device or a sparse file in a temp directory (as the other storage tests do).
* **QEMU aarch64, TCG** (existing scripts): proves the new kernel fragment boots, `VIRTIO_BALLOON`, `FUSE_FS`, `VIRTIO_FS` and `VSOCKETS` are present (read `/proc/config.gz` or `/proc/filesystems`), a vsock round trip when the CI host has `vhost-vsock` (`-device vhost-vsock-pci,guest-cid=3`, needs `/dev/vhost-vsock` on the host; **not verified** that the Gitea runner has it, otherwise skipped with a message), and the self-test markers. It proves the **guest**, not the Mac.
* **QEMU with `-accel hvf` on a Mac** (recipe B of `APPLE-SILICON.md`) is the stand-in for a real Apple Silicon run: same CPU, real timing, real page-size host. It proves the arm64 guest boots and runs k3s at native speed on Apple hardware. It does **not** prove anything about Virtualization.framework: different device models (virtio over PCI in QEMU; the framework's own presentation), different firmware, entitlements, vsock host API, balloon behaviour, NAT subnet, audio, sleep and wake.

**Needs a real Apple Silicon Mac (nothing here can be faked):** boot under the framework, console name (`hvc0`), time to the sign-in screen and RSS, the vsock listener and relay with a real Ollama (tokens per second through the bridge versus direct), balloon, virtio-fs mounts, `virtio-snd` sound and microphone prompt, ad-hoc signing and the entitlement, the Homebrew path, sleep/wake and clock fix, memory pressure with a large model on the host, k3s in the guest, save/restore if tried. The owner's stated hardware is an Intel MacBook, a Mac mini and a T2 MacBook (`AI-HARDWARE.md`: "the Intel MacBook and the Mac mini are the only devices"); `APPLE-SILICON.md` section 6 asks about the M4/M6 Macs and has no answer recorded. **Question for the owner:** is there an Apple Silicon Mac to test on, which model, memory and macOS version, and may an agent use it over SSH? A GitHub-hosted Apple Silicon runner would only build the launcher: nested virtualization is not available there for the M1 runners (from memory, **not verified**), and the project's CI target is Gitea.

An Intel Mac cannot run the arm64 guest. (The same launcher could boot the x86-64 image on an Intel Mac, from memory; out of scope.)

## 9. Security gate (v1.0.0 rule: no known security issue)

| Threat | Mitigation | Refusal test |
|---|---|---|
| A guest process reaches arbitrary host services through the bridge | Destination fixed to `127.0.0.1`; port allow-list; connections capped and idle-timed | try to connect a non-listed port (closed), a port in the list while the host has nothing listening (clean error, no hang), 200 connections (cap), a slow-loris peer (timeout) |
| Launcher control socket abused by another local user | `~/Library/Application Support/IYAKKI/run/control.sock`, mode 0600, directory 0700, peer UID check with `LOCAL_PEERCRED` (from memory) | wrong UID refused; oversize/garbage refused |
| UI reachable by other Macs | The launcher listens on `127.0.0.1` only; the guest UI binds to the guest loopback; no port forward from the LAN | bind address test |
| UI certificate warning (BACKLOG gate 2: no scary certificate warnings for our own services) | `iyakki-remote` makes a self-signed certificate; a browser warns on `https://127.0.0.1`. Options: (a) a small WKWebView window in the launcher that pins the VM's certificate (no warning, but then the launcher is a GUI app), (b) the launcher keeps a local CA in the Mac's Keychain (modifies the trust store: needs consent and is intrusive), (c) plain `http://127.0.0.1` through the launcher (browsers treat loopback as a secure context, but whether Safari accepts `Secure` cookies there is **not verified**; the machine login cookie is `Secure`). Decision 4 | measure in a browser on the Mac before choosing |
| Shared folder escapes | Only folders the person named; read-only default; no symlink-outside-root issue is expected at the framework level but is **not verified** | shared folder with a symlink to `/etc` read from the guest |
| Secrets | Engine keys only in the guest vault; the launcher stores none; control and relay never log payloads; `config.json` has no secrets | canary string through the relay, grep the launcher's log |
| Tampered update | SHA-256 + release signature before the symlink switch; rollback kept | bad hash, bad signature, truncated download |
| Data loss at update | Separate disk, clone before version change, blank-disk rule only on all-zero disks | the refusal test of section 7.3 |
| Pod abuses the GPU | The pod-visible listener is off by default and per port | pod with the listener off cannot connect |

## 10. Phases, sizes, risks

Sizes: S under 300 changed lines or one day, M up to about 1500 lines or about a week, L over that. Effort is for one person, estimates not promises.

| Phase | Work | Size | Needs a Mac |
|---|---|---|---|
| 0. Prototype | On a Mac: `brew install vfkit`; boot the existing `iyakki-aarch64.img` with `--bootloader efi` and `virtio-net,nat`, then the new kernel pair with `--bootloader linux` (needs the phase 1 kernel for vsock); record: time to sign-in, RSS, console name, NAT subnet, whether `iyakki.web` answers over the NAT address, Ollama through the NAT gateway, k3s starts. Writes `docs/BOOT-TEST.md` rows. No code | S | yes |
| 1. Image variant `apple-vm` | Kernel fragment additions (section 3), `build.sh` variant, assets `Image` + `rootfs.cpio.gz`, `verify-kernel-config.sh` against k3s needs, QEMU test additions; incremental kernel build only | S-M | no |
| 2. Guest side | `iyakki vsock-relay`, control channel client, `host-bridge.json`, AI profile source and the Host bridge card, `iyakki.host=vz` UI hiding (Wi-Fi, Bluetooth, brightness pages), blank-data-disk rule in `iyakki-storage`, balloon load, clean shutdown/time commands; about 1500 lines with tests | M | no |
| 3. Launcher | `crates/iyakki-mac`: `Machine` trait and fake; objc2 implementation (boot, disk, NAT, console log, balloon, vsock device, virtio-fs); relay and allow-list; control socket and CLI (`doctor`, `init`, `run`, `stop`, `status`, `open`, `bridge`, `update`); image manager; sleep/wake clock fix; about 2500 lines with tests. Step 1: darwin `cargo check` with objc2-virtualization; step 2: boot to a console log (the 5-day stop-loss for Swift fallback) | L | step 2 onward |
| 4. Packaging | Homebrew tap formula, ad-hoc signing with entitlements, LaunchAgent, `docs/MACOS-VM.md` user page, `LICENSING.md` item, FEATURES rows, KNOWN-ISSUES lines for the unverified | S-M | yes |
| 5. Devices | virtio-snd sound and microphone (test), virtio-fs host folders in the Files tile, optional browser-as-camera/microphone relay (L, shared with the phone relay) | M (L with relay) | yes |
| 6. Later | WKWebView window or menubar app, notarised `.dmg`, save/restore state, balloon on memory pressure, x86-64 image on Intel Macs, GPU-in-guest experiment | each M | yes |

Total to a usable 1.0.0 path (phases 0 to 4): roughly 4 to 6 weeks of one person's work, of which only phases 0, 3 (from step 2) and 4 need the Mac (**estimate**; the Mac days are the risk).

**Risks.** (1) No Apple Silicon Mac to test on: everything after the Linux-side work is blind until one is available. (2) objc2 ergonomics (stop-loss to Swift). (3) Framework behaviour differs from vfkit's docs on some macOS release (Lima's page already lists kernel/GRUB quirks on older Intel Macs). (4) The certificate-warning gate for the browser UI. (5) Memory: a small VM on an 8 GB Mac plus a model on the same memory is tight; the honest message is in the Settings page. (6) The browser UI is a pixel stream: latency and the lack of native clipboard/keyboard feel (the future web UI of `UI-OS-SPLIT-PLAN.md` step 9 would improve it; the launcher's vsock port mapping is the transport that connector can use). (7) Audio/microphone through `virtio-snd` is unverified end to end. (8) No camera in 1.0.0 unless the browser relay is built.

## 11. Questions still open (not resolved by reading)

* Does the `snd-virtio` driver in 6.12 work with the framework's sound device, and does a command-line launcher get the microphone prompt?
* Which console device does the framework give a Linux guest (`hvc0` or `ttyAMA0`)?
* What does the NAT give: subnet, gateway, DNS, and is the host address stable? (Only needed for the fallback path.)
* Does `cargo check --target aarch64-apple-darwin` compile `objc2-virtualization` without an Apple SDK?
* Does the k3s download in this tree choose arm64, and does the aarch64 kernel pass `check-config`?
* Which cluster-facing address do pods use to reach a node-local listener on this image?
* Is save/restore of machine state available for Linux guests with vsock and virtio-fs attached?
* How do the existing Files tile and `SHARED-FOLDERS.md` / `HOST-FILES.md` map onto a virtio-fs mount (not read in detail).

## 12. Owner decisions (with recommendations)

1. **Launcher language:** Rust over `objc2-virtualization` (recommended; stop-loss to a Swift shim after 5 days without a boot), or Swift from the start. Recommended Rust.
2. **Phase 0 with vfkit first?** Recommended yes: downloaded by the person, Apache-2.0, nothing shipped; it measures boot time, memory and the bridge before we write the launcher.
3. **Boot mode:** direct kernel + initramfs (recommended; no GRUB, update is a file swap, uses the uncompressed `Image` we already build) or EFI with the disk image (the same image UTM uses; keeps GRUB menu entries such as "safe graphics" and "hardware check", slower).
4. **UI and the certificate:** headless VM with the browser UI on the Mac first (recommended). Pick how to avoid the certificate warning: (a) later WKWebView window that pins the certificate, (b) local CA, (c) loopback `http`. Recommended: measure (c) in Safari, Chrome and Firefox in phase 0; if it fails, (a).
5. **Bridge transport and authentication:** vsock to the runner's loopback with an allow-list (recommended), engine API keys where they exist, no HTTP-level token in v1; the LAN-address route stays as the fallback in the existing Mac / GPU host page.
6. **Control surface:** one CLI, `iyakki-mac`, plus an opt-in LaunchAgent (recommended); a menubar app only if the owner wants one later (it needs a GUI app and is a new maintenance cost).
7. **Distribution:** Homebrew tap built from source, ad-hoc signed, no Apple account (recommended). A notarised `.dmg` later only if the owner accepts the Apple Developer Program fee.
8. **Device scope for 1.0.0:** declare unsupported: USB passthrough, native clipboard, guest camera. Try: audio out and microphone through `virtio-snd`, virtio-fs host folders. Browser-as-camera/microphone relay after 1.0.0 (shared with the phone relay) unless the owner moves it in. Recommended as listed.
9. **Defaults:** VM memory and CPUs by Mac size as in section 3 (k3s off by default on 8 GB), data disk 32 GB sparse, separate raw file, clone before each version change, blank-disk formatting only when the disk is all zero. Recommended yes.
10. **Test hardware and minimum macOS:** which Apple Silicon Mac, may an agent use it over SSH; minimum macOS 14 (recommended) rather than 13. Without a Mac, phases 0, 3 (from step 2) and 4 wait.

## 13. Proposed FEATURES.md rows (not applied; `F-BOOT-09` is being edited by another agent)

Format of the table: ID, Feature, Phase, Status, Pri, Progress, Tests, Notes. Proposed new prefix `F-MACVM-` (no such prefix exists today). All start as `planned` with `—` for tests, to be filled when built. F-BOOT-09 should say, in its note, that bare-metal is out of scope by the owner's decision of 2026-10-27 and point here.

| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |
|---|---|---|---|---|---|---|---|
| F-MACVM-01 | `apple-vm` image variant: arm64 `Image` + `rootfs.cpio.gz` with vsock, virtio-fs, balloon and `iyakki.host=vz`, for direct kernel boot in Virtualization.framework | P4 | planned | P1 | 0% | — | Kernel fragment additions in `docs/development/APPLE-SILICON-VM-PLAN.md` section 3; QEMU test proves the guest only. |
| F-MACVM-02 | `iyakki-mac` launcher: Rust over Virtualization.framework (`objc2-virtualization`), `Machine` trait with a fake, boot, disks, NAT, console log, clean shutdown, sleep/wake clock fix | P4 | planned | P1 | 0% | — | Apache-2.0, ours; ad-hoc signed with `com.apple.security.virtualization`; needs a real Apple Silicon Mac. |
| F-MACVM-03 | Model bridge: vsock relay from the guest's loopback to the Mac runner's loopback (Ollama, LM Studio, llama.cpp, MLX), allow-list, connection cap, no payload logging | P4 | planned | P1 | 0% | — | Reuses `modelhost.rs` and the provider config unchanged; refusal tests in section 9 of the plan. |
| F-MACVM-04 | AI hardware profile "models run on the host bridge": host chip and memory from `host-bridge.json`, tier and fits from the host's memory minus the VM, Host bridge card, Lemonade/Ollama/Vulkan/NVIDIA hidden with the reason | P4 | planned | P1 | 0% | — | Extends `docs/AI-HARDWARE.md`; fake `host-bridge.json` tests. |
| F-MACVM-05 | Data on a separate sparse disk with safe first-boot formatting (only an all-zero, partition-less disk), APFS clone before each version change, rollback of the system pair | P4 | planned | P1 | 0% | — | Counterpart of BACKLOG release gate 5; refusal test for a disk with data. |
| F-MACVM-06 | Install on macOS through a user-side Homebrew tap (builds from source, ad-hoc signed), `iyakki-mac doctor/init/run/open/update`, opt-in LaunchAgent | P4 | planned | P2 | 0% | — | No Apple account; notarised `.dmg` is a later decision. |
| F-MACVM-07 | Devices in the VM: audio out and microphone through `virtio-snd`, host folders through virtio-fs; guest camera, USB passthrough and native clipboard declared unsupported | P4 | planned | P2 | 0% | — | Browser-as-camera/microphone relay is a separate later row. |
| F-MACVM-08 | Test plan: pure launcher logic and the guest relay on Linux, QEMU aarch64 for the guest, QEMU+HVF as the Mac stand-in, a checklist for a real Apple Silicon Mac | P4 | planned | P2 | 0% | — | What each proves is in section 8; Mac results go to `docs/BOOT-TEST.md`. |

## 14. What I could not verify

* Nothing was built, run or booted; no Mac was used. Every device behaviour of Virtualization.framework is from the `vfkit` and Lima pages fetched today or from memory, not from Apple's reference (the Apple pages were not fetched).
* The licences: Lima, Colima, UTM, vfkit, libkrun, krunkit (GitHub API / licence files, 2026-10-27), Tart (FSL-1.1-ALv2 licence file, fetched), `objc2*` and `block2` (crates.io, fetched). **Not re-read:** QEMU's, k3s's, Ollama's, llama.cpp's, MLX's and LM Studio's licences (from memory), Apple's macOS licence terms for Linux VMs.
* Startup time, memory, binary and kernel sizes, module sizes: nothing measured.
* The aarch64 kernel `.config` actually built (only the fragment was read) and the k3s `check-config` result.
* The k3s download's architecture choice, the signed-manifest format of the update path, the Files/shared-folder interplay, `hostengines.rs` beyond its header.
* Notarisation cost and Gatekeeper behaviour on recent macOS releases, the microphone permission for a command-line process, the NAT subnet, `snd-virtio` against the framework, save/restore for Linux guests, nested virtualization on hosted Apple runners.
* Whether the objc2 darwin target can be type-checked from Linux without an SDK.
