# IYAKKI 0.1.0-alpha.8: release notes

*An OS for agents.* The release after [alpha.7](RELEASE-NOTES-0.1.0-alpha.7.md), cut on 2026-11-21 (tag `v0.1.0-alpha.8`, commit 0f987f0c), the same day as alpha.7. The plan was `docs/development/ALPHA-8-PLAN.md`: small apps onto the SDK, evidence from machines, no new features. The release gate (`release-green --image`) passed on this commit and the signed lean image booted in QEMU.

**Preview for developers and testers.** Most of what is new was checked by unit tests, fakes and QEMU; each feature's row in [FEATURES.md](FEATURES.md) says what was not tried on a real machine.

## The numbers

On 1,760 active rows: implementation 99.6 percent, test 67.1 percent (1,733 rows with all code written and unit-tested). No Test number moved in this release: the new evidence tooling only raises a number with proof, and no scenario has been run live yet.

## What is new

* **Small apps are tile crates.** Disks (`iyakki-tile-disks`, drive model in `iyakki-disks-model`), Dash (`iyakki-tile-dash`), Learn (`iyakki-tile-learn`, `iyakki-study-model`), Checkup and Mesh (`iyakki-tile-checkup`, `iyakki-tile-mesh`, plain mirrors in `iyakki-tile-api`), after Board, Firewall and Hostfiles in alpha.7. K8s has its model crate and its port. The ratchet that keeps old shell handles out of migrated tiles now matches the word `Services` as a type or path only.
* **One test tool.** The Doctor "Manual tests" tab reads the autotest runner's checklist files, writes evidence in the runner's schema (`human/<ROW>.<clause>.<machine>.json`), accepts owner, admin or person passes with a recorded 10 percent sample for the owner, reads imported evidence, shows the runner's screenshots and has an import chat tool.
* **Gate fixes:** poison-safe locks in the Learn port, generated files kept fresh, computed clamps use the safe helper.

## What did not make it

Doctor, Runs, Scan, LinuxWin, the K8s tile, DevTools (lanes finished after the gate; they are in alpha.9), the acting-person binding for the UI socket, the eight row splits, Flows and Git tiles, and the browser tiles for Edit and Cli. The decisions taken on the owner's behalf are in `docs/development/ALPHA-8-DECISIONS.md` and the cut schedule is `docs/development/ALPHA-8-TO-12-SCHEDULE.md`.
