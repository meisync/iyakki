# The "My IYAKKI" wake phrase: recording takes, training, evaluating

Crate `crates/iyakki-wake` (step 4 of [`MY-IYAKKI-PLAN.md`](MY-IYAKKI-PLAN.md)). Our own code and data only: no commercial keyword engine, no pre-trained third-party model. Everything below runs on the dev PC under `~/iyakki-work` (never `/tmp`).

**Honest state:** the pipeline works end to end, but the only model trained so far used the built-in formant stub (not real speech). Its accuracy on real voices and rooms is unknown until real takes are recorded and evaluated. The spotter is not connected to any microphone in this version.

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

## 5. Use (later)

Copy the file to the data folder as `voice/my-iyakki-wake.iywk`; `voice_wake_status` reports it. Opting in (`voice_assistant_set` with `wake`, or `iyakki dev config voice.my_iyakki_wake true`) only saves the choice until the microphone wiring is built.

## Rules the code keeps

Off by default and opt-in; never runs while off or muted (samples dropped on arrival, buffers wiped); the indicator is true exactly while the microphone is open to the spotter; audio lives only in a 3 s in-memory ring; no file, network or process call in the runtime modules (a test reads the sources); the second stage (Whisper-tiny on the last two seconds) is described in the plan and not built.
