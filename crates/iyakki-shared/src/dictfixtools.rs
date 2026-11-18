//! Types moved out of `iyakki_agent::dictfixtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nDICTATION WORDING: dictation_wording_status says whether dictated text is corrected with the assistant as the person talks (Settings -> Voice, the Voice tile). dictation_wording_set (on = true or false) switches it; it asks the person. text_tidy fixes punctuation, capitals and mishearings in text you are given (text), keeping the words; it never sends text to another machine the person has not agreed to. text_revise (text, instruction) returns a revised text for the person's instruction (shorter, formal, translate to Tamil, ...) as a proposal only: it changes nothing, so show it to the person and let them apply it (the Voice tile's Assistant pane has Apply and Undo); the pane's open state is the setting voice.assistant_open.";
