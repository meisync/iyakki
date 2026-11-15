#!/usr/bin/env bash
# ./run.sh compat-check   The compatibility gates of docs/development/CORE-COMPATIBILITY-DESIGN.md (steps 1 to 3 of section 10):
#   1. every versioned surface in the code has a row in docs/development/compat-surfaces.txt (and no row is stale);
#   2. the public surface of crates/iyakki-core-api matches docs/development/core-surface.txt: an item that disappeared or changed shape
#      fails unless the file records it as a `removed` line with a note, and a line of the committed file may not vanish from the working
#      file without becoming a `removed` line;
#   3. the golden fixtures under tests/compat/ are read by the current code (backward) and unknown parts survive (forward).
# Prints what to do when a gate fails. Exit 0 when all pass.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
fail=0
SURF=docs/development/core-surface.txt
export LC_ALL=C   # sort and comm must agree

# the committed file: no item line may vanish from the working file (it must become a removed line instead)
if git rev-parse --git-dir >/dev/null 2>&1 && git cat-file -e "HEAD:$SURF" 2>/dev/null; then
  lost="$(comm -23 <(git show "HEAD:$SURF" | grep -v '^#' | grep -v '^removed	' | grep -v '^$' | sort -u) <(grep -v '^#' "$SURF" | grep -v '^removed	' | grep -v '^$' | sort -u) | head -n 50)"
  if [ -n "$lost" ]; then
    gone_noted="$(grep '^removed	' "$SURF" | cut -f2 | sort -u)"
    unnoted="$(comm -23 <(printf '%s\n' "$lost" | sort -u) <(printf '%s\n' "$gone_noted"))"
    if [ -n "$unnoted" ]; then
      echo "compat-check: FAIL core-surface: these lines of the committed $SURF are gone without a removed<TAB>item<TAB>note line (additions only, charter section 4):"
      printf '%s\n' "$unnoted" | head -n 20 | sed 's/^/  - /'
      fail=1
    fi
  fi
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
run() { # name, command...
  local name="$1"; shift
  if out="$("$@" 2>&1)"; then echo "compat-check: PASS $name"; else echo "compat-check: FAIL $name"; printf '%s\n' "$out" | tail -n 40; fail=1; fi
}
run "versioned surfaces are listed (docs/development/compat-surfaces.txt)" cargo test -q -p iyakki-core-api --test compat_surfaces
run "core API surface only grows (docs/development/core-surface.txt)" cargo test -q -p iyakki-core-api --test core_surface
run "deprecated names have an alias test and are removed on time (docs/development/deprecations.txt)" cargo test -q -p iyakki-core-api --test compat_deprecations
run "golden fixtures: wire protocols and catalogue (iyakki-core-api, tests/compat)" cargo test -q -p iyakki-core-api --test compat_alpha2
run "golden fixtures: extension snapshot and helper frames (iyakki-webview)" cargo test -q -p iyakki-webview --test compat_alpha2
run "golden fixtures: data stores, approvals, access log, views (iyakki-agent)" cargo test -q -p iyakki-agent --lib compat_tests
run "upgrade and rollback harness lint (iyakki-core-api, qemu-rollback-test.sh)" cargo test -q -p iyakki-core-api --test compat_step6
run "golden fixtures: approval pins (iyakki-shell)" cargo test -q -p iyakki-shell --lib compat_tests
run "test hygiene: no test reads the real HOME (iyakki-agent tests/test_hygiene)" cargo test -q -p iyakki-agent --test test_hygiene
if [ -n "${IYAKKI_HOME_HYGIENE:-}" ]; then run "test hygiene: cargo test leaves the real home folder alone" dist/release/home-hygiene.sh; fi
if [ "$fail" != 0 ]; then
  cat <<'TXT'
compat-check: FAILED. What to do:
  * NEW surface or item: add the row / append the line (IYAKKI_CORE_SURFACE_WRITE=1 cargo test -p iyakki-core-api --test core_surface), add a fixture.
  * REMOVED or CHANGED: a new version may add, never change the meaning of and never silently remove (charter section 4). Prefer an alias or a
    new name; if it must go, IYAKKI_CORE_SURFACE_NOTE="why, and which version or alias covers it" with the WRITE command above, and say so in
    the release notes.
  * A fixture test fails: the code stopped reading something a release wrote. Fix the code, never the fixture.
TXT
  exit 1
fi
echo "compat-check: all gates pass"
