# IYAKKI sample media

Three small files so a fresh install (or a first boot on a new machine) has something to play before the person has
copied any of their own music or videos. They are installed read-only in `/usr/share/iyakki/samples/` on every image and
listed by MEDIA as the library folder "IYAKKI samples" (tagged `sample`).

| file | what it is | length | size |
|---|---|---|---|
| `Music/IYAKKI Sample Melody.mp3` | a slow pentatonic tune, synthesised; ID3: title "IYAKKI Sample Melody", artist "IYAKKI", album "IYAKKI Samples", genre "melody" | 26 s | 313,167 bytes |
| `Music/IYAKKI Sample Beats.mp3` | a kuthu-style drum loop (kick, thavil-like skin hits, snare, hats, bell), synthesised; same tags, genre "kuthu" | 20 s | 241,067 bytes |
| `Videos/IYAKKI Sample Video.webm` | 640x360, 15 fps, VP9 + Opus: a colour-bar test pattern with moving shapes and a two-note tone | 12 s | 259,489 bytes |

WebM with VP9 and Opus is what the `web` image's engine (WPE WebKit with GStreamer) plays without the optional H.264 codecs.

## How they were made

Entirely by `make-samples.py` (Python, numpy and ffmpeg: libmp3lame, libvpx-vp9, libopus): every sample is computed from
sine waves, noise and arithmetic, and every picture is drawn pixel by pixel. No recording, picture, sample library or
other third-party material is used. Run `python3 make-samples.py OUT_DIR` to make them again (the random seed is fixed).

## Licence

Dedicated to the public domain under CC0 1.0 Universal (https://creativecommons.org/publicdomain/zero/1.0/): copy,
change, sell or ship them with anything, no attribution needed. See `LICENSE`.
