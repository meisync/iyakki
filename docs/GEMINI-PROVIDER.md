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
