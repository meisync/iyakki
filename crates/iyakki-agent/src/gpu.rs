//! GPUs for local models (F-GPU-01, F-GPU-03; docs/GPU-K3S.md): what graphics hardware this machine has and what state its driver is in.
//!
//! * **Detection is read-only sysfs** (`/sys/bus/pci/devices`, with the DRM nodes each device owns, `/dev/dri`) and table-driven:
//!   vendor ids to names and the device ids of the cards people own to marketing names, in this file, no `lspci`, no pci.ids. Every
//!   function takes the root of the file system it reads (`/` on the machine, a folder of fake files in the tests).
//! * **The verdict for stage 1 (Vulkan)**: Mesa brings the Vulkan driver inside the pod image, so the host needs only a kernel
//!   driver bound to the card and a render node: AMD (amdgpu, RADV), Intel (i915 or xe, ANV), NVIDIA Turing and newer through nouveau
//!   (NVK, needs the GSP firmware). A card that has none of those gets the reason in plain words.
//! * **What uses it**: pods that ask for the GPU through the extended-resource name or the CDI annotation ([`pods_using_gpu`]).
//! * **Test prompt**: one chat request to the local model server, with the answer's tokens per second ([`test_prompt`]).
//!
//! Nothing here starts a process. The kernel log is passed in as text; the cluster is asked through the caller's `kubectl`.

pub use iyakki_shared::gpu::*;
