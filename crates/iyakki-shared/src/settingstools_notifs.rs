//! Types moved out of `iyakki_agent::settingstools_notifs` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nNOTIFICATIONS (Settings -> Notifications): notification_filters lists every source with how much of it reaches the bell (show, quiet = inbox only, off = not kept); it is free. notification_filter_set changes one source and asks first. Information, new devices and finished jobs never add to the bell's number.";
