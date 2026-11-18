# Google Gemini (experimental, optional)

Google Gemini can be used as a cloud provider for two things, each only when you choose it:

* **Speech-to-text.** Settings -> Voice then uses Gemini instead of the engine on this computer.
* **The assistant itself.** The chat model is a Gemini model; the assistant's tools still act on this computer, with the same approvals.

It is **experimental and off by default**. The on-device engines stay the default. Nothing is sent to Google unless you chose Gemini for that job *and* answered Yes on the consent card.

## What is sent

| Use | What goes to Google |
|---|---|
| Speech-to-text | The recording (WAV, inline in one request, at most 15 MB; a longer one is refused with a message), a plain instruction to transcribe it word for word, and the language hint if you set one. |
| The assistant | Your messages, the earlier turns of the chat, the tool descriptions, and the **results of the tools** the assistant ran (file text, web pages, command output) so that it can answer. |

Nothing else is sent: not when Gemini is not the chosen speech engine or chat model, not at start-up, not in the background. The key travels in the `x-goog-api-key` header (the chat endpoint also takes it as a Bearer header), never in an address, and is removed from every error text and log line.

## The consent card

The first time, and whenever you switch the tier, a card asks **"Send audio and text to Google?"** with **No** selected. It says, according to Google's published terms (check them yourself):

* on the **free tier**, Google may use what you send to improve its products, and people may review it;
* with your **own paid key**, what you send is not used to train its models.

Your answer is kept for you and for this computer, with the date (`gemini.json` in the data folder). It stays until you take it back (Settings -> Models, key `C`, or the chat tool `gemini_consent_revoke`). The assistant can read the state and take your yes back, but it can never give the yes: there is no tool for it. Another person on the same computer has to answer for themselves. Asking again after a tier change is automatic.

## Limits of the free tier

The free tier has small limits per minute and per day, and Google changes them. When Google answers "too many requests", IYAKKI waits as long as Google asks (at most 20 seconds, at most twice), then says that the free limit is reached and suggests waiting (a few minutes, or until tomorrow for the day's limit) or using your own paid key. The tier setting cannot be detected from the key; it only changes the notice on the card and this advice.

## Get a key

1. Open aistudio.google.com in a browser and sign in with a Google account.
2. Choose "Get API key" and create one. The free tier needs no card.
3. In IYAKKI open Settings -> Models and press `g` (administrators). Paste the key in the first field, choose the tier, the model names (defaults `gemini-2.5-flash`; they are plain text you can change) and whether the assistant and speech-to-text use Gemini.
4. Answer the consent card. Yes sends one tiny harmless test request ("Reply with the single word: OK") to check the key.

The key is stored in the system vault. You type it; the assistant cannot. To stop using Gemini: press `g` and choose "no" for both uses, or ask the assistant to run `gemini_set` with `enabled` false (it asks first and puts your earlier chat model and speech engine back).

## In chat

| Tool | Asks? | What it does |
|---|---|---|
| `gemini_status` | no | On or off, key stored or not (never the key), tier, models, what uses it, the state of your consent. |
| `gemini_set` | every time | Tier, on or off, model names, whether chat and speech use it. Hard-ask: never covered by a session grant. |
| `gemini_consent_revoke` | every time | Takes your yes back. |

## How it works inside

* `crates/iyakki-agent/src/gemini.rs`: the consent file, the one gate (`GuardedUpstream` and `GuardedStreamer` wrap every transport of the services, so no code path can reach Google without the yes), speech-to-text (`generateContent`, inline data), the error words, the 429 rule, and the settings change.
* Speech-to-text is the engine `SpeechEngine::Gemini` in `voice.stt`.
* The chat uses the provider `google-gemini` (OpenAI-compatible, `https://generativelanguage.googleapis.com/v1beta/openai`), through the ordinary provider code, tool calls included.
* Tests use a fake transport only. A live test (`gemini::tests::live_gemini_...`, ignored by default) reads a key from the mode-600 file named by `IYAKKI_LIVE_GEMINI_KEY_FILE`.

## Known issues (KI-460)

* Not tried with a real key or against Google: every test uses a fake transport. The OpenAI-compatible chat endpoint and the way Gemini accepts WAV audio are as Google documents them, not verified here.
* Free-tier data use and the review by people are Google's published terms, which can change; the card says so and asks you to check.
* The 429 advice for the chat shows both tiers' words, because the chat error does not know the tier.
* If a recording is longer than 15 MB it is refused, not split.

## Live voice assistant (EXPERIMENTAL, off by default; alpha.4, slice 1)

A live voice session with Gemini: one streaming WebSocket, the microphone in, Gemini's answer out **as text**, read aloud by the **local voice** ("Gemini thinks, IYAKKI speaks"). Design: [development/GEMINI-VOICE-DESIGN.md](development/GEMINI-VOICE-DESIGN.md). Code: `crates/iyakki-agent/src/geminilive.rs` (the session), `geminilive_ws.rs` (WebSocket over TLS, written here, no new dependency), `settingstools_geminivoice.rs` (chat tools).

**What leaves the computer:** the microphone, from the moment a session starts until it ends, and (slice 2, not built) the results of the tools Gemini asks for. It has its **own yes** (a yes to speech-to-text is not a yes to an open microphone), asked again after a tier change, revocable (`gemini_consent_revoke` takes back both). The existing consent gate stands in front of the connection and of every audio chunk; taking the yes back closes the microphone within half a second. A red sign shows while it is open (it uses the one red band of `clientmedia::indicator`). Nothing is recorded on this computer; only the day's count of microphone seconds is kept.

**Limits you set:** session length (default 600 s), quiet time (30 s), a daily microphone cap (1800 s), the spoken-reply cap (600 characters; the rest stays on the screen). Google's own limit is said plainly when it is reached. When it cannot start (off, no yes, no key, no network, a limit) the local voice answers and one line says why.

**Chat tools:** `gemini_voice_status` (free), `gemini_voice_set` (asks every time; on or off, start key name, model, the limits), `gemini_voice_stop` (free; closes the microphone at once). The assistant cannot type the key or give the consent.

### What Google's documentation said (read 2026-11-15, public pages only)
* Live API, WebSocket: `https://aistudio.google.com/docs/live-api/get-started-websocket` (moved from ai.google.dev), reference `https://aistudio.google.com/api-reference/live`, capabilities `https://aistudio.google.com/docs/live-api/capabilities`, session limits `https://aistudio.google.com/docs/live-api/session-management`, models `https://aistudio.google.com/docs/models`, the full text `https://aistudio.google.com/llms-full.txt`. The Live API is marked preview.
* Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key=API_KEY` (v1beta only on those pages; ephemeral tokens use `BidiGenerateContentConstrained` with `access_token`, not used here).
* First message `{"setup": {"model": "models/<name>", ...}}`, then wait for `setupComplete`. Messages: `realtimeInput.audio {data (base64), mimeType "audio/pcm;rate=16000"}`, `realtimeInput.text`, `realtimeInput.audioStreamEnd`, `toolResponse.functionResponses[{name,id,response}]`. Server: `setupComplete`, `serverContent {modelTurn.parts[].inlineData, inputTranscription.text, outputTranscription.text, turnComplete, interrupted, generationComplete}`, `toolCall.functionCalls`, `goAway.timeLeft`, `usageMetadata`, `sessionResumptionUpdate`.
* Audio in: raw 16-bit little-endian PCM, 16 kHz. Audio out: 24 kHz (`audio/pcm;rate=24000`).
* The native audio models "only support AUDIO response modality", so IYAKKI asks for AUDIO, turns on `outputAudioTranscription`/`inputAudioTranscription`, takes the answer text from the transcription and throws the audio away. Model names on the docs (the default `gemini-3.8-live` is the name the docs' own WebSocket example uses, `MODEL_NAME = "gemini-3.8-live"`, and the models page says "Gemini 3.8 Live ... `gemini-3.8-live` ... Default Live API model for most low-latency voice agent experiences without reasoning delays"; editable), `gemini-3.8-live-extended-thinking`, `gemini-3.1-flash-live-preview`, `gemini-2.5-flash-native-audio-preview-12-2025`.
* Limits: audio-only session 15 minutes, audio+video 2 minutes without compression, a connection lives about 10 minutes, `goAway` warns before it closes, session resumption handles last 2 hours; context window 128k tokens (native audio) or 32k. The pages list no close codes and no per-minute limits; an HTTP 429 `RESOURCE_EXHAUSTED` is the general rate-limit answer (`https://aistudio.google.com/llms-full.txt`, errors table).
* Settled offline (slice 2, 2026-11-15): the reference's own example of the setup message (`bidiGenerateContentSetup` in llms-full.txt) has `generationConfig.responseModalities`, and `tools`, `realtimeInputConfig`, `inputAudioTranscription` and `outputAudioTranscription` as siblings of `generationConfig`. That is what IYAKKI sends. The get-started page puts `responseModalities` at the top of `setup`, and the translate page puts the transcription switches inside `generationConfig`; those two pages disagree with the reference, so the reference was followed. A real run still has to confirm that Google accepts it.

### Known issues (slice 1)
* **Fake server only.** Every test talks to an in-process fake that speaks the documented shapes. Nothing was run against Google (no key), no microphone, no speaker. The TLS path of the WebSocket is not exercised by a test (the plain path is).
* No Settings page, no Super+V key, no microphone feed yet (the shell apps folder is being split; slices 2 to 5 are in the design). The chat tools exist in the agent crate and are registered in the shell chat after the split. Until the page exists no person can answer the consent card, so a session cannot start.
* Tool calls from Gemini are refused with a fixed text (no result leaves) until slice 2.

### Slice 2: function calls (tools) and the wake phrase hand-off
Code: `geminilive_tools.rs` (catalogue, router, dispatcher), `geminilive_wake.rs` (hand-off). Tests use fakes only.

* **Same tools as the chat.** The function declarations are built from `Tools::defs()` of the account (the chat's own set, role-gated, skills included; descriptions cut to 300 characters; the schema reduced to the OpenAPI subset Google documents for `parameters`). Calls run through the same wrappers in the chat's order: unknown name, policy refusal, safe tools run, an earlier grant, Auto mode (never for the always-ask list), else the card is shown on the screen. The approval comes only from the person's press on that card (the `Screen` port the shell fills); nothing reads what was said, so a spoken "yes" approves nothing. While the card waits the model is told "waiting for the person on the screen" and the session does not count as idle; the decided result follows as a user text turn marked as a tool result (the docs do not say a call can be answered twice). A denial or a 60-second silence runs nothing and the model is told so. A tool that always asks (hard-ask) is refused when nobody is at the screen. Results are cut for speaking (whole lines and the count of the rest; the whole text goes to the screen). Every audit line starts with `voice.`.
* **What the docs say about limits** (https://aistudio.google.com/docs/live-api/tools, read 2026-11-15): "The document does not state any maximum number or size of function declarations or tools, and it gives no recommended number"; function declarations are "defined as part of the session configuration" and changing them mid-session is not covered; scheduling values `INTERRUPT`, `WHEN_IDLE`, `SILENT` and `behavior: NON_BLOCKING` exist (async calls: supported on `gemini-3.8-live`, required on the extended-thinking model, not on `gemini-3.1-flash-live-preview`). `toolCallCancellation` has `ids[]` (reference). The context window is 128k tokens for native audio models, and declarations count toward it.
* **Tool router.** Because Google names no limit, the limit is our own budget (48 declarations, 48 000 bytes; `MAX_DECLARED`, `MAX_DECL_BYTES`). When the account's tools fit, all are declared. When they do not, the first set (radio, media, volume, calendar, alarm, weather, time and notes tools) is declared with two more functions: `find_tools(area)` lists matching tool names with one line each and `call_tool(name, arguments)` runs one through the same dispatcher. The log line says how many were declared and how many are routed. Excluded on purpose (the list may only shrink, a test counts it): `ask_user` (its answer is a chat button) and `vault_request` (a secret is never spoken or sent to Google).
* **Ratchet.** A test builds the chat's real tool set of the agent crate for a user and an administrator and fails if any tool is neither reachable by voice nor on the exclusion list. The shell's extra tools (Settings and browser tools) reach voice the same way because the dispatcher is built from whatever tool set the shell hands it.
* **Wake phrase hand-off.** The local spotter listens (its sound never leaves); only when it fires does a chime play and a session open. The sound that made it fire is wiped and never sent: the first audio Google gets is what comes after. Three opt-ins are read again as sound arrives: the wake phrase, the Gemini voice assistant and `wake_handoff` (off by default; chat tool argument of `gemini_voice_set`). The consent still applies: with no yes the one-line fallback comes and nothing is sent. A fake spotter and a fake network sink prove it.

### What only a real run can settle
Whether Google accepts the setup message as sent (`generationConfig` placement, the sibling `tools` and transcription fields, `parameters` schemas of the chat's tools: the Live API may reject some schema or a very long list; `parametersJsonSchema` is an alternative the reference lists), the real size at which the tool list fails, whether a late `realtimeInput.text` carrying a tool result is understood well by the model, whether `toolCall` arrives with the `id` field for every call, the real close codes and quota wording, the quality of the output transcription, TLS and the real endpoint, and everything heard on a speaker.

### Slice 4: Gemini's own voice, barge-in, echo cancellation, delay log (engine; no real run)
Code: `geminilive_audio.rs`, `echocancel.rs` (`StreamCleaner`, `resample_24k_to_16k`), `geminilive.rs` (`VoiceOut`, `LiveEvent::Audio`, the delay log).

* **`voice_out = local | gemini`** (default `local`; chat tool `gemini_voice_set`, which always asks). With `gemini` the model's audio (`inlineData`, `audio/pcm;rate=24000`, 16-bit PCM) is handed to `AudioOut`, cut into segments (the first after 300 ms, then about 800 ms each) and played through the SAME shared audio path as every other sound (`story::Sink`; the product's `SpeechSink` is `Services::play_wav_output` with the speech priority, so the mixer ducks the radio and media, or the play command is used). No second route. The audio is held in memory only, never written to disk (a test scans the source). With `local` the audio is thrown away on arrival, as before.
* **Speaker hold.** Before the first segment of a turn `soundhold::conflict_for_new_player` is asked; when another program or player holds the speaker a question goes to the screen (`AudioEvent::SpeakerHeld`) and nothing plays until the person answers (`answer_hold`); a no drops that turn's audio and the text stays on the screen. The playback announces itself to the same registry so other players can ask it to stop.
* **Barge-in.** The microphone chunk goes through the echo canceller against what was played; while the model speaks, audio goes to Google only when what is left is the person (level gate with 400 ms hold-over), so the speaker never retriggers the model. Loud for 120 ms counts as the person talking over it: the playing segment is stopped at once, the queue and buffer are dropped, the echo reference is cut at that instant, the rest of that answer already on its way is not played, and the audio that keeps flowing interrupts the model on Google's side (the setup says `realtimeInputConfig.activityHandling = START_OF_ACTIVITY_INTERRUPTS`, the documented default, `https://aistudio.google.com/api-reference/live`; `activityStart` is only allowed when automatic detection is off, so it is not used). `interrupted` from Google drops the half answer too.
* **Echo cancellation.** F-VOICE-36's NLMS canceller with the Geigel double-talk guard was reused (not rewritten) and given a streaming form: one filter that keeps what it learned from chunk to chunk against the reference of what was played (`Reference`, which already holds start times). It is always on in the Gemini voice mode, whatever the My IYAKKI echo setting says. For the first 1.5 s of the model's speech (the filter is still learning the room) the gate is twice as strict.
* **Delay log.** From the end of the person's speech (key released, or the gate saw the speech end) to the first audio and the first text of the answer; the last 20 turns are kept in memory and `gemini_voice_status` shows last and median.

Honest limits: tested with synthetic signals only (a generated far end through a generated three-reflection room, 0 to 20 dB echo, a tone as the person), not a real room, speaker or microphone; the sound card's real latency and the speaker's non-linearity are not in the tests. Segments leave small gaps at their joins because the shared path has no gapless stream for a growing sound (the own mixer is the real fix). Only the sound this session plays is in the reference, not other apps. The shell does not call any of it yet. Whether Google's `interrupted` arrives as soon as the audio keeps flowing, and the real first-audio delay, need a real run.

### Shell side (EXPERIMENTAL)
* **Settings -> Gemini voice (experimental)** (`apps/settings_geminivoice.rs`): on or off, the consent card, who speaks, the caps, the start key's name, the wake hand-off, the status with the delay and the limit meter, and Stop. The card is a choice with No first; only a key or tap on the page says Yes (no tool does); it is its own yes, kept per person and tier, and C takes it back (which also takes back the speech-to-text yes).
* **On every screen** (`iyakki-shell/src/shell_gemvoice.rs`): the start key (Super+V by default; set on the page or with `gemini_voice_set`; not used on the Mac profile for Command-only chords) starts a session and ends the open one, the Mic button does the same, Esc and Stop end it at once, Done talking hands the turn over. The shell sees no key release, so there is no hold-to-talk: a session is hands free (Gemini decides when you stopped) unless you press Done talking. A tool that asks, and the speaker question, appear as a card with Allow and Deny; only a press answers. The red sign is the shell's one red band (`clientmedia::indicator`).
* **Where the tools come from:** the person's base chat tools plus the Settings tools (`apps/gemvoice.rs`). The chat's own chain also holds tools that exist only inside a chat (the open project, flows, dashboard); they are not part of a voice session yet.
* **Wake hand-off:** when the existing wake listener hears the phrase and `wake_handoff` is on, a chime plays and a session opens. Nothing is sent before that point.
* **Not verified:** everything above runs against a fake live connection, fake microphone and fake speaker in the tests. Nothing was run with a real key, a real microphone, a real speaker or on a real screen; Super+V may collide with a desktop's own shortcut.
