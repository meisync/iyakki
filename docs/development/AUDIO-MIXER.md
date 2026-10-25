# IYAKKI audio mixer

Status: core, one entry point (`audiosink.rs`) and the Settings -> Sound switch implemented and tested (`crates/iyakki-agent/src/mixer/`), **off by default** (`sound.mixer`), not yet run on a real T2 Mac.

## Why

On the MacBookPro16,2 (`AppleT2x4`, one playback subdevice, S24_LE, 4 channels, 48 kHz) ALSA `dmix` uses about 90 % of a core. So the T2 plan is `plug > softvol > plughw`: one program at a time, and the SDK answers `Busy` ([`SDK-API.md`](SDK-API.md), owner's answer 1). The owner asked for an in-house mixer instead of dmix.

## Process model

One mixer thread inside the agent (`Services`, process-wide instance `mixer::global()`, started on first use). It is the only writer to the card: it feeds `aplay -q -D <IYAKKI output> -t raw -f S16_LE -r 48000 -c 2 -` through a pipe (the same mechanism the radio and the cues use), so the existing `iyakki` PCM (`plug > softvol > plughw`) still carries the master volume and the Mute switch exactly once. No new daemon, no unix socket yet.

* Sources (`Mixer::open_source(kind, rate, channels) -> SourceWriter`) write plain interleaved PCM at any rate (8-192 kHz) and 1-8 channels into a 500 ms ring. The mixer converts rate and channel layout. `write_i16_wait` blocks when the ring is full, which paces a decoder to the speaker.
* **Decoders are sources.** The in-house MP3 decoder, and later the optional user-downloaded ffmpeg helper (`iyakki-audiodec`, a separate process writing s16le PCM plus rate and channels on stdout), feed a source with `SourceWriter::feed_reader(reader)` (a thread that reads a pipe, waits for room and finishes the source at end of data). The mixer never depends on ffmpeg.
* The engine (`engine.rs`) is pure: `mix_period` takes 20 ms (960 frames) from each source, resamples (`resample.rs`, windowed sinc, 16 zero crossings, Hann, tabulated), maps channels (mono to all, stereo to N like `softvol::fan_route`, N to stereo folds even/odd), applies per-source gain, ducking and master gain with linear ramps across the period, adds in f32 and saturates to 16-bit (`clamp`, counted as `clipped`).
* Pacing: the thread writes two periods ahead of a monotonic clock, one period per 20 ms. The pipe blocks as a backstop; after a long stall it does not burst. Clock drift against the card (tens of ppm) shows up as an occasional underrun; not measured on the T2.
* Latency: ducking and new sources reach the card after about 40 ms (lead) plus the ALSA buffer.

## CPU budget

Target under 3 % of one core while mixing four sources. The cost is the resampler (about 3 M multiply-adds per second for a 44.1 kHz stereo source) plus one add per sample. Mixing time is measured per period and published as `Stats.cpu_percent`. A test mixes four sources for 10 s of audio and bounds the time (5 % of real time release, 30 % debug; in a debug build it measured about 16 %, a release number has not been taken yet).

## Idle, silence and shutdown

No data from any source for `idle_ms` (1.5 s): the thread closes the pipe (aplay drains and exits), the card is released and can sleep, and other programs may open it. With no source at all the thread sleeps (about 4 wake-ups per second). The next source reopens the card; audio waits in its ring meanwhile, nothing is lost. A source that sends digital silence keeps the card open (documented, not special-cased). The T2 shutdown behaviour of `quiet.rs` is unchanged: it finds the `aplay` holder like any other and ends it, and the softvol goes to 0 first.

## Priorities and ducking

alarm > speech > notification > media > radio. While a source is sounding (audible within the last 300 ms hang, so speech does not pump between sentences), every source of LOWER priority is brought to a factor: alarm 0.10 (-20 dB), speech 0.25 (-12 dB), notification 0.40 (-8 dB); media and radio do not duck. Attack 30 ms, release 400 ms, both linear and interpolated per sample. A paused source does not duck. Per-source gain (0-4) is smoothed over a period; master gain is too.

## Opening, `Busy` and exclusive

| situation | answer |
|---|---|
| mixer off (`sound.mixer = off`, default) | `Disabled`: the caller keeps its old path; the SDK `audio.play` / `speech.say` say `Busy` as before |
| mixer on | a source is always accepted (no more `Busy` for a second program) unless: the card cannot be opened (`Busy{holder}` for 2 s, e.g. a program outside the mixer holds it), an exclusive source is open (only alarms pass), or 16 sources are open (a more important source evicts the oldest least important one; an equal one gets `Busy`) |
| exclusive source (`open_source_with(.., exclusive = true)`) | refused unless nothing else plays; others get `Busy{holder}`; at unity gain, same rate and channels, the 16-bit samples pass through unchanged |

Who may open: UI code, radio, cues, speech and media players are IYAKKI's own. SDK pods need the `audio` (or `speech`) capability from `app.json`, checked by the host before `open_audio_source`; per-pod kind is `media` (`audio.play` channel `music`), `notification` (`cue`) or `speech`. Pods can never open `alarm`.

## Volume and mute

The softvol control `IYAKKI` (or the hardware control) stays the master: the mixer plays into the same `iyakki` PCM. Where the selector has no softvol (players scale themselves), the mixer follows `Selector::gain()` as its master factor each period (0 when muted).

## Sound doctor and Devices

`Services::mixer_stats()` / `mixer::Mixer::stats()`: running, sink open, source count, CPU percent, underruns, clipped samples, ducking, last problem; `Stats::line()` is the one-line text. `audioout::Status.sharing` says `OneAtATime`, `Dmix` or `IyakkiMixer` (shown as "IYAKKI mixer" in the summary). The browser-silent doctor (`sounddoctor.rs`) is unchanged: it still reads `/proc/asound` and will see the mixer's `aplay` as a holder of the card.

## Setting

`sound.mixer` (`SoundSettings.mixer`, default false). `audioout::sharing_for(plan, mixer_on)`: dmix plans stay dmix; exclusive plans (T2 fixed-period, hardware-volume cards) become `IyakkiMixer`. The ALSA file is unchanged (`Plan::Soft`), so turning the setting off needs no rewrite. The Settings -> Sound switch is described below.

## One entry point: `audiosink.rs`

Apps do not know about the mixer (owner, 2026-10-24: "why does every app need to know"). The agent has one place that asks for the speaker:

* `audiosink::open_output(kind, rate, channels, label, play_cmd) -> Output` (streaming PCM; `Services::open_output` fills in the play command) and `audiosink::play_wav(kind, label, wav, play_cmd) -> Output` (a WAV), `audiosink::play_wav_blocking(..)` (a short cue, returns when played).
* `Output` has `write(&[i16])`, `set_volume(pct)`, `finish(max, keep_waiting)`, `stop()`, `is_playing()`, `failure()`.
* Mixer on (`sound.mixer`): a mixer source with the kind's priority; the master volume and mute are the mixer's, the output's own volume is the source gain. If the mixer cannot take the sound (a program outside it holds the card) the play-command path below is used.
* Mixer off: exactly the old behaviour, the play command of Settings -> Voice started with a pipe, the output and softvol plan of Settings -> Sound, `gain()`, one program at a time.
* `audiosink::mixing()` tells the SDK whether programs share the speaker: `speech.say` returns no `Busy` while it is true, priority and ducking decide; with it off the old `Busy` rules hold (`SDK-API.md`). `audio.play` has no real backend yet.
* A test (`audiosink_tests::only_the_entry_point_knows_about_the_mixer_setting`) fails if any agent file other than `audiosink.rs`, `audioout.rs` (which applies the setting) and `mixer/` reads `mixer::enabled`, `global_if_enabled`, `prefs.mixer` or `open_audio_source`.

Kinds: radio (`RadioPlayer`, kind Radio), reading aloud and SDK speech (`voice::Player::start`, kind Speech; `Player::start_as` for another kind), system sound cues (`syssounds::SystemOut`, kind Notification), audio files the built-in media path plays through `RadioPlayer` (`media.rs`, kind Radio today). Alarm is for future alarms; nothing sounds one yet.

Not on the entry point: the media player's ffmpeg/aplay pipeline (`player.rs`, an external process that writes to `aplay` itself), host speech engines that speak by themselves (espeak-ng, speech-dispatcher: they open the speaker themselves, `Player::from_speech`), the Settings test sounds and speaker tests (they test a chosen device on purpose), `quiet.rs`, and the browser.

## Settings -> Sound switch

Settings -> Sound has the row **Mix sounds from several apps (experimental)** (`M`, button Mix sounds, `Enter` on the row; administrators only, audited as `settings.sound.mixer`), saved in `SoundSettings.mixer` through `update_sound`, which calls `audioout::apply_prefs` and so `mixer::set_enabled`. Its explanation says the browser's sound is not mixed yet. While it is on, a status row shows running or idle, sources, CPU percent and underruns (`mixer::stats_if_enabled`). It applies from the next sound: a radio that already plays keeps its old player until its next restart.

* **Browser and video audio are NOT solved.** WPE/GStreamer's `alsasink` opens ALSA directly (`iyakkiweb` PCM from `softvol::web_pcm`) and holds the card while it plays, so with the mixer open the two compete. There is no PulseAudio/PipeWire here and an ALSA `ioplug` plugin would be C code (out of scope). `snd-aloop` is not in the kernel fragments (`dist/buildroot/board/iyakki/*.fragment` have no `CONFIG_SND_ALOOP`), so a loopback card needs a kernel change. Candidates: (1) enable `CONFIG_SND_ALOOP`, point `iyakkiweb` at `hw:Loopback,0` and let the mixer read `hw:Loopback,1` (clean, needs a rebuilt kernel, a capture reader, and measuring CPU on the T2); (2) an ALSA `file` PCM writing raw PCM to a FIFO that a reader thread turns into a source (no kernel change; the null-clocked slave makes pacing unproven); (3) a GStreamer `fdsink`/`appsink` in the helper (needs helper changes). Until one of these is built and measured, browser audio and the mixer are one-at-a-time as today.
