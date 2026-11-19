#!/bin/sh
# Regenerates the tiny sound fixtures of the embedded player's tests (F-SDK-141). Needs ffmpeg (a build-machine tool, never shipped).
# Every file is a one second 440 Hz sine tone at half of full scale (the lavfi sine is 1/8 of it, so the volume filter raises it by 4); each stays under 100 KB. Run it from anywhere; it writes next to itself.
set -eu
cd "$(dirname "$0")"
src="sine=frequency=440:duration=1:sample_rate=22050"
ff() { ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$src" -af volume=4 -map_metadata -1 -bitexact "$@"; }
ff -ac 1 -c:a pcm_s16le tone-1s.wav
ff -ac 1 -c:a libmp3lame -b:a 32k -id3v2_version 0 -write_xing 0 tone-1s.mp3
ff -ac 1 -c:a libvorbis -q:a 0 tone-1s.ogg
ff -ac 1 -ar 48000 -c:a libopus -b:a 24k tone-1s.opus
ff -ac 1 -c:a flac tone-1s.flac
# video: Motion-JPEG in AVI (64 x 48, ten pictures a second, one second, with the tone as PCM), the same without sound, and MPEG-4 in AVI
# (the player refuses it with a sentence)
vid="testsrc=size=64x48:rate=10:duration=1"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$vid" -f lavfi -i "$src" -af volume=4 -map_metadata -1 -c:v mjpeg -q:v 5 -pix_fmt yuvj420p -c:a pcm_s16le -ac 1 -f avi clip-1s.avi
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$vid" -map_metadata -1 -c:v mjpeg -q:v 5 -pix_fmt yuvj420p -f avi clip-silent.avi
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$vid" -map_metadata -1 -c:v mpeg4 -f avi clip-mpeg4.avi
ls -l
