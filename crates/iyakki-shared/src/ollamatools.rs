//! Types moved out of `iyakki_agent::ollamatools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nOLLAMA: ollama_status says where Ollama runs (in the cluster, or on this machine's host) and its memory and CPU limits. ollama_install asks the person on a card: how=cluster installs it in the cluster; for an Ollama that runs on the host, how=move copies its models into the cluster (only when there is room) and how=fresh starts empty in the cluster and keeps the host models where they are. The host Ollama is stopped only after the cluster one answers and nothing is deleted from the host. To remove Ollama use quick_uninstall with item ollama (data kept unless asked).";
