# Voice assistant design: a hands-free session that drives the assistant, with approvals on the person's side

Status: **design plus a small first slice (2026-11-13, lane M48).** Built now: the voice backend seam (`crates/iyakki-agent/src/voicebackend.rs`), the `voice_backends` tool, the `spoken_approvals` setting (off by default), the tools `voice_session_settings_status` and `voice_session_settings_set`, and the rule that voice approval is refused for the hard-ask list (tests with fakes). Everything else below is the agreed shape for later work. Builds on [`MY-IYAKKI-PLAN.md`](MY-IYAKKI-PLAN.md) (the push-to-talk session that exists) and [`WAKE-PHRASE.md`](WAKE-PHRASE.md) (the experimental wake phrase). Alexa is in [`ALEXA-FEASIBILITY.md`](ALEXA-FEASIBILITY.md).

## The owner's request in one sentence

A full voice assistant that can work the computer, always on the person's permission, using an on-device or a cloud speech provider the person chose.

## What exists and what this adds

The session already runs the loop `listening -> speech to text -> route -> chat agent (same tools, same approval cards) -> speech out -> follow-up window` (`voiceassist.rs`, ports in `voiceassist_ports.rs`, the Voice tile's Assistant tab). Its speech-to-text is whatever `voice.stt` says, else a Lemonade Whisper model (`voicedesk::stt_engine`). Hard-ask tools (`agent::hard_ask` plus a conservative name list, `voiceassist::is_hard`) already could not be approved by a spoken yes. What changes:

1. **Spoken approvals become a choice, off by default.** Until now a spoken "yes, do it" approved an ordinary ask. Now `voice.spoken_approvals` (Settings -> Voice, key `A`; `iyakki dev config voice.spoken_approvals`; chat tool `voice_session_settings_set`) must be on for that. Off: every approval card is answered by a tap or key (`Y` / `N`); a spoken **no** still denies, a spoken yes does nothing and the assistant says so. On: a spoken yes approves ordinary asks only.
2. **A named seam for speech-to-text** ([`VoiceBackend`](#the-backend-seam)) so a cloud provider (Gemini, lane M47) or a later Alexa provider is one more implementation, not a change to the session.
3. **`voice_backends`** (free chat tool): which backends exist, set up or not, where the sound goes, whether each asks for consent, which one `voice.stt` uses. Never a key.

## What a session looks like to the person

* **Before:** My IYAKKI is on (opt-in), not muted. Push to talk (tap, Space) or, only if the person turned the experimental wake phrase on, "My IYAKKI".
* **While the microphone is open:** the red recording sign (bottom left, on every screen; in the Voice tile the red dot) and the level bar. It is drawn exactly while the microphone is open, never otherwise.
* **What was heard:** the transcript of each request is shown ("heard: ...") before and while the assistant works. A short or unsure transcript is read back ("Did you say ...?") and nothing runs until a yes by tap, key or voice (a read-back is not an approval of an action).
* **Stop:** Esc/Stop on the tab, the word "stop" (handled locally, never sent to a model), and Mute (survives a restart). Stop closes the microphone and the speaker at once and cancels the running turn.
* **Approvals** appear **on screen** as the normal card ("IYAKKI wants to: ..."). The assistant also says what it wants. Answer by tap or key (Allow `Y`, Deny `N`). By voice only: "no" (always), and "yes" only when spoken approvals are on and the tool is not hard.
* **Long results** are summarised aloud and shown in full; nothing secret is read aloud (see threats).

## Approval rules (each one is a test)

| Case | Tap / key | Spoken yes | Spoken no |
|---|---|---|---|
| Ordinary ask, spoken approvals **off** (default) | approves | refused: "That one needs a tap or a key" | denies |
| Ordinary ask, spoken approvals **on** | approves | approves | denies |
| Hard-ask tool (any setting) | approves | **refused** | denies |
| `voice_session_settings_set` itself | approves | **refused** (it is on the hard-ask list; a voice cannot grant itself voice approval) | denies |
| Silence, an unclear answer twice | ordinary: denies; hard: the card waits for a tap | never a yes | |

Implementation: `PendingApproval { hard, voice_ok }` in `voiceassist.rs`, `voice_ok = !is_hard(tool) && Config::spoken_approvals`; `route_approval` approves by voice only when `voice_ok`. The setting is read by the worker about four times a second (`voiceassist_ports::Live`), so turning it off acts at once. It is a machine setting like every other `voice.*` key (administrators change it); a per-person store for voice settings does not exist yet and is not invented here.

## The backend seam

```
trait VoiceBackend { id(); label(); kind() -> OnDevice | Network | Cloud; needs_consent() (default: kind != OnDevice);
                     configured(); engine(); transcribe(wav, lang) -> Result<String,String> }
Registry::{register, get, list, selected(voice.stt), status(voice.stt), from_services}
```

* `from_services` registers two `EngineBackend`s over the existing `voice::transcribe`: `voice-settings` (the engine chosen in Settings -> Voice: a local command, or an OpenAI-style server on this computer, the network or the internet) and `lemonade` (the Whisper model). `kind` comes from `voicedesk::destination`.
* **Scope of the refactor, said plainly:** the running session (`ServicesStt`) still calls `voicedesk::transcribe` directly; only the Lemonade lookup was split out as `voicedesk::lemonade_stt_engine`. Moving the session onto `Registry::selected` is a follow-up, to be done together with the cloud provider so the consent check and the call sit in one place.
* **Plugging in Gemini (lane M47):** implement `VoiceBackend` with `kind() = Cloud` (so `needs_consent()` is true), make its engine what `voice.stt` stores, and `register` it in `from_services`. Its consent card must have been answered before `transcribe` is called; `voice_backends` shows it as "asks you before sound is sent". Automatic selection (`voice.stt` empty) never picks a cloud backend (test).

## What is sent where

| Backend | Sound goes to | Text goes to | Consent |
|---|---|---|---|
| On-device (local command such as whisper.cpp, Lemonade Whisper on this computer) | nowhere | nowhere | none |
| A speech server on the person's network | that machine | | existing per-destination consent (Voice tile card, `speech_server_allowed`) |
| Gemini (lane M47) | Google, only after M47's consent card, free tier by default with its limits or the person's own key | the transcript goes to the chat model the person already chose (a separate, existing consent) | consent card before first use; key in the vault, never shown |
| Alexa (post-alpha.3, see feasibility) | Amazon (audio and the Alexa answer) | | consent card; a different kind of provider (Alexa does the understanding and answers itself) |

Text-to-speech is a separate choice (`voice.tts`) with its own consent; the answer text goes to a cloud voice only after the person agreed.

## Limits

* No background listening unless the person turned on the **wake phrase**, which stays **experimental and off by default** and keeps its sign and mute. A cloud backend is **never** fed by the wake spotter: the spotter hears locally and discards; only a request after a hit (or a tap) is transcribed. Until the second-stage confirm exists (not built), a false wake would open a request window with a visible sign; the pause rule ends it and silence is dropped, not sent.
* One microphone owner at a time; not on a locked screen or without a signed-in person; browser sessions use the browser's own consent.
* Nothing is written to disk by the session (sound stays in memory; a test reads the source).
* Low-confidence or one-word transcripts never run a tool without a read-back.
* The assistant only has the tools the person's account has, with the same risk levels, Auto/Ask modes and per-person grants as the chat.

## Threats and what answers them

| Threat | Answer |
|---|---|
| **Voice injection** from a TV, a video, a web page playing sound, another person | Approvals need a tap/key by default; a spoken yes never approves hard-ask tools; the microphone opens only on a tap or the opt-in wake phrase; barge-in is gated against the assistant's own output; control words are local. Remaining risk: with spoken approvals ON, a played "yes, do it" could approve an ordinary ask: that is why it is off by default and the settings text says so. A speaker verification step is the later mitigation. |
| **Injection through content** (a web page or file read aloud contains instructions) | Unchanged chat rules: untrusted tool output cannot grant approvals; Auto mode never runs hard-ask tools. Reading aloud does not add trust. |
| **The assistant speaking secrets** | Never: vault values, keys and recovery codes are not available to the chat tools (`vault_request` is hard-ask and answered on screen), `voice_backends` and the status tools print no key reference, and spoken text goes through `speakable`/`spoken_form` which strip code and addresses. A test asserts that the backends status contains no key reference. A long result is summarised and shown, not read in full. |
| **Accidental approvals** (a cough, "yes" said to someone else) | Spoken approvals off by default; when on, only exact yes phrases (`is_yes`; "yes but delete everything" is not a yes); unclear twice denies; silence is never a yes; hard-ask cards wait for a tap. |
| **A cloud provider receives more than the person expects** | `needs_consent`, a consent card naming the destination, `voice_backends` status, audit lines for on/off changes (never sound), the wake spotter never feeds a cloud backend. |
| **Turning on the dangerous settings by voice** | `voice_assistant_set` and `voice_session_settings_set` are hard-ask: a card with a tap, never a spoken yes, never "always allow". |
| **Echo / the assistant hearing itself** | Capture gated while speaking until a real canceller exists (known limit, plain in the settings text). |

## Not verified

No real microphone, speaker, cloud provider or hardware was involved: everything above is covered by fakes (a scripted agent, a fake microphone, fake backends). Gemini behaviour belongs to lane M47.

## Build order from here

1. Move `ServicesStt` onto `Registry::selected` together with the Gemini backend and its consent card.
2. A `VoiceBackend`-style seam for the answer voice (TTS) if a cloud voice is added.
3. Real-hardware run of the latency table in `MY-IYAKKI-PLAN.md`.
4. Speaker verification before anyone considers spoken approvals for more than the lowest-risk asks.
