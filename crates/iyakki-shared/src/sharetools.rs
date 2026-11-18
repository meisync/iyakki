//! Types moved out of `iyakki_agent::sharetools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSHARED FOLDERS: share_list shows the person's shared folders. share_create, share_start, share_stop and share_delete each ask the person on a card that says what becomes reachable and by whom. An administrator may share any folder of the disks; everyone else only folders inside their own home. For a password share first call vault_request for the password, then pass its entry name as password_entry; never ask for or type a password or a link secret in the chat, and never repeat a link's secret part (it is in Settings -> Shared folders). A running listing share offers a whole folder as one zip: a folder's address followed by ?zip. Prefer access `link` or `local`; `open` makes the files visible to everyone on the network.";
