#!/usr/bin/env bash
# Builds the public tree: the tracked files of HEAD minus dist/release/public-exclude.txt, checked, as ONE commit by the project identity.
# Never pushes. The private history stays where it is.
#   dist/release/export-public.sh [OUT_DIR]       default OUT_DIR: ${IYAKKI_WORK:-$HOME/iyakki-work}/public-export
# Settings: IYAKKI_PUBLIC_NAME, IYAKKI_PUBLIC_EMAIL (commit identity), IYAKKI_PUBLIC_MESSAGE.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
OUT="${1:-${IYAKKI_WORK:-$HOME/iyakki-work}/public-export}"
NAME="${IYAKKI_PUBLIC_NAME:-IYAKKI Project}"
EMAIL="${IYAKKI_PUBLIC_EMAIL:-noreply@users.noreply.github.com}"
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
MSG="${IYAKKI_PUBLIC_MESSAGE:-IYAKKI $VER}"
fail() { echo "export-public: REFUSED: $*" >&2; exit 1; }

[ -z "$(git status --porcelain)" ] || fail "the working tree has uncommitted changes; commit first so the export is of HEAD"
case "$OUT" in "$PWD"*) fail "OUT_DIR must be outside the repository";; esac
[ ! -e "$OUT" ] || [ -z "$(ls -A "$OUT" 2>/dev/null)" ] || fail "$OUT exists and is not empty (remove it yourself)"
mkdir -p "$OUT"

# 1. tracked files of HEAD (submodules are separate repositories and are not copied)
git archive HEAD | tar -x -C "$OUT"
# 2. exclusions
while IFS= read -r pat; do
  case "$pat" in ''|'#'*) continue;; esac
  ( cd "$OUT" && shopt -s nullglob globstar && for f in $pat; do rm -rf -- "$f"; done )
done < dist/release/public-exclude.txt
echo "export-public: $(find "$OUT" -type f | wc -l) files after exclusions"

# 3. checks on the result
bad=0
# 3a. personal data and private infrastructure patterns
if grep -rInE '/home/[a-z][a-z0-9_-]+/|\b192\.168\.(0|1)\.[0-9]+\b.*(owner|my )|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|sk-ant-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[0-9A-Z]{16}' "$OUT" --exclude-dir=.git --exclude='*.png' --exclude='*.webp' --exclude='*.wasm' --exclude='*.mp3' --exclude='*.webm' --exclude='*.mp4' --exclude='*.ttf' 2>/dev/null | grep -v 'iyakki-secrets\|relcheck\|/tests/\|fixtures\|example' | head -20 | tee /tmp/export-public-hits.$$ | grep -q .; then
  echo "export-public: pattern hits above (home paths, key blocks, tokens); review them" >&2; bad=1
fi
rm -f /tmp/export-public-hits.$$
# 3b. relative markdown links that now point at nothing
python3 - "$OUT" <<'P' || bad=1
import os, re, sys
root = sys.argv[1]; missing = []
for d, _, fs in os.walk(root):
    for f in fs:
        if not f.endswith(".md"): continue
        p = os.path.join(d, f)
        for m in re.finditer(r"\]\((?!https?:|mailto:|#)([^)#\s]+)", open(p, errors="ignore").read()):
            t = os.path.normpath(os.path.join(d, m.group(1)))
            if not os.path.exists(t): missing.append((os.path.relpath(p, root), m.group(1)))
for p, t in missing[:60]: print(f"  dangling link: {p} -> {t}", file=sys.stderr)
if missing: print(f"export-public: {len(missing)} dangling link(s)", file=sys.stderr); sys.exit(1)
P
# 3c. the identity and the old logo
[ ! -e "$OUT/dist/brand/owner-logo.png" ] || { echo "export-public: owner-logo.png is in the tree" >&2; bad=1; }
[ "$bad" = 0 ] || fail "checks failed (the export is left in $OUT for you to look at)"

# 4. one commit
cd "$OUT"
git init -q -b main
git -c user.name="$NAME" -c user.email="$EMAIL" add -A
git -c user.name="$NAME" -c user.email="$EMAIL" commit -q -m "$MSG"
echo "export-public: OK. One commit $(git rev-parse --short HEAD) by $NAME in $OUT. Nothing was pushed."
