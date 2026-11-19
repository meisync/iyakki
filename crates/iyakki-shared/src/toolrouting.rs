//! Types moved out of `iyakki_agent::toolrouting` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nTOOL ROUTING: tool_routing_get shows whether the model is offered every tool on every request (more tokens per request) or only the groups the chat needs. tool_routing_set switches that for this person only and asks every time; you cannot change the machine-wide setting.";
