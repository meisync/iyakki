# AI hardware: which AI tools this machine shows

IYAKKI works out an honest "AI hardware profile" of the machine it runs on and uses it to show, recommend or hide the AI tools
in Store -> Setup. The point: a machine with no GPU is not offered a GPU runtime it cannot use, and a machine that can run a 32B
model says so. Nothing is ever impossible: an administrator can switch on **Show all AI tools** (Settings -> Cloud mode, key A)
and every tool is listed again, marked "Not recommended here" with the same reason.

Code: `crates/iyakki-agent/src/aiprofile.rs` (the profile and the one table of rules), `aiprofile_remote.rs` (the cluster's GPUs),
tests in `aiprofile_tests.rs`, `aiprofile_remote_tests.rs` and `crates/iyakki-shell/src/apps/store_setup_ai_tests.rs`.

## What counts as memory for a model

Not the machine's headline numbers, but the memory a model can really use:

| Machine | Counted | Rule |
|---|---|---|
| Apple Silicon (M1 to M6, base, Pro, Max, Ultra) | unified memory, minus what macOS keeps | two thirds of the memory up to 36 GB, three quarters above (macOS's default limit for the GPU; a 32 GB Mac gives about 21 GB, a 64 GB Mac 48 GB) |
| Discrete GPU (NVIDIA, AMD, Intel Arc) | dedicated video memory, minus a reserve | VRAM minus **1.5 GB** for the screen's framebuffer, the compositor and other programs (a 12 GB card gives 10.5 GB) |
| Integrated GPU of an AMD processor (APU) | the shared pool the driver exposes | the larger of the BIOS carve-out (`mem_info_vram_total`) and the shared pool (`mem_info_gtt_total`), counted once, never more than RAM minus **4 GB** |
| Intel integrated graphics (UHD, Iris Xe) | nothing | the kernel exposes no pool to count; models run on the CPU |
| No usable GPU | system RAM minus **4 GB** | shown, but never "capable": the CPU is slow |

The carve-out and the shared pool are different things. The carve-out is a slice of RAM the firmware sets aside for graphics at
boot (often 512 MB); the shared pool (GTT) is system RAM the driver may map for the GPU on demand (by default up to half of RAM).
They come from the same memory, so they are not added up.

"Excluding video mem/card" is read two ways, both implemented: **video memory is not counted as is** (the reserve above
leaves the screen and other programs their share), and **small cards do not count** (an LLM-capable GPU has at least **6 GB usable**).
If the owner meant something else, the constants at the top of `aiprofile.rs` are the one place to change.

## Thresholds

| Constant | Value | Meaning |
|---|---|---|
| `MIN_LLM_GB` | 6 GB | usable memory for a GPU or Apple chip to be "LLM capable" (7B at 4 bits is about 5 GB plus 1 GB of conversation cache) |
| `DISCRETE_RESERVE_GB` | 1.5 GB | kept free on a discrete card |
| `SYSTEM_RESERVE_GB` | 4 GB | kept for the system (the same as the Mac model host guide) |
| `CONTEXT_GB` | 1 GB | added to a model's size when asking if it fits |

Tiers (usable memory, models are 4-bit: 3B about 2.5 GB, 7B 5 GB, 13B 9 GB, 32B 20 GB, 70B 42 GB):

| Tier | Usable | Words | Cards that land here |
|---|---|---|---|
| None | under 6 GB | too little memory for models of useful size | 2, 4, 6 GB cards, an 8 GB Mac |
| Small | 6 GB and up | runs 7B models comfortably | 8 GB card |
| Medium | 10 GB and up | runs 7B and 13B models comfortably | 12 and 16 GB cards |
| Large | 21 GB and up | runs models up to 32B comfortably | 24 GB card, a 32 GB Mac |
| XL | 43 GB and up | runs models up to 70B comfortably | 48 GB card, a 64 GB Mac |

"fits: 7B Q4, 13B Q4" lists the sizes that fit. Several GPUs: the best one is counted (models are not spread over cards) and all
are listed in the notes.

## How it is read

Read-only files, under a root (`/` on the machine, a folder of fake files in the tests); a missing or garbled file is "unknown", never a crash:

- Apple Silicon: the device-tree model of Asahi Linux (`/proc/device-tree/model`, "Apple MacBook Pro (14-inch, M2 Pro, 2023)")
  names the chip and tier; otherwise the DMI model id (`Mac14,9` and so on). Known: M1 `MacBookAir10,1`, `MacBookPro17,1`,
  `MacBookPro18,x`, `Macmini9,1`, `iMac21,x`, `Mac13,x`; M2 `Mac14,x`; M3 `Mac15,x`; M4 `Mac16,x`; M5 `Mac17,x`; M6 `Mac18,x`
  (**assumed by the numbering, not verified**); a newer id is "Apple M-series", chip unnamed. The tier (Pro, Max, Ultra) comes from the id
  only where it is certain; otherwise it is left out. The memory is `MemTotal`.
- PCI display devices (`gpu::detect`), amdgpu's `mem_info_vram_total` and `mem_info_gtt_total`, Intel's `lmem_total_bytes`.
- NVIDIA tells no total through sysfs (`nvidia-smi` is not shipped): the card's model gives it (a table of cards that come in one
  size), else a PCI memory window of 4 GB or more (resizable BAR), else **"memory size unknown"**. A GPU of unknown size is shown and
  **hides nothing**.

IYAKKI runs on Intel Macs today. Apple Silicon is the Mac model host bridge ([MAC-MODEL-HOST.md](MAC-MODEL-HOST.md)) and the future native
install ([APP-MODE-AND-APPLE-SILICON.md](APP-MODE-AND-APPLE-SILICON.md)): on an Intel machine the profile describes the Intel machine, and the
number of Mac model hosts set up in Models is added to the line ("2 Mac model hosts set up"). The remote Mac's memory is not read.

## The table: what each profile shows

| Tool | Capable GPU or Apple chip | GPU under 6 GB | CPU only | Apple Silicon |
|---|---|---|---|---|
| Lemonade, Ollama | shown | hidden | hidden | shown (not recommended on an 8 GB chip) |
| GPU for local models (Vulkan) | recommended | hidden | hidden | hidden (Metal, not Vulkan) |
| NVIDIA driver | shown when NVIDIA is the best GPU | hidden | hidden | hidden |
| Use a Mac's GPU | not recommended (own GPU) | recommended | recommended | hidden (this is the Mac) |
| Elasticsearch | not recommended under 6 GB of RAM | | | |
| Kokoro, Indic-TTS, everything else | shown (the voices run on the CPU) | | | |

Reasons are plain words on the card and in the popup: "Hidden because this machine has no GPU with 6 GB or more; local models would
run on the CPU and be very slow." In cloud mode the table is not consulted (cloud mode keeps its own rules). The launcher has no tile
that needs LLM capability (Chat works with cloud models; there is no local model host tile), so no tile is hidden or de-emphasised there.
Core tiles (Settings, Files, Store, Disks) are never touched.

## A remote cluster with a GPU

If the cluster IYAKKI works against is **remote** (not on this machine) and has a suitable GPU, Lemonade, Ollama and the other
model servers are shown even when this machine has none, with the reason: "Shown because your cluster 'lab' has 2 x NVIDIA GeForce
RTX 4090 (24 GB each)." The best of the local and the remote GPU decides.

- The cluster's GPUs are read with one read-only call, `kubectl get nodes -o json` (the existing runner): `nvidia.com/gpu`,
  `amd.com/gpu`, `gpu.intel.com/i915` in allocatable and capacity, and the labels of GPU Feature Discovery and the AMD labeller
  (`nvidia.com/gpu.product`, `nvidia.com/gpu.memory` in MiB, `amd.com/gpu.vram`). A cluster GPU keeps **1 GB** free for its runtime
  context. GPUs with no memory label are "GPU found, memory unknown": shown, nothing hidden.
- Never blocks drawing: the screens read a cache; a refresh runs on a thread when due (60 s after a good answer; after a failure
  30 s, doubled up to 10 minutes). A cluster that does not answer keeps its last value, marked "last known".
- Which cluster decides the gating: the one chosen for the workspace that answers and is not on this machine (the Dev cluster resolution registers it).
- **Every cluster of the list** (`clusters::gpu_summary`, `clusters::refresh_gpus`, `aiprofile_remote::clusters_cache`): each remote cluster of Settings → Clusters is read the same way, on a thread, with the same back-off, and shows as a badge in the cluster switcher and on the Clusters page (`GPU 2 x NVIDIA GeForce RTX 4090 (24 GB each)`; ` (last known)` after a failure; no badge for no GPU). The Clusters page's Test feeds the same cache from its own `get nodes`. Clusters on this machine are not asked.
- The installs on **this** machine (Vulkan runtime, NVIDIA driver) stay tied to this machine's hardware. Model servers deploy as
  before (Review and deploy); only the gating changed. There is no separate GPU operator or device plugin card to gate.

## Where it shows

- Store -> Setup: cards hidden or marked, the note on a card ("Not recommended here: ..." / "Shown because ..."), the "Recommended" star.
  The tiles page draws the facts line (`This machine: ...`, `Machine::ai_line`) under the filters and, when tools are hidden, the sentence
  "2 AI tools are hidden because ..." with a **Show all AI tools** link for an administrator (the same audited switch as Settings -> Cloud mode,
  key A; a user sees the sentence and is told to ask an administrator).
- Settings -> Models: the row "This machine" with what fits ("fits: 3B Q4, 7B Q4, 13B Q4"). The model picker shared by the chat, the AI IDE's chat and TALES shows one more hint line, "This machine: fits: 3B Q4, 7B Q4, 13B Q4. Add one in Settings, Models." (or, for a CPU-only machine, that the models above run elsewhere): a suggestion sized to the usable memory; no configured model is ever left out of the list because of it.
- Settings -> Cloud mode: the row "This machine (AI)" in plain words, and the admin switch **Show all AI tools** (key A, default off, kept in
  the machine settings, written to the audit log).
- Plain words, for example: "Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models", "NVIDIA GeForce RTX 3060, 12 GB: about
  10.5 GB usable for models", "AMD Radeon 780M (Phoenix) (shared): up to 8 GB usable for models", "No GPU: CPU only, 15.5 GB RAM".

## What was not measured

Everything is tested with fake files and a fake `kubectl`. No Apple Silicon machine, no AMD machine and no NVIDIA machine was
available: the Intel MacBook and the Mac mini are the only devices. The device-id tables, the Apple model-id table, the usable-memory
rules and the cluster labels follow vendor documentation and are not checked against real hardware. M6 ids are a guess from the numbering.
If a real machine is misjudged, switch on Show all AI tools and report the model id.
