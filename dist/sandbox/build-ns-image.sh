#!/usr/bin/env bash
# dist/sandbox/build-ns-image.sh --iyakki FILE --rootfs DIR [--out DIR] [--tag NAME] [--build]
#   Prepares the build context of the namespace-tier sandbox image (dist/sandbox/Dockerfile.ns): the static `iyakki` program and the
#   operator's rootfs folder. Refuses a rootfs that holds a set-uid or set-gid file, a device node, or a symbolic link that points
#   outside the folder. With --build it then runs `podman build` (or `docker build`) on the context; without it only the context is
#   made and the command is printed. Nothing is downloaded and no base image is pulled. The rootfs is not shipped by IYAKKI.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN=""; ROOT=""; OUT=""; TAG="iyakki-sandbox-ns:local"; BUILD=0
fail() { echo "build-ns-image: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
  case "$1" in
    --iyakki) BIN="${2:-}"; shift 2;; --rootfs) ROOT="${2:-}"; shift 2;; --out) OUT="${2:-}"; shift 2;;
    --tag) TAG="${2:-}"; shift 2;; --build) BUILD=1; shift;;
    *) fail "unknown option $1 (usage: --iyakki FILE --rootfs DIR [--out DIR] [--tag NAME] [--build])";;
  esac
done
[ -n "$BIN" ] && [ -f "$BIN" ] && [ -x "$BIN" ] || fail "--iyakki FILE: the static iyakki program is needed"
[ -n "$ROOT" ] && [ -d "$ROOT" ] || fail "--rootfs DIR: the folder the sandboxed program sees as / is needed"
case "$TAG" in *[!a-zA-Z0-9._:/-]*|"") fail "the tag is plain letters, digits and . _ : / -";; esac
if command -v file >/dev/null 2>&1 && file -L "$BIN" | grep -q "dynamically linked"; then fail "$BIN is dynamically linked: the image has no libraries (use the static musl build)"; fi
[ -z "$(find "$ROOT" -type f \( -perm -4000 -o -perm -2000 \) -print -quit)" ] || fail "the rootfs holds a set-uid or set-gid file: $(find "$ROOT" -type f \( -perm -4000 -o -perm -2000 \) -print -quit)"
[ -z "$(find "$ROOT" \( -type b -o -type c \) -print -quit)" ] || fail "the rootfs holds a device node: the sandbox makes its own /dev"
ROOT_ABS="$(cd "$ROOT" && pwd -P)"
while IFS= read -r link; do
  target="$(readlink "$link")"
  case "$target" in
    /*) ;;  # an absolute link is resolved inside the pivoted root, so it cannot leave it
    *) resolved="$(python3 -c 'import os,sys; print(os.path.normpath(os.path.join(os.path.realpath(sys.argv[1]), sys.argv[2])))' "$(dirname "$link")" "$target")"; case "$resolved" in "$ROOT_ABS"|"$ROOT_ABS"/*) ;; *) fail "the symbolic link $link points outside the rootfs";; esac;;
  esac
done < <(find "$ROOT" -type l)
[ -n "$OUT" ] || OUT="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-sandbox-ns.XXXXXX")"
mkdir -p "$OUT/rootfs"
cp "$BIN" "$OUT/iyakki"; chmod 0555 "$OUT/iyakki"
cp -a "$ROOT/." "$OUT/rootfs/"
cp "$HERE/Dockerfile.ns" "$OUT/Dockerfile"
echo "build-ns-image: context ready in $OUT"
if [ "$BUILD" = 1 ]; then
  ENGINE="$(command -v podman || command -v docker || true)"
  [ -n "$ENGINE" ] || fail "neither podman nor docker is installed: build the context yourself: <engine> build -t $TAG $OUT"
  "$ENGINE" build -t "$TAG" "$OUT"
else
  echo "build-ns-image: build it with: podman build -t $TAG $OUT"
fi
