#!/bin/sh
# Runs as PID 1 inside the virtual-radio test VM (see hwsim-test.sh). Everything here happens in the VM only.
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
mount -t proc proc /proc; mount -t sysfs sys /sys; mount -t devtmpfs dev /dev; mount -t tmpfs tmp /tmp
SCEN=$(sed -n 's/.*scenario=\([a-z]*\).*/\1/p' /proc/cmdline)
AP_PASS="hwsim-test-pass-1"
case "$SCEN" in bad) CL_PASS="a-wrong-passphrase" ;; *) CL_PASS="$AP_PASS" ;; esac
SSID="IYAKKI-TEST"
echo "GUEST: scenario=$SCEN kernel=$(uname -r)"
finish() { echo "GUEST: done"; sync; reboot -f; sleep 5; }

# cfg80211, mac80211 and mac80211_hwsim are built into the test kernel (radios=2 comes from the command line)
sleep 1
echo "GUEST: interfaces:"; iw dev
PHY1=$(iw dev wlan1 info | sed -n 's/.*wiphy \([0-9]*\)/phy\1/p')
echo "GUEST: wlan1 is on $PHY1"

# the client radio lives in its own network namespace, so the ping really crosses the (virtual) air
unshare -n sleep 600 &
NSPID=$!
sleep 1
iw phy "$PHY1" set netns "$NSPID" || { echo "GUEST: FATAL netns move"; finish; }
NS="nsenter -t $NSPID -n"
$NS ip link set lo up
$NS ip link set wlan1 up
mkdir -p /tmp/wpa_ctrl
cat > /tmp/wpa.conf <<EOC
ctrl_interface=/tmp/wpa_ctrl
network={
  ssid="$SSID"
  psk="$CL_PASS"
  key_mgmt=WPA-PSK
  proto=RSN
  pairwise=CCMP
  group=CCMP
  scan_freq=2437
}
EOC
cat > /tmp/udhcpc.script <<'EOC'
#!/bin/sh
case "$1" in
  bound|renew) ip addr flush dev "$interface"; ip addr add "$ip/${mask:-24}" dev "$interface"; echo "GUEST: udhcpc bound $ip mask ${mask:-24} router $router dns $dns" ;;
esac
EOC
chmod +x /tmp/udhcpc.script

echo "GUEST: starting the access point"
/bin/hotspot_hwsim wlan0 "$SSID" "$AP_PASS" 6 40 > /tmp/ap.log 2>&1 &
APPID=$!
sleep 2
echo "GUEST: starting wpa_supplicant"
$NS wpa_supplicant -dd -t -Dnl80211 -i wlan1 -c /tmp/wpa.conf -f /tmp/wpa.log &
WPAPID=$!

if [ "$SCEN" = "bad" ]; then
  sleep 18
else
  n=0
  while [ $n -lt 25 ]; do
    grep -q "CTRL-EVENT-CONNECTED" /tmp/wpa.log 2>/dev/null && break
    sleep 1; n=$((n+1))
  done
  if grep -q "CTRL-EVENT-CONNECTED" /tmp/wpa.log; then
    echo "GUEST: supplicant connected after ${n}s"
    $NS udhcpc -i wlan1 -n -q -t 5 -T 2 -s /tmp/udhcpc.script 2>&1 | sed 's/^/GUEST: udhcpc: /'
    echo "GUEST: ip in namespace:"; $NS ip -4 addr show wlan1
    if $NS ping -c 3 -W 2 10.42.0.1 > /tmp/ping.log 2>&1; then echo "GUEST: PING-OK"; else echo "GUEST: PING-FAILED"; fi
    cat /tmp/ping.log | sed 's/^/GUEST: ping: /'
  else
    echo "GUEST: supplicant did NOT connect in 25 s"
  fi
fi

echo "GUEST: ==== station wlan1 link ===="; $NS iw dev wlan1 link
kill $WPAPID 2>/dev/null; sleep 1
kill $APPID 2>/dev/null; sleep 2
echo "GUEST: ==== AP LOG ===="; cat /tmp/ap.log
echo "GUEST: ==== SUPPLICANT LOG (key lines) ===="
grep -E "CTRL-EVENT|WPA: |EAPOL: |RSN: |State: |Associated|Authentication with|Trying to|nl80211: (Add|Set|Key|Event)|Key negotiation|4-Way|reason=" /tmp/wpa.log | cut -c1-220 | tail -n 120
echo "GUEST: ==== dmesg tail ===="; dmesg | tail -n 15
finish
