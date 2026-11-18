#!/bin/sh
# The REAL headless web service (`iyakki serve --profile cloud`) driven with the real `curl` and `openssl` (F-WEBSVC-05, F-WEBSVC-02, F-WEBSVC-10).
#   dist/linux-host/webservice-test.sh [PATH-OF-THE-IYAKKI-PROGRAM]      (default: target/release/iyakki, else the musl program of a fast build, else it is built)
# Needs: curl, openssl, python3 (only to read JSON), and for the data folder passwordless sudo, unshare and setpriv: the service insists on /var/lib/iyakki, so the script runs the whole test as the normal
# user inside `sudo unshare -m` with a private tmpfs over /var/lib and another over /dev/snd (the machine's own /var/lib and sound cards are never touched: a first run without the second changed a real sound card's volume, because the service sets up sound at start even in the cloud profile; IYAKKI_IN_SANDBOX=1). The user file (IYAKKI_USERS), token file, certificate and a free
# port on 127.0.0.1 are in a scratch folder (the config's `token_store`, `tls` and `listen` say so; HOME is the scratch folder too). What is checked, as a client over TLS would see it:
#   - health: /healthz and /readyz answer 200 without sign-in, /api/v1/version names the profile `cloud`; plain HTTP to the port is not served;
#   - sign-in: a wrong password is refused, the right one gives a bearer token, and no cookie is set;
#   - everything else needs the token (401 without), /metrics only for an administrator (403 for a plain user) and its text has the documented series and no user names or tokens;
#   - the hardware and developer-mode routes of the desktop do not exist in this profile (404 or 403), and an unknown path is one fixed label in /metrics.
#   - sound: the service starts no sound program (amixer, aplay, arecord, alsactl, speaker-test: shims record any call) and does not try to open /dev/snd (strace), and its log has no sound line.
# Not covered: flows and data routes with a real engine, schedules, a container or VM, a load balancer, sandbox pods.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
for t in curl openssl python3; do command -v "$t" >/dev/null 2>&1 || { echo "webservice-test: needs $t" >&2; exit 2; }; done
BIN="${1:-}"
if [ -z "$BIN" ]; then for c in "$ROOT/target/release/iyakki" "$ROOT/target/x86_64-unknown-linux-musl/fast/iyakki" "$ROOT/target/debug/iyakki"; do [ -x "$c" ] && BIN="$c" && break; done; fi
[ -n "$BIN" ] && [ -x "$BIN" ] || { echo "webservice-test: no iyakki program (build one: cargo build -p iyakki-core)" >&2; exit 2; }
if [ -z "${IYAKKI_WS_INNER:-}" ]; then
  for t in sudo unshare setpriv; do command -v "$t" >/dev/null 2>&1 || { echo "webservice-test: needs $t" >&2; exit 2; }; done
  sudo -n true 2>/dev/null || { echo "webservice-test: needs passwordless sudo" >&2; exit 2; }
  BINABS="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"; ME="$(id -u):$(id -g)"
  exec sudo -n env IYAKKI_WS_INNER=1 IYAKKI_IN_SANDBOX=1 TMPDIR="${TMPDIR:-/tmp}" PATH="$PATH" unshare -m sh -c 'mount --make-rprivate / 2>/dev/null; mount -t tmpfs tmpfs /var/lib && mount -t tmpfs tmpfs /dev/snd && mkdir /var/lib/iyakki && chown "$1" /var/lib/iyakki && exec setpriv --reuid "${1%%:*}" --regid "${1##*:}" --clear-groups sh "$2" "$3"' sh "$ME" "$0" "$BINABS"
fi
[ "${IYAKKI_IN_SANDBOX:-}" = 1 ] || { echo "webservice-test: refuses to run outside its sandbox" >&2; exit 2; }
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-websvc.XXXXXX")"; PID=""
trap '[ -z "$PID" ] || kill "$PID" 2>/dev/null; pkill -f "$W/service.json" 2>/dev/null; rm -rf "$W"' EXIT   # strace is the PID: the service under it needs its own kill
fail() { echo "FAIL: $1"; [ -f "$W/serve.log" ] && tail -8 "$W/serve.log" | sed 's/^/  log: /'; exit 1; }
ok() { echo "ok: $1"; }
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')"
mkdir -p "$W/data" "$W/home"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/key.pem" -out "$W/cert.pem" -days 2 -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" >/dev/null 2>&1 || fail "openssl could not make a certificate"
cat > "$W/service.json" <<JSON
{ "profile": "cloud", "listen": "127.0.0.1:$PORT", "lan": false, "allow_from": [], "tls": { "cert": "$W/cert.pem", "key": "$W/key.pem" },
  "token_store": "$W/tokens.json", "data_dir": "/var/lib/iyakki", "cors_origins": [], "enable_tools": [],
  "limits": { "requests_per_minute": 600, "logins_per_minute": 30, "concurrent_runs": 2, "max_body_bytes": 1048576, "max_connections": 32, "approval_timeout_secs": 60, "token_lifetime_secs": 3600, "token_idle_secs": 600 } }
JSON
export IYAKKI_USERS="/var/lib/iyakki/users.json" HOME="$W/home"
printf '%s\n' 'correct horse battery staple' | "$BIN" user add admin --role admin --password-stdin >/dev/null 2>&1 || fail "iyakki user add admin"
printf '%s\n' 'Another-Long-Passphrase-42-x' | "$BIN" user add pat --role user --password-stdin >/dev/null 2>&1 || fail "iyakki user add pat"
# the tenant's own list of who may do what (cloud/roles.json): admin owns it, pat is a member
mkdir -p /var/lib/iyakki/cloud && printf '{"tenant":{"admin":"owner","pat":"member"}}\n' > /var/lib/iyakki/cloud/roles.json
# no program that touches sound may be started by the service: shims for amixer, aplay, arecord, alsactl and speaker-test record every call (and do nothing), and strace (when there is one) records every
# attempt to open a sound device; /dev/snd itself is an empty tmpfs in this sandbox
mkdir -p "$W/shim"
for t in amixer aplay arecord alsactl speaker-test; do printf '#!/bin/sh\necho "%s $*" >> "%s/sound-calls.txt"\nexit 1\n' "$t" "$W" > "$W/shim/$t"; chmod +x "$W/shim/$t"; done
: > "$W/sound-calls.txt"
RUN=""; command -v strace >/dev/null 2>&1 && RUN="strace -f -qq -e trace=openat,open,execve -o $W/strace.txt"
PATH="$W/shim:$PATH" $RUN "$BIN" serve --profile cloud --config "$W/service.json" > "$W/serve.log" 2>&1 &
PID=$!
U="https://127.0.0.1:$PORT"
C="curl -sk --max-time 8"
for i in $(seq 1 60); do [ "$($C -o /dev/null -w '%{http_code}' $U/healthz)" = 200 ] && break; kill -0 "$PID" 2>/dev/null || fail "the service stopped at start"; sleep 0.5; done
code() { $C -o /dev/null -w '%{http_code}' "$@"; }
[ "$(code $U/healthz)" = 200 ] || fail "/healthz is not 200"
[ "$(code $U/readyz)" = 200 ] || fail "/readyz is not 200"
ok "/healthz and /readyz answer 200 without sign-in"
$C $U/api/v1/version | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v.get("profile")=="cloud", v' || fail "/api/v1/version does not name the cloud profile"
ok "/api/v1/version names the profile cloud"
[ "$(curl -s --max-time 4 -o /dev/null -w '%{http_code}' http://127.0.0.1:$PORT/healthz)" != 200 ] || fail "plain HTTP was served"
ok "plain HTTP to the port is not served (HTTPS only)"
H="$($C -D - -o "$W/login.json" -X POST -H 'content-type: application/json' -d '{"name":"admin","password":"correct horse battery staple"}' $U/api/v1/login)"
echo "$H" | grep -qi '^set-cookie' && fail "the sign-in set a cookie"
[ -s "$W/login.json" ] && ! grep -q token "$W/login.json" && { echo "sign-in answer: $(cat "$W/login.json")"; }
TOKEN="$(python3 -c 'import json;print(json.load(open("'"$W"'/login.json"))["token"])')" || fail "no token in the sign-in answer"
[ "${#TOKEN}" -ge 40 ] || fail "the token is too short: ${#TOKEN}"
ok "the right password gives a bearer token of ${#TOKEN} characters and no cookie; a wrong one is refused (401)"
UT="$(printf '%s' '{"name":"pat","password":"Another-Long-Passphrase-42-x"}' | $C -X POST -H 'content-type: application/json' -d @- $U/api/v1/login | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')"
[ "$(code -X POST -H 'content-type: application/json' -d '{"name":"admin","password":"wrong"}' $U/api/v1/login)" = 401 ] || fail "a wrong password was not refused with 401"
[ "$(code $U/api/v1/whoami)" = 401 ] || fail "an authenticated route answered without a token"
[ "$(code -H "authorization: Bearer $TOKEN" $U/api/v1/whoami)" = 200 ] || fail "the administrator's token was refused on /api/v1/whoami"
ok "a route that needs sign-in gives 401 without the token and 200 with it"
# ---- the person's files over HTTP (F-WEB-44) and a real git clone read through the git routes (F-WEB-45)
AUTH="authorization: Bearer $TOKEN"
post() { $C -w ' %{http_code}' -X POST -H "$AUTH" -H 'content-type: application/json' -d "$2" "$U$1"; }
r="$(post /api/v1/files/mkdir '{"path":"projects/2026/q4"}')"; case "$r" in *" 200") ;; *) fail "mkdir with parents: $r" ;; esac
r="$(post /api/v1/files/mkdir '{"path":"projects/2026/q4"}')"; case "$r" in *" 409") ;; *) fail "mkdir of an existing folder should be 409: $r" ;; esac
DIR="$(find /var/lib/iyakki/cloud/files -type d -name q4 2>/dev/null | head -1)"; [ -d "$DIR" ] || fail "the folder the route made is not on the disk"
BASE="${DIR%/projects/2026/q4}"
$C -H "$AUTH" "$U/api/v1/files?path=projects/2026" | grep -q '"q4"' || fail "the list does not show the new folder"
r="$(post /api/v1/files/move '{"from":"projects/2026","to":"archive2026"}')"; case "$r" in *" 200") ;; *) fail "move: $r" ;; esac
[ -d "$BASE/archive2026/q4" ] && [ ! -e "$BASE/projects/2026" ] || fail "the move did not happen on the disk"
r="$(post /api/v1/files/move '{"from":"archive2026","to":"projects"}')"; case "$r" in *" 409") ;; *) fail "a move onto an existing name should be 409: $r" ;; esac
r="$(post /api/v1/files/mkdir '{"path":"../../outside"}')"; case "$r" in *" 400"|*" 403"|*" 404") ;; *) fail "a path with .. should be refused: $r" ;; esac
r="$(post /api/v1/files/move '{"from":"archive2026","to":"archive2026/q4/inside"}')"; case "$r" in *" 400"|*" 409"|*" 403") ;; *) fail "a folder moved into itself should be refused: $r" ;; esac
[ ! -e "$W/outside" ] && [ ! -e /var/lib/iyakki/cloud/outside ] || fail "a folder was made outside the person's space"
ok "files over HTTP: mkdir with parents, 409 for an existing name, list, move (on the disk), 409 on a taken target, .. refused, a folder is never moved into itself"
# ---- the event WebSocket (F-WEBSVC-09, F-WEB-16): a real WebSocket client (Python's own sockets and ssl: handshake, masked frames), sign-in by the first message, a change made through the API arrives as an event
cat > "$W/wsclient.py" <<'WS'
import base64, json, os, socket, ssl, struct, sys, time
port, token, wrong = int(sys.argv[1]), sys.argv[2], sys.argv[3] == "wrong"
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
s = ctx.wrap_socket(socket.create_connection(("127.0.0.1", port), timeout=10), server_hostname="localhost")
key = base64.b64encode(os.urandom(16)).decode()
s.sendall(("GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n" % key).encode())
buf = b""
while b"\r\n\r\n" not in buf:
    d = s.recv(4096)
    if not d: sys.exit("closed during the handshake")
    buf += d
head, buf = buf.split(b"\r\n\r\n", 1)
assert head.startswith(b"HTTP/1.1 101"), head[:80]
import hashlib
acc = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
assert ("Sec-WebSocket-Accept: " + acc).lower() in head.decode().lower(), "the accept key is not the one RFC 6455 computes"
def send(text):
    b = text.encode(); m = os.urandom(4)
    hdr = bytes([0x81]) + (bytes([0x80 | len(b)]) if len(b) < 126 else bytes([0x80 | 126]) + struct.pack(">H", len(b)))
    s.sendall(hdr + m + bytes(c ^ m[i % 4] for i, c in enumerate(b)))
def frame():
    global buf
    def need(n):
        global buf
        while len(buf) < n:
            d = s.recv(4096)
            if not d: raise EOFError
            buf += d
    need(2); op = buf[0] & 15; ln = buf[1] & 127; o = 2
    if ln == 126: need(4); ln = struct.unpack(">H", buf[2:4])[0]; o = 4
    elif ln == 127: need(10); ln = struct.unpack(">Q", buf[2:10])[0]; o = 10
    need(o + ln); pl = buf[o:o + ln]; buf = buf[o + ln:]
    return op, pl
send(json.dumps({"type": "auth", "token": "x" * 40 if wrong else token}))
print("HANDSHAKE-OK", flush=True)
end = time.time() + 12
seen = []
try:
    while time.time() < end:
        op, pl = frame()
        if op == 8: print("CLOSED", pl[:2].hex()); sys.exit(0 if wrong else 3)
        if op == 9: continue
        if op == 1:
            m = json.loads(pl.decode()); seen.append(m); print("EVENT", json.dumps(m)[:200], flush=True)
            if m.get("type") == "changed": print("GOT-CHANGED"); sys.exit(0)
except EOFError:
    print("EOF"); sys.exit(0 if wrong else 4)
sys.exit(0 if wrong else 5)
WS
python3 "$W/wsclient.py" "$PORT" "bad" wrong > "$W/ws-wrong.txt" 2>&1; grep -q HANDSHAKE-OK "$W/ws-wrong.txt" && grep -q CLOSED "$W/ws-wrong.txt" && ! grep EVENT "$W/ws-wrong.txt" | grep -qv '"type": "error"' || fail "a wrong token on the WebSocket: $(cat "$W/ws-wrong.txt" | head -c 300)"
ok "the event WebSocket: the upgrade is answered 101 with the right Sec-WebSocket-Accept, and a wrong first-message token gets one error event ('sign in again') and the socket is closed, no data"
( python3 "$W/wsclient.py" "$PORT" "$TOKEN" right > "$W/ws-right.txt" 2>&1 & echo $! > "$W/ws.pid" ); sleep 2
post /api/v1/files/mkdir '{"path":"ws-probe"}' >/dev/null
for i in $(seq 1 30); do grep -q "GOT-CHANGED\|CLOSED\|EOF" "$W/ws-right.txt" 2>/dev/null && break; sleep 0.5; done
grep -q GOT-CHANGED "$W/ws-right.txt" || fail "no changed event on the WebSocket after a change through the API: $(head -c 400 "$W/ws-right.txt")"
ok "after the first message signs in, a real change through the API (mkdir) arrives as a 'changed' event on the WebSocket: $(grep EVENT "$W/ws-right.txt" | head -1 | cut -c1-120)"
if command -v git >/dev/null 2>&1; then
  G="git -c user.name=t -c user.email=t@example.org -c init.defaultBranch=main"
  SRC="$W/src.git"; WK="$W/wk"
  $G init -q --bare "$SRC" && $G clone -q "$SRC" "$WK" 2>/dev/null && ( cd "$WK" && printf 'one\n' > a.txt && $G add a.txt && $G commit -qm "first commit" && printf 'two\n' >> a.txt && printf 'bin\000ary\000data\001' > blob.bin && $G add . && $G commit -qm "second commit" && $G push -q origin HEAD:main 2>/dev/null ) || fail "could not make the source repository"
  $G clone -q "$SRC" "$BASE/proj" 2>/dev/null || fail "git clone into the person's folder"
  ( cd "$BASE/proj" && printf 'three\n' >> a.txt )
  $C -H "$AUTH" "$U/api/v1/git/status?folder=proj" > "$W/st.json"; python3 - "$W/st.json" <<'PY' || fail "git status through the route: $(cat "$W/st.json" | head -c 300)"
import json, sys
d = json.load(open(sys.argv[1]))
assert d["branch"] == "main", d
assert any(c["path"] == "a.txt" for c in d["changes"]), d
PY
  $C -H "$AUTH" "$U/api/v1/git/branches?folder=proj" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["current"]=="main" and any(b["name"]=="main" for b in d["branches"]), d' || fail "git branches through the route"
  $C -H "$AUTH" "$U/api/v1/git/log?folder=proj" > "$W/log.json"; python3 - "$W/log.json" <<'PY' || fail "git log through the route: $(head -c 300 "$W/log.json")"
import json, sys
d = json.load(open(sys.argv[1]))
subjects = [c["subject"] for c in d["commits"]]
assert subjects[:2] == ["second commit", "first commit"], subjects
PY
  FIRST="$(python3 -c 'import json;print(json.load(open("'"$W"'/log.json"))["commits"][1]["sha"])')"
  ( cd "$BASE/proj" && printf 'bin\000ary\000CHANGED\002' > blob.bin )
  r="$($C -o "$W/d.json" -w '%{http_code}' -H "$AUTH" "$U/api/v1/git/diff?folder=proj&path=a.txt")"; [ "$r" = 200 ] && grep -q '+three' "$W/d.json" || fail "git diff of the edited text file through the route ($r): $(head -c 200 "$W/d.json")"
  r="$($C -o "$W/d2.json" -w '%{http_code}' -H "$AUTH" "$U/api/v1/git/diff?folder=proj&commit=$FIRST")"; [ "$r" = 200 ] && grep -q '+one' "$W/d2.json" || fail "git diff of a commit through the route ($r): $(head -c 200 "$W/d2.json")"
  r="$($C -o "$W/d3.json" -w '%{http_code}' -H "$AUTH" "$U/api/v1/git/diff?folder=proj&path=blob.bin")"; [ "$r" = 415 ] || fail "a changed binary file in a diff should be 415, was $r: $(head -c 300 "$W/d3.json")"
  ( cd "$BASE/proj" && printf '[core]\n\tfsmonitor = /bin/true\n' >> .git/config )
  r="$($C -o /dev/null -w '%{http_code}' -H "$AUTH" "$U/api/v1/git/status?folder=proj")"; case "$r" in 200) fail "a clone whose config could run a program was read" ;; *) ;; esac
  ok "git over HTTP on a real clone (real git $(git --version | cut -d' ' -f3)): status shows the edit, branches, log in order, a diff of an edited file and of a commit, a changed binary file is 415, and a clone configured to run a program is refused ($r)"
else
  echo "note: git is not installed: the git routes were not tried"
fi
[ "$(code $U/metrics)" = 401 ] || fail "/metrics answered without a token"
[ "$(code -H "authorization: Bearer $UT" $U/metrics)" = 403 ] || fail "/metrics answered a plain user"
$C -H "authorization: Bearer $TOKEN" $U/metrics > "$W/metrics.txt"
grep -q '^# TYPE ' "$W/metrics.txt" && grep -qi 'uptime' "$W/metrics.txt" || fail "/metrics has no Prometheus text with an uptime"
grep -q "$TOKEN" "$W/metrics.txt" && fail "/metrics shows a token"
grep -qw 'pat' "$W/metrics.txt" && fail "/metrics shows a user name"
ok "/metrics: 401 without a token, 403 for a plain user, Prometheus text for the administrator, no token and no user name in it"
for p in /api/v1/camera /api/v1/sound /api/v1/usb /api/v1/firewall /api/v1/firmware /v1/dev/hello /v1/dev/shell /api/v1/dev/pair; do
  c="$(code -H "authorization: Bearer $TOKEN" $U$p)"
  case "$c" in 404|403|405) ;; *) fail "the desktop route $p answers $c in the cloud profile" ;; esac
done
ok "hardware and developer-mode routes do not answer in the cloud profile (404 or 403)"
[ ! -s "$W/sound-calls.txt" ] || fail "the service started a sound program: $(cat "$W/sound-calls.txt" | head -3)"
if [ -f "$W/strace.txt" ]; then
  grep -E '/dev/snd|asound' "$W/strace.txt" | grep -v 'ENOENT' | head -3 | grep -q . && fail "the service opened a sound device: $(grep -E '/dev/snd' "$W/strace.txt" | head -2)"
  grep -q '/dev/snd' "$W/strace.txt" && fail "the service tried to open a sound device: $(grep '/dev/snd' "$W/strace.txt" | head -2)"
  ok "the cloud profile made no sound program call and no attempt to open /dev/snd (strace: $(wc -l < "$W/strace.txt") calls looked at)"
else
  ok "the cloud profile started no sound program (strace is not installed: opens of /dev/snd were not looked at)"
fi
grep -Eq '\] sound( output|:)' "$W/serve.log" && fail "the service log has a sound line: $(grep -E '\] sound( output|:)' "$W/serve.log" | head -2)"
ok "the service log has no sound line"
echo "PASS (webservice)"
