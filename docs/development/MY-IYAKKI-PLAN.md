# "My IYAKKI": a personal voice assistant joined to the chat agent (plan; push-to-talk slice built)

Status: **first slice built 2026-11-08 (build order steps 1 to 3, push to talk; see "First slice: what exists" below)**; the rest is still a plan. Plan written 2026-11-07 after the owner asked for it ("plan for 'my iyakki' to start a personal voice assistant, to integrate with the chat agent"). Nothing here is built; the parts it stands on are listed under "What exists". Related: [`../VOICE.md`](../VOICE.md), [`../ASSISTANT-ROBUSTNESS.md`](../ASSISTANT-ROBUSTNESS.md), [`FLOW-AUTOPILOT-PLAN.md`](FLOW-AUTOPILOT-PLAN.md), [`AGENT-WAIT-PLAN.md`](AGENT-WAIT-PLAN.md), [`TILE-ABI.md`](TILE-ABI.md).

## What it is

Say **"My IYAKKI"** (or press a key, or tap the mic) and talk; the chat agent answers **aloud**, can use its tools (with the same approvals as in the chat), and listens again for a follow-up. It runs on the device with local speech models by default, shows clearly when it is listening, and never keeps audio.

## What exists (it stands on these)

| Piece | Where | State |
|---|---|---|
| Speech to text (Lemonade Whisper, or a command), consent per destination | `voice.rs`, `voicedesk.rs`, `voicepack.rs` | working |
| Text to speech (Kokoro, host engines, browser voice), highlight while reading | `voice.rs`, `speechtime.rs`, `hostvoice.rs` | working |
| End of speech by a pause, robust to short noises | `voicedesk::PauseDetector` | built 2026-11-07, simulated levels only |
| Keep listening (record again after each pause) | Voice tile, chat microphone | built 2026-11-07 |
| Wording corrected by the assistant (one-shot, no tools) | `dictfix` | built 2026-11-07, scripted tests only |
| The chat agent: tools, approvals, Agent mode, runs, memory | `agent.rs`, `assistrun.rs`, chat tile | working |
| Mixer and levels (the speaker and the microphone start at known levels every boot) | `audioout.rs`, `micdefaults.rs` | fixed 2026-11-07 |
| Camera/mic indicator rule: nothing records without a visible indicator and the person's click | Voice tile, privacy rules | in force |

The one rule that **changes**: today nothing records without a click. A wake phrase means the microphone is open to a *local* spotter that discards audio. That needs an explicit opt-in, a permanent visible indicator and a hardware-style mute (below).

## Modes (build in this order)

1. **Push to talk** (tap or key): record until the pause, answer aloud. No always-on microphone. Ships first; it is the whole conversation loop without the wake word.
2. **Hands-free session**: after an answer the assistant keeps listening for a **follow-up window** (default 8 s, configurable) so a conversation does not need the wake phrase every time; ends on silence, on "that's all", or on Stop.
3. **Wake phrase "My IYAKKI"** (opt-in, per person, off by default): a local spotter listens continuously; on a hit it plays a short chime, shows the listening indicator and starts mode 2. The phrase is configurable (the assistant's name).

## The loop

```
idle -> [wake | key | tap] -> listening (PauseDetector ends it) -> STT -> route -> chat agent
      -> answer text -> speech (TTS) with barge-in -> follow-up window -> idle
```

* **Route**: a small, cheap decision in front of the agent: control words handled locally and instantly ("stop", "cancel", "louder", "mute", "that's all", "repeat"); everything else goes to the chat agent. No second model for routing; it is a word list plus the agent.
* **The chat agent in a voice session**: the same agent, tools and approvals as the chat, with a **voice session** system addition: short spoken answers (one to three sentences unless asked), no markdown, numbers and names read plainly, long results summarised aloud and shown on screen in full, ask a clarifying question instead of guessing when a tool needs a choice. The conversation joins the person's chat history (one thread, marked "voice") so nothing is lost when they switch to the keyboard.
* **Approvals by voice**: low-risk tools run as in chat. A tool that asks the person (writes, installs, deletes, sends, anything the hard-ask list names) **shows the normal approval card and the assistant says what it wants to do; the person answers by tapping or by saying "yes, do it"/"no"**; for the risky set (those on the `hard_ask` list) a spoken yes is **not enough**: it needs a tap or a key, so a TV or a passer-by cannot approve a deletion. This is a hard rule, written as a test.
* **Barge-in**: speaking over the answer stops it (the speech stops at the next word boundary, the new utterance is captured). Echo of the assistant's own voice must not trigger it: the microphone level while the assistant speaks is compared with what it is playing (the mixer knows the output level), and capture is ducked or gated while speaking until a real echo canceller exists (see Risks).
* **Autopilot and long work**: "My IYAKKI, build this and tell me when it is done" uses the pending-work ledger ([`AGENT-WAIT-PLAN.md`](AGENT-WAIT-PLAN.md)): the assistant answers "I'll tell you when it's done" **only when a ticket exists**, and the waker speaks the result later (a chime and a short sentence), never mid-conversation without a gap.

## The wake phrase: how to do it without a licence trap

The owner's rules apply: no lawyer, so choose routes that avoid licence questions; prefer our own code; permissive licences only; no vendor model shipped without clear terms.

| Option | Verdict |
|---|---|
| A commercial keyword engine (Porcupine and similar) | **No**: commercial licence, custom phrases tied to an account. |
| An open spotter with pre-trained models (openWakeWord and similar) | **Careful**: the code may be permissive but the shipped models and their training data may carry non-commercial terms; do not ship them without reading each model's terms. Use only if a model's terms are clearly permissive. |
| **In-house spotter (recommended)**: a small network (a few hundred KB) on log-mel features that answers "is this the phrase", trained by us on **synthetic speech from our own TTS voices plus the owner's own recordings, with noise and room mixes**, run in pure Rust | Our code, our data, no licence question. Needs a training script (kept in the repository, run on the dev PC or the k3s pipeline), an evaluation set and a threshold per person. Quality depends on data; start with the phrase "My IYAKKI" for the owner's voice and a few TTS voices, measure false accepts per hour. |
| Whisper (tiny) on every window with a phrase match | **Fallback only**: works with nothing new, but costs CPU all day and adds seconds of delay; acceptable as an energy-gated second check after the in-house spotter fires, to cut false wakes. |

Whatever the spotter: **it runs locally, keeps no audio (a ring buffer of a few seconds in memory, overwritten), and the audio of a false wake is dropped, not sent anywhere**. The second-stage check (Whisper-tiny on the last two seconds) confirms the phrase before the chime. Optional later: **speaker verification** (it answers only to the enrolled voice) with a local embedding model; not needed for v1 and it must never be the only security (the approvals rule above is).

## Privacy, safety and the visible indicator

* **Opt-in per person**, off by default; turning it on explains plainly what listens, what is kept (nothing), and where audio goes (local models: nowhere; a remote speech model only with the existing per-destination consent).
* **A permanent indicator** while the always-on spotter is armed (a small mic glyph in the top bar, a different, clearly "recording" state while a request is being captured) and a **mute** that stops the spotter at once (one tap, a key, and a state that survives a restart until turned on again). The tile never claims it is off when it is on.
* **No audio on disk, ever**: the existing "the sound is kept in memory only" promise stays; a test asserts no audio file is written (as the Voice tile test does).
* **Not for locked screens by default**: while the vault is locked or nobody is signed in the assistant only answers harmless questions (time, weather, music), never tool calls that touch data.
* **Children and shared rooms**: a per-person setting; the assistant never acts for a person who is not signed in.
* **Everything reachable by chat** (owner rule): `voice_assistant_status`, `voice_assistant_set` (asks every time; arming the always-on microphone is a hard-ask), plus the settings keys.

## Where it lives

* **A new service in the agent crate** (`voiceassist.rs`: the state machine, the follow-up window, barge-in, the routing, the spoken-session prompt) with a trait for audio in/out and the chat agent, so tests drive it with fake audio and a scripted model.
* **A new small crate for the spotter** (`iyakki-wake`: features, the network, the training script, the evaluation set, tests) so it compiles and tests alone (fast loop, see [`COMPILE-SPEED-PLAN.md`](COMPILE-SPEED-PLAN.md)).
* **UI**: the top-bar mic glyph and a small "listening / thinking / speaking" overlay on every tile; the Voice tile gets an "Assistant" tab (settings, the phrase, the follow-up window, test the wake phrase, the live level); the chat tile marks voice turns. Built on the tile ABI later; native first.
* **Cloud mode**: the device may be remote; the browser captures the microphone and plays the answer (WebRTC or a WebSocket stream), the spotter then runs in the browser or is replaced by push to talk. Design the audio in/out trait for a stream source from day one.

## Hardware and platform facts to respect

* The T2 Macs' built-in microphone is not supported (the driver can hang the computer); the assistant uses a USB microphone there and says so.
* Laptops with digital microphones (Intel SOF, AMD ACP) may have no capture device at all without a topology package; the Voice page already says so in words.
* Always-on capture costs CPU and, on laptops, battery: the spotter must stay under a few percent of one core, and the assistant pauses it on battery saver, in suspend and while a call or recording uses the microphone.
* One microphone owner at a time (the existing rule: the recorder, dictation and the assistant never run together).

## Latency budget (local models, a laptop with a GPU)

| Step | Target |
|---|---|
| Wake phrase heard to chime | under 300 ms |
| End of speech to transcript | under 700 ms (Whisper tiny/base on the GPU or CPU) |
| Transcript to first spoken word | under 1.5 s (the agent streams; the first sentence goes to TTS as soon as it ends) |
| Barge-in to silence | under 200 ms |

Measure each on the Lenovo; if a step misses, say so and show the real number.

## Build order

1. **Voice session in the chat agent** (system addition, spoken style, one thread with a voice mark) and the **state machine** with fake audio and a scripted model (tests: the loop, follow-up window, control words, barge-in, approvals-by-voice rule, no audio on disk).
2. **Push-to-talk assistant** on the Lenovo: tap or key, spoken answers, the follow-up window; real numbers for the latency table.
3. **Indicator, mute, opt-in and the chat tools**; the Assistant tab in Settings -> Voice.
4. **`iyakki-wake` spotter**: data generation and training script, evaluation (false accepts per hour on recorded room noise and TV speech, misses on the owner's voice), integration behind the opt-in; second-stage Whisper-tiny confirm.
5. **Echo handling** beyond gating (a simple adaptive echo canceller against the mixer's output reference), tested with recordings.
6. **Waker integration** (results spoken later), cloud-mode audio streams, speaker verification (optional).

## Decisions for the owner

1. **The phrase and the name**: "My IYAKKI" as the default phrase, and the assistant's name configurable per person. Is that right, or should the phrase be only "IYAKKI"? (A longer phrase makes false wakes rarer.)
2. **Always-on by default?** Recommended: off by default, push to talk first, the wake phrase only after the person opts in and sees the indicator.
3. **Spoken approvals**: recommended as above (a spoken yes for ordinary asks, a tap for the hard-ask list). Say if you want a stricter rule (taps only).
4. **Who trains the spotter**: the owner records the phrase (about 50 clean takes in a few rooms) plus our synthetic voices; or synthetic only to start. Recommended: both.

## What this does not solve (said plainly)

* A good wake phrase from synthetic data is not guaranteed; the first spotter may miss often or wake on TV speech, so the opt-in comes with a visible miss/false-wake counter and a sensitivity setting.
* Echo cancellation is hard; until it exists, barge-in works best with headphones or a low speaker volume, and the plan says so in the settings text.
* Speech recognition quality for names, accents and Indian languages depends on the Whisper model chosen; the dictation wording fix helps in the Voice tile, but the assistant must not act on a transcript it is unsure of: low-confidence or very short transcripts get "did you say ...?" before any tool runs.

## First slice: what exists (2026-11-08)

Owner decisions applied: push to talk first, wake phrase later, a spoken yes for ordinary asks, the hard-ask list always needs a tap or key.

| Piece | Where |
|---|---|
| State machine (idle, listening, speech to text, route, agent, speech with barge-in, follow-up window, approvals, "did you say") with traits for microphone, speech to text, speaker, agent and the thread log | `crates/iyakki-agent/src/voiceassist.rs`, tests with fake audio and a scripted model in `voiceassist_tests.rs` |
| Voice-session prompt (short spoken answers, no markdown, screen for the long ones) | `voiceassist::SESSION_PROMPT`, added to the system prompt by `RunAgent` |
| Real ports and the worker thread (recorder, Voice-settings speech engine, shared speaker, chat agent run, one chat session titled "Voice conversation") | `voiceassist_ports.rs` |
| Voice tile, fifth tab ASSISTANT (F5): opt-in, Talk/Stop, mute, red dot only while the microphone is open, approval and "did you say" cards | `crates/iyakki-shell/src/apps/voice_mine.rs` |
| Chat tools `voice_assistant_status` (free) and `voice_assistant_set` (asks; on the hard-ask list) | `voiceassisttools.rs`, wired in `chat.rs` |
| Settings keys `voice.my_iyakki`, `voice.my_iyakki_muted`, `voice.my_iyakki_follow_ms` (also `iyakki dev config`) | `voice.rs` (`VoiceSettings`), `devmode/config.rs` |

Rules as built (each is a test): control words never reach the model; a transcript with low confidence or a single short word is read back ("Did you say ...?") and nothing runs until a yes (voice, tap or key); a spoken yes approves an ordinary ask only; for the hard-ask list (`agent::hard_ask` plus a conservative name list, `voiceassist::is_hard`) a spoken yes is refused with "needs a tap or a key" and only `approve` (tap or key) approves, while a spoken no always denies; silence is never a yes; muting closes the microphone at once and survives a restart; the machine and its ports touch no file (a test reads the source for file calls).

Verified only with fakes: there is no microphone, speaker or speech engine in the build environment. Unverified on hardware: the latency table, the echo gate (the speaker level is a fixed estimate of 0.35 while speaking until the mixer reports its output level), barge-in with real echo, and speech-engine confidence (the engines in use give none, so only the short-transcript rule fires). The tab's agent has the base tools plus the voice tools; the chat tile's full tool stack (built inside `chat.rs`) is not shared yet, so some tools the chat has are not offered by voice. The voice thread is saved as a chat session titled "Voice conversation"; the chat does not mark its turns yet. Not started: the wake-phrase spotter, echo cancellation, the top-bar mic glyph, results spoken later by the waker, cloud-mode audio streams.

To try it on the Lenovo: see the steps in the report that came with this change (turn on in Voice -> ASSISTANT, tap Talk, speak, listen; try "stop", "louder", "that's all"; ask for something that asks, e.g. "set the follow-up to 5 seconds", and answer by voice, then ask for something on the hard-ask list and see that only the tap approves).
