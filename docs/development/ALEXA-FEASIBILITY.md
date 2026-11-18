# Alexa: what is possible for IYAKKI (feasibility, no code)

Written 2026-11-13 (lane M48) from public Amazon documentation found by web search on that day. Where a page could not be read in full, this says so. Nothing here is built and no network code for Alexa exists. Related: [`VOICE-ASSISTANT-DESIGN.md`](VOICE-ASSISTANT-DESIGN.md).

## Summary

* The thing that would make IYAKKI an "Alexa device" (Alexa listening to the person through IYAKKI's microphone and answering through its speaker) is the **Alexa Voice Service (AVS)**. **Its developer tools are, according to Amazon's own deprecation page, no longer generally available for new Alexa Built-in products;** Amazon points developers to its "Works with Alexa" program instead. Older AVS pages are still online and still describe the protocol, but they cannot be relied on as an open door. So the first thing needed is an answer from Amazon, not code.
* **Alexa Skills Kit / Smart Home** work the other way round: the person speaks to an Echo or the Alexa app, Alexa calls a skill or a smart-home endpoint that IYAKKI would expose. That is a way for Alexa to *control IYAKKI* by voice, **not** a voice front end inside IYAKKI.
* Recommendation: **not for alpha.3.** Keep Alexa as a documented, planned provider behind the voice seam, and ask Amazon (or find the current program page) before spending time. Local speech and Gemini already cover "voice that works the computer".

## Path A: Alexa Voice Service (AVS), Alexa as the voice front end

What the public docs describe (legacy pages, see availability caveat above):

| Piece | Detail |
|---|---|
| Account and registration | An Amazon developer account; an AVS **product** registered in the developer console (device type, security profile) which gives a client id and a product id. |
| Authorization | **Login with Amazon (LWA)**. For a device without a keyboard the docs describe **code-based linking**: the device gets a user code and verification URL, the person enters the code on a phone/PC, the device receives a refresh token and exchanges it for short-lived access tokens. |
| Transport | **HTTP/2** to the AVS endpoint with TLS. One long-lived **downchannel** stream (a GET) for directives pushed by Alexa; **events** are POSTs on separate streams; a ping keeps the connection alive; connection timeout guidance of at least 60 minutes. |
| Events | `SpeechRecognizer.Recognize` is the core event; it is a **multipart** request: a JSON part and an audio part. Other interfaces a minimal client must handle or declare: `System` (synchronize state, exception handling), `SpeechSynthesizer` (Speak directive, with the audio returned as an attachment), `Alexa.Capabilities` publishing, and optionally `AudioPlayer`, `Alerts` (timers), `Notifications`. Unsupported ones can be left out of the declared capabilities. |
| Audio in | `AUDIO_L16_RATE_16000_CHANNELS_1` (16 kHz, 16-bit mono PCM, little-endian), or Opus (added in SpeechRecognizer 2.0). |
| Audio out | Speak directives return audio attachments in the response (MP3 in the docs I could find; the decoder choice has to be checked against the current spec). |
| Wake word | Amazon requires **cloud-based wake word verification** for voice-initiated products. IYAKKI's own spotter is experimental (see `WAKE-PHRASE.md`), so a first AVS provider would be push to talk only. |
| SDK | The **AVS Device SDK** is a C++ SDK, Apache-2.0 per its repository (to be re-checked at the time). Using it conflicts with the "own code, no heavy dependency" rule; it would have to be built on the device like the AV add-on, or its protocol re-implemented. |
| Certification | Commercial devices go through Amazon's certification/"Alexa Built-in" review. A personal development device can run under the developer's own account without it, but **may not be shipped** to others as an Alexa product without Amazon's agreement. For an open-source operating system image this is the blocking point: shipping a client id for everyone is not possible, and each person would need their own developer account and product, which is not a consumer-friendly flow. |

### What the owner would have to do

1. Check with Amazon whether new AVS products are accepted today (the deprecation notice says general availability ended) and under which program ("Works with Alexa"?), and what the terms say for open-source software that is distributed to other people.
2. Create an Amazon developer account and, if allowed, register a product (a security profile with LWA).
3. Decide whether each person brings their own developer registration (privacy-clean, hard for non-technical people) or IYAKKI ships one registration (needs Amazon's agreement and certification).

### What IYAKKI code would need (estimate, in-house)

| Part | Size (order of magnitude) | Notes |
|---|---|---|
| LWA code-based linking (device code, polling, refresh, token storage in the vault) | 400 to 600 lines | TLS HTTP client already exists |
| HTTP/2 client with multiplexed streams, flow control, HPACK, a persistent downchannel | 2,000 to 3,500 lines if written in-house; none of the current HTTP code is HTTP/2 | the largest piece; a permissive-licence HTTP/2 library is the alternative, which the dependency rules need to approve |
| Multipart builder and parser (JSON + audio parts, boundaries) | 300 lines | `voice::multipart` builds multipart/form-data already |
| Directive handling: System, SpeechRecognizer (expect speech, stop capture), SpeechSynthesizer (play attachment, report started/finished), capabilities publish | 800 to 1,200 lines | state machine with fake server tests |
| Audio: 16 kHz LPCM capture exists; MP3 (or Opus) decode for Speak audio | MP3/Opus decoder is the other big piece if not already present | check `aacpipe`/audio code for what decoders exist |
| Provider glue behind the seam, consent card, settings, chat tools, docs, tests | 600 lines | |

Roughly **4,000 to 7,000 lines and several weeks** including a fake AVS server for tests. A caution: because Alexa does the understanding and answers itself, an AVS provider does **not** fit the speech-to-text trait. It replaces the whole turn (audio in, Alexa's spoken answer out), so it is a *session provider* beside the chat-agent session, and Alexa's answers would not use IYAKKI's tools or approval cards. That is a different product from "IYAKKI's assistant with a cloud ear".

### Privacy

Audio of every request goes to Amazon, and Amazon's account and retention rules apply to it. IYAKKI could not show or delete it. It would need a consent card, an off switch, and a plain label "this is Amazon's assistant, not IYAKKI's".

## Path B: Alexa Skills Kit / Smart Home (Alexa controls IYAKKI)

Alexa calls a **skill** (custom intents) or a **smart-home** endpoint (e.g. lights-style on/off, scenes) that must be reachable on the internet over HTTPS (an AWS Lambda or a web endpoint), linked with account linking (OAuth). The person talks to an Echo/phone. Needs: an Amazon developer account, a skill listing (testing in "development" stage is possible for the developer's own account without certification; public skills are certified), a public HTTPS endpoint and OAuth on the IYAKKI side. IYAKKI has a domain listener and cloud mode ideas, but this exposes the device to Amazon's cloud calls and needs care (each command would still need IYAKKI's approval rules). It does not add a voice front end to the IYAKKI screen and the person's audio goes to Amazon via the Echo. Possible later as "control IYAKKI from an Echo", still not for alpha.3.

## Recommendation

1. **Not in alpha.3.** The path that matches the request (AVS) may be closed to new products, needs the owner's Amazon registration and a decision on distribution, and is thousands of lines of protocol work.
2. **Next step costs nothing:** the owner (or a person with an Amazon developer account) checks the current AVS / Works with Alexa status and terms and writes the answer into this file.
3. If AVS is open to personal development devices: **post-alpha.3, an AVS "session provider" behind the voice seam**, push to talk only, per-person developer registration, clearly labelled as Amazon's assistant, built with a fake AVS server first. Reuse: vault for tokens, consent cards, the visible microphone sign, `voice_backends` listing (kind Cloud).
4. If AVS is closed: record that here, and offer Path B only if people ask for "control IYAKKI from an Echo".

## Sources and limits of this research

Web search results on 2026-11-13: Amazon's *Deprecated Features* page for AVS (developer.amazon.com/docs/alexa-voice-service/deprecated-features.html), the *Manage an HTTP/2 Connection with AVS* and *Create and Manage HTTP/2 Requests with AVS* pages, *SpeechRecognizer* and *Authorize an AVS Device* (code-based linking) pages, and *Wake Word Verification Requirements*. A direct fetch of the AVS "get started" page returned 404, so the registration and certification steps are described from search summaries and general knowledge of the program, not read in full. Some Amazon pages still say AVS is free and open; they appear stale next to the deprecation notice. The audio-out codec and the current license of the AVS Device SDK were not confirmed. Check each before relying on it.
