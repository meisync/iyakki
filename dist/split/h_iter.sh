#!/bin/bash
# h_iter.sh [cargo args]: one round: compile, fix field uses, add the missing methods to ServicesHandle
cd "$(dirname "$0")/../.."
dist/split/handle_errs.sh "$@" | tail -2
python3 dist/split/fix_handle_uses.py | head -1
names=$(grep -v '^F:' /tmp/h_missing.txt | paste -sd,)
[ -n "$names" ] && python3 dist/split/add_handle.py "$names"
