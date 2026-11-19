#!/usr/bin/env bash
# The steps of the "Hardware firmware" Setup item, by hand, for a developer (or to prepare a USB stick on another computer):
#   dist/scripts/firmware-notes/fetch.sh [--out DIR] [--dry-run] [--with-nvidia] [--snapshot FILE|download] (--names A,B,... | --log FILE)
# What it does (the same list logic as crates/iyakki-agent/src/fwextra.rs; crates/iyakki-agent/src/fwextra_cli_tests.rs compares them):
#   1. names: the firmware files a driver asked for: --names, or the lines "Direct firmware load for X failed" / "firmware: failed to load X"
#      of a kernel log file (`dmesg > log.txt` on the computer that lacks them; `iyakki dev dmesg` from another computer);
#   2. each name is looked up in the pinned manifest dist/firmware/manifest.txt, following links; of the versions of one .ucode family
#      (iwlwifi-cc-a0-77, -76, ...) only the first the release has is kept; nvidia/ files only with --with-nvidia;
#   3. --dry-run prints the list (file PATH / doc PATH) and stops: the licences are the lines "file SIZE SHA LICENCE-TEXT PATH" of the manifest
#      (read them, and the texts under LICENSES/, before you copy anything onto a machine);
#   4. otherwise every file and licence text is fetched from git.kernel.org at the pinned commit (or taken from the snapshot tarball when
#      --snapshot is given: a file, or the word download for the 1 GB release tarball), size and SHA-256 checked against the manifest, into
#      DIR/firmware-extra/ (default ./firmware-extra-out). One wrong byte and nothing stays.
# Then: copy DIR/firmware-extra to a USB stick and choose it on the device (Store -> Setup -> Hardware firmware -> From a USB stick), or push it
# over the cable: iyakki dev files push DIR/firmware-extra '~/firmware-extra'. The device checks every file again and installs only after
# the licence list is accepted ON THE DEVICE. Nothing here installs anything. Needs bash, awk, curl, sha256sum, tar.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="${FIRMWARE_MANIFEST:-$HERE/../../firmware/manifest.txt}"
OUT="firmware-extra-out"; DRY=0; NV=0; SNAP=""; NAMES=""; LOG=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    --with-nvidia) NV=1; shift ;;
    --snapshot) SNAP="$2"; shift 2 ;;
    --names) NAMES="$2"; shift 2 ;;
    --log) LOG="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST" >&2; exit 2; }
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
if [ -n "$LOG" ]; then
  sed -n -e 's/.*Direct firmware load for \([^ ]*\) failed.*/\1/p' -e 's/.*firmware: failed to load \([^ ]*\) (.*/\1/p' "$LOG" > "$WORK/names"
elif [ -n "$NAMES" ]; then
  printf '%s\n' "$NAMES" | tr ',' '\n' > "$WORK/names"
else
  echo "give --names A,B or --log FILE" >&2; exit 2
fi

# resolve the names against the manifest: lines "file PATH" and "doc PATH"
awk -v nv="$NV" '
  FILENAME == ARGV[1] {
    if ($1 == "file") { files[$5] = 1; tok[$5] = $4 }
    else if ($1 == "link") { links[$2] = $3 }
    else if ($1 == "doc") { docs[$4] = 1 }
    next
  }
  function norm(p,   n, a, i, out, k) {
    n = split(p, a, "/"); k = 0
    for (i = 1; i <= n; i++) {
      if (a[i] == "" || a[i] == ".") continue
      if (a[i] == "..") { if (k == 0) return ""; k--; continue }
      st[++k] = a[i]
    }
    out = ""
    for (i = 1; i <= k; i++) out = out (i > 1 ? "/" : "") st[i]
    return out
  }
  function safe(p) { return p != "" && p !~ /^\// && p !~ /(^|\/)\.\.?(\/|$)/ && p ~ /^[A-Za-z0-9_.,+\/-]+$/ }
  function resolve(name,   cur, i, dir, t) {
    if (!safe(name)) return ""
    cur = name
    for (i = 0; i < 5; i++) {
      if (cur in files) return cur
      if (!(cur in links)) return ""
      dir = (cur ~ /\//) ? cur : ""; sub(/\/[^\/]*$/, "", dir); if (cur !~ /\//) dir = ""
      t = links[cur]
      cur = norm(dir == "" ? t : dir "/" t)
    }
    return ""
  }
  function family(name,   s) {
    if (name !~ /\.ucode$/) return ""
    s = name; sub(/\.ucode$/, "", s)
    if (match(s, /[-_][0-9]+$/)) return substr(s, 1, RSTART) "*.ucode"
    return ""
  }
  {
    name = $0; gsub(/[\r"]/, "", name)
    if (name == "") next
    fam = family(name)
    p = resolve(name)
    if (p == "") next
    if (fam != "") { if (fam in seenfam) next; seenfam[fam] = 1 }
    if (!nv && p ~ /^nvidia\//) next
    if (!(p in picked)) { picked[p] = 1; order[++n] = p }
  }
  END {
    for (i = 1; i <= n; i++) {
      print "file " order[i]
      m = split(tok[order[i]], t, "+")
      for (j = 1; j <= m; j++) {
        d = "LICENSES/" t[j]
        if (d in docs) want[d] = 1
        if (t[j] ~ /^GPLv2/ && ("LICENSES/GPL-2.0" in docs)) want["LICENSES/GPL-2.0"] = 1
      }
    }
    if ("WHENCE" in docs) want["WHENCE"] = 1
    for (d in want) print "doc " d
  }
' "$MANIFEST" "$WORK/names" | LC_ALL=C sort -u > "$WORK/list"
[ -s "$WORK/list" ] || { echo "none of the names is a file of the pinned release: nothing to do" >&2; exit 1; }
if [ "$DRY" = 1 ]; then cat "$WORK/list"; exit 0; fi

TAG="$(awk '$1=="tag"{print $2}' "$MANIFEST")"; COMMIT="$(awk '$1=="commit"{print $2}' "$MANIFEST")"; BASE="$(awk '$1=="base"{print $2}' "$MANIFEST")"
SNAP_SHA="$(awk '$1=="snapshot"{print $3}' "$MANIFEST")"; SNAP_URL="$(awk '$1=="snapshot"{print $4}' "$MANIFEST")"
DEST="$OUT/firmware-extra"; mkdir -p "$DEST"
want_of() { awk -v p="$1" '($1=="file"&&$5==p)||($1=="doc"&&$4==p){ if ($1=="file") print $2, $3; else print $2, $3 }' "$MANIFEST"; }
if [ -n "$SNAP" ]; then
  if [ "$SNAP" = download ]; then SNAP="$WORK/snapshot.tar.gz"; echo "downloading the 1 GB release tarball ..."; curl -fS --retry 3 -o "$SNAP" "$SNAP_URL"; fi
  [ "$(sha256sum "$SNAP" | cut -d' ' -f1)" = "$SNAP_SHA" ] || { echo "the snapshot is not the pinned release (sha256 differs); nothing was written" >&2; exit 1; }
  awk '{print "linux-firmware-'"$TAG"'/" $2}' "$WORK/list" > "$WORK/members"
  tar -xzf "$SNAP" -C "$WORK" --no-same-owner -T "$WORK/members"
  while read -r _ p; do mkdir -p "$DEST/$(dirname "$p")"; cp "$WORK/linux-firmware-$TAG/$p" "$DEST/$p"; done < "$WORK/list"
else
  while read -r _ p; do
    mkdir -p "$DEST/$(dirname "$p")"
    curl -fS --retry 3 --proto '=https' --proto-redir '=https' -o "$DEST/$p" "$BASE/$p?id=$COMMIT"
  done < "$WORK/list"
fi
# every file against size and sha256 of the manifest
bad=0
while read -r _ p; do
  read -r size sha < <(want_of "$p")
  got_size="$(stat -c %s "$DEST/$p")"; got_sha="$(sha256sum "$DEST/$p" | cut -d' ' -f1)"
  if [ "$got_size" != "$size" ] || [ "$got_sha" != "$sha" ]; then echo "MISMATCH $p" >&2; rm -f "$DEST/$p"; bad=1; fi
done < "$WORK/list"
[ "$bad" = 0 ] || { echo "some files did not match the pinned checksums and were deleted" >&2; exit 1; }
printf '{ "tag": "%s", "commit": "%s", "note": "made by fetch.sh; the device checks every file again" }\n' "$TAG" "$COMMIT" > "$DEST/firmware-extra.json"
echo "ok: $(grep -c '^file' "$WORK/list") file(s) and $(grep -c '^doc' "$WORK/list") licence text(s) in $DEST"
