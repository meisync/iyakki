#!/bin/sh
# Regenerates the Opus test fixtures of crates/iyakki-audiodec (F-RADIO-15) with ffmpeg (a reference tool only; never linked or shipped).
# For each case: NAME.opus (made by the libopus encoder inside ffmpeg from a synthetic signal) and NAME.ref.s16 (the same file decoded by
# libopus inside ffmpeg: 48 kHz, signed 16-bit little-endian, interleaved, the pre-skip already removed). The tests read both files;
# ffmpeg is not needed to run them. Total size must stay under 200 KB.
set -eu
cd "$(dirname "$0")"
mk() { # name seconds channels lavfi-source encoder-args...
  name=$1; secs=$2; ch=$3; src=$4; shift 4
  ffmpeg -v error -y -f lavfi -i "$src" -t "$secs" -ar 48000 -ac "$ch" -c:a libopus "$@" "$name.opus"
  ffmpeg -v error -y -c:a libopus -i "$name.opus" -f s16le -acodec pcm_s16le -ar 48000 "$name.ref.s16"
}
# a tone with a slow sweep over noise: tonal and noisy bands together
MIX='aevalsrc=0.30*sin(2*PI*(300*t+700*t*t))+0.20*sin(2*PI*1500*t)+0.08*(random(0)-0.5)|0.25*sin(2*PI*(500*t+400*t*t))+0.15*sin(2*PI*3100*t)+0.08*(random(1)-0.5):s=48000:d=1'
MONO='aevalsrc=0.30*sin(2*PI*(300*t+700*t*t))+0.20*sin(2*PI*1500*t)+0.10*sin(2*PI*5200*t)+0.05*(random(0)-0.5):s=48000:d=1'
# clicks: short blocks (transient frames)
CLICKS='aevalsrc=if(lt(mod(t\,0.05)\,0.002)\,0.8*(random(0)-0.5)\,0.01*sin(2*PI*220*t)):s=48000:d=1'
mk celt-mono-20ms  0.30 1 "$MONO"   -application lowdelay -frame_duration 20 -b:a 48k
mk celt-stereo-20ms 0.10 2 "$MIX"   -application lowdelay -frame_duration 20 -b:a 96k
mk celt-mono-10ms-wb 0.10 1 "$MONO" -application lowdelay -frame_duration 10 -b:a 32k -cutoff 8000
mk celt-stereo-5ms 0.08 2 "$MIX"    -application lowdelay -frame_duration 5 -b:a 128k
mk celt-mono-2p5ms 0.06 1 "$MONO"   -application lowdelay -frame_duration 2.5 -b:a 64k
mk celt-stereo-clicks 0.10 2 "$CLICKS" -application lowdelay -frame_duration 20 -b:a 96k
# speech-like source for SILK: a pitched buzz through a few formants, with pauses (voiced, unvoiced and silent frames)
SPEECH='aevalsrc=0.5*sin(2*PI*(120+30*sin(2*PI*1.5*t))*t)*(0.5+0.5*sin(2*PI*2*t))+0.15*sin(2*PI*700*t)*sin(2*PI*3*t)+0.04*(random(0)-0.5)*(0.5+0.5*sin(2*PI*4*t)):s=48000:d=1'
SPEECH2='aevalsrc=0.45*sin(2*PI*(110+25*sin(2*PI*1.2*t))*t)*(0.5+0.5*sin(2*PI*2*t))+0.1*sin(2*PI*900*t)|0.35*sin(2*PI*(160+20*sin(2*PI*1.7*t))*t)*(0.5+0.5*sin(2*PI*2.5*t))+0.1*sin(2*PI*1300*t):s=48000:d=1'
mk silk-mono-wb-20ms 0.16 1 "$SPEECH"  -application voip -frame_duration 20 -b:a 16k -cutoff 8000
mk silk-mono-nb-20ms 0.12 1 "$SPEECH"  -application voip -frame_duration 20 -b:a 12k -cutoff 4000
mk silk-mono-mb-20ms 0.12 1 "$SPEECH"  -application voip -frame_duration 20 -b:a 14k -cutoff 6000
mk silk-mono-wb-10ms 0.10 1 "$SPEECH"  -application voip -frame_duration 10 -b:a 20k -cutoff 8000
mk silk-mono-wb-60ms 0.18 1 "$SPEECH"  -application voip -frame_duration 60 -b:a 16k -cutoff 8000
mk silk-stereo-wb-20ms 0.10 2 "$SPEECH2" -application voip -frame_duration 20 -b:a 24k -cutoff 8000
# hybrid (speech layer below 8 kHz plus CELT above): no cutoff or a super-wide one at a low rate in voip mode
mk hybrid-mono-fb-20ms 0.20 1 "$SPEECH"  -application voip -frame_duration 20 -b:a 20k
mk hybrid-mono-swb-10ms 0.12 1 "$SPEECH" -application voip -frame_duration 10 -b:a 24k -cutoff 12000
mk hybrid-stereo-fb-20ms 0.12 2 "$SPEECH2" -application voip -frame_duration 20 -b:a 32k
# in-band forward error correction: the packets carry a second, coarser copy of the previous frame (read and skipped by the decoder)
mk silk-mono-wb-fec 0.20 1 "$SPEECH"  -application voip -frame_duration 20 -b:a 20k -cutoff 8000 -fec 1 -packet_loss 25
mk silk-stereo-wb-fec 0.12 2 "$SPEECH2" -application voip -frame_duration 20 -b:a 32k -cutoff 8000 -fec 1 -packet_loss 25
# a stream that changes mode: tone music, then speech, then clicks (audio application lets the encoder choose)
SWITCH='aevalsrc=if(lt(t\,0.15)\,0.3*sin(2*PI*440*t)+0.2*sin(2*PI*1320*t)+0.1*sin(2*PI*5300*t)\,if(lt(t\,0.3)\,0.5*sin(2*PI*(120+30*sin(2*PI*1.5*t))*t)*(0.5+0.5*sin(2*PI*8*t))+0.15*sin(2*PI*700*t)\,0.3*sin(2*PI*660*t)+0.15*(random(0)-0.5))):s=48000:d=1'
mk switch-mono-audio 0.45 1 "$SWITCH" -application audio -frame_duration 20 -b:a 24k
# the same clicks in mono (short blocks through the single-channel band code)
mk celt-mono-clicks 0.15 1 "$CLICKS" -application lowdelay -frame_duration 20 -b:a 48k
