#!/bin/sh
# Keep the data volume of a release image as an upgrade fixture (F-UPG-07).
#
#   dist/release/capture-upgrade-fixture.sh NAME DATA_DIR --synthetic [--dest DIR]
#
# NAME      the release the volume came from, for example `beta1`; the fixture goes to tests/fixtures/upgrade/NAME
# DATA_DIR  the mounted data volume's IYAKKI folder (what `/var/lib/iyakki` is on the device), made by running THAT release's image with
#           made-up accounts and made-up data only
# --synthetic  you confirm that the volume holds only made-up data; there is no other way to run this
#
# Only the stores the upgrade tests know are copied (the list below). Anything that looks like a home folder path of a person or an IP
# address stops the copy and names the file: edit the data on the test image, not the fixture. The agent crate's `upgrade_tests`
# (`every_previous_release_fixture_passes_the_same_checks`) picks every folder under tests/fixtures/upgrade except `current` up by itself.
set -eu
NAME="${1:-}"; DATA="${2:-}"; SYN=0; DEST=""
[ -n "$NAME" ] && [ -n "$DATA" ] || { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
shift 2
while [ $# -gt 0 ]; do
  case "$1" in
    --synthetic) SYN=1 ;;
    --dest) shift; DEST="$1" ;;
    *) echo "capture-upgrade-fixture: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
[ "$SYN" = 1 ] || { echo "capture-upgrade-fixture: refusing without --synthetic (the volume must hold made-up data only)" >&2; exit 2; }
case "$NAME" in *[!a-z0-9._-]*|""|current) echo "capture-upgrade-fixture: NAME must be letters, digits, . _ - and not 'current'" >&2; exit 2 ;; esac
[ -d "$DATA" ] || { echo "capture-upgrade-fixture: $DATA is not a folder" >&2; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -n "$DEST" ] || DEST="$HERE/../../tests/fixtures/upgrade"
OUT="$DEST/$NAME"
[ ! -e "$OUT" ] || { echo "capture-upgrade-fixture: $OUT exists already; remove it first on purpose" >&2; exit 2; }
LIST="$(cd "$DATA" && find settings.json users.json audit.log devices.json approval-pins-*.json browser-*.json radio-*.json \
  dash inbox k3s-dev devhome vault home/*/Flows -type f 2>/dev/null | sort || true)"
[ -n "$LIST" ] || { echo "capture-upgrade-fixture: $DATA holds none of the known stores" >&2; exit 1; }
BAD=""
for f in $LIST; do
  case "$f" in vault/*) continue ;; esac   # sealed binary files: nothing readable in them
  if grep -qE '/home/[a-z][a-z0-9_-]*/|/Users/[A-Za-z]|([0-9]{1,3}\.){3}[0-9]{1,3}' "$DATA/$f" 2>/dev/null; then BAD="$BAD $f"; fi
done
[ -z "$BAD" ] || { echo "capture-upgrade-fixture: these files name a home folder or an address; clean the test image's data and run again:$BAD" >&2; exit 1; }
for f in $LIST; do
  mkdir -p "$OUT/$(dirname "$f")"
  cp "$DATA/$f" "$OUT/$f"
done
echo "capture-upgrade-fixture: $(echo "$LIST" | wc -l) files into $OUT"
