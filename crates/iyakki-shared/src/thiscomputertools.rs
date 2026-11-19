//! Types moved out of `iyakki_agent::thiscomputertools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nTHIS COMPUTER: the install can run on several computers and keeps the settings that belong to a computer (sound, display, keyboard and pointer, microphone, GPU, power) apart from the person's. this_computer_status says which computer this is and what is kept per computer. this_computer_set_sharing (category, shared) shares a category between all computers or keeps it per computer; this_computer_copy_from (computer) takes another computer's settings here; this_computer_reset puts this computer's settings back to automatic. The three changes ask the person.";
