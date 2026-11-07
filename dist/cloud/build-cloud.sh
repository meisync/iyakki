#!/usr/bin/env bash
# One entry point for the IYAKKI cloud artifacts (docs/CLOUD-IMAGES.md).
#
#   dist/cloud/build-cloud.sh docker [--arm64] [--no-build]   OCI image (linux/amd64; --arm64 adds linux/arm64), SBOM, licences, tarball, sums, signature
#   dist/cloud/build-cloud.sh iso    [--dry-run] [--no-build] the cloud ISO (VMs, optical) by REPACKING the existing web+mac builds: no Buildroot make
#   dist/cloud/build-cloud.sh disk                            the stick-ready raw disk image (GPT + ESP + boot volume) from that repack
#   dist/cloud/build-cloud.sh qcow2                           the same disk as qcow2 for VMs
#   dist/cloud/build-cloud.sh all                             docker, iso, disk, qcow2
#   dist/cloud/build-cloud.sh test [docker|iso|hw|all]        container test, QEMU boot tests, hardware-quirk checks (default: all)
#   dist/cloud/build-cloud.sh clean                           remove this version's output folder and the local image tags
#
# Output: ~/iyakki-work/cloud/<version>/ (IYAKKI_CLOUD_OUT changes the base). Logs: ~/iyakki-work/logs/cloud-*.log.
# Rules kept: incremental (cargo reuses target/; repack reuses the finished builds), one cargo at a time (flock + memory caps), nothing is
# pushed or logged in anywhere, nothing is written to a drive, no sudo, no container runs privileged or with a host mount.
# docker build pulls NOTHING (FROM scratch). Tools needed: cargo, docker, qemu-img, qemu-system-x86_64 + OVMF, xorriso, curl, python3.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
DESCRIBE="$(cd "$ROOT" && git describe --tags --always --dirty 2>/dev/null || echo unknown)"
EPOCH="$(cd "$ROOT" && git log -1 --format=%ct 2>/dev/null || echo 0)"
WORK="${IYAKKI_CLOUD_OUT:-$HOME/iyakki-work/cloud}"
OUT="$WORK/$VERSION"
LOGS="$HOME/iyakki-work/logs"
REPO="${IYAKKI_IMAGE_REPO:-iyakki-cloud}"          # a local name; nothing is ever pushed by this script
SOURCE_URL="${IYAKKI_SOURCE_URL:-https://example.invalid/iyakki-source}"   # placeholder until the owner names the public repository
KEY="${IYAKKI_RELEASE_KEY:-$WORK/release-dev.key}"  # a local development signing key, made on first use; use the real release key for a release
CLOUD_BR="$ROOT/target/buildroot-cloud-release"
mkdir -p "$OUT" "$LOGS"
say() { echo "build-cloud: $*" >&2; }
die() { echo "build-cloud: $*" >&2; exit 1; }

# one cargo at a time, capped memory and jobs (the shared lock is also taken by the other build scripts)
capped() { flock "$HOME/.cache/iyakki-build.lock" systemd-run --user --scope -q -p MemoryMax=9G -p MemorySwapMax=6G nice env CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}" "$@"; }

build_bin() { # ARCH -> path of the static program
  local arch="$1" tgt
  case "$arch" in
    amd64) tgt=x86_64-unknown-linux-musl ;;
    arm64) tgt=aarch64-unknown-linux-musl ;;
    *) die "unknown architecture $arch" ;;
  esac
  if [ "${NO_BUILD:-0}" != 1 ]; then
    if [ "$arch" = arm64 ]; then
      # no emulation: the cross C compiler of the aarch64 Buildroot toolchain, as dist/buildroot/build.sh uses it
      local cross; cross="$(ls "$ROOT"/target/buildroot-*aarch64*/host/bin/aarch64-*-linux-musl-gcc 2>/dev/null | head -n1 || true)"
      [ -n "$cross" ] || die "arm64: no cross compiler (target/buildroot-*aarch64*/host/bin/aarch64-*-linux-musl-gcc); build the aarch64 toolchain first: dist/buildroot/build.sh libre aarch64 (not started here)"
      export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$cross" CC_aarch64_unknown_linux_musl="$cross" AR_aarch64_unknown_linux_musl="${cross%gcc}ar"
    fi
    say "cargo build --release --locked --target $tgt -p iyakki-core (incremental; log $LOGS/cloud-cargo-$arch.log)"
    (cd "$ROOT" && capped cargo build --release --locked --target "$tgt" -p iyakki-core) >"$LOGS/cloud-cargo-$arch.log" 2>&1 || { tail -20 "$LOGS/cloud-cargo-$arch.log" >&2; die "cargo build failed (see $LOGS/cloud-cargo-$arch.log)"; }
  fi
  local b="$ROOT/target/$tgt/release/iyakki"
  [ -x "$b" ] || die "$b is missing"
  echo "$b"
}

host_bin() { # a program that runs on this machine (the static amd64 one)
  local b="$ROOT/target/x86_64-unknown-linux-musl/release/iyakki"; [ -x "$b" ] || die "$b is missing: run  build-cloud.sh docker  first"; echo "$b"
}

make_ctx() { # ARCH BIN -> prepared docker context folder
  local arch="$1" bin="$2" c="$OUT/ctx-$arch"
  rm -rf "$c"; mkdir -p "$c/etc/ssl/certs" "$c/etc/iyakki" "$c/licenses" "$c/var/lib/iyakki" "$c/tmp"
  cp "$bin" "$c/iyakki"
  [ -r /etc/ssl/certs/ca-certificates.crt ] || die "no /etc/ssl/certs/ca-certificates.crt on this machine"
  cp /etc/ssl/certs/ca-certificates.crt "$c/etc/ssl/certs/"
  for f in LICENSE NOTICE THIRD_PARTY_LICENSES.txt legal/THIRD-PARTY-NOTICES.md legal/sbom.cdx.json; do cp "$ROOT/$f" "$c/licenses/"; done
  printf 'The CA bundle /etc/ssl/certs/ca-certificates.crt is the Mozilla root list as packaged by the build machine (MPL-2.0 data).\nThe program /iyakki is statically linked (musl, MIT) with the permissive Rust crates listed in THIRD_PARTY_LICENSES.txt and sbom.cdx.json.\nNo GPL or LGPL program is in this image. Corresponding source of the program: %s\n' "$SOURCE_URL" >"$c/licenses/README.txt"
  cp "$ROOT/dist/service/service.cloud.json" "$c/etc/iyakki/service.cloud.example.json"
  "$bin" cloud example example-tenant >"$c/etc/iyakki/tenant.example.json" 2>/dev/null || rm -f "$c/etc/iyakki/tenant.example.json"
  cp "$HERE/Dockerfile" "$c/Dockerfile"
  chmod 1777 "$c/tmp"; find "$c" -exec touch -d "@$EPOCH" {} + 2>/dev/null || true
  # no secrets in the context: refuse private keys and token files
  if grep -rIl -e "PRIVATE KEY-----" -e "ghp_" -e "sk-ant-" "$c" --exclude=ca-certificates.crt 2>/dev/null | grep -q .; then die "a secret-looking string is in the build context $c"; fi
  say "context $c: $(du -sh "$c" | cut -f1), $(find "$c" -type f | wc -l) files"
}

sign_out() {
  [ -f "$KEY" ] || { say "making a local development signing key $KEY (a real release uses the release key: IYAKKI_RELEASE_KEY)"; "$(host_bin)" release keygen "$KEY" >/dev/null; chmod 600 "$KEY"; }
  "$(host_bin)" release sign "$OUT" --key "$KEY" --title "IYAKKI cloud" --version "$VERSION" --git-describe "$DESCRIBE" --date "$(date -u -d "@$EPOCH" +%Y-%m-%d)" | tail -n 12
}

cmd_docker() {
  local arm=0
  for a in "$@"; do case "$a" in --arm64) arm=1 ;; --no-build) NO_BUILD=1 ;; *) die "docker: unknown option $a" ;; esac; done
  command -v docker >/dev/null || die "docker is not installed"
  local arches="amd64"; [ "$arm" = 1 ] && arches="amd64 arm64"
  local created; created="$(date -u -d "@$EPOCH" +%Y-%m-%dT%H:%M:%SZ)"
  local tars=""
  for arch in $arches; do
    local bin; bin="$(build_bin "$arch")"
    local sum; sum="$(sha256sum "$bin" | cut -d' ' -f1)"
    echo "$sum  iyakki-$arch" >>"$OUT/binary.sha256.new"
    make_ctx "$arch" "$bin"
    local tag="$REPO:$VERSION-$arch"
    say "docker build $tag (no base image is pulled: FROM scratch)"
    # classic builder is enough for a FROM-scratch COPY image; memory-capped
    DOCKER_BUILDKIT=0 nice docker build --memory 6g --platform "linux/$arch" \
      --label "org.opencontainers.image.title=IYAKKI cloud" \
      --label "org.opencontainers.image.description=IYAKKI: the desktop in a browser and the cloud tenant tools, one static program" \
      --label "org.opencontainers.image.version=$VERSION" \
      --label "org.opencontainers.image.revision=$DESCRIBE" \
      --label "org.opencontainers.image.created=$created" \
      --label "org.opencontainers.image.licenses=Apache-2.0" \
      --label "org.opencontainers.image.source=$SOURCE_URL" \
      --label "org.opencontainers.image.vendor=IYAKKI" \
      --label "dev.iyakki.binary.sha256=$sum" \
      -t "$tag" -f "$OUT/ctx-$arch/Dockerfile" "$OUT/ctx-$arch" >"$LOGS/cloud-docker-$arch.log" 2>&1 || { tail -20 "$LOGS/cloud-docker-$arch.log" >&2; die "docker build failed ($LOGS/cloud-docker-$arch.log)"; }
    [ "$arch" = amd64 ] && docker tag "$tag" "$REPO:$VERSION"
    docker save -o "$OUT/iyakki-cloud-$VERSION-$arch.tar" "$tag"
    tars="$tars $OUT/iyakki-cloud-$VERSION-$arch.tar"
    say "image $tag: $(docker image inspect "$tag" --format '{{.Size}}' | awk '{printf "%.1f MB", $1/1e6}') ; tarball $(du -h "$OUT/iyakki-cloud-$VERSION-$arch.tar" | cut -f1)"
  done
  mv "$OUT/binary.sha256.new" "$OUT/binary.sha256"
  cp "$ROOT/legal/sbom.cdx.json" "$OUT/iyakki-sbom.cdx.json"
  cp "$ROOT/THIRD_PARTY_LICENSES.txt" "$ROOT/LICENSE" "$ROOT/NOTICE" "$OUT/" 2>/dev/null || true
  sign_out
  say "docker done: $OUT"
}

cmd_iso() {
  local dry=0 recipe_base="${IYAKKI_CLOUD_BASE:-target/buildroot-web}" recipe_kernel="${IYAKKI_CLOUD_KERNEL:-target/buildroot-mac}"
  for a in "$@"; do case "$a" in --dry-run) dry=1 ;; --no-build) NO_BUILD=1 ;; *) die "iso: unknown option $a" ;; esac; done
  # The cloud variant is a REPACK of the finished web + mac builds with VARIANT=cloud-x86_64 in /etc/iyakki-release (which turns Cloud mode on
  # by itself), the release profile (no vendor firmware), and the new program. No Buildroot make runs; the cost is one zstd pass (minutes).
  [ -s "$ROOT/$recipe_base/images/rootfs.cpio" ] && [ -s "$ROOT/$recipe_kernel/images/bzImage" ] || die "iso: $recipe_base and $recipe_kernel need finished builds (not started here: that is a Buildroot full build)"
  if [ "$dry" = 1 ]; then
    WEB_IRIS="${WEB_IRIS:-1}" IYAKKI_IMAGE_PROFILE=release "$ROOT/dist/buildroot/repack.sh" cloud --base "$recipe_base" --kernel-from "$recipe_kernel" --graft git --dry-run
    return
  fi
  [ "${NO_BUILD:-0}" = 1 ] || build_bin amd64 >/dev/null
  say "repack (log $LOGS/cloud-repack.log)"
  (cd "$ROOT" && WEB_IRIS="${WEB_IRIS:-1}" IYAKKI_IMAGE_PROFILE=release nice "$ROOT/dist/buildroot/repack.sh" cloud --base "$recipe_base" --kernel-from "$recipe_kernel" --graft git) >"$LOGS/cloud-repack.log" 2>&1 || { tail -20 "$LOGS/cloud-repack.log" >&2; die "repack failed"; }
  local iso="$CLOUD_BR/images/iyakki-x86_64.iso"
  [ -s "$iso" ] || die "no ISO at $iso"
  cp "$iso" "$OUT/iyakki-cloud-$VERSION-x86_64.iso"
  cp "$CLOUD_BR/images/iyakki-install-x86_64.img" "$OUT/iyakki-cloud-$VERSION-x86_64.img"
  say "ISO  $(du -h "$OUT/iyakki-cloud-$VERSION-x86_64.iso" | cut -f1)   disk image  $(du -h "$OUT/iyakki-cloud-$VERSION-x86_64.img" | cut -f1)"
  for l in "$ROOT"/target/buildroot-web/legal-info/manifest.csv; do [ -f "$l" ] && cp "$l" "$OUT/buildroot-legal-manifest.csv"; done
  [ -f "$ROOT/legal/sbom-images.cdx.json" ] && cp "$ROOT/legal/sbom-images.cdx.json" "$OUT/iyakki-image-sbom.cdx.json"
  sign_out
}

cmd_disk() { # the stick-ready image is made by the repack together with the ISO; this checks and shows it
  local f="$OUT/iyakki-cloud-$VERSION-x86_64.img"
  [ -s "$f" ] || { say "no disk image yet: running iso first"; cmd_iso "$@"; }
  file "$f" | sed 's/^/  /'
  say "disk image $f ($(du -h "$f" | cut -f1)); write it to a stick with:  iyakki-usb --install --erase /dev/sdX $f   (you run it; this script never writes a drive)"
}

cmd_qcow2() {
  local f="$OUT/iyakki-cloud-$VERSION-x86_64.img" q="$OUT/iyakki-cloud-$VERSION-x86_64.qcow2"
  [ -s "$f" ] || die "qcow2: no disk image at $f (run  iso  first)"
  command -v qemu-img >/dev/null || die "qemu-img is not installed"
  qemu-img convert -f raw -O qcow2 -c "$f" "$q"
  say "qcow2 $q: $(du -h "$q" | cut -f1) on disk, $(qemu-img info "$q" | sed -n 's/^virtual size: //p')"
  sign_out
}

cmd_clean() {
  rm -rf "$OUT"
  for t in $(docker images "$REPO" --format '{{.Repository}}:{{.Tag}}' 2>/dev/null); do docker rmi "$t" >/dev/null 2>&1 || true; done
  say "removed $OUT and the local $REPO tags (cargo and Buildroot outputs are kept)"
}

# ---------------------------------------------------------------------------------------------------------------- tests
PASS=0; FAIL=0
ok() { PASS=$((PASS+1)); echo "PASS  $*"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL  $*"; }
chk() { local what="$1"; shift; if "$@" >/dev/null 2>&1; then ok "$what"; else bad "$what"; fi; }

test_docker() {
  command -v docker >/dev/null && command -v curl >/dev/null || die "test docker: docker and curl are needed"
  local img="$REPO:$VERSION" port=18443 pw="correct horse battery staple"
  c="iyakki-cloud-test-$$"; vol="iyakki-cloud-test-vol-$$"; vol2="iyakki-cloud-test-vol2-$$"   # globals: the trap runs after this function returns
  docker image inspect "$img" >/dev/null 2>&1 || die "test docker: $img does not exist (run  docker  first)"
  local safe=(--read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 128 --memory 512m --tmpfs /tmp:rw,size=64m,mode=1777)
  trap 'docker rm -f "$c" "$c-adopt" "$c-cp" >/dev/null 2>&1; docker volume rm "$vol" "$vol2" >/dev/null 2>&1; true' RETURN
  [ "$(docker image inspect "$img" --format '{{.Config.User}}')" = "65532:65532" ] && ok "image runs as 65532:65532" || bad "image user"
  chk "image has a HEALTHCHECK" sh -c "docker image inspect $img --format '{{.Config.Healthcheck.Test}}' | grep -q healthcheck"
  chk "image has licence, version and source labels" sh -c "docker image inspect $img --format '{{json .Config.Labels}}' | grep -q 'Apache-2.0' && docker image inspect $img --format '{{json .Config.Labels}}' | grep -q $VERSION"
  docker create --name "$c-cp" "$img" >/dev/null 2>&1 && chk "image holds /licenses/sbom.cdx.json and the CA bundle" sh -c "docker cp $c-cp:/licenses/sbom.cdx.json - >/dev/null && docker cp $c-cp:/etc/ssl/certs/ca-certificates.crt - >/dev/null"; docker rm -f "$c-cp" >/dev/null 2>&1
  local leak; leak="$(docker save "$img" | tar -xO 2>/dev/null | grep -a -c -e 'BEGIN [A-Z ]*PRIVATE KEY' || true)"
  [ "${leak:-0}" = 0 ] && ok "no private key text in the saved image" || bad "private key text in the image ($leak)"
  # 1. an account without the pairing page: password on stdin
  docker volume create "$vol" >/dev/null
  printf '%s\n' "$pw" | docker run -i --rm "${safe[@]}" -v "$vol:/var/lib/iyakki" "$img" user add admin --role admin --password-stdin >"$LOGS/cloud-test-useradd.log" 2>&1 && ok "iyakki user add admin (password on stdin)" || { bad "user add: $(tail -2 "$LOGS/cloud-test-useradd.log")"; }
  # 2. the node under the safety flags
  docker run -d --name "$c" "${safe[@]}" -e IYAKKI_ADOPT=0 -v "$vol:/var/lib/iyakki" -p 127.0.0.1:$port:8443 "$img" serve >/dev/null || { bad "container did not start"; return; }
  local i st=""
  for i in $(seq 1 40); do st="$(docker inspect "$c" --format '{{.State.Health.Status}}' 2>/dev/null || true)"; [ "$st" = healthy ] && break; sleep 3; done
  [ "$st" = healthy ] && ok "docker HEALTHCHECK reports healthy (iyakki healthcheck)" || bad "health status: $st; log: $(docker logs "$c" 2>&1 | tail -3 | tr '\n' ' ')"
  [ "$(curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1:$port/healthz)" = 200 ] && ok "GET /healthz from the host: 200" || bad "healthz from host"
  local login; login="$(curl -sk -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d "{\"user\":\"admin\",\"password\":\"$pw\"}" https://127.0.0.1:$port/api/login)"
  [ "$login" = 200 ] && ok "sign-in with the password works (200)" || bad "sign-in answered $login"
  [ "$(curl -sk -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d '{"user":"admin","password":"wrong wrong wrong"}' https://127.0.0.1:$port/api/login)" = 401 ] && ok "a wrong password is refused (401)" || bad "wrong password not refused"
  # 3. write boundaries (non-root, read-only root)
  docker run --rm "${safe[@]}" -e IYAKKI_USERS=/etc/iyakki/users.json -v "$vol:/var/lib/iyakki" "$img" user add evil --role admin --password-stdin </dev/null >/dev/null 2>&1 && bad "wrote /etc/iyakki outside the volume" || ok "cannot write outside /var/lib/iyakki (/etc is read-only)"
  printf '%s\n' "second password here" | docker run -i --rm "${safe[@]}" -e IYAKKI_USERS=/var/lib/iyakki/users2.json -v "$vol:/var/lib/iyakki" "$img" user add bob --role user --password-stdin >/dev/null 2>&1 && ok "can write inside /var/lib/iyakki" || bad "cannot write to the volume"
  [ "$(docker exec "$c" /iyakki healthcheck >/dev/null 2>&1; echo $?)" = 0 ] && ok "docker exec iyakki healthcheck: exit 0" || bad "exec healthcheck"
  docker rm -f "$c" >/dev/null
  # 4. adopt mode on a fresh volume: the code is in the log, the page makes the administrator
  docker volume create "$vol2" >/dev/null
  docker run -d --name "$c-adopt" "${safe[@]}" -e IYAKKI_ADOPT=1 -v "$vol2:/var/lib/iyakki" -p 127.0.0.1:$port:8443 "$img" serve >/dev/null
  local code=""
  for i in $(seq 1 30); do code="$(docker logs "$c-adopt" 2>&1 | sed -n 's/.*IYAKKI-ADOPT node .*type the code \([A-Z0-9]\{4\}-[A-Z0-9]\{4\}\).*/\1/p' | head -1)"; [ -n "$code" ] && break; sleep 2; done
  [ -n "$code" ] && ok "adopt mode printed a pairing code in the container log" || { bad "no pairing code in the log: $(docker logs "$c-adopt" 2>&1 | tail -3 | tr '\n' ' ')"; return; }
  curl -sk https://127.0.0.1:$port/healthz | grep -q '"mode":"adopt"' && ok "/healthz answers in adopt mode" || bad "adopt healthz"
  curl -sk https://127.0.0.1:$port/ | grep -q "<form" && ! curl -sk https://127.0.0.1:$port/ | grep -q "$code" && ok "adopt page shows the form and never the code" || bad "adopt page"
  [ "$(curl -sk -o /dev/null -w '%{http_code}' -d 'code=AAAA-AAAA&name=admin&password=longenough123' https://127.0.0.1:$port/adopt)" = 403 ] && ok "a wrong code is refused (403)" || bad "wrong code accepted"
  [ "$(curl -sk -o /dev/null -w '%{http_code}' -d "code=$code&name=owner&password=an owner password 1" https://127.0.0.1:$port/adopt)" = 200 ] && ok "the right code creates the first administrator (200)" || bad "adopt with the code failed"
  sleep 4
  [ "$(curl -sk -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d '{"user":"owner","password":"an owner password 1"}' https://127.0.0.1:$port/api/login)" = 200 ] && ok "after adoption the normal front end signs the new administrator in" || bad "sign-in after adoption"
  [ "$(curl -sk -o /dev/null -w '%{http_code}' -d "code=$code&name=x&password=longenough123" https://127.0.0.1:$port/adopt)" != 200 ] && ok "the pairing page is gone for good" || bad "pairing page still adopts"
  # 5. kubernetes manifests: client-side dry run only (the local cluster is not touched)
  if command -v kubectl >/dev/null; then
    kubectl apply --dry-run=client --validate=false -f "$ROOT/dist/k3s-web.yaml" -o name >/dev/null 2>&1 && ok "kubectl apply --dry-run=client dist/k3s-web.yaml" || bad "dist/k3s-web.yaml does not parse"
    [ -f "$HERE/k3s-cloud.yaml" ] && { kubectl apply --dry-run=client --validate=false -f "$HERE/k3s-cloud.yaml" -o name >/dev/null 2>&1 && ok "kubectl dry run of dist/cloud/k3s-cloud.yaml" || bad "k3s-cloud.yaml does not parse"; }
  else echo "SKIP  kubectl not installed"; fi
}

# VM tests need the cloud ISO/disk and OVMF
OVMF_CODE=/usr/share/OVMF/OVMF_CODE_4M.fd; OVMF_VARS_T=/usr/share/OVMF/OVMF_VARS_4M.fd
qemu_run() { # NAME ARGS...: starts qemu in the background, port forward 18444 -> 8443; echoes the pid file
  local name="$1"; shift
  nice qemu-system-x86_64 -accel kvm -accel tcg -m 2048 -smp 2 -display none -no-reboot -pidfile "$OUT/qemu-$name.pid" -daemonize "$@" >"$LOGS/cloud-qemu-$name.log" 2>&1
}
qemu_stop() { [ -f "$OUT/qemu-$1.pid" ] && kill "$(cat "$OUT/qemu-$1.pid")" 2>/dev/null; rm -f "$OUT/qemu-$1.pid"; true; }
wait_http() { local i; for i in $(seq 1 "${2:-100}"); do curl -sk -m 3 "$1" >/dev/null 2>&1 && return 0; sleep 3; done; return 1; }

test_iso() {
  command -v qemu-system-x86_64 >/dev/null && [ -r "$OVMF_CODE" ] || die "test iso: QEMU and OVMF are needed"
  local img="$OUT/iyakki-cloud-$VERSION-x86_64.img" iso="$OUT/iyakki-cloud-$VERSION-x86_64.iso"
  [ -s "$img" ] || die "test iso: no disk image ($img); run  iso  first"
  local t="$OUT/vmtest"; rm -rf "$t"; mkdir -p "$t/cfg/tls"
  # 1. headless boot of the disk image: no display, no serial, one virtio NIC, no config: the node answers and offers the adopt page
  cp "$img" "$t/disk.img"; cp "$OVMF_VARS_T" "$t/vars.fd"
  trap 'qemu_stop a; qemu_stop b; qemu_stop c; qemu_stop d' RETURN
  qemu_run a -machine q35 -drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE -drive if=pflash,format=raw,file="$t/vars.fd" \
    -drive file="$t/disk.img",format=raw,if=virtio -netdev user,id=n0,hostfwd=tcp:127.0.0.1:18444-:8443 -device virtio-net-pci,netdev=n0
  if wait_http https://127.0.0.1:18444/healthz 100; then
    ok "disk image boots under OVMF with no display and no serial; /healthz answers"
    curl -sk https://127.0.0.1:18444/healthz | grep -q '"mode":"adopt"' && ok "no config and no account: the node is in adopt mode (Cloud mode came on by itself: variant cloud)" || bad "not in adopt mode: $(curl -sk https://127.0.0.1:18444/healthz)"
    curl -sk https://127.0.0.1:18444/ | grep -q "<form" && ok "the adopt page is served" || bad "adopt page missing"
    curl -sk https://127.0.0.1:18444/ | grep -qE '[A-Z0-9]{4}-[A-Z0-9]{4}' && bad "the page shows a code" || ok "the page does not show the pairing code"
  else bad "the disk image did not answer on /healthz in 5 minutes (log $LOGS/cloud-qemu-a.log)"; fi
  qemu_stop a
  # 2. the config drive: ISO 9660 labelled IYAKKI-CONFIG with a hostname and an account (the hash is made by `iyakki user add`, never a plain password)
  local hb; hb="$(host_bin)"; local pw="drive password 12345"
  printf '%s\n' "$pw" | IYAKKI_USERS="$t/users.json" "$hb" user add ops --role admin --password-stdin >/dev/null 2>&1
  local hash; hash="$(python3 -I -c "import json,sys;print(json.load(open(sys.argv[1]))['users'][0]['hash'])" "$t/users.json")"
  printf 'hostname = cfgnode\nadmin.name = ops\nadmin.hash = %s\n' "$hash" >"$t/cfg/iyakki-cloud.cfg"
  printf '{"schema":1,"items":[]}\n' >"$t/cfg/tenant-config.json"
  xorriso -as mkisofs -quiet -V IYAKKI-CONFIG -o "$t/config.iso" "$t/cfg" >/dev/null 2>&1 || die "xorriso could not make the config drive"
  cp "$img" "$t/disk2.img"; cp "$OVMF_VARS_T" "$t/vars2.fd"
  qemu_run b -machine q35 -drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE -drive if=pflash,format=raw,file="$t/vars2.fd" \
    -drive file="$t/disk2.img",format=raw,if=virtio -drive file="$t/config.iso",format=raw,if=virtio,readonly=on -netdev user,id=n0,hostfwd=tcp:127.0.0.1:18445-:8443 -device virtio-net-pci,netdev=n0
  if wait_http https://127.0.0.1:18445/healthz 100; then
    [ "$(curl -sk -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d "{\"user\":\"ops\",\"password\":\"$pw\"}" https://127.0.0.1:18445/api/login)" = 200 ] && ok "config drive read: the account from admin.hash signs in, no adopt mode" || bad "config drive account does not sign in (healthz: $(curl -sk https://127.0.0.1:18445/healthz))"
  else bad "node with a config drive did not answer (log $LOGS/cloud-qemu-b.log)"; fi
  qemu_stop b
  # 2b. the same config drive with a console, to read what the node said about it
  qemu_run d -machine q35 -kernel "$CLOUD_BR/images/bzImage" -initrd "$CLOUD_BR/images/rootfs.cpio.zst" -append "console=ttyS0 panic=-1" -serial "file:$t/console-d.log" \
    -drive file="$t/config.iso",format=raw,if=virtio,readonly=on -netdev user,id=n0,hostfwd=tcp:127.0.0.1:18447-:8443 -device virtio-net-pci,netdev=n0
  wait_http https://127.0.0.1:18447/healthz 60 || true
  tr -d '\r' <"$t/console-d.log" | grep -a "IYAKKI-CLOUD seed" | sed 's/^/      /'
  # (kernel messages can interleave with the line, so count the lines instead of matching a whole one)
  local n; n="$(tr -d '\r' <"$t/console-d.log" | grep -ac "cloud seed: wrote")"
  [ "${n:-0}" -ge 2 ] && ok "config drive read on the console: $n files written (hostname and account)" || bad "fewer than 2 'seed: wrote' lines on the console (see $t/console-d.log)"
  qemu_stop d
  # 3. direct kernel boot with a serial console: the code is printed on the console and the whole adoption works
  local base="$CLOUD_BR"
  if [ -s "$base/images/bzImage" ]; then
    qemu_run c -machine q35 -kernel "$base/images/bzImage" -initrd "$base/images/rootfs.cpio.zst" -append "console=ttyS0 iyakki.cloud=1 panic=-1" -serial "file:$t/console.log" \
      -netdev user,id=n0,hostfwd=tcp:127.0.0.1:18446-:8443 -device virtio-net-pci,netdev=n0
    wait_http https://127.0.0.1:18446/healthz 100 || true
    local code; code="$(tr -d '\r' <"$t/console.log" | sed -n 's/.*IYAKKI-ADOPT node .*type the code \([A-Z0-9]\{4\}-[A-Z0-9]\{4\}\).*/\1/p' | head -1)"
    [ -n "$code" ] && ok "the console shows the pairing code and the node name" || bad "no pairing code on the console (see $t/console.log)"
    [ -n "$code" ] && { [ "$(curl -sk -o /dev/null -w '%{http_code}' -d "code=$code&name=owner&password=an owner password 1" https://127.0.0.1:18446/adopt)" = 200 ] && ok "adoption through the page works in the VM" || bad "adoption failed"; }
    qemu_stop c
  else echo "SKIP  no kernel/initrd at $base/images for the console test"; fi
}

test_hw() { # checks that need no VM: the Mac mini 2014 (Macmini7,1) needs tg3, AHCI/PCIe storage, USB storage, no Wi-Fi wait
  local frag="$ROOT/dist/buildroot/board/iyakki/linux.fragment"
  for k in CONFIG_TIGON3 CONFIG_SATA_AHCI CONFIG_ATA_PIIX CONFIG_BLK_DEV_NVME CONFIG_USB_STORAGE CONFIG_USB_XHCI_HCD CONFIG_USB_EHCI_HCD CONFIG_EFI_STUB CONFIG_FB_EFI; do
    grep -q "^$k=[ym]" "$frag" && ok "kernel fragment sets $k" || bad "kernel fragment lacks $k"
  done
  local br="$CLOUD_BR"; [ -d "$br/target" ] || br="$ROOT/target/buildroot-mac"
  if ls "$br"/target/lib/modules/*/kernel/drivers/net/ethernet/broadcom/tg3.ko* >/dev/null 2>&1; then ok "tg3 module is in $(basename "$br")"; else bad "tg3.ko missing in $br"; fi
  grep -q "tigon/tg3" "$ROOT/dist/buildroot/board/iyakki/restricted-firmware.table" && ok "tg3 firmware is in the restricted-firmware table (fetched by the person, never shipped)" || bad "tg3 firmware not listed"
  chk "Wi-Fi never holds the boot (background item)" grep -q "wifi_in_background" "$ROOT/crates/iyakki-core/src/boot.rs"
  chk "self-install needs both kernel parameters (unit test)" grep -q "self_install_needs_both_parameters" "$ROOT/crates/iyakki-agent/src/cloudseed.rs"
}

cmd_test() {
  local what="${1:-all}"
  case "$what" in
    docker) test_docker ;;
    iso) test_iso ;;
    hw) test_hw ;;
    all) test_hw; test_docker; test_iso ;;
    *) die "test: docker, iso, hw or all" ;;
  esac
  echo "RESULT: $PASS passed, $FAIL failed"
  [ "$FAIL" = 0 ]
}

sub="${1:-}"; [ -n "$sub" ] && shift || true
case "$sub" in
  docker) cmd_docker "$@" ;;
  iso) cmd_iso "$@" ;;
  disk) cmd_disk "$@" ;;
  qcow2) cmd_qcow2 "$@" ;;
  all) cmd_docker "$@"; cmd_iso; cmd_disk; cmd_qcow2 ;;
  test) cmd_test "$@" ;;
  clean) cmd_clean ;;
  *) sed -n '2,11p' "$0"; exit 2 ;;
esac
