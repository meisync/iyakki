# Alpha.8 to alpha.12: one cut a day, beta checkpoint on Friday 27 November (owner order, 2026-11-21)

**For:** the owner and the team. **You will get:** what each alpha carries, the cut date, and what has to be true to cut it.

Owner order: "We finish the pending items by Friday. Alpha.12 by Friday." Today is Saturday 21 November. Each cut needs `release-green --image` PASS on the cut commit (about 1 to 1.25 hours). **Owner rule (2026-11-21, replaces "one gate a day"): no limit on features per day; 20 hours a day for feature implementation and test fixes, and a gate window of four hours, 20:00 to 24:00 (local time), for the full gates.** Inside the window the gate can be run again after a fix (a full gate with image takes about 1 to 1.25 hours, so up to three runs fit). Outside the window nothing runs `release-green`; the strict `-D warnings` check, the seven contract tests, `compat_surfaces`, `facet_defaults`, the clamp and poison scans, the generator `--check`s and the filtered tests of touched crates run any time. The alpha under test is frozen from 19:45: lanes merge into the next alpha's branch until the window closes. A cut is made on the first gate PASS inside the window. A cut is only started when the lanes of that alpha have merged. A lane that is not finished moves to the next alpha with its exact remainder written in its row Notes; nothing is cut with a half-merged lane. Fixes found by a gate go on the alpha being cut first and are merged forward after it passes.

| Alpha | Cut | Scope | Cut when |
|---|---|---|---|
| alpha.8 | Sun 22 Nov | Small apps on tile crates (Disks, Dash, Learn, Checkup, Mesh, Scan, LinuxWin, Doctor, Runs, K8s, DevTools); 66 autotest scenarios and 50 human checklists; the decisions record; SEC3 (acting person bound to the socket login, approval ticket); the eight row splits and the machine-kind picker; wasm32 Edit and Cli tiles; pilot autotest run on the image | gate PASS with image on the merge of E5 to E8, SEC3, SPLIT1 |
| alpha.9 | Mon 23 Nov | Medium apps on the SDK: Flows (job traits already done), Git, System, Rag, Lab, Inbox, Tales; K8s port-forward facet and exec in the Terminal view; first real autotest results folded into Test numbers by the evidence rules | gate PASS with image |
| alpha.10 | Tue 24 Nov | Large apps, ports first: Voice, Store, Chat, Ide: port traits and model crates, tile extraction where the code is ready; Browser and Settings: port design and the first slice. Ratchets only shrink | gate PASS with image |
| alpha.11 | Wed 25 Nov | Evidence: autotest nightly on the owner's go-ahead, hardware rounds on the four machines with the Manual tests tab (a person does the checklists), upgrade and rollback fault-injection scenarios (power cut during update), fixes for what the rounds find, KNOWN-ISSUES triage | gate PASS with image; round results recorded |
| alpha.12 | Fri 27 Nov | Beta candidate checkpoint: the beta gates of `BETA-GOAL.md` audited one by one on a single commit, install guide, recovery guide, known limits, SBOM and notices refreshed, release notes, public export; the beta label follows only when every gate holds | gates audited, gate PASS with image |

## What is honestly at risk by Friday

* **Test 75 percent** (now 67.1): the evidence rules raise a number only with proof, so it depends on people doing the hardware rounds and on rows being split; I expect 69 to 72 by Friday, not 75.
* **Large apps** (Voice 3.2k lines, Store 8.3k, Chat 8.3k, Ide 13.9k, Browser 17.2k, Settings 20.4k): ports and first extractions in alpha.10, finished extractions may move to after beta; they are not a beta requirement.
* **Hardware rounds** need a person at each machine; the Manual tests tab and the checklists are ready, the time is not mine to give.
* **Upgrade and rollback** with a power cut is new work (fault injection in QEMU first, hardware second).
* One gate a day assumes no gate failure; a failure costs a re-run (about 1.25 hours each).
