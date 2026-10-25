# Import from another IYAKKI drive

**Status: DESIGN ONLY (2026-10-24, owner request). Nothing here is built.** Tags: **EXISTS** (read in code or docs, path given),
**GAP** (missing), **PROPOSED** (owner decides). No code, script, or `FEATURES.md` was changed to write this.

> **Licence reasoning, plainly.** The rules for firmware in section 7 are the **owner's working rule** (2026-10-23 and 2026-10-24, no lawyer
> involved; `LICENSING.md` items 7 and 13, `docs/RESTRICTED-FIRMWARE.md`, `docs/FIRMWARE-LICENCES.md`). They are **not legal advice** and have
> not been checked by a lawyer. Where a licence text says something this design cannot honour, the file is not copied.

## 1. The case

A person has a second IYAKKI drive (a stick, or an internal disk of another machine) and plugs it into a machine that runs its own IYAKKI
install. They want to bring over what is theirs, without cloning a drive and without surprises. The source is only ever **read**.

## 2. Correction to the premise: what `mac-secret` is

The request calls `mac-secret` "the per-volume key" of the vault. In the code it is **not**: `/var/lib/iyakki/mac-secret` is 32 random
bytes that key the HMAC that derives private Wi-Fi/Ethernet MAC addresses (`crates/iyakki-devctl/src/mac.rs` header, `docs/ADMIN-GUIDE.md`
line 271). The vault is **self-contained** (`crates/iyakki-agent/src/vault.rs` header): secrets are sealed with a random 256-bit data key
(XChaCha20-Poly1305); that key is stored wrapped once per **key slot**, each slot protected by an Argon2id key derived from that slot's
password; files are `vault/system.afv` and `vault/users/<user>.afv` (`services.rs` lines 783 to 787). So:

* a vault file can be moved to another drive as it is and opened there with the same password; there is no per-volume key to carry;
* `mac-secret` must **not** be imported: it is device identity. Copying it would make two machines derive the same "private" address on the
  same network. The target keeps its own (or makes its own).
* "Export/import with the person's password" is therefore the right shape, and mostly exists: the import reads each source vault with the
  person typing the password of the slot, re-seals each secret into the target vault (a new data key, the target's slots).

## 3. Where the data lives (so the preview can name it)

Data folder `/var/lib/iyakki` = the `IYAKKI-DATA` ext4 volume (`docs/ADMIN-GUIDE.md` "Data locations"; `docs/DATA.md` is not about this).
Known items, with the code that owns them:

| Item | Path under the data folder | Owner of the format |
|---|---|---|
| Settings | `settings.json` (system) and per-user files (names not enumerated here: **phase 0 task**, diff a used drive) | `crates/iyakki-agent/src/settings.rs` |
| Accounts | `users.json` (0600; password hashes, roles, scopes) | `ADMIN-GUIDE.md`, `iyakki user` |
| Audit log | `audit.log` (0600, hash chain) | `ADMIN-GUIDE.md` "audit log" |
| Vaults | `vault/system.afv`, `vault/users/<user>.afv` | `vault.rs`, `services.rs` |
| Documents, chats, runs, skills, knowledge | `docs/`, `sessions/`, `runs/`, `skills/`, `kb/` | ADMIN-GUIDE.md table |
| Home folders | `home/<user>/` (bound onto `/home`; Pictures, Videos, Workspaces, tool logins such as `.claude`) | ADMIN-GUIDE.md |
| Browser profile | `web-profile/<user>/` (0700; cookies, local storage, cache) | `docs/FULL-BROWSER.md`, `webengine.rs` `profile_dir` |
| Installed store items | `installed.json` records (`pkg.rs`, `store/install.rs`), `cli/` (downloaded tools, e.g. `cli/bin/7zz`, k3s), apps, driver packs `firmware/<pack>/` | Store |
| Restricted-firmware packs | `firmware/fw-<name>/` (+ licence texts under `licences/fw-<name>/`), acceptance in `fwpack/fw-<name>/accepted.json` | `fwpack.rs` (`accepted_path`, `work_dir`) |
| Apple Mac firmware | packs `firmware/mac-wifi-brcmfmac`, `firmware/mac-bt-broadcom`, `firmware/mac-facetimehd`; job work folders `mac-firmware/` (download, `state.json`, `last.json`) and `camera-firmware/` | `macfw.rs`, `camfw.rs`, `docs/MAC-WIFI-BLUETOOTH-SETUP.md`, `docs/MAC-CAMERA.md` |
| Dev cluster | `k3s-dev/` | `k3sdev.rs`, `docs/K3S-DEV.md` |
| Device identity and network | `mac-secret`, `wifi.json`, `net-mac.conf`, `tls/` | `mac.rs`, `ADMIN-GUIDE.md` |
| Boot machinery | `iyakki/root/`, `iyakki/patches/`, `boot-prev/`, `boot-next/`, `stage1-kmsg.log`, `boot-lean.log`, `crash.log` | never importable |

## 4. Category rules

"Default" is what the preview ticks. Every category is a separate tick-box; nothing is imported that is not ticked.

| Category | Rule | Default | Notes |
|---|---|---|---|
| Settings | Offer **per key**, not the whole file. Keys that name this machine (host name, display, audio device, network, time zone detected, GPU) are shown as "this machine" and unticked. Secrets in settings are references (`vault:NAME`, `file:/abs/path`), never values (`vault.rs` header); a reference to a vault secret not imported is listed as "will not resolve" | ticked: preferences; unticked: machine-specific | schema-checked; unknown keys from a newer version are shown and skipped |
| Per-user json files | Only for accounts that are imported or already exist by **name** | with the account | unknown files are not copied (allow-list of file names, section 6) |
| Documents and home (`home/<user>/`, `docs/`, `sessions/`, `runs/`, `kb/`) | Copy into the same-named local account's home, under a folder `Imported from <label> <date>/` unless the person picks "merge" | ticked | uid/gid are remapped to the local account (the source's numbers mean nothing here); modes kept, setuid/setgid dropped; sizes shown |
| Users and accounts | Import **a user** (name, role, scope) as a new account; password hash is copied **only** if the person confirms the same password (the hash is not shown); otherwise the account is created locked and an admin sets a password. An existing local name is a **conflict** (section 8), never merged silently. Role `admin` needs the local admin's approval (section 9) | unticked | `users.json` is never copied as a file |
| Vault | Per vault file: ask the slot password, open the source with `LocalVault::open`/`unlock` (EXISTS), list names (not values), person ticks secrets, they are **re-sealed** into the target vault (`LocalVault::set`; limits `MAX_SECRETS` 500, 64 KiB each). The source file is never copied byte for byte (a copy would carry the source's key slots) | unticked | wrong password = nothing read; the typed password is not stored; values never reach the audit log |
| Browser profile | Per user, copied whole or not at all (cookies and local storage are one database set). Shown with size and "sign-ins may be asked again: sites bind sessions to a device". Chromium-style profiles may refuse a profile from another machine; copy is best-effort, a verify step opens nothing | unticked | stop the target's browser first (profile locks) |
| Installed skills, tools, apps | Reinstall by **record**, not by file copy: read `installed.json`, offer each item with its source and version, and install through the Store pipeline so signature, scope and review checks run again (`docs/STORE.md`). Items from "Official/Verified" stores are reinstalled from there; local/unsigned items are copied only after the same review prompt a local pack gets today | ticked for items the target's store can fetch | `cli/` binaries (7zz, k3s, ffmpeg) are third-party downloads installed at the person's request and "never shipped" (memory note: third-party tools, installer only): they are **not copied**, they are offered as "download again here" |
| Firmware the person installed | Section 7 | unticked | the only category with a licence step |
| k3s data (`k3s-dev/`) | **Excluded**: size (can be tens of GB) and node identity (certificates, node name, tokens); copying makes two nodes with one identity | n/a | shown in the preview as "not importable" with the reason |
| Logs (`audit.log`, `crash.log`, `*.log`, `stage1-kmsg.log`) | **Excluded**. The audit hash chain belongs to one machine; the import adds its own lines instead | n/a | the source `audit.log` may be *read* to show what the drive was used for, nothing else |
| `mac-secret`, `wifi.json`, `net-mac.conf`, `tls/`, SSH host keys | **Excluded**: device identity. A Wi-Fi network list may be offered later as per-network entries (passwords are secrets: ask) | n/a | decision D6 |
| Boot machinery (`iyakki/`, `boot-*`, `/boot` volume) | **Never** | n/a | the import never touches the source's boot volume |

## 5. How the source is opened (PROPOSED, built from existing parts)

1. **Find.** `iyakki_storage::list` returns block devices with `Content::Iyakki` (label `IYAKKI-DATA`, ext4) and `mounted_at` (EXISTS,
   `crates/iyakki-storage/src/lib.rs`). The candidate is any `Content::Iyakki` device that is **not** the device behind `/var/lib/iyakki`
   (`data_location`). Hazard that EXISTS today: `find_data` returns "the first IYAKKI volume that is not mounted", so with two IYAKKI
   drives a boot that does not receive the volume from the first stage could pick the wrong one (`boot.rs` `mount_data_volume`). The
   import must pick by explicit device, never by `find_data`.
2. **Mount** on a private directory (mode 0700, created under `/run/iyakki/import/<random>/`) with `ro,noload,nosuid,nodev,noexec`.
   `noload` stops the kernel from replaying the source's journal (a plain `ro` mount still replays it, which writes); the cost is that a
   source that was not unmounted cleanly may show slightly old data, and the preview says so when the superblock is not clean.
3. **Pin.** Open the mount root once (`openat` directory fd, `O_NOFOLLOW`) and do all later work relative to it.
4. **Never** write anything under that mount, never `chown` it, unmount (lazy-unmount is not allowed) at the end and on any error.

A source that is a **file** (a disk image) or a **folder** on a host share (`docs/HOST-FILES.md`) can reuse steps 3 and 4 later. A source
that is a remote machine over SSH is out of scope.

## 6. Safety of paths (all imports)

The walker is one function, tested alone, used by every category:

* Only names on an **allow-list per category** are visited; everything else in a directory is ignored (so an unknown file, a `.ssh`
  folder of a home, or a socket is never touched unless the category names it).
* Entries: regular files, directories, and symlinks **as links**. Devices, FIFOs, sockets: skipped and counted.
* A symlink is copied as a link only if its target, resolved lexically inside the category root, stays inside; otherwise skipped and listed.
  It is never followed. An absolute target is rewritten or skipped (`/etc/shadow` inside a home must not become a local alias).
* Path components `..`, empty, NUL, newline, tab and names longer than 255 are refused (same rule as `lean-sync.py` `make_manifest`).
* Files are opened with `O_NOFOLLOW`; after open, `fstat` must still say "regular file" on the same device as the pinned root (no hard-link
  or mount trick leaves the volume). Hard links are copied as separate files unless the whole link set is inside the import.
* Setuid, setgid, sticky bits are dropped; file capabilities and xattrs `security.*` are dropped.
* Writes go to a temporary name in the target folder and are renamed in (no half files); the target is never a symlink (`O_EXCL`).
* Target destinations are chosen by the importer from the local account's home (`iyakki_agent::scope::home_root`, `docs/DATA.md` "Where files
  may come from"), never from a path in the source.

## 7. Firmware the person installed (the owner's working rule)

**What this covers.** Restricted-firmware packs (`fw-qualcomm-bt`, `fw-realtek-net`, `fw-broadcom-net`, `fw-atheros-carl9170`; 72 files,
`dist/buildroot/board/iyakki/restricted-firmware.table`) and the Apple packs extracted on the person's own Mac (`mac-wifi-brcmfmac`,
`mac-bt-broadcom`, `mac-facetimehd`). **It does not cover** firmware that is already in the image (Intel, AMD, brcmfmac, ...): that
arrives with the image on every drive.

**The rule.** Allowed **only** as a copy between the person's own drives, at a click, with the vendor licence **shown again and accepted
for this install**, and **logged**. Never into an image, never into an export for someone else, never onto a different Mac model, and never a
file whose licence forbids it.

| Step | What happens |
|---|---|
| 0 Offer | Listed only when the **target** has matching hardware (the same checks the Setup tile uses: `fwpack.rs` `Def::hw` PCI/USB ids; for Apple packs the DMI product in the pack record `matches`, `crates/iyakki-drivers/src/pack.rs` `Match::dmi_product`). No matching hardware: shown greyed with "this computer does not have the chip" and not importable |
| 1 Read the source record | `firmware/<pack>/` has the pack record (`apply::Record`: `matches`, `license`, `source`, files with SHA-256). `fw-*` packs also have `fwpack/<pack>/accepted.json` and `licences/` texts |
| 2 Licence table check | Each file path + size + SHA-256 is looked up in `restricted-firmware.table`. A file **not in the table** (for `fw-*` packs) is not copied, and the pack fails whole: the folder route of the Setup tile does the same ("a file that does not match refuses the whole stick", RESTRICTED-FIRMWARE.md 3b). Licence text is re-read from the pack's `licences/` folder and its digest compared with the table's `doc` lines |
| 3 Per-pack licence rule | See the table below. Packs whose text forbids this copy are listed "not importable: <reason>" |
| 4 Show the licence again | The same form as the Setup tile: vendor, licence summary (`fwpack.rs` `licence_note`), "I have read the licence and accept". Accepting on the source drive earlier counts for nothing |
| 5 Record | On accept: write a **new** `accepted.json` for the target (`fwpack::accept`: who, when, licence digest, commit) and audit line `firmware.import` (pack, files count, source drive serial, licence id, user). Without the acceptance on record nothing is installed (the existing job rule) |
| 6 Install | Not a raw copy: the importer builds a `Pack` from the verified bytes and calls the normal installer (`apply::install_pack`, as `macfw.rs`/`camfw.rs` do), so size limits, name checks and the driver reload run as for a fresh install. The licence texts are kept next to the files |
| 7 Same-Mac check for Apple packs | The source pack's `matches` (DMI product) must equal this Mac's `product_name` (`macfw.rs` reads `class/dmi/id/product_name`); different model = refused, no override. The Wi-Fi set is chosen by board and chip (`macfw.rs` `candidates`), so another model's files are not "equivalent" |

| Pack | Licence point (from `FIRMWARE-LICENCES.md` / `RESTRICTED-FIRMWARE.md`) | Import between own drives |
|---|---|---|
| `fw-qualcomm-bt` | `LICENSE.qcom`: only with Qualcomm chipsets; "shall not subject the Materials to any third party license terms" | allowed **only if the target has a matching Qualcomm Bluetooth chip** (the hardware check does that) and the text is shown again; otherwise refused |
| `fw-realtek-net` | `LICENSE.r8169`: distribution allowed with the copyright notice accompanying it | allowed with the notice file copied along (it is in `licences/`) |
| `fw-broadcom-net` | `LICENCE.tigon/bnx2/bnx2x`: same wording | as above |
| `fw-atheros-carl9170` | GPL-2.0-or-later binary; source is the carl9170fw project, not in linux-firmware | **not imported**: it is simpler and safer to run the normal download (it comes from upstream with the source named); a copy between drives would pass a GPL binary without its source offer. The person is pointed to the Setup tile |
| `mac-wifi-brcmfmac`, `mac-bt-broadcom`, `mac-facetimehd` | Apple's files; "stay on this machine, never uploaded" (`macfw.rs`/`camfw.rs` `NOTICE`); whether Apple's terms allow use on a Mac you own is the owner's open legal question (`LICENSING.md` item 13, `BETA-PLAN.md` section 8) | allowed only between the person's own drives **and the same Mac model**; the licence text shown is Apple's notice as the pack carries it (`LicenseRef-Apple-firmware-supplied-by-the-user`) plus the owner's wording "these files are Apple's and are for this Mac only". If the owner decides this copy is not acceptable (D4), the Apple rows go "not importable" and the person extracts again on the new Mac |

**What is never done.** Firmware is never part of any export or backup file written for transfer to a person other than the owner of both
drives; an "export" bundle of the import feature (section 9) excludes `firmware/`, `fwpack/`, `mac-firmware/`, `camera-firmware/` by name,
and a test pins it. The work folders `mac-firmware/` (Apple recovery image download, 0.5 to 1 GB) and `camera-firmware/` are temporary and
never imported. Images are not built by the import (`IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1` stays the only way, RESTRICTED-FIRMWARE.md section 4).

## 8. Conflicts

| Situation | Rule |
|---|---|
| Item exists in the target (same account name, file, secret name, pack) | **Never overwritten.** Per item the preview offers: Skip (default), Keep both (imported copy gets ` (imported)` or a dated folder), Replace (a second confirm, and for accounts, vault secrets and firmware only with the admin's typed password) |
| Settings key differs | shown as `source value` against `this value`; default keep this |
| Account name exists | Skip or import under another name; roles are never merged |
| File inside a home | existing file kept; imported one gets a numbered name; folders merge but never remove anything |
| Vault secret name exists | Skip or rename (`name-imported`); values not shown |
| Store item already installed at same or newer version | skipped, listed |
| Pack already installed | skipped, unless the licence digest or the table pin changed: then the licence is shown again |
| Two selected items collide with each other | refused before any copy starts |
| Disk space short | the plan shows the shortfall and does not start; the `1.2 x` plus reserve rule of `lean-install.sh` is the model |

## 9. UI, approvals, audit (PROPOSED)

**Where.** Settings -> Storage -> **Import from a drive** (admin or the person themself for their own account), and the same page at
first-run as **Import from a drive** (skippable) so a new install can pick up a person's things at once. Terminal: `iyakki import --list`,
`--dry-run`, `--yes` (the `iyakki` CLI pattern, `crates/iyakki-agent/src/cli.rs`), so it can be done over SSH.

**Flow.** 1 pick the drive (model, size, serial, label, free space, IYAKKI version of the source, from its `iyakki-release` when found;
mounts read-only for a short scan). 2 **Preview**: a tree of categories with counts and sizes, tick-boxes, per-item "skip", conflicts in
yellow, "not importable" in grey with the reason. 3 **Dry run** (button "Check only"): runs steps 1 to 4 of section 10 without writing and
prints the plan file. 4 Approvals. 5 Progress with Cancel. 6 Result list: copied, skipped, failed, with reasons.

**Approvals.** All with the existing policy engine (`docs/ADMIN-GUIDE.md` scopes, the approval prompt pattern of the assistant tools).

| Item | Who may approve |
|---|---|
| Own settings, documents, home | the signed-in person |
| Another person's documents, accounts, role `admin`, browser profile of another user | an admin, with the typed password |
| Vault secrets | the vault's owner, with the slot password (typed for the source and, for the system vault, an admin unlocking the target) |
| Firmware | an admin (same as the Setup tile), with the licence form |
| The assistant | may **list** the preview but never start an import or read secrets (it already may not: `agent.rs` line 212 refuses names with vault/credential/secret); import is a human action |

**Audit lines** (JSON in `audit.log`, as `firmware.accept`): `import.start` (source serial, label, categories), `import.item` for
summary counts per category (never contents; never a secret name list longer than a count), `import.skip`, `import.conflict`,
`firmware.import`, `import.done` or `import.fail`. The source `audit.log` is not merged.

## 10. Bounds, order, dry run

* One import at a time. **Size cap** per import (default 20 GiB, configurable by the admin) and a time cap (default 2 hours, cancel any time);
  the plan shows both before it starts. File count cap (default 500,000) so a directory bomb stops.
* Read rate-limited so the second drive's USB bus does not starve the system; one file at a time, 1 MiB reads.
* **Order:** (1) find and mount read-only, (2) scan with the walker and compute the preview (names, sizes, hashes only for firmware and
  settings), (3) conflict analysis, (4) write the **plan file** (`iyakki-import-plan-1`, 0600, like `docs/INSTALLER.md` "plan file"),
  (5) approvals, (6) apply category by category: vault last (it needs passwords), firmware after accounts, (7) fresh-read verify of what was
  written for files that carry a hash, (8) unmount, delete the temp mount, write the result.
* **Interrupted:** nothing is half-applied: each file is temp-then-rename; a re-run sees the finished items as conflicts-with-identical-content
  and skips them. The plan file records what completed.
* **Dry run** produces the same plan and result list with no write on the target and no unlock of any vault (vault contents are listed only
  after the person types the slot password, so a dry run lists "vault: 2 files, locked").

## 11. Tests

| Kind | Test |
|---|---|
| Unit | the walker on a fixture tree: symlink escape (relative `..`, absolute, loop), device node, FIFO, long name, name with newline/tab, setuid file, hard link across, file replaced by a symlink between scan and copy (TOCTOU, via a hook) |
| Unit | allow-list per category: an unknown file next to `settings.json` is not copied; `mac-secret`, `tls/`, `k3s-dev/`, `*.log`, `iyakki/` never appear in any plan |
| Unit | conflict engine: every row of section 8; default is Skip; Replace needs the confirm flag |
| Unit | uid remap and mode rules; setuid dropped |
| Unit | vault: wrong password reads nothing; right password re-seals into a different data key (compare wrapped key bytes); values never in the audit text; limits (501st secret refused) |
| Unit | firmware: file not in the table refuses the pack; wrong SHA-256 refuses; `matches` mismatch refuses; GPL pack refused; licence digest recomputed; acceptance written for the target only; `firmware/` and `fwpack/` absent from any export (grep test over the export writer) |
| Unit | source mount arguments contain `ro`, `noload`, `nosuid`, `nodev`, `noexec` (text test on the mounter) |
| Integration | a fake IYAKKI data folder (made with the existing fixtures of `fwpack_tests.rs`, `macfw_tests.rs`) imported into a fake target; dry run writes nothing (directory hash before and after) |
| Integration | crash test: stop after N files; re-run finishes without duplicates |
| QEMU | two virtual drives: boot the first, attach the second as a USB disk, run `iyakki import --dry-run` then `--yes` over SSH with the existing harness (`qemu-lean-test.py` style, source attached `snapshot=on`, so its bytes are provably unchanged); a second case with the source's journal dirty |
| Hardware | Mac mini and MacBook with a real stick; an Apple pack from a different model (must refuse); a stick with a failing sector (error handling) |

## 12. Gaps and effort (PROPOSED)

| Piece | State | Size |
|---|---|---|
| Source mounter + walker + plan file | GAP | M |
| Settings key merge, user records, home copy | GAP | M |
| Vault re-seal flow | partly EXISTS (`LocalVault` API) | S |
| Store reinstall-by-record | partly EXISTS (`installed.json`, Store pipeline) | M |
| Firmware import (reuse `fwpack` table, `accept`, `install_pack`) | partly EXISTS | M |
| UI (Settings page, first-run page, progress) | GAP | M-L |
| CLI, audit lines, tests | GAP | M |

## OPEN DECISIONS (owner)

* **D1** Cap defaults (20 GiB, 2 h, 500,000 files)?
* **D2** Should "Replace" exist at all in the first version, or only Skip and Keep both?
* **D3** Account import: may a password hash move if the person confirms the password, or always set a new password on the target?
* **D4** Apple firmware between own drives of the **same Mac model**: allowed (as written) or "extract again on the new Mac" only? (`LICENSING.md` item 13 leaves the legal question open.)
* **D5** Qualcomm pack: allow the copy when the target has the chip (as written) or send the person to the Setup tile like carl9170?
* **D6** Wi-Fi network list (`wifi.json`) as an optional per-network import, or excluded for good?
* **D7** Browser profile: offer at all in the first version?
* **D8** First-run page: shown always, or only when an `IYAKKI-DATA` volume other than the new one is attached?
* **D9** Correct the premise in the requests and docs: the vault has no per-volume key; `mac-secret` is the private-MAC secret (section 2).
