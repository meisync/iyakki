# The downloads manifest: versions and checksums without rebuilding the ISO

Written 2026-10-28. Status: the safe core is built (parse, rules, signature, rollback protection, last-good copy, a `sign`/`verify` tool, two
items wired: the versions of kubectl and helm) and the fetch is built (section 9): once per 24 h and on a click, with the result on the CLIs tab. The
rest of the items are the migration backlog in section 8.

## 1. The problem and the idea

Everything IYAKKI downloads at the person's click (Node.js, k3s, kubectl, helm, gh, Claude Code, ffmpeg, Python, DuckDB, ...) has a version and a
checksum rule written into the code (`clis.rs`, `quickinstall*.rs`, ...) and described in `legal/downloads.txt`. When a vendor ships a new version,
a new IYAKKI release is the only way to follow it. 35 rows of the licence gate are "REVIEW" because they float or have no vendor checksum
([LICENCE-REVIEW-ROWS.md](../project/LICENCE-REVIEW-ROWS.md)).

The idea (owner, 2026-10-28): the version and checksum data becomes one JSON file kept in a GitHub location and **signed by the owner**. The ISO
carries a copy (the offline fallback and the allow-list). The Store pulls a newer one and updates item versions and checksums **without
rebuilding the ISO**. The person still clicks each download as today.

## 2. What the manifest is, and what it is not

* It is **data about items the ISO already knows**: a version, an address, a checksum for each. Nothing else.
* It never contains a script, a command, a licence text, an item the program does not already list, or a switch that turns an item on.
* It does not replace the licence gate: the licence, the "downloaded, not shipped" flag and the allowed hosts are fixed by the ISO's bundled copy.

## 3. Format (`downloads.json`, schema 1)

One JSON object; every unknown field makes the file malformed (strict parsing).

```json
{
  "schema": 1,
  "sequence": 7,
  "issued": "2026-11-03",
  "items": [
    {
      "id": "cli:kubectl",
      "covers": [],
      "licence": "Apache-2.0",
      "proprietary": false,
      "downloaded_not_shipped": true,
      "kind": "float+sum",
      "version": "v1.36.1",
      "url": "https://dl.k8s.io/release/{v}/bin/linux/{arch}/kubectl",
      "hosts": ["k8s.io"],
      "checksum": { "type": "vendor-file", "url": "https://dl.k8s.io/release/{v}/bin/linux/{arch}/kubectl.sha256", "format": "single" }
    }
  ]
}
```

| Field | Meaning | Who may change it |
|---|---|---|
| `schema` | 1. A program that does not know the number refuses the file | nobody (new program release) |
| `sequence` | whole number from 1, grows with every published manifest | every new manifest |
| `issued` | `YYYY-MM-DD`; may not go backwards | every new manifest |
| `id` | the item. `cli:<id>` for the CLI tile's tools; other REVIEW rows keep their `legal/downloads.txt` id | fixed |
| `covers` | other `legal/downloads.txt` rows that are the same program (the Quick install row `nodejs` is the same download as `cli:nodejs`) | fixed |
| `licence` | SPDX id or `LicenseRef-...`, the same text as the `legal/downloads.txt` row | fixed |
| `proprietary` | vendor software that IYAKKI only downloads (Claude Code, Copilot CLI, Apple files) | fixed |
| `downloaded_not_shipped` | must be `true`; the validator refuses `false` | fixed |
| `kind` | `float+sum`, `none-vendor` or `no-pin` (below) | fixed |
| `version` | exact version; empty = the version floats (`float+sum` items follow the vendor) | the manifest |
| `url` | the download address template (`{v}`, `{arch}`, ... as in the code); empty = the manifest does not set one | the manifest, but only if the bundled copy has one |
| `hosts` | the hosts (and their sub-domains) the `url` and the checksum address may name | fixed |
| `checksum` | see below | the manifest, within its kind |

**Kinds** (the three kinds of the REVIEW list, same names as in [LICENCE-REVIEW-ROWS.md](../project/LICENCE-REVIEW-ROWS.md)):

* `float+sum`: the version may float; the vendor's checksum file is fetched and verified at every download. `checksum` must be a `vendor-file`.
* `none-vendor`: an exact `version` is pinned; the vendor publishes no checksum. `checksum` is absent, or (once the owner has taken one) our own
  `sha256`. The item's screen keeps its warning until a checksum exists.
* `no-pin`: nothing to pin in IYAKKI's data (Apple files are hashed per file in the code, `apt` verifies its own indexes). Listed so that every
  REVIEW row is accounted for; the manifest changes nothing for them.

**Checksum.**

* `{"type": "vendor-file", "url": ..., "format": "single" | "lines" | "manifest" | "md5"}`: **preferred** wherever the vendor publishes one. The
  file is fetched from the vendor at download time, never copied into IYAKKI's data, so it cannot go stale.
* `{"type": "sha256", "files": [{"file": ..., "sha256": ...}], "date": "YYYY-MM-DD", "source": "..."}`: our own value, **only where the vendor
  publishes none**. `date` and `source` (where and how the value was taken, e.g. "PyPI file list read over HTTPS") are required. A vendor
  file is never replaced by our own value, and a `none-vendor` item has one `version` that the value belongs to.
* MD5-only vendors (ffmpeg's static build) stay `vendor-file` / `md5` until the owner decides to pin a SHA-256 of one build (REVIEW row "pin or replace").

The signature is **detached**: `downloads.json.sig` holds 128 hex characters, the ed25519 signature of the exact bytes of `downloads.json`.
Nothing is re-serialised before checking, so there is no canonical form to get wrong.

## 4. Threat model

What the manifest protects against, and what it does not.

| Threat | Defence |
|---|---|
| Someone takes over the GitHub repository or the CDN in front of it and serves a malicious manifest | Signature: the private key is not on GitHub. Unsigned, wrongly signed, tampered and malformed files are refused. |
| A signed-but-old manifest is replayed to push people back to a vulnerable version | `sequence` below the highest held (bundled or saved) is refused; `issued` earlier than the one held is refused; the same sequence with other content is refused. |
| The saved copy on the data volume is edited | The saved file is verified again with the built-in key every time it is read; a bad file is ignored and the bundled copy is used. |
| A leaked or careless signing: the manifest names a new host, a new item, a new licence, a proprietary item | Not possible even with a valid signature: the rules (section 5) are checked against the bundled copy, and the `sign` tool runs the same rules before it signs. |
| A bad version string or address injects arguments (`1.0 && ...`, `https://good@evil/`) | Version characters are limited to `[A-Za-z0-9._+-~]` (40 at most); addresses are plain `https://host/path` on the item's own hosts, no user info, no port, no spaces or backslashes, no placeholder in the host. |
| A bad checksum value for a real download | Only the checksum's address or value changes, never its kind; vendor files stay vendor files. A wrong value stops the download (it is a mismatch), it cannot make a bad file pass for a different version. |
| The signing key is lost | Not recoverable by a manifest: a new key means a new ISO (the public key is built in). The ISO keeps working with its bundled copy. Keep a backup of the key (outside the repository). |
| The signing key is stolen | The thief can only do what a valid manifest can do (change versions, addresses on allowed hosts and checksums of existing items) and cannot go below the sequence already used. Rotate by a new ISO with a new public key. |
| **Not covered:** freezing | An attacker (or an outage) can withhold the newest manifest; people keep the last good one. There is no expiry date because many IYAKKI machines have no trustworthy clock. |
| **Not covered:** a compromised vendor | If a vendor publishes a malicious file and its matching checksum, the manifest cannot tell. It limits who may be asked (the allowed hosts) and nothing more. |

The key is an ed25519 key. The code uses `ed25519-dalek` (already in the workspace for the app-store package signatures, `pkg.rs`), `verify_strict`,
and the key encodings of `pkg.rs` (64 hex characters). No new dependency.

## 5. The rules, in one place (`dlmanifest::validate` and `validate_against`)

Every fetched manifest, after the signature:

1. Parses with `schema` 1 and no unknown fields; at most 1 MiB, 200 items; ids are plain and unique.
2. Every `url` and checksum address is `https://` on the item's `hosts`.
3. `downloaded_not_shipped` is `true`; versions use plain characters.
4. **Against the bundled copy**: every id must exist there (a manifest cannot add an item); `covers`, `licence`, `proprietary`,
   `downloaded_not_shipped`, `kind` and `hosts` must equal the bundled values; an item without an address in the bundled copy cannot get one; the
   checksum may change address or value but not kind.
5. `sequence` is above the highest held and `issued` is not earlier than the one held.

Items missing from a fetched manifest keep their bundled data (the file may be partial). The merged result is what the program uses.

## 6. Where things live

| Part | Place |
|---|---|
| The manifest and its signature (published) | `https://raw.githubusercontent.com/meisync/iyakki-downloads/main/downloads.json` and `.../downloads.json.sig` (chosen by the owner 2026-10-28; the constant `dlmanifest::fetch::ADDRESSES`). **Keep the repository name stable**: it is built into every ISO. The list has a second place for a fallback address (a mirror), empty today; an empty place is skipped. The repository may not exist yet: that is a quiet "no update". |
| The ISO's copy (offline fallback and allow-list) | `legal/downloads-manifest.json`, built into the program (`include_str!`), so it is in every image and needs no file on the data volume |
| The owner's public key | `legal/downloads-manifest.pub` (`unset` until the owner has made a key), built into the program |
| The last good fetched manifest | one file, `<data>/cli/dlmanifest/last-good.dlm`: first line the signature, the rest the manifest exactly as signed; written to a temporary file, synced and renamed, so a crash leaves the old one |
| The private key | **never in the repository**, never in the ISO. Created with `./run.sh dlmanifest keygen <path outside the repository>` (mode 600, refuses to overwrite). `sign` reads it from a path given at run time. |

Offline, or with a refused manifest, the program uses the last good copy if it is newer than the bundled one, otherwise the bundled one.
A newer ISO with a higher bundled `sequence` ignores an older saved copy.

## 7. Working with it

```
./run.sh dlmanifest keygen ~/secure/dlm.key          # once; prints the public key to put in legal/downloads-manifest.pub (then a new ISO)
./run.sh dlmanifest gen                              # print the bundled manifest made from the code and legal/downloads.txt
./run.sh dlmanifest check                            # check the bundled manifest and whether a key is built in
./run.sh dlmanifest sign downloads.json --key ~/secure/dlm.key   # rules first, then downloads.json.sig
./run.sh dlmanifest verify downloads.json            # signature with the built-in key, rules, sequence against the bundled copy
```

Making a new manifest: copy `legal/downloads-manifest.json`, raise `sequence`, set `issued`, change `version`/`url`/`checksum` of the items that moved, run
`sign`, then `verify`, then publish the two files.

### Relation to the licence gate (`./run.sh licence-check`)

`legal/downloads.txt` stays the **source of the licence decisions** (reason, date, owner sign-off per row) and is still read by `iyakki-relcheck`
exactly as before: a row with no pin, a missing checksum, an unknown licence or a `shipped` delivery fails; REVIEW rows are listed. The manifest is
checked **against** it, not instead of it: `cargo test -p iyakki-agent --lib dlmanifest` fails when a REVIEW row is not covered, when an item's licence differs from
its row, when a `checksum` row has no vendor checksum file in the manifest, when a `none-vendor` row's version differs from the pin in the manifest, or
when the bundled file no longer matches what `clis.rs` and the rows say (`regenerate_the_bundled_file`, an ignored test, rewrites it).
So the two cannot drift. Generating `downloads.txt` from the manifest was rejected: the file carries human review text that the manifest should not.
A later step may add a manifest cross-check to `iyakki-relcheck` itself (it has no JSON dependency today; backlog).

## 8. Migration order and what stays in code

Done (2026-10-28):

1. The format, the rules, the signature check, rollback protection, the atomic last-good file, the bundled copy of the 35 REVIEW rows
   (23 CLI-tile items covering 8 Quick install rows, plus `mac-firmware`, `mac-camera`, `linux-apps`, `linux-windows`) and the `dlmanifest` tool.
2. Two paths wired as proof: the **version** lookup of `cli:kubectl` and `cli:helm` (`clis::latest_version`). A fetched manifest that names a version replaces the
   `stable.txt` lookup; with no manifest, or a damaged one, the old lookup runs. The checksum is still the vendor's file from the code's address.

Backlog, in this order (each step is small and has its own test):

1. ~~**Fetch and apply**~~ done 2026-10-28, see section 9. (Was: the Store (or a background job) downloads `downloads.json` and `.sig` from the configured address (size limit 1 MiB, https
   only, redirects checked against the host) and calls `Store::apply`. Show "manifest updated, N items changed" (the `changed` list is returned) and
   keep the person's click for every download. Audit entry per accepted or refused manifest.)
2. **Other float+sum items**: version through the manifest for `k3s`, `gh`, `nodejs` (LTS pick stays), `uv`, `opentofu`, `terraform`,
   `grpcurl`, `arduino-cli`, `ollama`, `claude-code`, `github-copilot`; then the `url` and the checksum address.
3. **none-vendor items**: `pin` for pip/npm/zip tools (`aws-cli`, `gcloud`, `esptool`, `azure-cli`, `codex`, `gemini-cli`, `aider`, `python`, `duckdb`) read from the manifest; add our own
   `sha256` with date and source where a value can be taken (PyPI file lists, release digests) and verify it at download.
4. **Fixed digests** (`cli:espflash`, `cli:7zip`: `Checksum::Fixed` in the code) and the cluster images (`digest` rows in `quickinstall_*.rs`, `gpu_*.rs`): out of the REVIEW list but the same mechanism; they
   move when step 3 works. GPU driver and firmware packs (`fwextra`, `fwpack`) have their own table with per-file hashes and stay in code until the owner decides.
5. **Gate**: a manifest cross-check in `iyakki-relcheck licence-check` (or an `--manifest` mode) so the gate itself, not only a test, fails on a row
   without a checksum in the manifest.
6. **Stays in code for good**: the code that downloads, verifies and unpacks; the host allow-lists' fallback; the licence texts and screens; the
   per-file hashes of Apple's files; the decision that an item exists at all. A new item always needs a new ISO.
7. **Release steps**: owner makes the key, puts the public key in `legal/downloads-manifest.pub`, publishes the first signed manifest and sets the address; until then the public key is `unset` and no fetched manifest is accepted (fail closed).

## 9. Fetching it (built 2026-10-28: `dlmanifest::fetch`)

**When.** On its own at most once per 24 hours (the first time the CLIs tab is drawn in a session and the last check is older; after "no connection" an
hour is enough), and at every click on **Check for updates** on the CLIs tab (key R; administrators with a full terminal). It runs on its own thread:
the screen and the boot never wait for it. Nothing runs at boot; the first window that shows the CLIs tab starts it.

**How.** The in-house HTTPS client (`netprobe` over `gitnet::NetGit`, no curl, no new dependency) gets the address, then the address plus `.sig`. Each
request has a 15 s limit; the manifest at most 256 KB (the signature at most 4 KB). The two files go through `Store::apply` exactly as in section 5:
signature with the built-in key, schema, rules against the bundled copy, rollback floor. Only version, address and checksum of items the ISO already
has can change. The saved manifest (`last-good.dlm`) is written only by an accepted manifest, atomically. A failure of any kind (no network, HTTP 404
or any other status, a missing signature file, too large, a bad signature, a rollback, a rule broken) leaves it as it was.

**What is remembered.** `<data>/cli/dlmanifest/fetch-state.json`: the time of the last check, its result in plain words and the notice of the last applied
update. This small file is written after every check (success or failure) because the 24-hour rule and the "Checked 3 h ago" line need it; it never
holds a manifest, a signature or anything secret.

**Results and audit lines** (`audit.log`, detail has no secrets: a sequence number, item ids, a short reason):

| Result | Shown as | Audit line |
|---|---|---|
| a newer manifest accepted | "Checked just now: updated" and, above it, "Download list updated: N items changed (names)" | `dlmanifest.fetch.ok` |
| the same manifest as held | "up to date" | `dlmanifest.fetch.unchanged` |
| signature, rollback, size or rule failed | "refused: signature did not match" (or the other reason in words), in red | `dlmanifest.fetch.refused` |
| no network, no answer, HTTP 404 (repository not there yet), no signature file | "no connection" (quiet: no dialog) | `dlmanifest.fetch.offline` |

The person still clicks every download; a changed version is used by the next click on Install or Update.

**Second path.** `cli:helm` is wired like `cli:kubectl`: a fetched manifest that names a version replaces the `helm-latest-version` lookup; without one the
lookup in `clis.rs` runs. `dlmanifest::WIRED` lists the wired items.

**Not verified** (needs the published repository and a real run): the real GitHub answer (content type, caching of raw.githubusercontent.com: it may
serve a file up to a few minutes old), the signature of the real published file, and a run on the Mac.


## 10. Pinning the REVIEW rows that can be pinned (2026-10-28): what is built, what the owner fills

Four rows of [LICENCE-REVIEW-ROWS.md](../project/LICENCE-REVIEW-ROWS.md) ask for a "pin". The code for all four is in place; **none of the values is
in the repository yet**, because none can be computed offline and none may be invented. With no value the behaviour is exactly as before (the screen keeps
its warning); with a value the download is verified harder and the warning goes away.

| Row | Built | Value still to be filled by the owner | Where the check runs |
|---|---|---|---|
| `cli:aws-cli` | `openpgp.rs`: an in-house OpenPGP detached-signature check (RSA, SHA-256/384/512, armored or binary; no new dependency, about 350 lines with its own big-number code). The `.sig` next to the zip is fetched and checked while the download streams in. | The Amazon public key, ASCII-armored, in `legal/aws-cli-pgp.asc` (the file says `unset` now) | `clis.rs` download, before unpacking; a bad or missing signature = nothing installed |
| `cli:duckdb`, `cli:esptool` | `dlmanifest::pins::pip_lock`: when the active manifest has `sha256` values for the item's exact version, pip runs as `pip install --require-hashes -r requirements.lock` with one `name==version --hash=sha256:...` line per project (several hashes per project are fine). pip refuses any file whose hash is not listed, dependencies included. | The SHA-256 of every file pip may take, **including every dependency**, for the pinned version (duckdb has no dependencies; esptool has several, so it needs the whole set) | `clis.rs` pip install |
| `cli:ffmpeg` | `dlmanifest::pins::sha256_for`: a `sha256` value for the item's exact version and file name is compared with the downloaded file in addition to the builder's MD5 file. | A pinned build (below) and its SHA-256 per architecture | `clis.rs` download, after the MD5 check |

### What the owner runs

```
# 1. AWS CLI: put the key in place, compare the fingerprint with the one on Amazon's page "Installing or updating to the latest version of the AWS CLI"
gpg --show-keys --fingerprint legal/aws-cli-pgp.asc        # then commit the file; `./run.sh licence-check` still lists the row as REVIEW until you write the accepted row

# 2. DuckDB / esptool: take the values of one PyPI release over HTTPS, one command per project (the tool itself and each dependency)
curl -fsSL https://pypi.org/pypi/duckdb/1.3.2/json -o duckdb.json
./run.sh dlmanifest pin legal/downloads-manifest.json cli:duckdb --version 1.3.2 --pypi-json duckdb.json \
    --source "PyPI JSON of duckdb 1.3.2 read over HTTPS" --date 2026-MM-DD
# esptool 5.6.0: the same with `pypi.org/pypi/esptool/5.6.0/json`, then once per dependency version (`pip download esptool==5.6.0 -d /tmp/x` names them;
# `pip install --require-hashes` complains about any that are missing when the check is run). Existing values of the same file name are replaced.

# 3. ffmpeg: the vendor floats and publishes MD5 only. A pin needs three things:
#    a) choose one build from https://johnvansickle.com/ffmpeg/old-releases/ (one version for amd64 and arm64);
#    b) in the code, `clis.rs` ffmpeg: `pin: "<version>"` and the two addresses to the old-releases file (`.../old-releases/ffmpeg-{v}-{arch}-static.tar.xz`
#       and its `.md5`); in `legal/downloads.txt` the two ffmpeg rows from `checksum` to `none-vendor | <version>`;
#       `./run.sh dlmanifest gen > legal/downloads-manifest.json` rebuilds the bundled file (the kind becomes none-vendor with that version);
#    c) download each archive over a link you trust and record it:
./run.sh dlmanifest pin legal/downloads-manifest.json cli:ffmpeg --version <version> --file ./ffmpeg-<version>-amd64-static.tar.xz \
    --source "downloaded from johnvansickle.com old-releases over HTTPS" --date 2026-MM-DD
./run.sh dlmanifest pin legal/downloads-manifest.json cli:ffmpeg --version <version> --file ./ffmpeg-<version>-arm64-static.tar.xz --source "..." --date 2026-MM-DD

# 4. Always last, after any edit of legal/downloads-manifest.json: raise `sequence` by one, set `issued`, re-sign and check
./run.sh dlmanifest sign legal/downloads-manifest.json --key <your key file outside the repository>
./run.sh dlmanifest verify legal/downloads-manifest.json
cargo test -p iyakki-agent --lib dlmanifest && cargo test -p iyakki-agent --test licence_gate && ./run.sh licence-check --no-images
```

`pin` refuses an item that is not `none-vendor`, a version that differs from the item's, a value that is not 64 hex characters, and runs the same
rules as `sign`. It never touches `sequence`, `issued` or the signature. No value in this section was computed or guessed by IYAKKI's authors; the
test `the_bundled_manifest_has_no_invented_values` only checks that any value present carries a date and a source.

### What the PGP check does not do

It does not read expiry, revocation or user ids: the trust decision is "this key file in the repository", so withdrawing a key means changing the file.
Only the first armored block of the key file is read (one key with its subkeys); only RSA keys and SHA-2 signatures are accepted. Tests use two
throw-away RSA-2048 keys made with GnuPG (`crates/iyakki-agent/tests/fixtures/openpgp/`, no private half kept): good armored and binary signatures verify;
a changed file, a damaged signature, another key, truncated or garbage input and a missing `.sig` are refused.
