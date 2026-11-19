//! Types moved out of `iyakki_agent::accessibilitytools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nACCESSIBILITY: accessibility_status shows the screen reader, high contrast, text size, large pointer, focus ring, reduced motion, sticky keys and slow keys. accessibility_set changes them and always asks the person first. Never claim a screen reader works because it is switched on: it also needs a reading voice (Settings -> Voice), and you cannot hear it. Super+Alt+S toggles the reader from any screen, including sign-in.";
