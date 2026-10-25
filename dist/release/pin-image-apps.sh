#!/usr/bin/env bash
# Fills in the real image digests of the example image apps (image-apps/*.json) from Docker Hub.
#   dist/release/pin-image-apps.sh [image-apps-folder]
# A release step run by the owner with a network connection (curl and python3 on the host). Not part of any image. The files keep their
# tag in the reference (name:tag@sha256:...); only an all-zero placeholder digest is replaced, a real pin is never changed (delete it first to re-pin).
# NOT RUN when it was written (no network); the registry calls follow Docker Hub's documented token + manifest protocol.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="${1:-$ROOT/image-apps}"
ZERO="sha256:0000000000000000000000000000000000000000000000000000000000000000"
ACCEPT='application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json'
for f in "$DIR"/*.json; do
  ref="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("image",""))' "$f")"
  case "$ref" in *"@$ZERO") ;; *) echo "skip $(basename "$f"): already pinned or not a container image"; continue ;; esac
  name="${ref%@*}"                       # docker.io/library/busybox:1.36
  repo="${name#docker.io/}"; tag="${repo##*:}"; repo="${repo%:*}"
  case "$name" in docker.io/*) ;; *) echo "skip $(basename "$f"): not on Docker Hub ($name)"; continue ;; esac
  token="$(curl -fsS "https://auth.docker.io/token?service=registry.docker.io&scope=repository:$repo:pull" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
  # the digest of the manifest (list) the tag points at is the SHA-256 of the exact bytes the registry returns for it
  digest="sha256:$(curl -fsS -H "Authorization: Bearer $token" -H "Accept: $ACCEPT" "https://registry-1.docker.io/v2/$repo/manifests/$tag" | sha256sum | cut -d' ' -f1)"
  python3 -I - "$f" "$ZERO" "$digest" <<'PY'
import sys
p, zero, digest = sys.argv[1:4]
t = open(p).read()
assert zero in t
open(p, "w").write(t.replace(zero, digest))
PY
  echo "pinned $(basename "$f") -> $digest"
done
