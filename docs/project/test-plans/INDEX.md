# Beta test plans: index

These plans turn the 567 'class B' rows (code exists, needs a device or a real service) from `docs/project/retriage-2026-11-06.tsv` into checklists. Plans are grouped by what the tester has in hand, not by feature. Every row is in exactly one plan.

| Plan | Checks | Minutes | Who and what hardware |
|---|---|---|---|
| [mac-mini-2018.md](mac-mini-2018.md) | 104 | 679 | Main test machine. Anyone with the Mac mini, a monitor (touch monitor for the touch rows), keyboard and mouse. |
| [mac-mini-2018-apps.md](mac-mini-2018-apps.md) | 46 | 295 | Anyone with the Mac mini and the image installed or booted. No accounts needed. |
| [macbook-pro-t2.md](macbook-pro-t2.md) | 29 | 231 | Anyone with a T2 MacBook Pro and a USB-C stick. |
| [pc-uefi-amd.md](pc-uefi-amd.md) | 11 | 90 | Anyone with a spare UEFI PC or laptop, preferably with AMD graphics. |
| [pc-uefi-intel-nvidia.md](pc-uefi-intel-nvidia.md) | 4 | 45 | Anyone with a PC that has an NVIDIA card (4090 PC) or an Intel iGPU. |
| [pc-bios-and-qemu.md](pc-bios-and-qemu.md) | 7 | 78 | Anyone with a build PC (QEMU) and an old BIOS PC. |
| [apple-silicon-vm.md](apple-silicon-vm.md) | 3 | 45 | Anyone with an M-series Mac. |
| [phone-and-network.md](phone-and-network.md) | 43 | 235 | Anyone with phones (iPhone and Android), a second computer and a home Wi-Fi router. |
| [real-accounts-and-keys.md](real-accounts-and-keys.md) | 55 | 358 | Anyone who can create throwaway accounts. See 'what a throwaway account needs' below. |
| [live-models.md](live-models.md) | 58 | 395 | Anyone with a PC or Mac running Ollama (qwen3:8b, qwen2.5-coder:14b) or Lemonade. |
| [k3s-and-cluster.md](k3s-and-cluster.md) | 73 | 557 | Anyone who can start k3s on the Mac mini and has a PC for a second view. |
| [cloud-tenants-and-web.md](cloud-tenants-and-web.md) | 42 | 263 | Anyone with a real cluster (cloud mode) and browsers. |
| [usb-stick-and-install.md](usb-stick-and-install.md) | 27 | 200 | Anyone with spare USB sticks and a spare computer. |
| [browser-real-sites.md](browser-real-sites.md) | 36 | 260 | Anyone with the Mac mini (or a PC) and internet. |
| [accessibility-and-touch.md](accessibility-and-touch.md) | 29 | 165 | Anyone, including a first-time user. Best done by someone who did not build the screens. |
| **Total** | **567** | **3896** | |

Minutes are a rough guess (5 minutes a check, more for installs and long runs, plus 20 for setup). Reading the first-time notes: the Mac mini touch rows, the Lock menu, the time zone question, tab restore, the AUTH tile and the per-sound volume rows have not been seen on a real screen yet; look at them first.

## Suggested order for a first week

1. mac-mini-2018.md (the main machine; start with the cold-boot touch rows and Reset touch screen).
2. mac-mini-2018-apps.md, accessibility-and-touch.md (same machine, same day).
3. phone-and-network.md, usb-stick-and-install.md (needs phones and spare sticks).
4. macbook-pro-t2.md, pc-uefi-amd.md (repeat the laptop freeze), pc-uefi-intel-nvidia.md, pc-bios-and-qemu.md, apple-silicon-vm.md.
5. browser-real-sites.md, live-models.md, k3s-and-cluster.md.
6. real-accounts-and-keys.md, cloud-tenants-and-web.md (need accounts and a cluster).

## Other files

- [RESULTS-TEMPLATE.md](RESULTS-TEMPLATE.md): how to report.
- [generate_plans.py](generate_plans.py): rebuilds all plans from the TSV and FEATURES.md and checks the 567 count.

## How results flow back

1. The tester fills the Result line of each check and copies the template from RESULTS-TEMPLATE.md.
2. The owner pastes the filled template (and the attached logs) to an assistant.
3. The assistant updates the FEATURES.md rows: a pass moves the row to verified on that hardware (with the date and image version in the notes); a fail adds the symptom and the log reference to the row and to KNOWN-ISSUES; a note is kept as text.
4. The matrix test (`feature_matrix.rs`) must still pass after the update. Failed rows stay in the next plan run.
5. Only the owner decides when a row counts as done for the beta.
