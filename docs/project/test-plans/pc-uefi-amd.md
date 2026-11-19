# UEFI PC (AMD or any other): boot, Wi-Fi cards, firmware, laptop keys

Plan file: `pc-uefi-amd.md` | Checks: 11 | Estimated time: about 90 minutes (1.5 hours)

Use `RESULTS-TEMPLATE.md` to report. Lines marked with (A), (code) or 'add ...' in the row text are developer tasks: skip that part and test the rest.

## What you need

- A UEFI PC or laptop that may be booted from USB (Secure Boot setting noted).
- The IYAKKI PC image on a USB stick.
- Network cable. For firmware rows: a PC with the matching extra card (Wi-Fi/Ethernet) named in the row.
- A second Wi-Fi card for the hotspot row.

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

1. Boot and display.
2. Network and firmware cards.
3. Laptop keys and suspend.
4. Everything else.

The checks below are already in this order. Do not skip a failed early row: write it down and go on.

## Known open problem: freeze after opening youtube.com with no network

A laptop (AMD Radeon 610M, 2560x1600) froze about 20 seconds after youtube.com was opened with no network.
Repeat it before anything else on this machine:

- [ ] EXTRA-FREEZE Repeat the freeze: boot the image, make sure there is no network (unplug Ethernet, Wi-Fi off), open the browser tile, open youtube.com, wait 60 seconds. -> expect the page shows an error or a retry message and the screen keeps responding; if it freezes, **photograph the screen** (the whole screen, including any clock or cursor), note how many seconds after opening it froze, and whether the mouse still moved; if it fails, copy: after the power cycle, `iyakki dev diag` output, the newest `kmsg-<boot-id>.log` and `engine.log` from /var/lib/iyakki/log, and `iyakki dev logs --system`.
  Result: ______ (pass / fail / note)

- [ ] EXTRA-FREEZE-SAFE Safe graphics mode (Settings -> Browser -> Graphics is on Automatic): boot the image on the laptop and open the browser tile, then open any page in the full engine (for example youtube.com). The browser starts on the graphics card. -> expect it to run, or the computer to freeze. If it freezes, power-cycle (hold the power button), sign in and open the browser again: expect a message at the bottom of the screen saying "The last session stopped right after the browser started on this graphics card, so the browser now draws in software (slower). Settings, Browser, Graphics changes this." and pages that load in software. Then open Settings -> Browser: the Graphics row says "Automatic: software because the last session stopped right after the browser started on this graphics card" and has a "Try hardware again" button. Restart the laptop cleanly (power off from the menu) and open the browser again: the row still says software (not forced back) and, after that clean session, offers to try the card again. Also ask the chat "how is the browser drawing?" (browser_graphics_get) and "switch the browser to software" (it must ask on a card each time). Attach `engine.log` (look for the `graphics mode:` and `graphics: hardware` lines) and `sessions.tsv` from /var/lib/iyakki/log, and the audit line `browser.graphics`.
  Result: ______ (pass / fail / note)

(These extra steps have no row ID and are not part of the 567-row count.)


## Checks

Fill the Result line: pass, fail or note. For a fail, attach the files named in the row.

- [ ] F-KBD-05 Power/Sleep key opens a Power off / Restart / Cancel card (accounts that may power off; before sign-in it opens the login screen's own card, F-SHELL-35); nothing happens until a choice; WiFi/Bluetooth/airplane keys switch rfkill with a visible notice, waiting a moment so a kernel that switches by itself is not undone: do this: Press power key and Wi-Fi/airplane keys on a laptop **(can destroy test data or stop the machine: read the step twice)** -> expect this: it works as the row says; the open point is: Not tried with a real power button or rfkill device; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-KBD-16 Sleep on the power cards (signed in: Power menu, power key; login screen): a Sleep button and the key `s`, shown only when the kernel offers a sleep state (`/sys/power/state` lists `freeze` or `mem`, and the program runs as root); the session is locked first (signed out, which seals the vault) so the screen asks for the password on wake; the state written is `freeze` when offered, else `mem`, after quiet sound, clock-floor save and sync; the session is not marked clean (it resumes); audited as `power.sleep`; the kernel fragment adds CONFIG_SUSPEND and CONFIG_PM_SLEEP (needs a kernel rebuild): do this: Enable suspend in kernel and test on a PC laptop; label T2 sleep unsupported -> expect this: it works as the row says; the open point is: Not tried on any hardware; kernel suspend not built; T2 wake is known Linux problem KI-073; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-DRIVERS-31 Firmware pack job engine (`fwpack.rs`, `iyakki quick-install fw-qualcomm-bt` (or `fw-realtek-net`, `fw-broadcom-net`, `fw-atheros-carl9170`) with `--from FOLDER`, `--accept-licence`, `--force-hardware`): hardware by PCI/USB ids, the vendor licence accepted first (record keyed to the SHA-256 of the exact licence texts of the pin), files and licence texts fetched over https from a pinned linux-firmware commit on git.kernel.org (resumable, redirects only inside the host), each checked for size and SHA-256 against the generated table (`pin-firmware.sh`; one wrong byte installs nothing), installed as a driver pack on the data volume (linked at every start, survives restarts and in-place stick updates; the licence texts kept with the files), the driver loaded again, "Reboot to finish" only when the driver is built in, did not load or has no device bound (never restarts by itself), plain result headline; offline route from a folder or stick against the same table: do this: Run quick-install fw-realtek-net etc. on a PC with the matching card and see it load -> expect this: it works as the row says; the open point is: Fake network and fake /sys only; nothing run on machine with the hardware; id tables from memory; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines).
  Result: ______ (pass / fail / note)

- [ ] F-DRIVERS-32 Store, Setup, Hardware tiles for the four firmware packs (new `Item` variants and `Group::Firmware`): shown only where the matching hardware is, never in cloud mode, never in "Install all recommended"; the card says what is lost without the file; **Read the licence and install** opens a form (vendor, licence, a folder or stick with the file picker, "I have read the licence and accept"); on accept the acceptance is recorded and `firmware.accept` goes to the audit log, then the job starts; administrators only; **Reboot to finish** asks "Restart this machine now?" and restarts through the normal host path, only after a yes: do this: Open Setup Hardware cards on a PC with a matching card and view at the standard sizes -> expect this: it works as the row says; the open point is: Form and tile not drawn at every window size or seen on a screen; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines).
  Result: ______ (pass / fail / note)

- [ ] F-DRIVERS-50 Hardware firmware, detection (`fwextra.rs`): the files this computer asks for and lacks, from the kernel log (`Direct firmware load for X failed`, `firmware: failed to load X`, `Falling back to sysfs fallback for`, looser lines only when a word is a file of the release; requests are kept in `fwextra/seen.txt` so a rolled-over log does not forget them), the driver and PCI address from the line, the card from `/sys` (Wi-Fi, Ethernet, graphics, Bluetooth by class; a few marketing names such as AX200), `.ucode` version families collapsed to the first version the release has, grouped by vendor with each file's licence from WHENCE: `Free to redistribute with notice`, `Restricted to the vendor's hardware` (MediaTek, Marvell, Qualcomm, Broadcom, NVIDIA), old hexadecimal-format wording, or a vendor agreement; NVIDIA GSP shown on its own with what it needs: do this: On a PC with a missing-firmware card check the card lists the real file names from dmesg -> expect this: it works as the row says; the open point is: Fixtures only; kernel log shapes from memory; never run on a computer lacking firmware; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines).
  Result: ______ (pass / fail / note)

- [ ] F-DRIVERS-51 Hardware firmware, source and install (`fwextra.rs`, `dist/firmware/manifest.txt`, `dist/scripts/firmware-notes/pin.sh`): pinned linux-firmware tag 20261019 (commit a2dcb924, snapshot sha256 f37337e6... of kernel.org's tarball, size and sha256 of 4649 files and 134 licence texts from the real tarball); only the needed files and licence texts are fetched one by one from git.kernel.org at the pinned commit (https, redirects outside the host refused by `fwpack::fetch_resume`, resumable), each checked for size and sha256; a folder, a USB stick (`firmware-extra`, an unpacked release, or the release tarball read selectively after a whole-file sha256 check) or the developer-link staging folder gives the same bytes; links, odd names and traversal are never followed; nothing is fetched or installed before the licence list of exactly that selection is accepted (a changed list asks again); free space, memory-only data volume and size cap are checked; installed as the driver pack `fw-hardware` with the licence texts and WHENCE entries; drivers reloaded where safe, never a graphics driver, otherwise `Restart to finish`; a remove action deletes the pack and the kept download; audit lines without secrets: do this: Install the hardware firmware pack on a PC with a card needing it and reload the driver -> expect this: it works as the row says; the open point is: Never installed on a real computer; unverified that modprobe -r/modprobe brings the card up; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines).
  Result: ______ (pass / fail / note)

- [ ] F-DRIVERS-52 Hardware firmware, getting the files onto a machine whose Wi-Fi has no firmware (`fwextra_net.rs`, `fwextra_cli.rs`): the card lists four ways in the order of detection (Ethernet with internet, a cable to another computer that shares its internet, a USB stick, files pushed from the other computer) each with one plain line, `plug in an Ethernet cable` when Wi-Fi is the only network; **the other computer's internet over the cable** after the click on the device only: address 10.77.0.2/30 and routes to the allowed host and two resolvers (no default route unless `use for everything`), names resolved through the cable with a built-in DNS query, a marked hosts-file line, all removed on Stop, failure and at the end of the job; **`iyakki usb share-internet [--device NAME] [--uplink IFACE] [--apply or --undo]`** on the other Linux computer prints exact `ip`, `sysctl` and nftables commands (one table of its own, the cable only), runs them only with `--apply` as root (never sudo) and `--undo` restores forwarding; macOS and Windows steps are documentation only; **`iyakki firmware fetch --for DEVICE [--usb-link]`** on the other computer downloads and verifies the files and pushes them with `iyakki dev files push` into `~/firmware-extra` on the device, which verifies again and installs only after the click there; `dist/scripts/firmware-notes/fetch.sh` does the same by hand and a test compares its file list with the item's: do this: Get firmware onto a Wi-Fi-less PC through each of the four ways, at least Ethernet-share and USB stick -> expect this: it works as the row says; the open point is: Nothing verified over a real cable; firewall, Thunderbolt link, ICS not tried; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines).
  Result: ______ (pass / fail / note)

- [ ] F-HW-02 Real microphone / camera capture (cpal, v4l features): do this: Check if cpal/v4l path is still used; if not merge into camera/audio rows, else run on a PC with mic and camera -> expect this: it works as the row says; the open point is: Compiles; never run against real devices; may be superseded by devctl/camera code; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and the sound lines from `iyakki dev dmesg`.
  Result: ______ (pass / fail / note)

- [ ] F-DASH-04 DuckDB connector through the `duckdb` command (not linked): `-json -batch -no-stdin -readonly` on a database file or on CSV, Parquet and JSON files named in the SQL, run by the argument-array runner with a timeout and an output cap, the row limit by wrapping the query, files confined to the account's folders, a missing program says to install DuckDB from the CLI tile (catalog entry `duckdb`, MIT licence, never installed silently); the choice against linking the crate is justified in the docs: do this: Run dash::live_tests::duckdb_live with IYAKKI_LIVE_DUCKDB on a PC with the real duckdb CLI -> expect this: it works as the row says; the open point is: Notes: duckdb flags and download name never run against a real DuckDB; if it fails, copy: `iyakki dev diag` output, `iyakki dev logs --system`, and `iyakki dev kube logs <pod>` for the pod that failed.
  Result: ______ (pass / fail / note)

- [ ] F-JOBS-04 External coding agents (Claude Code and others) get the six job tools through the policy bridge with the same approvals, refusals and audit lines, and the generated agent instructions tell them to call `mcp__iyakki__job_start` for anything expected to take over about 30 seconds instead of a foreground shell, not to poll with sleep or until-loops, and to use `job_wait`: do this: On a PC with the user's own Claude Code, run a long build and see it call mcp__iyakki__job_start; confirm vendor setting names from vendor docs -> expect this: it works as the row says; the open point is: 'NOT tried with a real Claude Code session'; hook setting not generated (vendor names unconfirmed); if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

- [ ] F-LINAPP-02 Linux apps: base downloads at the person's click, never shipped: Ubuntu 22.04/24.04 (`ubuntu-base` tarball checked against the folder's SHA256SUMS over HTTPS) and Debian 12/13 (`debian:<suite>-slim` from Docker Hub pulled by our own registry client; manifest digest recorded, optional pin, every layer checked); https-only redirects to known hosts, free-space check before the transfer, licence and trademark notice: do this: From a PC fetch real Ubuntu and Debian bases through the page; owner picks Debian pin policy; GPG check stays out of scope or is added in-house -> expect this: it works as the row says; the open point is: 'the real servers were never contacted'; GPG signatures not verified; no Debian pin in the release table; if it fails, copy: `iyakki dev diag` output and `iyakki dev logs --system`; a photo of the screen if it is a display problem.
  Result: ______ (pass / fail / note)

