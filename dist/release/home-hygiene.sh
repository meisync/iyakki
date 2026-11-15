#!/usr/bin/env bash
# ./dist/release/home-hygiene.sh   Fails if `cargo test` of iyakki-agent and iyakki-flow changes the names in the real $HOME,
# $HOME/Flows or the other well-known folders (tests must use temp folders). Slow (builds and runs two test suites): the compat-check
# runs it only with IYAKKI_HOME_HYGIENE=1. The static part (no test reads the real HOME) always runs in `cargo test -p iyakki-agent --test test_hygiene`.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
IYAKKI_HOME_HYGIENE=1 exec cargo test -q -p iyakki-agent --test test_hygiene
