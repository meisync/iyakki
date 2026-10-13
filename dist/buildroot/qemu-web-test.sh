#!/bin/bash
# Boot the optional "web" image variant in QEMU and open pages with the embedded full web engine (the WPE WebKit helper), saving screenshots.
# See qemu-web-test.py (this is a thin wrapper):   ./qemu-web-test.sh [build-dir] [out-dir]
#   QEMU="sudo qemu-system-x86_64"  URLS="..."  WAIT=40  SSH_KEY=~/.ssh/afos_diag.pub
exec python3 "$(dirname "$0")/qemu-web-test.py" "$@"
