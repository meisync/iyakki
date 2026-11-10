# The "My IYAKKI" wake phrase: recording takes, training, evaluating

Crate `crates/iyakki-wake` (step 4 of [`MY-IYAKKI-PLAN.md`](MY-IYAKKI-PLAN.md)). Our own code and data only: no commercial keyword engine, no pre-trained third-party model. Everything below runs on the dev PC under `~/iyakki-work` (never `/tmp`).

**Honest state:** the pipeline works end to end, but the only model trained so far used the built-in formant stub (not real speech); on synthetic held-out data it missed 94 % of the phrase at the useful thresholds. Its accuracy on real voices and rooms is unknown until real takes are recorded and evaluated. The spotter IS now connected to the microphone (section 5), strictly opt-in and off by default, but that wiring has only been tested with a fake microphone and a fake spotter: it has never listened to a real voice or run on the real sound card.

## 1. Record about 50 real takes (owner)

Use any recorder that writes WAV (the Voice tile's RECORD tab saves 16-bit mono 16 kHz WAV in your Recordings folder). Other rates and stereo are converted on reading.

Folder `~/iyakki-work/wake-data/` with three sub-folders, WAV files only:

| Folder | What | Name | How many |
|---|---|---|---|
| `positive/` | you saying "My IYAKKI" once, then stop | `real_<speaker>_<nn>.wav`, e.g. `real_amy_01.wav` | about 50: normal, quiet, fast, slow, far from the microphone, near a fan, TV on |
| `negative/` | things that must not wake it: "my yaki", "hey ikki", "my taxi", ordinary sentences, TV or radio speech, your own talking | `real_<speaker>_neg_<nn>.wav` | 50 or more, 2 to 10 s each |
| `background/` | the room with nobody talking to it: fan, kitchen, TV, music, street | `real_bg_<nn>.wav` | 10 minutes or more in total; also useful as the test of false accepts per hour |

Rules: names starting `real_` mark real recordings (the weights header then says real voices were included). The part before the second underscore is the speaker; validation holds out whole speakers. Keep a few minutes of room sound that you do NOT put in the training folder: copy it to `~/iyakki-work/wake-eval/background/` for the final test, and 10 extra takes to `wake-eval/positives/`.

## 2. Generate synthetic data (optional, already scripted)

```
cargo run --release -p iyakki-wake --bin iyakki-wake-gen -- --out ~/iyakki-work/wake-data
```
Without `--tts-command` this uses the formant stub and prints that it is NOT real speech. With a local speech tool: `--tts-command "mytool --voice {voice} --rate {speed} --out {out} {text}"` (the tool writes a WAV to `{out}`; none is installed on the build PC, so this path is untested against a real engine). The generator adds sounds-alike phrases, general speech, TV-like mixes and noise.

## 3. Train

```
cargo run --release -p iyakki-wake --bin iyakki-wake-train -- --data ~/iyakki-work/wake-data --out ~/iyakki-work/wake/my-iyakki-wake.iywk --epochs 20 --seed 1
```
Deterministic for a seed (any thread count). Augmentation per example: gain, time shift, noise at 0 to 25 dB SNR, a synthetic exponential-decay room, speed 0.9 to 1.1, fragments of the phrase as negatives. The weights file has a versioned header (data kind: synthetic or real included, suggested threshold) and a checksum.

## 4. Evaluate

```
cargo run --release -p iyakki-wake --bin iyakki-wake-eval -- --weights ~/iyakki-work/wake/my-iyakki-wake.iywk --positives ~/iyakki-work/wake-eval/positives --background ~/iyakki-work/wake-eval/background
```
Prints the miss rate and the false accepts per hour for thresholds 0.50 to 0.99 through the same streaming spotter the device would run. Choose the lowest threshold with an acceptable false accept rate (a good target is under 1 per hour in your room). `--bench` prints the CPU cost.

## 5. Use (opt-in)

Copy the trained file to the data folder as `voice/my-iyakki-wake.iywk`. Without a usable file the switch explains that and stays off (Settings and the chat tool show the same sentence).

Turning it on (all three need My IYAKKI itself to be on first):

* **Settings -> Voice**, key `M` (administrators): "Wake phrase" row; `N` cycles the sensitivity (low, normal, high).
* **Chat**: `voice_assistant_set` with `wake` true or false and `wake_sensitivity` low, normal or high. It is on the hard-ask list: an approval card every time, a tap or key approves, a spoken yes never does. `voice_wake_status` (free) says whether it is opted in, what the model was trained on and whether anything listens.
* **Developer mode**: `iyakki dev config voice.my_iyakki_wake true` and `voice.my_iyakki_wake_sensitivity 0|1|2` (saves the choice; the listener still checks the model).

What happens when it is on:

* The shell owns the listener (`iyakki_agent::wakelisten`, started by `shell_wake.rs`), so it hears on every screen, not only in the Voice tile. It exists only while a person is signed in, may use voice, is on this machine's own microphone (a browser session's microphone is not used) and the saved settings say wake on, My IYAKKI on, not muted, with a usable model file.
* The microphone is opened through the one shared path of the Voice tile (`voiceassist_ports::open_capture`, the saved microphone choice and the named-format PCM of `micpcm`); the wake code opens no device itself. Audio is read in 100 ms frames, handed to the spotter and forgotten; the only sound that exists is the spotter's three-second ring in memory. Nothing is stored or sent.
* **Indicator**: a red sign, bottom left of every screen, "Microphone open: listening for My IYAKKI. Click to switch off", drawn exactly while the microphone is open to the spotter (a click switches the wake phrase off). It is stacked with the camera and recording signs and cannot be hidden by a tile.
* Muting, turning My IYAKKI off, opting out, signing out or locking the screen closes the microphone at once and wipes the ring. While the Voice tile is open it has the microphone (push to talk) and the listener lets go of it.
* A **detection** closes the listener's microphone, writes one audit line (`voice.wake heard`, no sound, no text) and opens the Voice tile on the Assistant tab with Talk tapped: the existing session, with its pause rule and follow-up window unchanged. The listener stays quiet until the session had the microphone (or 30 s).
* **Sensitivity** moves the model's suggested threshold: low +0.07, normal as trained, high -0.10 (clamped to 0.50 to 0.99). Changing it restarts the spotter from nothing.
* **Audit log**: `voice.assistant wake-on` / `wake-off` / `wake-sensitivity` from Settings, the chat tool and the sign's click; never audio.

**Not verified:** nothing here has run on a real microphone, a real voice, TV speech or room noise; the model is the synthetic one; false wakes per hour and misses in a real room are unknown. Not built: the second-stage Whisper-tiny confirm, a chime, miss and false-wake counters in the tile, the top-bar glyph (the red sign is the indicator), wake in browser sessions (the browser's microphone has its own consent and is not used), a wake while the Voice tile is already open.

## Rules the code keeps

Off by default and opt-in; never runs while off or muted (samples dropped on arrival, buffers wiped); the indicator is true exactly while the microphone is open to the spotter; audio lives only in a 3 s in-memory ring; no file, network or process call in the runtime modules (a test reads the sources); the second stage (Whisper-tiny on the last two seconds) is described in the plan and not built. Tests of the wiring: `iyakki-agent` `wakelisten::tests` (fake microphone, fake spotter), `voiceassisttools::tests`, `iyakki-shell` `wake_tests`.
