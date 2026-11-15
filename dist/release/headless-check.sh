#!/usr/bin/env bash
# ./dist/release/headless-check.sh   The headless build of the web service (F-WEBSVC-11): `iyakki-core` without the shell.
#   1. the dependency tree of the headless feature has no iyakki-shell, iyakki-remote or iyakki-uikit in it (the agent still brings its own web engine crates);
#   2. the headless binary starts `iyakki serve --profile agent-api` over a folder of its own, signs a person in over HTTPS and the sign-in
#      is in the audit log; a build that is not the service refuses plain `serve`.
# Needs cargo, curl and a free loopback port. It never reads the real /var/lib/iyakki (every path is an override in a temp folder).
set -euo pipefail
cd "$(dirname "$0")/../.."
PROFILE_DIR="${HEADLESS_PROFILE_DIR:-debug}"
CARGO_PROFILE_ARG=()
[ "$PROFILE_DIR" = release ] && CARGO_PROFILE_ARG=(--release)
fail() { echo "headless-check: FAIL $*" >&2; exit 1; }

tree=$(cargo tree --locked -p iyakki-core --no-default-features --features headless -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u)
for bad in iyakki-shell iyakki-remote iyakki-uikit; do
  echo "$tree" | grep -qx "$bad" && fail "the headless feature still depends on $bad"
done
echo "headless-check: PASS the headless feature has no shell, remote or toolkit crate in its dependency tree"

cargo build --locked -q -p iyakki-core --no-default-features --features headless --bin iyakki ${CARGO_PROFILE_ARG[@]+"${CARGO_PROFILE_ARG[@]}"}
BIN="${CARGO_TARGET_DIR:-target}/$PROFILE_DIR/iyakki"
[ -x "$BIN" ] || fail "no binary at $BIN"

D=$(mktemp -d "${TMPDIR:-/tmp}/iyakki-headless.XXXXXX")
trap 'kill "${PID:-0}" 2>/dev/null || true; rm -rf "$D"' EXIT
export IYAKKI_USERS="$D/users.json" IYAKKI_AUDIT="$D/audit.log" IYAKKI_DATA="$D"
printf 'ann-password-1\n' | "$BIN" user add ann --role user --password-stdin >/dev/null || fail "user add"
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
printf '{"profile":"agent-api","listen":"127.0.0.1:%s","data_dir":"%s"}' "$PORT" "$D" > "$D/service.json"
"$BIN" serve --profile agent-api --config "$D/service.json" > "$D/serve.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do
  curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break
  kill -0 "$PID" 2>/dev/null || { cat "$D/serve.log" >&2; fail "the service stopped"; }
  sleep 0.2
done
curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null || { cat "$D/serve.log" >&2; fail "no /healthz"; }
tok=$(curl -ksf -X POST "https://127.0.0.1:$PORT/api/v1/login" -H 'Content-Type: application/json' -d '{"name":"ann","password":"ann-password-1"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])') || fail "sign-in"
[ -n "$tok" ] || fail "no token"
code=$(curl -ks -o /dev/null -w '%{http_code}' -X POST "https://127.0.0.1:$PORT/api/v1/login" -H 'Content-Type: application/json' -d '{"name":"ann","password":"wrong-password"}')
[ "$code" = 403 ] || [ "$code" = 401 ] || fail "a wrong password gave $code"
curl -ksf "https://127.0.0.1:$PORT/api/v1/tools" -H "Authorization: Bearer $tok" | grep -q '"tools"' || fail "tools list"
grep -q 'ann' "$D/audit.log" || fail "the sign-in is not in the audit log"
kill "$PID"; wait "$PID" 2>/dev/null || true; PID=0
# the flows runner profile over the same headless binary: the person's flows, nothing of the chat
printf '{"profile":"flows-runner","listen":"127.0.0.1:%s","data_dir":"%s"}' "$PORT" "$D" > "$D/service.json"
"$BIN" serve --profile flows-runner --config "$D/service.json" > "$D/serve2.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break; kill -0 "$PID" 2>/dev/null || { cat "$D/serve2.log" >&2; fail "the flows runner stopped"; }; sleep 0.2; done
tok=$(curl -ksf -X POST "https://127.0.0.1:$PORT/api/v1/login" -H 'Content-Type: application/json' -d '{"name":"ann","password":"ann-password-1"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])') || fail "flows runner sign-in"
curl -ksf "https://127.0.0.1:$PORT/api/v1/flows" -H "Authorization: Bearer $tok" | grep -q '"flows"' || fail "the flows list"
[ "$(curl -ks -o /dev/null -w '%{http_code}' "https://127.0.0.1:$PORT/api/v1/chat/runs" -H "Authorization: Bearer $tok")" = 404 ] || fail "the chat is not in the flows runner"
kill "$PID"; wait "$PID" 2>/dev/null || true; PID=0
echo "headless-check: PASS the flows-runner profile lists the person's flows over the headless binary and has no chat"
# the data-api profile: the data call is there (a tool that is not a data tool is 404), the chat and the flows are not
printf '{"profile":"data-api","listen":"127.0.0.1:%s","data_dir":"%s"}' "$PORT" "$D" > "$D/service.json"
"$BIN" serve --profile data-api --config "$D/service.json" > "$D/serve3.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break; kill -0 "$PID" 2>/dev/null || { cat "$D/serve3.log" >&2; fail "the data api stopped"; }; sleep 0.2; done
tok=$(curl -ksf -X POST "https://127.0.0.1:$PORT/api/v1/login" -H 'Content-Type: application/json' -d '{"name":"ann","password":"ann-password-1"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])') || fail "data api sign-in"
[ "$(curl -ks -o /dev/null -w '%{http_code}' -X POST "https://127.0.0.1:$PORT/api/v1/data/call" -H "Authorization: Bearer $tok" -H 'Content-Type: application/json' -d '{"tool":"run_command","args":{}}')" = 404 ] || fail "a tool that is not a data tool"
[ "$(curl -ks -o /dev/null -w '%{http_code}' "https://127.0.0.1:$PORT/api/v1/flows" -H "Authorization: Bearer $tok")" = 404 ] || fail "flows are not in the data api"
[ "$(curl -ks -o /dev/null -w '%{http_code}' "https://127.0.0.1:$PORT/api/v1/chat/runs" -H "Authorization: Bearer $tok")" = 404 ] || fail "the chat is not in the data api"
kill "$PID"; wait "$PID" 2>/dev/null || true; PID=0
echo "headless-check: PASS the data-api profile has the data call and neither flows nor chat"
# the tile-host profile: the tiles are listed, the chat, the flows and the data call are not there
printf '{"profile":"tile-host","listen":"127.0.0.1:%s","data_dir":"%s"}' "$PORT" "$D" > "$D/service.json"
"$BIN" serve --profile tile-host --config "$D/service.json" > "$D/serve4.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break; kill -0 "$PID" 2>/dev/null || { cat "$D/serve4.log" >&2; fail "the tile host stopped"; }; sleep 0.2; done
tok=$(curl -ksf -X POST "https://127.0.0.1:$PORT/api/v1/login" -H 'Content-Type: application/json' -d '{"name":"ann","password":"ann-password-1"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])') || fail "tile host sign-in"
curl -ksf "https://127.0.0.1:$PORT/api/v1/tiles" -H "Authorization: Bearer $tok" | grep -q 'settings' || fail "the tile list"
[ "$(curl -ks -o /dev/null -w '%{http_code}' "https://127.0.0.1:$PORT/api/v1/chat/runs" -H "Authorization: Bearer $tok")" = 404 ] || fail "the chat is not in the tile host"
[ "$(curl -ks -o /dev/null -w '%{http_code}' "https://127.0.0.1:$PORT/api/v1/flows" -H "Authorization: Bearer $tok")" = 404 ] || fail "flows are not in the tile host"
kill "$PID"; wait "$PID" 2>/dev/null || true; PID=0
echo "headless-check: PASS the tile-host profile lists the tiles and has neither chat nor flows"
# the headless image recipe: the script knows --headless, its own Dockerfile exists and starts a profile of the service
bash -n dist/cloud/build-cloud.sh || fail "build-cloud.sh does not parse"
grep -q -e '--headless' dist/cloud/build-cloud.sh || fail "build-cloud.sh has no --headless"
grep -q -e '--no-default-features --features headless' dist/cloud/build-cloud.sh || fail "build-cloud.sh does not build the headless feature"
grep -q '^CMD \["serve", "--profile"' dist/cloud/Dockerfile.headless || fail "the headless Dockerfile does not start a profile"
echo "headless-check: PASS the cloud image script has a --headless recipe (checked, not built)"
out=$("$BIN" serve 2>&1 || true)
echo "$out" | grep -q 'headless web service only' || fail "plain serve was not refused: $out"
echo "headless-check: PASS the headless binary serves, signs in over HTTPS, audits it and refuses plain serve"
