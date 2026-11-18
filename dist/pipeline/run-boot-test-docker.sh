#!/bin/bash
# The same boot test without k3s (plain Docker, same image, same checks). Use it when no kubeconfig for this machine's k3s is set up.
#   Usage: dist/pipeline/run-boot-test-docker.sh IMAGE [SECONDS]      Results: $PIPELINE_RESULTS_DIR/<run-id>/ (default $HOME/iyakki-work/pipeline-results)
set -u
img="$(readlink -f "${1:?usage: run-boot-test-docker.sh IMAGE [SECONDS]}")"; secs="${2:-90}"
[ -f "$img" ] || { echo "no such image: $img" >&2; exit 2; }
root="${PIPELINE_RESULTS_DIR:-$HOME/iyakki-work/pipeline-results}"
id="${RUN_ID:-$(basename "$img" | sed 's/\.[^.]*$//')-$(date -u +%m%d-%H%M%S)}"
res="$root/$id"; mkdir -p "$res" && chmod 777 "$res" || exit 2
docker run --rm --device /dev/kvm -v "$(dirname "$img")":/image:ro -v "$res":/results \
  -e IMAGE="/image/$(basename "$img")" -e SECONDS="$secs" -e REQUIRE_KVM="${REQUIRE_KVM:-1}" \
  "${BOOT_TEST_IMAGE:-iyakki-build/boot-test:1}"
rc=$?
echo "results: $res"; exit $rc
