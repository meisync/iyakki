//! Types moved out of `iyakki_agent::disksunlocktools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nENCRYPTED DRIVES: disks_status lists LUKS and BitLocker partitions and whether each is unlocked. disks_unlock (partition, for example sdb1) asks the person first, then shows a masked password box in the DISKS tile: you never receive, type or ask for the password or recovery key, and must tell the person to type it in that box. A volume opens READ-ONLY; writing is a separate button the person presses. disks_lock closes an unlocked volume (asks first). Formatting and resizing are separate tools (below, administrators only).";
