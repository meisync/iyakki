//! Types moved out of `iyakki_agent::sharedvaulttools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSHARED VAULT: shared_vault_provider shows which cryptography keeps the cloud tenant's shared vault (built-in, which is not externally reviewed, or the person's own external vault). To change it call shared_vault_provider_set; it asks the person every time. Give the NAME of the vault secret that holds the external access token, never the token. Switching does not move existing secrets.";
