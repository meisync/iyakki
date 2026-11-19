# UEFI PC with Intel and/or NVIDIA graphics

Plan file: `pc-uefi-intel-nvidia.md` | Checks: 4 | Estimated time: about 45 minutes (0.8 hours)

Use `RESULTS-TEMPLATE.md` to report. Lines marked with (A), (code) or 'add ...' in the row text are developer tasks: skip that part and test the rest.

## What you need

- PC with an NVIDIA card (nouveau/NVK path) and/or an Intel iGPU machine.
- The IYAKKI PC image on a USB stick.
- Network cable.

## Safe start

Boot from the stick; do not install. Nothing on the PC disk is touched unless a step says so in bold.

## How to collect logs

Use developer mode on the test machine (Settings -> Developer mode; approve the pairing on the machine). From the paired computer:

- `iyakki dev diag` : one bundle with state and recent logs. Attach it to every failed row. It holds no secrets (a row checks this; if you see a key in it, tell the owner and do not share it).
- `iyakki dev logs --system` : the system log. Attach the lines around the failure.
- `iyakki dev dmesg` : the kernel log, for boot, Wi-Fi, sound and USB problems.
- The data-folder logs: files under `/var/lib/iyakki/log` (for example `engine.log` for the browser and `kmsg-<boot-id>.log` after a crash). On a machine with no network, copy them to a USB stick from the Files tile.
- A photo of the screen helps for any display, freeze or layout problem. Say how long after the action it happened.

## Order of checks

1. Boot, firmware, graphics entry.
2. GPU in a pod (needs k3s running).
3. Hidden-tool reasons on the tiles page.

The checks below are already in this order. Do not skip a failed early row: write it down and go on.

## Checks

Fill the Result line: pass, fail or note. For a fail, attach the files named in the row.

- [ ] F-AIHW-01 AI hardware profile (`aiprofile.rs`, docs/AI-HARDWARE.md): what a model can really use: Apple Silicon unified memory (M1 to M6 from the DMI model id or the Asahi device tree, Pro/Max/Ultra where certain; two thirds up to 36 GB, three quarters above), discrete GPU video memory minus 1.5 GB (amdgpu sysfs, Intel `lmem_total_bytes`, NVIDIA by the card's model or a large PCI window, else 'memory size unknown'), the shared pool of an AMD APU (larger of carve-out and GTT, at most RAM minus 4 GB), RAM minus 4 GB for CPU only; LLM capable at 6 GB usable; tiers None/Small/Medium/Large/XL (6/10/21/43 GB) in plain words and 'fits: 7B Q4, 13B Q4'; several GPUs: the best counted; garbage files never panic: do this: Read the profile on an Apple Silicon machine, an AMD GPU PC and an NVIDIA PC; compare to real memory -> expect this: it works as the row says; the open point is: 'Fake files only: no Apple Silicon, AMD or NVIDIA machine was available'; M6 ids are a guess; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-AIHW-02 Show, recommend or hide the AI tools by profile, one table (`aiprofile::rule`): Lemonade, Ollama, GPU runtime (Vulkan), NVIDIA driver, Mac model host, Elasticsearch (RAM); a plain reason on the card and popup ('Hidden because this machine has no GPU with 6 GB or more ...'); a GPU of unknown size or no profile hides nothing; cloud mode keeps its own rules; Store -> Setup uses it (`Machine::shows`, `heavy_note`, `recommend`, `ai_line`, `hidden_ai_note`): do this: Look at the tiles page on the Mac mini and the 4090 PC; confirm hidden-tool reasons read right -> expect this: it works as the row says; the open point is: Tiles page tested at five sizes, 'not looked at by a person'; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-GPU-01 GPU for local models in k3s, stage 1 (any vendor, Vulkan): Store → Setup card "GPU for local models (Vulkan)" (group Local models, a detached job): finds the GPUs (read-only sysfs, in-house PCI table), checks the driver (amdgpu, i915/xe, nouveau on Turing+), opens the render nodes, deploys a generic device plugin (Apache-2.0, pulled by the cluster) that advertises them as `iyakki.dev/gpu`, proves it with an unprivileged smoke pod that lists the Vulkan devices (Mesa from the pod image), and gives Lemonade the GPU; says plainly which GPU was found and which pods use it: do this: Run the Setup card on an Intel iGPU machine and on the 4090 PC with nouveau/NVK; see Vulkan0 in a pod -> expect this: it works as the row says; the open point is: Tried end to end with a Radeon 780M; 'Not yet on Intel or on NVIDIA through nouveau/NVK'; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-GPU-04 PC image (variant `pc`, `pc-x86_64` in `/etc/iyakki-release`): the macweb recipe for ordinary x86-64 PCs (`./run.sh build-pc-layered`, no compile, output `target/buildroot-pc`) with the whole GPU firmware set (NVIDIA GSP for Turing, Ampere and Ada, all AMD, all Intel) and common PC Wi-Fi, Bluetooth and Ethernet firmware; a boot entry that turns nouveau off for the NVIDIA driver; new kernel options (nouveau GSP by default, `HSA_AMD`, newer Intel pin control): do this: Rebuild the kernel with the options, boot the pc image on the 4090 PC and check firmware, Wi-Fi and the GPU entry -> expect this: it works as the row says; the open point is: 'The image is built; it has not been booted on a PC yet'; kernel options need a rebuild; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

