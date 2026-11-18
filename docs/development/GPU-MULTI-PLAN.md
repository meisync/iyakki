# More than one GPU in the dev cluster (AMD and NVIDIA together)

Status: **plan**, decided 2026-10-30, not built. Builds on the GPU steps in [`docs/GPU-K3S.md`](../GPU-K3S.md) (F-GPU-01 and F-GPU-03).

## What the owner wants

* A computer with two GPUs (the test machine: a Lenovo Legion Pro 5 16ARX8 with the AMD Radeon integrated graphics and an NVIDIA GeForce RTX
  laptop GPU) uses **both** for local models in k3s.
* The person **chooses**: one card, the other card, or **both with the model split across them**. Nothing is split unless they ask.

## Where it stands today

* Stage 1 (Vulkan): one generic device plugin advertises every DRM render node as the same resource, `iyakki.dev/gpu` (8 shares of each). A pod
  that asks for `iyakki.dev/gpu: 1` gets *a* GPU; it cannot ask for a particular one or for two different ones. Lemonade is patched with
  `iyakki.dev/gpu: 1` and `LEMONADE_LLAMACPP=vulkan`.
* Stage 3 (NVIDIA): NVIDIA's own device plugin and CDI give a pod `nvidia.com/gpu`. It has **never run on a real NVIDIA card**; the first run, on the
  Lenovo (2026-10-30), failed to unpack NVIDIA's `.run` (the archive is zstd-compressed and its own helper is a glibc program); fixed in
  `gpu_nvidia::extract_makeself_zstd`, not yet proven past that step.
* On that Lenovo the NVIDIA card has no DRM render node until the NVIDIA modules start with their firmware, so Vulkan sees only the AMD graphics.

## Design

1. **One resource per GPU, not one pool.** The plugin lists each GPU separately and names it by what it is: `iyakki.dev/gpu-0`, `gpu-1` ... with the
   card's name, vendor and memory shown on the GPU page. A pod can then ask for exactly the card it wants, or for two.
2. **A "Use for models" choice per GPU** on the GPU page (and `iyakki gpu use ...`): `on` or `off` for each card, and a **Split** switch that is
   offered only when two or more cards are on. The saved choice lives with the other GPU settings on the data volume.
3. **What a pod gets, by choice:**
   * one card on: the pod asks for that card's resource; `llama-server` is told which Vulkan device to use (`--device VulkanN`);
   * both on, split off: nothing changes, two model servers can each pin one card (the person decides in the model's own settings);
   * both on, split on: one pod asks for both resources and `llama-server` gets `--split-mode layer --tensor-split A,B`, with the weights set from
     each card's memory (the bigger card takes the bigger share) unless the person types their own.
4. **The NVIDIA side needs its own Vulkan driver in the pod.** Mesa (RADV) drives the AMD card; the NVIDIA card needs NVIDIA's Vulkan ICD from the
   stage that stage 3 already downloads. The pod mounts that stage (as the CUDA device plugin does) and the loader sees both ICDs.
5. **Lemonade** keeps one backend setting; for a split it must run the Vulkan backend. The Deployment is patched from the choice, never by hand.
6. **Safe defaults.** A single-GPU machine behaves exactly as today. Split is off until switched on. Mixed cards run at the slower card's pace: the
   page says so in one line before the switch is turned on.

## What has to be true first

1. The NVIDIA install finishes on the Lenovo (unpack, modules with the GSP firmware, device nodes, device plugin). Until then the NVIDIA card has no
   device for a pod to hold, and the split cannot be tested.
2. A render node exists for the NVIDIA card (`nvidia-drm` with `modeset=1`), or Vulkan reaches it through `/dev/nvidia*` and the ICD file.

## Lemonade: "Run on" per model (owner request 2026-10-31)

A choice next to the Lemonade model settings: **Automatic**, **CPU only**, one entry per GPU (for example "AMD Radeon (Raphael)", "NVIDIA GeForce RTX ...") and,
when two or more cards are on, **Both (split)**. CPU only = no GPU layers (`-ngl 0`) and no GPU resource in the pod; one GPU = that card's own resource plus
`--device VulkanN`. Wanted per model (a small model on the CPU, a big one on the NVIDIA card); to check: whether the shipped Lemonade takes these per model
or only per backend. Order of work: (1) NVIDIA power: the card idles at about 20 W because the device plugin keeps it awake, so a manual on/off switch
that stops the plugin and unloads the driver; (2) one resource per GPU and a pod test on each card; (3) the "Run on" choice.

## NVIDIA idle power (measured 2026-10-31, Lenovo Legion Pro 5)

Driver 595.99.02, `NVreg_DynamicPowerManagement=0x02`: "Runtime D3: Enabled (fine-grained)", `power/control` = `auto`, but `runtime_status` stays `active`
(suspended 1.6 s against 252 s active in 4 minutes). Suspect: the NVIDIA device plugin's NVML health checks keep the card open. To prove: stop the plugin and
read `runtime_status` again.

## Not decided / to measure

* Whether the NVIDIA Vulkan ICD works inside the musl host's container with the generic plugin, or needs NVIDIA's plugin (CDI) for that card.
  The test is `llama-server --list-devices` in a pod that holds both cards.
* The tensor-split weights: memory-proportional is the starting guess; measure on the Lenovo.

## Tests (when built)

* the plugin manifest names one resource per render node and keeps `iyakki.dev/gpu` as the total for old pods;
* the choice (`on` / `off` / `split`) round-trips and turns into the right pod resources and `llama-server` flags, in fakes;
* a one-GPU machine produces exactly today's manifest.
