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
# 3b. relative markdown links: a link to a file that was left out on purpose becomes plain text; any other link that points at nothing is an error
python3 - "$OUT" "$PWD" <<'P' || bad=1
import os, re, sys
from urllib.parse import unquote
root, src = sys.argv[1], sys.argv[2]
link = re.compile(r"\[([^\]]*)\]\((<[^>]+>|[^)\s]+)\)")
fence = re.compile(r"```.*?```|`[^`\n]*`", re.S)
unlinked, broken = 0, []
for d, _, fs in os.walk(root):
    if os.sep + ".git" in d: continue
    for f in fs:
        if not f.endswith(".md"): continue
        p = os.path.join(d, f)
        text = open(p, errors="ignore").read()
        keep = [(m.start(), m.end()) for m in fence.finditer(text)]
        def fix(m):
            global unlinked
            if any(a <= m.start() < b for a, b in keep): return m.group(0)
            raw = m.group(2).strip("<>")
            if re.match(r"(https?:|mailto:|#)", raw) or not raw: return m.group(0)
            t = unquote(raw.split("#")[0])
            if not t: return m.group(0)
            if os.path.exists(os.path.normpath(os.path.join(d, t))): return m.group(0)
            rel = os.path.relpath(os.path.normpath(os.path.join(d, t)), root)
            if os.path.exists(os.path.join(src, rel)):
                unlinked += 1
                return m.group(1)
            broken.append((os.path.relpath(p, root), raw))
            return m.group(0)
        new = link.sub(fix, text)
        if new != text: open(p, "w").write(new)
for p, t in broken[:60]: print(f"  dangling link: {p} -> {t}", file=sys.stderr)
print(f"export-public: {unlinked} link(s) to left-out files made plain text, {len(broken)} broken link(s)")
sys.exit(1 if broken else 0)
P
# 3c. (nothing else yet)
[ "$bad" = 0 ] || fail "checks failed (the export is left in $OUT for you to look at)"

# 4. one commit
cd "$OUT"
git init -q -b main
git -c user.name="$NAME" -c user.email="$EMAIL" add -A
git -c user.name="$NAME" -c user.email="$EMAIL" commit -q -m "$MSG"
echo "export-public: OK. One commit $(git rev-parse --short HEAD) by $NAME in $OUT. Nothing was pushed."
