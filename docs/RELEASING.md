# Cutting a release

How an owner cuts a release of IYAKKI, step by step. It is written for `0.1.0-alpha.1` and works for any later version. Nothing here
publishes anything by itself: every script prepares files and stops. Committing, tagging and pushing are the owner's decision
(`dist/release/commit-alpha.sh` does them only with `--yes-i-am-the-owner`).

## Before you start

* Tools: Rust (stable, with the `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` targets for images), `cargo-deny`, `cargo-about`,
  QEMU (`qemu-system-x86_64`, OVMF) for the smoke tests, Buildroot's host packages for images (`docs/LOCAL-TEST.md`).
* The tree builds offline with the lock files committed (`cargo build --workspace --locked`).
* **Temporary files.** The test suites and the QEMU scripts leak `iyakki-*` folders in the temporary directory. On a small or RAM-backed
  `/tmp` that fills the disk or the inodes (`df -i /tmp` at 100% means *tests leak temp dirs*). The release scripts and `./run.sh test` set
  `TMPDIR` to `target/tmp` (or `~/.cache/iyakki-tmp`) and remove `iyakki-*` entries older than a day when they finish. Doing it by hand:
  `export TMPDIR=$PWD/target/tmp; mkdir -p $TMPDIR`. `./run.sh alpha-check` warns when `/tmp` inode use is above 70%.

## 1. Choose the version

The version lives in one place: `[workspace.package] version` in `Cargo.toml`. Every crate inherits it (`version.workspace = true`);
the separate GUI workspace `crates/iyakki-gui/Cargo.toml` repeats it and has its own `Cargo.lock`. Change both, then
`cargo build --workspace` and `cd crates/iyakki-gui && cargo build` to update the two lock files. The binary prints it
(`iyakki --version`, the `IYAKKI <version> starting` boot line, the window title, SYSTEM overview "About"), and images carry
`/etc/iyakki-release` (version, build date, git describe, variant, kernel) written by `dist/buildroot/board/iyakki/post-build.sh`
from values `build.sh` exports. **Rebuild the images after a version change**; `./run.sh release` refuses images older than `Cargo.toml`.

Add the version to `CHANGELOG.md` (`## [x.y.z] - date`) and write `RELEASE-NOTES-x.y.z.md`. `iyakki-relcheck version` fails when the workspace,
the GUI workspace, either lock file, the changelog or the release notes disagree (it also runs as a test:
`cargo test -p iyakki-core --test release_version`).

## 2. Run the gate

    ./run.sh alpha-check              # everything; add --dry for the fast checks only, --no-qemu, --no-tests

It builds and tests the workspace (locked), builds the window binary, runs `cargo deny` for both workspaces, regenerates
`THIRD_PARTY_LICENSES.txt`, writes `SBOM.json`, runs the QEMU smoke tests that exist when images exist (it never builds images), checks docs
links, the FEATURES.md totals, version consistency and that no secrets are in the tree, and prints a PASS / FAIL / WARN / SKIP table. Exit code is non-zero
on any FAIL. Logs go to `target/alpha-check/`. It also runs a **macOS cross-check** (WARN-level, never fails the gate): `./run.sh check-macos` does a `cargo check` of the agent, shell,
core and the window (`crates/iyakki-gui`) for each installed `*-apple-darwin` Rust target, so Linux-only code without a `cfg` fence is caught
on a Linux machine. It proves the code compiles, not that it runs (nothing is linked or started); without the target
(`rustup target add aarch64-apple-darwin`) it prints a skip and the gate row is a WARN. Fix the FAILs; read the WARNs. Then fill in `ALPHA-GATE.md` (what the script cannot do: the Mac
mini, bare metal, the window in a real session).

## 3. Build the images

    ./run.sh build            # x86_64 libre ISO + install image, 30-40 minutes the first time
    ./run.sh build-arm        # ARM64 UEFI image (optional)
    ./run.sh build-web        # optional, big (hours, ~30 GB disk, 16+ GB RAM); add --h264 for H.264/AAC
    ./run.sh build-mac        # optional Mac variant (untested)
    ./run.sh build-macweb     # optional Mac variant with the full web engine (untested; big, hours)

**Do not rebuild everything for a release.** Run `dist/buildroot/changed.sh <variant>` first: `NOTHING` means the images are current, `IYAKKI-ONLY` means
`dist/buildroot/repack.sh <variant>` (minutes), and `macweb` can be assembled from the `web` and `mac` builds with `repack.sh macweb --base
target/buildroot-web --kernel-from target/buildroot-mac`. Rules and costs: [`BUILD-INCREMENTAL.md`](BUILD-INCREMENTAL.md).

Then run `./run.sh alpha-check --no-tests` again so the QEMU boot, persistence and install tests use the fresh images. The build also leaves
`legal-info/` (GPL/LGPL sources and licence texts) in `target/buildroot-*/`: keep it with the images.

## 4. Collect, sign, verify

    ./run.sh release                # uses the Cargo.toml version
    ./run.sh release --with-web     # also the optional web image as iyakki-web-*  (about 2 GB: say so on the page)
    ./run.sh release --with-macweb  # also the Mac + web image as iyakki-macweb-*  (same size warning)

This fills `target/release-<version>/` with `iyakki-x86_64.iso`, `iyakki-install-x86_64.img`, the ARM64 image if built,
the static private store servers `iyakki-store-server-x86_64` and `-aarch64` (about 1 MB each, built by `dist/store-server/build-static.sh`; the aarch64 one needs the
cross compiler of `./run.sh build-arm` or `$IYAKKI_AARCH64_CC`, and is skipped with a warning when it cannot be built; `--no-store-server` skips both; they are
summed and signed like everything else and are what the Private store wizard and the Dockerfile route use, docs/STORE.md section 13.1), the Linux x86_64
window tarball (`iyakki-desktop`, `run-window.sh`, LICENSE, NOTICE, THIRD_PARTY_LICENSES.txt, README-WINDOW.md; macOS and Windows are "not in
alpha"), the release notes, changelog, known issues, SECURITY.md and SBOM.json; writes `SHA256SUMS` + `.sig`, the signed `release.json` + `.sig`
and the download page `index.html`; then verifies its own output.

**The signing key.** The first run creates `~/.config/iyakki/release.key` (mode 0600) on this machine and prints only the **public** key.
Back the key file up offline now (encrypted); without it you cannot sign as the same identity again. Put the public key in the README, on the
download page and in the announcement, so people can compare it from a second place. The secret half is never printed.

People check a download with:

    sha256sum -c SHA256SUMS --ignore-missing
    iyakki release verify . --pubkey <public key>      # signature on SHA256SUMS and on release.json, then every file

`iyakki-usb` compares an image with `release.json` (or `<image>.sha256`) next to it before it writes a stick (`--manifest FILE` to name one).
It checks the hash, not the signature: verify the signature with `iyakki release verify` first.

`release.json` (signed) contains: schema, title, version, date, git commit and describe, kernel, the signing key, variants, minimum requirements,
and every file with its size, SHA-256 and variant.

## 5. Commit, tag, push (owner)

`COMMIT-PLAN.md` lists the commits. `dist/release/commit-alpha.sh --plan` shows them; `--yes-i-am-the-owner` makes them and the annotated tag
`v<version>` (without pushing); add `--push` to push the branch and the tag to `origin`. The push commands are printed. Rollback:
`git tag -d v<version>` and `git reset --soft <commit before the plan>` (nothing is lost; the files stay in the tree). If the tag was pushed,
`git push origin :refs/tags/v<version>` removes it remotely (tell people; do not move a published tag).

## 6. Publish

Upload the whole `target/release-<version>/` folder (the page `index.html` links the files), create the release entry from
`RELEASE-NOTES-<version>.md`, and fill in `SECURITY.md`'s mailbox (the placeholder is marked `OWNER: fill in`) **before** announcing.
Attach or keep `legal-info/` for the images you distribute.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `no space left on device` while testing, `df -h` is fine | `df -i /tmp` is full: tests leak temp dirs. `rm -rf /tmp/iyakki-*`, and set `TMPDIR` (see above). |
| `release: ... STALE` | The image is older than `Cargo.toml`: rebuild with `./run.sh build`, or `--allow-stale` knowing About will show `unknown`. |
| `version: ... Cargo.lock: ... is at 0.1.0` | A lock file was not updated: `cargo build --workspace` (and the same in `crates/iyakki-gui`). |
| `links: ... does not exist` | A markdown link or `#anchor` is broken; the output gives file:line. |
| `secrets: BLOCK` | The scan found a real-looking key or token or a secret file name; remove it (and rotate it if it was ever pushed). `--all` also lists the REVIEW items (test fixtures, detectors). |
| `cargo deny` fails | A dependency has a licence outside `deny.toml`. Do not add one without reading `docs/LICENSES.md`. |
