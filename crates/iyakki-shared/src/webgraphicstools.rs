//! Types moved out of `iyakki_agent::webgraphicstools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nBROWSER GRAPHICS: browser_graphics_get shows how the browser engine draws (automatic, software or hardware) and why. browser_graphics_set changes it (automatic, software, hardware, or try_hardware_again after an automatic fall back) and asks the person every time. Use it when the computer froze right after the browser started, or the person asks for the safe mode. A change applies the next time the browser engine starts.";
