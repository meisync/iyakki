#!/usr/bin/env bash
# dist/scripts/app-isolation-check.sh   The isolation check of docs/development/APP-ISOLATION-CHECK.md against a running web service:
#   1. API walls: no token, a bad token and an unknown tile are refused; a person cannot read another person's run;
#   2. every tile calls only its own tools (read from the audit log after the browser has driven each tile);
#   3. the web UI and every tile in a phone-sized window (needs google-chrome and node 22; skipped with a note when missing).
# It starts the service itself over a temp folder (never the real /var/lib/iyakki) with two test accounts. Build the service first:
#   cargo build -p iyakki-core --no-default-features --features headless --bin iyakki     (BIN=path/to/iyakki to use another)
set -uo pipefail
cd "$(dirname "$0")/../.."
HERE="$(pwd)"
BIN="${BIN:-target/debug/iyakki}"
[ -x "$BIN" ] || { echo "app-isolation: no binary at $BIN (see the header of this script)"; exit 2; }
D=$(mktemp -d "${TMPDIR:-/tmp}/iyakki-isolation.XXXXXX"); OUT="${ISOLATION_OUT:-$D/shots}"; mkdir -p "$OUT"
trap 'kill "${PID:-0}" 2>/dev/null || true; rm -rf "$D/data" "$D/chrome"' EXIT
export IYAKKI_USERS="$D/data/users.json" IYAKKI_AUDIT="$D/data/audit.log" IYAKKI_DATA="$D/data"; mkdir -p "$D/data"
printf 'ann-test-pass-1\n' | "$BIN" user add ann --role user --password-stdin >/dev/null || { echo "user add failed"; exit 2; }
printf 'bob-test-pass-1\n' | "$BIN" user add bob --role user --password-stdin >/dev/null || { echo "user add failed"; exit 2; }
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
printf '{"profile":"agent-api","listen":"127.0.0.1:%s","data_dir":"%s"}' "$PORT" "$D/data" > "$D/data/service.json"
"$BIN" serve --profile agent-api --config "$D/data/service.json" > "$D/serve.log" 2>&1 & PID=$!
for _ in $(seq 1 80); do curl -ksf "https://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break; kill -0 "$PID" 2>/dev/null || { cat "$D/serve.log" >&2; exit 2; }; sleep 0.25; done
B="https://127.0.0.1:$PORT"; bad=0
ok() { echo "PASS $1"; }; no() { echo "FAIL $1"; bad=$((bad+1)); }
tokof() { curl -ksf -X POST "$B/api/v1/login" -H 'Content-Type: application/json' -d "{\"name\":\"$1\",\"password\":\"$2\"}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'; }
ANN=$(tokof ann ann-test-pass-1); BOB=$(tokof bob bob-test-pass-1)
code() { curl -ks -o /dev/null -w '%{http_code}' "$@"; }
[ "$(code "$B/api/v1/tiles")" = 401 ] && ok "no token: the tile list answers 401" || no "no token was not refused"
[ "$(code "$B/api/v1/tiles" -H 'Authorization: Bearer iyk_nonsense')" = 401 ] && ok "a bad token answers 401" || no "a bad token was not refused"
[ "$(code -X POST "$B/api/v1/tiles/nope/events" -H "Authorization: Bearer $ANN" -H 'Content-Type: application/json' -d '{"width":320,"height":200,"events":[]}')" = 404 ] && ok "an unknown tile answers 404" || no "an unknown tile was not refused"
RUN=$(curl -ks -X POST "$B/api/v1/chat/runs" -H "Authorization: Bearer $ANN" -H 'Content-Type: application/json' -d '{"prompt":"isolation check"}' | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')
if [ -n "$RUN" ]; then
  [ "$(code "$B/api/v1/chat/runs/$RUN" -H "Authorization: Bearer $ANN")" = 200 ] && ok "a person reads their own run" || no "a person cannot read their own run"
  [ "$(code "$B/api/v1/chat/runs/$RUN" -H "Authorization: Bearer $BOB")" = 404 ] && ok "another person cannot read it (404)" || no "another person could read the run"
  curl -ks "$B/api/v1/chat/runs" -H "Authorization: Bearer $BOB" | grep -q "$RUN" && no "another person's run list shows it" || ok "another person's run list does not show it"
else no "no run could be started"; fi
if command -v google-chrome >/dev/null 2>&1 && command -v node >/dev/null 2>&1; then
  CDP_PROFILE="$D/chrome" node "$HERE/dist/scripts/app-isolation/mobile-tiles.mjs" "$B" ann ann-test-pass-1 "$OUT" || bad=$((bad+1))
else echo "SKIP the phone-sized window check: google-chrome and node 22 are needed"; fi
# every tile calls only its own tools (the audit log has one line per call)
python3 - "$D/data/audit.log" <<'P' || bad=$((bad+1))
import json, re, sys
allowed = {"settings": {"settings_get", "settings_set"}, "edit": set(), "runs": {"runs_list", "runs_get", "runs_decide"}, "chat": {"chat_models", "chat_send", "runs_get", "chat_stop"}, "counter": set()}
seen = {}
for l in open(sys.argv[1]):
    try: d = json.loads(l)
    except ValueError: continue
    if d.get("event") in ("tile.call", "tile.asked", "tile.decide"):
        m = re.search(r'tile=(\w+).*?tool=(\w+)', d.get("detail", ""))
        if m: seen.setdefault(m.group(1), set()).add(m.group(2))
fail = False
for t, tools in sorted(seen.items()):
    extra = tools - allowed.get(t, set())
    print(("FAIL" if extra else "PASS"), f'tile "{t}" called {sorted(tools)}' + (f" (not its own: {sorted(extra)})" if extra else ""))
    fail |= bool(extra)
print(("SKIP no tile call was recorded (the browser check did not run)" if not seen else "PASS no tile called anything outside its list") if not fail else "FAIL a tile called a tool that is not its own")
sys.exit(1 if fail else 0)
P
echo "screenshots: $OUT"
[ "$bad" = 0 ] && echo "app-isolation: PASS" || { echo "app-isolation: $bad FAIL"; exit 1; }
