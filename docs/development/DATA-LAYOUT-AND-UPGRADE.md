# Data layout and upgrade rules

This is the inventory of what IYAKKI keeps on disk between boots, which of it is versioned, what a loader does with an older,
a newer or a damaged file, and the rules a change to any of it must follow. It backs gate item 5 of v1.0.0 in
[BACKLOG.md](../project/BACKLOG.md): an install made by the previous release upgrades without losing data, and install and update tools
never touch what they were not asked to touch.

Written 2026-10-27 from the code in the working tree (the loaders were read, not guessed). Fixtures: `tests/fixtures/upgrade/alpha1`
(the layout alpha.1 wrote, derived from `git show v0.1.0-alpha.1:...`) and `tests/fixtures/upgrade/current` (what this release writes);
all synthetic, no real tokens, passwords or keys. A `beta` folder is added when the beta is tagged. The alpha.1 folder also holds `users.json`, `vault/` (made with the code's own functions, synthetic password), `radio-amy.json`, `browser-amy.json` and `browser-data-amy.json`. `browser-groups-*`, `browser-headers-*`, `browser-block-*`, `browser-media-*`, `browser-av-*` and `browser-open-*` did not exist in alpha.1, so there is no alpha.1 file to test.

## Where things live

| Place | What | Survives an update |
|---|---|---|
| `/var/lib/iyakki` (the `IYAKKI-DATA` volume) | all state below | yes, if the volume is a disk (not RAM) |
| `/var/lib/iyakki/home/<user>` | a person's own files (documents, `Flows/`, `Dashboards/`, ...) | yes |
| `~/.config/iyakki/dev` (`$IYAKKI_DEV_HOME`) on the developer's own computer | `id.key`, `devices.json` of `iyakki dev` | yes (not on the device) |
| the boot partition (`boot/`, `EFI/`) | kernel, initramfs, GRUB menu | replaced by an update, on purpose |
| `/etc/iyakki` | `ai.conf` and similar, read only | image content |

## How the stores behave

Three patterns exist today. Pattern S is the rule for everything new and for the stores listed as S; A and B are what the older
stores do, and the table says which is which.

* **S, schema-versioned (`iyakki-schema`).** The file has a `schema` number (missing = 0 = the alpha.1 layout). An older file is
  lifted by a registered migration, in memory, then written atomically; the old bytes are kept once as `<file>.pre-v<N>`. A
  NEWER file is read as far as it can be and never written (the store answers "written by a newer IYAKKI ... update IYAKKI first").
  A file that is not JSON (or does not fit the store at all) is moved to `<file>.corrupt-<unix time>`, the store starts with its
  defaults, and a line goes to the audit log (`data.store` event, written by the shell's audit log the next time it records anything,
  and at start). Fields this release does not know are kept (`#[serde(flatten)] extra`) and written back.
* **V, versioned in its own way (older than `iyakki-schema`).** A number exists (`version` / `config.version`), the same refusal
  rules were added around it (never write a newer file, damaged file kept), no new field was added.
* **A, tolerant but silent.** serde defaults make older files load (every field added since alpha.1 has `#[serde(default)]`), unknown
  fields are ignored on read and therefore LOST on the next save, and a damaged file reads as empty and is replaced by the next save.
  Fine while the layout only grows; not enough for a real migration. These are the next candidates for pattern S, in the order of
  the "Data safety" column.

Atomic write everywhere means: write a temporary file next to the target, sync, rename. A power cut leaves the old file or the new
one, never half of either (the `*.tmp*` leftovers are harmless). Exceptions are named in the table.

## The table

"Older" is what happens when an alpha.1 (or earlier-layout) file is read; "Newer" is a file written by a later release; "Damaged" is
a file that is not valid JSON. Every file name with `<user>` exists once per person. Values are never copied into this document.

### Pattern S and V: the stores that matter most for data safety (tested)

| Store | File | Schema | Writer | Older | Newer | Damaged | Tests |
|---|---|---|---|---|---|---|---|
| Settings (providers, chat/embedding choice, RAG, time, sound, voice, agents, cloud mode ...) | `settings.json` | S: `schema`, 1 | `settings::SettingsStore` | lifted, `.pre-v1`, new fields default | read-only, saves refuse | moved aside, defaults (an invalid but parseable file is still refused loudly and left alone) | `upgrade_tests::settings_*` |
| Inbox (notifications, tasks) | `inbox/<user>.json` | V: `version`, 2 (was added after alpha.1: 0 = ungrouped notes) | `inbox::Inbox` | tidy-up regroups notes, `.pre-v2`, notice shown once | saves refuse, file kept | moved aside, empty inbox for that person only | `upgrade_tests::*inbox*` |
| Dashboard sources (no secrets, only vault references) | `dash/sources-<user>.json`, `dash/sources-system.json` | S: `schema`, 1 (0 = bare list) | `dash::store::Sources` | wrapped into `{schema, sources}`, `.pre-v1` | read, saves and removes refuse | moved aside, empty list | `upgrade_tests::dashboard_sources_*` |
| Dev cluster (k3s) | `k3s-dev/state.json` | S: `schema`, 1 | `k3sdev::save_state` | stamped, `.pre-v1` | saves refuse | moved aside: "no cluster managed here", its data folder untouched | `upgrade_tests::the_dev_cluster_state_*` |
| Flows | `home/<user>/Flows/<id>/meta.json` (versions, tags, published) | S: `schema`, 1 | `iyakki_flow::store::FlowStore` | stamped, `.pre-v1` | every file of that flow folder refuses to be written | `meta.json` moved aside (the flow and `versions/vN.json` stay) | `upgrade_tests::*flow*` |
| Flows | `.../<id>/flow.json`, `versions/vN.json`, `by.json` | `meta.version` is the published version, not a schema; fields added since alpha.1 have defaults | `FlowStore` | loads | not rewritten while `meta.json` is newer | unreadable draft reads as missing | `upgrade_tests::*flow*` |
| Flow assist | `home/<user>/Flows/<id>/assist.json`, `suggestions/<id>.json`, `home/<user>/Flows/_assist.json` | S: `schema`, 1 (`iyakki_flow::assist_store::ASSIST_SCHEMA`) | `AssistStore` | stamped, `.pre-v1`, unknown fields kept | refuses to write a file with a newer schema | unreadable file moved aside, defaults used | `iyakki-flow::assist_tests::the_assist_files_have_a_valid_schema_and_keep_what_a_newer_release_wrote` |
| Flows | `defaults.assist` in `flow.json` and `versions/vN.json` | a field with a serde default, written only when not the default (old flows load as off, content hashes unchanged) | `iyakki_flow::model::AssistPolicy` | loads as off | an older release ignores it | n/a | `iyakki-flow::assist_tests::a_flow_written_before_assist_loads_as_off_and_a_default_writes_nothing_new` |
| Flow runs | `home/<user>/Flows/runs/<id>/run.json`, `log.txt` | A | `runstore::RunStore` | loads (each run carries the flow it ran) | unknown fields lost if the engine saves the run again | unreadable run is skipped | (not yet) |
| Firewall | `firewall.json` | V: `config.version`, 1 (the firewall is newer than alpha.1: no alpha.1 file exists) | `iyakki_fw::service::FileStore` | (future) | load refused, safe defaults, saves refuse | moved to `firewall.json.bad`, or `.corrupt-<time>` if `.bad` exists; blocks incoming (the safe default) | `iyakki-fw::upgrade_tests::*` |
| Network watch (devices seen on the LAN, trusted or not) | `devices.json` in the data folder | S: `schema`, 1 | `devwatch::DeviceWatch` | stamped, `.pre-v1`, trusted flags kept | saves skipped | moved aside (a clean slate would trust every stranger as "normal") | `upgrade_tests::the_network_watch_*` |
| Paired devices of `iyakki dev` (on the developer's computer) | `<dev home>/devices.json` | S: `schema`, 1 | `devmode::client::Home` | stamped, `.pre-v1` | add/remove/default refuse | moved aside, nothing paired | `upgrade_tests::the_paired_devices_*` |
| Developer mode on the device: switch, port, paired keys, open pairing, Wi-Fi grant, patch trust | `<data>/devmode/{state,keys,pairing,fwallow,shell,patch-trust}.json` | A, with a guard: a damaged file is moved aside (audit line), a file marked with a newer `schema` is not written | `devmode::DevMode::{read,write}` | loads | refused (only if the file carries `schema`) | moved aside, defaults (keys: nobody paired; the person pairs again) | `devmode::tests*` (existing) |
| Approval pins (what a person kept approved) | `approval-pins-<user>.json` | S: `schema`, 1 (0 = bare list of keys) | `iyakki-shell` chat | wrapped into `{schema, pins}`, `.pre-v1` | read, saves refuse | moved aside, no pins | `iyakki-shell::upgrade_tests::approval_pins_*` |
| Audit log | `audit.log` (JSON lines, append only) | hash chain: `hash = sha256(ts|user|event|detail|prev)`; unchanged since alpha.1 | `accounts::AuditLog` | verifies | a line with extra fields still verifies (the hash covers the five known fields); a release that changes the hash must keep the old rule for old lines | a cut last line is skipped and no longer swallows the next line (repaired in this change); a tampered line is reported by `verify` | `iyakki-shell::upgrade_tests::the_audit_log_*`, `a_line_cut_short_*` |
| Accounts | `users.json` (argon2 hashes, role, per-person overrides) | none (`{users:[...]}`) | `accounts::UserStore` | loads; old hashes verify; overrides kept; the first administrator stays (setup refuses to run again) | unknown fields of a record or of the file are kept and written back (`#[serde(flatten)] extra`; sub-fields of `overrides` are not); no stamp, so a newer file cannot be detected | open fails with an error and the file stays in place (sign-in is blocked until it is repaired; moving it would offer the first-run setup to anybody), a `.corrupt-<time>` COPY is kept (max 3) and an audit line says so; a file cut while running keeps the accounts already read and leaves a copy too | `iyakki-shell::upgrade_tests::the_accounts_of_alpha1_*`, `a_damaged_accounts_file_*` |
| Vault | `vault/system.afv`, `vault/users/*.afv` | V: `v: 1` | `vault::LocalVault` | opens sealed, same password and slots unlock it, opening and signing in never write it | refused as corrupt, never rewritten | refused (that person's vault stays locked, no new vault is created over it); the system vault is not affected. A system vault that cannot be opened no longer stops the services from starting: administrators are told it is locked, the file is untouched, an audit line says so. Content is never read by any test except the synthetic fixture secrets | `upgrade_tests::the_vaults_of_alpha1_*`, `signing_in_to_an_alpha1_data_folder_*`, `a_vault_from_a_newer_release_or_a_damaged_one_*` |
| Radio stations | `radio-<user>.json` (bare list; `cert_pin` added after alpha.1, not written when empty) | A | `radio::StationStore` | loads, pin empty, file unchanged by reading | n/a (no stamp) | moved to `.corrupt-<time>` (max 3, audit line) before the empty list is used; a save refuses while a damaged file is in the way | `upgrade_tests::the_radio_stations_of_alpha1_*` |
| STORE ratings and comments | `<store>/ratings-<user>.json` (schema 1, mode 0600, keyed `kind:name`, no version; local only, never sent) | S | `store::ratings` | new in this release: no alpha.1 file | stamp 1 written on first save, `.pre-v1` backup | moved to `.corrupt-<time>`, the account starts empty; a newer file is never written | `store::ratings::tests::*` |
| Browser settings and data | `browser-<user>.json` (written key by key; `cache`, `smooth` added), `browser-data-<user>.json` (bookmarks, history, zoom, night sites, downloads) | A | `browser_config`, `browser_data` | load; an existing choice (scripts off) is not forced to the new default; new keys default | n/a (no stamp) | moved to `.corrupt-<time>` (max 3, audit line) before the defaults are used; same for `browser-groups/open/block/media/av/headers-<user>.json`; a save refuses while a damaged file is in the way | `iyakki-shell::upgrade_tests::the_browser_settings_and_data_of_alpha1_*`, `damaged_browser_files_*` |
| Settings, fields added since alpha.1 | `settings.json`: `sound` (boosts, volume modes, mixer, source levels), `time.zone_asked`, vision, power, update check, web access ... | S | `SettingsStore` | alpha.1's sound and time values kept, later fields at their defaults (the mixer choice starts at the default: on) | | | `upgrade_tests::settings_fields_added_since_alpha1_*` |

### Pattern A: per-person and per-feature files (not yet migrated)

Loader = tolerant serde with defaults; damaged = reads as empty and is replaced by the next save; unknown fields are lost on save.
Their files are listed so that nothing on the data volume is a surprise. Classification was done by reading how each file is loaded
and saved, not by an exhaustive audit of every field; the first time one of these changes shape, give it a `schema` through
`iyakki-schema` in the same change.

| Group | Files |
|---|---|
| Browser | `browser-<user>.json`, `browser-profile` data under `web-profile/<user>`, `cache/web/<user>` (cache: may be deleted any time) |
| Developer tools | `devtools-<user>.json`, `data-recent-<user>.json`, `git-tile-<user>.json`, `connections.json` and `git.json` (Git connections; key table next to it), `ssh/<user>/connections.json`, `ide-state-<user>-<workspace>.json`, `ide-workspaces-<user>.json`, `k8s-tile-<user>.json`, `clusters-<user>.json` (has `version: 1`, pattern V), `platform-profiles-<user>.json`, `installed-services-<user>.json`, `host-choices-<user>.json`, `host-mounts-<key>.json`, `devcluster-<user>.json` and `devcluster-hidden-<user>.json`, `dev-tiles.json` |
| Layout and look | `layout-<user>.json`, `screensaver-<user>.json`, `touch-map.json` (Touch Bar), camera choice (`camfw` `last.json`), `radio-<user>.json`, `media-<user>.json`, `media-index-<user>.json`, `media-downloads-<user>.json` |
| Messages | `msg/accounts.json` (no secrets; tokens are in the vault), `msg/state.json` |
| Calendar, notes, knowledge | `calendar/<user>.json`, notes files in the person's home, `kb/` (knowledge base; rebuilt from the documents when lost), `docs/` |
| Jobs and runs | `jobs/<id>/meta.json` and logs, `tasks/` (Autopilot), `runs/<id>/` and `sessions/<id>.json` (chat sessions) |
| Packages and store | `store/installed.json`, `store/highwater.json` (never goes backwards: a rollback guard), `store/revocations.json`, `store/lifecycle.json`, `packages/installed.json`, `appdata/...` |
| Extra firmware and add-ons | `fwextra/accepted.json`, `fwextra/last.json`, `fwpack` same names, `addons/ffmpeg/` (downloaded by the person; never part of the image), `gpu/...` |
| Models and AI | `model-hosts.json`, `mcp-servers.json`, `ext-models.json`, `ext-sessions.json`, `usage.jsonl`, `tools.jsonl` (the assistant's tool log, pruned by `tool_log_days`), `quick-install` states under `cluster` |
| Network | `net-mac.conf`, `wifi.json`, `tls/`, `ssh/dropbear_ed25519_host_key`, `directlink.json`, `directlink-state.json`, `ssh authorized keys` |
| System | `system.conf`, `hwreport.txt`, `swapfile`, `mac-secret` (a secret: never in a fixture) |

Things that are not "state" but must not be lost by an update: `home/<user>/` (everything a person made), `kb/`, `vault/`.

## Rules for a change that touches a stored file

1. Adding a field: give it `#[serde(default)]`. That is enough for pattern A and S; no migration. A new file needs no migration at all.
2. Renaming, removing, moving a field, or changing the meaning of a value, or changing the shape of the root: this is a migration.
   Bump the store's `Spec.current`, add a `Migration { to, what, run }` that works on `serde_json::Value` (it must be idempotent
   and must keep fields it does not understand), add an `alpha1`/`beta` fixture for the old shape if none exists, and a test.
3. A store on pattern A that needs a migration is moved to pattern S in the same change: add `schema` and an `extra` map, use
   `iyakki_schema::{load_typed, save_typed}`, register the migration `0 -> 1` as a stamp (the alpha.1 layout) and your own as `1 -> 2`.
4. Never write a file whose `schema` is higher than the code knows. `save` and `save_typed` check this again right before the write.
5. Never delete a file to "fix" a damaged one: `iyakki_schema::move_aside`. Never drop the `.pre-v<N>` copy or the `.corrupt-*` copy
   automatically; they are small, and the person or support can delete them.
6. Update and install tools write only what they were asked to: the boot partition files of the payload, `boot-next/`, `boot-prev/`
   and `devmode/` on the data volume (see below). No code path of an update may open anything else for writing.
7. The audit chain: the hash input (`ts|user|event|detail|prev`) is frozen. A new field may be added to a line; it is not part of the
   hash. If the rule ever has to change, put a `hash_version` in the new lines and keep verifying old lines by the old rule.
8. Fixtures are synthetic. Never copy a real file; invent values. No tokens, passwords, keys, vault content or personal files.

## What the update tools do and do not touch

| Tool | Writes | Never writes | Evidence |
|---|---|---|---|
| `iyakki dev update` / `update-pack` (device side: `devmode::update`) | boot partition files named in the payload manifest (`boot/`, `EFI/`; `grub.cfg` last); on the data volume only `boot-next/` (the staged payload and its `UPDATING.json` marker) and `boot-prev/` (the previous kernel and initramfs for the boot menu entry "previous version") | anything else on the data volume; anything on the boot partition outside `boot/` and `EFI/` | `devmode::tests_update_safety` (new) and `devmode::tests_update` (power cut at ten points) |
| Checksums | every staged file is checked against the manifest as it arrives and again before anything is written; a mismatch stops the update before the first write (not even the copy of the old version is made) | | `tests_update_safety::a_staged_file_that_does_not_match_its_checksum_*` |
| Another variant (macweb vs pc) or an older image | refused unless forced; Secure Boot state change is never forced | | `tests_update::an_update_that_is_not_for_this_device_*`, `tests_update_safety::a_payload_for_another_variant_*` |
| An update cut in the middle | the marker `boot/UPDATING` and `boot-next/UPDATING.json` say so; `iyakki dev update --status` and the shell tell "A system update from X to Y did not finish. Choose "IYAKKI (previous version)" in the boot menu, then run `iyakki dev update` again"; a cut while the payload is still ARRIVING touches no boot file and says "No unfinished update." (the next run asks only for the missing files) | | `tests_update_safety::a_payload_cut_while_it_arrives_*`, `a_cut_while_the_boot_files_are_replaced_*` |
| `iyakki-usb` install (`apply`) | the partitions the plan names (whole disk, free space, reuse); kept partitions are checked by a sentinel hash before and after | partitions marked `Keep`; it refuses mounted partitions and the running disk | `iyakki-usb::planner_tests`, `apply` tests (existing) |
| `iyakki dev update-pack IMAGE` (`iyakki-usb::bootpack`) | a folder of the extracted boot files with hashes | the image: opened read-only | `bootpack::tests::the_boot_files_come_out_with_their_hashes_nothing_else_does_and_the_image_is_never_written` |

Gaps, said plainly:

* **`iyakki-usb --update` does not exist yet.** BETA-PLAN lists it (workstream j) and F-UPD-02 plans the boot counter and automatic
  rollback. Today the only in-place update is `iyakki dev update` over the paired link. A stick or a disk is updated by writing the
  image again, which erases the data volume unless the plan keeps it (`Keep` rows, tested in the planner).
* **No "discard" for an unfinished update.** "Resume" is `iyakki dev update` again with the same payload; the boot menu entry
  "IYAKKI (previous version)" is the way back. There is no command that clears the marker without finishing; adding one is only safe
  together with a check that the boot files are either all old or all new.
* **QEMU upgrade run:** `dist/buildroot/qemu-upgrade-test.sh`. Run on 2026-10-27 against `target/buildroot-libre` (read-only; the data disk is a temporary file): boot 1 passed (the volume made from the alpha.1 fixtures is found, the marker is there, five fixture files are byte-identical after the boot, `e2fsck` is clean). Boots 2 and 3 need an image built after the `upgrade` selftest stage was added to `iyakki-core/src/main.rs`; the existing images print SKIP (exit 2). No image was built for this.

## Real-hardware upgrade checklist (the part no test here can do)

Run on the Mac mini and the T2 MacBook, and on one PC, with a data volume made by alpha.1:

1. Before: copy `settings.json`, `inbox/`, `dash/`, `home/*/Flows`, `devices.json`, `firewall.json`, `audit.log` and the list of installed
   apps to the developer's computer (`iyakki dev pull`, administrator scope); note `iyakki dev update --status`.
2. `iyakki dev update <payload>` (power cut once on purpose at some point on the second run).
3. Boot the new version. Check: sign-in works with the old accounts; chat provider and model are still chosen; inbox shows the
   grouped notes and the one-time notice; dashboards and their sources are there; the flows list shows every version; the dev cluster
   is still managed; the firewall rules are as before; the audit log verifies (Users, Audit); paired computers still work without pairing again.
4. Look for `*.pre-v*` and `*.corrupt-*` files in `/var/lib/iyakki` and under the home folders: `.pre-v*` is expected once per
   migrated file; `.corrupt-*` must not exist after a clean upgrade.
5. Second boot: nothing is rewritten (file modification times of the stores unchanged except the ones the person used).
6. Boot "IYAKKI (previous version)" from the menu and confirm it still starts on the same data. alpha.1 ignores the new `schema` and
   `extra` fields, so settings, inbox, flows, the dev cluster and both `devices.json` files read as before. Two stores change
   shape and are NOT readable by alpha.1 afterwards: the dashboard sources and the approval pins (a bare list became
   `{schema, sources|pins}`), which alpha.1 then shows as empty. That is why `.pre-v1` is kept: copy it back to go back.
