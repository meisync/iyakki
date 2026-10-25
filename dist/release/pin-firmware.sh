#!/usr/bin/env bash
# Writes the pinned table of the "restricted firmware" packs:
#   dist/release/pin-firmware.sh [COMMIT]          (default: the commit below; needs curl, awk, sha256sum and a network)
# Output: dist/buildroot/board/iyakki/restricted-firmware.table, the single source of truth that
#   * crates/iyakki-agent/src/fwpack.rs reads (include_str!) for the Setup tiles' downloads (size + sha256 of every file), and
#   * dist/buildroot/board/iyakki/restricted-firmware.sh reads for IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 builds.
# What is in the packs and why: docs/RESTRICTED-FIRMWARE.md and docs/FIRMWARE-LICENCES.md (policy: when in doubt it is not shipped).
# The file lists come from the real WHENCE of that linux-firmware commit (never typed by hand); every file and licence text is downloaded
# from the upstream repository at that commit and its size and SHA-256 are written down. Not part of any image. Run again with another
# commit to move the pin (then review the diff of the table and re-read the licence texts).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist/buildroot/board/iyakki/restricted-firmware.table"
# upstream tag 20251011, resolved by git.kernel.org to this commit (2026-10-23)
COMMIT="${1:-1a935bfb330d4a46786eaa903580db9e757b84fa}"
BASE="https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain"
case "$COMMIT" in *[!0-9a-f]*|"") echo "pin-firmware: the commit must be 40 hex digits" >&2; exit 2 ;; esac
[ "${#COMMIT}" = 40 ] || { echo "pin-firmware: the commit must be 40 hex digits" >&2; exit 2; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
get() { curl -fsS --retry 3 --max-time 120 -o "$2" "$BASE/$1?id=$COMMIT"; }
get WHENCE "$WORK/WHENCE"

# one WHENCE block = "Driver:" line, "File:" lines (before or after), one "Licence:" line. pack<TAB>path for the doubtful ones.
awk '
  function flush(   i) {
    if (drv == "") return
    for (i = 1; i <= n; i++) {
      p = f[i]
      if (lic ~ /LICENSE\.qcom/ && p ~ /^qca\//)               print "qualcomm-bt\t" p
      else if (lic ~ /LICENSE\.r8169/ && p ~ /^rtl_nic\//)     print "realtek-net\t" p
      else if (lic ~ /LICENCE\.(tigon|bnx2|bnx2x)( |$|\.)/)    print "broadcom-net\t" p
      else if (drv ~ /^Driver: carl9170 /)                     print "atheros-carl9170\t" p
    }
  }
  /^Driver: / { flush(); drv = $0; n = 0; lic = ""; next }
  /^(File|RawFile): / { f[++n] = $2; next }
  /^Licen[cs]e: / { lic = $0; next }
  END { flush() }
' "$WORK/WHENCE" | LC_ALL=C sort -u > "$WORK/list"
[ "$(wc -l < "$WORK/list")" -ge 50 ] || { echo "pin-firmware: WHENCE gave too few files; the format may have changed" >&2; exit 1; }

# the licence texts that travel with each pack
cat > "$WORK/docs" <<'EOF'
qualcomm-bt	LICENSE.qcom
qualcomm-bt	qca/NOTICE.txt
realtek-net	LICENSE.r8169
broadcom-net	LICENCE.tigon
broadcom-net	LICENCE.bnx2
broadcom-net	LICENCE.bnx2x
atheros-carl9170	GPL-2
EOF

{
  echo "# Pinned restricted firmware (made by dist/release/pin-firmware.sh; do not edit by hand)."
  echo "# Policy: these files are NOT in the images IYAKKI publishes; the person downloads them at a click or builds an image for themselves."
  echo "# Lines:  commit <sha> | base <url> | pack <id> | doc <size> <sha256> <path> (licence text) | file <size> <sha256> <path>"
  echo "commit $COMMIT"
  echo "base $BASE"
  for pack in qualcomm-bt realtek-net broadcom-net atheros-carl9170; do
    echo "pack $pack"
    awk -F'\t' -v p="$pack" '$1 == p { print $2 }' "$WORK/docs" | while read -r path; do
      mkdir -p "$WORK/dl/$(dirname "$path")"; get "$path" "$WORK/dl/$path"
      echo "doc $(wc -c < "$WORK/dl/$path" | tr -d ' ') $(sha256sum "$WORK/dl/$path" | cut -d' ' -f1) $path"
    done
    awk -F'\t' -v p="$pack" '$1 == p { print $2 }' "$WORK/list" | while read -r path; do
      mkdir -p "$WORK/dl/$(dirname "$path")"; get "$path" "$WORK/dl/$path"
      echo "file $(wc -c < "$WORK/dl/$path" | tr -d ' ') $(sha256sum "$WORK/dl/$path" | cut -d' ' -f1) $path"
    done
  done
} > "$WORK/table"
mv "$WORK/table" "$OUT"
echo "pin-firmware: wrote $OUT ($(grep -c '^file ' "$OUT") files, $(grep -c '^doc ' "$OUT") licence texts) at commit $COMMIT"
