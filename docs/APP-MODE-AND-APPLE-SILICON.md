# App mode and Apple Silicon: what is possible, what is not (plan; the Mac model host bridge is built, nothing else yet)

Written 2026-10-21 from the owner's request: an "app mode" that can run Debian/Ubuntu store apps, Windows `.exe` files and Apple store apps, and on Apple Silicon Macs (M1 to M6) a mode that installs k3s and lets models use the Mac's GPU. This page is the honest feasibility check; it is not legal advice, and the dates and chip support below are from the project's knowledge and must be re-checked before any promise is made.

## 1. App mode

| Source | Possible? | How | Licence / legal notes |
|---|---|---|---|
| Debian / Ubuntu packages (`.deb`, their repositories) | Yes, as a container or chroot | IYAKKI itself is musl + BusyBox, so glibc programs do not run on the host. A Debian or Ubuntu root file system is downloaded for the person at their click (official `debootstrap`/cloud-image/OCI base), run rootless in a sandbox (k3s pod or a namespaced chroot), packages installed with `apt` inside it. Graphical apps need a display bridge: a headless Wayland compositor (Weston, MIT) inside the sandbox, captured into an IYAKKI window like the web engine does. | Debian and Ubuntu repositories are free to use; the images are downloaded, not shipped. Ubuntu's name and logo are trademarks (do not rebrand). Each package keeps its own licence. |
| Windows `.exe` | Partly | Wine (LGPL) inside the same Debian/Ubuntu sandbox, installed at the click. No Windows is shipped or needed. Many programs run; games and anything with kernel drivers or anti-cheat often do not. | Wine is LGPL. The Windows programs are the user's own and under their own licences. We must not ship Microsoft files (fonts, DLLs); Wine's replacements are used. |
| Apple store apps (iOS/iPadOS/macOS) | **No legal or technical route** | Apple apps run only on Apple's operating systems and are licensed for Apple hardware. There is no permitted emulation path on a Linux image, and the App Store is not available outside Apple devices. | Recommend dropping this from the plan. The honest alternative is the web: most Apple-store apps have web versions that the browser tile can run. |

Order if the owner approves: (1) the Debian/Ubuntu sandbox with `apt` and a terminal (**built, phase 1, untried on a device**); (2) a display bridge for graphical apps; (3) Wine on top; (4) a Store section "Linux apps" with curated entries.

Open questions: disk space (a Debian root is 150 MB to 1 GB plus packages), whether to allow network access per app, and whether a pod in the cluster is better than a plain sandbox.

## 2. Apple Silicon Macs: k3s and the GPU

IYAKKI on Apple Silicon would run on the Asahi Linux kernel and drivers (see `docs/APPLE-SILICON.md`). What that gives for models:

| Chip | GPU on Linux | Models on the GPU |
|---|---|---|
| M1, M2 (and Pro/Max/Ultra) | Supported by Asahi: open Mesa driver, Vulkan (Honeykrisp) and OpenGL | Vulkan backends (llama.cpp, Ollama Vulkan) can use it, slower than Apple's Metal |
| M3, M4, M5, M6 | Not supported by Asahi when this was written (support is still in progress and there is no promise for future chips) | None from Linux; do not promise it |

So "M1 to M6" cannot be promised for Linux GPU use. Two honest routes:

1. **Native, M1/M2 only:** IYAKKI on Asahi, the same GPU Quick install (Vulkan) as on PCs, k3s installed by the existing Quick install. Needs the Apple Silicon variant to exist (planned, not built).
2. **Bridge to macOS, any M-series:** keep macOS on the Mac and run the model there with Apple's Metal (Ollama, LM Studio, llama.cpp or MLX, all permissive or free to use); IYAKKI, or a k3s cluster running on another machine, reaches it as an OpenAI-compatible endpoint, which IYAKKI already supports as a model provider (it worked with a local network model and Lemonade). IYAKKI can add: a guided "Add a Mac as a model host" page (what to install on the Mac, the address and port to enter, a test prompt, tokens per second), optional discovery on the local network (mDNS if available), and the right authentication. This is the only route that uses M3 to M6 GPUs today. If IYAKKI itself runs as a virtual machine on the Mac, the virtual machine cannot use the Metal GPU, so the same bridge to the macOS side applies (a local port or vsock).

3. **A light Linux VM on macOS for k3s (owner's idea, 2026-10-21), any M-series:** a small Linux virtual machine on the Mac (Apple's Virtualization framework, or a tool built on it such as Lima (Apache-2.0) or Colima (MIT); `krunkit`/libkrun (Apache-2.0) is the variant that can also give containers a GPU) runs k3s, and IYAKKI installs and manages it. Models then run in one of two places:
   - on the macOS host with Metal (route 2), with pods in the VM calling the host through the VM's gateway address; this is the fast path and works on every chip;
   - inside the VM on the GPU, only through a virtual GPU (libkrun with a Vulkan-to-Metal layer such as MoltenVK, Apache-2.0). Container tools on macOS use this today for llama.cpp, but it is slower than native Metal and depends on the tool and macOS version: treat it as an option to test, not a promise.
   Plain Virtualization-framework guests have no Metal compute access, so without the virtual GPU a model in the VM runs on the CPU. Costs: the VM's memory is taken from the Mac (set a limit), and the macOS side needs a small helper app or script that IYAKKI downloads for the person, never ships. IYAKKI would be the Linux guest or the manager of one, to be decided.

Recommendation: do route 2 first (small, works on every chip, legal), add route 3 (the light VM for k3s) as the way to run the cluster on a Mac that stays on macOS, keep route 1 as the Apple Silicon variant work already listed in `docs/APPLE-SILICON.md`, and say plainly in the product which chips use which route.

## 3. Decisions of the owner (2026-10-21)

**Status (2026-10-21, later the same day):** decision 2 is built as phase 1, [`LINUX-APPS.md`](LINUX-APPS.md): the Debian/Ubuntu sandbox (IYAKKI menu → Linux apps, Store → Setup → Linux apps, CLI `linux`, AI IDE `sandbox`) with downloads checked against the distribution's hashes, a namespaced `pivot_root` runtime built from system calls (kernel needs already met by the IYAKKI kernels), `apt` jobs, folders and a network switch. Decision 1 is built as image apps ([`BUILD-YOUR-OWN-IMAGE-APP.md`](BUILD-YOUR-OWN-IMAGE-APP.md)). Graphical apps (the display bridge of section 1) are a design only (LINUX-APPS.md section 11); Wine is not started. Nothing has run on a device yet. Decision 3 (the Mac bridge) is in `MAC-MODEL-HOST.md`.


1. **Apple store apps and every app-store plan are dropped for now.** Instead IYAKKI offers ready-made *image apps* (container images the person can run) and lets the person build their own (the AI IDE and `tile_ship` already cover custom apps).
2. **Debian/Ubuntu sandbox: yes**, first. Wine (Windows `.exe`) comes after it, on top.
3. **Apple Silicon: the bridge first** (a macOS model host reached over the network, any M-series), because Asahi does not support every chip; the light VM for k3s second; native Asahi stays a later option for M1/M2.

## 4. Status (2026-10-21)

- **Route 2, the bridge to macOS: built**, tested with fake servers only, not yet with a real Mac. See `docs/MAC-MODEL-HOST.md`: guided page (Store -> Setup -> Use a Mac's GPU, Settings -> Models -> Mac / GPU host), Test with tokens per second, Add to Models as `mac-<nickname>`, optional scan of the local /24 on click, a Service + Endpoints in `iyakki-ai` for pods and a pod-based test. The same page serves any other GPU computer.
- Route 3 (the light VM for k3s): documented above, **not built**.
- Route 1 (native Asahi, M1/M2 only) and the Debian/Ubuntu sandbox: unchanged (see `docs/APPLE-SILICON.md`; the sandbox is another task).
