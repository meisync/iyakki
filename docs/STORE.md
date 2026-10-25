# STORE: the app store

The STORE tile is where a person finds, installs, updates and removes things that extend IYAKKI, and where
publishers (IYAKKI itself, later other people) put them. The first version works completely offline, from what is
compiled into the system, from a folder, or from a USB stick. A remote catalog and payments are designed here and
stubbed in the code; nothing in this version talks to a payment provider.

Contents: 1 What an app is, 2 Trust model, 3 Threat model, 4 Catalog, 5 Installing, 6 User guide,
7 Publisher guide, 8 Paid apps and licences, 9 Assistant tools, 10 Open questions, 12 Driver packs (bare-metal hardware),
13 Private store (your own store in a cluster) and publishing to it, 14 Several stores and the portal, 18 Developer mode (installing from a paired computer). Making things to publish: docs/APP-AUTHORING.md.

Code: `crates/iyakki-agent/src/store/` (`manifest.rs`, `trust.rs`, `revocation.rs`, `catalog.rs`, `install.rs`,
`license.rs`, `builtin.rs`, `tools.rs`, `publish.rs`), the package format in `pkg.rs`, the tile in
`crates/iyakki-shell/src/apps/store.rs` (drawing and keys only).

## 1. What an app is

An app is one **package**: the signed, reviewable `IYAKKI-PKG 1` file of `pkg.rs`, with optional store metadata.

```text
IYAKKI-PKG 1
type: skill                  skill | tool | mcp | cli | agent | template | theme | extension | tile | app
name: lang-python            lowercase letters, digits, dashes
version: 1.2.0               digits and dots; only ever goes up
license: MIT                 SPDX name
sha256: <hash of everything after the header>
signer: <ed25519 public key, hex>                    (optional)
store: {"title":"…","summary":"…", …}                (optional, one line of JSON, signed with the header)
cert: {"v":1,"publisher":"acme", …}                  (optional publisher certificate, one line of JSON)
signature: <signature over every line above, hex>    (only with signer)
---
<payload>
```

The format is backward compatible: a package without `store:` and `cert:` is exactly what 1.0 wrote and still
installs; `store:` and `cert:` sit between `signer:` and `signature:`, so the signature covers them. A system older
than this one refuses a package that has them (it does not know the field) rather than ignore the metadata.

Kinds and what installing one does:

| Kind | Payload | Installing |
|---|---|---|
| `skill` | skill Markdown | goes through the SKILLS store (`pkg.rs`), personal or for everyone |
| `tool` | manifest JSON and a WASM module | goes through the tool-package store; stays in the sandbox with its budgets |
| `mcp` | `{"name","command","args"}` | added to the MCP tile's servers (administrators; the command is shown as a permission) |
| `cli` | none: a catalog entry with `action: open cli <id>` | opens the CLI tile's own installer (checksummed downloads there) |
| `agent`, `template`, `theme` | JSON | kept, verified, in `store/content/<kind>/`; other tiles read them through `Store::content` (a clean hook for the dashboard and story tiles) |
| `extension` | JSON | the general kind for things that extend another tile (a dashboard connector or panel, ...); kept like the other content kinds |
| `tile` | a JSON bundle of a web app: `{"v":1,"manifest":{…app.json…},"files":{path: text or {"b": base64}}}` (at most 64 files, 512 KB each, 1.5 MB in all) | a **custom app**: kept verified in `store/content/<scope>/tile/`; it gets a tile in the launcher (docs/APP-AUTHORING.md, "Tile apps"). Its permissions are the capabilities it declared (`tile:storage`, `tile:clipboard`, `tile:notifications`, `tile:files`, `tile:agent`, `net:<host>`; never the vault); an app that fails its own lint (remote script not declared, eval, undeclared capability, secret) or whose header and bundle disagree is refused. Its saved data is `store/appdata/tile/<scope>/<name>/<user>.json`, the data the lifecycle layer offers to keep or delete on removal. Shown in the STORE as an Extension "for launcher" with Open |
| `app` | WASM module with a UI surface | reserved; refused with a plain message until the UI surface exists |

**Extensions.** Several kinds extend other tiles, and the store groups them as *Extensions* with a target and a kind
(`store/manifest.rs`, `extension_of`): `template` is a template for DASH, `theme` a theme (for any tile, or `target` in the
metadata), `agent` a preset for CHAT, `tool` a WASM tool for CHAT, and `extension` takes its `ext_kind` (`connector`, `template`,
`theme`, `panel`, `preset`, `tool`, `driver`) and `target` (`dash`, `tales`, `ide`, `chat`, `any`, `system`) from the metadata. A row shows "for DASH".
`driver` packs (target `system`) have their own chip, **Drivers**, and their own view: section 12.

Store metadata (`store:`), all optional, all size-capped, validated on every read:

* `title` (60), `summary` (140), `description` (Markdown, 8000), `icon` (PNG or SVG, base64, 12 KiB), up to two
  `screenshots` (16 KiB each), `categories` (5), `tags` (8).
* `publisher` `{id, name, url}`.
* `pricing` `{model: free|paid|subscription|donation, price (minor units, 499 = 4.99), currency, purchase_url,
  checkout_id, trial_days}`.
* `permissions`: the wasm-tool vocabulary, `read:<path>`, `write:<path>`, `net:<host>`, plus `vault:<name>` and
  `run:<program>`; shown in plain words ("connect to example.com", "run /usr/bin/foo"). For a tool or MCP package
  the permissions are **read from the payload**, not from the metadata; if they differ the review says so.
* `min_version` (IYAKKI version), `changelog`, `support_url`, `source_url`, `dependencies` `[{name, min_version}]`.
* licence, version and size come from the header and the file.

## 2. Trust model

Every package gets one trust level, worked out offline from bytes and keys already on the device:

| Level | Meaning | Badge | Rules |
|---|---|---|---|
| **Official** | signed by a pinned IYAKKI root key (or built into this image) | cyan shield | installs after the permission review |
| **Verified** | the publisher's identity was reviewed by IYAKKI; their key carries a certificate signed by a root key | green shield | same, plus the publisher is named |
| **Trusted (yours)** | signed by a key in the administrator's `trusted-keys.txt` | blue shield | same as Verified, named by the label the administrator gave |
| **Community** | a valid signature from a key nobody vouches for | amber shield | allowed with a clear warning that must be accepted |
| **Unsigned / local** | no signature (sideload) | red outline | administrators only, strongest warning |
| **Developer (local)** | installed from a paired developer computer in developer mode (source `developer:<computer>`), whatever key signed it | amber shield | only while an administrator has developer mode on; the account's role decides personal or shared; never updated from a store or promoted; drivers refused (section 18, docs/DEVELOPER-MODE.md) |
| **Revoked** | the key, the publisher or this package is on a revocation list | struck-through | refused; if installed, disabled |

**Keys.** `official-roots.txt` (compiled in) lists the pinned root public keys with a validity window. The
administrator's own keys live in `trusted-keys.txt` (`pkg.rs`). Everything is ed25519 (`ed25519-dalek`) and SHA-256
(`sha2`), both already in the tree; no crate was added.

**Publisher certificate.** `cert = {v, publisher: {id, name, url}, key, not_before, not_after, scopes[], issuer, sig}`.
`sig` is a root key's signature over the certificate without `sig` (canonical form below). The device checks:
the issuer is a pinned root valid now, `key` equals the package's signer, the dates include today, the package kind
is in `scopes`. A certificate that fails only on dates or scope does not make the package refused, it makes it
**Community** with the reason shown ("the publisher's certificate expired on 2027-01-01").

**Canonical signing.** Catalogs, certificates, revocation lists and licence tokens are JSON. The signature covers
`"IYAKKI-STORE <kind> v1\n"` followed by the document with `sig` removed, keys sorted, no spaces. The prefix keeps a
signature made for one purpose (a licence) from being replayed as another (a certificate). Numbers are integers.

**Key rotation.** `official-roots.txt` may hold several roots with overlapping windows. A new root is added in an
IYAKKI update signed as usual; the old one is kept until every certificate it issued has expired or been re-issued,
then dropped in a later update. Publishers re-issue by asking for a new certificate (for the same key, or for a new key signed into
new packages; installed apps keep their publisher through the certificate's publisher id). The revocation list can retire a
root-issued key at once.

**Revocation list.** `{v, serial, issued, expires, keys[], publishers[], packages[{name, kind?, sha256?, before?, reason}],
licenses[], issuer, sig}`, signed by a root key. The device keeps the list with the highest `serial` it has seen
(a lower serial is refused: a replayed old list cannot un-revoke anything). Freshness: a list older than 30 days is
used but the store says so; the catalog and list are fetched together on a manual check, from the USB stick
if that is all there is. Installed apps are re-checked against the list whenever it changes and when the tile opens.

**Rollback protection.** Versions only go up. The store remembers the highest version of each package it ever
installed (`highwater.json`) and refuses a lower one even after the app was removed. A revocation entry with
`before: "1.4.0"` raises the floor for everybody. Catalogs have their own `serial`, also only up.

**TOCTOU-safe install.** The package bytes are read once into memory (from the network, a folder or a USB stick),
hashed against the catalog's `sha256`, parsed, signature-checked, shown to the person and installed **from the same
bytes**. The installed copy is written from memory, never re-read from the source, and re-verified every time it is loaded.

**Installing never grants anything silently.** The review lists the permissions in plain words and the person must
confirm. Tools stay in the WASM sandbox with their fuel, memory and time budgets, and every *call* of a tool still
asks with its capabilities (unchanged from `wtool.rs`). Skills are plain text that steers the assistant: the review
shows the whole text.

**Who may install.** The account needs the STORE tile (role template or per-user override). Installing for everyone
(system-wide) needs `manage_skills` (administrators and operators); installing for yourself needs `skills`. MCP
servers and unsigned packages need `manage_settings` (administrators). Adding a trusted key or a catalog is
administrator-only.

## 3. Threat model

| Threat | Defence |
|---|---|
| Malicious package | hash and signature checked before anything is shown; the payload is untrusted until reviewed; WASM sandbox with budgets; MCP command and tool capabilities are listed from the payload itself; nothing runs at install time |
| Key theft | revocation list (key, publisher, package) signed by a root; certificates expire (default one year); rotation plan above; installed apps from a revoked key are disabled on the next refresh |
| Downgrade / rollback | per-package high-water mark; catalog and revocation serials only rise; revocation `before` floor; catalogs past `expires` cannot install |
| Typosquatting | names within edit distance of a different publisher's name (including look-alike characters such as `0`/`o`, `1`/`l`) produce a warning in the review and in search |
| Dependency confusion | an update must come from the publisher that installed the app (same signer or same certificate publisher) unless it is Official; a dependency is resolved only in the catalog that lists the app |
| Payment spoofing | the store never takes money and never shows a price it did not verify: the review recomputes everything from the package, the catalog price is a display hint, and the *only* thing that unlocks a paid app is a signed licence token checked offline; the checkout opens in the browser tile on the provider's own address |
| Malicious or stale catalog | catalog signature (key → root, certificate or unsigned-and-labelled), serial and `expires`; the entry's `sha256` pins the exact bytes; trust is recomputed from the package, so a catalog cannot make a package more trusted than its own signature |
| Lying metadata | the permission list is derived from the payload; the metadata copy in the catalog is only a preview and a difference is flagged |
| Oversized or malformed input | size caps on package, metadata, icon, catalog (2 MiB), revocation list; all parsing is strict, errors are plain sentences |
| Device theft of a licence | the token is bound to a device id and, optionally, an account; a copied token fails on another device |

Not defended: a compromised IYAKKI root key (mitigated by offline storage, see 10), a publisher whose reviewed
identity was fake (the review process, see 7), a user who accepts every warning.

## 4. Catalog

`catalog.json`, served from any static location:

```json
{"v":1,"id":"iyakki-official","name":"IYAKKI Official","serial":7,"issued":1790000000,"expires":1797776000,
 "mirrors":["https://mirror.example/store/"],"key":"<hex>","cert":null,
 "entries":[{"id":"lang-python","kind":"skill","name":"lang-python","version":"1.0.0","title":"Python","summary":"…",
   "categories":["languages"],"publisher":{"id":"iyakki","name":"IYAKKI","url":"…"},
   "pricing":{"model":"free"},"permissions":[],"license":"Apache-2.0","size":1234,"sha256":"…",
   "url":"packages/lang-python-1.0.0.iyp","signer":"<hex>","cert":null,"featured":true,"added":1790000000}],
 "sig":"<hex>"}
```

* **Sources.** Built-in (compiled in, always there, so the tile has content on the first boot), a URL (any static
  HTTP location; empty by default), a folder or USB stick (`catalog.json` and the packages beside it; no network
  needed). Administrators add and remove sources in Settings. Package URLs are relative to the catalog or absolute;
  mirrors are tried in order.
* **Caching.** A downloaded catalog is verified, then cached in `store/cache/`; offline the cache is used and labelled
  with its age. Folder catalogs are read live.
* **Updates.** A "Check for updates" button; an optional daily check that runs when the tile is opened (off by default;
  no background process, no telemetry, no identifiers in the request: a plain GET of two files).
* **Visible only on request.** None of this is shown by default (see 6): the built-in catalog, the example apps and Discover appear
  when the catalog is switched on or a catalog is added. The assistant's `store_search` sees the same set.
* **Built-in catalog.** The shipped skills (k3s, ten `lang-*`), the sample skills and MCP demo, the CLI catalog as
  "Tools" that open the CLI tile, and three DEMO paid apps whose button says "Buy (coming soon)". The shipped skills
  show as **Preinstalled** and can be removed and reinstalled.
* **Hook for other tiles.** `Store::content(kind)` lists verified installed content of a kind (`template`, `theme`,
  `agent`) so the dashboard and story tiles can offer store content without the store knowing about them.

## 5. Installing, step by step

1. Fetch (background thread, never the UI thread) or read the bytes once.
2. Check the size and the catalog's sha256. Parse: hash and signature (an error, not a warning).
3. Work out the trust level; look for a revocation; check the name against similar names; check the version rules
   (newer than installed, not below the high-water mark, same publisher as before); check `min_version`.
4. Paid and no valid licence: state **Locked**, stop here with "buy or add a licence".
5. Show the review: each check with a tick or a cross, who signed it, the chain to the IYAKKI root or a warning about
   an unknown key, the permissions in plain words, the warnings. Revoked: only "Close".
6. On confirmation, install the reviewed bytes through the kind's own store, record it in `installed.json`
   (kind, name, version, hash, signer, level, source, scope), keep a verified copy so disable/enable and reinstall work.

## 6. User guide

**The tile opens on Installed**, everything present on this system that the store knows or can detect, read from the real
stores and never from a catalog: *MCP* servers (the MCP tile's list, including the sample), *CLI* tools (the CLI tile's
installed state: Claude Code, Copilot CLI, DuckDB, kubectl, ...), *Skills* (shared and your own, including the preinstalled k3s and
`lang-*` skills), *Apps* (the tiles of the system, shown as System apps: they cannot be removed or turned off) and *Extensions*
(WASM tool packages, themes, templates, presets and the like, each with "for DASH" or the tile it extends). Chips with counts
filter by type; `/` searches; `s` orders by name, recently installed, size or trust (the tiles of the system always come last).
A row shows the icon, name, type, version, trust badge (Official, Verified, Trusted, Community, Unsigned or Revoked; hover for
the plain words), status (On, Turned off, Update 1.2 when a catalog knows one), size, and the buttons **Open** (jumps to the tile
where it lives: the CLI tile, the MCP tile, SKILLS, DASH, TALES ...), **Turn off/on** (for what the store installed) and
**Remove** (shows what goes and what stays; a system app refuses with the reason), **Update** and, on the page, **Go back** and **Pin**
(section 15). Enter opens the page: permissions in plain words, publisher, licence, the files, the source, the version history, the
data and what uses it. Tags, groups and your own titles are section 16.

**The catalog is off by default.** Discover (featured, new, categories, search, filters for Free/Paid, trust level, type),
Publishers and the example apps appear only after an administrator switches *Show the catalog (coming soon)* on in Settings (key
`g`) or adds a catalog (a web address, a folder or a USB stick), which switches it on by itself. Settings (administrators) also
holds the catalogs, trusted keys, the daily check (off), installing from a file and adding a licence. Keys: arrows choose, Enter
opens, Esc goes back, `/` searches, `i` installs from the catalog, `r` removes, `u` updates, `o` opens, `c` checks for news,
Tab changes page.

### 6.1 The Setup tab (the system's own building blocks)

Tabs: **Installed**, Discover and Publishers (when the catalog is on), **Setup**, **Settings**. Setup is the former *Quick install* page of the
IYAKKI menu (the menu's *Quick install* entry, key `q`, now opens it; the page itself no longer exists). It lists what the system can
install for itself, from the engine in `iyakki_agent::quickinstall` (unchanged; the command `iyakki quick-install ITEM` is still what
every button runs as a detached job of the workspace `iyakki-install`, shown beside the clock and in the installer list). The screen is
`crates/iyakki-shell/src/apps/store_setup.rs` (what a tile is and does) and `store_setup_view.rs` (the picture: tiles and popup).

Since 2026-10-23 Setup is a page of **tiles**, like an app store: one tile per app, tool or driver, and a click opens a popup for it.
(The first version was a list of cards under folded headings. It hid things: with k3s installed nothing was "recommended", so every
heading was folded, and the installed and running k3s was not on the screen at all. Nothing folds any more.)

* **Name.** "Setup": short, says what it does (sets the machine up), fits a 360 px wide tab strip; "Essentials" implies everything is
  needed, "Tools & services" is long and overlaps the CLI tile.
* **The page.** A search box, a **category bar** (*All*, *Get started*, *AI and models*, *Voices*, *Coding agents*, *Data*, *Media*,
  *Hardware*, *Linux apps*: only the categories this machine has), the filter chips (*All*, *Installed*, *Not installed*, *Recommended*,
  *Updates*) and **Install all recommended**. Below them the tiles, in sections with a plain heading each (title, what it holds, how many
  are installed). Every item is on the page once, in its own category; nothing is folded. Columns follow the width: one on a phone
  (360 px), three at 800 px, four at 1280, up to six on a big screen (`view::grid`); tiles never overlap or leave the rectangle, also in
  a 50:50 or 30:70 pane. The page scrolls with the wheel, the trackpad, the scroll bar and the keys (`scrolllist.rs`).
* **A tile** has a badge (the item's icon in the colour of its category: the same icons and colours the launcher tiles use, no palette
  of its own), the name, the category, one line of purpose, and the markers:
  * a **tick** (green) when it is installed; a **green dot and "Running"** for a service that is up (k3s from the supervisor's state,
    read each frame; Lemonade, Kokoro, the registry, Elasticsearch and Indic-TTS while they are installed, as before);
  * an **Update** pill with an up arrow when a newer version exists (the engine's upgrade lookup, `quickinstall::check_upgrades`, run
    once on a thread of its own when the tab opens and again after an install finished; the drawing never waits for the network);
  * a **bar** (and a ring) while it installs, a red **warning** triangle when the last install failed, an amber ring for "installed
    earlier, not running" and "found on this machine", a small **star** when it is recommended for this machine.
  The status line says it in words: *Running · v1.31.4*, *Installed · v26*, *Update to 1.38*, *Stopped*, *Not installed*,
  *Installing 40 %*, *Failed*.
* **The popup** (a click or Enter on a tile; Esc, the close mark or a click outside closes it): the badge, the name, the category, the
  status line (*Installed v1.37 · Running · Update available v1.38*), what it is, and a table: *Size and time*, *Licence*, *Where it goes*,
  *Needs* ("Installs k3s first"), *Needed by* (what depends on it), *Memory*, then the notes (heavy for this machine, not runnable, found
  on the machine and left alone, used by other tools, a desktop window never installs a cluster). **One main button**: Install, Upgrade
  to x, Open, **Start** (a stopped cluster), Retry, Reinstall, Sign in, Install own copy, Review and deploy, View log. Beside it, only
  what makes sense now: **Stop** (a running cluster, administrators; it asks the dev cluster supervisor, `K3sDev::stop`, on a thread of its
  own), Open its page, **Install all in this category (n)** (the same review as Install all recommended), **View log**, **Details**
  (state on this machine, item id, installer steps, the equivalent command), Delete download (the camera driver), Log out (a signed-in
  coding agent), **Remove (undo)** (only the voice pack, below) and Close. The body scrolls inside the popup when it is longer than the screen;
  the buttons stay. There is no Remove for a cluster or a server.
* **Recommended** is computed from facts the engine and the machine already have (`Machine::recommend`): no k3s, a GPU, an NVIDIA GPU, a
  Mac model; too little memory (the heavy items' `min_ram_mb`) keeps heavy items out and says why in the popup; a desktop window never
  recommends a cluster. A recommended tile stays in its own category and carries the star; the *Recommended* filter lists them.
  **Install all recommended** shows every step (dependencies first), its size and the total download, and starts the jobs on `y`.
* **Who**: the same rule as the old page, `manage_settings` (administrators) for everything that changes the machine (installing,
  upgrading, starting, stopping); others see read-only tiles and popups (the main button reads "Ask an administrator" and cannot be
  pressed). Open, Details, search and filters are for everyone who has the Store.
* **Keys**: arrows move between tiles (Up and Down by rows, across the category headings), Enter or Space opens the popup, `i` presses a
  tile's main button without the popup, `d` details, `o` open, `l` the installer list, `/` search, `f` filter, `c` category, `a` install
  all recommended, PageUp/PageDown scroll, Home/End first and last tile, Esc steps back (popup, then search, filter, category, note, leave).
  In the popup: Enter the main button, `d`, `a` (all in this category), `l`, `o`, `s` (stop a running cluster), Esc.
* **Clusters and the AI profile.** The popup of an item that goes into a cluster (Registry, Lemonade, the voice pack, Indic-TTS, DuckDB, Elasticsearch; not Ollama, which installs on this machine) shows a *Deploys to* row with the confirmation text (`Deploy Lemonade to cluster "This machine" (local)`) and, with more than one cluster, the **cluster switcher** under the close mark (`apps/cluster_host.rs`; *Manage clusters...* opens Settings → Clusters). The installers run in this machine's own cluster: with another cluster active Setup refuses to start them and says how to go on (the Indic-TTS review, which goes through Platform services, follows the active cluster); the review of several items names where the cluster items go; the start and the hand-off are audited with the cluster. Under the filters the page shows the machine's facts line (`This machine: ...`) and, when the AI profile hides tools, "N AI tools are hidden because ..." with the **Show all AI tools** link (administrators; audited like Settings → Cloud mode, key A). See docs/AI-HARDWARE.md.
* **Voice: Whisper tiny + Kokoro** (*Voices*, `voicepack.rs`, docs/VOICE.md "Voice in one click"): recommended only where Settings → Voice has no speech engine (`Machine::voice_engines`). Its button opens a scrolling **review** instead of starting at once: what is downloaded (about 75 MB and about 330 MB, estimates), where it runs (this machine's Lemonade in the cluster, on the CPU), memory, languages (Kokoro English only), and the full MIT and Apache-2.0 texts with the espeak-ng (GPL) note and "Not shipped with IYAKKI; downloaded at your click"; `y` or the button confirms, Esc cancels. It deploys to the cluster on this machine like Lemonade (with another cluster active Setup refuses), pulls only the missing models, tests them, sets the speech engines and offers **Remove (undo)**.
* **The VOICE tile's "Set up Kokoro"** opens the Store on Setup with Kokoro's popup open.
* **Not here**: removing (the CLI tile removes tools, *Installed* removes packages); there is no remove for a cluster or a server (only the voice pack has Remove (undo)). The
  restricted firmware packs (`fwpack.rs`) have tiles only where their hardware is, but their licence step has its own page: the tile
  says where the pack stands and its button is not pressable here yet.
* **Tests**: `store::setup::tests` (fake presence and facts, stand-in install program): the grid at 360x740 to 2880x1800 and in panes,
  every marker as pixels, the popup per state, category, search, keys, scrolling, roles, the camera tile; the Setup tab is in the monkey
  tests (`monkey_store_setup_tab`); `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib store::setup::tests::pictures -- --ignored`
  writes pictures to look at.

**What else could leave the IYAKKI menu (an owner decision; nothing was removed except the Quick install page).** The menu keeps
About, *Quick install* (now only a way into Store → Setup, kept so that the old key `q` and old habits work), Media player, Linux apps,
Mac drivers, Settings and, for administrators, Developer mode. Candidates to remove later, with what already covers them: *Quick install*
(the Store's Setup tab; the STORE tile), *Media player* (the MEDIA tile; its page only adds a typed path and the install button, which
Setup also has), *Settings* and *Developer mode* (both are plain jumps to the Settings tile; Developer mode is also Settings →
Developer mode), *Mac drivers* (Setup's Mac firmware card "Open its page", Settings → Devices and the Devices popover reach it) and
*Linux apps* (Setup's "Linux apps" card "Open its page" reaches it). *About* has no other home. Removing any of them means deleting its
`ENTRIES` line in `brandmenu.rs` and the tests that name it; the pages themselves stay reachable from their cards.

## 7. Publisher guide (offline)

The tool is `iyakki pkg`, runnable on any machine without a network:

```text
iyakki pkg keygen <name>                       writes <name>.key (private, keep it) and <name>.pub
iyakki pkg build  <skill.md|payload> --kind K --version V --license L [--name N] [--meta meta.json] [--key k.key] -o out.iyp
iyakki pkg build  --kind tool --version V --license L --manifest tool.json --module m.wasm [--meta meta.json --key k.key] -o out.iyp
iyakki pkg sign   <pkg.iyp> --key name.key [--meta meta.json] [--cert cert.json] -o signed.iyp
iyakki pkg verify <pkg.iyp> [--roots roots.txt] [--trusted keys.txt] [--revocations list.json]
iyakki pkg catalog-add <catalog.json> <pkg.iyp> [--key name.key] [--url path-or-address] [--id ID --catalog-name N] [--featured] [--expires-days D]
iyakki pkg cert-issue --root root.key --publisher id --name NAME --key PUBHEX [--url U] [--days 365] [--scopes skill,tool] [-o cert.json]
iyakki pkg revoke-sign --root root.key --serial N [--bad-key HEX]... [--publisher id]... [--package name[@before]]... [--license SERIAL]... -o list.json
iyakki pkg license-issue --key pub.key --app NAME --device ID [--account A] [--days N] [--plan P] [--serial S] [-o token.json]
```

Submission and review for the trust mark (outline; the process itself is a business decision, section 10): the
publisher sends a package, the public key and proof of identity; IYAKKI reviews the identity, the source, the
permissions against what the app claims, and the licence; on approval IYAKKI issues a certificate for the key and
lists the package in the official catalog (or the publisher hosts their own catalog; the certificate makes their
packages Verified wherever they are hosted). Re-review on a permission increase. A publisher that misbehaves has its
certificate revoked on the next list.

## 8. Paid apps and licences

IYAKKI never processes a payment and never sees card data.

* **Pricing display.** `pricing` is shown on the page. Models: free, paid (one time), subscription, donation (free to
  install, asks to give).
* **Purchase flow (designed, stubbed).** The Buy button opens `purchase_url` (or a store checkout built from
  `checkout_id`) in the browser tile on the payment provider's own address. After paying, the provider (or the store
  backend) returns a **licence token**, delivered by download link, email or QR code; the person adds it with
  "Add licence" (clipboard or file, so a USB stick works), and later the store can fetch it itself. In this version
  the button is disabled and says "Buy (coming soon)".
* **Licence token.** `{v, app, publisher, device, account?, serial, plan, issued, expires?, issuer, sig}`, signed by the
  publisher key that signs the package or by an IYAKKI root. Verified offline: signature, issuer allowed for this
  app, `app` matches, `device` matches this machine's store device id, `account` matches the signed-in account if
  present, not expired, serial not on the revocation list. Stored in `store/licenses/<app>.json` (mode 0600).
* **Locked until licensed.** A paid or subscription package without a valid token is **Locked**: it can be browsed but not
  installed. An expired subscription token locks updates and, after the grace stated by the app, disables the app.
* **Refunds and revocation.** A refund adds the token's serial to the revocation list; the app is then Locked again
  on the next list update. Offline devices keep working until they see a newer list, by design.
* **Trials.** `trial_days` is metadata for the checkout; a trial is itself a token with an `expires`.

## 9. Assistant tools

`store_search(query, kind?, free?)` and `store_info(id)` are read-only and use the built-in, cached and local catalogs
only (no network). `store_install(id, scope?)` always asks. It is on the "never automatic" list (`hard_ask`), for every trust
level and every permission set, because that list sees only the call and not the package: Auto and Autopilot cannot
run it: the person sees the same review (trust level, publisher, permissions in plain words). The
tool downloads and verifies at the moment of the call and refuses if what it finds differs from what the person was
shown (trust level or permissions). The assistant may say "there is a skill for this in the store".

`app_scaffold`, `store_validate`, `store_publish` and `store_server_status` are the authoring tools (section 13, docs/APP-AUTHORING.md):
`store_publish` is on the hard-ask list for every server, package and trust level, `app_scaffold` writes only inside the workspace and asks.

`driver_status` (read-only) says which devices of this machine lack a driver or firmware, and `driver_install(pack, source,
accept_licence?)` installs a driver pack. The second is also on the hard-ask list, for every trust level: it changes the
whole machine, so Auto and Autopilot cannot run it. Section 12.

## 10. Open questions (for the project owner)

* **Payment provider** and the legal entity that sells (merchant of record versus direct); tax handling.
* **Revenue share** between IYAKKI and publishers: left open; the format needs nothing from it.
* **Review process** for the trust mark: who reviews, how long, what proof of identity, what it costs a publisher.
* **Official root key**: who generates it, where it lives (an offline hardware token is the proposal), the backup
  holders, and the rotation calendar. Until it exists `official-roots.txt` is empty and only built-in items are Official.
* **Where the official catalog is hosted** (the project's Gitea releases or a mirror) and the default URL.
* **Refund policy** and what happens to a subscription app after expiry (grace period).
* **Full WASM apps** with a UI surface: the `app` kind is reserved until that surface is designed.

Which AI cards a machine shows (GPU runtime, model servers, the Mac host) follows its AI hardware profile: [AI-HARDWARE.md](AI-HARDWARE.md). An administrator can switch on Show all AI tools in Settings -> Cloud mode.

## 12. Driver packs (bare-metal hardware)

For machines that need a firmware file, a module option or, rarely, a prebuilt module that the standard image does not
carry; first the Macs of docs/MAC-HARDWARE.md. A **driver pack** is an `extension` package with `ext_kind: driver` and
`target: system`. It is **data only**: nothing is compiled, downloaded or run at install time, and nothing is built on the device.

Code: `crates/iyakki-drivers` (pack format and checks, detection, the known-drivers table, firmware placement, kernel
module checks, camera firmware extraction), `store/drivers.rs` (review, install, built-in entries, assistant tools),
`apps/store_drivers.rs` (the Drivers chip). The boot hook is `iyakki-core/src/boot.rs` (`late_boot`).

**Payload** (JSON in the package payload; the metadata and the signature are the normal ones): `matches` (when the pack
applies: `dmi_product`, `dmi_vendor`, `pci` `vendor:device`, `usb` `vendor:product`, `*` wildcards at an end; fields in one
match are AND, matches are OR), `files` (firmware: relative path, base64, optional sha256), `modprobe` (lines for
`/etc/modprobe.d`: only `options`, `blacklist`, `softdep`, plain words), `load` and `reload` (modules to load, and to
unload and load again once the firmware is in place), `cmdline` (kernel option **suggestions**, shown and never applied),
`modules` (prebuilt kernel modules, each naming the kernel release it was built for) and `why`. Limits: 64 files, 3 MiB a
file, 6 MiB in all, paths without `..` or hidden names. `extension` packages may be up to 12 MiB (other kinds keep 64 KiB).

**What installing does** (`iyakki-drivers/src/apply.rs`): writes `<data>/firmware/<pack>/` (`pack.json`, `files/`,
`modules/`); links each file into `/lib/firmware/<path>`, the folder the kernel always searches, without replacing a file
the image already has; writes `/etc/modprobe.d/iyakki-<pack>.conf`; unloads and loads `reload` drivers and loads `load`
ones; shows the kernel option suggestions. The root file system is in RAM, so the supervisor does the linking again at
every boot, after the data volume is mounted and before the network, sound and Bluetooth drivers load. Drivers that load
earlier (storage, input, display) cannot use pack firmware. Where the files go when the code is not on the device image
(the desktop window, tests): a sandbox folder inside the data folder, and no program is started.

**Prebuilt modules** are the only code a pack can bring, and the strictest case: the pack must be **Official, Verified
or Trusted** (never Community, Unsigned or local); the module's `.modinfo` `vermagic` must equal the running kernel's
release and, when the image has a stock module to compare with, the same build flags; the same check is repeated at every boot
(a changed kernel skips the module with a message); a non-GPL licence taints the kernel and the review says so. The
kernel configuration does not require signed modules; a build that did would refuse unsigned ones itself.

**Who and where.** Only administrators, only for the whole machine (the review refuses anything else). Packs use the trust
levels, revocation list, version rule (only ever up) and permission review of the rest of the store; the permissions shown
are derived from the payload (`write files in /lib/firmware`, `write files in /etc/modprobe.d`, `run the program modprobe`,
`run the program insmod`).

**Sources.** A signed `.iyp` from a catalog, a folder or a USB stick; **or, for a machine that has no network yet, files
the person supplies** from a folder or USB stick: the device takes only the names the entry accepts (for example `*.hcd` for
Broadcom Bluetooth, `brcmfmac*.bin|.txt|.clm_blob` for brcmfmac), builds an unsigned local pack and runs the normal review
(unsigned: the strongest warning, administrators only). The **camera firmware** is extracted from Apple's `AppleCameraInterface`
file that the person gives, after Apple's licence notice is accepted, with SHA-256 checks on the input and the output
(`facetime.rs`); the device does not download Apple's disk image.

**The Drivers chip.** In Installed, next to MCP, CLI, Skills, Apps and Extensions. It lists *Drivers for this machine*:
the built-in entries whose rules match the machine's DMI product and vendor, PCI ids and USB ids (a generic PC gets an empty
list), each with its state (driver loaded, firmware missing, installed and waiting for a reload, no driver) and one of four
availability labels (works with the standard image, works after installing the pack, needs the Mac image variant, not
supported); buttons: Install (catalog pack, from a USB stick or folder, or from a file for the camera), Remove, Why (what it
is, what the person supplies, the licence, the devices found). Entries of the built-in catalog that need files open this view.

**Built-in entries** (no blobs, only instructions and rules): Intel graphics (works), Broadcom Wi-Fi firmware for
brcmfmac-class chips, BCM4360 Wi-Fi (works only with Broadcom's non-free `wl` that you build yourself on a Linux PC and install from a USB stick as a signed pack; never shipped), Broadcom Bluetooth firmware, FaceTime HD camera, T2 Macs (needs
the Mac image variant: apple-bce), fan and sensors, Cirrus audio, and a real pack for Apple keyboard options.

**Not here:** kernel patches (t2linux), anything that needs a compiler, proprietary drivers we may not redistribute.

## 13. Private store

A private store is a small server of your own that holds apps for your devices: `iyakki-store-server` (about 1 MB, static; the
same code as `iyakki store-server`, which stays as an alias), in a `FROM scratch` image. IYAKKI can deploy it to a k3s instance you pick (Settings, Private store,
Deploy to k3s, or the assistant's `platform_deploy` with service `private-store`; you approve the plan either way), add it to this
device's STORE with its key pinned, and publish to it from the command line, the assistant or the IDE.

Code: `store/server.rs` (requests, verification, catalog), `store/server_http.rs` (sockets, `iyakki store-server`), `store/vet.rs`
(the checks shared with `store_validate`), `store/author.rs` (scaffold, pack, publish client), `store/authtools.rs` (assistant tools),
`store/privatestore.rs` (the platform service), `platform/localbin.rs` (finding the static program, section 13.1), `crates/iyakki-store-server`
(the small binary), `catalog.rs` (pin, withdrawn packages).

**What the server does.** `GET /catalog.json` (signed with the store's catalog key, serial up on every change), `GET /packages/<sha256>.iyp`
(immutable, cached for a year), `GET /yanked.json` (signed), `GET /healthz`, `GET /v1/info` (name, key, fingerprint), `GET /` (a plain HTML
page with no script, a content-security policy and a trust badge per app). `POST /v1/publish` takes the raw package or a multipart
file part, with `Authorization: Bearer <token>`. `DELETE /v1/packages/<name>[/<version>]?reason=...` (administrator) withdraws a
package. `POST /v1/publishers` and `GET /v1/publishers` (administrator) manage publishers. No CORS headers are ever sent; OPTIONS is refused.

**What a publish must pass.** The hash and the ed25519 signature (unsigned packages are refused), a kind from the allow-list (skill,
tool, mcp, agent, template, theme, extension, tile), the size caps of the kind and `--max-body-mib`, a version newer than every version of that
name ever published (withdrawn ones count), the name belongs to the publisher who first used it and is not a look-alike of another
publisher's name, the store information validates and names the token's publisher (or none), no secret in the payload, and the trust
level worked out from the store's own `roots.txt` and `trusted-keys.txt` (in the data folder): a key nobody vouches for lists the
package as **Community**, a key in `trusted-keys.txt` as **Trusted**, a certificate under a root as **Verified**; `--require-trusted`
refuses Community. Devices work the trust level out again from the package, so a store cannot make anything more trusted.
Everything that writes is serialised; the catalog is rewritten atomically.

**Tokens.** Each publisher has a token (`iyk_` and 64 hex characters, shown once); only its SHA-256 is kept, in `<data>/publishers.json`.
The administrator token is the bootstrap one (`--publish-token-file`, from a Secret): it adds publishers, withdraws packages, and can publish
as `admin`. Add a publisher with `POST /v1/publishers` or `iyakki store-server publisher-add --data DIR --pub-id alice` (inside the pod:
`kubectl exec deploy/private-store -- /iyakki store-server publisher-add ...`). Tokens are only accepted in the Authorization header. Ten failed
sign-ins from an address in a minute lock it out for the rest of the minute; changes (20 a minute) and reads (600) are limited per address;
`<data>/audit.log` records every publish, withdrawal, sign-in failure and rate limit (never a token or a secret).

**The catalog key.** Generate it on the administrator's device and keep it off the pod's disk and the network:

```text
iyakki pkg keygen store-catalog                       # writes store-catalog.key (private) and store-catalog.pub
kubectl create namespace iyakki-private-store
kubectl -n iyakki-private-store create secret generic private-store \
  --from-file=admin-token=<(openssl rand -hex 24) --from-file=catalog-key=store-catalog.key
iyakki store-server --data /data --publish-token-file /secrets/admin-token --catalog-key-file /secrets/catalog-key
```

When IYAKKI deploys the store for you it does this itself: the Secret `private-store` holds a random admin token and a random
**seed** (`--catalog-seed-file`: the key is SHA-256 of a fixed label and the seed), both created on this device and kept in your vault
(`svc.<instance>.admin-token`, `svc.<instance>.catalog-seed`); the key is derived inside the pod and never written to its disk. Losing the
seed means a new key, and every device must add the store again; back the vault and the Secret up.

**Deploying.** The image is built on the device from the static `iyakki-store-server` (section 13.1 says where that file comes from): IYAKKI
copies it into `<data>/store-server/build-<version>/` with a three-line Dockerfile (`FROM scratch`, `USER 10001`, `EXPOSE 8080`), builds it
for the cluster's CPU with podman or docker and pushes it to the in-cluster registry (Settings, Dev cluster, Registry A); pods pull it from
there, never from Docker Hub. The file is looked for **before** anything is created in the cluster, so a missing program changes nothing. The
other route is the multi-stage `dist/store-server/Dockerfile` (`FROM scratch`, numeric user 10001, port 8080). The Deployment runs one
replica as user 10001 with a read-only root file system, all capabilities dropped, probes and limits; storage is a PVC; the Secret is
mounted read-only. Options: namespace, storage size, exposure (NodePort, ClusterIP, LoadBalancer, Ingress), public URL, publishers to create
now, read-only, trusted keys only.

**No registry yet? One click.** The image has to reach the cluster through a registry inside it (the server is built on your device; it is not on Docker Hub).
If the cluster has none, the deploy review says so in plain words and shows **Create registry** instead of failing. Pressing it shows what will be created
(namespace `iyakki-registry`; the CNCF Distribution `registry:2`, Apache-2.0, which the *cluster* pulls from Docker Hub once, IYAKKI does not ship it; a volume of
5, 10, 20 or 50 Gi on the cluster's default StorageClass such as local-path; a NodePort Service on 30500 or the next free port), then runs as a detached job
(the installer ring beside the clock, the log in JOBS, also Store → Setup → Data services → Local image registry). When it is ready the deploy plans again and continues
by itself. It needs the cluster to answer, a Ready node and a StorageClass; otherwise one sentence says which is missing and nothing is created. Pods and the node pull
`localhost:<port>/...` (containerd allows plain HTTP for localhost and a NodePort answers on every node), so **no `registries.yaml`, no node restart and no root**: the cluster is not restarted.
IYAKKI pushes the image itself over the registry v2 API (`ocipush.rs`: a one-file image, so no docker, podman or curl is needed). A second click only reports
`registry ready · localhost:30500 · used by N images` and offers Remove (type `iyakki-registry`; this deletes the volume and its images). Platform services → Registry row does
the same without a deploy. **The registry has no sign-in**: anything that can reach the machine's NodePort can push to it, like every NodePort Service; keep that port off untrusted networks.
Status: built and tested with fakes only; not yet run on a real cluster.

**TLS.** The server speaks plain HTTP. Put TLS in front: an Ingress with a certificate (cert-manager or your own), or a sidecar proxy. IYAKKI refuses to
send a publish token over `http://` to anything but a loopback or private-network address or a cluster-internal name.

**Adding it to a device.** STORE, Settings, Private store, Add a private store (check its key): type the address, the device reads `/catalog.json` in
the background and shows the **fingerprint** of the catalog key (32 hex characters in groups of four, the same as `/v1/info`, the HTML page and
the server's start-up line). Compare it with the one the administrator gave you; only "They match" pins the key. From then on the catalog is accepted
only when signed by exactly that key (another key at the same address is refused and its serial is not remembered), counts as signed by a key you
trust, and has its withdrawn packages hidden (`yanked.json`, signed by the same key). The packages themselves keep their own trust levels:
signed by an unknown key means Community with the usual warning, unless the administrator adds the key (Trust a publisher key). The Private store card shows the
state, opens the deploy wizard with the service preselected, and **Publish here** makes the selected private catalog the default publishing target.

**Publishing.**

```text
iyakki pkg validate .                                  # the install-review checks on a project folder (or a .iyp)
iyakki pkg pack . --key me.key -o app.iyp              # build and sign the project's package
export STORE_TOKEN=iyk_...                             # your publish token; never on the command line
iyakki pkg publish --server https://store.example.lan --token-env STORE_TOKEN app.iyp
```

From the assistant or the IDE: `store_publish {package_path, target?}` always asks (even in Auto and Autopilot): the person sees the server, the package
(name, version, permissions, trust level) and approves; the token is the vault entry `store-token-<catalog id>` (add it in the vault; the model never sees it).
Section 9 and docs/APP-AUTHORING.md describe `app_scaffold` and `store_validate`.

**Limits (alpha).** A withdrawal removes a package from the catalog and stops new installs on devices that refresh; it does **not** disable
copies already installed: the revocation list of section 2 needs a root signature, and a private store has none. To pull something back from
devices, publish a newer version, or ask people to remove it. Not run against a real cluster yet: the plan, the manifest and the image build are tested with fakes;
the server itself is tested over loopback. Not yet: per-publisher key pinning, an expiry for tokens, a web form for publishers.

### 13.1 The server program

A `FROM scratch` image holds only a **static** executable of the **cluster's CPU**. The program that is running (the desktop window, or any
dynamically linked build) can never be it and is never chosen. IYAKKI judges a file by reading its ELF headers (no program interpreter, no
`DT_NEEDED`, the right machine; static-pie musl builds pass, glibc builds do not) and never runs it. The file is looked for in this order, for the
architecture of the cluster's Ready nodes (amd64 or arm64):

1. **Shipped with the install**: `/usr/share/iyakki/store-server/<arch>/iyakki-store-server` on the images, or `store-server/` beside the window
   binary or in the release tarball. The release folder also carries `iyakki-store-server-x86_64` (and `-aarch64` when it could be built), signed with the rest.
2. **The cache**: `<data>/store-server/` (`iyakki-store-server-<arch>`; an older static `iyakki` there also works). A file you choose or a build you ran is kept here.
3. **The source tree's** `target/<arch>-unknown-linux-musl/release/iyakki-store-server`, when the tree is found (`$IYAKKI_SOURCE`, the word in
   `<data>/store-server/source`, or above the running program; the word `none` there turns this off).
4. **Build it now**: if the source tree, `cargo`, the musl target (`rustup target add x86_64-unknown-linux-musl`) and a linker exist, the result card offers
   **Build now**. You approve the exact command, it runs as a detached job (JOBS shows it; Stop ends it) with a moving bar, and the deploy continues by
   itself when it succeeds; if it fails the card shows the last lines. `dist/store-server/build-static.sh [x86_64|aarch64]` is the same build by hand (for
   aarch64 it uses the cross compiler of `./run.sh build-arm`, `aarch64-linux-musl-gcc`, `aarch64-linux-gnu-gcc` or `$IYAKKI_AARCH64_CC`).
5. **Otherwise** the card lists exactly what is missing and offers **Choose a binary file...** (type a path; the headers are checked, a static file of the
   wrong CPU is called out as such, a good one is copied to the cache) and **Open docs** (this section). The Dockerfile route always works.

The same discovery, **Build now** and **Choose a file** apply to any platform service that declares a locally built program
(`ServiceDef::local_binary`, docs/PLATFORM-SERVICES.md). Not yet: a graphical file picker for the chosen file (the path is typed).

### 13.2 Removing or repairing the private store

An install can stop half-way (the namespace exists but nothing else; the volume and the Secret but no server; the server crash-looping), a token can be
lost, or the cluster can be rebuilt under you. Open STORE, Settings, **Repair, reset or remove the private store...** (or Settings, Dev cluster, Platform
services, the private store's row, **Repair or reset...**). The deploy wizard does the same look first: before it asks for options it reads the cluster, and
when something of the store is already there it shows this page instead of failing on "already exists".

**State** (`store/privatemanage.rs`, `private_store_state`; read-only, only the chosen cluster and namespace, `kubectl` or the managed cluster's `k3s kubectl`):

| State | Meaning |
|---|---|
| Not installed | nothing in the cluster, no record on this device, no STORE entry |
| Healthy | namespace, volume, Secret, server ready with endpoints, address, the admin token in your vault, a record here |
| Partly installed | says in plain words what is there and what is missing (namespace, data volume, admin token Secret, server, address) |
| Broken | the cluster does not answer (nothing is changed; start the cluster, then look again) · no kubectl on this machine · the namespace is stuck Terminating · the namespace is not IYAKKI's · the server pod crash-loops or cannot pull its image · the server has no ready pod or endpoints · the admin token is gone from your vault · the cluster has the store but this device has no record · this device lists a store (installed services or STORE) but the cluster has nothing |

**Repair** puts back only what is missing and keeps the data: only the missing objects of the manifest are applied (the image is pushed again only when the
server itself is missing), an existing Secret is never regenerated, a lost token is read back from the cluster's Secret into your vault (never printed), a
crashing server is restarted, a store without a record is adopted again. It is idempotent: run it twice and the second run changes nothing. If the Secret itself is
gone while the volume is kept, the preview says that a new token is made and the catalog key changes, so devices add the store again.

**Reset and reinstall** deletes the store's workloads, then the whole namespace (volume and Secret with it), **waits until the namespace is gone** (2 minutes, with
progress), cleans this device's entries (the vault entries `svc.<instance>.*`, the STORE catalog entry for this store and its `store-token-<id>`, the installed-services
record) and continues in the deploy wizard. The data and the catalog key are **gone**; every device has to add the store again. If the namespace sticks in
Terminating, the message says why (something in it has not finished deleting: a volume still in use, or an object with a finalizer) and offers **Reset again, clearing
finalizers**, which clears the finalizers of the objects left in that one namespace and nothing else. It only works on a namespace IYAKKI made for the store.

**Remove** deletes the server, its address and ingress. You choose **keep my published apps** (default: the volume, the Secret, the namespace and your vault entries stay; Repair
brings the store back with its apps) or **delete everything** (also the volume, the Secret, the namespace, this store's vault entries, its STORE entry and the record). The in-cluster
registry is separate and is kept unless you switch **Registry too** on. Trusted publisher keys are never touched, nor anything that belongs to another store.

Anything that deletes shows the exact list first and needs the namespace typed (`iyakki-private-store` by default); a wrong word starts nothing. Reset and Remove
run as a detached job (`iyakki store-manage reset|remove`, shown beside the clock and in JOBS with a live log; Esc hides the page and the job goes on); the cluster side is
repeatable (a namespace that is already gone is success, so it works when only the namespace exists) and the vault, STORE and record cleanup happens in your session when the job
succeeds (a detached program cannot open your vault). Not run against a real cluster: tested with a scripted kubectl.

## 14. Several stores and the portal

The project's **official store catalog is listed on the project's portal**; a person adds a store by its link. There can be several
stores (up to 8 added ones, plus the built-in catalog); each has its own signing key and trust, and the person downloads from any.

**Adding a store.** STORE, Settings, *Add a store* (`a`): paste the portal link, either an https address of a `catalog.json` or a portal page
address ending in `/` (it resolves to `<link>/catalog.json`). The device reads the catalog in the background and shows its **name**, number of
apps, the **signer's fingerprint** and the **trust level** (Official or Verified when a pinned IYAKKI root vouches for the key; otherwise "not vouched for
by IYAKKI, add it only if you know who runs it"; a plain `http://` link is flagged). Nothing is enabled until the person presses y ("They match");
Esc cancels. The key is then pinned (section 13): another key at that address is refused. Remote catalogs must be signed, so an unsigned one is
not accepted. A folder or USB stick is added with `f`.

**The official store.** *Add the official IYAKKI store* (`w`) is the same flow without typing, using the one constant `OFFICIAL_STORE_URL`
(`crates/iyakki-agent/src/store/catalog.rs`). It is empty until the owner publishes the address: the row says "coming soon" and nothing is added.
No address is invented. Until the official root key is pinned in a build (F-APPSTORE-14) the trust line says so honestly.

**Switching stores.** With two or more catalogs Discover shows a chip **Store: All** (or the store's name). `s` or a click cycles All, the built-in
catalog, then each added store; the list, Featured, New, categories and Publishers show only that store's apps. The choice is remembered per
user (`store-<user>.json` in the data folder) and falls back to All if that store is removed or switched off.

**The same app in two stores.** In *All* an app that several stores list appears once (the newest version; the first catalog wins a tie), its page names
the store it comes from (*Catalog*) and the others (*Also in: Store A 1.0.0*). The *Store: ...* button (or `s`) on the page opens the other store's
version. **Install always uses the catalog of the version shown**: the package is fetched from that catalog and the installed record keeps its source;
there is no fallback to another store with the same id. An update must still come from the publisher that installed the app (section 5).

**Limits.** The cap of 8 added stores is kept. Never run against a real portal (none exists yet); tested with folder catalogs and a fake network.


## 15. Keeping things current: check, update, go back, pin, remove

One layer, `iyakki_agent::store::lifecycle`, does this for every installable kind; the STORE tile, the terminal's `store` command and the
assistant's `store_*` tools only ask it. (F-APPSTORE-20 to 24.)

**What exists, per kind.**

| Kind | Latest comes from | Update | Check after installing | Go back | Remove |
|---|---|---|---|---|---|
| Skill | the store that installed it (or another store, same publisher key) | yes | the skill parses and loads | yes (1 version) | file, index entry, kept copies |
| WASM tool | same | yes | hash, signature and manifest load again | yes | package, registry entry, kept copies |
| MCP server | same | yes (replaces the definition in one write) | the server starts and answers `tools/list` within 10 s | yes | list entry, kept copies |
| Theme, template, agent preset and other content | same | yes | the file verifies | yes | file, kept copies |
| Driver pack | same | from the Drivers chip only (it changes the whole machine) | the pack is listed | yes | the pack's own removal |
| Command-line tool | the vendor (the CLI catalog's version address) | yes (same download, checksum check and swap) | the new program must start before it replaces the old | yes (1 version) | program, links, kept version |
| Platform service | its pinned upgrade path | only through `platform_upgrade` / the SERVICES tile | the platform engine's own | no: the data may have been migrated | `platform_remove` (PVC, Secrets, vault entries and namespace kept unless asked) |
| Tile (System app) | the system image | no | | | no ("built in") |

There are no separate extensions or tile-apps beyond these: "extension" kinds are the content kinds above, and apps with their own screen are
not supported yet. Skills that shipped with the image and the sample MCP server say *built in*, can be removed and restored from the
built-in catalog as before, and are changed only by a system update. Things written or installed by hand, or from a file, have *no way to
check*; a store that was removed or switched off gives *source gone*; a store that cannot be reached or was never downloaded gives *can't
check offline*; a withdrawn publisher or package gives *revoked*.

**Where an update may come from.** The store that installed the item. Another store is used only if its package is signed by the same
publisher key, and then the update needs a confirmation that names that store (F-APPSTORE-04 still applies: a different publisher is refused).

**Safe upgrade.** The bytes reviewed are the bytes installed (checked again at install). If the new version asks for permissions the
installed one did not, the review marks them **NEW**, and the update is not applied without the yes; unchanged permissions need no card.
Update all skips such items, anything with warnings, pinned items, items from another store and driver packs, and says why. Versions only go
up: an ordinary install of an older package is refused by the high-water mark. The one exception is the explicit **Go back**, which is
recorded, checked like any package, and pins the item at that version so the update does not come straight back (`p` unpins).

**Transaction.** A journal entry is written, the new version is installed, it is checked as in the table, and if the check fails the old
version is installed again from its kept verified copy and the record, pin and previous version are restored exactly. If the process
dies in the middle, the next time the store opens (`Store::recover`) the journal is read and the files are put in the state the record
says; command-line tool links are mended from their record. Command-line tools use a folder per version, never write into the folder
that is running, link by atomic rename, and keep the version before as the one rollback; pip and npm tools alternate between two folders.

**Kept versions.** The previous version's verified package is kept for one generation (`pkgs/<sha>.iyp`, at most the size limit of its
kind), the one before it is deleted. Removing deletes both. The history (the last 12 changes), pins and the previous-version pointers are
in `lifecycle.json`; every change is also a line in `lifecycle.log` (capped) and in the audit log (`store.update`, `store.rollback`,
`store.remove`).

**Uninstall.** The dialog lists what goes (files, registrations, kept copies), what uses it (declared dependencies of installed
packages; MCP servers that run a command-line tool) and its data. Data is **kept** unless the person chooses *Remove and delete data* and
types the item's name. Data means a folder under `<store>/appdata/<kind>/<scope>/<name>` and vault entries named after the item
(`svc.<name>...`, `mcp.<name>...`); no kind creates such a folder today, so for most items the dialog says there is nothing to keep.
Roles are unchanged: a non-administrator cannot remove what is shared or a command-line tool, and an MCP server needs an administrator.

**Checking for news.** *Check* (`c`) refreshes the catalogs and revocation lists and asks each installed command-line tool's vendor for its
newest version, on a background thread; nothing is fetched otherwise. The daily check (STORE, Settings, `o`) stays off by default. The
Installed page shows an **Updates N** chip, **Update all N (U)** opens a review, then runs one item at a time with progress and ends
with a report. Tools that publish only "latest" (the pip and npm based ones) have no version number, so they say *no way to check*;
*Update* still installs the newest. Limit: never run against a real catalog host or vendor, only folders and fake downloads.

**Terminal.** `store list --outdated` (or `--all`), `store update <id>` or `--all`, `store remove <id> [--keep-data|--delete-data
--confirm=<name>] [--yes]`, `store rollback <id>`, `store pin <id>`, `store unpin <id>`. An id is `kind:name` (`skill:meeting-notes`,
`cli:kubectl`) or a bare name. `--permissions` and `--yes` confirm new permissions and warnings.

**Assistant tools.** `store_outdated` reads. `store_update`, `store_remove` and `store_rollback` always ask (they are hard-ask:
never run by Auto or Autopilot, never "allow for the session"), the card names the item, the versions, the source, what is deleted and
what is kept. The assistant cannot grant new permissions or accept warnings; it tells the person to use the tile. Deleting data
needs the item's name typed by the person (`ask_user`), like `platform_remove`. Driver packs and platform services have their own tools.

## 16. Arranging the store your way

Tags, groups and display titles are yours alone: one small file per account (`org-<user>.json` beside the other store files, written
atomically, never sent anywhere), tolerant of removed items (entries for things no longer installed or listed are dropped), and
exported/imported as a file. (F-APPSTORE-25, 26.)

* **Tags**: free words, up to 8 per item, up to 200 different ones, lower case, `Tab` completes from your existing tags. The *Tags*
  manager (`T`) filters (several tags narrow the list), renames (renaming onto an existing tag merges) and deletes.
* **Groups**: named (24 characters), one of 8 colours, ordered, foldable; an item is in one group or *Ungrouped*. Deleting a group
  keeps its items. Up to 32 groups.
* **Display title**: replaces the name in lists. The real name is always the row's subtitle, the first lines of the item's page
  (*Real name*, id, publisher) and the heading of every install and update review, which never use the display title.
* **Group by**: None, My groups, Category (the publisher's categories, read-only), Kind, Source; remembered. Sort adds *updates first*.
  Search also finds by tag, group and display title.
* **Drag and drop** (My groups): drag an item onto a heading or onto another item (the top half puts it before, the bottom half after),
  or a heading onto another heading to reorder groups. A press becomes a drag only after 8 pixels. In other views a drop explains why it
  does nothing.
* **Keyboard**: `m` move menu (digits choose), Shift+Up/Down reorder, `t` tags (Discover: `#`), `e` display title, `n` new group, `v` group by,
  `g` organise menu (Discover: `O`) with rename/move/delete group, export and import. On a heading: Enter or Space folds, `e` renames,
  `k` changes the colour, `x` deletes, Shift+Up/Down moves the group. In Discover `w` (not `m`) chooses for whom a new install is.
* **Export/import**: the organise menu saves `iyakki-organisation.json` through the host's save dialog and imports a file from a path,
  merging (groups of the same name join, tags are added, the file's titles win) or replacing.
* The assistant's `store_search` and `store_info` show a person's title, tags and group (read-only); there is no tool that changes them.


## 17. Custom apps (kind `tile`) and publishing from the IDE

A `tile` package is a web app that runs in its own launcher tile; its format, bridge and rules are in docs/APP-AUTHORING.md ("Tile apps").
Installing one shows the same review as every other kind (trust level, signature, hash, the capabilities in plain words, warnings) and
then adds a tile for the person (or for everyone, by an operator). Updating, rolling back, pinning and removing go through the lifecycle layer
of section 15; removing asks what to do with the app's saved data (kept by default, deleted only after typing its name). The private store
server takes the `tile` kind by default. The IDE's **Publish** button runs the whole route (checks, version, store, signing key, token,
review card, publish, install back to test): docs/APP-AUTHORING.md, "Publish to a private store from the IDE". A withdrawal by the store's
administrator stops new installs but does not remove copies already installed (section 13, Limits).

### 17.1 Publishing from the IDE into the private store deployed on this device

Deploying the private store (the Platform wizard, or the IDE's "Deploy a new private store to k3s…") records the instance and keeps its admin
token in your vault, but it does not add the store to this device's STORE: adding means looking at the catalog key's fingerprint. The
Publish form closes that gap. When a private store is recorded on this device (running or not) and is not yet a catalog, the **Store**
choice lists it first and selects it:

`Private store on this device (not added yet): <name> · key <first words of the fingerprint>`

* Choosing it makes (or reuses) your signing key, then shows one card: the store, its address, the **full catalog key fingerprint** read from
  its `catalog.json`, and where to compare it (the store prints it when it starts: `kubectl logs -n <namespace> deploy/private-store`, and its
  home page shows it). Nothing is added before you press **Add it and publish**. That button runs the same step as Settings, Private store,
  Add this store: the catalog is read again and a key that differs from the one shown is refused; then the source is added with the key
  pinned, chosen as the publishing target, and the normal review card follows (server, package, permissions, trust, files). The token is
  the vault entry `svc.<instance>.admin-token`, found whether the address was recorded as `localhost`, `127.0.0.1` or the device's own
  address, with or without a trailing slash or `/catalog.json`. Only an administrator can add a store; other accounts are told so before
  anything is written.
* A plain `http://` address is accepted only for this device itself, a private network or a loopback; a public address still needs `https`
  (the review card says "plain http" as a warning). A remote store is never added without its key pinned.
* When the store is deployed but its pod is not ready (or the NodePort does not answer), the form says so and names the one next step (wait a
  minute, or open the K8S tile and check pod `private-store` in the namespace); no key is made and no version is raised.
* When a private store is already deployed, the "Deploy a new private store" row reads "already deployed: <instance>, not redeployed" and
  choosing it explains the way forward (Manage, Repair or Reset). Once added, the store is an ordinary row, starred when it is the target.
* The assistant does the same through its own approval card: `store_publish` and `tile_ship` answer "deployed but not added" with the key
  fingerprint and ask to be called again with `add_deployed_store` set to it; the approval card then reads "ADD the private store … key
  fingerprint … and CHOOSE it as the publishing target; then PUBLISH …". A different fingerprint, or an account that is not an administrator,
  is refused and nothing is added.
* Failures say what happened and what to do: a refused connection (pod not ready), 401/403 (token wrong), 404 (not a private store), 409
  (version already there), 413 (too big), 429 (slow down), 5xx (check the pod), a project file or key folder that cannot be written (the
  drive-aware wording of `scope::explain_io`).

Status: tested with a real store server on a loopback socket and recorded deploys only; not yet run against a real cluster NodePort.

## 18. Developer mode: installing from a developer's computer

Developer mode (docs/DEVELOPER-MODE.md) lets a computer that an administrator paired with the device install, upgrade, reinstall and
remove packages over the network (`iyakki dev install`, `push --watch`), and the IDE's Publish dialog offers **This device (developer
mode)** and each paired device next to the private stores. It is **not a second installer**: every package goes through
`Store::review` and `Store::install` above with the source `developer:<computer>` (code: `devmode/ops.rs`).

* **Trust level.** The review replaces whatever level the signature would give with **Developer (local)** (`Level::Developer`, an amber
  shield, "Developer (local)" and a DEV ribbon on the launcher tile); a revocation still wins. The level needs the warning acknowledgement
  like Community and Unsigned; developer mode gives it, because the administrator's switch and the pairing are the consent.
* **Never promoted.** The lifecycle layer (section 15) says *Installed from a paired developer computer: it is never updated from a store*
  and offers no update. A store version of the same app is refused while the developer build is installed (the publisher rule of section 5:
  a different signer), so a developer build is never silently replaced and never silently replaces anything; remove it first.
* **Roles.** The paired key acts as an account of the device; that account's policy decides as in section 2 (yourself: `skills`; everyone:
  `manage_skills`; MCP servers: administrators, for everyone). Unsigned folder builds are signed with the computer's key; unsigned `.iyp`
  files are accepted as Developer (local) for any role that may install, because they can never look like anything else.
* **Out of reach.** Driver packs and kernel modules (`extension` with `ext_kind: driver`) are refused before the review and by the review
  (section 12's rules apply only on the Drivers page); command-line tools and `app` packages are refused as for any source. `iyakki dev
  uninstall` removes only what developer mode installed.
* **Versions.** The same version may be installed again (a rebuild: a warning, not a refusal); lower versions are refused by the
  high-water mark as always.


## Publishing by an agent in one step

`tile_ship` (and `store_publish` with a folder) make the publisher key themselves when the person has none (`my-publisher-key`, a 0600 file in
the data folder; its public half is added to this device's trusted keys, which vouches only for packages that key signs), use the admin token
that deploying the private store kept in the vault when the person typed none, and show one approval card. With no private store they answer
"Deploy the private store first (STORE → Private store → Deploy; one click now creates the registry)". The store server still lists a package
from a key it has not been told about as Community until its administrator adds the key to `trusted-keys.txt`; on this device the same package
shows as Trusted because of the device-side trust above.
