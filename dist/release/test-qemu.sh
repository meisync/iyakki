#!/bin/sh
# ./run.sh test-qemu [--list] [--only NAME[,NAME]] [BUILD_DIR]: boot an existing image in QEMU once and run the named session checks
# (dist/qemu/session-checks.list). Opt-in: it is NOT part of the test layers (test-core, test-apps) or of alpha-check; it can join them as a third
# layer later (docs/development/QEMU-TESTS.md). Needs a built image folder ($IYAKKI_QEMU_BUILD or the argument) and a built `iyakki` ($IYAKKI_CLIENT).
# Exit status: 0 all selected checks passed, 1 a check failed, 2 the guest could not be started.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec python3 "$here/../qemu/session-checks.py" "$@"
