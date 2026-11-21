# Alpha.8 plan (draft, opened 2026-11-21)

**Target:** Test 75% with evidence from machines, kept apart by level; fixes for what the tests find. No new features unless the owner opens a row. Goal after alpha.8: the beta gates in `BETA-GOAL.md`.

Carry-over from alpha.7 (cut 2026-11-21; see the cut notes):
1. **Autotest** (`./run.sh autotest`, `docs/development/AUTOTEST.md`): pilot batch A, then batches B to E from the plan (about 55 rows, +1,150 points for the first 40). Nightly schedule written, enabled only on the owner's go-ahead.
2. **Manual tests tab** in the Doctor app: the team goes through the 25 human checklists on each machine; results exported and merged into `human/` evidence.
3. **Real-device rounds** on the Mac mini 2014 and 2018, Lenovo and HP with the alpha.7 image; fixes as found (KI entries by the triage rule).
4. **Open code**: F-ARCH-07 (the named-owner pairs decision), F-SPLIT-03 booted-device run, F-SDK-34 touch-screen run and the wasm32 tile build for Edit and Cli (`tiles::builtin()`).
4a. **Small apps onto the SDK (owner 2026-11-21: small in alpha.8, medium in alpha.9)**: Firewall, Disks, Dash, DevTools, K8s, Learn (size S in `APP-MIGRATION-INVENTORY.md`), plus finishing the port-migrated ones that are one step short of a tile crate (Runs, Doctor, Checkup, Mesh, Scan, LinuxWin, Site, DevTools engine). Each needs its leaf model crate and port first; the inventory is regenerated after every batch. Every moved app keeps all its tests green and gets autotest scenarios.
5. **Owner decisions waiting**: D-SEC1-SHARED and D-SEC1-APPROVED (bind the acting person to the socket login), D-SPLIT-07-RATE, D-SDK-34-03, D-BROWSERPUSH-SERVICE, D-ARCH-07-TYPES and D-ARCH-07-DISKTOOLS, QEMU as real evidence for software-only rows, public export timing.
6. **Parked** (never deprecated): F-ARCH-08, F-BOOT-11, F-BROX-26 (GPU card path in the shell), browser JSON viewer and cookie editor, incremental layout, legacy PDF CMaps, JPEG 2000, in-house video decoders.

7. **Alpha.9 (preview, owner 2026-11-21)**: medium apps onto the SDK: Flows (job traits done), Git, System, Mesh, Rag, Lab, Inbox, Edit, Cli, Tales. Large apps (Voice, Store, Chat, Ide, Browser, Settings) after alpha.9, ordered in the beta plan; they need their ports designed first.
