# NVIDIA NIM: which models a key can use

Read on 2026-11-15 from public pages only (no account details sent). Owner report: the NVIDIA provider listed models the person's key cannot use.

## What the public pages say

- `https://docs.api.nvidia.com/nim/reference/llm-apis` documents `POST /v1/chat/completions` at `https://integrate.api.nvidia.com`. It does not document `GET /v1/models`, the error for a model the account cannot use, or the rate limit.
- NVIDIA developer forum threads (for example `https://forums.developer.nvidia.com/t/function-not-found-for-account/357670`, and many titled "Request to enable Public API Endpoints - 404 Function not found") show that `GET /v1/models` lists models that a chat call then refuses. The refusal is:
  - HTTP status **404**, body `{"status":404,"title":"Not Found","detail":"Function '<function id>': Not found for account '<account id>'"}`.
  - Staff and users link it to the account missing the "Public API Endpoints" permission, or to a model that is disabled; the same key works for other models.
  - Some newer models hang instead of answering (not handled: the normal time-out applies).
- Rate limit: forum threads from many users report about **40 requests per minute** on a free build.nvidia.com key and HTTP **429** when it is exceeded. NVIDIA does not publish this in the API reference; the figure is already in `providers::NVIDIA_STEPS`.

These are forum reports, not NVIDIA documentation; the code therefore treats only the combination of status 404 (or 403) and the words "Not found for account" in the body as "this key cannot use this model". A bare 404 or 403 proves nothing about the model.

## What IYAKKI does (`iyakki-agent::modelavail`, `providers.rs`)

1. **Listing**: `list_models` for an NVIDIA provider drops ids that are clearly not chat models (a small word table `NOT_CHAT`: embed, rerank, guard, safety, parse, retriev, clip, whisper, diffusion ...) and puts `nvidia/` (Nemotron) models first.
2. **Learned availability**: a chat or test call that fails the 404 way is remembered for 24 hours in `model-availability.json` next to the other provider state (key: salted hash of the key, provider id, model id; value: time). No key is stored. A success clears the entry. The model picker hides remembered models and says "N models are hidden because your key cannot use them (show)".
3. **Check my models** (Settings, Models, key K on an NVIDIA provider; chat tool `models_check`, hard-ask): a one-token request to each chat model, one after the other, 1.6 s apart (about 40 a minute), the chosen model first, at most 40 (the tool takes `max`, 1 to 200), stopping at the first 429 and saying so.
4. **Other providers**: OpenAI, OpenRouter and the rest answer for every model they list (or list only what the key can use), so nothing is tracked for them. Not done on purpose: a probe costs money there.

Not done: ordering the probe by "newest" (the listing carries no date we parse; the chosen model goes first, then NVIDIA's own).
