#!/bin/bash
# Boot a disk image in QEMU inside k3s and check it.      Usage: dist/pipeline/run-boot-test.sh IMAGE [SECONDS]
#   IMAGE    raw disk image (for example ~/iyakki-work/images/<name>/iyakki-lean-x86_64.img)
#   SECONDS  how long the guest runs (default 90)
# Environment (all optional):
#   PIPELINE_RESULTS_DIR  default $HOME/iyakki-work/pipeline-results   (a run goes to <dir>/<run-id>/)
#   RUN_ID                default <image-name>-<UTC time>
#   BOOT_TEST_IMAGE       default localhost:5000/iyakki-build/boot-test:1  (the local registry that k3s mirrors as http; see README.md)
#   PIPELINE_NODE         default this machine's hostname (the k3s node must run on THIS machine: the Job mounts its paths)
#   PRIVILEGED            default true  (false = try without; QEMU then falls back to TCG unless /dev/kvm is allowed)
#   REQUIRE_KVM           default 1     (fail instead of running slow emulation)
#   WAIT_EXTRA            default 240   (seconds the wrapper waits beyond SECONDS for image pull and start)
#   KEEP_JOB              1 = do not delete the Job when done (default: deleted; the results stay on disk)
# Exit: 0 pass, 1 a check failed, 2 the harness/Job failed or timed out.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
img="${1:?usage: run-boot-test.sh IMAGE [SECONDS]}"
secs="${2:-90}"
[ -f "$img" ] || { echo "no such image: $img" >&2; exit 2; }
img="$(readlink -f "$img")"
res_root="${PIPELINE_RESULTS_DIR:-$HOME/iyakki-work/pipeline-results}"
run_id="${RUN_ID:-$(basename "$img" | sed 's/\.[^.]*$//')-$(date -u +%m%d-%H%M%S)}"
run_id="$(echo "$run_id" | tr 'A-Z_.' 'a-z--' | tr -c 'a-z0-9\n-' '-' | cut -c1-48 | sed 's/-*$//')"
res="$res_root/$run_id"
node="${PIPELINE_NODE:-$(hostname)}"
extra="${WAIT_EXTRA:-240}"

# Guard: hostPath mounts (image, results, /dev/kvm) only make sense when the k3s node IS this machine.
mid="$(kubectl get node "$node" -o jsonpath='{.status.nodeInfo.machineID}' 2>/dev/null)"
if [ -z "$mid" ] || [ "$mid" != "$(cat /etc/machine-id)" ]; then
  echo "kubectl (KUBECONFIG=${KUBECONFIG:-default}) has no node '$node' that is this machine; set KUBECONFIG to this machine's k3s and/or PIPELINE_NODE. See README.md." >&2
  exit 2
fi
mkdir -p "$res" && chmod 777 "$res" || exit 2
kubectl get ns iyakki-build >/dev/null 2>&1 || kubectl apply -f "$here/namespace.yaml" >/dev/null || exit 2

sed -e "s|@RUN_ID@|$run_id|g" \
    -e "s|@NODE@|$node|g" \
    -e "s|@BOOT_TEST_IMAGE@|${BOOT_TEST_IMAGE:-localhost:5000/iyakki-build/boot-test:1}|g" \
    -e "s|@IMAGE_FILE@|$(basename "$img")|g" \
    -e "s|@IMAGE_DIR@|$(dirname "$img")|g" \
    -e "s|@RESULT_DIR@|$res|g" \
    -e "s|@SECONDS@|$secs|g" \
    -e "s|@DEADLINE@|$((secs + extra + 60))|g" \
    -e "s|@REQUIRE_KVM@|${REQUIRE_KVM:-1}|g" \
    -e "s|@PRIVILEGED@|${PRIVILEGED:-true}|g" \
    "$here/boot-test-job.yaml.tmpl" > "$res/job.yaml"

job="boot-test-$run_id"
kubectl apply -f "$res/job.yaml" >/dev/null || { echo "could not create the Job" >&2; exit 2; }
echo "run $run_id: Job $job, results in $res"

end=$(( $(date +%s) + secs + extra ))
while [ ! -f "$res/DONE" ]; do
  st="$(kubectl -n iyakki-build get job "$job" -o jsonpath='{.status.failed}/{.status.succeeded}' 2>/dev/null)"
  case "$st" in 1/*|*/1) sleep 5; break;; esac     # Job ended; give the result file a moment to appear
  [ "$(date +%s)" -ge "$end" ] && { echo "timed out after $((secs + extra)) s" >&2; break; }
  sleep 3
done

cleanup() { [ "${KEEP_JOB:-0}" = 1 ] || kubectl -n iyakki-build delete job "$job" --wait=false >/dev/null 2>&1; }
if [ ! -f "$res/DONE" ]; then
  echo "no result written; pod state:" >&2
  kubectl -n iyakki-build describe pod -l job-name="$job" 2>&1 | tail -15 >&2
  kubectl -n iyakki-build logs -l job-name="$job" --tail=20 >&2 2>&1
  cleanup; exit 2
fi
kubectl -n iyakki-build logs "job/$job" > "$res/pod.log" 2>&1
cleanup
cat "$res/result.json"
echo "serial log: $res/serial.log   screendumps: $res/shot-*.png"
grep -q '"error"' "$res/result.json" && exit 2
[ "$(cat "$res/DONE")" = pass ] || exit 1
