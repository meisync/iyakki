#!/bin/sh
# Outbound-only Linux apps network on a real kernel (F-LINAPP-13): namespace, veth pair, table, forwarding and teardown.
#   dist/qemu/sandbox-net-test.sh [build-output-dir]
# Needs: the built image, qemu-system-x86_64 (KVM recommended), python3 (listeners on the QEMU host, and the result reader).
# The guest (iyakki.selftest=sandboxnet) joins a throw-away network namespace to the machine as the program does for a sandbox, then:
#   allowed destination 10.0.2.2:PORT_A (a listener on this host, through QEMU's user network)  -> must connect
#   10.0.2.2:PORT_B (a listener too, but not on the allow-list)                                  -> must be refused
#   the machine's own address on the pair (a listener inside the guest)                          -> must be refused
# and afterwards no veth, no table, and the forwarding switch as it was. PASS = the guest printed RESULT PASS with every check PASS.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3
PA="${PORT_A:-18080}"; PB="${PORT_B:-18081}"
for p in "$PA" "$PB"; do
  python3 -c "import socket,sys,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1', $p)); s.listen(16)
while True:
    c,_=s.accept(); c.close()" &
  CLEANUP="${CLEANUP:+$CLEANUP; }kill $! 2>/dev/null || true"
done
sleep 1
python3 -c "import socket; [socket.create_connection(('127.0.0.1', p), 2).close() for p in ($PA, $PB)]" || fail "the two listeners on this host are not up (ports $PA, $PB busy?)"
echo "=== guest: iyakki.selftest=sandboxnet (allowed $PA, not listed $PB)"
boot "$WORK/net.log" "iyakki.selftest=sandboxnet iyakki.testnet=$PA,$PB" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
tr -d '\r' < "$WORK/net.log" | grep -a "IYAKKI-SELFTEST sandboxnet" | sed 's/^/  /' || true
result "$WORK/net.log" sandboxnet
echo "PASS (sandboxnet)"
