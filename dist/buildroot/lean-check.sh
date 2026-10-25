#!/bin/sh
# What the lean boot experiment did on the machine it runs on (docs/BOOT-SPEED.md). Read-only; BusyBox sh; over SSH on the entry
# "IYAKKI - lean boot + remote diagnostics (experimental)":
#
#   ssh root@HOST 'sh -s' < dist/buildroot/lean-check.sh
#
# Times are the KERNEL clock (seconds since the kernel started). What happens before it (firmware, GRUB reading the kernel and the
# initial RAM disk from the stick) is invisible here: it is the stopwatch time from Enter on the menu entry to the first picture
# minus the kernel clock at that picture (see the owner instructions in the document).
dm() { dmesg 2>/dev/null; }
t() { # first kernel log line matching $1 -> its time, or -
  l="$(dm | grep -m1 -- "$1")"
  [ -n "$l" ] && printf '%s' "$l" | sed -n 's/^[^0-9]*\[ *\([0-9.]*\)\].*/\1/p' || echo "-"
}
row() { printf '  %-40s %8s s\n' "$1" "$(t "$2")"; }
echo "== lean boot check =="
echo "kernel  $(uname -r)   machine $(cat /sys/class/dmi/id/product_name 2>/dev/null)   up now $(cut -d' ' -f1 /proc/uptime) s"
echo "-- which entry was used --"
echo "  cmdline: $(cat /proc/cmdline)"
if grep -q 'iyakki.ssh=1' /proc/cmdline; then echo "  remote diagnostics entry: yes"; else echo "  remote diagnostics entry: no"; fi
echo "-- is the root the lean overlay? --"
if dm | grep -q 'stage1: started'; then echo "  first stage ran: YES (lean boot)"; else echo "  first stage ran: NO (this is the normal full boot, or the log has rotated)"; fi
grep -E ' / (overlay|rootfs|ramfs|tmpfs) ' /proc/mounts | sed 's/^/  root mount: /'
grep -E ' (/var/lib/iyakki|/\.stage1/boot) ' /proc/mounts | sed 's/^/  volume: /'
r="$(sed -n 's/.*lowerdir=\([^,]*\).*/\1/p' /proc/mounts | head -n1)"; [ -n "$r" ] && echo "  overlay lower layer: $r (the tree on the data volume)"
ls "/var/lib/iyakki/iyakki/root" 2>/dev/null | sed 's/^/  roots on the data volume: /'
echo "  current pointer: $(cat /var/lib/iyakki/iyakki/root/current 2>/dev/null | head -n1)"
echo "-- first stage's own log lines (kernel clock in front) --"
dm | grep 'stage1:' | sed 's/^/  /'
echo "-- timeline (kernel clock) --"
row "kernel start"                         "Linux version"
row "initramfs unpacked"                   "Freeing initrd memory"
row "PID 1 (/init) starts"                 "Run /init as init process"
row "first stage starts looking"           "stage1: started"
row "volumes found"                        "stage1: volumes found"
row "root mounted"                         "stage1: root iyakki/root"
row "switching root"                       "stage1: switching root"
row "IYAKKI data volume (skip logic)"      "by the first stage"
row "splash: first drawing"                "splash: "
row "supervisor up"                        "supervisor up"
row "boot summary (to login)"              "boot (to login)"
row "sign-in screen: first picture"        "shell: first picture on screen"
row "boot summary (after login)"           "boot (after login)"
echo "-- size of what GRUB read --"
l="$(dm | grep -m1 'Freeing initrd memory')"; [ -n "$l" ] && echo "  initrd: $(printf '%s' "$l" | sed -n 's/.*memory: \([0-9]*\)K.*/\1/p' | awk '{printf "%.1f MB", $1/1024}') unpacked (lean is about 1 MB; the normal one about 660 MB)"
echo "-- memory --"
grep -E '^(MemTotal|MemFree|MemAvailable|Shmem|SwapTotal|SwapFree):' /proc/meminfo | sed 's/^/  /'
echo "-- zram and the other early-boot services --"
dm | grep -i "zram0\|Adding .*swap\|iyakki.*\(storage:\|data:\|clock\|k3s\|hotplug\|home:\|zram\|swap\)" | head -n 14 | sed "s/^/  /"
echo "-- problems in the kernel log --"
dm | grep -i 'fail\|error\|cannot\|denied\|oops\|bug:' | grep -vi 'firmware\|ACPI\|iwlwifi\|bluetooth' | head -n 12 | sed 's/^/  /'
echo "-- gaps over 1 s between kernel log lines --"
dm | awk '{ if (match($0, /\[ *[0-9.]+\]/)) { v=substr($0, RSTART+1, RLENGTH-2)+0; if (v-p > 1.0) printf "  +%.1f s before: %s\n", v-p, substr($0, 1, 110); p=v } }'
echo "== end =="
