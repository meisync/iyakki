# Gemini voice assistant (alpha.4 design)

Owner request 2026-11-15: a Gemini voice assistant, **manual (press to talk) or linked with the wake-up word**. It is item 3 of the alpha.4 plan, after the core<->apps split and the Looks.

## What exists (it stands on these)
- **My IYAKKI** ([MY-IYAKKI-PLAN.md](MY-IYAKKI-PLAN.md)): push to talk is built; the chat agent answers aloud and keeps listening for a follow-up; speech to text and text to speech run locally (Lemonade Whisper, Kokoro, host engines). The **wake phrase** is built as a local spotter, **experimental, off by default** (owner 2026-11-10).
- **Gemini, experimental and optional** (F-VOICE-41, F-ASSIST-52, [GEMINI-PROVIDER.md](../GEMINI-PROVIDER.md)): Gemini as the speech-to-text provider and as the chat/tool-calling provider (OpenAI-compatible endpoint). Key in the vault, a consent card per tier (kept per person and computer, revocable), chat tools `gemini_status` / `gemini_set` / `gemini_consent_revoke` (hard-ask; the assistant cannot type the key or give consent).

So today a Gemini voice turn is three separate round trips: record, send to Gemini for text, send text to the chat model, speak locally. This design adds **one live session**.

## The design

**A Gemini live voice session** as a new engine of My IYAKKI: one streaming connection (a WebSocket to Google's live API: microphone audio in as 16-bit PCM, the model's reply as audio and/or text out, with interruption support). The model can call IYAKKI's tools: the live session's function calls go through the **same tool wrappers, grants and approval cards as the chat** (an approval card still needs the person's own press; the model never approves). Details of the API (endpoint, model names, message shapes, audio formats, limits) must be read from Google's current documentation at implementation time: they change, and nothing here is copied from memory.

**Two ways to start it (both ends the same way):**
1. **Manual:** a key (default `Super+V`, changeable), the mic button in the top bar, or a tap on the assistant tile. Press to open, release (push to talk) or pause (hands-free) to hand the turn over; Esc ends the session. Allowed without the wake phrase ever being switched on.
2. **Wake phrase** ("My IYAKKI"): the existing **local** spotter listens (audio is discarded on the device, never sent); on a hit it plays a short chime, shows the indicator and opens the session. Nothing before the wake phrase leaves the computer. Still **experimental and off by default**, with its permanent visible "microphone is open" indicator.

**Who speaks (decision needed, see below):** (a) *Gemini speaks*: lowest delay, a Gemini voice; (b) *Gemini thinks, IYAKKI speaks*: Gemini returns text and the local voice (Kokoro) reads it: slower, but the voice is local and the same as the rest of the system. Build (b) first because it reuses the local pipeline and the highlight-while-reading, then add (a).

## Everything the chat can do, the voice assistant can do (owner 2026-11-15: "like play radio by voice, all our chat skills can be given to the Gemini assistant")

The live session is given **the same tool set as the chat agent for that account**: every chat tool and every installed skill (their function definitions are sent to Gemini as its function declarations; a call comes back and runs through the same wrappers, grants and approvals). "Play some jazz on the radio" is `radio_search` then `radio_play`; "pause", "volume down", "what is playing" are `radio_pause`, `radio_volume`, `radio_status`; the same for calendar, camera, files, flows, settings and the rest.

Rules that follow from that:
1. **One catalogue, no second list.** The voice tool list is built from the chat's tool list at session start (role-gated, plus the person's installed skills). A ratchet test (like the chat reachability table) fails if a chat tool is missing from what the voice session offers, so the voice assistant never lags the chat.
2. **Approvals are never spoken.** A tool that asks first shows the same card on the screen, the assistant says "I'm asking on the screen" and waits; **a spoken "yes" is not an approval** (only the person's own press answers, as for every approval card; the live model never approves). Hard-ask tools (changing the key, consent, drives, firewall...) are refused for voice-only use unless the person is present at the screen to press.
3. **A tool shared with the speaker asks the speaker question on the screen** when another app holds the speaker (the radio and sound hold rule), not by voice.
4. **Long answers are spoken short:** a tool result is shortened for speaking (a summary and the count), the full text goes to the chat transcript and the screen. Lists are read as "I found five stations; the first is...".
5. **Too many tools for one session:** the live API limits the size of the tool list. Send the tool groups the account actually has, and load a group on demand ("tool router": a small first set, the rest offered when the request names an area). Which tools were offered is shown in the session's log.
6. **Skills keep their own limits:** a skill's tools follow the skill's grants and the sandbox rules; a skill cannot widen what voice may do.
7. Everything a voice session does is in the same audit log as the chat, marked `voice`.

## Safety and privacy rules (all of them stay)
1. **Off by default, experimental, labelled so everywhere** (like the wake phrase and the Gemini provider). The consent card names exactly what leaves the computer: *microphone audio during the session*, and the tool results the model asks for.
2. **A visible indicator while the session is open** (the same red sign rule as cameras): nothing records without it and the person's start (a key, a tap, or the opt-in wake phrase).
3. **The key lives in the vault**; it is never in a log, a chat tool argument or a prompt; the assistant cannot set it or give consent (existing rule).
4. **Tools follow the chat's approvals:** hard-ask tools ask on the screen every time; Auto mode and Autopilot rules are the chat's; the live model gets only the tools the chat agent would get for that account (role gating).
5. **Limits:** session length cap, idle timeout, a cost/limit meter (the free-tier limit is said plainly, as for speech to text), and a hard stop key.
6. **Fallback:** no key, no consent, no network or a refused request: the local pipeline answers instead and says why in one line.
7. **Cloud mode:** off unless the tenant allows it (tenant config), like other egress.
8. **No recording is kept** unless the person turns on the existing transcript history; audio is never written to disk.

## Slices
1. **Manual session, Gemini thinks and IYAKKI speaks:** WebSocket client in-house (we have the TLS and WebSocket code), PCM in, text out, hand to the existing voice pipeline; consent, indicator, limits; Settings page and chat tools (parity rule). Tests: a fake live server; a real run needs a real key (owner supplies; never stored in the repo).
2. **Tool calls:** function calls routed through the chat tool wrappers and approvals; tests with the fake server for every approval path.
3. **Wake phrase hand-off:** the local spotter opens the session; tests that no audio leaves before the wake phrase (a fake network sink), the indicator, the opt-in.
4. **Gemini speaks, barge-in, echo cancellation** (F-VOICE-36 echo cancellation is at 60%): interrupt the model by talking; no self-triggering.
5. **Polish:** shortcut settings, languages, the cost meter, docs.

## Honest limits to expect
Nothing here can be proven without a real Gemini key, a real microphone and speaker, and a quiet room; the matrix rows will say "fake server only" until then. Delay depends on the network.

## Decided (owner)
- 2026-11-15: the Gemini voice assistant ships **as EXPERIMENTAL** (optional, off by default, labelled so on its Settings page, in the chat tools, in the docs and in the release notes), like the wake phrase and the Gemini provider.

- 2026-11-15: the default manual key is **Super+V** (changeable in Settings).

- 2026-11-15: **voice out starts with the local voice reading Gemini's text** (Gemini's own voice comes in slice 4); the **wake phrase stays experimental and local-only**, with this session as its first hand-off.

## Decisions still for the owner
None open.
