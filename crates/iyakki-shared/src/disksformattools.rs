//! Types moved out of `iyakki_agent::disksformattools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nFORMAT AND RESIZE: disks_format_plan (partition, and either filesystem [ext4, fat32, exfat, ntfs] with an optional label, or new_size) shows exactly what would happen and changes nothing. disks_format and disks_resize ask the person first (every time), then the DISKS tile shows the plan again and the person types the drive's or partition's name or label and waits out a 5 second countdown: you cannot type, confirm or skip that, so tell the person to do it on the screen. Always call disks_format_plan first and explain what will be erased. exFAT, NTFS and resizing need programs the person installs from CLI tools; if the plan says one is missing, say so. The system drive and anything mounted or unlocked are refused.";
