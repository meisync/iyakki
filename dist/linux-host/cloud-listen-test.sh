#!/bin/bash
# F-CLOUD-11 (M80): `iyakki cloud listen` on the real ports 80 and 443 with the real firewall gate, in a private network namespace.
# Run as: sudo -n env IYAKKI_IN_SANDBOX=1 unshare -nm bash dist/linux-host/cloud-listen-test.sh PATH/TO/iyakki
# Everything is in scratch folders; nothing outside the namespace is touched.
set -u
[ "${IYAKKI_IN_SANDBOX:-}" = 1 ] || { echo refuse; exit 2; }
mount --make-rprivate /; mount -t tmpfs tmpfs /run; mount -t tmpfs tmpfs /var/lib   # the firewall saves its settings under /var/lib/iyakki: keep that in the namespace
BIN=$1; W=$(mktemp -d /tmp/m80cl.XXXX)
ip link set lo up
mkdir $W/www; echo backend-ok > $W/www/index.html
(cd $W/www && python3 -m http.server 18080 --bind 127.0.0.1 >$W/be.log 2>&1 &) 
$BIN cloud cert self-signed secure.test --out $W/certs 2>&1 | tail -2
cp $W/certs/secure.test.crt $W/certs/securecert.crt; cp $W/certs/secure.test.key $W/certs/securecert.key
cat > $W/routes.json <<J
{"routes":[
{"host":"app.test","path_prefix":"/","target":{"local":{"port":18080}},"scheme":"http","access":"public"},
{"host":"secure.test","path_prefix":"/","target":{"local":{"port":18080}},"scheme":"https","access":"public","cert":{"vault":"securecert"}}]}
J
$BIN cloud listen --routes $W/routes.json --cert-dir $W/certs --route-tenant app.test=t1 --route-tenant secure.test=t1 >$W/l.log 2>&1 &
LP=$!
sleep 3; cat $W/l.log
F=0; ok(){ echo "ok: $1"; }; bad(){ echo "FAIL: $1"; F=1; }
curl -s -m 5 --resolve app.test:80:127.0.0.1 http://app.test/ | grep -q backend-ok && ok "port 80 routes app.test to the backend" || bad "port 80"
curl -s -m 5 -k -i --resolve secure.test:443:127.0.0.1 https://secure.test/ | tee $W/h2 | grep -q backend-ok && ok "port 443 serves the certificate and routes secure.test" || bad "port 443"
head -1 $W/h2 | grep -q 'HTTP/2 200' && ok "curl negotiated HTTP/2 by ALPN" || bad "h2"
curl -s -m 5 -o /dev/null -w '%{http_code}' -H 'Host: nope.test' http://127.0.0.1/ | grep -q 404 && ok "unknown host gets 404" || bad "404"
ss -ltn | grep -q '0.0.0.0:80 ' && ss -ltn | grep -q '0.0.0.0:443 ' && ok "listens on 0.0.0.0:80 and :443" || bad "sockets"
nft list ruleset | grep -q 'tcp dport 80 ' && nft list ruleset | grep -q 'tcp dport 443 ' && ok "the real gate opened 80 and 443 for the local network only" || bad "gate rules"
nft list ruleset | grep 'dport 443 ' | grep -q 'saddr 192.168.0.0/16' && ok "the 443 rule names local source ranges" || bad "gate sources"
has443(){ nft list ruleset | grep -q 'tcp dport 443 '; }
has80(){ nft list ruleset | grep -q 'tcp dport 80 '; }
kill -TERM $LP; wait $LP 2>/dev/null
has443 || has80 && bad "SIGTERM left a gate rule behind" || ok "SIGTERM stopped the listener and removed its 80 and 443 rules"
$BIN cloud listen --routes $W/routes.json --cert-dir $W/certs --route-tenant app.test=t1 --route-tenant secure.test=t1 >$W/l2.log 2>&1 &
LP=$!; sleep 3
has443 && has80 && ok "restarted: both rules are there" || bad "restart rules"
kill -KILL $LP; wait $LP 2>/dev/null
has443 && ok "SIGKILL leaves the rules (nothing can run)" || bad "SIGKILL test premise"
# the next start has only the http route: the stale 443 rule must be dropped, the 80 rule kept
cat > $W/routes2.json <<J
{"routes":[{"host":"app.test","path_prefix":"/","target":{"local":{"port":18080}},"scheme":"http","access":"public"}]}
J
$BIN cloud listen --routes $W/routes2.json --route-tenant app.test=t1 >$W/l3.log 2>&1 &
LP=$!; sleep 3
has443 && bad "the stale 443 rule survived the restart" || ok "the restart dropped the stale 443 rule"
has80 && ok "and kept the 80 rule it needs" || bad "80 rule missing"
curl -s -m 5 --resolve app.test:80:127.0.0.1 http://app.test/ | grep -q backend-ok && ok "port 80 still served after the restart" || bad "port 80 after restart"
kill -INT $LP; wait $LP 2>/dev/null
has80 && bad "SIGINT left the 80 rule" || ok "SIGINT removed the 80 rule"
pkill -f "http.server 18080"
rm -rf $W
[ $F = 0 ] && echo PASS || { echo FAILED; exit 1; }
