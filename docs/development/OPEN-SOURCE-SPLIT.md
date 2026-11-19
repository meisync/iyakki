# Open-source split: which parts could be separate libraries

Status: note only (2026-10-27). Nothing is extracted, no repository is created, nothing is published.

Owner request (2026-10-27): "whatever we write like custom mixers etc, can we open source separately (just make note of it, we can create sub git modules, release with MIT free or Apache license)".

How to read this note: every number below was taken by reading the tree (`crates/*/Cargo.toml`, `wc -l` of `src`, grep of `use crate::` lines) on 2026-10-27. Where a claim comes from memory or judgement instead, it says so. Line counts include test files inside `src`. "Entanglement" counts `use crate::` lines inside the candidate itself that point outside it, plus the iyakki-* crates it depends on. A candidate with no outside imports can be copied out as-is.

## 1. Inventory of the workspace

Main workspace (`Cargo.toml`, version 0.1.0-alpha.1, licence Apache-2.0) and the separate `crates/iyakki-gui` workspace (own `Cargo.lock`, `deny.toml`, `src/{desktop,installer,main}.rs`). Lines of Rust are `src` only.

| Crate | Purpose | Lines | Internal deps (iyakki-*) | External deps of note |
|---|---|---|---|---|
| iyakki-agent | The agent runtime: chat, tools, flows host, devmode, web service, mixer, audio, git, dashboards, data tools (huge) | 249,494 | core-api, core-fake, data, devctl, drivers, edit, flow, js, ledger, mem, netscan, proxy, schema, sec, stage, storage, usb, vfs, wasm, web, webview | chacha20poly1305, argon2, zeroize, sha2, miniz_oxide, ruzstd, nanomp3, ed25519-dalek, aes, ctr, rustls, rcgen, libc |
| iyakki-shell | The desktop shell, tiles, rendering, input, apps (huge) | 235,616 | agent, data, devctl, drivers, edit, flow, fw, hwinfo, js, ledger, netscan, proxy, rfb, schema, sec, storage, usb, web, webview, zram | tiny-skia, fontdue, rustybuzz, unicode-bidi, argon2 |
| iyakki-web | Reader browser engine: URLs, HTML, CSS, SVG, JPEG/GIF/WebP decoders, content blocker, cookies, cache | 18,837 | none | none (dependency-free per its Cargo.toml) |
| iyakki-flow | Flow engine (automations) | 12,169 | schema | serde, serde_json |
| iyakki-usb | Safe USB image writer | 11,364 | storage, zram | libc, sha2 |
| iyakki-drivers | Driver packs, firmware placement, vermagic checks | 7,905 | hwinfo | sha2, miniz_oxide |
| iyakki-core | The core binary wiring everything (daemon) | 6,249 | many (18) | tokio |
| iyakki-js | QuickJS-ng in WebAssembly with fuel/memory/time limits and web host modules | 5,104 | proxy, web | wasmtime |
| iyakki-remote | Shell in a web browser: HTTPS/WebSocket server, sign-in, screen streaming | 4,721 | agent, fw, sec, shell, web | rustls, rcgen, tiny-skia |
| iyakki-devctl | Network/Bluetooth/display/sound hardware switching | 4,392 | sec | libc |
| iyakki-fw | Firewall: policy model, ruleset generator, in-house nf_tables netlink writer, apply/confirm/revert | 4,268 | schema | libc |
| iyakki-storage | Disk detection, data volume, persistence | 4,104 | none | libc |
| iyakki-edit | Text editing engine: undo buffer, search, highlighting, line diff | 3,297 | none | none |
| iyakki-netscan | LAN scan (model servers, cameras, printers) and the mDNS responder/resolver | 3,177 | none | ureq |
| iyakki-relcheck | Release checks: link checker, secret scan, licence gate, SBOM, version consistency | 3,179 | none | none |
| iyakki-data | Data tools (polars based) | 3,119 | none | polars |
| iyakki-webview | Wire protocol to the embedded full web engine helper, client, fake helper | 2,331 | none | none |
| iyakki-proxy | Proxy/fetch with TLS and content handling | 1,480 | sec, wasm, web | ureq, rustls, webpki-roots, rcgen |
| iyakki-zram | Compressed swap, in-house mkswap | 1,275 | none | libc |
| iyakki-stage1 | Tiny first-stage init | 1,319 | none | libc |
| iyakki-rfb | RFB (VNC) client: state machine, Raw/CopyRect/Hextile/ZRLE, in-house inflate | 1,150 | agent (see below) | none |
| iyakki-sec | Security policy primitives | 1,049 | none | libc, serde |
| iyakki-wasm | Wasm sandbox for apps | 931 | ledger, sec, vfs | wasmtime |
| iyakki-core-api | Contract between apps and core (traits, serde types) | 617 | sec | serde |
| iyakki-hwinfo | Reads hardware from /proc and /sys | 530 | storage | none |
| iyakki-schema | Versioned JSON files: migrations, atomic write, backup, refuse newer | 413 | none | serde, serde_json |
| iyakki-ledger | Hash-chain ledger | 313 | none | sha2 |
| iyakki-hw | Audio/camera hardware | 300 | sec, vfs, wasm | cpal, v4l |
| iyakki-swarm | libp2p swarm | 301 | wasm | libp2p-* |
| iyakki-ui | UI component contract | 301 | edit, sec, vfs, wasm | none |
| iyakki-vfs | Virtual filesystem trait | 257 | none | none |
| iyakki-mem | Memory | 238 | vfs | none |
| iyakki-core-fake | In-memory fake of the core API | 231 | core-api, sec | none |
| iyakki-store-server | Private store server binary | 48 | agent | none |
| iyakki-gui (own workspace) | Desktop window and installer (eframe) | 1,090 | agent, core, data, proxy, shell, storage, ui, usb | eframe |

Two things stand out. First, `iyakki-agent` and `iyakki-shell` hold about 485,000 of the roughly 620,000 lines, so the best libraries are modules inside them, not whole crates. Second, many leaf crates already have no iyakki dependency (web, edit, netscan, relcheck, webview, storage, zram, schema, vfs, ledger, sec).

Verified oddities: `iyakki-rfb` lists `iyakki-agent` in its Cargo.toml although the crate is described as "no dependencies" (grep of its sources shows only `use crate` lines; the agent dependency may be a dev-dependency or a leftover; check before extraction). Full per-file dependency data for `iyakki-flow` and `iyakki-webview` were counted together with rfb and schema: 56 `use crate::` lines and one `use iyakki_webview` line (in flow, rfb, schema, webview sources combined), so they are self-contained apart from `iyakki-schema` for flow.

## 2. Candidates for standalone libraries

Licence column: "dual" means `MIT OR Apache-2.0`. Effort: S = days, M = 1 to 2 weeks, L = several weeks. Names are neutral and still need a crates.io and GitHub name check (not done).

| # | Component and where | Lines | Entanglement (verified) | Proposed repo | Licence | Effort | Value for outsiders | Risks |
|---|---|---|---|---|---|---|---|---|
| 1 | Audio mixer: `crates/iyakki-agent/src/mixer/` (engine 703, mod 594, resample 91, sink 236, stress_tests 391, tests 531) plus `audiosink.rs`, `audiosink_stream.rs`, `audiosink_tests.rs` (737) | about 3,900 | Mixer files use only `super::` and std (grep: no `crate::` outside the mixer). `audiosink.rs` has one `use crate::mixer`. Takes the whole `mixer/` directory and the three audiosink files. `sink.rs` uses `std::process` (spawns an audio tool; not checked which) | `softmix` (or `pcm-mixer`) | dual | S | Pure-Rust software mixer with resampling, no dmix and no C libraries; Rust audio crates are mostly cpal wrappers, so a mixer with stress tests has some demand | Real-time audio code: glitches become bug reports. API not settled (it is "one audio entry point" for IYAKKI). Sink is Linux/ALSA specific |
| 2 | Word timeline and heard-position follower: `crates/iyakki-agent/src/wordtime.rs` | 1,659 | Zero `crate::` imports (grep). One file | `wordtime` | dual | S | Niche (karaoke style highlighting, TTS read-along); useful but small audience | Low risk. Needs the doc comments checked for IYAKKI-specific wording |
| 3 | mDNS responder and resolver: `crates/iyakki-netscan/src/mdns/` (wire 306, responder 506, net 447, tests 538, mod 32) | 1,829 | Only `super::` imports. The rest of netscan (LAN scan) uses `ureq` and stays behind | `mdns-lite` | dual | S | Dependency-free mDNS with a responder (not just a client) has outside demand; existing crates are heavier (async runtimes) | Network-facing parser: fuzz it before release. Multicast behaviour differs across OSes (Linux-tested; macOS/Windows unknown) |
| 4 | Versioned JSON store: `crates/iyakki-schema` | 413 | No iyakki deps. Serde only | `jsonstore-migrate` | dual | S | Small and clean, "refuse to overwrite a file written by a newer release" is a nice rule | Small; many similar crates exist. Low value alone, good as a proof run for the process |
| 5 | Release checks and licence/secret gate: `crates/iyakki-relcheck` (links 280, secrets 213, cargolic 181, spdx 226, sbom 208, imagescan 439, gate 256, profile 242 ...) | 3,179 | No Cargo deps. 20 `use crate::` lines inside itself only. But content is IYAKKI-shaped (Buildroot image scan, legal-info, FEATURES totals, `./run.sh` profile) | `releasegate` | dual | M | A zero-dependency markdown link checker, secret scan, SPDX check and SBOM generator is useful to any project | The image scan and FEATURES parts are IYAKKI specific and would need splitting into a generic core plus a project profile. Security-sensitive (a bad secret scan gives false comfort); say clearly it is not a substitute for gitleaks |
| 6 | Firewall rule engine: `crates/iyakki-fw` (model, validate, ruleset, in-house nf_tables netlink writer, apply/confirm/revert, blocked log) | 4,268 | Depends on `iyakki-schema` (small, extractable together). 29 internal `use crate::` lines across files. `nl.rs` and `backend.rs` are the netlink writer | `nft-netlink` (writer only) or `nft-policy` | dual | M | Netlink nf_tables writer without libnftnl and without shelling out to `nft` is rare; confirm-or-revert apply is a useful pattern | Security-sensitive: it changes a host firewall. Kernel ABI drift; Linux only; root needed to test. Split the netlink writer from the policy model first |
| 7 | RFB (VNC) client: `crates/iyakki-rfb` | 1,150 | Cargo.toml lists `iyakki-agent` (unverified why). Sources are self-contained | `rfb-client` | dual | S to M | Raw/CopyRect/Hextile/ZRLE with an in-house inflate and a pure state machine (no I/O) is easy to embed | Remove the agent dependency first. Parser for untrusted server data: fuzz. Small feature set (no encryption beyond what the link gives) |
| 8 | Flow engine: `crates/iyakki-flow` | 12,169 | Depends on `iyakki-schema` only. Contents are IYAKKI semantics (agent flows, approvals) | `flowkit` (name TBD) | dual (later) | L | An automation engine others could embed, but only if the model is generic | Large, opinionated, and the central product feature. Publishing it early freezes its API and invites forks. Recommend not in the first wave |
| 9 | Webview protocol: `crates/iyakki-webview` and `docs/WEBVIEW-PROTOCOL.md` | 2,331 | No deps. Self-contained (client, wire, fake helper) | `webview-wire` | dual | S | Only useful if the protocol is adopted by others; today only IYAKKI's helper speaks it | Low value without a second implementation; an API promise for little return |
| 10 | Web service route table and OpenAPI layer: `crates/iyakki-agent/src/webservice/` (server 350, auth 190, config 183, handlers 200, runs 191, routes 133, mod 146, openapi 98, real 82, tls 70, tools 72, deps 38; tests 931) | about 2,500 | Entangled: imports `crate::agent` (6), `crate::chat` (4), `crate::runs` (3), `crate::httpmini` (3). `deps.rs` looks like an existing seam | `route-table` (generic part only) | dual | M | Route table that also emits OpenAPI is nice, but the generic part is probably under 600 lines (routes, openapi, part of server) | Needs a refactor to separate generic from agent handlers. Web server code is security-sensitive (auth, TLS) |
| 11 | HTTP helper: `crates/iyakki-agent/src/httpmini.rs` and TLS config bits (`webservice/tls.rs`, `iyakki-proxy`, `iyakki-remote`) | 249 plus | httpmini has zero `crate::` imports. proxy and remote pull rustls, rcgen, ureq | none for now | n/a | n/a | Tiny. Crates like `ureq` and `httparse` already do this | Do not publish: low value, a security-sensitive surface |
| 12 | Markdown renderer: `crates/iyakki-shell/src/mdtext.rs` | 600 | One `use crate::` line; rendering side tied to shell fonts/UI types (not inspected closely) | none yet | n/a | M | Unclear: it renders into shell text runs, not HTML | Split into parser (reusable) and renderer (IYAKKI) first |
| 13 | SVG renderer: `crates/iyakki-web/src/svg.rs` | 2,633 | One `use crate::` line (probably the image type) | `svg-lite` (with image decoders, see 14) | dual | M | Dependency-free SVG rasteriser is useful; competes with resvg (MPL-2.0, much more complete). Be honest about the subset | Untrusted input (SVG from web pages): fuzz and resource limits needed. Conformance expectations |
| 14 | Image decoders: `jpeg.rs` 612, `gif.rs` 402, `webp.rs` 2,241, `image.rs` 104 in iyakki-web | 3,359 | One `use crate::` line each (the shared image type); easy to carry as a small crate | `pixeldecode` (jpeg, gif, webp, no deps) | dual | S | Pure Rust, no unsafe (not verified, grep for `unsafe` before claiming), WebP lossy/lossless decoding without libwebp is wanted | Decoders of untrusted data: highest risk class here. Needs fuzzing and an honest "subset" statement (progressive JPEG? animated WebP?: check). The `image` and `image-webp` crates already exist, so the pitch is "tiny and dependency-free" |
| 15 | In-house archive readers: `crates/iyakki-agent/src/archive.rs` | 618 | Zero `crate::` imports. Uses `miniz_oxide`/`ruzstd` from the agent's dependency list (assumed, verify) | `archive-readers` | dual | S | zip/tar/zst reading with small footprint | Untrusted input (zip slip, bombs). Existing crates (`zip`, `tar`) are mature, so little value unless it is dependency-minimal |
| 16 | Text editing engine: `crates/iyakki-edit` | 3,297 | No deps. Used by iyakki-ui, agent, shell | `edit-buffer` | dual | S | Undo buffer, search, light highlighting, line diff with per-hunk review, no dependencies. Ropey and similar exist; the per-hunk diff review is the differentiator | API tied to IYAKKI's editor, stability promise needed |
| 17 | Other leaves | | `iyakki-zram` (1,275; in-house mkswap and zram setup), `iyakki-storage` (4,104), `iyakki-ledger` (313 hash chain), `iyakki-usb` (11,364 safe image writer) | later | dual | S to M | zram and ledger are small utilities; the USB writer is useful but destructive (it erases disks) | usb and storage are dangerous if misused. Not in the first wave |
| 18 | Browser engine as a whole: `iyakki-web` (18,837, no deps) | 18,837 | No iyakki deps; HTML, CSS, URL, content blocker, cookies, cache, readers | `reader-engine` | dual | L | "Dependency-free reader browser engine" is a headline, but the blocker lists and loader policies are product specific | Big surface, a promise to keep up with the web. Prefer to publish pieces (svg, decoders, url) first |

Not candidates: iyakki-core, iyakki-shell, iyakki-agent, iyakki-gui, iyakki-remote, iyakki-drivers, iyakki-devctl, iyakki-hwinfo, iyakki-store-server (product wiring, device specific, or entangled).

Note on `iyakki-js` (QuickJS-ng under wasmtime with fuel limits): reusable idea, but it depends on `iyakki-web` and `iyakki-proxy` for host modules and the QuickJS-ng build is a vendor download. Not listed.

## 3. How to do it with git

### Choosing the path

| Option | What it is | For | Against |
|---|---|---|---|
| A. Extract and keep as a path dependency in the monorepo (no split) | Leave code where it is, add a README and a `publish = false` guard | Free | Not what the owner asked for |
| B. Separate repository, added to the main repo as a **git submodule**, consumed by `path = "vendor/softmix"` | The owner's "sub git modules" idea | One checkout, offline builds, exact pin per commit, no registry needed, no name squatting issue | Submodules confuse contributors and CI (need `--recursive`); every library change is a two-step (library commit, then bump pointer); Buildroot and the image build scripts must fetch submodules |
| C. Separate repository, **published to crates.io**, consumed as `softmix = "0.1"` | Standard Rust way | Simple for outside users and for IYAKKI; semver does the pinning; docs.rs gives documentation | Needs a name check and an owner decision to publish; yanking is not deletion; the source and its history become permanent; IYAKKI builds need network or a vendored copy (the project already builds offline from `vendor`/cache? unverified) |
| D. Separate repository, **cargo git dependency** (`git = "...", rev = "..."`) | Middle path | No submodule and no registry; pins by commit | Needs network at build; Cargo.lock holds the pin; crates.io itself refuses git deps for publishing the main project (not an issue while IYAKKI is not on crates.io) |

Recommendation: C for libraries that are truly independent (crates.io for discoverability), with B only as a transitional step while an API is still changing (a submodule lets IYAKKI and the library evolve in lock step). Do not choose D for a release image: network at build time conflicts with the "incremental, reproducible builds" goal. If the owner does not want crates.io yet, B is acceptable and loses nothing; publishing can happen later from the same repository.

### Keeping history

Prefer `git filter-repo --path crates/iyakki-netscan/src/mdns --path-rename ...` on a fresh clone (it rewrites history to only the touched paths and removes the rest; needs the `git-filter-repo` tool, which is a separate Python script under GPL-2.0; it is a developer tool, not shipped, so fine). Alternative: `git subtree split --prefix=crates/iyakki-schema -b split/schema`, which keeps history of one directory with no extra tool, but cannot rename files or merge several paths. Use filter-repo when a component is a few files inside a big crate (mixer, wordtime, mdns); use subtree split when the component is a whole crate (schema, rfb). After splitting, make the monorepo depend on the new repository and delete the local copy in one commit, so the code never lives in two places.

Run on a throwaway clone, never in the working repository. History carries every author's name and email as committed; the owner must accept this, or squash to a single initial commit (loses history and blame, gains privacy).

### Per-repository setup

- Licence files: `LICENSE-MIT` and `LICENSE-APACHE` at the root, `license = "MIT OR Apache-2.0"` in Cargo.toml, a `NOTICE` file for the Apache part (copy the structure of the existing `NOTICE`, name the project, copyright holder as the owner's chosen entity; no personal names without the owner's agreement).
- CI per repository: build, `cargo test`, `cargo clippy`, `cargo deny check` (each repo gets its own `deny.toml`, like `crates/iyakki-gui/deny.toml`), plus MSRV. The existing `.github/workflows/ci.yml` is the starting point (not read in detail for this note).
- Contribution terms: DCO (`Signed-off-by` line, enforced by a bot or a check). Recommended over a CLA because it asks nothing of contributors beyond the right to submit, needs no legal entity to collect agreements, and works with dual licensing as long as the DCO text and the CONTRIBUTING file say contributions are licensed under the repository's licence (`MIT OR Apache-2.0`). A CLA is only needed if the owner wants to relicense later without asking contributors. Check `CONTRIBUTING.md` in the main repo: a grep for DCO or Signed-off there found no match (verified), so the text needs adding.
- Docs: README with scope and honest status, CHANGELOG, SECURITY.md pointing to a contact address, a "not for production yet" notice for 0.x.

### Licence recommendation

Dual `MIT OR Apache-2.0` for every library, as the Rust standard library and most of the ecosystem do. Reasons: Apache-2.0 gives an explicit patent grant and matches the main repository; MIT is accepted by users who cannot or will not take Apache (some GPL-2 only combinations); the owner named both. Cost: two licence files and two sets of notices per repo, `cargo deny` allow-lists must list both (they do not conflict), and a contributor submitting code grants both. It also means the library authors cannot later restrict either grant for already released versions. Single Apache-2.0 is acceptable if the owner wants one file less (it is what the main project uses); single MIT drops the patent grant and is the weakest choice for code that implements codecs and protocols. Keep the main product on Apache-2.0 (the existing release decisions say Apache-only base; unchanged).

### Order of extraction (leaf first)

1. Components with zero outside imports and no dependencies: `wordtime`, `mdns`, `schema`, mixer (needs one extra step: move `audiosink` along, or leave it behind as the adapter).
2. Parsers and decoders after a fuzzing pass: `rfb-client`, image decoders, svg, archive readers.
3. Items needing refactor: `releasegate` (split generic from profile), `nft-netlink` (split from policy), route table.
4. Large or central parts: flow engine, browser engine, last or never.

## 4. Before publishing anything

1. **Licence audit.** Run `./run.sh licence-check` (relcheck `licence-check .`; gate point 7 per `run.sh`, documented in `LICENSING.md`) on the main workspace first, then on each extracted repo with its own `cargo deny check`. Only permissive dependencies (the standing rule). Confirm copyright headers say what the owner wants.
2. **Personal names and home paths.** The grep for `afos` or `/home/` (case-insensitive) in `*.rs` and `*.toml` returned hits in: `iyakki-usb/src/drives.rs`, `iyakki-core/src/homebind.rs`, `iyakki-core/src/host.rs`, `iyakki-storage/src/loss_tests.rs`, `iyakki-storage/src/loss.rs`, `iyakki-relcheck/src/sbom.rs`, `iyakki-sec/src/policy.rs`, `iyakki-agent/src/linuxwin_tests.rs`, `iyakki-agent/src/datatools.rs`, `iyakki-agent/src/wtool.rs`. None of these is in the first-wave candidates except `relcheck/sbom.rs`; most are likely legitimate `/home/` paths in code (not checked). Each hit needs a look. Also grep Cargo.toml `authors`/`repository`, docs, test fixtures, and git author names in history (see 3).
3. **No secrets in history.** Use the existing secret scan (`iyakki-relcheck/src/secrets.rs`, run through `./run.sh alpha-check`) on the working tree, but it only reads the tree. For history, run an external scanner such as gitleaks (`gitleaks detect --source <clone> --log-opts="--all"`) or, without installing anything, `git log -p --all | grep -E` for key patterns (slow on a large history, so run it on the filtered clone only, which is small). Run it after filter-repo, because only the filtered history will ever be published. Gitleaks is MIT licensed and a developer tool only; the "third-party tools, installer only" rule applies to what is shipped, not to what the developer runs. Rotate anything found; do not rely on deleting it.
4. **README with scope and honest status**: what it is, what it is not (for example "a subset of WebP; no animation"), version 0.x, security status (not audited), platforms tested (Linux only unless proven), how it differs from existing crates.
5. **Tests that run standalone**: `cargo test` must pass in the new repo without IYAKKI crates. The mixer's `tests.rs` and `stress_tests.rs`, mdns `tests.rs` and others exist and are inline (good); check that nothing needs a device, a network or a `testdata` file from the monorepo.
6. **Docs and doc-comments free of IYAKKI-only wording** (for example "the shell", "the tile", `docs/...` links that will not resolve).
7. **Fuzz targets** for every parser (mdns wire, rfb, jpeg, webp, gif, svg, archive) before announcing. In-house, `cargo-fuzz` is MIT/Apache; a simple random-input loop test is the zero-tool fallback.
8. Check names on crates.io and GitHub, and trademark: "IYAKKI" must not be part of library names (neutral names per the owner's earlier rule).

## 5. Recommendation

First three components:

| Rank | Component | Why |
|---|---|---|
| 1 | Audio mixer plus audiosink (`softmix`) | The owner named it. About 3,900 lines with 900 lines of stress tests, no outside `crate::` imports in the mixer, effort S. Real outside value (pure Rust, no dmix, no C). Risk is only real-time quality and the Linux-only sink; state that in the README |
| 2 | mDNS responder and resolver (`mdns-lite`) | About 1,800 lines, only `super::` imports, tests included (538 lines), dependency free, rare in that form. Parse-of-network-input risk is handled by a fuzz pass before the announcement |
| 3 | `wordtime` follower (single file, 1,659 lines, zero imports) or `iyakki-schema` | Cheapest possible extraction, good as the process rehearsal (filter-repo, licence files, CI, DCO, publishing). Recommend `wordtime` as the third since the owner asked about the audio family, and `schema` as the pipeline rehearsal if the owner wants the smallest risk |

Next wave (after the process works): RFB client, image decoders and SVG (after fuzzing), relcheck (after the generic split), nft-netlink (after the split and a security review). Do not publish: flow engine, browser engine, shell, agent, core.

### Open questions and owner decisions (with recommendations)

| # | Decision | Recommendation |
|---|---|---|
| 1 | Licence of the libraries | `MIT OR Apache-2.0` dual; main product stays Apache-2.0 |
| 2 | Submodule or crates.io | Separate repos first. crates.io for stable leaves once names are checked; submodule only while an API is moving. Not git dependencies for images |
| 3 | Which GitHub organisation or account owns the repos, and what copyright holder text goes in the licences | Owner to name an entity; no personal names in the repo unless the owner says so (existing rule) |
| 4 | Keep history (filter-repo) or squash | Keep history for the first three after the author-name check; squash if the owner does not want author emails public |
| 5 | DCO or CLA | DCO |
| 6 | API stability promise | 0.x with explicit "may break" notice until IYAKKI has used the API for one release; no promises for parsers beyond bug fixes |
| 7 | Maintenance commitment (issues, security reports) | A SECURITY contact and a stated "best effort" policy in each README; fewer repos is less work, so start with three |
| 8 | Publish to crates.io at all | Decide after the first repo is public and the process has cost known; reserve names early only if the owner agrees (reserving by publishing an empty crate is discouraged) |
| 9 | Fuzzing gate for parsers | Yes, before announcing any parser |
| 10 | Does extraction come before or after alpha.2/beta | After alpha.2 scope is frozen. It changes the dependency layout and the build, and a Buildroot or image build in progress must not be disturbed. Do the first extraction as one isolated change |

### What this note did not verify (from memory or judgement)

- Whether `iyakki-rfb` really needs `iyakki-agent` (Cargo.toml lists it; source imports not traced).
- Which external crate each candidate uses at the module level (the mixer and wordtime show only std imports; archive.rs imports were not listed by crate, only that `crate::` is absent).
- Absence or presence of `unsafe` in decoders; supported feature subsets of JPEG, WebP, GIF, SVG.
- What `mixer/sink.rs` spawns through `std::process`.
- How the builds consume dependencies (offline or not), the contents of `.github/workflows/ci.yml`, and `CONTRIBUTING.md` beyond the DCO grep.
- Market claims ("rare", "wanted") are judgement from memory of the Rust crate ecosystem and were not checked against crates.io; crates.io and GitHub name availability was not checked.

## Owner decisions (2026-10-27)

* **No crates.io for now.** Components are separate repositories used as **git submodules** of the main repository, and only where it is really worth it (a component other people can benefit from). Publishing to the registry is revisited after the first repository is public (this replaces the recommendation to publish stable leaf crates).
* Everything else in the decision table above stands as a recommendation until the owner answers it: library licence (`MIT OR Apache-2.0` recommended), the account or organisation and copyright holder text, history handling, DCO text in `CONTRIBUTING.md`, 0.x stability notice, fuzzing before announcing parsers, and timing after the alpha.2 scope freeze.
* Image builds must not depend on a network fetch of a submodule at build time: the submodule is checked out and pinned by commit in the main repository, and the licence gate runs on it like any other path.

## The browser: what can be separate (owner question, 2026-10-27)

There are two different things called "our browser":

1. **`iyakki-web`, the in-house reader engine** (18,837 lines of Rust, no dependencies at all): URLs, HTML, CSS, SVG, JPEG, GIF and WebP decoders, a content blocker, cookies and a cache. It can be a standalone repository. It is a reader engine, not a full modern browser: it does not run today's script-heavy sites. `iyakki-js` (QuickJS-ng as WebAssembly, 5,104 lines) sits on top of it and on `iyakki-proxy`, so a "reader browser with script" needs those two as well. Before publishing: fuzz the HTML, CSS and image parsers (they read untrusted input), and write a README that says plainly what it renders and what it does not.
2. **The full Browser tile**, which embeds **WPE WebKit**. WebKit is upstream software (BSD and LGPL), not ours, and it is linked dynamically. What is ours and could be separate: the wire protocol and client `iyakki-webview` (2,331 lines, no dependencies, with a fake helper), the helper program `dist/webview/wpe-helper/helper.c` (about 1,260 lines, Apache-2.0, links the LGPL WebKit library dynamically), and the protocol document. The tile itself (permission prompts, developer tools panel, media indicator, scrolling) lives in `iyakki-shell` and is entangled with the shell, so it is not a candidate. Publishing the helper as source is fine; shipping a binary that includes WebKit needs the LGPL conditions (source offer, replaceable library), which the image's `legal-info` already handles.

Recommendation: publish `iyakki-web` first (after fuzzing), as its own repository and submodule, because it is dependency-free and useful on its own. Publish the helper and protocol together as one small repository only if someone else wants to embed WPE over a process boundary. Do not try to publish the Browser tile.
