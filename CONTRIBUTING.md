# Contributing to IYAKKI

**For:** anyone who wants to build, change or report on IYAKKI.
**You will get:** how to build and test, the rules that tests enforce, where to add a tile, tool, skill, flow node or connector,
and how to report problems. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first.

## Build and test

```
cargo build --workspace                 # needs Rust (rustup); libasound2-dev libudev-dev pkg-config for the hardware features
cargo test --workspace                  # the whole automated suite (a few thousand tests)
./run.sh test                           # the same, plus a build of the desktop window (crates/iyakki-gui, a separate workspace)
cargo deny check licenses sources       # licence audit (also run in crates/iyakki-gui)
./run.sh window                         # try it
./run.sh screens                        # render every screen to PNG files in /tmp/iyakki-screens
./run.sh alpha-check [--dry]            # the release gate (docs links, versions, secrets, licences, QEMU smoke tests)
dist/buildroot/build.sh libre           # the ISO (30-40 minutes first time, downloads about 1 GB)
```

**Do not rebuild everything.** Images take 30 minutes to 10 hours. Run `dist/buildroot/changed.sh <variant>` first and reuse the existing images
when it says `NOTHING`; for a changed program or overlay use `dist/buildroot/repack.sh <variant>`; share caches as described in
[`docs/BUILD-INCREMENTAL.md`](docs/BUILD-INCREMENTAL.md).

QEMU tests of images: `dist/buildroot/qemu-test.sh`, `qemu-persist-test.sh`, `qemu-install-test.sh` (see
[`docs/LOCAL-TEST.md`](docs/LOCAL-TEST.md)). The CI workflow is `.github/workflows/ci.yml`.

**Tests leak temp folders.** Many tests create `/tmp/iyakki-*` directories. On a machine where `/tmp` is a small tmpfs this can exhaust
its **inodes** and every tool starts failing with `ENOSPC: no space left on device` although `df` shows free space (check
`df -i /tmp`). Run tests with their own temp folder: `mkdir -p ~/.cache/iyakki-tmp && TMPDIR=$HOME/.cache/iyakki-tmp cargo test
--workspace`. If it already happened: `sudo mount -o remount,nr_inodes=4194304 /tmp` (not persistent) and `rm -rf /tmp/iyakki-*`.

## Code style

* Rust 2021, `cargo fmt` defaults are fine; keep functions small and name tests as sentences
  (`the_hardware_pages_are_admin_only`). A change comes with tests that exercise it.
* **Prefer writing it ourselves over adding a dependency.** If a dependency is needed it must be **permissive** (Apache-2.0, MIT,
  BSD, ISC, Zlib, Unicode, CC0, ...); `deny.toml` enforces it and CI fails on copyleft or unknown licences. Check licences before you add
  a crate or bundle a font, and update `THIRD_PARTY_LICENSES.txt` (`cargo about`, see [`docs/LICENSES.md`](docs/LICENSES.md)).
  Base programs (BusyBox, kernel...) are only ever executed, never linked.
* Never block the UI thread: long work goes on a thread and reports over a channel that the tile polls on `Tick`.
* Never write to a drive without the person confirming its name and size; destructive actions always ask first.
* Secrets: never in arguments, logs, tests or settings files; use vault references. The release check scans for secrets.
* Words: English first, plain and short; product name **IYAKKI**; tile names as they appear in the UI (CHAT, AI IDE, NET SCAN...).

## The rules that tests enforce

**The feature matrix (`FEATURES.md`).** A feature is not finished until it has a row and tests that exercise it. One row per
feature: ID `F-<AREA>-<nn>` (next free number; IDs are never reused), phase, status (`done`, `partial`, `planned`, `blocked`,
`deprecated`), priority, progress, the tests, notes. `done` needs 100%, `planned` 0%, `partial` 1-99%, `blocked` below 100%. `done`, `partial` and `deprecated` rows need at least one test
(or a `script:` check that exists); `partial`, `blocked` and `deprecated` rows need a note saying why (a deprecated row names its replacement);
the totals line between `<!-- summary:start -->` and `<!-- summary:end -->` must match the rows. Check with
`cargo test -p iyakki-core --test feature_matrix`. A row may not cite a test that does not exist
(`crate::path::test_name`, crate directory under `crates/`).

**The boundary (`BOUNDARY.md`).** Code outside a listed file may not start a process or make a system-level call. To touch the
base OS from a new place, add the row (file, program or calls, why, what could replace it) in the same change.
`cargo test -p iyakki-core --test boundary` fails otherwise, and also fails when a listed file no longer needs its row. No patches to
upstream sources (kernel, Buildroot, base programs).

**Licences.** Permissive only; `cargo deny check licenses sources` in both workspaces.

**Docs.** Every command, key, path and menu name you document must be true for the tree. The link checker
(`./run.sh alpha-check`, `iyakki-relcheck links`) fails on broken relative links.

## Adding things

| You want to add | Do this |
|---|---|
| **A tile** | 1. Add the app id to `App` (and `ALL`, `id`, `label`) in `crates/iyakki-sec/src/policy.rs` and to the role templates. 2. Write `crates/iyakki-shell/src/apps/<name>.rs` implementing `App` (`title`, `icon`, `accent`, `hint`, `help`, `handle`, `render`); register the module in `apps/mod.rs`. 3. Add the `Tile` to `TILES` and the arm to `Shell::open` in `crates/iyakki-shell/src/shell.rs`, the group in `crates/iyakki-shell/src/launcher.rs` (`Group::of`) and an icon in the `Icon` enum of `gfx.rs`. 4. Add it to the `screens` example (`crates/iyakki-shell/examples/screens.rs`). 5. FEATURES row, tests, a USER-GUIDE entry. |
| **An assistant tool** | Implement it in a module of `crates/iyakki-agent/src/` (see `datatools.rs`, `k8stools.rs`), register its spec and risk (Safe, Ask, or `hard_ask` in `agent.rs` if it must always ask), apply the policy checks and path scopes, test with a fake backend (`assistrun_tests.rs` shows how). Add it to the tool prompt only if the model needs to know. |
| **A skill** | A Markdown file with `name`, `description`, `triggers` and instructions. Built-in: `dist/skills/` plus `BUILTIN_SKILLS` in `crates/iyakki-shell/src/samples.rs`. Distributed: `iyakki pkg build ... --kind skill`, signed ([`docs/STORE.md`](docs/STORE.md)). |
| **An MCP server** | No code in IYAKKI: add it in the MCP tile; the client is `crates/iyakki-agent/src/mcp.rs`. |
| **A flow node** | Implement `NodeKind` in `crates/iyakki-flow/src/kinds/` and register it in `kinds::builtin()`; test with `FakeHost` ([`docs/FLOWS.md`](docs/FLOWS.md#node-sdk-adding-a-kind)). |
| **A dashboard connector** | Implement the seam in `crates/iyakki-agent/src/dash/connector.rs`, register it in `registry.rs`; connectors are read-only by rule ([`docs/DASHBOARD.md`](docs/DASHBOARD.md#how-to-add-a-connector)). |
| **A store extension / driver pack** | `crates/iyakki-agent/src/store/` and `crates/iyakki-drivers` ([`docs/STORE.md`](docs/STORE.md)). |
| **A base-OS touchpoint** | A row in `BOUNDARY.md` (see above). |

## Commits and pull requests

* One logical change per commit; a short imperative-or-descriptive subject in sentence case, no trailing full stop, details in the
  body (the history reads like `Media queue, free-text categories, sample media; chat renders SVG and data images`).
* The commit includes code, tests, the FEATURES.md rows and the docs it changes.
* Run `cargo test --workspace` and `cargo deny check licenses sources` before you push.
* In a pull request say what changed, how you tested it and what you could **not** verify.

## Reporting issues

Use the project's issue tracker (OWNER: fill in the issues address) or the contact in [`SUPPORT.md`](SUPPORT.md). Include the
version (`iyakki --version`), how you run IYAKKI, what you did, what you expected, what happened, and for crashes the end of
`crash.log` ([`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md#what-to-send-with-a-report)). Never attach `users.json`, vault
files, keys or a full audit log. **Security problems do not go in public issues:** see [`SECURITY.md`](SECURITY.md).
