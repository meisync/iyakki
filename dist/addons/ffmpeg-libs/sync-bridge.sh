#!/bin/sh
# Copies the version map and the build recipe from a checkout of the iyakki-media-bridge repository into dist/addons/ffmpeg-libs/bridge/
# (the image build installs that folder as /usr/share/iyakki/av-addon/bridge; nothing is fetched from the network at image build time).
#   dist/addons/ffmpeg-libs/sync-bridge.sh /path/to/iyakki-media-bridge
# BRIDGE-SOURCE records the SHA-256 of every copied file and RECIPE.SHA256 the recipe hash the agent checks before it hands the recipe to k3s.
# The hash is over the files in this order: build.sh, configure-args.txt, files/iyakki-av-check (see avbuild::recipe_hash).
set -eu
SRC="${1:?path of the iyakki-media-bridge checkout}"
HERE="$(cd "$(dirname "$0")" && pwd)"; D="$HERE/bridge"
for f in versions.map recipe/build.sh recipe/configure-args.txt recipe/files/iyakki-av-check; do [ -f "$SRC/$f" ] || { echo "sync-bridge: $SRC/$f is missing" >&2; exit 1; }; done
mkdir -p "$D/recipe/files"
cp "$SRC/versions.map" "$D/versions.map"
cp "$SRC/recipe/build.sh" "$D/recipe/build.sh"
cp "$SRC/recipe/configure-args.txt" "$D/recipe/configure-args.txt"
cp "$SRC/recipe/files/iyakki-av-check" "$D/recipe/files/iyakki-av-check"
RH="$(cat "$D/recipe/build.sh" "$D/recipe/configure-args.txt" "$D/recipe/files/iyakki-av-check" | sha256sum | cut -d' ' -f1)"
printf '%s\n' "$RH" > "$D/RECIPE.SHA256"
{
  echo "# copied from the iyakki-media-bridge repository by sync-bridge.sh; sha256 of each file as copied"
  (cd "$D" && sha256sum versions.map recipe/build.sh recipe/configure-args.txt recipe/files/iyakki-av-check)
  echo "recipe-hash $RH"
} > "$D/BRIDGE-SOURCE"
echo "sync-bridge: done, recipe hash $RH"
