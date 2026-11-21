# Beta goal (draft, 2026-11-21, for the owner to confirm)

**For:** the owner and anyone deciding when IYAKKI may be called beta.
**You will get:** what "beta" means for this project, the measurable gates, what is explicitly not required, and the order of work from alpha.8.

## What beta means

A person who is not on the team can download an image, install it on supported hardware, use the daily features, and get updates, without asking anyone for help, and what does not work is written down. Beta is not "everything finished": it is "nothing important is silently broken".

## Gates (all must hold on one candidate commit)

1. **Implementation 100%** on every active row: all code written and unit-tested (reached for the split and SDK rows in alpha.7; F-ARCH-08 and F-BOOT-11 stay parked and are reported with and without).
2. **Test 75% or more** (alpha.7 ended near 67%), with evidence kept apart by level: unit, QEMU, browser, each paired machine, human. Rungs are raised only by the autotest evidence rules; QEMU evidence never sets rung 100.
3. **Release gate green**: `release-green --image` PASS with no flakes, no entry in `known-flaky.txt`, licence check green, public CI green.
4. **Real hardware**: the candidate image installed and used on the four machines we own (Mac mini 2014, Mac mini 2018, Lenovo, HP) and on Apple Silicon through the VM route, using the Manual tests tab of the Doctor app; every `BUG` known issue open at that point is closed or listed as a known limit with a workaround.
5. **Upgrade and rollback proven**: an installed alpha.7 system updates to the candidate by the signed update path and can go back; a power cut during the update leaves a bootable machine (QEMU fault-injection runs plus one hardware run).
6. **Security**: the UI socket review items closed (acting person bound to the socket login, approval tickets), the five-layer doc matches the code, no secret in logs, signing keys held outside the repo, SBOM and licences regenerated.
7. **Documents**: install guide, recovery guide, known limits, what is experimental (wake phrase, client family) labelled in the product and the site.
8. **Public repository**: GitHub is the primary repository (history audited), CI green on the candidate, issue and PR templates, contributing and security policy in place.

## Not required for beta

Official store payments, ARM boards (KV260, Pi 5), Asahi native Apple Silicon, Secure Boot route 2 (own shim signed by Microsoft), cloud multi-tenant, Matrix end-to-end rooms, JPEG 2000 and Indic PDF recognition, in-house video decoders. They stay in the matrix as parked or partial, never deprecated.

## Order of work from alpha.8

1. Automated evidence first: the autotest runner, scenarios for the first 55 rows, nightly schedule (on the owner's go-ahead), the Doctor Manual tests tab in use by the team.
2. Real-device rounds with the alpha.7 image on every machine, fixes as found, one build host at a time.
3. Close the open `BUG` and `UNPROVEN` known issues by evidence or by fixing.
4. Security items above, then upgrade/rollback fault injection.
5. App migration onto the SDK in batches only while the test levels stay green: small apps in alpha.8, medium apps in alpha.9, the six large ones (Voice, Store, Chat, Ide, Browser, Settings) after that; the large ones are not a beta requirement unless the owner says so.
6. Browser speed on real engines with real GPUs (the harness exists; measurement is manual).
7. Release candidate: gates 1 to 8 on one commit, then tag `0.1.0-beta.1`.
