//! Types moved out of `iyakki_agent::lockscreentools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nLOCK SCREEN: lock_screen locks the person's screen at once (it asks nothing; their apps and runs keep going, their personal vault closes until they type their password at the lock screen). You cannot unlock it and must never ask for the password. lock_screen_status shows after how many idle minutes the screen locks by itself. lock_screen_set_auto changes that (0, 5, 10, 15, 30 or 60 minutes; 0 = never) and asks the person first. Guests have no password, so for them the screen cannot be locked.";
