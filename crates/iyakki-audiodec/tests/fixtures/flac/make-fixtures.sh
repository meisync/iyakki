#!/bin/sh
# Regenerates the FLAC fixtures of the in-house decoder's tests (F-SDK-142). Needs ffmpeg (a build-machine tool, never shipped).
# Each .flac is 0.2 s at 8 kHz made from sines and a little noise; the .s16 next to it is what ffmpeg's own decoder gives (16-bit
# little-endian, interleaved), which the test compares sample for sample. Every file is under 20 KB.
set -eu
cd "$(dirname "$0")"
src2="aevalsrc=0.4*sin(2*PI*440*t)+0.1*sin(2*PI*1330*t)+0.02*(random(0)-0.5)|0.3*sin(2*PI*660*t)+0.02*(random(1)-0.5):s=8000:d=0.2"
src1="aevalsrc=0.4*sin(2*PI*440*t)+0.1*sin(2*PI*1330*t)+0.02*(random(0)-0.5):s=8000:d=0.2"
enc() { out=$1; shift; ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$SRC" -map_metadata -1 -bitexact "$@" "$out.flac"; }
SRC=$src2
enc stereo-indep -ch_mode indep -compression_level 8
enc stereo-ms -ch_mode mid_side -compression_level 8
enc stereo-ls -ch_mode left_side -compression_level 8
enc stereo-rs -ch_mode right_side -compression_level 8
enc stereo-fixed -lpc_type none -compression_level 4
enc stereo-rice2 -compression_level 12 -lpc_passes 3
SRC=$src1
enc mono-lpc -compression_level 8
enc mono-verbatim -compression_level 0 -lpc_type none
enc mono-24bit -sample_fmt s32 -bits_per_raw_sample 24
for f in *.flac; do
  n=${f%.flac}
  case $n in mono-24bit) ffmpeg -hide_banner -loglevel error -y -i "$f" -f s32le "$n.s32" ;; *) ffmpeg -hide_banner -loglevel error -y -i "$f" -f s16le "$n.s16" ;; esac
done
ls -l
