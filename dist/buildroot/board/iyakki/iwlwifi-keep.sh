#!/bin/sh
# The Intel Wi-Fi firmware files worth keeping for the kernel in MODULES_DIR, as keep-list patterns on stdout (prune-firmware.sh format):
#   iwlwifi-keep.sh MODULES_DIR FIRMWARE_DIR
# linux-firmware holds many versions of each chip's file (iwlwifi-cc-a0-77.ucode, -89, -100 ...; 265 MB for the whole folder) and the
# kernel asks for the newest API version IT knows (MODULE_FIRMWARE of iwlwifi.ko: `iwlwifi-<chip>-<max>.ucode`), then tries older ones.
# So for each declared name: the newest file of that chip not newer than the declared maximum, and the one below it, plus the chip's
# .pnvm files. Names come from `modinfo -F firmware` on every iwlwifi*.ko under MODULES_DIR (or from the file IWLWIFI_DECLARED, one
# name per line: used by the tests and by hosts without modinfo). Without any declared name everything is kept (the safe answer).
set -eu
MODS="$1"; FW="$2"
DECL="$(mktemp)"; trap 'rm -f "$DECL"' EXIT
if [ -n "${IWLWIFI_DECLARED:-}" ] && [ -f "$IWLWIFI_DECLARED" ]; then
  cat "$IWLWIFI_DECLARED" > "$DECL"
elif command -v modinfo >/dev/null 2>&1; then
  find "$MODS" -name 'iwlwifi*.ko' -o -name 'iwlwifi*.ko.*' 2>/dev/null | while IFS= read -r m; do modinfo -F firmware "$m" 2>/dev/null || true; done > "$DECL"
fi
grep -q 'iwlwifi-' "$DECL" 2>/dev/null || { echo 'iwlwifi-*'; echo 'intel/iwlwifi/*'; exit 0; }
(cd "$FW" && ls -1 | grep -E '^iwlwifi-.*\.(ucode|pnvm)$' || true) | LC_ALL=C sort > "$DECL.have"
awk -v declfile="$DECL" '
  BEGIN { while ((getline l < declfile) > 0) { if (l ~ /^iwlwifi-.*\.(ucode|pnvm)$/) want[l] = 1 } }
  { have[$0] = 1; names[++n] = $0 }
  END {
    for (w in want) {
      if (w ~ /\.pnvm$/) { if (w in have) keep[w] = 1; continue }
      base = w; sub(/-[0-9]+\.ucode$/, "", base); max = w; sub(/^.*-/, "", max); sub(/\.ucode$/, "", max); max += 0
      # versions of this chip that exist and are not newer than the declared maximum
      c = 0
      for (i = 1; i <= n; i++) {
        f = names[i]
        if (index(f, base "-") != 1 || f !~ /-[0-9]+\.ucode$/) continue
        rest = substr(f, length(base) + 2); if (rest !~ /^[0-9]+\.ucode$/) continue
        v = rest; sub(/\.ucode$/, "", v); v += 0
        if (v <= max) { c++; vs[c] = v; fs[c] = f }
      }
      # the two newest
      for (k = 0; k < 2; k++) { best = -1; bi = 0; for (j = 1; j <= c; j++) if (vs[j] > best) { best = vs[j]; bi = j } if (bi) { keep[fs[bi]] = 1; vs[bi] = -2 } }
    }
    for (f in keep) print f
  }' "$DECL.have" | LC_ALL=C sort
rm -f "$DECL.have"
