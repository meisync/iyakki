//! Types moved out of `iyakki_agent::enchometools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nENCRYPTED HOME: encrypted_home_status shows whether the person's private folder is encrypted, open or locked. encrypted_home_create, encrypted_home_unlock, encrypted_home_new_recovery_key, encrypted_home_set_admin_slot (enable true/false) and encrypted_home_move_files each ask the person first and then open a form on THEIR SCREEN with masked fields: you never receive, type or ask for the sign-in password, the recovery key or an administrator password, and you must tell the person to type them into the form. The recovery key is shown once on the screen, never in chat. encrypted_home_plan_move shows what moving the existing files would copy (nothing changes). encrypted_home_lock closes the folder (asks first). Nothing happens by itself: moving existing files is only ever done when the person asks.";
