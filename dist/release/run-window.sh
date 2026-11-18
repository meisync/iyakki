#!/usr/bin/env bash
# Starts IYAKKI in a window. Your own drives are never shown (virtual disks only); add --real-disks to see them.
# Data lives in ~/.local/share/iyakki-dev (delete the folder to start fresh).
cd "$(dirname "$0")" && exec ./iyakki-desktop "$@"
