#!/bin/bash
# Runs inside `unshare -rn` (a private network namespace; the host's network is never touched). Arguments: the fw_demo example
# and a file for the blocked-log output. Makes a veth pair to a second namespace, applies the sample ruleset and tries to connect.
D=$1; W=$2
ip link set lo up
unshare -n sleep 40 >/dev/null 2>&1 & P=$!
sleep 0.5
ip link add ex0 type veth peer name ex1
ip link set ex1 netns $P
ip addr add 203.0.113.1/24 dev ex0; ip link set ex0 up
nsenter -t $P -n sh -c 'ip link set lo up; ip addr add 203.0.113.9/24 dev ex1; ip link set ex1 up'
python3 -c "
import socket,threading,time
for p in (8000,9000):
    s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',p)); s.listen(5)
time.sleep(12)" >/dev/null 2>&1 &
sleep 0.5
try() { nsenter -t $P -n python3 -c "
import socket
s=socket.socket(); s.settimeout(1.5)
try: s.connect(('203.0.113.1',$1)); print('$1 open')
except Exception as e: print('$1 blocked', type(e).__name__)"; }
$D watch > $W 2>&1 &
sleep 0.5
echo before; try 8000
$D apply
echo after; try 8000; try 9000; try 9000
$D counters
sleep 5; cat $W
kill $P 2>/dev/null
