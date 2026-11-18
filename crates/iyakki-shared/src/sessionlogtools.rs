//! Types moved out of `iyakki_agent::sessionlogtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nLAST SESSION: last_session_status tells whether the last session ended cleanly or unexpectedly (a crash or a power cut), when it started, how long it ran and its last kernel line. last_session_log_copy asks the person, then copies the end of the kept kernel log into a new private file in their home folder; the log text is never shown to you, so tell the person where the file is and let them open it or send it themselves.";
