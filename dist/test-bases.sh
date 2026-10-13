#!/bin/sh
# Try the static IYAKKI binary on several ordinary Linux distributions (in containers): does it start, make an account, run
# a WebAssembly guest in its sandbox and serve the desktop to a browser? Nothing is installed on this computer.
#   dist/test-bases.sh [path-to-static-iyakki] [image ...]
# The binary is the musl build:  cargo build --release --target x86_64-unknown-linux-musl -p iyakki-core
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$HERE/target/x86_64-unknown-linux-musl/release/iyakki}"
[ $# -gt 0 ] && shift
IMAGES="${*:-alpine:latest debian:stable-slim ubuntu:latest fedora:latest archlinux:latest}"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 2; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"; docker rm -f iyakki-base-test >/dev/null 2>&1' EXIT
cat > "$TMP/hello.wat" <<'WAT'
(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1)
  (data (i32.const 0) "hello from the sandbox") (func (export "_start") (call $l (i32.const 0) (i32.const 22))))
WAT
fail=0
for img in $IMAGES; do
  printf '%-24s ' "$img"
  docker rm -f iyakki-base-test >/dev/null 2>&1
  if ! docker pull -q "$img" >/dev/null 2>&1; then echo "SKIP (could not pull)"; continue; fi
  res=""
  # 1. the program starts and recognises where it is
  out=$(docker run --rm -v "$BIN:/iyakki:ro" "$img" /iyakki detect 2>&1) || { echo "FAIL detect: $out"; fail=1; continue; }
  res="detect=$out"
  # 2. a WebAssembly guest runs in the sandbox
  out=$(docker run --rm -v "$BIN:/iyakki:ro" -v "$TMP:/f:ro" "$img" /iyakki run /f/hello.wat 2>&1)
  echo "$out" | grep -q "hello from the sandbox" || { echo "FAIL wasm: $out"; fail=1; continue; }
  res="$res wasm=ok"
  # 3. an account, then the browser front end answers and signs the account in
  docker run -d --name iyakki-base-test -p 127.0.0.1::18080 -v "$BIN:/iyakki:ro" "$img" sh -c 'echo "a long test password" | /iyakki user add admin --role admin --password-stdin >/dev/null && exec /iyakki web http:18080' >/dev/null
  port=""
  for i in 1 2 3 4 5 6 7 8 9 10; do
    port=$(docker port iyakki-base-test 18080/tcp 2>/dev/null | head -1 | sed 's/.*://')
    [ -n "$port" ] && curl -s -o /dev/null "http://127.0.0.1:$port/healthz" && break
    sleep 1
  done
  health=$(curl -s "http://127.0.0.1:$port/healthz" 2>/dev/null)
  [ "$health" = "ok" ] || { echo "FAIL web: no answer ($(docker logs iyakki-base-test 2>&1 | tail -2 | tr '\n' ' '))"; fail=1; continue; }
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$port/api/login" -H 'Content-Type: application/json' -H "Origin: http://127.0.0.1:$port" -H "Host: 127.0.0.1:$port" -d '{"user":"admin","password":"a long test password"}')
  [ "$code" = "200" ] || { echo "FAIL login: HTTP $code"; fail=1; continue; }
  echo "PASS  $res web=ok login=ok"
  docker rm -f iyakki-base-test >/dev/null 2>&1
done
exit $fail
