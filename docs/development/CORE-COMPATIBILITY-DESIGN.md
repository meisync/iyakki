# Compatibility charter: forward and backward compatibility for every core feature

Status: design, 2026-11-11. Written for the owner and for whoever adds a core feature. It does not replace the pieces that already exist
([DATA-LAYOUT-AND-UPGRADE.md](DATA-LAYOUT-AND-UPGRADE.md) for files, [CORE-CONTRACT.md](CORE-CONTRACT.md) for the typed calls,
[CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md) for apps, [CLIENT-SPLIT-DESIGN.md](CLIENT-SPLIT-DESIGN.md) for clients); it puts one rule set over all of them
and says what is missing. While the project is alpha (0.x) the rules below are the target; from the first beta they are binding.

**Build status (2026-11-11):** steps 1 to 3 of section 10 are built and run in `./run.sh compat-check` (also a step of `alpha-check`): the inventory of versioned
surfaces (`compat-surfaces.txt`, 91 rows), the public-surface snapshot of `iyakki-core-api` (`core-surface.txt`) and the first golden fixtures
(`tests/compat/alpha2/`, how to add a release folder: `tests/compat/README.md`). Steps 4 to 7 are not built. What the first run found is in section 9.

## 1. What the words mean here

* **Backward compatible:** a newer core reads everything an older release wrote, serves clients and apps built for older interface versions, and installs
  older packages. Nothing an alpha.N person created is lost by updating to N+1.
* **Forward compatible:** an older core, client or app meets something newer (a newer file, message, manifest, hook or capability) and **degrades safely**: it keeps
  what it does not understand, ignores what it may ignore, and refuses, in plain words, only what it cannot do without understanding. It never corrupts, never
  guesses, never crashes.
* **Safe rollback:** after an update the person can go back to the previous version (the boot menu keeps it) and the data they have is still usable by it.

The rule behind all three: **a new version may add, never change the meaning of, and never silently remove.**

## 2. The surfaces that must stay compatible

| # | Surface | Today | Missing |
|---|---|---|---|
| A | Rust interface of `iyakki-core-api` (traits, types) | no promise; changes with each commit | public-surface snapshot gate (section 8) |
| B | Published C interface (the `cdylib`, planned, CORE-ACCESS-MODEL 7) | not built | `iyakki_core_abi_version`, header check, symbol-set test |
| C | Wire protocols: contract `hello` (CORE-CONTRACT), client session protocol (`client.rs`, version range), helper socket (WEBVIEW-PROTOCOL, message ids), extension snapshot (version number) | each has a version field and a refusal path | one shared negotiation rule and golden transcripts |
| D | Data on disk: JSON stores (`iyakki-schema`: schema number, ordered migrations, `.pre-vN` copy, newer file never rewritten, unknown fields kept) and binary formats (vault, ledger, recordings, boot files) | JSON solid; binary formats vary | one header rule for binary formats; delayed migration commit |
| E | Packages and manifests: `app.json`, catalogue entries, bundle `contents`, wizard specs, capability names | `capabilities` list validated; no manifest version, no `requires` | `manifest_version`, `requires`, alias rule |
| F | The system itself: update packs, kernel, boot loader, base image, core, apps (component updates plan) | previous version kept in the boot menu | component skew table, QEMU upgrade and rollback test |
| G | Clients (browser engine, window program, phone) talking to an instance | protocol range negotiated | stated skew window, engine cache-busting |

## 3. One version model

* **Interface version = `major.minor`** per surface (a protocol, the core interface, a manifest, a wizard spec). *Minor* changes are additive only. *Major* changes may
  break and need section 6's overlap window.
* **Features are found by capability, not by version number.** `hello` lists the capabilities each side has; a client that needs one asks (`needed`), one that merely
  likes it offers it (`offered`) and works without it. A version number answers "can we talk at all", a capability answers "can you do this".
* **Every number has an owner file** (one constant per surface, in `iyakki-core-api`) and appears in `dev info`, Settings, About and the support bundle, so a report
  always says which versions met.
* **Schema numbers are per file, not per release** (as `iyakki-schema` does today): a release can leave most files at their old number.

## 4. Rules for changing anything (the part reviewers check)

1. **Add, do not alter.** New optional field, new message type, new enum case, new capability, new error code. Never reuse a name, number, tag or field id for something else.
2. **Unknown is data.** Typed files keep unknown fields (`#[serde(flatten)] extra`) and write them back; a message with an unknown field is processed without it; a message
   type or hook kind nobody here knows is **ignored and reported**, never an error that drops the connection (the extension snapshot already does this).
3. **Enums are open.** Public enums that cross a boundary carry an `Unknown(raw)` case (or `#[non_exhaustive]` plus a serde fallback). A `match` on a foreign value has a
   real default branch with a safe behaviour.
4. **Required means forever.** A field becomes required only in a major version. New fields are optional with a documented default that preserves the old behaviour.
5. **Meaning is part of the name.** If the behaviour changes, the thing gets a new name (`camera.view` stays as it was; a stricter variant is a new capability).
   Renames keep the old name as an **alias** for the deprecation window (capability names, error codes, settings keys, tool names).
6. **Errors are stable strings** (`version_mismatch`, `not_granted`, ...), documented, never renumbered; human text can change, codes cannot.
7. **Defaults are explicit and versioned.** A new default that changes behaviour for existing people ships off for upgraded installs and on for new ones, or ships behind
   a capability the person turns on.
8. **No silent removal.** Removal follows section 6. A removed capability answers `removed` with the replacement named, not "unknown".
9. **Experimental is outside the promise.** Anything labelled experimental (the client endpoint today) lives under an `experimental.` name or an off-by-default setting, may break
   between releases, and is promoted by a rename with an alias.

## 5. Negotiation

* **Every connection starts with `hello`** (contract, client session, helper): `{version range, capabilities offered, capabilities needed, deprecations seen}`.
* The agreed version is the **highest both support**; if the ranges do not overlap, the side that is older gets a refusal that **names which side to update** and to what.
* Missing `needed` capabilities refuse the connection with the list; missing `offered` ones only shrink the feature set, and both sides are told what degraded (the client
  shows "this machine cannot do X yet", the instance logs it).
* Servers keep answering **at least the previous minor of the current major and the last minor of the previous major** (the overlap window, section 6).
* Unknown hook kinds, message ids and events are acknowledged as `ignored` so the sender can show or log them.
* Any component that talks to another and is updated separately (shell/agent, core/helper, instance/client, core/loader) has an entry in the **skew table** (section 7); the
  table is checked by a test that starts the old side from a recorded transcript.

## 6. Deprecation and removal

* **Announce:** release notes, the `deprecations` list in `hello`, and an access-log line each time a deprecated name is used (so a person can see which app still uses it).
* **Window:** in the beta and after, a deprecated thing stays for **at least two releases and at least 90 days**, whichever is longer. In alpha: at least one release, and the
  release notes say so.
* **Remove only in a major**, with a migration or alias already shipped, and the removal listed in the upgrade notes.
* **Security exceptions:** a vulnerable behaviour may be removed at once; the refusal says why and the fix.

## 7. Data and rollback

* **Forward-only migrations, tested.** Keep the `iyakki-schema` rules: ordered migrations in memory, `.pre-vN` copy before the first change, atomic writes, a newer file is read-only
  and never rewritten, corrupt files are moved aside, nothing is deleted.
* **Two-phase migration (new).** Destructive or one-way migrations are split: phase 1 writes the new data **next to** the old (or additively); phase 2 (drop the old) runs only
  after the new version has **booted and confirmed** (`bootconfirm` already exists) and after a grace period. Until then, rolling back to the previous version still reads
  the data it knows. Binary stores follow the same pattern.
* **Binary formats** carry a header (`magic`, `format version`, `min reader version`, length-prefixed sections). A reader that is older than `min reader version` refuses with
  the sentence "update IYAKKI to open this"; otherwise it skips sections it does not know. Vault and ledger changes always ship a reader for the previous format in the same release.
* **Rollback matrix** (tested, section 8): for each release N, installing N, creating data, updating to N+1, using it, and booting the previous version must leave the
  person's data readable (read-only if the old release finds a newer schema, never damaged).
* **Component skew table** (to be kept in this document and tested): loader and kernel are replaced together with the boot partition; the base image and core travel
  together; apps and extensions declare what they need (below); a client works with an instance whose protocol range includes it, and the instance keeps the window of section 5.
* **Apps and bundles declare `requires`:** `{"core_interface": ">=0.3 <1", "capabilities_needed": [...], "capabilities_optional": [...], "manifest_version": 1}`. The Store
  refuses an install the core cannot serve (naming the version), an update shows "needs a newer IYAKKI", and a removed core feature that an installed app still needs blocks the core update on the card.
  A bundle member can carry its own `requires`. Wizard specs and extension snapshots carry their own version and follow the same unknown-is-data rule.

## 8. How it is enforced (gates, in the order I would build them)

1. **Public-surface snapshot.** A small in-house script writes the public items of `iyakki-core-api` (types, fields, variants, trait methods, constants) to
   `docs/development/core-surface.txt`; the gate fails if an item disappeared or changed shape without a version bump and a note. Additions only update the file. (Like the existing
   ratchets: it can only get stricter.)
2. **Golden fixtures per release.** `tests/compat/<release>/` holds real files (every versioned JSON store, a vault, a ledger sample), wire transcripts (contract, client, helper),
   manifests (app, bundle, wizard) and extension snapshots recorded by that release. Tests: **the current code reads all of them** (backward) and **the previous release's reader tolerates
   the current output** where the rules promise it (forward: unknown fields kept, unknown messages ignored). Adding a release adds a folder; nothing is edited.
3. **Round-trip and unknown-field tests** generated from the types: serialise, add a random unknown field and an unknown enum case, read, write, compare; no loss.
4. **Version-skew tests** for each pair in the skew table, driven by transcripts (old client against new instance, new client against old instance, old helper against new shell).
5. **QEMU upgrade and rollback.** Boot release N from its recorded image, create data and approvals, apply the N+1 update pack, boot, check data and apps, boot "previous version", check
   again (the three existing QEMU tests are the base; this adds the second image).
6. **Deprecation test.** Every deprecated name must have an alias test and a removal date in a table the gate reads; an expired entry fails the gate.
7. **Fuzz the boundaries:** the wire decoders and manifest parsers get a bounded fuzz run in the gate (unknown tags, truncation, huge lengths), as the client protocol tests already do for a few cases.
8. **A compat report** in `dev info`, Settings and the support bundle: versions of the core, every interface, schemas in use, connected peers, deprecated names seen. Support never has to ask "which version?".

## 9. What this means for work already in flight

| Item | Needs |
|---|---|
| Capability catalogue (19 names) | names are immutable; renames get aliases; add `removed`/`deprecated` states to the catalogue entry |
| Client session protocol (`client.rs`) | already has a version range; add `deprecations`, golden transcript, skew test |
| Extension snapshot | already ignores unknown kinds; add golden snapshot and the version-9 refusal as a fixture |
| `app.json` / bundles / wizards | add `manifest_version` and `requires` before the first public store item exists; wizard spec version |
| Access log file, approvals file, views file | schema numbers present? verify each uses `iyakki-schema`; golden fixtures |
| Published C interface | design the ABI version and the symbol-set test with the `cdylib` step |
| Alpha policy | until beta: break allowed only with a migration and a release-notes line; the gates above run from the start so beta begins with a clean history |

### Findings of the first gate run (2026-11-11)

Fixed in the same change (small, behaviour otherwise unchanged): the client `version_mismatch` words now say which side to update; the extension snapshot version refusal
says the same; `Approval` keeps unknown fields (`extra`); `Views` and `View` (the views file) keep unknown fields; the browser graphics setting no longer drops unknown
fields when the mode is changed. Not fixed, each with why:

| # | Finding | Why it is left | Proposed fix |
|---|---|---|---|
| F-1 (fixed) | `CapabilityRequest` has `#[serde(deny_unknown_fields)]`: an `app.json` capability entry with a field this release does not know is refused | strictness is a safety choice for manifests; relaxing it changes what the Store accepts | accept and report unknown fields once `manifest_version` and `requires` exist (step 4) Status: fixed. `CapabilityRequest` keeps unknown fields in `extra` (no `deny_unknown_fields`), `appcaps::unknown_request_fields` reports them (the Store parse queues an audit note); unknown capability names are still refused. `core-surface.txt` carries a `removed` note. |
| F-2 (fixed) | `store/approvals.json` is a bare map without a schema number, and `store::approvals::FileBook` reads a file it cannot parse as an empty map, so the next `put` rewrites the file with one entry (every other approval is lost) | the file is in the bundle work's area, and the right fix (schema, move aside, refuse to write over an unreadable file) changes behaviour | wrap in `{schema, approvals}` through `iyakki-schema`, `.pre-v1`, never write over an unreadable file Status: fixed. `store/approvals.json` is schema 1 (`{schema, approvals}`) through `iyakki-schema`; the alpha.2 bare map is lifted with `approvals.json.pre-v1`; unknown fields kept; a damaged file (or entries that do not fit) is moved to `.corrupt-<time>` and never read as empty then overwritten; a newer schema is read-only; refusals are queued for the audit log. |
| F-3 (fixed) | the access log (`access/log.jsonl`) has no schema number, and trimming rewrites lines from the typed `AccessEvent`, which has no `extra` | adding a field to `AccessEvent` breaks struct literals in two other crates' tests | add `extra` and a first-line header with the schema when the log next changes Status: fixed. every line written carries `"v":1`; trimming keeps the raw lines, so fields of newer lines survive; lines without `v` are the alpha.2 layout. |
| F-4 (fixed) | the views file has no schema number (pattern A) | not needed until its shape changes | stamp `schema` through `iyakki-schema` at the next change Status: fixed. `views-<user>.json` has a `Spec` (schema 1), migrates with `.pre-v1`, damaged files are moved aside and a newer file is not written. |
| F-5 | public boundary enums are closed: `client::ErrorCode`, `ClientMsg`, `ServerMsg`, `contract::error::ErrorCode`, `SettingKind`, `StreamKind`, `RunState`, `FlowEvent`, `ConsentKind`, `Risk`, `Transport`, `broker::Denial`. A message of a type this release does not know decodes as `bad_message` (an error value, tested as never a panic) but is not "ignored and reported" as rule 2 asks | a new `Unknown` case breaks exhaustive `match` in `iyakki-remote/clientsession.rs` and the client engine, which others are editing | add `#[serde(other)] Unknown` (or `Unknown(String)` for codes) and a default branch at every `match`, in one change that owns those files |
| F-6 | the typed contract `negotiate` answers `version: None` without saying which side must update; the contract `hello` has no `deprecations` list (nor the client session `hello_ok`) | needs the step 4 design (the field, the window table) | add `update: "client" \| "machine"` to `Negotiated` and `deprecations` to both hellos |
| F-7 | the C helper's own snapshot-version message (`dist/webview/wpe-helper/extsnap.h`) does not say which side to update (the Rust text does) | the C side is compiled and tested against the Rust text by `iyakki-agent::browserext` | change both in one commit with the C tests |
| F-8 | binary formats (backup `IYAKKIBK`, recordings `IYAKREC1`, volume `IYAKVOL1`, snapshot `IYSNAP1`, delta `IYBDELT1`, wake weights, knowledge base) carry a magic with a version digit but no `min reader version` and have no alpha.2 sample fixture | section 7's header rule is design only | a header rule helper and one sample per format in the next release folder |
| F-9 | the developer-mode state (`devmode/mod.rs`) is a `Spec` with `current: 0` and no migrations; the other pattern-A files in DATA-LAYOUT-AND-UPGRADE.md lose unknown fields on save | known and listed there | move each to pattern S when its shape next changes |
| F-10 | the surface scanner (`core_surface.rs`) does not list items behind a non-`pub mod` (only their `pub use`) or the bodies of `impl Trait for Type`, and the capability catalogue contents are checked by `capability-catalogue.txt`, not by the surface file | an honest limit of a text scanner without `rustdoc` JSON | switch to `cargo rustdoc --output-format json` when it is stable here |

## 10. Order

1. This charter, plus an inventory test that lists every versioned surface and fails if a new one appears without an entry here. **Built 2026-11-11:**
   `crates/iyakki-core-api/tests/compat_surfaces.rs` against `docs/development/compat-surfaces.txt` (consts named VERSION, SCHEMA, MAGIC or ABI, every `iyakki_schema::Spec`,
   and a few manual rows; a new one, a changed value or a vanished one fails).
2. The public-surface snapshot gate and the open-enum/unknown-field helpers. **Gate built 2026-11-11:** `tests/core_surface.rs` (in-house tolerant scanner, no dependency)
   against `docs/development/core-surface.txt`; additions are appended with `IYAKKI_CORE_SURFACE_WRITE=1`, a removal or a change of shape needs `IYAKKI_CORE_SURFACE_NOTE` and becomes
   a `removed` line, and `dist/release/compat-check.sh` also compares with the committed file. **Open-enum helpers: not built** (section 9, F-5).
3. Golden fixtures for the surfaces that exist (data stores, client protocol, helper protocol, extension snapshot) and the backward and forward tests. **Built 2026-11-11** for
   every `Spec` store, the approvals, access log and views files, the client session transcript, the contract `hello`, the capability catalogue, an `app.json`, the extension
   snapshot and four helper frames: `tests/compat/alpha2/`. Not yet: vault, ledger and recording samples, bundle and wizard manifests (owed by the work that owns them).
4. `manifest_version` and `requires`, enforced by the Store; the `deprecations` list and the compat report.
5. Two-phase migration with the boot-confirm commit point.
6. The QEMU upgrade and rollback test.
7. The C interface rules with the `cdylib`.

## 11. Open questions

* The window length after beta: two releases and 90 days (proposal), or longer for the vault and ledger formats?
* How far back must a client engine reach: the previous two minors of the current major (proposal)?
* Are third-party apps allowed to depend on experimental capabilities (proposal: only with a visible "experimental" badge on the install card)?
* Does the compat report show peer versions to every account or only to administrators (proposal: administrators)?
