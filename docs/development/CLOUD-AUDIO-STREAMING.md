# Cloud audio streaming for My IYAKKI (F-VOICE-38)

Plan step 6 of docs/development/MY-IYAKKI-PLAN.md: when the assistant runs on a cloud machine (a tenant, see CLOUD-MULTITENANT-PLAN.md), the microphone and speaker are on the person's own device, so sound has to travel both ways.

## What is built (machine side, no network)

`iyakki-agent::audiostream`:

* **Frame**: 20 ms of 16 kHz mono signed 16-bit sound with a sequence number (wraps), a time in milliseconds and a checksum (CRC-32 of the whole frame). A receiver refuses a short, mis-numbered, wrong-version, oversized (more than 100 ms) or damaged frame by name and never plays it. A sender cannot make an oversized frame.
* **cut**: turns a recording into numbered frames; the last is padded with silence.
* **Jitter**: puts frames back in order. It holds a few frames before it starts (the delay, 60 ms at depth 3), drops repeats and frames that arrive after their place was given up, fills a lost frame with silence of the same length so the sound keeps its time, and refuses to hold memory for a wild jump in the numbers.

Tested with a shuffled, repeated, lost and wrapped sequence. Nothing has crossed a network.

## What is not built

* **Transport**: one TLS connection per direction (the person's device to the tenant for the microphone, the tenant to the device for the answer), over the same authenticated session the Connector already holds, never a separate open port. No UDP: a stalled TCP stream is better handled by dropping old frames than by retries; the sender drops the oldest frame when its queue passes 500 ms.
* **Consent**: the first use in a session shows a card that names the cloud machine and says that microphone sound will leave the device; it is answered by the person with a tap or key, never by voice. The permanent microphone sign and mute stay on the device and mute stops the stream at the source.
* **Compression**: none in the first version (256 kbit/s per direction is acceptable on a wired or good wireless link); Opus is a later step and would be a dependency decision for the owner.
* **Echo**: the device plays the assistant and records the person, so the echo canceller (F-VOICE-36) must run on the device, where both the played and the recorded sound are known. On the cloud machine it would have no reference.
* **Barge-in**: the device stops playing at once when the person speaks and tells the machine the position played, so the assistant knows what was heard.
* **Chat tool and Settings control**: `voice_cloud_stream_set` (asks every time) and a switch on the Voice page; neither exists until the transport does.

## Risks

Sound leaves the device: stated before the first use, off by default, audit line on every on and off (never sound). A hostile peer can send garbage: frames are checked and capped before they reach any decoder. Delay: the buffer adds about 60 ms plus the network; a link slower than that is reported as "too slow for voice" instead of stuttering.
