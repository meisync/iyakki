//! Types moved out of `iyakki_agent::webgraphicstools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nBROWSER GRAPHICS: browser_graphics_get shows how the browser engine draws (automatic, software or hardware) and why. browser_graphics_set changes it (automatic, software, hardware, or try_hardware_again after an automatic fall back) and asks the person every time. Use it when the computer froze right after the browser started, or the person asks for the safe mode. A change applies the next time the browser engine starts.";

/// The assistant's rule for the graphics acceleration tools (F-BROX-25).
pub const ACCEL_RULE: &str = "\nBROWSER ACCELERATION: browser_accel_get shows which parts of the browser's graphics acceleration (compositing, rasterisation, video decoding, WebGL) are on, for the person or one site, and why a part is off. browser_accel_set turns it on or off for the person, for one site, or (administrators) for the whole computer, and asks each time. Use it when a site draws wrongly or the person asks to save power or to use the graphics card.";
