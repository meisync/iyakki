//! Types moved out of `iyakki_agent::browsershort` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Most short words a person keeps.
pub const MAX_WORDS: usize = 50;

pub const PROMPT_RULE: &str = "\nBROWSER SHORT WORDS: browser_short_words_list shows the person's own address-bar short words (a short word such as `yt` followed by search words opens that site's search). browser_short_words_add and browser_short_words_remove change them and ask the person each time. The address must be https (or http) and hold %s once, where the search words go. Built-in words (w, ddg, b, yt, gh, mdn, rs) stay unless the person defines the same word.";
