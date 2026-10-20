#!/usr/bin/env bash
# The commit plan for 0.1.0-alpha.1 (COMMIT-PLAN.md describes it). It REFUSES to do anything without the explicit flag below.
#   dist/release/commit-alpha.sh --plan                     print the commits and the files in each (changes nothing)
#   dist/release/commit-alpha.sh --yes-i-am-the-owner       make the commits and the annotated tag v0.1.0-alpha.1 (no push)
#   dist/release/commit-alpha.sh --yes-i-am-the-owner --push  ... and push the branch and the tag to origin
# Nothing is committed unless the working tree matches the plan: the script stops before the first commit if a file is bigger
# than 10 MB, if something is already staged, or if the version is not 0.1.0-alpha.1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; cd "$ROOT"
VERSION="0.1.0-alpha.1"; TAG="v$VERSION"
TRAILER="Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
YES=0; PUSH=0; PLAN=0
for a in "$@"; do case "$a" in
  --yes-i-am-the-owner) YES=1 ;; --push) PUSH=1 ;; --plan) PLAN=1 ;;
  -h|--help) sed -n 2,9p "$0"; exit 0 ;; *) echo "unknown option $a" >&2; exit 2 ;; esac; done

# Each commit: subject | pathspecs (space separated). Order matters: a file goes into the first commit whose pathspec matches.
# The body of each commit is in body() below.
SUBJECTS=(
  "test(matrix): allow digits in the area part of a feature id and B- ids for workspace boards"
  "feat(agent): assistant, browser, IDE, K3s and deploy, platform services, store, flows and data tools"
  "feat(data,flow,drivers,zram): new crates for tables, flows, driver packs and compressed swap"
  "feat(shell): tiles, IDE, dashboard, flows, store and settings screens"
  "feat(web): JavaScript engine, web platform and full web engine bridge"
  "feat(core): supervisor, boot, USB writer, storage, hardware and desktop window"
  "build(image): Buildroot images, kernel fragments, QEMU tests, deploy manifests, skills and samples"
  "docs: guides, design notes and feature matrix"
  "release: 0.1.0-alpha.1 version, release scripts and checks, changelog, notes, security policy, SBOM"
  "chore: remaining files"
)
PATHS=(
  "crates/iyakki-core/tests/feature_matrix.rs"
  "crates/iyakki-agent"
  "crates/iyakki-data crates/iyakki-flow crates/iyakki-drivers crates/iyakki-zram"
  "crates/iyakki-shell"
  "crates/iyakki-js crates/iyakki-web crates/iyakki-webview dist/webview dist/js"
  "crates/iyakki-core crates/iyakki-usb crates/iyakki-storage crates/iyakki-hw crates/iyakki-hwinfo crates/iyakki-sec crates/iyakki-proxy crates/iyakki-ui crates/iyakki-gui crates/iyakki-vfs crates/iyakki-wasm crates/iyakki-ledger crates/iyakki-mem crates/iyakki-swarm crates/iyakki-remote crates/iyakki-netscan crates/iyakki-devctl crates/iyakki-edit :(exclude)crates/iyakki-gui/Cargo.lock :(exclude)crates/iyakki-core/Cargo.toml"
  "dist/buildroot dist/Dockerfile dist/k3s-manifest.yaml dist/k3s-web.yaml dist/systemd dist/test-bases.sh dist/samples dist/skills dist/templates tests :(exclude)dist/release"
  "docs README.md CONTRIBUTING.md CODE_OF_CONDUCT.md BOUNDARY.md LICENSING.md FEATURES.md Agent*Architecture*Specification.md"
  "crates/iyakki-relcheck dist/release run.sh Cargo.toml Cargo.lock crates/iyakki-gui/Cargo.lock crates/iyakki-core/Cargo.toml CHANGELOG.md RELEASE-NOTES-$VERSION.md KNOWN-ISSUES.md SECURITY.md SUPPORT.md ALPHA-GATE.md COMMIT-PLAN.md SBOM.json THIRD_PARTY_LICENSES.txt NOTICE LICENSE about.toml about.hbs deny.toml .gitignore .iyakki/project.json"
  "."
)

body() { # $1 = index
  case "$1" in
  0) cat <<'T'
The feature-matrix test (crates/iyakki-core/tests/feature_matrix.rs) was loosened in two ways, disclosed here on purpose:

* the id check now accepts digits in the area part of a feature id (F-K8S-01 was rejected before), and
* ids that start with B- (bugs on a workspace's feature board) are accepted next to F- ids.

Everything else the test checks (cited tests exist, no `done` without evidence, progress agrees with status, totals) is unchanged.
T
  ;;
  1) echo "Assistant and agent mode, the browser agent, AI IDE services, K3s and deploy tools, platform services (Gitea, private store), the app store, flows and data/dashboard tools, with their tests." ;;
  2) echo "iyakki-data (tables), iyakki-flow (flow model and runner), iyakki-drivers (driver packs) and iyakki-zram (compressed swap)." ;;
  3) echo "The shell's tiles and screens: IDE, DASH, DATA, FLOWS, STORE, board, chat additions and the settings pages, with their tests." ;;
  4) echo "The QuickJS-based script engine and web platform, the reader browser's HTML, and the WPE bridge for the optional web image." ;;
  5) echo "Supervisor and boot changes, iyakki --version and the boot banner, the USB writer's release.json check, storage and hardware crates, the desktop window title and version." ;;
  6) echo "Buildroot configuration, kernel fragments, the /etc/iyakki-release file, QEMU test scripts, deploy manifests, skills and sample media." ;;
  7) echo "User, admin and design documentation, the architecture notes, BOUNDARY and LICENSING, and FEATURES.md with its rows and totals." ;;
  8) cat <<T
Version 0.1.0-alpha.1 for the workspace, the separate GUI workspace and both lock files. Release tooling: ./run.sh release and
./run.sh alpha-check (dist/release), iyakki-relcheck (link checker, secret scan, version and totals checks, SBOM), signed release.json,
CHANGELOG, release notes, KNOWN-ISSUES, SECURITY, SUPPORT, ALPHA-GATE, COMMIT-PLAN, SBOM.json and the regenerated third-party licences.
T
  ;;
  *) echo "Files that no earlier commit claimed." ;;
  esac
}

files_for() { # $1 = index; prints the files that this commit would contain (not yet claimed by an earlier one)
  local i="$1" f
  # shellcheck disable=SC2086
  git status --porcelain --untracked-files=all -- ${PATHS[$i]} 2>/dev/null | sed -e 's/^...//' -e 's/.* -> //' -e 's/^"\(.*\)"$/\1/' | while read -r f; do
    grep -qxF "$f" "$CLAIMED" 2>/dev/null || echo "$f"
  done | sort -u
}

CLAIMED="$(mktemp "${TMPDIR:-/tmp}/iyakki-claimed.XXXXXX")"; trap 'rm -f "$CLAIMED"' EXIT
plan() {
  : > "$CLAIMED"
  for i in "${!SUBJECTS[@]}"; do
    list="$(files_for "$i")"
    n="$(printf '%s' "$list" | grep -c . || true)"
    echo; echo "[$((i+1))] ${SUBJECTS[$i]}  ($n files)"
    [ "$n" -gt 0 ] && printf '%s\n' "$list" | sed 's/^/      /' | head -${PLAN_LINES:-12}
    [ "$n" -gt "${PLAN_LINES:-12}" ] && echo "      ... and $((n - ${PLAN_LINES:-12})) more"
    printf '%s\n' "$list" >> "$CLAIMED"
  done
  echo; echo "[tag]  git tag -a $TAG -m \"IYAKKI $VERSION (alpha)\""
}

if [ "$PLAN" = 1 ] || [ "$YES" = 0 ]; then
  [ "$YES" = 0 ] && [ "$PLAN" = 0 ] && echo "This script makes commits and a tag. It will not run without --yes-i-am-the-owner (use --plan to look)." >&2
  plan
  [ "$PLAN" = 1 ] && exit 0
  exit 2
fi

# ---- checks, then the real thing ----
echo "About to make ${#SUBJECTS[@]} commits and the tag $TAG on branch $(git rev-parse --abbrev-ref HEAD), starting from $(git rev-parse --short HEAD)."
plan
V="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
[ "$V" = "$VERSION" ] || { echo "Cargo.toml says $V, not $VERSION: stop." >&2; exit 1; }
[ -z "$(git diff --cached --name-only)" ] || { echo "Something is already staged: unstage it first (git reset) so the plan stays exact." >&2; exit 1; }
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && { echo "The tag $TAG already exists." >&2; exit 1; }
big="$(git status --porcelain --untracked-files=all | sed 's/^...//' | while read -r f; do [ -f "$f" ] && [ "$(stat -c %s "$f")" -gt 10485760 ] && echo "$f"; done || true)"
[ -z "$big" ] || { echo "Files over 10 MB would be committed:"; echo "$big"; exit 1; }
echo; echo "Committing ..."
: > "$CLAIMED"
MSG="$(mktemp "${TMPDIR:-/tmp}/iyakki-msg.XXXXXX")"; trap 'rm -f "$CLAIMED" "$MSG"' EXIT
for i in "${!SUBJECTS[@]}"; do
  list="$(files_for "$i")"
  [ -n "$list" ] || { echo "[$((i+1))] nothing to commit: ${SUBJECTS[$i]}"; continue; }
  printf '%s\n' "$list" | while read -r f; do git add -A -- "$f"; done
  printf '%s\n' "$list" >> "$CLAIMED"
  { echo "${SUBJECTS[$i]}"; echo; body "$i"; echo; echo "$TRAILER"; } > "$MSG"
  git commit -q -F "$MSG"
  echo "[$((i+1))] $(git log -1 --format='%h %s')"
done
git tag -a "$TAG" -m "IYAKKI $VERSION (alpha)

First public alpha. See CHANGELOG.md, RELEASE-NOTES-$VERSION.md and KNOWN-ISSUES.md.

$TRAILER"
echo "Tagged $TAG."
if [ "$PUSH" = 1 ]; then
  git push origin "$(git rev-parse --abbrev-ref HEAD)"
  git push origin "$TAG"
else
  echo "Not pushed. When you are ready:"
  echo "  git push origin $(git rev-parse --abbrev-ref HEAD) && git push origin $TAG"
fi
