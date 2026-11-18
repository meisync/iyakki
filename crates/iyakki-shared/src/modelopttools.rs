//! Types moved out of `iyakki_agent::modelopttools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nMODELS: models_list shows the providers and the chosen models (never a key). model_options_get reads the request options of a model (max_tokens, temperature, top_p, reasoning_budget). model_options_set changes them and provider_models_fetch asks a provider for its model names with the person's key; models_check tries each NVIDIA model with a one-token request to find the ones the key can use (it spends the rate limit); all three ask the person every time. Never ask for or type an API key in the chat.";
