#!/bin/sh
# Put the licence texts of the firmware an image carries next to it:   firmware-licences.sh TARGET_DIR SOURCE_DIR...
# The linux-firmware licences (AMD, Intel, NVIDIA, Broadcom, Realtek, ...) allow redistribution in binary form only if the licence text
# travels with the files (NVIDIA: "Customer provides a copy of this license with the SOFTWARE"; AMD and Intel: reproduce the notice in the
# documentation). Buildroot keeps them in legal-info/ only, so this copies every LICENCE.* / LICENSE.* file and WHENCE (which names the
# licence of each file) from the first SOURCE_DIR that has them into TARGET_DIR/usr/share/doc/linux-firmware/. Nothing is copied when the
# image has no firmware (the libre image). Also GPL-2/GPL-3 (WHENCE points at them), qca and qcom NOTICE.txt, and wireless-regdb's LICENSE
# when it sits next to the linux-firmware build dir. Used by post-build.sh. LICENSING.md item 7.
set -eu
TARGET="$1"; shift
[ -d "$TARGET/lib/firmware" ] || exit 0
# an image whose only "firmware" is the wireless regulatory database (regulatory.db, from wireless-regdb, ISC licence: the web variant)
# carries nothing from linux-firmware: there is no linux-firmware source tree and no licence set to copy
[ -n "$(find "$TARGET/lib/firmware" -type f ! -name regulatory.db ! -name regulatory.db.p7s 2>/dev/null | head -n 1)" ] || exit 0
for src in "$@"; do
  [ -f "$src/WHENCE" ] || continue
  DEST="$TARGET/usr/share/doc/linux-firmware"
  mkdir -p "$DEST"
  for f in "$src"/LICENCE.* "$src"/LICENSE "$src"/LICENSE.* "$src"/WHENCE "$src"/GPL-2 "$src"/GPL-3; do
    [ -f "$f" ] && install -m 0644 "$f" "$DEST/$(basename "$f")"
  done
  # notices that WHENCE names next to LICENSE.qcom / LICENSE.QualcommAtheros_ath10k (qca/NOTICE.txt, qcom/NOTICE.txt)
  for n in qca qcom; do
    [ -f "$src/$n/NOTICE.txt" ] && install -m 0644 "$src/$n/NOTICE.txt" "$DEST/NOTICE.$n.txt"
  done
  # regulatory.db comes from the wireless-regdb package (ISC licence: the notice must stay with copies)
  for r in "$(dirname "$src")"/wireless-regdb-*/LICENSE; do
    [ -f "$r" ] && install -m 0644 "$r" "$DEST/LICENSE.wireless-regdb"
  done
  echo "firmware-licences: $(ls "$DEST" | wc -l) licence texts from $src"
  exit 0
done
echo "firmware-licences: the image carries firmware but no linux-firmware source tree was found; the licence texts are NOT in the image" >&2
exit 1
